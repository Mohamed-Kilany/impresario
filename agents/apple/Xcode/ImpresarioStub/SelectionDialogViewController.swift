import UIKit

/// The Android `AlertDialog` on the Selection screen: a centered "Pick one"
/// card with exactly three option rows and no Cancel button. Choosing a row
/// reports the option; tapping outside or pressing Menu cancels, matching
/// Android's cancellable dialog.
final class SelectionDialogViewController: UIViewController {
    private let dialogTitle: String
    private let options: [String]
    private let onChoice: (String) -> Void

    private let card = UIView()
    private var firstOption: StubButton?

    init(title: String, options: [String], onChoice: @escaping (String) -> Void) {
        self.dialogTitle = title
        self.options = options
        self.onChoice = onChoice
        super.init(nibName: nil, bundle: nil)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) is not supported")
    }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .clear

        let backdrop = BackdropControl()
        backdrop.backgroundColor = UIColor.black.withAlphaComponent(0.55)
        backdrop.translatesAutoresizingMaskIntoConstraints = false
        backdrop.addTarget(self, action: #selector(cancel), for: .touchUpInside)
        view.addSubview(backdrop)

        card.backgroundColor = StubTheme.dialogBackground
        card.layer.cornerRadius = StubTheme.dp(4)
        card.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(card)

        let title = StubTheme.titleLabel(dialogTitle)
        title.font = .systemFont(ofSize: StubTheme.dp(20), weight: .bold)
        title.translatesAutoresizingMaskIntoConstraints = false
        card.addSubview(title)

        let rows = UIStackView()
        rows.axis = .vertical
        rows.alignment = .fill
        rows.spacing = 0
        rows.translatesAutoresizingMaskIntoConstraints = false
        card.addSubview(rows)

        for (index, option) in options.enumerated() {
            if index > 0 {
                let separator = UIView()
                separator.backgroundColor = StubTheme.dialogSeparator
                separator.heightAnchor.constraint(equalToConstant: 1).isActive = true
                rows.addArrangedSubview(separator)
            }
            let button = StubTheme.plainButton(option)
            button.titleInset = StubTheme.dp(24)
            button.tag = index
            button.addTarget(self, action: #selector(choose(_:)), for: .primaryActionTriggered)
            rows.addArrangedSubview(button)
            if firstOption == nil {
                firstOption = button
            }
        }

        let width = card.widthAnchor.constraint(equalToConstant: StubTheme.dp(560))
        width.priority = .defaultHigh
        let maximumWidth = card.widthAnchor.constraint(
            lessThanOrEqualTo: view.safeAreaLayoutGuide.widthAnchor,
            constant: -StubTheme.dp(96)
        )

        NSLayoutConstraint.activate([
            backdrop.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            backdrop.trailingAnchor.constraint(equalTo: view.trailingAnchor),
            backdrop.topAnchor.constraint(equalTo: view.topAnchor),
            backdrop.bottomAnchor.constraint(equalTo: view.bottomAnchor),

            card.centerXAnchor.constraint(equalTo: view.centerXAnchor),
            card.centerYAnchor.constraint(equalTo: view.centerYAnchor),
            width,
            maximumWidth,

            title.leadingAnchor.constraint(equalTo: card.leadingAnchor, constant: StubTheme.dp(24)),
            title.trailingAnchor.constraint(equalTo: card.trailingAnchor, constant: -StubTheme.dp(24)),
            title.topAnchor.constraint(equalTo: card.topAnchor, constant: StubTheme.dp(20)),

            rows.leadingAnchor.constraint(equalTo: card.leadingAnchor),
            rows.trailingAnchor.constraint(equalTo: card.trailingAnchor),
            rows.topAnchor.constraint(equalTo: title.bottomAnchor, constant: StubTheme.dp(12)),
            rows.bottomAnchor.constraint(equalTo: card.bottomAnchor),
        ])
    }

    override var preferredFocusEnvironments: [UIFocusEnvironment] {
        if let firstOption {
            return [firstOption]
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

    override func accessibilityPerformEscape() -> Bool {
        cancel()
        return true
    }

    #if os(tvOS)
    override func pressesBegan(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
        if presses.contains(where: { $0.type == .menu }) {
            cancel()
            return
        }
        super.pressesBegan(presses, with: event)
    }
    #endif

    @objc private func choose(_ sender: StubButton) {
        guard options.indices.contains(sender.tag) else { return }
        let option = options[sender.tag]
        onChoice(option)
        dismiss(animated: UIView.areAnimationsEnabled)
    }

    @objc private func cancel() {
        dismiss(animated: UIView.areAnimationsEnabled)
    }
}

/// A full-screen dismiss target that must never join the focus chain.
private final class BackdropControl: UIControl {
    override var canBecomeFocused: Bool { false }
}
