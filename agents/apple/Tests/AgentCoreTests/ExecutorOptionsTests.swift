import XCTest
@testable import AgentCore

final class ExecutorOptionsTests: XCTestCase {
    func testDefaults() {
        let options = ExecutorOptions(platform: .ios, appId: "com.example.app")
        XCTAssertEqual(options.pollIntervalMs, 250)
        XCTAssertEqual(options.scrollMaxAttempts, 10)
        XCTAssertEqual(options.focusMaxSteps, 15)
        XCTAssertFalse(options.animationsDisabled)
        XCTAssertFalse(options.clearAppDataOnRestart)
    }

    func testParsesHandshakeOptions() {
        let options = ExecutorOptions.from(
            options: [
                "poll_interval_ms": "50",
                "scroll_max_attempts": "4",
                "focus_max_steps": "9",
                "animations_disabled": "true",
                "clear_app_data": "1",
                "unknown_key": "ignored",
            ],
            platform: .tvos,
            appId: "com.example.tv"
        )
        XCTAssertEqual(options.platform, .tvos)
        XCTAssertEqual(options.appId, "com.example.tv")
        XCTAssertEqual(options.pollIntervalMs, 50)
        XCTAssertEqual(options.scrollMaxAttempts, 4)
        XCTAssertEqual(options.focusMaxSteps, 9)
        XCTAssertTrue(options.animationsDisabled)
        XCTAssertTrue(options.clearAppDataOnRestart)
    }

    func testIgnoresInvalidAndNonPositiveValues() {
        let options = ExecutorOptions.from(
            options: [
                "poll_interval_ms": "0",
                "scroll_max_attempts": "-3",
                "focus_max_steps": "many",
                "animations_disabled": "no",
            ],
            platform: .ios,
            appId: "a"
        )
        XCTAssertEqual(options.pollIntervalMs, 250)
        XCTAssertEqual(options.scrollMaxAttempts, 10)
        XCTAssertEqual(options.focusMaxSteps, 15)
        XCTAssertFalse(options.animationsDisabled)
    }

    func testBoolSpellings() {
        for value in ["1", "true", "TRUE", "yes", "on"] {
            XCTAssertTrue(
                ExecutorOptions.from(options: ["animations_disabled": value], platform: .ios, appId: "a")
                    .animationsDisabled,
                value
            )
        }
        for value in ["0", "false", "no", "off", ""] {
            XCTAssertFalse(
                ExecutorOptions.from(options: ["animations_disabled": value], platform: .ios, appId: "a")
                    .animationsDisabled,
                value
            )
        }
    }

    func testClampsToAtLeastOne() {
        let options = ExecutorOptions(platform: .ios, pollIntervalMs: 0, scrollMaxAttempts: 0, focusMaxSteps: -1)
        XCTAssertEqual(options.pollIntervalMs, 1)
        XCTAssertEqual(options.scrollMaxAttempts, 1)
        XCTAssertEqual(options.focusMaxSteps, 1)
    }
}
