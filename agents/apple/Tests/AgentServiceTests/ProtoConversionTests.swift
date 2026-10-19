import AgentCore
import AgentService
import ImpresarioWire
import XCTest

/// Pure conversion tests: the bridge between the generated protobuf messages
/// and the protobuf-free `AgentCore` model (spec 12.1).
final class ProtoConversionTests: XCTestCase {
    // MARK: selectors

    func testSupportedSelectorStrategies() throws {
        let cases: [(Impresario_V1_SelectorStrategy, SelectorStrategy)] = [
            (.accessibilityID, .accessibilityId),
            (.label, .label),
            (.labelContains, .labelContains),
            (.predicate, .predicate),
            (.iosType, .iosType),
        ]
        for (proto, expected) in cases {
            var raw = Impresario_V1_Selector()
            raw.strategy = proto
            raw.value = expected == .iosType ? "button" : "login"
            raw.index = 2
            let spec = try ProtoConversion.selector(raw)
            XCTAssertEqual(spec.strategy, expected)
            XCTAssertEqual(spec.index, 2)
            XCTAssertEqual(spec.value, expected == .iosType ? "button" : "login")
        }
    }

    func testAndroidAndWebStrategiesAreInvalidOnApple() {
        for proto: Impresario_V1_SelectorStrategy in [.css, .xpath, .text, .textContains, .androidID, .androidDesc, .androidClass] {
            var raw = Impresario_V1_Selector()
            raw.strategy = proto
            raw.value = "#login"
            XCTAssertThrowsError(try ProtoConversion.selector(raw)) { error in
                guard let agentError = error as? AgentError else {
                    return XCTFail("expected an AgentError, got \(error)")
                }
                XCTAssertEqual(agentError.code, .invalidSelector)
                XCTAssertTrue(agentError.message.contains("PARITY.md"), agentError.message)
            }
        }
    }

    func testUnspecifiedSelectorStrategyIsInvalid() {
        XCTAssertThrowsError(try ProtoConversion.selector(Impresario_V1_Selector())) { error in
            XCTAssertEqual((error as? AgentError)?.code, .invalidSelector)
        }
    }

    func testUnknownIosTypeIsInvalid() {
        var raw = Impresario_V1_Selector()
        raw.strategy = .iosType
        raw.value = "not-a-type"
        XCTAssertThrowsError(try ProtoConversion.selector(raw)) { error in
            XCTAssertEqual((error as? AgentError)?.code, .invalidSelector)
        }
    }

    // MARK: commands

    func testEveryActionTypeMaps() throws {
        let cases: [(Impresario_V1_ActionType, ActionKind)] = [
            (.click, .click),
            (.type, .type),
            (.append, .append),
            (.clear, .clear),
            (.longPress, .longPress),
            (.focus, .focus),
            (.select, .select),
            (.scroll, .scroll),
            (.scrollTo, .scrollTo),
            (.swipe, .swipe),
            (.back, .back),
        ]
        for (proto, expected) in cases {
            var raw = Impresario_V1_RunAction()
            raw.type = proto
            raw.text = "x"
            raw.count = 3
            raw.durationMs = 750
            raw.direction = .down
            let spec = try ProtoConversion.runAction(raw)
            XCTAssertEqual(spec.kind, expected)
            XCTAssertEqual(spec.text, "x")
            XCTAssertEqual(spec.count, 3)
            XCTAssertEqual(spec.durationMs, 750)
            XCTAssertEqual(spec.direction, .down)
            XCTAssertNil(spec.selector, "no selector was set")
        }
    }

    func testUnspecifiedActionTypeIsAProtocolViolation() {
        XCTAssertThrowsError(try ProtoConversion.runAction(Impresario_V1_RunAction())) { error in
            XCTAssertEqual((error as? AgentError)?.code, .protocolViolation)
        }
    }

    func testActionKeepsTheRawDefaultsSoTheExecutorAppliesThem() throws {
        // The executor defaults count to 1 and duration to 1000 ms; the bridge
        // must not invent values of its own (spec 12.3).
        var raw = Impresario_V1_RunAction()
        raw.type = .scroll
        let spec = try ProtoConversion.runAction(raw)
        XCTAssertEqual(spec.count, 0)
        XCTAssertEqual(spec.durationMs, 0)
        XCTAssertNil(spec.direction)
    }

    func testEveryCheckKindMaps() throws {
        let cases: [(Impresario_V1_CheckKind, CheckKind)] = [
            (.visible, .visible),
            (.hidden, .hidden),
            (.enabled, .enabled),
            (.disabled, .disabled),
            (.focused, .focused),
            (.textEquals, .textEquals),
            (.textContainsCheck, .textContains),
            (.count, .count),
        ]
        for (proto, expected) in cases {
            var raw = Impresario_V1_ValidateCheck()
            raw.kind = proto
            raw.expectedText = "yes"
            raw.expectedCount = 4
            let spec = try ProtoConversion.validateCheck(raw)
            XCTAssertEqual(spec.kind, expected)
            XCTAssertEqual(spec.expectedText, "yes")
            XCTAssertEqual(spec.expectedCount, 4)
        }
        XCTAssertThrowsError(try ProtoConversion.validateCheck(Impresario_V1_ValidateCheck())) { error in
            XCTAssertEqual((error as? AgentError)?.code, .protocolViolation)
        }
    }

    func testEveryRemoteKeyMaps() throws {
        let cases: [(Impresario_V1_RemoteKey, RemoteKey)] = [
            (.keyOk, .ok),
            (.keyEnter, .enter),
            (.keyBack, .back),
            (.keyUp, .up),
            (.keyDown, .down),
            (.keyLeft, .left),
            (.keyRight, .right),
            (.keyHome, .home),
            (.keyMenu, .menu),
            (.keyPlay, .play),
            (.keyPause, .pause),
            (.keyPlaypause, .playPause),
            (.keyRewind, .rewind),
            (.keyFastforward, .fastForward),
            (.keyRed, .red),
            (.keyGreen, .green),
            (.keyYellow, .yellow),
            (.keyBlue, .blue),
            (.keyBackspace, .backspace),
        ]
        for (proto, expected) in cases {
            var raw = Impresario_V1_KeyPress()
            raw.key = proto
            raw.repeat = 2
            raw.intervalMs = 50
            let spec = try ProtoConversion.keyPress(raw)
            XCTAssertEqual(spec.key, expected)
            XCTAssertEqual(spec.repeatCount, 2)
            XCTAssertEqual(spec.intervalMs, 50)
        }
        XCTAssertThrowsError(try ProtoConversion.keyPress(Impresario_V1_KeyPress())) { error in
            XCTAssertEqual((error as? AgentError)?.code, .protocolViolation)
        }
    }

    func testAppControlCarriesClearData() throws {
        var raw = Impresario_V1_AppControl()
        raw.op = .restart
        raw.clearData_p = true
        let spec = try ProtoConversion.appControl(raw)
        XCTAssertEqual(spec.op, .restart)
        XCTAssertTrue(spec.clearData)

        let ops: [(Impresario_V1_AppOp, AppOpKind)] = [
            (.launch, .launch),
            (.terminate, .terminate),
            (.restart, .restart),
            (.background, .background),
        ]
        for (proto, expected) in ops {
            raw.op = proto
            XCTAssertEqual(try ProtoConversion.appControl(raw).op, expected)
        }
        XCTAssertThrowsError(try ProtoConversion.appControl(Impresario_V1_AppControl())) { error in
            XCTAssertEqual((error as? AgentError)?.code, .protocolViolation)
        }
    }

    func testCommandRequiresABody() {
        var raw = Impresario_V1_Command()
        raw.commandID = "c-1"
        XCTAssertThrowsError(try ProtoConversion.command(raw)) { error in
            XCTAssertEqual((error as? AgentError)?.code, .protocolViolation)
        }
    }

    func testCancelCarriesTheTarget() throws {
        var cancel = Impresario_V1_Cancel()
        cancel.targetCommandID = "c-9"
        var raw = Impresario_V1_Command()
        raw.commandID = "c-10"
        raw.body = .cancel(cancel)
        let command = try ProtoConversion.command(raw)
        XCTAssertEqual(command.body, .cancel(target: "c-9"))
    }

    func testScreenshotCommandHasNoBody() throws {
        var raw = Impresario_V1_Command()
        raw.commandID = "c-11"
        raw.body = .screenshot(Impresario_V1_Screenshot())
        XCTAssertEqual(try ProtoConversion.command(raw).body, .screenshot)
    }

    // MARK: errors, results, events

    func testErrorCodesRoundTrip() {
        let codes: [AgentErrorCode] = [
            .elementNotFound, .timeout, .notInteractable, .staleElement,
            .focusUnreachable, .unsupportedAction, .invalidSelector, .checkFailed,
            .appCrashed, .cancelled, .transport, .protocolViolation, .internalError,
            .alreadyInUse, .deviceUnavailable, .unauthenticated,
        ]
        for code in codes {
            let info = ProtoConversion.errorInfo(AgentErrorReport(code: code, message: "m", details: ["k": "v"]))
            let back = ProtoConversion.errorReport(info)
            XCTAssertEqual(back.code, code)
            XCTAssertEqual(back.message, "m")
            XCTAssertEqual(back.details, ["k": "v"])
        }
    }

    func testResultClampsNegativeNumbersAndKeepsObservedValues() {
        let raw = CommandResultData(
            commandId: "c-1",
            ok: false,
            durationMs: -5,
            error: AgentErrorReport(code: .checkFailed, message: "nope"),
            observedText: "hello",
            observedCount: -1
        )
        let result = ProtoConversion.commandResult(raw)
        XCTAssertEqual(result.commandID, "c-1")
        XCTAssertFalse(result.ok)
        XCTAssertEqual(result.durationMs, 0)
        XCTAssertEqual(result.error.code, .checkFailed)
        XCTAssertEqual(result.observedText, "hello")
        XCTAssertEqual(result.observedCount, 0)
        XCTAssertTrue(result.screenshotPng.isEmpty)
    }

    func testEventsMap() {
        let result = ProtoConversion.event(
            .result(CommandResultData(commandId: "c-1", ok: true, durationMs: 12))
        )
        XCTAssertEqual(result.result.commandID, "c-1")
        XCTAssertEqual(result.result.durationMs, 12)

        let heartbeat = ProtoConversion.event(.heartbeat(seq: 3, unixMs: 42, currentCommandId: "c-2"))
        XCTAssertEqual(heartbeat.heartbeat.seq, 3)
        XCTAssertEqual(heartbeat.heartbeat.unixMs, 42)
        XCTAssertEqual(heartbeat.heartbeat.currentCommandID, "c-2")

        let closed = ProtoConversion.event(
            .closed(reason: "shutdown", error: AgentErrorReport(code: .cancelled, message: "bye"))
        )
        XCTAssertEqual(closed.closed.reason, "shutdown")
        XCTAssertEqual(closed.closed.error.code, .cancelled)
    }

    func testLogLevelsMapWithUnspecifiedAsDebug() {
        XCTAssertEqual(ProtoConversion.logLevel(from: .unspecified), .debug)
        XCTAssertEqual(ProtoConversion.logLevel(from: .debug), .debug)
        XCTAssertEqual(ProtoConversion.logLevel(from: .info), .info)
        XCTAssertEqual(ProtoConversion.logLevel(from: .warn), .warn)
        XCTAssertEqual(ProtoConversion.logLevel(from: .error), .error)
        XCTAssertEqual(ProtoConversion.logLevel(.warn), .warn)

        let entry = ProtoConversion.logEntry(
            LogLine(unixMs: 7, level: .error, source: "agent", message: "boom")
        )
        XCTAssertEqual(entry.unixMs, 7)
        XCTAssertEqual(entry.level, .error)
        XCTAssertEqual(entry.source, "agent")
        XCTAssertEqual(entry.message, "boom")
    }

    func testCapabilitiesAndPlatformsMap() {
        XCTAssertEqual(ProtoConversion.platform(.ios), .ios)
        XCTAssertEqual(ProtoConversion.platform(.tvos), .tvos)
        XCTAssertNil(ProtoConversion.platform(from: .android))
        XCTAssertNil(ProtoConversion.platform(from: .unspecified))
        XCTAssertEqual(ProtoConversion.platform(.ios), .ios)

        let capabilities = AgentCapability.forPlatform(.tvos).map(ProtoConversion.capability)
        XCTAssertTrue(capabilities.contains(.dpadFocus))
        XCTAssertFalse(capabilities.contains(.touchGestures))
    }

    func testDeviceInfoMaps() {
        let info = ProtoConversion.deviceInfo(
            AgentDeviceInfo(
                model: "Apple TV",
                osName: "tvOS",
                osVersion: "18.0",
                screenWidthPx: 1920,
                screenHeightPx: 1080,
                pixelRatio: 1.0,
                runtime: "XCUITest"
            )
        )
        XCTAssertEqual(info.model, "Apple TV")
        XCTAssertEqual(info.screenWidthPx, 1920)
        XCTAssertEqual(info.screenHeightPx, 1080)
        XCTAssertEqual(info.pixelRatio, 1.0)
        XCTAssertEqual(info.runtime, "XCUITest")
    }

    func testHandshakeRequestPrefersTheBodyTokenAndFallsBackToMetadata() {
        var raw = Impresario_V1_HandshakeRequest()
        raw.protocolVersion = "1.0.0"
        raw.platform = .tvos
        raw.appID = "com.example.tv"

        let withBody = ProtoConversion.handshakeRequest(raw, metadataToken: "meta")
        XCTAssertEqual(withBody.authToken, "meta")
        XCTAssertEqual(withBody.platform, .tvos)

        raw.authToken = "body"
        let withBoth = ProtoConversion.handshakeRequest(raw, metadataToken: "meta")
        XCTAssertEqual(withBoth.authToken, "body")
    }

    func testHandshakeResponseMapsARejection() {
        let response = ProtoConversion.handshakeResponse(
            HandshakeResult(
                accepted: false,
                sessionId: nil,
                agentVersion: "0.1.0",
                protocolVersion: "1.0.0",
                capabilities: [],
                deviceInfo: nil,
                error: AgentErrorReport(code: .alreadyInUse, message: "busy")
            )
        )
        XCTAssertFalse(response.accepted)
        XCTAssertEqual(response.sessionID, "")
        XCTAssertEqual(response.error.code, .alreadyInUse)
        XCTAssertFalse(response.hasDevice)
    }
}
