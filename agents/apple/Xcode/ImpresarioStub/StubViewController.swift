import UIKit

/// The stub home screen: one row per test scenario, each opening its own
/// dedicated screen. This mirrors Android `MainActivity` exactly: the same
/// title, subtitle, row order, row labels and accessibility ids. Every row is
/// a focusable button, so the same app works under touch and under TV D-pad
/// navigation (the first row takes focus).
final class StubViewController: UIViewController {
    private static let scenarios: [(id: String, label: String, make: () -> UIViewController)] = [
        ("stub_scenario_basic", "Basic controls", { BasicScenarioViewController() }),
        ("stub_scenario_text", "Text entry", { TextScenarioViewController() }),
        ("stub_scenario_scroll", "Scroller", { ScrollScenarioViewController() }),
        ("stub_scenario_select", "Selection", { SelectScenarioViewController() }),
        ("stub_scenario_flags", "Flags", { FlagsScenarioViewController() }),
        ("stub_scenario_keys", "Key log", { KeysScenarioViewController() }),
        ("stub_scenario_info", "Device info", { InfoScenarioViewController() }),
        ("stub_scenario_media", "Playback", { MediaViewController() }),
    ]

    private let scrollView = UIScrollView()
    private let column = UIStackView()
    private var firstButton: StubButton?

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = StubTheme.background

        column.axis = .vertical
        column.alignment = .fill
        column.spacing = StubTheme.stackGap
        column.translatesAutoresizingMaskIntoConstraints = false

        let title = StubTheme.titleLabel("Impresario stub")
        title.accessibilityIdentifier = "stub_title"
        // Android puts contentDescription "stub badge" on the title. UIKit has
        // one label per element, so the text stays the label and the badge is
        // exposed as the hint.
        title.accessibilityHint = "stub badge"
        column.addArrangedSubview(title)

        let subtitle = StubTheme.label("One screen per test scenario.", size: 15)
        subtitle.alpha = 0.8
        column.addArrangedSubview(subtitle)
        column.setCustomSpacing(0, after: title)

        for scenario in Self.scenarios {
            let button = StubTheme.button(scenario.label, id: scenario.id)
            button.addTarget(self, action: #selector(openScenario(_:)), for: .primaryActionTriggered)
            column.addArrangedSubview(button)
            if firstButton == nil {
                firstButton = button
            }
        }

        scrollView.accessibilityIdentifier = nil
        scrollView.showsVerticalScrollIndicator = false
        scrollView.contentInsetAdjustmentBehavior = .never
        scrollView.translatesAutoresizingMaskIntoConstraints = false
        scrollView.addSubview(column)
        view.addSubview(scrollView)

        NSLayoutConstraint.activate([
            scrollView.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            scrollView.trailingAnchor.constraint(equalTo: view.trailingAnchor),
            scrollView.topAnchor.constraint(equalTo: view.safeAreaLayoutGuide.topAnchor),
            scrollView.bottomAnchor.constraint(equalTo: view.bottomAnchor),
            column.leadingAnchor.constraint(
                equalTo: scrollView.contentLayoutGuide.leadingAnchor,
                constant: StubTheme.horizontalInset
            ),
            column.trailingAnchor.constraint(
                equalTo: scrollView.contentLayoutGuide.trailingAnchor,
                constant: -StubTheme.horizontalInset
            ),
            column.topAnchor.constraint(
                equalTo: scrollView.contentLayoutGuide.topAnchor,
                constant: StubTheme.verticalInset
            ),
            column.bottomAnchor.constraint(
                equalTo: scrollView.contentLayoutGuide.bottomAnchor,
                constant: -StubTheme.verticalInset
            ),
            column.widthAnchor.constraint(
                equalTo: scrollView.frameLayoutGuide.widthAnchor,
                constant: -2 * StubTheme.horizontalInset
            ),
        ])
    }

    override var preferredFocusEnvironments: [UIFocusEnvironment] {
        if let firstButton {
            return [firstButton]
        }
        return super.preferredFocusEnvironments
    }

    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        #if os(tvOS)
        setNeedsFocusUpdate()
        updateFocusIfNeeded()
        #endif
    }

    @objc private func openScenario(_ sender: StubButton) {
        guard let scenario = Self.scenarios.first(where: { $0.id == sender.accessibilityIdentifier }) else {
            return
        }
        navigationController?.pushViewController(
            scenario.make(),
            animated: UIView.areAnimationsEnabled
        )
    }
}
