import Foundation

// ---------------------------------------------------------------------------
// Wire model (spec TASK.md section 12, protocol v1)
//
// `AgentCore` is deliberately free of protobuf and XCTest imports: the gRPC
// side (the `AgentService` target) converts generated messages to and from
// these types, and everything below is testable with plain `swift test` and a
// fake `UIDriver`.
// ---------------------------------------------------------------------------

/// A target platform of this agent (spec 12.1: one UI-test bundle each).
public enum PlatformKind: String, Sendable, Equatable, CaseIterable {
    case ios
    case tvos

    public var isTv: Bool { self == .tvos }

    /// Parses the wire name (`"ios"`, `"tvos"`).
    public static func parse(_ raw: String) -> PlatformKind? {
        switch raw.trimmingCharacters(in: .whitespaces).lowercased() {
        case "ios": return .ios
        case "tvos": return .tvos
        default: return nil
        }
    }
}

/// Log levels of the `StreamLogs` stream (proto `LogLevel`).
public enum LogLevel: Int, Sendable, Comparable, Equatable {
    case debug = 0
    case info = 1
    case warn = 2
    case error = 3

    public static func < (lhs: LogLevel, rhs: LogLevel) -> Bool {
        lhs.rawValue < rhs.rawValue
    }
}

/// One log line of the agent-internal `StreamLogs` bus (spec 12.3).
public struct LogLine: Sendable, Equatable {
    public let unixMs: Int64
    public let level: LogLevel
    public let source: String
    public let message: String

    public init(unixMs: Int64, level: LogLevel, source: String, message: String) {
        self.unixMs = unixMs
        self.level = level
        self.source = source
        self.message = message
    }
}

// ---------------------------------------------------------------------------
// Selectors
// ---------------------------------------------------------------------------

/// Selector strategies Apple platforms support (spec 5.3 + 12.3).
public enum SelectorStrategy: String, Sendable, Equatable, CaseIterable {
    case accessibilityId
    case label
    case labelContains
    case predicate
    case iosType
}

/// A selector as understood by the Apple side (spec 5.3): strategy, value and
/// the 0-based `index` among the matches ("negative = from the end" cannot
/// reach the wire because the proto field is unsigned; the dispatcher still
/// handles out-of-range indexes by reporting `ELEMENT_NOT_FOUND`).
public struct SelectorSpec: Sendable, Equatable {
    public let strategy: SelectorStrategy
    public let value: String
    public let index: Int

    public init(strategy: SelectorStrategy, value: String, index: Int) {
        self.strategy = strategy
        self.value = value
        self.index = index
    }

    public func describe() -> String { "\(strategy.rawValue):\(value)#\(index)" }

    /// The raw selector drivers see: `index` selection happens in the
    /// executor (spec 11.3), so drivers always return *all* matches.
    public func lookup() -> SelectorSpec {
        SelectorSpec(strategy: strategy, value: value, index: 0)
    }

    /// Validates a selector built from the wire; throws `INVALID_SELECTOR`
    /// (spec 12.3 maps only predicate syntax errors separately at the driver).
    public static func make(strategy: SelectorStrategy, value: String, index: Int) throws -> SelectorSpec {
        if strategy == .iosType, AgentElementType(rawValue: value) == nil {
            throw AgentError.invalidSelector("unknown iOS element type '\(value)'")
        }
        return SelectorSpec(strategy: strategy, value: value, index: index)
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

public enum CheckKind: String, Sendable, Equatable, CaseIterable {
    case visible
    case hidden
    case enabled
    case disabled
    case focused
    case textEquals
    case textContains
    case count
}

public enum ActionKind: String, Sendable, Equatable, CaseIterable {
    case click
    case type
    case append
    case clear
    case longPress
    case focus
    case select
    case scroll
    case scrollTo
    case swipe
    case back
}

public enum Direction: String, Sendable, Equatable, CaseIterable {
    case up
    case down
    case left
    case right

    /// The `XCUIRemote` button that moves focus in this direction.
    public var remoteButton: RemoteButton {
        switch self {
        case .up: return .up
        case .down: return .down
        case .left: return .left
        case .right: return .right
        }
    }
}

/// Remote keys (spec 6.2, proto `RemoteKey`).
public enum RemoteKey: String, Sendable, Equatable, CaseIterable {
    case ok
    case enter
    case back
    case up
    case down
    case left
    case right
    case home
    case menu
    case play
    case pause
    case playPause = "playpause"
    case rewind
    case fastForward = "fastforward"
    case red
    case green
    case yellow
    case blue
    case backspace
    case num0
    case num1
    case num2
    case num3
    case num4
    case num5
    case num6
    case num7
    case num8
    case num9
    case volumeUp = "volumeup"
    case volumeDown = "volumedown"
    case mute
    case channelUp = "channelup"
    case channelDown = "channeldown"
    case stop
    case record
    case info
    case guide
    case exit
    case subtitle
    case audio
    case search
    case settings
}

public enum AppOpKind: String, Sendable, Equatable, CaseIterable {
    case launch
    case terminate
    case restart
    case background
}

public struct RunActionSpec: Sendable, Equatable {
    public let kind: ActionKind
    public let selector: SelectorSpec?
    public let text: String
    public let direction: Direction?
    public let count: Int
    public let durationMs: Int

    public init(
        kind: ActionKind,
        selector: SelectorSpec? = nil,
        text: String = "",
        direction: Direction? = nil,
        count: Int = 0,
        durationMs: Int = 0
    ) {
        self.kind = kind
        self.selector = selector
        self.text = text
        self.direction = direction
        self.count = count
        self.durationMs = durationMs
    }
}

public struct ValidateCheckSpec: Sendable, Equatable {
    public let kind: CheckKind
    public let selector: SelectorSpec?
    public let expectedText: String
    public let expectedCount: Int

    public init(kind: CheckKind, selector: SelectorSpec? = nil, expectedText: String = "", expectedCount: Int = 0) {
        self.kind = kind
        self.selector = selector
        self.expectedText = expectedText
        self.expectedCount = expectedCount
    }
}

public struct KeyPressSpec: Sendable, Equatable {
    public let key: RemoteKey
    public let repeatCount: Int
    public let intervalMs: Int

    public init(key: RemoteKey, repeatCount: Int, intervalMs: Int) {
        self.key = key
        self.repeatCount = repeatCount
        self.intervalMs = intervalMs
    }
}

public struct AppControlSpec: Sendable, Equatable {
    public let op: AppOpKind
    public let clearData: Bool

    public init(op: AppOpKind, clearData: Bool) {
        self.op = op
        self.clearData = clearData
    }
}

public enum CommandBody: Sendable, Equatable {
    case runAction(RunActionSpec)
    case validateCheck(ValidateCheckSpec)
    case keyPress(KeyPressSpec)
    case screenshot
    case appControl(AppControlSpec)
    case probe(ProbeCommandSpec)
    case cancel(target: String)
    case unspecified
}

/// One `Command` of the `Session` stream.
public struct AgentCommand: Sendable, Equatable {
    public let id: String
    public let timeoutMs: Int
    public let body: CommandBody

    public init(id: String, timeoutMs: Int, body: CommandBody) {
        self.id = id
        self.timeoutMs = timeoutMs
        self.body = body
    }
}

// ---------------------------------------------------------------------------
// Results and events
// ---------------------------------------------------------------------------

/// Wire error codes (proto `ErrorCode`); the bridge maps them 1:1.
public enum AgentErrorCode: String, Sendable, Equatable {
    case elementNotFound
    case timeout
    case notInteractable
    case staleElement
    case focusUnreachable
    case unsupportedAction
    case invalidSelector
    case checkFailed
    case appCrashed
    case cancelled
    case transport
    case protocolViolation
    case internalError
    case alreadyInUse
    case deviceUnavailable
    case unauthenticated
    case unverifiable
}

/// An executor-side failure that maps onto the wire `ErrorInfo`.
public struct AgentErrorReport: Sendable, Equatable {
    public let code: AgentErrorCode
    public let message: String
    public let details: [String: String]

    public init(code: AgentErrorCode, message: String, details: [String: String] = [:]) {
        self.code = code
        self.message = message
        self.details = details
    }
}

/// One `CommandResult` of the `Session` stream.
public struct CommandResultData: Sendable, Equatable {
    public let commandId: String
    public let ok: Bool
    public let durationMs: Int
    public let error: AgentErrorReport?
    public let observedText: String?
    public let observedCount: Int?
    public let screenshotPng: Data?
    public let probe: ProbeResultSpec?

    public init(
        commandId: String,
        ok: Bool,
        durationMs: Int,
        error: AgentErrorReport? = nil,
        observedText: String? = nil,
        observedCount: Int? = nil,
        screenshotPng: Data? = nil,
        probe: ProbeResultSpec? = nil
    ) {
        self.commandId = commandId
        self.ok = ok
        self.durationMs = durationMs
        self.error = error
        self.observedText = observedText
        self.observedCount = observedCount
        self.screenshotPng = screenshotPng
        self.probe = probe
    }
}

/// One `Event` of the `Session` stream.
public enum SessionEvent: Sendable, Equatable {
    case result(CommandResultData)
    case heartbeat(seq: Int, unixMs: Int64, currentCommandId: String)
    case closed(reason: String, error: AgentErrorReport?)

    /// Reasons of `SessionClosed` (mirrors the Android agent).
    public enum CloseReason: String {
        case clientClosed = "client_closed"
        case shutdown = "shutdown"
    }
}

// ---------------------------------------------------------------------------
// Handshake
// ---------------------------------------------------------------------------

/// A parsed `HandshakeRequest` (spec 7.1/7.2 rules 2, 3 and the token rule).
public struct HandshakeRequestData: Sendable, Equatable {
    public let protocolVersion: String
    public let masterVersion: String
    public let platform: PlatformKind?
    public let appId: String
    public let options: [String: String]
    public let authToken: String

    public init(
        protocolVersion: String,
        masterVersion: String,
        platform: PlatformKind?,
        appId: String,
        options: [String: String],
        authToken: String
    ) {
        self.protocolVersion = protocolVersion
        self.masterVersion = masterVersion
        self.platform = platform
        self.appId = appId
        self.options = options
        self.authToken = authToken
    }
}

/// Capabilities the Apple agent advertises (spec 7.1 `Capability`).
public enum AgentCapability: String, Sendable, Equatable {
    case logs
    case screenshot
    case touchGestures
    case dpadFocus
    case keyEvents
    case appControl
    case probeShowing
    case probePlayer
    case probeDrm

    /// iOS advertises the touch gesture set; tvOS the D-pad set. Both speak
    /// `KEY_EVENTS` (iOS only for `home`, spec 6.2) and both implement the
    /// probe kinds, with the evidence limits recorded in docs/PROBES.md.
    public static func forPlatform(_ platform: PlatformKind) -> [AgentCapability] {
        switch platform {
        case .ios:
            return [.logs, .screenshot, .touchGestures, .keyEvents, .appControl,
                    .probeShowing, .probePlayer, .probeDrm]
        case .tvos:
            return [.logs, .screenshot, .dpadFocus, .keyEvents, .appControl,
                    .probeShowing, .probePlayer, .probeDrm]
        }
    }
}

/// Device identity reported in the handshake.
public struct AgentDeviceInfo: Sendable, Equatable {
    public let model: String
    public let osName: String
    public let osVersion: String
    public let screenWidthPx: Int
    public let screenHeightPx: Int
    public let pixelRatio: Double
    public let runtime: String

    public init(
        model: String,
        osName: String,
        osVersion: String,
        screenWidthPx: Int,
        screenHeightPx: Int,
        pixelRatio: Double,
        runtime: String
    ) {
        self.model = model
        self.osName = osName
        self.osVersion = osVersion
        self.screenWidthPx = screenWidthPx
        self.screenHeightPx = screenHeightPx
        self.pixelRatio = pixelRatio
        self.runtime = runtime
    }
}

/// The decision of `AgentState.handshake(_:)`.
public struct HandshakeResult: Sendable, Equatable {
    public let accepted: Bool
    public let sessionId: String?
    public let agentVersion: String
    public let protocolVersion: String
    public let capabilities: [AgentCapability]
    public let deviceInfo: AgentDeviceInfo?
    public let error: AgentErrorReport?

    public init(
        accepted: Bool,
        sessionId: String?,
        agentVersion: String,
        protocolVersion: String,
        capabilities: [AgentCapability],
        deviceInfo: AgentDeviceInfo?,
        error: AgentErrorReport?
    ) {
        self.accepted = accepted
        self.sessionId = sessionId
        self.agentVersion = agentVersion
        self.protocolVersion = protocolVersion
        self.capabilities = capabilities
        self.deviceInfo = deviceInfo
        self.error = error
    }
}
