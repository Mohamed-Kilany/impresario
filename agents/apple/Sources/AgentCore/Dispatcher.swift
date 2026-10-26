import Foundation

/// Session-level command dispatcher (spec 7.2 rules 3–5): strict ordering, exactly
/// one result per command, immediate `Cancel` handling, heartbeats, and the
/// `SessionClosed` event. Free of gRPC types: the service feeds commands in and
/// serializes the emitted events onto the response stream.
///
/// The dispatcher is an actor, so admission, state changes and event emission
/// are serialized; command bodies run on detached tasks so a long command never
/// delays a cancel or a heartbeat.
public actor Dispatcher {
    public typealias Emitter = @Sendable (SessionEvent) async -> Void

    private let executor: CommandExecutor
    private let heartbeatIntervalMs: Int
    private let now: @Sendable () -> Int64

    private var emit: Emitter?
    private var outbox: [SessionEvent] = []
    private var flushing = false

    private var pending: [AgentCommand] = []
    private var started: Set<String> = []
    private var active: Active?
    private var draining = false
    private var inputClosed = false
    private var closedEmitted = false
    private var shutdownRequested = false
    private var heartbeatTask: Task<Void, Never>?
    private var heartbeatSeq = 0

    private struct Active {
        let id: String
        let task: Task<Void, Never>
    }

    public init(
        executor: CommandExecutor,
        heartbeatIntervalMs: Int = 5000,
        now: @escaping @Sendable () -> Int64 = { Int64(Date().timeIntervalSince1970 * 1000) }
    ) {
        self.executor = executor
        self.heartbeatIntervalMs = heartbeatIntervalMs
        self.now = now
    }

    // ------------------------------------------------------------------
    // lifecycle
    // ------------------------------------------------------------------

    /// Binds the event sink and starts heartbeats (`heartbeatIntervalMs <= 0`
    /// disables them, for tests).
    public func start(emit: @escaping Emitter) async {
        guard self.emit == nil else { return }
        self.emit = emit
        if heartbeatIntervalMs > 0 {
            let nanos = UInt64(heartbeatIntervalMs) * 1_000_000
            heartbeatTask = Task.detached { [weak self] in
                while !Task.isCancelled {
                    do {
                        try await Task.sleep(nanoseconds: nanos)
                    } catch {
                        return
                    }
                    guard let self else { return }
                    await self.emitHeartbeat()
                }
            }
        }
        await flush()
    }

    // ------------------------------------------------------------------
    // commands
    // ------------------------------------------------------------------

    /// Accepts one command from the session stream (spec 7.2 rules 3–5).
    public func enqueue(_ command: AgentCommand) async {
        guard !closedEmitted else { return }

        if command.id.isEmpty {
            await emitNow(.result(Self.protocolResult(id: "", message: "the command id is empty")))
            return
        }
        if case .cancel(let target) = command.body {
            await handleCancel(id: command.id, target: target)
            return
        }
        if started.contains(command.id) {
            await emitNow(
                .result(Self.protocolResult(id: command.id, message: "duplicate command id '\(command.id)'"))
            )
            return
        }
        if active != nil || draining {
            pending.append(command)
            return
        }
        start(command)
    }

    /// The master half-closed the request stream: finish the work already
    /// accepted, then emit `SessionClosed{client_closed}` (spec 7.2 rule 5).
    public func clientClosed() async {
        inputClosed = true
        if active == nil && pending.isEmpty && !closedEmitted {
            await emitClosed(.clientClosed)
        }
    }

    /// A command the bridge could not even parse (spec 7.2 rule 4: exactly one
    /// result per command, in arrival order). The id is remembered so a
    /// duplicate is still reported as a duplicate.
    public func reject(id: String, error: AgentError) async {
        guard !closedEmitted else { return }
        if !id.isEmpty {
            if started.contains(id) {
                await emitNow(
                    .result(Self.protocolResult(id: id, message: "duplicate command id '\(id)'"))
                )
                return
            }
            started.insert(id)
        }
        await emitNow(
            .result(CommandResultData(commandId: id, ok: false, durationMs: 0, error: error.report))
        )
    }

    /// The master called `Shutdown`: cancel the in-flight command, drop the
    /// queue, and emit `SessionClosed{shutdown}`.
    public func shutdown() async {
        shutdownRequested = true
        if let active {
            active.task.cancel()
        } else {
            await emitClosed(.shutdown)
        }
    }

    // ------------------------------------------------------------------
    // introspection (tests and diagnostics)
    // ------------------------------------------------------------------

    public var currentCommandId: String? { active?.id }
    public var pendingCount: Int { pending.count }
    public var isClosed: Bool { closedEmitted }

    // ------------------------------------------------------------------
    // internals
    // ------------------------------------------------------------------

    private func start(_ command: AgentCommand) {
        started.insert(command.id)
        let task = Task.detached { [executor] in
            let result = await executor.execute(command)
            await self.finish(result)
        }
        active = Active(id: command.id, task: task)
    }

    private func finish(_ result: CommandResultData) async {
        active = nil
        await emitNow(.result(result))
        if shutdownRequested {
            await emitClosed(.shutdown)
            return
        }
        await drainPending()
        if inputClosed && active == nil && pending.isEmpty && !closedEmitted {
            await emitClosed(.clientClosed)
        }
    }

    private func drainPending() async {
        guard !draining else { return }
        draining = true
        while active == nil && !closedEmitted, !pending.isEmpty {
            let next = pending.removeFirst()
            if started.contains(next.id) {
                await emitNow(
                    .result(
                        Self.protocolResult(id: next.id, message: "duplicate command id '\(next.id)'")
                    )
                )
                continue
            }
            start(next)
        }
        draining = false
    }

    private func handleCancel(id: String, target: String) async {
        guard let active, active.id == target else {
            await emitNow(
                .result(Self.protocolResult(id: id, message: "no in-flight command '\(target)' to cancel"))
            )
            return
        }
        active.task.cancel()
        await emitNow(.result(CommandResultData(commandId: id, ok: true, durationMs: 0)))
    }

    private func emitHeartbeat() async {
        guard !closedEmitted else { return }
        heartbeatSeq += 1
        await emitNow(
            .heartbeat(seq: heartbeatSeq, unixMs: now(), currentCommandId: active?.id ?? "")
        )
    }

    private func emitClosed(_ reason: SessionEvent.CloseReason) async {
        guard !closedEmitted else { return }
        closedEmitted = true
        heartbeatTask?.cancel()
        await emitNow(.closed(reason: reason.rawValue, error: nil))
    }

    private func emitNow(_ event: SessionEvent) async {
        outbox.append(event)
        await flush()
    }

    /// Serialized emission: the actor may re-enter while the sink awaits, so a
    /// single flusher drains the outbox in order (spec 7.2 rule 4).
    private func flush() async {
        guard emit != nil, !flushing else { return }
        flushing = true
        while !outbox.isEmpty, let emit {
            let event = outbox.removeFirst()
            await emit(event)
        }
        flushing = false
    }

    private static func protocolResult(id: String, message: String) -> CommandResultData {
        CommandResultData(
            commandId: id,
            ok: false,
            durationMs: 0,
            error: AgentError.protocolError(message).report
        )
    }
}
