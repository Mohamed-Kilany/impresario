import UIKit
import ImpresarioHook

/// The fixture app under test (`ImpresarioStub-iOS` / `-tvOS`): a fixture with
/// the same stable accessibility identifiers as the Android stub
/// (`agents/android/agent-target-stub`), so the same plays exercise both
/// platforms. Not part of the agent: the agent drives this app from outside.
@main
final class StubAppDelegate: UIResponder, UIApplicationDelegate {
    var window: UIWindow?

    /// Launch argument the agent passes with `animations_disabled` (spec 12.3);
    /// Apple has no public API to switch another app's animations off, so the
    /// app under test honours the flag itself.
    static let animationsDisabledArgument = "-impresario-animations-disabled"

    func application(
        _ application: UIApplication,
        didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]? = nil
    ) -> Bool {
        if ProcessInfo.processInfo.arguments.contains(Self.animationsDisabledArgument) {
            UIView.setAnimationsEnabled(false)
        }
        // Debug-only in-app hook (D-41): availability never changes a play;
        // it only lets Impresario Agent raise confidence when it is listening.
        ImpresarioHook.shared.start()
        let window = UIWindow(frame: UIScreen.main.bounds)
        window.rootViewController = StubViewController()
        window.makeKeyAndVisible()
        self.window = window
        return true
    }
}
