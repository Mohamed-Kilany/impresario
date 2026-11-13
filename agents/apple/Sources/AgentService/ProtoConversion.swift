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
        response.probeSources = probeSources()
        return response
    }

    /// Per-source availability advertised in the handshake (TASK-3 section
    /// 7.1). The optional in-app hook is always reachable when the app links
    /// the SDK; its absence at runtime leaves only the affected claim
    /// unverifiable.
    static func probeSources() -> [Impresario_V1_ProbeSourceInfo] {
        func source(
            _ name: String,
            _ available: Bool,
            _ evidenceClass: String,
            _ reason: String
        ) -> Impresario_V1_ProbeSourceInfo {
            var info = Impresario_V1_ProbeSourceInfo()
            info.source = name
            info.available = available
            info.`class` = evidenceClass
            info.reason = reason
            return info
        }
        return [
            source("a11y_tree", true, "system", "XCUITest accessibility tree"),
            source("screenshot_region", true, "authoritative", "XCUIScreen screenshot region analysis"),
            source("app_hook", true, "authoritative", "ImpresarioHook debug SDK over loopback"),
            source("audio_session", false, "system", "no public API for another app's audio session"),
            source("now_playing_ui", false, "system", "not implemented on iOS/tvOS"),
            source("syslog", false, "system", "host-assisted syslog collector not implemented"),
            source("dvt_sysmon", false, "system", "host-assisted DVT collector not implemented"),
            source("dvt_graphics", false, "system", "host-assisted DVT collector not implemented"),
            source("dvt_network", false, "system", "host-assisted DVT collector not implemented"),
            source("pcapd", false, "system", "host-assisted pcapd collector not implemented"),
            source("net_flow", false, "system", "network metadata collector not implemented"),
        ]
    }

    public static func capability(_ raw: AgentCapability) -> Impresario_V1_Capability {
        switch raw {
        case .logs: return .logs
        case .screenshot: return .screenshot
        case .touchGestures: return .touchGestures
        case .dpadFocus: return .dpadFocus
        case .keyEvents: return .keyEvents
        case .appControl: return .appControl
        case .probeShowing: return .probeShowing
        case .probePlayer: return .probePlayer
        case .probeDrm: return .probeDrm
        case .inspector: return .inspector
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
        case .unverifiable: return .unverifiable
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
        case .unverifiable: return .unverifiable
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
        case .probe(let probe):
            body = .probe(try probeCommand(probe))
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
        let key = try remoteKey(raw.key)
        return KeyPressSpec(
            key: key,
            repeatCount: Int(raw.`repeat`),
            intervalMs: Int(raw.intervalMs)
        )
    }

    /// The wire `RemoteKey` → the `AgentCore` key vocabulary. Shared by the
    /// command path and the `Inspector.Input` key action.
    public static func remoteKey(_ raw: Impresario_V1_RemoteKey) throws -> RemoteKey {
        switch raw {
        case .keyOk: return .ok
        case .keyEnter: return .enter
        case .keyBack: return .back
        case .keyUp: return .up
        case .keyDown: return .down
        case .keyLeft: return .left
        case .keyRight: return .right
        case .keyHome: return .home
        case .keyMenu: return .menu
        case .keyPlay: return .play
        case .keyPause: return .pause
        case .keyPlaypause: return .playPause
        case .keyRewind: return .rewind
        case .keyFastforward: return .fastForward
        case .keyRed: return .red
        case .keyGreen: return .green
        case .keyYellow: return .yellow
        case .keyBlue: return .blue
        case .keyBackspace: return .backspace
        case .keyNum0: return .num0
        case .keyNum1: return .num1
        case .keyNum2: return .num2
        case .keyNum3: return .num3
        case .keyNum4: return .num4
        case .keyNum5: return .num5
        case .keyNum6: return .num6
        case .keyNum7: return .num7
        case .keyNum8: return .num8
        case .keyNum9: return .num9
        case .keyVolumeup: return .volumeUp
        case .keyVolumedown: return .volumeDown
        case .keyMute: return .mute
        case .keyChannelup: return .channelUp
        case .keyChanneldown: return .channelDown
        case .keyStop: return .stop
        case .keyRecord: return .record
        case .keyInfo: return .info
        case .keyGuide: return .guide
        case .keyExit: return .exit
        case .keySubtitle: return .subtitle
        case .keyAudio: return .audio
        case .keySearch: return .search
        case .keySettings: return .settings
        case .unspecified, .UNRECOGNIZED:
            throw AgentError.protocolError("the remote key is unspecified or unknown")
        }
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

    // ------------------------------------------------------------------
    // probes (TASK-3 section 5, D-26/D-28)
    // ------------------------------------------------------------------

    public static func probeCommand(_ raw: Impresario_V1_ProbeCommand) throws -> ProbeCommandSpec {
        let kind: ProbeKindSpec
        switch raw.kind {
        case .showing: kind = .showing
        case .player: kind = .player
        case .drm: kind = .drm
        case .unspecified, .UNRECOGNIZED:
            throw AgentError.protocolError("the probe kind is unspecified or unknown")
        }
        return ProbeCommandSpec(
            kind: kind,
            selector: raw.hasSelector ? try selector(raw.selector) : nil,
            claims: raw.claims.map { ProbeClaimSpec(claim: $0.claim, negated: $0.negated) },
            assertions: raw.assertions.map {
                ProbeAssertionSpec(
                    property: $0.property,
                    operator: $0.operator,
                    value: $0.value,
                    negated: $0.negated
                )
            },
            minConfidence: confidenceName(raw.minConfidence, default: .corroborated),
            windowMs: Int(raw.windowMs),
            requireDecoded: raw.requireDecoded,
            requireSource: raw.requireSource,
            minCoverage: raw.minCoverage
        )
    }

    public static func probeResult(_ raw: ProbeResultSpec) -> Impresario_V1_ProbeResult {
        var result = Impresario_V1_ProbeResult()
        result.verdicts = raw.verdicts.map { verdict in
            var item = Impresario_V1_ClaimVerdict()
            item.claim = verdict.claim
            item.verdict = verdictState(verdict.verdict)
            item.confidence = probeConfidence(verdict.confidence)
            item.reason = verdict.reason ?? ""
            return item
        }
        result.evidence = raw.evidence.map { item in
            var evidence = Impresario_V1_ProbeEvidence()
            evidence.source = item.source.rawValue
            evidence.kind = item.kind
            evidence.observationType = item.observationType
            evidence.observation = item.observation
            evidence.atUnixMs = UInt64(clamping: max(0, item.atUnixMs))
            evidence.windowMs = UInt32(clamping: max(0, item.windowMs))
            evidence.`class` = evidenceClass(item.evidenceClass)
            evidence.supports = item.supports
            evidence.contradicts = item.contradicts
            return evidence
        }
        result.confidence = probeConfidence(raw.confidence)
        result.unverifiableReason = raw.unverifiableReason ?? ""
        result.observed = raw.observed
        result.windowMs = UInt32(clamping: max(0, raw.windowMs))
        return result
    }

    public static func probeConfidence(_ raw: String) -> Impresario_V1_ProbeConfidence {
        switch raw {
        case "authoritative": return .authoritative
        case "corroborated": return .corroborated
        case "inferred": return .inferred
        default: return .none
        }
    }

    private static func confidenceName(
        _ raw: Impresario_V1_ProbeConfidence,
        default fallback: Impresario_V1_ProbeConfidence
    ) -> String {
        let value = raw == .none ? fallback : raw
        switch value {
        case .authoritative: return "authoritative"
        case .corroborated: return "corroborated"
        case .inferred: return "inferred"
        case .none, .UNRECOGNIZED: return confidenceName(fallback, default: .corroborated)
        }
    }

    public static func evidenceClass(_ raw: ProbeEvidenceClassSpec) -> Impresario_V1_EvidenceClass {
        switch raw {
        case .authoritative: return .authoritative
        case .system: return .system
        case .heuristic: return .heuristic
        }
    }

    public static func verdictState(_ raw: String) -> Impresario_V1_VerdictState {
        switch raw {
        case "pass": return .pass
        case "fail": return .fail
        default: return .unverifiable
        }
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
    // inspector (TASK-INSPECTOR)
    // ------------------------------------------------------------------

    public static func inspectorRect(_ raw: InspectorRectData) -> Impresario_V1_InspectorRect {
        var rect = Impresario_V1_InspectorRect()
        rect.x = Float(raw.x)
        rect.y = Float(raw.y)
        rect.w = Float(raw.w)
        rect.h = Float(raw.h)
        return rect
    }

    public static func inspectorNode(_ raw: InspectorNodeData) -> Impresario_V1_InspectorNode {
        var node = Impresario_V1_InspectorNode()
        node.id = raw.id
        node.parentID = raw.parentId
        node.role = raw.role
        node.name = raw.name
        node.text = raw.text
        node.testID = raw.testId
        node.rawID = raw.rawId
        node.bounds = inspectorRect(raw.bounds)
        node.viewportBounds = inspectorRect(raw.viewportBounds)
        node.focused = raw.focused
        node.enabled = raw.enabled
        node.selected = raw.selected
        if let checked = raw.checked {
            node.checked = checked
        }
        if let expanded = raw.expanded {
            node.expanded = expanded
        }
        if let pressed = raw.pressed {
            node.pressed = pressed
        }
        node.readonly = raw.readonly
        node.required = raw.required
        node.hidden = raw.hidden
        node.secure = raw.secure
        node.systemUi = raw.systemUi
        node.scrollable = raw.scrollable
        node.canScrollBackward = raw.canScrollBackward
        node.canScrollForward = raw.canScrollForward
        node.scrollAxis = raw.scrollAxis
        node.childrenHiddenCount = raw.childrenHiddenCount
        node.raw = raw.raw
        node.value = raw.value
        node.label = raw.label
        node.description_p = raw.description
        node.placeholder = raw.placeholder
        node.alt = raw.alt
        node.title = raw.title
        node.cssSelector = raw.cssSelector
        node.xpath = raw.xpath
        node.visibleFraction = Float(raw.visibleFraction)
        node.escapesClip = raw.escapesClip
        node.depth = raw.depth
        node.textTruncated = raw.textTruncated
        return node
    }

    /// One `Inspector.Input` request → the driver action. An unspecified or
    /// unknown kind is a protocol violation (mirrors the Android agent).
    public static func inspectorInput(
        _ raw: Impresario_V1_InspectorInputRequest
    ) throws -> InspectorInputAction {
        switch raw.kind {
        case .tap:
            return .tap(nodeId: raw.nodeID, x: Double(raw.x), y: Double(raw.y))
        case .scroll:
            guard let direction = direction(raw.direction) else {
                throw AgentError.protocolError("the inspector scroll direction is unspecified")
            }
            return .scroll(
                direction: direction,
                count: max(1, Int(raw.count)),
                nodeId: raw.nodeID,
                x: Double(raw.x),
                y: Double(raw.y)
            )
        case .key:
            return .key(try remoteKey(raw.key))
        case .type:
            return .type(nodeId: raw.nodeID, text: raw.text)
        case .swipe:
            return .swipe(
                x1: Double(raw.x),
                y1: Double(raw.y),
                x2: Double(raw.endX),
                y2: Double(raw.endY),
                durationMs: Int(raw.durationMs)
            )
        case .unspecified, .UNRECOGNIZED:
            throw AgentError.protocolError("the inspector input kind is unspecified or unknown")
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
        if let probe = raw.probe {
            result.probe = probeResult(probe)
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
