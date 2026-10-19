import UIKit

/// The single screen of the stub app: the fixture the plays point at.
///
/// Every identifier mirrors `agents/android/agent-target-stub`'s
/// `res/values/ids.xml` (`stub_title`, `stub_button`, `stub_counter`, …), so one
/// play can drive the Android and the Apple stub with only the selector
/// strategy changed.
final class StubViewController: UIViewController {
    // Fixture state.
    private var taps = 0
    private var keys: [String] = []

    // Elements that fixtures read or write.
    private let statusLabel = UILabel()
    private let counterLabel = UILabel()
    private let echoLabel = UILabel()
    private let keyLogLabel = UILabel()
    private let infoLabel = UILabel()
    private let input = UITextField()

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .systemBackground
        view.accessibilityIdentifier = "stub_root"

        let content = UIStackView()
        content.axis = .vertical
        content.spacing = Self.gap
        content.alignment = .fill
        content.translatesAutoresizingMaskIntoConstraints = false

        content.addArrangedSubview(title("Impresario stub", "stub_title"))
        content.addArrangedSubview(status("status: ready"))
        content.addArrangedSubview(section("Basic"))
        content.addArrangedSubview(button("Tap me", "stub_button", action: #selector(onTap)))
        content.addArrangedSubview(counter())
        content.addArrangedSubview(section("Text"))
        content.addArrangedSubview(textInput())
        content.addArrangedSubview(echo())
        content.addArrangedSubview(section("Select"))
        content.addArrangedSubview(button("Choose option", "stub_select", action: #selector(onSelect)))
        content.addArrangedSubview(section("State"))
        content.addArrangedSubview(button("Enabled", "stub_enabled", action: #selector(onEnabled)))
        content.addArrangedSubview(disabledButton())
        content.addArrangedSubview(label("always visible", "stub_visible"))
        content.addArrangedSubview(hiddenLabel())
        content.addArrangedSubview(section("Scroll"))
        content.addArrangedSubview(scroller())
        content.addArrangedSubview(section("Keys"))
        content.addArrangedSubview(keyLog())
        content.addArrangedSubview(keyRow())
        content.addArrangedSubview(section("Info"))
        content.addArrangedSubview(info())

        let scroll = UIScrollView()
        scroll.accessibilityIdentifier = "stub_page"
        scroll.translatesAutoresizingMaskIntoConstraints = false
        scroll.addSubview(content)
        view.addSubview(scroll)

        NSLayoutConstraint.activate([
            scroll.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            scroll.trailingAnchor.constraint(equalTo: view.trailingAnchor),
            scroll.topAnchor.constraint(equalTo: view.safeAreaLayoutGuide.topAnchor),
            scroll.bottomAnchor.constraint(equalTo: view.bottomAnchor),
            content.leadingAnchor.constraint(equalTo: scroll.contentLayoutGuide.leadingAnchor, constant: Self.gap),
            content.trailingAnchor.constraint(equalTo: scroll.contentLayoutGuide.trailingAnchor, constant: -Self.gap),
            content.topAnchor.constraint(equalTo: scroll.contentLayoutGuide.topAnchor, constant: Self.gap),
            content.bottomAnchor.constraint(equalTo: scroll.contentLayoutGuide.bottomAnchor, constant: -Self.gap),
            content.widthAnchor.constraint(equalTo: scroll.frameLayoutGuide.widthAnchor, constant: -2 * Self.gap),
        ])

        updateInfo()
    }

    // ------------------------------------------------------------------
    // layout
    // ------------------------------------------------------------------

    private static var gap: CGFloat {
        #if os(tvOS)
        return 24
        #else
        return 12
        #endif
    }

    private func title(_ text: String, _ id: String) -> UILabel {
        let label = label(text, id)
        label.font = .preferredFont(forTextStyle: .largeTitle)
        label.adjustsFontForContentSizeCategory = true
        label.textAlignment = .center
        return label
    }

    private func section(_ text: String) -> UILabel {
        let label = label(text, "stub_section_\(text.lowercased())")
        label.font = .preferredFont(forTextStyle: .headline)
        return label
    }

    private func label(_ text: String, _ id: String) -> UILabel {
        let label = UILabel()
        label.text = text
        label.accessibilityIdentifier = id
        label.numberOfLines = 0
        return label
    }

    private func status(_ text: String) -> UILabel {
        let label = label(text, "stub_status")
        label.font = .preferredFont(forTextStyle: .body)
        return label
    }

    private func counter() -> UILabel {
        let label = label("taps: 0", "stub_counter")
        label.font = .monospacedDigitSystemFont(ofSize: UIFont.systemFontSize, weight: .regular)
        return label
    }

    private func echo() -> UILabel {
        let label = label("echo: ", "stub_echo")
        label.font = .monospacedSystemFont(ofSize: UIFont.systemFontSize, weight: .regular)
        return label
    }

    private func button(_ text: String, _ id: String, action: Selector) -> UIButton {
        let button = UIButton(type: .system)
        button.setTitle(text, for: .normal)
        button.accessibilityIdentifier = id
        button.addTarget(self, action: action, for: .primaryActionTriggered)
        button.titleLabel?.font = .preferredFont(forTextStyle: .title3)
        return button
    }

    private func disabledButton() -> UIButton {
        let button = button("Disabled", "stub_disabled", action: #selector(onDisabled))
        button.isEnabled = false
        return button
    }

    private func textInput() -> UITextField {
        input.placeholder = "type here"
        input.accessibilityIdentifier = "stub_input"
        input.borderStyle = .roundedRect
        input.addTarget(self, action: #selector(onInputChanged), for: .editingChanged)
        return input
    }

    private func hiddenLabel() -> UILabel {
        let label = label("hidden unless toggled", "stub_hidden")
        label.isHidden = true
        return label
    }

    /// The scrollable fixture: rows `stub_row_1` … `stub_row_30` are off screen
    /// until scrolled to.
    private func scroller() -> UIScrollView {
        let scroller = UIScrollView()
        scroller.accessibilityIdentifier = "stub_scroller"
        scroller.translatesAutoresizingMaskIntoConstraints = false

        let rows = UIStackView()
        rows.axis = .vertical
        rows.spacing = 4
        rows.translatesAutoresizingMaskIntoConstraints = false
        for index in 1...30 {
            let row = label("Row \(index)", "stub_row_\(index)")
            row.font = .preferredFont(forTextStyle: .body)
            rows.addArrangedSubview(row)
        }
        scroller.addSubview(rows)

        NSLayoutConstraint.activate([
            scroller.heightAnchor.constraint(equalToConstant: Self.scrollerHeight),
            rows.leadingAnchor.constraint(equalTo: scroller.contentLayoutGuide.leadingAnchor),
            rows.trailingAnchor.constraint(equalTo: scroller.contentLayoutGuide.trailingAnchor),
            rows.topAnchor.constraint(equalTo: scroller.contentLayoutGuide.topAnchor),
            rows.bottomAnchor.constraint(equalTo: scroller.contentLayoutGuide.bottomAnchor),
            rows.widthAnchor.constraint(equalTo: scroller.frameLayoutGuide.widthAnchor),
        ])
        return scroller
    }

    private static var scrollerHeight: CGFloat {
        #if os(tvOS)
        return 400
        #else
        return 240
        #endif
    }

    private func keyLog() -> UILabel {
        keyLogLabel.text = "keys: "
        keyLogLabel.accessibilityIdentifier = "stub_key_log"
        keyLogLabel.font = .monospacedSystemFont(ofSize: UIFont.systemFontSize, weight: .regular)
        return keyLogLabel
    }

    private func keyRow() -> UIStackView {
        let row = UIStackView()
        row.axis = .horizontal
        row.spacing = Self.gap
        row.distribution = .fillEqually
        row.addArrangedSubview(button("A", "stub_key_a", action: #selector(onKeyA)))
        row.addArrangedSubview(button("B", "stub_key_b", action: #selector(onKeyB)))
        row.addArrangedSubview(button("C", "stub_key_c", action: #selector(onKeyC)))
        return row
    }

    private func info() -> UILabel {
        infoLabel.accessibilityIdentifier = "stub_info"
        infoLabel.font = .preferredFont(forTextStyle: .footnote)
        return infoLabel
    }

    // ------------------------------------------------------------------
    // actions
    // ------------------------------------------------------------------

    @objc private func onTap() {
        taps += 1
        counterLabel.text = "taps: \(taps)"
        statusLabel.text = "status: tapped \(taps)×"
        updateInfo()
    }

    @objc private func onSelect() {
        let sheet = UIAlertController(title: "Choose", message: nil, preferredStyle: .actionSheet)
        for option in ["Option A", "Option B", "Option C"] {
            sheet.addAction(UIAlertAction(title: option, style: .default) { [weak self] _ in
                self?.statusLabel.text = "status: selected \(option)"
            })
        }
        sheet.addAction(UIAlertAction(title: "Cancel", style: .cancel))
        present(sheet, animated: !UIView.areAnimationsEnabled)
    }

    @objc private func onEnabled() {
        statusLabel.text = "status: enabled pressed"
    }

    @objc private func onDisabled() {
        // Never fires: the button is disabled (fixture for `enabled` checks).
    }

    @objc private func onInputChanged() {
        echoLabel.text = "echo: \(input.text ?? "")"
    }

    @objc private func onKeyA() { onKey("a") }
    @objc private func onKeyB() { onKey("b") }
    @objc private func onKeyC() { onKey("c") }

    private func onKey(_ key: String) {
        keys.append(key)
        keyLogLabel.text = "keys: \(keys.joined())"
        statusLabel.text = "status: key \(key)"
    }

    // ------------------------------------------------------------------
    // plumbing
    // ------------------------------------------------------------------

    private func updateInfo() {
        let animations = UIView.areAnimationsEnabled ? "on" : "off"
        let version = ProcessInfo.processInfo.operatingSystemVersion
        infoLabel.text = [
            "bundle: \(Bundle.main.bundleIdentifier ?? "?")",
            "os: \(Self.osName) \(version.majorVersion).\(version.minorVersion)",
            "animations: \(animations)",
            "launch: \(ProcessInfo.processInfo.arguments.dropFirst().joined(separator: " "))",
        ].joined(separator: "\n")
    }

    private static var osName: String {
        #if os(tvOS)
        return "tvOS"
        #else
        return "iOS"
        #endif
    }
}
