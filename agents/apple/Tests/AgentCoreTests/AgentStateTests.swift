import XCTest
@testable import AgentCore

final class AgentStateTests: XCTestCase {
    private var driver: FakeUIDriver!
    private var state: AgentState!

    override func setUp() {
        super.setUp()
        driver = FakeUIDriver()
        state = AgentState(
            driver: driver,
            platform: .tvos,
            expectedToken: "",
            sessionIdFactory: { "session-1" }
        )
    }

    private func request(
        version: String = "1.0.0",
        platform: PlatformKind? = .tvos,
        appId: String = "com.example.tv",
        options: [String: String] = [:],
        token: String = ""
    ) -> HandshakeRequestData {
        HandshakeRequestData(
            protocolVersion: version,
            masterVersion: "0.1.0",
            platform: platform,
            appId: appId,
            options: options,
            authToken: token
        )
    }

    func testAcceptsACompatibleHandshake() {
        let result = state.handshake(request(options: ["animations_disabled": "true"]))

        XCTAssertTrue(result.accepted)
        XCTAssertEqual(result.sessionId, "session-1")
        XCTAssertEqual(result.protocolVersion, "1.0.0")
        XCTAssertEqual(result.agentVersion, "0.1.0")
        XCTAssertEqual(result.capabilities, [.logs, .screenshot, .dpadFocus, .keyEvents, .appControl])
        XCTAssertEqual(result.deviceInfo?.runtime, "XCUITest")
        XCTAssertNil(result.error)
        XCTAssertEqual(state.activeSessionId, "session-1")
        XCTAssertEqual(driver.prepared?.appId, "com.example.tv")
        XCTAssertEqual(driver.prepared?.animationsDisabled, true)
    }

    func testIosCapabilities() {
        let iosState = AgentState(driver: driver, platform: .ios)
        let result = iosState.handshake(request(platform: .ios, appId: "com.example.app"))
        XCTAssertEqual(result.capabilities, [.logs, .screenshot, .touchGestures, .keyEvents, .appControl])
    }

    func testRejectsAMajorVersionMismatch() {
        let result = state.handshake(request(version: "2.0.0"))
        XCTAssertFalse(result.accepted)
        XCTAssertEqual(result.error?.code, .protocolViolation)
        XCTAssertNil(state.activeSessionId)
    }

    func testAcceptsANewerMinorVersion() {
        XCTAssertTrue(state.handshake(request(version: "1.7.3")).accepted)
    }

    func testRejectsAMissingVersion() {
        let result = state.handshake(request(version: " "))
        XCTAssertFalse(result.accepted)
        XCTAssertEqual(result.error?.code, .protocolViolation)
    }

    func testRejectsTheWrongPlatform() {
        let result = state.handshake(request(platform: .ios))
        XCTAssertFalse(result.accepted)
        XCTAssertEqual(result.error?.code, .protocolViolation)
        XCTAssertTrue(result.error?.message.contains("tvos") == true)
    }

    func testRejectsAnUnspecifiedPlatform() {
        let result = state.handshake(request(platform: nil))
        XCTAssertFalse(result.accepted)
        XCTAssertEqual(result.error?.code, .protocolViolation)
    }

    func testRejectsASecondHandshakeWhileTheSessionIsActive() {
        XCTAssertTrue(state.handshake(request()).accepted)
        let second = state.handshake(request(appId: "com.example.other"))
        XCTAssertFalse(second.accepted)
        XCTAssertEqual(second.error?.code, .alreadyInUse)
        XCTAssertTrue(second.error?.message.contains("session-1") == true)
    }

    func testAcceptsAgainAfterTheStreamEnded() {
        XCTAssertTrue(state.handshake(request()).accepted)
        state.endStream()
        XCTAssertNil(state.activeSessionId)
        XCTAssertTrue(state.handshake(request()).accepted)
    }

    func testTokenIsEnforcedWhenConfigured() {
        let secured = AgentState(driver: driver, platform: .tvos, expectedToken: "secret")
        XCTAssertFalse(secured.handshake(request(token: "")).accepted)
        XCTAssertEqual(secured.handshake(request(token: "")).error?.code, .unauthenticated)
        XCTAssertFalse(secured.handshake(request(token: "wrong")).accepted)
        XCTAssertTrue(secured.handshake(request(token: "secret")).accepted)
    }

    func testOpenStreamRules() {
        XCTAssertTrue(state.handshake(request()).accepted)

        guard case .unknownSession = state.openStream(sessionId: "other") else {
            return XCTFail("expected unknownSession for a foreign id")
        }
        guard case .accepted = state.openStream(sessionId: "session-1") else {
            return XCTFail("expected the first stream to be accepted")
        }
        guard case .alreadyStreaming = state.openStream(sessionId: "session-1") else {
            return XCTFail("expected a second concurrent stream to be rejected")
        }
        state.endStream()
        guard case .unknownSession = state.openStream(sessionId: "session-1") else {
            return XCTFail("expected the session to be released")
        }
    }

    func testExecutorReceivesTheHandshakeOptions() {
        XCTAssertTrue(
            state.handshake(
                request(options: [
                    "poll_interval_ms": "40",
                    "scroll_max_attempts": "3",
                    "focus_max_steps": "7",
                ])
            ).accepted
        )
        guard case .accepted(let executor) = state.openStream(sessionId: "session-1") else {
            return XCTFail("expected an accepted stream")
        }
        XCTAssertEqual(executor.executorOptions.platform, .tvos)
        XCTAssertEqual(executor.executorOptions.appId, "com.example.tv")
        XCTAssertEqual(executor.executorOptions.pollIntervalMs, 40)
        XCTAssertEqual(executor.executorOptions.scrollMaxAttempts, 3)
        XCTAssertEqual(executor.executorOptions.focusMaxSteps, 7)
    }
}
