import Foundation
import XCTest

/// Programmatic handling of the system alerts that stand between a fresh
/// install and a working WLAN agent (spec 12.1 phase-0 item (b), I-88):
///
/// * “„X" möchte Geräte in deinem lokalen Netzwerk suchen und sich mit ihnen
///   verbinden” — iOS 14+/tvOS 14+ asks before a process may accept connections
///   from the local network (and before Bonjour browsing/advertising), so the
///   *first* run after an install blocks until somebody taps Allow;
/// * permission dialogs the app under test raises.
///
/// Two mechanisms, because neither suffices alone: `addUIInterruptionMonitor`
/// fires on the *next* interaction, and the direct SpringBoard query below
/// works right after the server starts. Both are best-effort — a run must never
/// fail because an alert could not be tapped.
enum SystemAlerts {
    /// Affirmative buttons we know. Extend when a new device language shows up;
    /// the second-button rule below covers the remaining local-network cases.
    static let allowLabels = [
        "Allow",
        "OK",
        "Allow While Using App",
        "Allow Once",
        "Erlauben",
        "Einmal erlauben",
        "Autoriser",
        "Consentir",
        "允许",
        "許す",
        "허용",
    ]

    /// Negative buttons: a two-button alert whose *first* button is one of
    /// these is the local-network dialog, whose second button is Allow.
    static let denyLabels = [
        "Don't Allow",
        "Nicht erlauben",
        "Ne pas autoriser",
        "不允许",
        "許可しない",
        "허용 안 함",
    ]

    // MARK: - interruption monitor

    /// Registers the monitor on `testCase`; it runs whenever an interaction
    /// happens while a system alert is up.
    static func register(on testCase: XCTestCase) {
        testCase.addUIInterruptionMonitor(withDescription: "impresario system alerts") { alert in
            guard let label = tapAllow(in: alert) else {
                return false
            }
            print("impresario agent: interruption monitor tapped '\(label)'")
            return true
        }
    }

    // MARK: - direct handling

    /// Polls SpringBoard for a pending alert for up to `timeout` seconds and
    /// taps its affirmative button. Returns true when an alert was dismissed.
    @discardableResult
    static func allowLocalNetworkIfPrompted(timeout: TimeInterval = 20) -> Bool {
        let deadline = Date().addingTimeInterval(max(0, timeout))
        let springboard = XCUIApplication(bundleIdentifier: "com.apple.springboard")
        var loggedButtons = false
        while Date() < deadline {
            if let alert = springboard.alerts.allElementsBoundByIndex.first(where: { $0.exists }) {
                if let label = tapAllow(in: alert) {
                    print("impresario agent: dismissed a system alert by tapping '\(label)'")
                    return true
                }
                if !loggedButtons {
                    let labels = alert.buttons.allElementsBoundByIndex.map { $0.label }
                    print(
                        "impresario agent: a system alert is up but its buttons are unknown "
                            + "(\(labels)) — tap the allow button on the device once"
                    )
                    loggedButtons = true
                }
            }
            Thread.sleep(forTimeInterval: 0.5)
        }
        return false
    }

    /// Taps the affirmative button of `alert`: a known allow label, else the
    /// second button of a two-button alert whose first button is a known deny
    /// label (iOS puts [Don't Allow, Allow]). Returns the tapped label.
    ///
    /// tvOS has no touch input: `XCUIElement.tap()` is unavailable, and
    /// guessing focus with `XCUIRemote` could select "Don't Allow" by mistake
    /// (which is sticky). The one-time local-network prompt is therefore
    /// granted by a human with the remote; the caller logs the buttons it saw.
    private static func tapAllow(in alert: XCUIElement) -> String? {
        #if os(tvOS)
        _ = alert
        print("impresario agent: tvOS system alert needs one remote-key grant")
        return nil
        #else
        for label in allowLabels {
            let button = alert.buttons[label]
            if button.exists && button.isHittable {
                button.tap()
                return label
            }
        }
        let buttons = alert.buttons.allElementsBoundByIndex.filter { $0.exists }
        guard buttons.count == 2 else {
            return nil
        }
        guard denyLabels.contains(buttons[0].label), buttons[1].isHittable else {
            return nil
        }
        buttons[1].tap()
        return buttons[1].label
        #endif
    }
}
