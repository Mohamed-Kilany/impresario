import AgentCore
import Dispatch
import Foundation
import GRPC
import NIO

/// Configuration of the in-app agent (spec 12.1).
///
/// The master allocates the agent port dynamically and passes it to the test
/// bundle as `IMPRESARIO_AGENT_PORT`; `50062` is only the standalone fallback
/// when no port is supplied, so both sides always agree.
public struct AgentServiceConfig: Sendable {
    /// The default port the master connects to (`APPLE_AGENT_PORT`).
    public static let defaultPort = 50062

    public var platform: PlatformKind
    /// Bind address; `0.0.0.0` serves loopback (simulator, iproxy) and LAN
    /// (tvOS over WLAN) masters alike.
    public var host: String
    public var port: Int
    /// The shared token; empty disables the check (spec 7.2.1).
    public var token: String
    public var heartbeatIntervalMs: Int
    /// Advertise `_impresario._tcp` so a master can discover the device.
    public var advertiseBonjour: Bool
    public var bonjourServiceName: String
    public var bonjourDomain: String

    public init(
        platform: PlatformKind,
        host: String = "0.0.0.0",
        port: Int = AgentServiceConfig.defaultPort,
        token: String = "",
        heartbeatIntervalMs: Int = 5000,
        advertiseBonjour: Bool = true,
        bonjourServiceName: String = "impresario-agent",
        bonjourDomain: String = "local."
    ) {
        self.platform = platform
        self.host = host
        self.port = port
        self.token = token
        self.heartbeatIntervalMs = heartbeatIntervalMs
        self.advertiseBonjour = advertiseBonjour
        self.bonjourServiceName = bonjourServiceName
        self.bonjourDomain = bonjourDomain
    }

    /// The port for this run: `IMPRESARIO_AGENT_PORT`, else the default.
    public static func portFromEnvironment(
        _ environment: [String: String] = ProcessInfo.processInfo.environment
    ) -> Int {
        guard
            let raw = environment["IMPRESARIO_AGENT_PORT"],
            let port = Int(raw),
            (1...65535).contains(port)
        else {
            return defaultPort
        }
        return port
    }
}

/// The in-app agent server: one [`AgentProvider`] over an [`AgentState`]
/// (spec 12.1). `start()` binds synchronously, so the port is serving as soon
/// as it returns.
public final class AgentService: @unchecked Sendable {
    public let config: AgentServiceConfig

    private let state: AgentState
    private let provider: AgentProvider
    private let group: MultiThreadedEventLoopGroup
    private let shutdownStream: AsyncStream<Void>
    private let shutdownContinuation: AsyncStream<Void>.Continuation
    private let lock = NSLock()
    private var server: Server?
    private var publisher: BonjourPublisher?
    private var stopped = false

    public init(config: AgentServiceConfig, driver: UIDriver) {
        self.config = config
        let state = AgentState(
            driver: driver,
            platform: config.platform,
            expectedToken: config.token
        )
        let (stream, continuation) = AsyncStream<Void>.makeStream()
        self.state = state
        self.shutdownStream = stream
        self.shutdownContinuation = continuation
        self.group = MultiThreadedEventLoopGroup(numberOfThreads: 2)
        self.provider = AgentProvider(
            state: state,
            token: config.token,
            heartbeatIntervalMs: config.heartbeatIntervalMs,
            onShutdown: { continuation.yield(()) }
        )
    }

    /// Binds the port and (optionally) publishes the Bonjour service.
    public func start() throws -> AgentServiceHandle {
        let server = try Server.insecure(group: group)
            .withServiceProviders([provider])
            .bind(host: config.host, port: config.port)
            .wait()
        let port = server.channel.localAddress?.port ?? config.port
        lock.lock()
        self.server = server
        lock.unlock()

        if config.advertiseBonjour {
            let publisher = BonjourPublisher(
                name: config.bonjourServiceName,
                port: port,
                log: state.logBus,
                domain: config.bonjourDomain,
                txt: [
                    "platform": config.platform.rawValue,
                    "protocol": AgentState.protocolVersion,
                    "agent": AgentState.agentVersion,
                ]
            )
            publisher.publish()
            lock.lock()
            self.publisher = publisher
            lock.unlock()
        }
        state.logBus.log(
            .info,
            "agent",
            "\(config.platform.rawValue) agent listening on \(config.host):\(port) "
                + "(agent \(AgentState.agentVersion), protocol \(AgentState.protocolVersion))"
        )
        return AgentServiceHandle(
            service: self,
            port: port,
            shutdown: shutdownStream
        )
    }

    /// Stops the advertisement, the server and the event loops. Idempotent and
    /// safe to call after a failed `start()`.
    public func stop() {
        lock.lock()
        if stopped {
            lock.unlock()
            return
        }
        stopped = true
        let publisher = self.publisher
        self.publisher = nil
        lock.unlock()

        publisher?.stop()

        // Streaming RPCs (the session stream, `StreamLogs`) outlive their
        // request: let them finish before the event loops stop, because their
        // response writers would otherwise schedule on a loop that is already
        // gone (SwiftNIO logs "Cannot schedule tasks on an EventLoop that has
        // already shut down" and announces a forced crash in future versions).
        // The drain is bounded: a client that holds a stream open must not
        // delay the agent for long.
        let idle = DispatchSemaphore(value: 0)
        provider.whenIdle { idle.signal() }
        _ = idle.wait(timeout: .now() + 2)

        // Quiesce in its own function so the `Server` (and the
        // `ServerQuiescingHelper` it owns) is released while the loops are
        // still running: the helper's deinit fails its collector promise, and
        // NIO schedules that completion on the loop even when the promise is
        // already fulfilled - after `syncShutdownGracefully` that scheduling is
        // the error above.
        stopServer()

        shutdownContinuation.finish()
        try? group.syncShutdownGracefully()
        state.logBus.log(.info, "agent", "agent stopped")
    }

    /// Quiesces the listener and every accepted connection on its own loop,
    /// then closes the server hard. `Server.close()` alone only closes the
    /// listening socket; the drain window is bounded because a client that
    /// keeps a stream open must not delay the agent, and `close()` is
    /// idempotent.
    private func stopServer() {
        lock.lock()
        let server = self.server
        self.server = nil
        lock.unlock()
        guard let server else { return }

        let quiesced = server.channel.eventLoop.makePromise(of: Void.self)
        server.initiateGracefulShutdown().cascade(to: quiesced)
        let force = server.channel.eventLoop.scheduleTask(in: .seconds(2)) {
            quiesced.succeed(())
        }
        quiesced.futureResult.whenComplete { _ in force.cancel() }
        try? quiesced.futureResult.wait()
        try? server.close().wait()
    }
}

/// A running agent. The UI test waits in `testServe()` until a master calls
/// `Shutdown` (spec 12.1, D-18) and then stops the service.
public final class AgentServiceHandle: @unchecked Sendable {
    /// The bound port (equals the configured port unless 0 was requested).
    public let port: Int
    private let service: AgentService
    private let shutdown: AsyncStream<Void>

    init(service: AgentService, port: Int, shutdown: AsyncStream<Void>) {
        self.service = service
        self.port = port
        self.shutdown = shutdown
    }

    /// Returns when a master requests `Shutdown` or the service stops.
    public func waitForShutdown() async {
        for await _ in shutdown {
            return
        }
    }

    /// Convenience for the UI test and loopback tests: wait for `Shutdown`,
    /// then stop.
    public func serveUntilShutdown() async {
        await waitForShutdown()
        stop()
    }

    /// Stops the server (idempotent).
    public func stop() {
        service.stop()
    }
}
