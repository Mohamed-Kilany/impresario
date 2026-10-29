import Foundation

/// Agent-wide state: version identity and single-session bookkeeping (spec 7.2
/// rules 2–3). Pure and testable; the gRPC provider owns exactly one instance.
///
/// A session is created by an accepted handshake and is released when its
/// `Session` stream ends, so the master can reconnect for the next Act without
/// restarting the agent app (spec 9.2/D-18).
public final class AgentState: @unchecked Sendable {
    public static let protocolVersion = "1.0.0"
    public static let agentVersion = "1.0.0"

    /// What the service should do with a `Session` stream.
    public enum StreamDecision {
        case accepted(CommandExecutor)
        case unknownSession
        case alreadyStreaming
    }

    public let platform: PlatformKind
    public let logBus: LogBus

    private let driver: UIDriver
    private let expectedToken: String
    private let sessionIdFactory: () -> String
    private let lock = NSLock()
    private var session: Session?
    private var streaming = false

    private struct Session {
        let id: String
        let executor: CommandExecutor
    }

    public init(
        driver: UIDriver,
        platform: PlatformKind,
        expectedToken: String = "",
        logBus: LogBus = LogBus(),
        sessionIdFactory: @escaping () -> String = { UUID().uuidString.lowercased() }
    ) {
        self.driver = driver
        self.platform = platform
        self.expectedToken = expectedToken
        self.logBus = logBus
        self.sessionIdFactory = sessionIdFactory
    }

    /// The live driver (the out-of-band `Screenshot` RPC uses it directly).
    public var uiDriver: UIDriver { driver }

    public var activeSessionId: String? {
        lock.lock()
        defer { lock.unlock() }
        return session?.id
    }

    // ------------------------------------------------------------------
    // handshake
    // ------------------------------------------------------------------

    /// Evaluates a `HandshakeRequest` (spec 7.1/7.2 rules 2–3 and the token
    /// rule) and, when accepted, creates the session.
    public func handshake(_ request: HandshakeRequestData) -> HandshakeResult {
        if !expectedToken.isEmpty && request.authToken != expectedToken {
            logBus.log(.warn, "agent", "handshake rejected: missing or invalid x-impresario-token")
            return rejection(.unauthenticated, "missing or invalid x-impresario-token")
        }

        let requestedVersion = request.protocolVersion.trimmingCharacters(in: .whitespaces)
        if requestedVersion.isEmpty {
            return rejection(.protocolViolation, "the protocol version is missing")
        }
        if major(of: requestedVersion) != major(of: Self.protocolVersion) {
            return rejection(
                .protocolViolation,
                "incompatible protocol version '\(requestedVersion)' (this agent speaks \(Self.protocolVersion))"
            )
        }

        guard let requestedPlatform = request.platform, requestedPlatform == platform else {
            let asked = request.platform?.rawValue ?? "an unspecified platform"
            return rejection(
                .protocolViolation,
                "this agent serves \(platform.rawValue); the handshake asked for \(asked)"
            )
        }

        lock.lock()
        if let current = session {
            lock.unlock()
            logBus.log(.warn, "agent", "handshake rejected: session \(current.id) is still active")
            return rejection(.alreadyInUse, "already in use by session \(current.id)")
        }
        let executorOptions = ExecutorOptions.from(
            options: request.options,
            platform: platform,
            appId: request.appId
        )
        let executor = CommandExecutor(
            driver: driver,
            options: executorOptions,
            log: { [logBus] level, source, message in
                logBus.log(level, source, message)
            }
        )
        let sessionId = sessionIdFactory()
        session = Session(id: sessionId, executor: executor)
        lock.unlock()

        (driver as? DriverExtras)?.prepare(
            appId: request.appId,
            animationsDisabled: executorOptions.animationsDisabled
        )
        logBus.log(
            .info,
            "agent",
            "session \(sessionId) accepted for '\(request.appId)' (\(platform.rawValue), master \(request.masterVersion))"
        )

        return HandshakeResult(
            accepted: true,
            sessionId: sessionId,
            agentVersion: Self.agentVersion,
            protocolVersion: Self.protocolVersion,
            capabilities: AgentCapability.forPlatform(platform),
            deviceInfo: driver.deviceInfo(),
            error: nil
        )
    }

    // ------------------------------------------------------------------
    // session stream lifecycle
    // ------------------------------------------------------------------

    /// Admits the `Session` stream that carries `x-impresario-session-id`.
    public func openStream(sessionId: String) -> StreamDecision {
        lock.lock()
        defer { lock.unlock() }
        guard let session, session.id == sessionId else {
            return .unknownSession
        }
        guard !streaming else {
            return .alreadyStreaming
        }
        streaming = true
        return .accepted(session.executor)
    }

    /// The stream is over (closed event delivered, client disconnected, or the
    /// transport failed): release the session for the next handshake.
    public func endStream() {
        lock.lock()
        defer { lock.unlock() }
        streaming = false
        session = nil
    }

    // ------------------------------------------------------------------
    // helpers
    // ------------------------------------------------------------------

    private func rejection(_ code: AgentErrorCode, _ message: String) -> HandshakeResult {
        HandshakeResult(
            accepted: false,
            sessionId: nil,
            agentVersion: Self.agentVersion,
            protocolVersion: Self.protocolVersion,
            capabilities: [],
            deviceInfo: nil,
            error: AgentErrorReport(code: code, message: message)
        )
    }

    private func major(of version: String) -> String {
        String(version.split(separator: ".").first ?? Substring(version))
    }
}
