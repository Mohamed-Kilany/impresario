import AgentCore
import Foundation
import GRPC
import ImpresarioWire

/// Serves the `Agent` gRPC service from an [`AgentState`] (spec 7.2).
///
/// Responsibilities: metadata checks (`x-impresario-token`,
/// `x-impresario-session-id`), protocol conversion, wiring the session stream
/// onto the [`Dispatcher`], and the out-of-band `CaptureScreenshot` and
/// `StreamLogs` RPCs (spec 7.2 rule 6).
public final class AgentProvider: Impresario_V1_AgentAsyncProvider, @unchecked Sendable {
    private let state: AgentState
    private let expectedToken: String
    private let heartbeatIntervalMs: Int
    private let onShutdown: @Sendable () -> Void

    private let lock = NSLock()
    private var activeDispatcher: Dispatcher?
    private var inflightRPCs = 0
    private var idleWaiters: [@Sendable () -> Void] = []

    public init(
        state: AgentState,
        token: String = "",
        heartbeatIntervalMs: Int = 5000,
        onShutdown: @escaping @Sendable () -> Void = {}
    ) {
        self.state = state
        self.expectedToken = token
        self.heartbeatIntervalMs = heartbeatIntervalMs
        self.onShutdown = onShutdown
    }

    // ------------------------------------------------------------------
    // Handshake
    // ------------------------------------------------------------------

    public func handshake(
        request: Impresario_V1_HandshakeRequest,
        context: GRPCAsyncServerCallContext
    ) async throws -> Impresario_V1_HandshakeResponse {
        let metadataToken = context.request.headers.first(name: AgentMetadata.tokenHeader)
        let parsed = ProtoConversion.handshakeRequest(request, metadataToken: metadataToken)
        return ProtoConversion.handshakeResponse(state.handshake(parsed))
    }

    // ------------------------------------------------------------------
    // Session (bidirectional)
    // ------------------------------------------------------------------

    public func session(
        requestStream: GRPCAsyncRequestStream<Impresario_V1_Command>,
        responseStream: GRPCAsyncResponseStreamWriter<Impresario_V1_Event>,
        context: GRPCAsyncServerCallContext
    ) async throws {
        try authorize(context)
        let sessionId = try requireSessionID(context)
        enterRPC()
        defer { leaveRPC() }

        let dispatcher: Dispatcher
        switch state.openStream(sessionId: sessionId) {
        case .accepted(let executor):
            dispatcher = Dispatcher(executor: executor, heartbeatIntervalMs: heartbeatIntervalMs)
        case .unknownSession:
            throw GRPCStatus(
                code: .notFound,
                message: "unknown session '\(sessionId)' (the handshake was rejected or the session ended)"
            )
        case .alreadyStreaming:
            throw GRPCStatus(
                code: .failedPrecondition,
                message: "a session stream is already open"
            )
        }
        defer { state.endStream() }
        setActive(dispatcher)

        // Events flow from the dispatcher's outbox into the gRPC response
        // stream through one serialized pump (spec 7.2 rule 4).
        let (events, continuation) = AsyncStream<SessionEvent>.makeStream()
        await dispatcher.start { event in
            continuation.yield(event)
        }
        state.logBus.log(.info, "agent", "session \(sessionId) stream opened")

        let pump = Task { [responseStream] in
            for await event in events {
                do {
                    try await responseStream.send(ProtoConversion.event(event))
                } catch {
                    return
                }
                if case .closed = event {
                    return
                }
            }
        }

        do {
            for try await raw in requestStream {
                await accept(raw, into: dispatcher)
            }
            // The master half-closed: finish the accepted work, then close.
            await dispatcher.clientClosed()
        } catch {
            state.logBus.log(.warn, "agent", "session \(sessionId) stream failed: \(error)")
            await dispatcher.shutdown()
        }

        await pump.value
        continuation.finish()
        clearActive(dispatcher)
        state.logBus.log(.info, "agent", "session \(sessionId) stream closed")
    }

    // ------------------------------------------------------------------
    // Out-of-band RPCs (spec 7.2 rule 6)
    // ------------------------------------------------------------------

    public func captureScreenshot(
        request: Impresario_V1_ScreenshotRequest,
        context: GRPCAsyncServerCallContext
    ) async throws -> Impresario_V1_ScreenshotResponse {
        try authorize(context)
        _ = try requireSessionID(context)
        do {
            let png = try state.uiDriver.screenshotPNG()
            guard !png.isEmpty else {
                throw AgentError.internalError("the driver returned an empty screenshot")
            }
            var response = Impresario_V1_ScreenshotResponse()
            response.png = png
            if let size = Png.dimensions(png) {
                response.width = UInt32(clamping: max(0, size.width))
                response.height = UInt32(clamping: max(0, size.height))
            }
            return response
        } catch let error as AgentError {
            return failedScreenshot(error.report)
        } catch {
            return failedScreenshot(AgentError.internalError(String(describing: error)).report)
        }
    }

    public func streamLogs(
        request: Impresario_V1_LogRequest,
        responseStream: GRPCAsyncResponseStreamWriter<Impresario_V1_LogEntry>,
        context: GRPCAsyncServerCallContext
    ) async throws {
        try authorize(context)
        let minLevel = ProtoConversion.logLevel(from: request.minLevel)
        let subscription = state.logBus.subscribe(minLevel: minLevel)
        enterRPC()
        defer {
            subscription.cancel()
            leaveRPC()
        }
        for await line in subscription.stream {
            try await responseStream.send(ProtoConversion.logEntry(line))
        }
    }

    public func shutdown(
        request: Impresario_V1_ShutdownRequest,
        context: GRPCAsyncServerCallContext
    ) async throws -> Impresario_V1_ShutdownResponse {
        try authorize(context)
        let dispatcher = takeActive()
        await dispatcher?.shutdown()
        state.logBus.log(
            .info,
            "agent",
            "shutdown requested (terminate_app=\(request.terminateApp))"
        )
        onShutdown()
        return Impresario_V1_ShutdownResponse()
    }

    // ------------------------------------------------------------------
    // internals
    // ------------------------------------------------------------------

    /// Runs `body` as soon as no streaming RPC is in flight (immediately when
    /// the agent is idle). The service uses it to drain the session and log
    /// streams before stopping the event loops: their response writers would
    /// otherwise schedule on a loop that is already gone, which SwiftNIO logs
    /// as "Cannot schedule tasks on an EventLoop that has already shut down".
    public func whenIdle(_ body: @escaping @Sendable () -> Void) {
        lock.lock()
        if inflightRPCs == 0 {
            lock.unlock()
            body()
            return
        }
        idleWaiters.append(body)
        lock.unlock()
    }

    private func enterRPC() {
        lock.lock()
        inflightRPCs += 1
        lock.unlock()
    }

    private func leaveRPC() {
        lock.lock()
        inflightRPCs -= 1
        let waiters = inflightRPCs == 0 ? idleWaiters : []
        if inflightRPCs == 0 {
            idleWaiters = []
        }
        lock.unlock()
        for waiter in waiters {
            waiter()
        }
    }

    /// One command from the wire: conversion failures still answer exactly one
    /// result, in order, with the specific error code (spec 7.2 rule 4).
    private func accept(_ raw: Impresario_V1_Command, into dispatcher: Dispatcher) async {
        do {
            await dispatcher.enqueue(try ProtoConversion.command(raw))
        } catch let error as AgentError {
            await dispatcher.reject(id: raw.commandID, error: error)
        } catch {
            await dispatcher.reject(
                id: raw.commandID,
                error: .internalError(String(describing: error))
            )
        }
    }

    private func failedScreenshot(_ error: AgentErrorReport) -> Impresario_V1_ScreenshotResponse {
        var response = Impresario_V1_ScreenshotResponse()
        response.error = ProtoConversion.errorInfo(error)
        return response
    }

    /// A configured token must be presented by every RPC (spec 7.2.1).
    private func authorize(_ context: GRPCAsyncServerCallContext) throws {
        guard !expectedToken.isEmpty else { return }
        let presented = context.request.headers.first(name: AgentMetadata.tokenHeader)
        guard presented == expectedToken else {
            throw GRPCStatus(
                code: .unauthenticated,
                message: "missing or invalid \(AgentMetadata.tokenHeader)"
            )
        }
    }

    /// Every RPC after the handshake names its session (spec 7.2.1).
    private func requireSessionID(_ context: GRPCAsyncServerCallContext) throws -> String {
        guard
            let sessionID = context.request.headers.first(name: AgentMetadata.sessionIDHeader),
            !sessionID.isEmpty
        else {
            throw GRPCStatus(
                code: .invalidArgument,
                message: "missing \(AgentMetadata.sessionIDHeader) metadata"
            )
        }
        return sessionID
    }

    private func setActive(_ dispatcher: Dispatcher) {
        lock.lock()
        defer { lock.unlock() }
        activeDispatcher = dispatcher
    }

    private func clearActive(_ dispatcher: Dispatcher) {
        lock.lock()
        defer { lock.unlock() }
        if activeDispatcher === dispatcher {
            activeDispatcher = nil
        }
    }

    private func takeActive() -> Dispatcher? {
        lock.lock()
        defer { lock.unlock() }
        let dispatcher = activeDispatcher
        return dispatcher
    }
}
