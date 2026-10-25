import UIKit

/// The empty host app of the UI-test bundles (`AgentHost-iOS` / `-tvOS`).
///
/// XCTest needs a *target application* to open an automation session, and the
/// APNs-style flow of a UI test launches it before the runner. The app under
/// test is named by the play (`app_id`) and launched by the runner through
/// `XCUIApplication(bundleIdentifier:)`, so this app only has to exist, stay
/// quiet and carry the local-network usage description in its Info.plist
/// (docs/DECISIONS.md I-88).
///
/// The same source builds for iOS and tvOS; only the text differs.
@main
final class AgentHostAppDelegate: UIResponder, UIApplicationDelegate {
    var window: UIWindow?

    func application(
        _ application: UIApplication,
        didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]? = nil
    ) -> Bool {
        let window = UIWindow(frame: UIScreen.main.bounds)
        window.rootViewController = AgentHostViewController()
        window.makeKeyAndVisible()
        self.window = window
        return true
    }
}

/// One screen of status text: on a real device this is what a human looks at
/// while the master drives the *other* app.
final class AgentHostViewController: UIViewController {
    private let label = UILabel()

    override func viewDidLoad() {
        super.viewDidLoad()
        #if os(tvOS)
        view.backgroundColor = .black
        #else
        view.backgroundColor = .systemBackground
        #endif
        view.accessibilityIdentifier = "agent_host_root"

        label.accessibilityIdentifier = "agent_host_status"
        label.numberOfLines = 0
        label.textAlignment = .center
        label.font = .preferredFont(forTextStyle: .title2)
        label.adjustsFontForContentSizeCategory = true
        label.text = [
            "Impresario agent host",
            "",
            "The XCUITest runner (\(Self.runnerName)) hosts the agent;",
            "this app is only the target application of the test bundle.",
            "",
            "State is visible in the xcodebuild output: look for",
            "\"impresario agent:\" lines.",
        ].joined(separator: "\n")
        label.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(label)
        NSLayoutConstraint.activate([
            label.leadingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.leadingAnchor, constant: 24),
            label.trailingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.trailingAnchor, constant: -24),
            label.centerYAnchor.constraint(equalTo: view.centerYAnchor),
        ])
    }

    private static var runnerName: String {
        #if os(tvOS)
        return "AgentUITests-tvOS"
        #else
        return "AgentUITests-iOS"
        #endif
    }
}
