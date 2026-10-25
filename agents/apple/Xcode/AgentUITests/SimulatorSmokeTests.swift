import AgentCore
import Foundation
import XCTest
// Xcode 26 splits the UI-automation classes into their own framework (I-89).
#if canImport(XCUIAutomation)
import XCUIAutomation
#endif

/// Runs the real `XCUIDriver` against the fixture app inside an iOS, iPadOS or
/// tvOS **simulator**. This test needs no master and no hardware: CI boots the
/// simulator, installs `AgentTargetStub-*`, builds the UI-test bundle and runs
/// this class with `-only-testing`. It is the cross-platform replacement for
/// "open Xcode on a Mac and press Test".
///
/// `AgentRunnerTests/testServe` remains the long-lived agent used by
/// `impresario run`; this class is a bounded smoke test so it can execute in a
/// normal `xcodebuild test-without-building` invocation.
final class SimulatorSmokeTests: XCTestCase {
    private static let iosStub = "com.impresario.agent.AgentTargetStub-iOS"
    private static let tvosStub = "com.impresario.agent.AgentTargetStub-tvOS"
    private static let hookTimeout: TimeInterval = 15

    func testDriverAgainstStub() throws {
        continueAfterFailure = false

        let platform = RunnerEnvironment.platform(for: Bundle(for: SimulatorSmokeTests.self))
        let appId = platform == .tvos ? Self.tvosStub : Self.iosStub
        let driver = XCUIDriver()
        driver.prepare(appId: appId, animationsDisabled: true)
        defer { try? driver.terminate(appId) }

        try driver.launch(appId)

        let title = try first(driver, "stub_title")
        XCTAssertEqual(driver.text(title), "Impresario stub")

        let button = try first(driver, "stub_button")
        XCTAssertTrue(driver.isVisible(button), "the fixture button must be visible")
        XCTAssertTrue(driver.isEnabled(button), "the fixture button must be enabled")

        let screenshot = try driver.screenshotPNG()
        let dimensions = Png.dimensions(screenshot)
        XCTAssertNotNil(dimensions, "the agent screenshot must be a PNG")
        XCTAssertGreaterThan(dimensions?.width ?? 0, 0)
        XCTAssertGreaterThan(dimensions?.height ?? 0, 0)

        let info = driver.deviceInfo()
        XCTAssertTrue(
            info.osName.lowercased().contains("ios") || info.osName.lowercased().contains("tvos"),
            "unexpected simulator OS \(info.osName)"
        )

        // The touch path is deterministic on iPhone/iPad. On tvOS the same
        // command goes through the D-pad focus navigator; CI exercises that in
        // the master-driven play (AgentRunnerTests/testServe), so do not make
        // this smoke test depend on focus-engine quirks.
        if platform == .ios {
            try driver.tap(button)
            let counter = try first(driver, "stub_counter")
            XCTAssertEqual(driver.text(counter), "taps: 1")

            try exerciseMediaHook(driver)
        }
    }

    /// Opens the playback fixture, starts the bundled video and waits for the
    /// in-app hook to report decoded media over loopback. A missing hook must
    /// fail here: this is the simulator proof that D-41's transport works.
    private func exerciseMediaHook(_ driver: XCUIDriver) throws {
        let media = try first(driver, "stub_scenario_media")
        // The playback button is below a 30-row fixture scroller. Swipe the
        // outer page (not the inner scroller) until XCUITest reports it
        // hittable; then hand it back to the driver under test.
        let app = XCUIApplication(bundleIdentifier: Self.iosStub)
        let page = app.scrollViews["stub_page"]
        var attempts = 0
        while !driver.isHittable(media), attempts < 20 {
            page.swipeUp()
            attempts += 1
        }
        guard driver.isHittable(media) else {
            XCTFail("could not scroll to the media fixture")
            return
        }
        try driver.tap(media)

        _ = try first(driver, "stub_media_title")
        try driver.tap(try first(driver, "stub_media_play"))

        let deadline = Date().addingTimeInterval(Self.hookTimeout)
        var last: MediaSnapshot?
        while Date() < deadline {
            if let snapshot = driver.mediaSnapshot() {
                last = snapshot
                if snapshot.decoded == true {
                    XCTAssertFalse(
                        (snapshot.source ?? "").isEmpty,
                        "decoded evidence must name the media source"
                    )
                    return
                }
            }
            Thread.sleep(forTimeInterval: 0.25)
        }
        XCTFail(
            "the in-app hook did not report decoded media within "
                + "\(Int(Self.hookTimeout))s (last snapshot: \(String(describing: last)))"
        )
    }

    private func first(_ driver: XCUIDriver, _ identifier: String) throws -> ElementRef {
        let spec = SelectorSpec(strategy: .accessibilityId, value: identifier, index: 0)
        let found = try driver.find(spec, timeout: 20)
        guard let element = found.first else {
            XCTFail("no element with accessibility id \(identifier)")
            throw AgentError.elementNotFound("missing fixture element \(identifier)")
        }
        return element
    }
}
