import Foundation
import XCTest
@testable import AgentCore

// MARK: - time and logs

final class TestClock: @unchecked Sendable {
    private let lock = NSLock()
    private var value: Int

    init(start: Int = 1_000_000) {
        self.value = start
    }

    var now: Int {
        lock.lock()
        defer { lock.unlock() }
        return value
    }

    func advance(_ milliseconds: Int) {
        lock.lock()
        value += milliseconds
        lock.unlock()
    }
}

struct RecordedLog: Equatable {
    let level: LogLevel
    let source: String
    let message: String
}

final class TestLog: @unchecked Sendable {
    private let lock = NSLock()
    private var entries: [RecordedLog] = []

    func log(_ level: LogLevel, _ source: String, _ message: String) {
        lock.lock()
        entries.append(RecordedLog(level: level, source: source, message: message))
        lock.unlock()
    }

    var all: [RecordedLog] {
        lock.lock()
        defer { lock.unlock() }
        return entries
    }

    func at(_ level: LogLevel) -> [RecordedLog] {
        all.filter { $0.level == level }
    }
}

// MARK: - fixtures

enum FakePNG {
    /// A 24-byte-header PNG (signature + IHDR) with enough payload for the
    /// wire; CRC values are not validated anywhere in the agent.
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

// MARK: - fake driver

final class FakeElement {
    let id: String
    var label: String
    var textValue: String
    var visible: Bool
    var enabled: Bool
    var hittable: Bool
    var focused: Bool
    var frame: CGRect
    var elementType: AgentElementType

    init(
        id: String,
        label: String = "",
        text: String = "",
        visible: Bool = true,
        enabled: Bool = true,
        hittable: Bool = true,
        focused: Bool = false,
        frame: CGRect = CGRect(x: 0, y: 0, width: 100, height: 40),
        type: AgentElementType = .button
    ) {
        self.id = id
        self.label = label
        self.textValue = text
        self.visible = visible
        self.enabled = enabled
        self.hittable = hittable
        self.focused = focused
        self.frame = frame
        self.elementType = type
    }

    var ref: ElementRef { ElementRef(self) }
}

final class FakeUIDriver: UIDriver, DriverExtras, @unchecked Sendable {
    // Configuration (set before use).
    var elements: [FakeElement] = []
    var deviceInfoValue = AgentDeviceInfo(
        model: "Apple TV Fake",
        osName: "tvOS",
        osVersion: "18.0",
        screenWidthPx: 1920,
        screenHeightPx: 1080,
        pixelRatio: 1.0,
        runtime: "XCUITest"
    )
    var screenshotData = FakePNG.valid
    var crashAfterLaunch = false
    var pressBackResult = false
    var longPressSelectResult = false
    var keyboardShown = false
    var ignoreTypeText = false
    var findError: Error?
    var screenshotError: Error?
    var longPressError: Error?
    var findHandler: ((SelectorSpec, Int) -> [FakeElement]?)?
    var tapHandler: ((FakeElement) -> Void)?
    var remoteHandler: ((RemoteButton) throws -> Void)?
    var scrollHandler: ((FakeElement?, Direction, Int) -> Void)?
    var selectValueHandler: ((FakeElement, String) -> Bool)?
    var focusedFrameProvider: (() -> CGRect?)?
    var mediaSnapshotValue: MediaSnapshot?
    var drmSnapshotValue: DrmSnapshot?
    var regionCoverageValue: Double?
    var showingDecodedValue: Bool?
    var showingSourceValue: String?

    private let lock = NSLock()
    private var _findCalls = 0
    private var _taps: [String] = []
    private var _presses: [(id: String, duration: TimeInterval)] = []
    private var _typed: [(id: String, text: String)] = []
    private var _deletes: [String] = []
    private var _remotes: [(button: RemoteButton, count: Int, interval: TimeInterval)] = []
    private var _homePresses = 0
    private var _backPresses = 0
    private var _scrolls: [(id: String?, direction: Direction, steps: Int)] = []
    private var _swipes: [(id: String?, direction: Direction)] = []
    private var _launches: [String] = []
    private var _terminations: [String] = []
    private var _activations: [String] = []
    private var _prepared: (appId: String, animationsDisabled: Bool)?
    private var _longPressDurations: [TimeInterval] = []
    private var _crashed = false
    private var _selectValues: [(id: String, value: String)] = []
    private var _screenshotCalls = 0

    private func locked<T>(_ body: () -> T) -> T {
        lock.lock()
        defer { lock.unlock() }
        return body()
    }

    // MARK: recordings

    var findCalls: Int { locked { _findCalls } }
    var taps: [String] { locked { _taps } }
    var presses: [(id: String, duration: TimeInterval)] { locked { _presses } }
    var typed: [(id: String, text: String)] { locked { _typed } }
    var deletes: [String] { locked { _deletes } }
    var remotes: [(button: RemoteButton, count: Int, interval: TimeInterval)] { locked { _remotes } }
    var homePresses: Int { locked { _homePresses } }
    var backPresses: Int { locked { _backPresses } }
    var scrolls: [(id: String?, direction: Direction, steps: Int)] { locked { _scrolls } }
    var swipes: [(id: String?, direction: Direction)] { locked { _swipes } }
    var launches: [String] { locked { _launches } }
    var terminations: [String] { locked { _terminations } }
    var activations: [String] { locked { _activations } }
    var prepared: (appId: String, animationsDisabled: Bool)? { locked { _prepared } }
    var longPressDurations: [TimeInterval] { locked { _longPressDurations } }
    var selectValues: [(id: String, value: String)] { locked { _selectValues } }
    var screenshotCalls: Int { locked { _screenshotCalls } }
    var crashed: Bool {
        get { locked { _crashed } }
        set { locked { _crashed = newValue } }
    }

    // MARK: UIDriver

    func find(_ s: SelectorSpec, timeout: TimeInterval) throws -> [ElementRef] {
        let call: Int = locked {
            _findCalls += 1
            return _findCalls
        }
        if let findError { throw findError }
        if let findHandler, let result = findHandler(s, call) {
            return result.map(\.ref)
        }
        return elements.filter { matches($0, s) }.map(\.ref)
    }

    func count(_ s: SelectorSpec) throws -> Int {
        try find(s, timeout: 0).count
    }

    func tap(_ e: ElementRef) throws {
        let element = try fakeElement(e)
        guard element.hittable else {
            throw AgentError.notInteractable("the fake element '\(element.id)' is not hittable")
        }
        locked { _taps.append(element.id) }
        tapHandler?(element)
    }

    func press(_ e: ElementRef, duration: TimeInterval) throws {
        let element = try fakeElement(e)
        locked { _presses.append((element.id, duration)) }
    }

    func text(_ e: ElementRef) -> String {
        (e.handle as? FakeElement)?.textValue ?? ""
    }

    func typeText(_ e: ElementRef, _ t: String) throws {
        let element = try fakeElement(e)
        locked { _typed.append((element.id, t)) }
        if !ignoreTypeText {
            element.textValue += t
        }
    }

    func deleteAll(_ e: ElementRef) throws {
        let element = try fakeElement(e)
        locked { _deletes.append(element.id) }
        element.textValue = ""
    }

    func isVisible(_ e: ElementRef) -> Bool { (e.handle as? FakeElement)?.visible ?? false }
    func isEnabled(_ e: ElementRef) -> Bool { (e.handle as? FakeElement)?.enabled ?? false }
    func isHittable(_ e: ElementRef) -> Bool { (e.handle as? FakeElement)?.hittable ?? false }
    func hasFocus(_ e: ElementRef) -> Bool { (e.handle as? FakeElement)?.focused ?? false }
    func frame(_ e: ElementRef) -> CGRect { (e.handle as? FakeElement)?.frame ?? .zero }

    func focusedFrame() -> CGRect? {
        if let focusedFrameProvider { return focusedFrameProvider() }
        return elements.first(where: { $0.focused })?.frame
    }

    func remote(_ b: RemoteButton, `repeat` count: Int, interval: TimeInterval) throws {
        locked { _remotes.append((b, count, interval)) }
        if let remoteHandler { try remoteHandler(b) }
    }

    func pressHome() throws {
        locked { _homePresses += 1 }
    }

    func scroll(_ c: ElementRef?, _ d: Direction, steps: Int) throws {
        let element = c?.handle as? FakeElement
        locked { _scrolls.append((element?.id, d, steps)) }
        if let scrollHandler { scrollHandler(element, d, steps) }
    }

    func swipe(_ e: ElementRef?, _ d: Direction) throws {
        let element = e?.handle as? FakeElement
        locked { _swipes.append((element?.id, d)) }
    }

    func screenshotPNG() throws -> Data {
        if let screenshotError { throw screenshotError }
        locked { _screenshotCalls += 1 }
        return screenshotData
    }

    func launch(_ bundleId: String) throws {
        locked { _launches.append(bundleId) }
        if crashAfterLaunch {
            crashed = true
        }
    }

    func terminate(_ bundleId: String) throws {
        locked { _terminations.append(bundleId) }
        crashed = false
    }

    func activate(_ bundleId: String) throws {
        locked { _activations.append(bundleId) }
    }

    func deviceInfo() -> AgentDeviceInfo { deviceInfoValue }

    // MARK: DriverExtras

    func prepare(appId: String, animationsDisabled: Bool) {
        locked { _prepared = (appId, animationsDisabled) }
    }

    func targetCrashed(appId: String) -> Bool { crashed }

    func keyboardVisible() -> Bool { keyboardShown }

    func longPressSelect(duration: TimeInterval) throws -> Bool {
        if let longPressError { throw longPressError }
        locked { _longPressDurations.append(duration) }
        return longPressSelectResult
    }

    func pressBack() throws -> Bool {
        locked { _backPresses += 1 }
        return pressBackResult
    }

    func selectValue(_ e: ElementRef, _ value: String) throws -> Bool {
        let element = try fakeElement(e)
        locked { _selectValues.append((element.id, value)) }
        return selectValueHandler?(element, value) ?? false
    }

    func mediaSnapshot() -> MediaSnapshot? { mediaSnapshotValue }

    func drmSnapshot() -> DrmSnapshot? { drmSnapshotValue }

    func regionCoverage(png: Data, rect: CGRect) -> Double? { regionCoverageValue }

    func showingDecoded(_ e: ElementRef) -> Bool? { showingDecodedValue }

    func showingSource(_ e: ElementRef) -> String? { showingSourceValue }

    // MARK: helpers

    private func fakeElement(_ ref: ElementRef) throws -> FakeElement {
        guard let element = ref.handle as? FakeElement else {
            throw AgentError.internalError("the element reference is not a FakeElement")
        }
        return element
    }

    private func matches(_ element: FakeElement, _ spec: SelectorSpec) -> Bool {
        switch spec.strategy {
        case .accessibilityId:
            return element.id == spec.value
        case .label:
            return element.label == spec.value
        case .labelContains:
            return element.label.contains(spec.value)
        case .predicate:
            return true
        case .iosType:
            return element.elementType.rawValue == spec.value
        }
    }
}

// MARK: - events

final class EventRecorder: @unchecked Sendable {
    private let lock = NSLock()
    private var events: [SessionEvent] = []

    func record(_ event: SessionEvent) {
        lock.lock()
        events.append(event)
        lock.unlock()
    }

    var all: [SessionEvent] {
        lock.lock()
        defer { lock.unlock() }
        return events
    }

    var results: [CommandResultData] {
        all.compactMap { event in
            if case .result(let result) = event { return result }
            return nil
        }
    }

    var heartbeats: [(seq: Int, unixMs: Int64, currentCommandId: String)] {
        all.compactMap { event in
            if case .heartbeat(let seq, let unixMs, let currentCommandId) = event {
                return (seq, unixMs, currentCommandId)
            }
            return nil
        }
    }

    var closedReasons: [String] {
        all.compactMap { event in
            if case .closed(let reason, _) = event { return reason }
            return nil
        }
    }
}

// MARK: - gates

/// One-shot async permits for deterministic dispatcher tests. Not
/// cancellation-safe by design (the cancellation tests use real sleeps).
final class GateSleeper: @unchecked Sendable {
    private let lock = NSLock()
    private var waiters: [CheckedContinuation<Void, Never>] = []
    private var permits = 0

    func open() {
        lock.lock()
        if waiters.isEmpty {
            permits += 1
            lock.unlock()
            return
        }
        let waiter = waiters.removeFirst()
        lock.unlock()
        waiter.resume()
    }

    func wait() async {
        await withCheckedContinuation { continuation in
            lock.lock()
            if permits > 0 {
                permits -= 1
                lock.unlock()
                continuation.resume()
                return
            }
            waiters.append(continuation)
            lock.unlock()
        }
    }

    var waiterCount: Int {
        lock.lock()
        defer { lock.unlock() }
        return waiters.count
    }
}

// MARK: - async test helpers

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

func waitUntil(
    _ seconds: Double = 5,
    _ message: String = "condition",
    _ condition: @escaping @Sendable () async -> Bool
) async throws {
    let deadline = Date().addingTimeInterval(seconds)
    while Date() < deadline {
        if await condition() { return }
        try await Task.sleep(nanoseconds: 2_000_000)
    }
    XCTFail("timed out waiting for \(message)")
    throw TestTimeout()
}

// MARK: - executor helpers

extension XCTestCase {
    func makeExecutor(
        driver: FakeUIDriver,
        platform: PlatformKind = .ios,
        appId: String = "com.example.app",
        pollIntervalMs: Int = 100,
        scrollMaxAttempts: Int = 6,
        focusMaxSteps: Int = 8,
        clock: TestClock,
        log: TestLog? = nil,
        sleeper: ((Int) async throws -> Void)? = nil
    ) -> CommandExecutor {
        let options = ExecutorOptions(
            platform: platform,
            appId: appId,
            pollIntervalMs: pollIntervalMs,
            scrollMaxAttempts: scrollMaxAttempts,
            focusMaxSteps: focusMaxSteps
        )
        let sleepClosure: (Int) async throws -> Void = sleeper ?? { milliseconds in
            clock.advance(milliseconds)
            await Task.yield()
        }
        return CommandExecutor(
            driver: driver,
            options: options,
            now: { clock.now },
            sleep: sleepClosure,
            log: { level, source, message in log?.log(level, source, message) }
        )
    }

    func command(
        _ body: CommandBody,
        id: String = "c1",
        timeoutMs: Int = 1_000
    ) -> AgentCommand {
        AgentCommand(id: id, timeoutMs: timeoutMs, body: body)
    }

    func select(
        _ strategy: SelectorStrategy,
        _ value: String,
        index: Int = 0
    ) -> SelectorSpec {
        SelectorSpec(strategy: strategy, value: value, index: index)
    }

    func assertError(
        _ result: CommandResultData,
        _ code: AgentErrorCode,
        file: StaticString = #filePath,
        line: UInt = #line
    ) {
        XCTAssertFalse(result.ok, "expected a failure, got \(result)", file: file, line: line)
        XCTAssertEqual(result.error?.code, code, result.error?.message ?? "missing error", file: file, line: line)
    }
}
