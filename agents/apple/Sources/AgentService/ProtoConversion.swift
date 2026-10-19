import AgentCore
import Foundation
import ImpresarioWire

/// The generated protobuf messages ↔ the protobuf-free `AgentCore` model
/// (spec 12.1). Every conversion lives here, so `AgentCore` stays testable
/// without gRPC and the service layer holds no parsing rules of its own.
public enum ProtoConversion {
    // ------------------------------------------------------------------
    // handshake
    // ------------------------------------------------------------------

    public static func handshakeRequest(
        _ raw: Impresario_V1_HandshakeRequest,
        metadataToken: String?
    ) -> HandshakeRequestData {
        HandshakeRequestData(
            protocolVersion: raw.protocolVersion,
            masterVersion: raw.masterVersion,
            platform: platform(from: raw.platform),
            appId: raw.appID,
            options: raw.options,
            authToken: raw.authToken.isEmpty ? (metadataToken ?? "") : raw.authToken
        )
    }

    public static func platform(from raw: Impresario_V1_Platform) -> PlatformKind? {
        switch raw {
        case .ios: return .ios
        case .tvos: return .tvos
        default: return nil
        }
    }

    public static func platform(_ raw: PlatformKind) -> Impresario_V1_Platform {
        switch raw {
        case .ios: return .ios
        case .tvos: return .tvos
        }
    }

    public static func handshakeResponse(_ raw: HandshakeResult) -> Impresario_V1_HandshakeResponse {
        var response = Impresario_V1_HandshakeResponse()
        response.accepted = raw.accepted
        response.sessionID = raw.sessionId ?? ""
        response.agentVersion = raw.agentVersion
        response.protocolVersion = raw.protocolVersion
        response.capabilities = raw.capabilities.map(capability)
        if let device = raw.deviceInfo {
            response.device = deviceInfo(device)
        }
        if let error = raw.error {
            response.error = errorInfo(error)
        }
        return response
    }

    public static func capability(_ raw: AgentCapability) -> Impresario_V1_Capability {
        switch raw {
        case .logs: return .logs
        case .screenshot: return .screenshot
        case .touchGestures: return .touchGestures
        case .dpadFocus: return .dpadFocus
        case .keyEvents: return .keyEvents
        case .appControl: return .appControl
        }
    }

    public static func deviceInfo(_ raw: AgentDeviceInfo) -> Impresario_V1_DeviceInfo {
        var info = Impresario_V1_DeviceInfo()
        info.model = raw.model
        info.osName = raw.osName
        info.osVersion = raw.osVersion
        info.screenWidthPx = UInt32(clamping: max(0, raw.screenWidthPx))
        info.screenHeightPx = UInt32(clamping: max(0, raw.screenHeightPx))
        info.pixelRatio = Float(raw.pixelRatio)
        info.runtime = raw.runtime
        return info
    }

    // ------------------------------------------------------------------
    // errors
    // ------------------------------------------------------------------

    public static func errorCode(_ raw: AgentErrorCode) -> Impresario_V1_ErrorCode {
        switch raw {
        case .elementNotFound: return .elementNotFound
        case .timeout: return .timeout
        case .notInteractable: return .notInteractable
        case .staleElement: return .staleElement
        case .focusUnreachable: return .focusUnreachable
        case .unsupportedAction: return .unsupportedAction
        case .invalidSelector: return .invalidSelector
        case .checkFailed: return .checkFailed
        case .appCrashed: return .appCrashed
        case .cancelled: return .cancelled
        case .transport: return .transport
        case .protocolViolation: return .protocol
        case .internalError: return .internal
        case .alreadyInUse: return .alreadyInUse
        case .deviceUnavailable: return .deviceUnavailable
        case .unauthenticated: return .unauthenticated
        }
    }

    public static func errorCode(_ raw: Impresario_V1_ErrorCode) -> AgentErrorCode {
        switch raw {
        case .elementNotFound: return .elementNotFound
        case .timeout: return .timeout
        case .notInteractable: return .notInteractable
        case .staleElement: return .staleElement
        case .focusUnreachable: return .focusUnreachable
        case .unsupportedAction: return .unsupportedAction
        case .invalidSelector: return .invalidSelector
        case .checkFailed: return .checkFailed
        case .appCrashed: return .appCrashed
        case .cancelled: return .cancelled
        case .transport: return .transport
        case .protocol: return .protocolViolation
        case .internal: return .internalError
        case .alreadyInUse: return .alreadyInUse
        case .deviceUnavailable: return .deviceUnavailable
        case .unauthenticated: return .unauthenticated
        case .unspecified, .UNRECOGNIZED: return .internalError
        }
    }

    public static func errorInfo(_ raw: AgentErrorReport) -> Impresario_V1_ErrorInfo {
        var info = Impresario_V1_ErrorInfo()
        info.code = errorCode(raw.code)
        info.message = raw.message
        info.details = raw.details
        return info
    }

    public static func errorReport(_ raw: Impresario_V1_ErrorInfo) -> AgentErrorReport {
        AgentErrorReport(
            code: errorCode(raw.code),
            message: raw.message,
            details: raw.details
        )
    }

    // ------------------------------------------------------------------
    // commands
    // ------------------------------------------------------------------

    public static func command(_ raw: Impresario_V1_Command) throws -> AgentCommand {
        let body: CommandBody
        switch raw.body {
        case .runAction(let action):
            body = .runAction(try runAction(action))
        case .validateCheck(let check):
            body = .validateCheck(try validateCheck(check))
        case .keyPress(let press):
            body = .keyPress(try keyPress(press))
        case .screenshot:
            body = .screenshot
        case .appControl(let control):
            body = .appControl(try appControl(control))
        case .cancel(let cancel):
            body = .cancel(target: cancel.targetCommandID)
        case nil:
            throw AgentError.protocolError("the command has no body")
        }
        return AgentCommand(
            id: raw.commandID,
            timeoutMs: Int(raw.timeoutMs),
            body: body
        )
    }

    public static func selector(_ raw: Impresario_V1_Selector) throws -> SelectorSpec {
        let strategy: SelectorStrategy
        switch raw.strategy {
        case .accessibilityID: strategy = .accessibilityId
        case .label: strategy = .label
        case .labelContains: strategy = .labelContains
        case .predicate: strategy = .predicate
        case .iosType: strategy = .iosType
        case .css, .xpath, .text, .textContains, .androidID, .androidDesc, .androidClass:
            throw AgentError.invalidSelector(
                "selector strategy \(raw.strategy) is not supported on Apple platforms; "
                    + "use accessibility_id, label, label_contains, predicate or ios_type (docs/PARITY.md)"
            )
        case .unspecified, .UNRECOGNIZED:
            throw AgentError.invalidSelector("the selector strategy is unspecified or unknown")
        }
        return try SelectorSpec.make(strategy: strategy, value: raw.value, index: Int(raw.index))
    }

    public static func runAction(_ raw: Impresario_V1_RunAction) throws -> RunActionSpec {
        let kind: ActionKind
        switch raw.type {
        case .click: kind = .click
        case .type: kind = .type
        case .append: kind = .append
        case .clear: kind = .clear
        case .longPress: kind = .longPress
        case .focus: kind = .focus
        case .select: kind = .select
        case .scroll: kind = .scroll
        case .scrollTo: kind = .scrollTo
        case .swipe: kind = .swipe
        case .back: kind = .back
        case .unspecified, .UNRECOGNIZED:
            throw AgentError.protocolError("the action type is unspecified or unknown")
        }
        return RunActionSpec(
            kind: kind,
            selector: raw.hasSelector ? try selector(raw.selector) : nil,
            text: raw.text,
            direction: direction(raw.direction),
            count: Int(raw.count),
            durationMs: Int(raw.durationMs)
        )
    }

    public static func validateCheck(_ raw: Impresario_V1_ValidateCheck) throws -> ValidateCheckSpec {
        let kind: CheckKind
        switch raw.kind {
        case .visible: kind = .visible
        case .hidden: kind = .hidden
        case .enabled: kind = .enabled
        case .disabled: kind = .disabled
        case .focused: kind = .focused
        case .textEquals: kind = .textEquals
        case .textContainsCheck: kind = .textContains
        case .count: kind = .count
        case .unspecified, .UNRECOGNIZED:
            throw AgentError.protocolError("the check kind is unspecified or unknown")
        }
        return ValidateCheckSpec(
            kind: kind,
            selector: raw.hasSelector ? try selector(raw.selector) : nil,
            expectedText: raw.expectedText,
            expectedCount: Int(raw.expectedCount)
        )
    }

    public static func keyPress(_ raw: Impresario_V1_KeyPress) throws -> KeyPressSpec {
        let key: RemoteKey
        switch raw.key {
        case .keyOk: key = .ok
        case .keyEnter: key = .enter
        case .keyBack: key = .back
        case .keyUp: key = .up
        case .keyDown: key = .down
        case .keyLeft: key = .left
        case .keyRight: key = .right
        case .keyHome: key = .home
        case .keyMenu: key = .menu
        case .keyPlay: key = .play
        case .keyPause: key = .pause
        case .keyPlaypause: key = .playPause
        case .keyRewind: key = .rewind
        case .keyFastforward: key = .fastForward
        case .keyRed: key = .red
        case .keyGreen: key = .green
        case .keyYellow: key = .yellow
        case .keyBlue: key = .blue
        case .keyBackspace: key = .backspace
        case .unspecified, .UNRECOGNIZED:
            throw AgentError.protocolError("the remote key is unspecified or unknown")
        }
        return KeyPressSpec(
            key: key,
            repeatCount: Int(raw.`repeat`),
            intervalMs: Int(raw.intervalMs)
        )
    }

    public static func appControl(_ raw: Impresario_V1_AppControl) throws -> AppControlSpec {
        let op: AppOpKind
        switch raw.op {
        case .launch: op = .launch
        case .terminate: op = .terminate
        case .restart: op = .restart
        case .background: op = .background
        case .unspecified, .UNRECOGNIZED:
            throw AgentError.protocolError("the app op is unspecified or unknown")
        }
        return AppControlSpec(op: op, clearData: raw.clearData_p)
    }

    public static func direction(_ raw: Impresario_V1_Direction) -> Direction? {
        switch raw {
        case .up: return .up
        case .down: return .down
        case .left: return .left
        case .right: return .right
        case .unspecified, .UNRECOGNIZED: return nil
        }
    }

    // ------------------------------------------------------------------
    // results, events, logs
    // ------------------------------------------------------------------

    public static func commandResult(_ raw: CommandResultData) -> Impresario_V1_CommandResult {
        var result = Impresario_V1_CommandResult()
        result.commandID = raw.commandId
        result.ok = raw.ok
        result.durationMs = UInt32(clamping: max(0, raw.durationMs))
        if let error = raw.error {
            result.error = errorInfo(error)
        }
        if let png = raw.screenshotPng {
            result.screenshotPng = png
        }
        if let text = raw.observedText {
            result.observedText = text
        }
        if let count = raw.observedCount {
            result.observedCount = UInt32(clamping: max(0, count))
        }
        return result
    }

    public static func event(_ raw: SessionEvent) -> Impresario_V1_Event {
        var event = Impresario_V1_Event()
        switch raw {
        case .result(let result):
            event.result = commandResult(result)
        case .heartbeat(let seq, let unixMs, let currentCommandId):
            var heartbeat = Impresario_V1_Heartbeat()
            heartbeat.seq = UInt64(clamping: max(0, seq))
            heartbeat.unixMs = UInt64(clamping: max(0, unixMs))
            heartbeat.currentCommandID = currentCommandId
            event.heartbeat = heartbeat
        case .closed(let reason, let error):
            var closed = Impresario_V1_SessionClosed()
            closed.reason = reason
            if let error {
                closed.error = errorInfo(error)
            }
            event.closed = closed
        }
        return event
    }

    public static func logLevel(from raw: Impresario_V1_LogLevel) -> AgentCore.LogLevel {
        switch raw {
        case .debug: return .debug
        case .info: return .info
        case .warn: return .warn
        case .error: return .error
        case .unspecified, .UNRECOGNIZED: return .debug
        }
    }

    public static func logLevel(_ raw: AgentCore.LogLevel) -> Impresario_V1_LogLevel {
        switch raw {
        case .debug: return .debug
        case .info: return .info
        case .warn: return .warn
        case .error: return .error
        }
    }

    public static func logEntry(_ raw: LogLine) -> Impresario_V1_LogEntry {
        var entry = Impresario_V1_LogEntry()
        entry.unixMs = raw.unixMs
        entry.level = logLevel(raw.level)
        entry.source = raw.source
        entry.message = raw.message
        return entry
    }
}
