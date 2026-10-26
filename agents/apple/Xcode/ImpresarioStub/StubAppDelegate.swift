import UIKit
import ImpresarioHook

#if os(tvOS)
/// Routes the tvOS Menu press to Android's Back action. A hidden-bar
/// navigation stack does not pop reliably when the top screen has no focused
/// control (Device info, Playback); a window-level recognizer still sees the
/// press there. At the home screen it declines so Menu exits the app as before.
private final class StubMenuRouter: NSObject, UIGestureRecognizerDelegate {
    weak var navigation: UINavigationController?

    @objc func onMenu() {
        guard let navigation, navigation.viewControllers.count > 1 else { return }
        navigation.popViewController(animated: UIView.areAnimationsEnabled)
    }

    func gestureRecognizerShouldBegin(_ gestureRecognizer: UIGestureRecognizer) -> Bool {
        guard let navigation, navigation.presentedViewController == nil else { return false }
        return navigation.viewControllers.count > 1
    }
}
#endif

/// The fixture app under test (`ImpresarioStub-iOS` / `-tvOS`): a fixture with
/// the same stable accessibility identifiers as the Android stub
/// (`agents/android/agent-target-stub`), so the same plays exercise both
/// platforms. Not part of the agent: the agent drives this app from outside.
@main
final class StubAppDelegate: UIResponder, UIApplicationDelegate {
    var window: UIWindow?
    #if os(tvOS)
    private var menuRouter: StubMenuRouter?
    #endif

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
        // Android opens each scenario as its own activity; a hidden-bar
        // navigation stack gives the same one-screen-per-scenario flow and
        // lets the tvOS Menu button (Android back) return to the home screen.
        let home = StubViewController()
        let navigation = UINavigationController(rootViewController: home)
        navigation.setNavigationBarHidden(true, animated: false)
        #if os(tvOS)
        let router = StubMenuRouter()
        router.navigation = navigation
        let menu = UITapGestureRecognizer(target: router, action: #selector(StubMenuRouter.onMenu))
        menu.allowedPressTypes = [NSNumber(value: UIPress.PressType.menu.rawValue)]
        menu.delegate = router
        window.addGestureRecognizer(menu)
        menuRouter = router
        #endif
        window.rootViewController = navigation
        window.makeKeyAndVisible()
        self.window = window
        return true
    }
}
