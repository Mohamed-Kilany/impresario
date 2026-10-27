import UIKit

/// Shared scaffold for the scenario screens, mirroring Android
/// `ScenarioActivity`: the same 48 x 36 dp padding, a bold 24 pt title with the
/// stable `stub_title` id, the scenario body, then a `stub_status` line that
/// starts at `status=idle`.
class ScenarioViewController: UIViewController {
    let column = UIStackView()
    let statusLabel = StubTheme.statusLabel()

    private var preferredFocusView: UIView?

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = StubTheme.background

        column.axis = .vertical
        column.alignment = .fill
        column.spacing = StubTheme.stackGap
        column.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(column)

        let contentTop: NSLayoutYAxisAnchor
        #if os(tvOS)
        let appBar = StubAppBar()
        view.addSubview(appBar)
        NSLayoutConstraint.activate([
            appBar.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            appBar.trailingAnchor.constraint(equalTo: view.trailingAnchor),
            appBar.topAnchor.constraint(equalTo: view.topAnchor),
        ])
        contentTop = appBar.bottomAnchor
        #else
        contentTop = view.safeAreaLayoutGuide.topAnchor
        #endif

        NSLayoutConstraint.activate([
            column.leadingAnchor.constraint(
                equalTo: view.fixtureLeadingAnchor,
                constant: StubTheme.horizontalInset
            ),
            column.trailingAnchor.constraint(
                equalTo: view.fixtureTrailingAnchor,
                constant: -StubTheme.horizontalInset
            ),
            column.topAnchor.constraint(
                equalTo: contentTop,
                constant: StubTheme.verticalInset
            ),
            column.bottomAnchor.constraint(
                lessThanOrEqualTo: view.fixtureBottomAnchor,
                constant: -StubTheme.verticalInset
            ),
        ])

        let title = StubTheme.titleLabel(scenarioTitle(), lineHeight: 32.5)
        title.accessibilityIdentifier = "stub_title"
        title.accessibilityHint = "stub badge"
        column.addArrangedSubview(title)

        buildScenario(into: column)

        statusLabel.accessibilityIdentifier = "stub_status"
        column.addArrangedSubview(statusLabel)
    }

    override var preferredFocusEnvironments: [UIFocusEnvironment] {
        if let preferredFocusView {
            return [preferredFocusView]
        }
        return super.preferredFocusEnvironments
    }

    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        guard preferredFocusView != nil else { return }
        #if os(tvOS)
        setNeedsFocusUpdate()
        updateFocusIfNeeded()
        #endif
    }

    /// The scenario title, e.g. `Basic controls`.
    func scenarioTitle() -> String {
        preconditionFailure("scenarioTitle() must be overridden")
    }

    /// Add the scenario's controls to `column`, before the shared status line.
    func buildScenario(into column: UIStackView) {
        preconditionFailure("buildScenario(into:) must be overridden")
    }

    func requestFocus(on view: UIView) {
        preferredFocusView = view
    }

    func addButton(
        to column: UIStackView,
        title: String,
        id: String,
        action: Selector,
        longPress: (() -> Void)? = nil
    ) -> StubButton {
        let button = StubTheme.button(title, id: id)
        button.addTarget(self, action: action, for: .primaryActionTriggered)
        button.onLongPress = longPress
        column.addArrangedSubview(button)
        return button
    }
}

// ----------------------------------------------------------------------------
// Basic controls
// ----------------------------------------------------------------------------

/// Click + long press with a tap counter and a status line.
final class BasicScenarioViewController: ScenarioViewController {
    private var taps = 0
    private let counter = StubTheme.label("taps=0", lineHeight: 24.5)
    private var tapButton: StubButton?

    override func scenarioTitle() -> String { "Basic controls" }

    override func buildScenario(into column: UIStackView) {
        counter.accessibilityIdentifier = "stub_counter"
        let button = addButton(
            to: column,
            title: "Tap me",
            id: "stub_button",
            action: #selector(onTap),
            longPress: { [weak self] in self?.statusLabel.text = "status=long-pressed" }
        )
        tapButton = button
        column.addArrangedSubview(counter)
        requestFocus(on: button)
    }

    @objc private func onTap() {
        // Android's long-click listener consumes the long press; no click is
        // delivered afterwards. tvOS can deliver the click separately, so
        // discard the one that belongs to the long select.
        if tapButton?.consumeLongPress() == true {
            return
        }
        taps += 1
        counter.text = "taps=\(taps)"
        statusLabel.text = "status=clicked"
    }
}

// ----------------------------------------------------------------------------
// Text entry
// ----------------------------------------------------------------------------

/// Text entry: a field with an echo label (type/append/clear/backspace).
final class TextScenarioViewController: ScenarioViewController {
    private let input = UITextField()
    private let echo = StubTheme.label("echo:", lineHeight: 24.5)
    private let inputUnderline = UIView()

    override func scenarioTitle() -> String { "Text entry" }

    override func buildScenario(into column: UIStackView) {
        input.placeholder = "type here"
        input.accessibilityIdentifier = "stub_input"
        // Android's empty EditText has no text; its hint is not the value.
        input.accessibilityLabel = ""
        input.font = .systemFont(ofSize: StubTheme.dp(18))
        input.textColor = StubTheme.foreground
        input.tintColor = StubTheme.inputUnderline
        input.attributedPlaceholder = NSAttributedString(
            string: "type here",
            attributes: [.foregroundColor: StubTheme.inputHint]
        )
        // Android EditText: transparent surface, 12 dp text inset and a 2 dp
        // underline 7 dp above the bottom of a 36.5 dp field. The underline is
        // the teal accent while editing and gray otherwise.
        input.backgroundColor = .clear
        input.borderStyle = .none
        input.heightAnchor.constraint(equalToConstant: StubTheme.dp(36.5)).isActive = true
        let padding = UIView(frame: CGRect(x: 0, y: 0, width: StubTheme.dp(12), height: 1))
        input.leftView = padding
        input.leftViewMode = .always
        input.addTarget(self, action: #selector(onInputChanged), for: .editingChanged)
        input.addTarget(self, action: #selector(onInputFocus), for: .editingDidBegin)
        input.addTarget(self, action: #selector(onInputBlur), for: .editingDidEnd)

        inputUnderline.backgroundColor = StubTheme.inputIdleUnderline
        inputUnderline.translatesAutoresizingMaskIntoConstraints = false
        input.addSubview(inputUnderline)
        NSLayoutConstraint.activate([
            inputUnderline.leadingAnchor.constraint(
                equalTo: input.leadingAnchor,
                constant: StubTheme.dp(4)
            ),
            inputUnderline.trailingAnchor.constraint(
                equalTo: input.trailingAnchor,
                constant: -StubTheme.dp(4)
            ),
            inputUnderline.bottomAnchor.constraint(
                equalTo: input.bottomAnchor,
                constant: -StubTheme.dp(7)
            ),
            inputUnderline.heightAnchor.constraint(equalToConstant: StubTheme.dp(2)),
        ])
        column.addArrangedSubview(input)

        echo.accessibilityIdentifier = "stub_echo"
        column.addArrangedSubview(echo)
        requestFocus(on: input)
    }

    @objc private func onInputChanged() {
        echo.text = "echo:" + (input.text ?? "")
    }

    @objc private func onInputFocus() {
        inputUnderline.backgroundColor = StubTheme.inputUnderline
    }

    @objc private func onInputBlur() {
        inputUnderline.backgroundColor = StubTheme.inputIdleUnderline
    }
}

// ----------------------------------------------------------------------------
// Scroller
// ----------------------------------------------------------------------------

/// Scrollable list with 30 `row N` entries, matching Android `ScrollActivity`.
/// The scroller takes the space between the title and the shared status line.
final class ScrollScenarioViewController: UIViewController {
    private let titleLabel = StubTheme.titleLabel("Scroller", lineHeight: 32.5)
    private let statusLabel = StubTheme.statusLabel()
    private let scroller = FocusableScrollView()

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = StubTheme.background

        titleLabel.accessibilityIdentifier = "stub_title"
        titleLabel.accessibilityHint = "stub badge"
        statusLabel.accessibilityIdentifier = "stub_status"
        scroller.accessibilityIdentifier = "stub_scroller"
        scroller.showsVerticalScrollIndicator = true
        scroller.translatesAutoresizingMaskIntoConstraints = false
        titleLabel.translatesAutoresizingMaskIntoConstraints = false
        statusLabel.translatesAutoresizingMaskIntoConstraints = false

        let list = UIView()
        list.accessibilityIdentifier = "stub_list"
        list.translatesAutoresizingMaskIntoConstraints = false

        let rows = UIStackView()
        rows.axis = .vertical
        rows.spacing = 0
        rows.translatesAutoresizingMaskIntoConstraints = false
        for index in 0..<30 {
            // Android's rows are TextViews with 12 dp top/bottom padding and
            // no stack gap; the padding, not a spacing property, gives the
            // 24 dp rhythm visible on the TV.
            let row = UIView()
            let label = StubTheme.label("row \(index)", lineHeight: 24.5)
            label.translatesAutoresizingMaskIntoConstraints = false
            row.addSubview(label)
            NSLayoutConstraint.activate([
                label.leadingAnchor.constraint(equalTo: row.leadingAnchor),
                label.trailingAnchor.constraint(equalTo: row.trailingAnchor),
                label.topAnchor.constraint(equalTo: row.topAnchor, constant: StubTheme.dp(12)),
                label.bottomAnchor.constraint(equalTo: row.bottomAnchor, constant: -StubTheme.dp(12)),
            ])
            rows.addArrangedSubview(row)
        }

        list.addSubview(rows)
        scroller.addSubview(list)
        view.addSubview(titleLabel)
        view.addSubview(scroller)
        view.addSubview(statusLabel)

        let contentTop: NSLayoutYAxisAnchor
        #if os(tvOS)
        let appBar = StubAppBar()
        view.addSubview(appBar)
        NSLayoutConstraint.activate([
            appBar.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            appBar.trailingAnchor.constraint(equalTo: view.trailingAnchor),
            appBar.topAnchor.constraint(equalTo: view.topAnchor),
        ])
        contentTop = appBar.bottomAnchor
        #else
        contentTop = view.safeAreaLayoutGuide.topAnchor
        #endif

        NSLayoutConstraint.activate([
            titleLabel.leadingAnchor.constraint(
                equalTo: view.fixtureLeadingAnchor,
                constant: StubTheme.horizontalInset
            ),
            titleLabel.trailingAnchor.constraint(
                equalTo: view.fixtureTrailingAnchor,
                constant: -StubTheme.horizontalInset
            ),
            titleLabel.topAnchor.constraint(
                equalTo: contentTop,
                constant: StubTheme.verticalInset
            ),

            scroller.leadingAnchor.constraint(
                equalTo: view.fixtureLeadingAnchor,
                constant: StubTheme.horizontalInset
            ),
            scroller.trailingAnchor.constraint(
                equalTo: view.fixtureTrailingAnchor,
                constant: -StubTheme.horizontalInset
            ),
            scroller.topAnchor.constraint(
                equalTo: titleLabel.bottomAnchor,
                constant: StubTheme.fixedStackGap
            ),
            scroller.bottomAnchor.constraint(
                equalTo: statusLabel.topAnchor,
                constant: -StubTheme.stackGap
            ),

            statusLabel.leadingAnchor.constraint(
                equalTo: view.fixtureLeadingAnchor,
                constant: StubTheme.horizontalInset
            ),
            statusLabel.trailingAnchor.constraint(
                equalTo: view.fixtureTrailingAnchor,
                constant: -StubTheme.horizontalInset
            ),
            statusLabel.bottomAnchor.constraint(
                equalTo: view.fixtureBottomAnchor,
                constant: -StubTheme.verticalInset
            ),

            list.leadingAnchor.constraint(equalTo: scroller.contentLayoutGuide.leadingAnchor),
            list.trailingAnchor.constraint(equalTo: scroller.contentLayoutGuide.trailingAnchor),
            list.topAnchor.constraint(equalTo: scroller.contentLayoutGuide.topAnchor),
            list.bottomAnchor.constraint(equalTo: scroller.contentLayoutGuide.bottomAnchor),
            list.widthAnchor.constraint(equalTo: scroller.frameLayoutGuide.widthAnchor),

            rows.leadingAnchor.constraint(equalTo: list.leadingAnchor),
            rows.trailingAnchor.constraint(equalTo: list.trailingAnchor),
            rows.topAnchor.constraint(equalTo: list.topAnchor),
            rows.bottomAnchor.constraint(equalTo: list.bottomAnchor),
        ])
    }

    override var preferredFocusEnvironments: [UIFocusEnvironment] {
        [scroller]
    }

    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        #if os(tvOS)
        setNeedsFocusUpdate()
        updateFocusIfNeeded()
        #endif
    }
}

// ----------------------------------------------------------------------------
// Selection
// ----------------------------------------------------------------------------

/// Selection: a dialog trigger and options A/B/C; the status shows the pick.
final class SelectScenarioViewController: ScenarioViewController {
    override func scenarioTitle() -> String { "Selection" }

    override func buildScenario(into column: UIStackView) {
        let trigger = addButton(
            to: column,
            title: "Choose",
            id: "stub_select",
            action: #selector(onChoose)
        )
        requestFocus(on: trigger)
    }

    @objc private func onChoose() {
        let options = ["Option A", "Option B", "Option C"]
        let dialog = SelectionDialogViewController(title: "Pick one", options: options) { [weak self] option in
            self?.statusLabel.text = "chosen: \(option)"
        }
        dialog.modalPresentationStyle = .overFullScreen
        dialog.modalTransitionStyle = .crossDissolve
        present(dialog, animated: UIView.areAnimationsEnabled)
    }
}

// ----------------------------------------------------------------------------
// Flags
// ----------------------------------------------------------------------------

/// Flags: enabled/disabled buttons and a visible/invisible label.
final class FlagsScenarioViewController: ScenarioViewController {
    override func scenarioTitle() -> String { "Flags" }

    override func buildScenario(into column: UIStackView) {
        let enabled = addButton(
            to: column,
            title: "Enabled",
            id: "stub_enabled",
            action: #selector(onEnabled)
        )

        let disabled = StubTheme.button("Disabled", id: "stub_disabled")
        disabled.isEnabled = false
        column.addArrangedSubview(disabled)

        let visible = StubTheme.label("visible text", lineHeight: 24.5)
        visible.accessibilityIdentifier = "stub_visible"
        column.addArrangedSubview(visible)

        // Android INVISIBLE: the label keeps its space but is not drawn and is
        // not exposed to accessibility. `isHidden` would remove the space.
        let hidden = StubTheme.label("hidden text", lineHeight: 24.5)
        hidden.accessibilityIdentifier = "stub_hidden"
        hidden.alpha = 0
        hidden.isAccessibilityElement = false
        hidden.accessibilityElementsHidden = true
        column.addArrangedSubview(hidden)

        requestFocus(on: enabled)
    }

    @objc private func onEnabled() {
        statusLabel.text = "status=enabled-clicked"
    }
}

// ----------------------------------------------------------------------------
// Key log
// ----------------------------------------------------------------------------

/// Key log: three focusable buttons to move focus across with the D-pad and a
/// label that records the last key (`key=DPAD_DOWN`, `key=MEDIA_PLAY_PAUSE`,
/// ...) so TV key handling is observable.
final class KeysScenarioViewController: ScenarioViewController {
    private let log = StubTheme.label("key=none", lineHeight: 24.5)
    private var lastFocusedIndex: Int?

    override func scenarioTitle() -> String { "Key log" }

    override func buildScenario(into column: UIStackView) {
        log.accessibilityIdentifier = "stub_key_log"

        let alpha = addButton(
            to: column,
            title: "Alpha",
            id: "stub_key_a",
            action: #selector(onAlpha)
        )
        let bravo = addButton(
            to: column,
            title: "Bravo",
            id: "stub_key_b",
            action: #selector(onBravo)
        )
        let charlie = addButton(
            to: column,
            title: "Charlie",
            id: "stub_key_c",
            action: #selector(onCharlie)
        )
        column.addArrangedSubview(log)

        #if os(tvOS)
        for (index, button) in [alpha, bravo, charlie].enumerated() {
            button.onPress = { [weak self] type in
                guard let name = Self.keyName(type) else { return }
                self?.log.text = "key=\(name)"
            }
            // tvOS can consume arrows in the focus engine before the focused
            // button sees them. The focus move itself still gives the same
            // DPAD_UP/DPAD_DOWN observation Android logs from onKeyDown.
            button.onFocusBecame = { [weak self] in
                guard let self else { return }
                if let last = self.lastFocusedIndex, last != index {
                    self.log.text = index > last ? "key=DPAD_DOWN" : "key=DPAD_UP"
                }
                self.lastFocusedIndex = index
            }
        }
        #endif

        requestFocus(on: alpha)
    }

    @objc private func onAlpha() { log.text = "clicked=Alpha" }
    @objc private func onBravo() { log.text = "clicked=Bravo" }
    @objc private func onCharlie() { log.text = "clicked=Charlie" }

    #if os(tvOS)
    private static func keyName(_ type: UIPress.PressType) -> String? {
        switch type {
        case .upArrow: return "DPAD_UP"
        case .downArrow: return "DPAD_DOWN"
        case .leftArrow: return "DPAD_LEFT"
        case .rightArrow: return "DPAD_RIGHT"
        case .select: return "DPAD_CENTER"
        case .menu: return "MENU"
        case .playPause: return "MEDIA_PLAY_PAUSE"
        default: return nil
        }
    }
    #endif
}

// ----------------------------------------------------------------------------
// Device info
// ----------------------------------------------------------------------------

/// Device info: what the app sees about itself (checks/textContains target).
final class InfoScenarioViewController: ScenarioViewController {
    override func scenarioTitle() -> String { "Device info" }

    override func buildScenario(into column: UIStackView) {
        let version = ProcessInfo.processInfo.operatingSystemVersion
        let tv = UIDevice.current.userInterfaceIdiom == .tv
        let text = "pkg=\(Bundle.main.bundleIdentifier ?? "?"); " +
            "sdk=\(version.majorVersion); " +
            "model=\(UIDevice.current.model); " +
            "tv=\(tv)"
        let info = StubTheme.label(text, size: 16)
        info.accessibilityIdentifier = "stub_info"
        #if os(tvOS)
        // Android's 16 sp TextView is a 43 px line box on the TV.
        info.heightAnchor.constraint(equalToConstant: StubTheme.dp(21.5)).isActive = true
        info.numberOfLines = 1
        #endif
        column.addArrangedSubview(info)
    }
}
