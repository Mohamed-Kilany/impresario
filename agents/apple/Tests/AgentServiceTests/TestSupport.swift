import AgentCore
import AgentService
import Foundation
import GRPC
import ImpresarioWire
import NIO
import XCTest

// ---------------------------------------------------------------------------
// Fakes and helpers for the loopback tests: a real `AgentService` on an
// ephemeral port with the generated gRPC client on the other side.
// ---------------------------------------------------------------------------

/// A 1×1 PNG (signature + IHDR + IDAT + IEND) the agent can parse.
enum TestPNG {
    static let valid: Data = {
        var bytes: [UInt8] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]
        bytes += [0x00, 0x00, 0x00, 0x0D]
        bytes += Array("IHDR".utf8)
        bytes += [0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01]
        bytes += [0x08, 0x06, 0x00, 0x00, 0x00]
        bytes += [0x1F, 0x15, 0xC4, 0x89]
        bytes += [0x00, 0x00, 0x00, 0x0A]
        bytes += Array("IDAT".utf8)
        bytes += [0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01]
        bytes += [0x0D, 0x0A, 0x2D, 0xB4]
        bytes += [0x00, 0x00, 0x00, 0x00]
        bytes += Array("IEND".utf8)
        bytes += [0xAE, 0x42, 0x60, 0x82]
        return Data(bytes)
    }()
}

struct TestTimeout: Error {}

func withTimeout<T: Sendable>(
    _ seconds: Double = 5,
    _ operation: @escaping @Sendable () async throws -> T
) async throws -> T {
    try await withThrowingTaskGroup(of: T.self) { group in
        group.addTask { try await operation() }
        group.addTask {
            try await Task.sleep(nanoseconds: UInt64(seconds * 1_000_000_000))
            throw TestTimeout()
        }
        guard let result = try await group.next() else { throw TestTimeout() }
        group.cancelAll()
        return result
    }
}

/// A single-iterator view over a response stream. NIO's producer is unicast —
/// `makeAsyncIterator()` may be called only once — so the tests keep one reader
/// per stream and read from it repeatedly.
final class EventReader: @unchecked Sendable {
    private var iterator: GRPCAsyncResponseStream<Impresario_V1_Event>.Iterator

    init(_ stream: GRPCAsyncResponseStream<Impresario_V1_Event>) {
        self.iterator = stream.makeAsyncIterator()
    }

    /// Reads events until `predicate` matches (with a timeout).
    func until(
        _ predicate: @escaping @Sendable (Impresario_V1_Event) -> Bool,
        timeout: Double = 5
    ) async throws -> Impresario_V1_Event {
        try await withTimeout(timeout) {
            while let event = try await self.nextEvent() {
                if predicate(event) { return event }
            }
            throw TestTimeout()
        }
    }

    /// Reads the stream to its end. Used to prove that a rejected RPC produces
    /// no events and fails with the expected status.
    func drain() async throws -> [Impresario_V1_Event] {
        var events: [Impresario_V1_Event] = []
        while let event = try await nextEvent() {
            events.append(event)
        }
        return events
    }

    private func nextEvent() async throws -> Impresario_V1_Event? {
        try await iterator.next()
    }
}

@Sendable
func isResult(_ event: Impresario_V1_Event) -> Bool {
    if case .result = event.body { return true }
    return false
}

@Sendable
func isHeartbeat(_ event: Impresario_V1_Event) -> Bool {
    if case .heartbeat = event.body { return true }
    return false
}

@Sendable
func isClosed(_ event: Impresario_V1_Event) -> Bool {
    if case .closed = event.body { return true }
    return false
}

// MARK: - fake driver

final class FakeElement {
    let id: String
    var label: String
    var textValue: String
    var visible: Bool
    var enabled: Bool
    var hittable: Bool
    var hasFocus: Bool
    var frame: CGRect

    init(
        id: String,
        label: String = "",
        text: String = "",
        visible: Bool = true,
        enabled: Bool = true,
        hittable: Bool = true,
        focused: Bool = false,
        frame: CGRect = CGRect(x: 0, y: 0, width: 100, height: 40)
    ) {
        self.id = id
        self.label = label
        self.textValue = text
        self.visible = visible
        self.enabled = enabled
        self.hittable = hittable
        self.hasFocus = focused
        self.frame = frame
    }

    var ref: ElementRef { ElementRef(self) }
}

final class FakeDriver: UIDriver, DriverExtras, @unchecked Sendable {
    var elements: [FakeElement] = [
        FakeElement(id: "login", label: "Log in", text: "", frame: CGRect(x: 20, y: 40, width: 200, height: 60)),
        FakeElement(id: "title", label: "Impresario", text: "Impresario"),
    ]
    var deviceInfoValue = AgentDeviceInfo(
        model: "iPhone Fake",
        osName: "iOS",
        osVersion: "18.0",
        screenWidthPx: 1179,
        screenHeightPx: 2556,
        pixelRatio: 3.0,
        runtime: "XCUITest"
    )
    var screenshotData = TestPNG.valid
    var screenshotError: Error?

    private let lock = NSLock()
    private var _taps: [String] = []
    private var _typed: [(id: String, text: String)] = []
    private var _remotes: [(button: RemoteButton, count: Int, interval: TimeInterval)] = []
    private var _prepared: (appId: String, animationsDisabled: Bool)?
    private var _screenshotCalls = 0
    private var _launches: [String] = []

    private func locked<T>(_ body: () -> T) -> T {
        lock.lock()
        defer { lock.unlock() }
        return body()
    }

    var taps: [String] { locked { _taps } }
    var typed: [(id: String, text: String)] { locked { _typed } }
    var remotes: [(button: RemoteButton, count: Int, interval: TimeInterval)] { locked { _remotes } }
    var prepared: (appId: String, animationsDisabled: Bool)? { locked { _prepared } }
    var screenshotCalls: Int { locked { _screenshotCalls } }
    var launches: [String] { locked { _launches } }

    // MARK: UIDriver

    func find(_ s: SelectorSpec, timeout: TimeInterval) throws -> [ElementRef] {
        elements.filter { element in
            switch s.strategy {
            case .accessibilityId: return element.id == s.value
            case .label: return element.label == s.value
            case .labelContains: return element.label.contains(s.value)
            case .predicate: return true
            case .iosType: return s.value == "button"
            }
        }.map(\.ref)
    }

    func count(_ s: SelectorSpec) throws -> Int {
        try find(s, timeout: 0).count
    }

    func tap(_ e: ElementRef) throws {
        guard let element = e.handle as? FakeElement else {
            throw AgentError.internalError("not a fake element")
        }
        guard element.hittable else {
            throw AgentError.notInteractable("the fake element '\(element.id)' is not hittable")
        }
        locked { _taps.append(element.id) }
    }

    func press(_ e: ElementRef, duration: TimeInterval) throws {
        guard let element = e.handle as? FakeElement else {
            throw AgentError.internalError("not a fake element")
        }
        locked { _taps.append(element.id) }
    }

    func text(_ e: ElementRef) -> String { (e.handle as? FakeElement)?.textValue ?? "" }

    func typeText(_ e: ElementRef, _ t: String) throws {
        guard let element = e.handle as? FakeElement else {
            throw AgentError.internalError("not a fake element")
        }
        locked { _typed.append((element.id, t)) }
        element.textValue += t
    }

    func deleteAll(_ e: ElementRef) throws {
        (e.handle as? FakeElement)?.textValue = ""
    }

    func isVisible(_ e: ElementRef) -> Bool { (e.handle as? FakeElement)?.visible ?? false }
    func isEnabled(_ e: ElementRef) -> Bool { (e.handle as? FakeElement)?.enabled ?? false }
    func isHittable(_ e: ElementRef) -> Bool { (e.handle as? FakeElement)?.hittable ?? false }
    func hasFocus(_ e: ElementRef) -> Bool { (e.handle as? FakeElement)?.hasFocus ?? false }
    func frame(_ e: ElementRef) -> CGRect { (e.handle as? FakeElement)?.frame ?? .zero }
    func focusedFrame() -> CGRect? { elements.first(where: { $0.hasFocus })?.frame }

    func remote(_ b: RemoteButton, `repeat` count: Int, interval: TimeInterval) throws {
        locked { _remotes.append((b, count, interval)) }
    }

    func pressHome() throws {}

    func scroll(_ c: ElementRef?, _ d: Direction, steps: Int) throws {}

    func swipe(_ e: ElementRef?, _ d: Direction) throws {}

    func screenshotPNG() throws -> Data {
        if let screenshotError { throw screenshotError }
        locked { _screenshotCalls += 1 }
        return screenshotData
    }

    func launch(_ bundleId: String) throws {
        locked { _launches.append(bundleId) }
    }

    func terminate(_ bundleId: String) throws {}

    func activate(_ bundleId: String) throws {}

    func deviceInfo() -> AgentDeviceInfo { deviceInfoValue }

    // MARK: DriverExtras

    func prepare(appId: String, animationsDisabled: Bool) {
        locked { _prepared = (appId, animationsDisabled) }
    }

    func targetCrashed(appId: String) -> Bool { false }
}

// MARK: - harness

/// A running agent on an ephemeral loopback port with a connected client.
final class AgentHarness {
    let service: AgentService
    let handle: AgentServiceHandle
    let driver: FakeDriver
    let platform: PlatformKind
    let token: String
    let group: MultiThreadedEventLoopGroup
    let connection: ClientConnection
    let client: Impresario_V1_AgentAsyncClient

    private init(
        service: AgentService,
        handle: AgentServiceHandle,
        driver: FakeDriver,
        platform: PlatformKind,
        token: String,
        group: MultiThreadedEventLoopGroup,
        connection: ClientConnection
    ) {
        self.service = service
        self.handle = handle
        self.driver = driver
        self.platform = platform
        self.token = token
        self.group = group
        self.connection = connection
        self.client = Impresario_V1_AgentAsyncClient(channel: connection)
    }

    static func start(
        platform: PlatformKind = .ios,
        token: String = "",
        heartbeatIntervalMs: Int = 0,
        driver: FakeDriver = FakeDriver()
    ) throws -> AgentHarness {
        let config = AgentServiceConfig(
            platform: platform,
            host: "127.0.0.1",
            port: 0,
            token: token,
            heartbeatIntervalMs: heartbeatIntervalMs,
            advertiseBonjour: false,
            bonjourServiceName: "impresario-agent-tests"
        )
        let service = AgentService(config: config, driver: driver)
        let handle = try service.start()
        let group = MultiThreadedEventLoopGroup(numberOfThreads: 1)
        let configuration = ClientConnection.Configuration.default(
            target: .hostAndPort("127.0.0.1", handle.port),
            eventLoopGroup: group
        )
        let connection = ClientConnection(configuration: configuration)
        return AgentHarness(
            service: service,
            handle: handle,
            driver: driver,
            platform: platform,
            token: token,
            group: group,
            connection: connection
        )
    }

    func handshake(
        platform: PlatformKind? = nil,
        appId: String = "com.example.app",
        protocolVersion: String = "1.0.0",
        token: String = "",
        metadataToken: String? = nil
    ) async throws -> Impresario_V1_HandshakeResponse {
        var request = Impresario_V1_HandshakeRequest()
        request.protocolVersion = protocolVersion
        request.masterVersion = "0.1.0"
        request.platform = ProtoConversion.platform(platform ?? self.platform)
        request.appID = appId
        request.authToken = token
        return try await client.handshake(
            request,
            callOptions: AgentMetadata.callOptions(token: metadataToken)
        )
    }

    func stop() {
        // The master closes its connection first and the test bundle stops the
        // agent afterwards: that is the order a real shutdown follows (and it
        // lets the service drain its streams instead of waiting for us).
        try? connection.close().wait()
        handle.stop()
        try? group.syncShutdownGracefully()
    }
}

// MARK: - command builders

func clickCommand(id: String = "c-1", selector idString: String = "login") -> Impresario_V1_Command {
    var action = Impresario_V1_RunAction()
    action.type = .click
    var selector = Impresario_V1_Selector()
    selector.strategy = .accessibilityID
    selector.value = idString
    action.selector = selector
    var command = Impresario_V1_Command()
    command.commandID = id
    command.timeoutMs = 2_000
    command.body = .runAction(action)
    return command
}

func screenshotCommand(id: String = "c-shot") -> Impresario_V1_Command {
    var command = Impresario_V1_Command()
    command.commandID = id
    command.body = .screenshot(Impresario_V1_Screenshot())
    return command
}

/// A `CLICK` with a CSS selector: not supported on Apple platforms.
func cssClickCommand(id: String = "c-bad") -> Impresario_V1_Command {
    var action = Impresario_V1_RunAction()
    action.type = .click
    var selector = Impresario_V1_Selector()
    selector.strategy = .css
    selector.value = "#login"
    action.selector = selector
    var command = Impresario_V1_Command()
    command.commandID = id
    command.body = .runAction(action)
    return command
}
