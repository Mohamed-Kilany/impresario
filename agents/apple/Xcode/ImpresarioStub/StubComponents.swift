import UIKit

/// Shared visual language for the fixture. The logical sizes, gaps, colours
/// and copy come from `agents/android/agent-target-stub`; the target is an
/// exact mirror, not a lookalike, so both platforms can be driven by the same
/// plays. Android expresses the sizes in dp/sp and the TV runs at density 2.0
/// (the hardware run reports `1920x1080 @2.00x`), while tvOS points are 1:1
/// pixels on the same 1080p panel. `dp()` therefore scales Android logical
/// units by 2 on tvOS; iOS keeps the 1:1 phone/tablet mapping.
enum StubTheme {
    static var scale: CGFloat {
        #if os(tvOS)
        return 2
        #else
        return 1
        #endif
    }

    /// Converts an Android dp/sp value into the matching Apple point size.
    static func dp(_ value: CGFloat) -> CGFloat { value * scale }

    // Android Theme.DeviceDefault on this TCL TV: black action bar, #303030
    // content, #C1C1C1 primary text, #5A595B button, #646365 focused button.
    static let background = UIColor(white: 48.0 / 255.0, alpha: 1)
    static let appBarBackground = UIColor.black
    static let appBarForeground = UIColor.white
    static let foreground = UIColor(white: 193.0 / 255.0, alpha: 1)
    // Exact pixels sampled from the Android TV reference screenshots. The
    // platform Button is not neutral grey: the channels differ by one.
    static let buttonBackground = UIColor(
        red: 90.0 / 255.0,
        green: 89.0 / 255.0,
        blue: 91.0 / 255.0,
        alpha: 1
    )
    static let buttonFocusedBackground = UIColor(
        red: 100.0 / 255.0,
        green: 99.0 / 255.0,
        blue: 101.0 / 255.0,
        alpha: 1
    )
    static let buttonDisabledBackground = UIColor(white: 61.0 / 255.0, alpha: 1)
    /// Android's TV button label is pure white, unlike the #C1C1C1 text views.
    static let buttonTitle = UIColor.white
    static let buttonDisabledTitle = UIColor(white: 120.0 / 255.0, alpha: 1)
    static let inputHint = UIColor(white: 0.6, alpha: 1)
    static let inputUnderline = UIColor(
        red: 128.0 / 255.0,
        green: 203.0 / 255.0,
        blue: 196.0 / 255.0,
        alpha: 1
    )
    static let inputIdleUnderline = UIColor(white: 0.42, alpha: 1)
    // Theme.DeviceDefault's AlertDialog on this TV is teal-tinted, not the
    // #303030 used by activities: #324248 surface, #3F4E53 selected row.
    static let dialogBackground = UIColor(
        red: 50.0 / 255.0,
        green: 66.0 / 255.0,
        blue: 72.0 / 255.0,
        alpha: 1
    )
    static let dialogTitle = UIColor(
        red: 200.0 / 255.0,
        green: 203.0 / 255.0,
        blue: 204.0 / 255.0,
        alpha: 1
    )
    static let dialogFocusedBackground = UIColor(
        red: 63.0 / 255.0,
        green: 78.0 / 255.0,
        blue: 83.0 / 255.0,
        alpha: 1
    )
    static let mediaBrokenBackground = UIColor(white: 32.0 / 255.0, alpha: 1)
    static let plainFocusedBackground = dialogFocusedBackground

    /// Android `MainActivity`/`ScenarioActivity` use 48 x 36 dp.
    static var horizontalInset: CGFloat { dp(48) }
    static var verticalInset: CGFloat { dp(36) }

    /// Android `MediaActivity` uses 24 x 20 dp.
    static var mediaHorizontalInset: CGFloat { dp(24) }
    static var mediaVerticalInset: CGFloat { dp(20) }

    /// Android `Ui.kt`: 10 dp on phones/tablets, 6 dp on television.
    static var stackGap: CGFloat {
        #if os(tvOS)
        return dp(6)
        #else
        return dp(10)
        #endif
    }

    /// Android `ScrollActivity` hardcodes `STACK_GAP_DP` (10 dp) above the
    /// scroller even on television; the status line below uses `stackGap`.
    static var fixedStackGap: CGFloat { dp(10) }

    /// Android TextView line boxes measured from the TV's accessibility tree:
    /// 24 sp -> 65 px, 18 sp -> 49 px, 16 sp -> 43 px, 15 sp -> 41 px. UIKit's
    /// SF metrics are shorter, so single-line labels pin the measured height
    /// to keep every following row on the same pixel as Android.
    static func titleLabel(_ text: String, lineHeight: CGFloat? = nil) -> UILabel {
        let label = UILabel()
        label.text = text
        label.textColor = foreground
        label.font = .systemFont(ofSize: dp(24), weight: .bold)
        label.numberOfLines = 0
        if let lineHeight {
            label.heightAnchor.constraint(equalToConstant: dp(lineHeight)).isActive = true
        }
        return label
    }

    static func label(
        _ text: String,
        size: CGFloat = 18,
        weight: UIFont.Weight = .regular,
        lineHeight: CGFloat? = nil
    ) -> UILabel {
        let label = UILabel()
        label.text = text
        label.textColor = foreground
        label.font = .systemFont(ofSize: dp(size), weight: weight)
        label.numberOfLines = 0
        if let lineHeight {
            label.heightAnchor.constraint(equalToConstant: dp(lineHeight)).isActive = true
        }
        return label
    }

    static func statusLabel(_ text: String = "status=idle") -> UILabel {
        let label = label(text, size: 16, lineHeight: 21.5)
        label.alpha = 0.85
        return label
    }

    private static func baseButton(_ title: String, id: String?) -> StubButton {
        // `.custom` keeps UIKit's system button fill out of the row. Android
        // paints only the 36 dp nine-patch bar inside the 48 dp row; the
        // custom background view below owns that bar and its focus colour.
        let button = StubButton(type: .custom)
        button.setTitle(title, for: .normal)
        button.accessibilityIdentifier = id
        button.titleLabel?.font = .systemFont(ofSize: dp(18))
        button.setTitleColor(buttonTitle, for: .normal)
        button.setTitleColor(buttonDisabledTitle, for: .disabled)
        // Android's platform Button centers its label inside the 12 dp
        // horizontal content padding. The dialog rows below opt back into a
        // left-aligned label because AlertDialog list items are not Buttons.
        button.contentHorizontalAlignment = .center
        button.backgroundColor = .clear
        return button
    }

    static func button(_ title: String, id: String) -> StubButton {
        let button = baseButton(title, id: id)
        button.heightAnchor.constraint(greaterThanOrEqualToConstant: dp(48)).isActive = true

        // Android's 48 dp Button draws a 36 dp background inside the 48 dp
        // view (the TV theme's 9-patch leaves 6 dp transparent above/below).
        // Reproduce that so the row pitch is 48 + 6 dp while the visible bar
        // stays 36 dp, exactly as in the Android screenshot.
        let background = UIView()
        background.translatesAutoresizingMaskIntoConstraints = false
        background.isUserInteractionEnabled = false
        background.isAccessibilityElement = false
        background.backgroundColor = buttonBackground
        background.layer.cornerRadius = dp(2)
        button.insertSubview(background, at: 0)
        NSLayoutConstraint.activate([
            // The TV 9-patch leaves 4 dp transparent on the left/right too:
            // Android paints x=104..1815 inside the 96..1824 content column.
            background.leadingAnchor.constraint(equalTo: button.leadingAnchor, constant: dp(4)),
            background.trailingAnchor.constraint(equalTo: button.trailingAnchor, constant: -dp(4)),
            background.topAnchor.constraint(equalTo: button.topAnchor, constant: dp(6)),
            background.bottomAnchor.constraint(equalTo: button.bottomAnchor, constant: -dp(6)),
        ])
        button.stubBackgroundView = background
        return button
    }

    static func plainButton(_ title: String, id: String? = nil) -> StubButton {
        let button = baseButton(title, id: id)
        button.heightAnchor.constraint(greaterThanOrEqualToConstant: dp(48)).isActive = true
        button.contentHorizontalAlignment = .leading
        button.plainStyle = true
        return button
    }
}

extension UIView {
    /// The anchor the fixture measures its Android dp insets from. tvOS
    /// reports an overscan safe area (40 pt per side on the 1080p panel), but
    /// Android TV's 48 dp insets are measured from the screen edge. The
    /// fixture therefore opts out of the safe area on tvOS only; iOS keeps it
    /// for notches and the home indicator.
    var fixtureLeadingAnchor: NSLayoutXAxisAnchor {
        #if os(tvOS)
        return leadingAnchor
        #else
        return safeAreaLayoutGuide.leadingAnchor
        #endif
    }

    var fixtureTrailingAnchor: NSLayoutXAxisAnchor {
        #if os(tvOS)
        return trailingAnchor
        #else
        return safeAreaLayoutGuide.trailingAnchor
        #endif
    }

    var fixtureBottomAnchor: NSLayoutYAxisAnchor {
        #if os(tvOS)
        return bottomAnchor
        #else
        return safeAreaLayoutGuide.bottomAnchor
        #endif
    }
}

#if os(tvOS)
/// Android's platform action bar. Every stub Activity carries the app label in
/// a black 48 dp bar above the #303030 content; the Apple mirror adds the same
/// bar because the hidden-bar navigation controller has no system equivalent.
final class StubAppBar: UIView {
    private let label = UILabel()
    private let elevation = CAGradientLayer()

    init() {
        super.init(frame: .zero)
        translatesAutoresizingMaskIntoConstraints = false
        backgroundColor = StubTheme.appBarBackground

        label.text = "Impresario Stub"
        label.textColor = StubTheme.appBarForeground
        // The Android action bar title measures 14 sp: a 19 px cap height and
        // a 195 px run for "Impresario Stub".
        label.font = .systemFont(ofSize: StubTheme.dp(14), weight: .medium)
        label.translatesAutoresizingMaskIntoConstraints = false
        addSubview(label)

        NSLayoutConstraint.activate([
            heightAnchor.constraint(equalToConstant: StubTheme.dp(48)),
            // Android's action bar title starts at 16 dp.
            label.leadingAnchor.constraint(equalTo: leadingAnchor, constant: StubTheme.dp(16)),
            label.centerYAnchor.constraint(equalTo: centerYAnchor),
        ])

        // Android draws a 14 px elevation gradient under the black bar; its
        // exact samples are 32..48 (of 48) over the 14 rows below the bar.
        let alphas: [CGFloat] = [
            16.0 / 48.0, 13.0 / 48.0, 10.0 / 48.0, 8.0 / 48.0,
            7.0 / 48.0, 6.0 / 48.0, 5.0 / 48.0, 4.0 / 48.0,
            3.0 / 48.0, 2.0 / 48.0, 2.0 / 48.0, 1.0 / 48.0,
            1.0 / 48.0, 1.0 / 48.0, 0,
        ]
        elevation.colors = alphas.map { UIColor(white: 0, alpha: $0).cgColor }
        elevation.locations = (0..<alphas.count).map {
            NSNumber(value: Double($0) / Double(alphas.count - 1))
        }
        elevation.startPoint = CGPoint(x: 0.5, y: 0)
        elevation.endPoint = CGPoint(x: 0.5, y: 1)
        layer.addSublayer(elevation)
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        elevation.frame = CGRect(
            x: 0,
            y: bounds.height,
            width: bounds.width,
            height: StubTheme.dp(7)
        )
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) is not supported")
    }
}
#endif

/// A UIButton with the fixture's two behaviours that UIKit does not provide
/// out of the box: a long-press that matches Android's long-click, and a
/// single place to observe every remote press (needed by the Key log screen).
final class StubButton: UIButton {
    var onLongPress: (() -> Void)?
    var onPress: ((UIPress.PressType) -> Void)?
    #if os(tvOS)
    /// Called when the button becomes focused. The Key log screen uses it to
    /// mirror Android's D-pad key log if tvOS moves focus without delivering
    /// the arrow press to the focused control.
    var onFocusBecame: (() -> Void)?
    #endif
    var titleInset: CGFloat = 0
    var plainStyle = false
    /// The Android 36 dp bar inside the 48 dp row on television; nil for the
    /// plain dialog rows that paint their own full-row highlight.
    weak var stubBackgroundView: UIView?

    private var isFocusedState = false

    /// Android's default long-press timeout is 500 ms.
    private static let longPressDuration: TimeInterval = 0.5
    /// Android delivers exactly one click per select. On tvOS the same select
    /// can reach the button through UIKit's tracking path and the remote
    /// responder path, so coalesce duplicate primary actions.
    private static let actionCoalescing: TimeInterval = 0.25
    private var selectStarted: Date?
    private var lastPrimaryAction: Date?
    private var suppressPrimaryUntil: Date?
    private var lastPressWasLong = false

    override var isEnabled: Bool {
        didSet { updateVisualState() }
    }

    #if os(tvOS)
    override func sendActions(for controlEvents: UIControl.Event) {
        if controlEvents == .primaryActionTriggered {
            let now = Date()
            if let until = suppressPrimaryUntil {
                if now < until {
                    suppressPrimaryUntil = nil
                    return
                }
                suppressPrimaryUntil = nil
            }
            if let last = lastPrimaryAction, now.timeIntervalSince(last) < Self.actionCoalescing {
                return
            }
            lastPrimaryAction = now
        }
        super.sendActions(for: controlEvents)
    }
    #endif

    /// True when the primary action being delivered belongs to a long select.
    /// Android suppresses the click after a long-click; the Basic screen uses
    /// this even if UIKit delivers the click before `pressesEnded` runs.
    func consumeLongPress() -> Bool {
        if lastPressWasLong {
            lastPressWasLong = false
            return true
        }
        if let started = selectStarted,
           Date().timeIntervalSince(started) >= Self.longPressDuration {
            return true
        }
        return false
    }

    override func titleRect(forContentRect contentRect: CGRect) -> CGRect {
        var rect = super.titleRect(forContentRect: contentRect)
        guard titleInset > 0 else { return rect }
        rect.origin.x += titleInset
        if rect.width > titleInset {
            rect.size.width -= titleInset
        }
        return rect
    }

    #if os(tvOS)
    override func pressesBegan(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
        var handledSelect = false
        for press in presses {
            onPress?(press.type)
            if press.type == .select {
                if selectStarted == nil {
                    selectStarted = Date()
                    lastPressWasLong = false
                }
                handledSelect = true
            }
        }
        if handledSelect {
            return
        }
        super.pressesBegan(presses, with: event)
    }

    override func pressesEnded(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
        var remaining = presses
        for press in presses where press.type == .select {
            remaining.remove(press)
            guard let started = selectStarted else { continue }
            selectStarted = nil
            let held = Date().timeIntervalSince(started)
            if held >= Self.longPressDuration, let onLongPress {
                lastPressWasLong = true
                suppressPrimaryUntil = Date().addingTimeInterval(Self.actionCoalescing)
                onLongPress()
            } else {
                sendActions(for: .primaryActionTriggered)
            }
        }
        if !remaining.isEmpty {
            super.pressesEnded(remaining, with: event)
        }
    }

    override func pressesCancelled(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
        selectStarted = nil
        lastPressWasLong = false
        super.pressesCancelled(presses, with: event)
    }

    override func didUpdateFocus(
        in context: UIFocusUpdateContext,
        with coordinator: UIFocusAnimationCoordinator
    ) {
        super.didUpdateFocus(in: context, with: coordinator)
        let focused = context.nextFocusedView === self
        isFocusedState = focused
        coordinator.addCoordinatedAnimations({
            self.applyFocusAppearance(focused)
        }, completion: nil)
        if focused {
            onFocusBecame?()
        }
    }
    #endif

    private func updateVisualState() {
        guard let stubBackgroundView else { return }
        if !isEnabled {
            stubBackgroundView.backgroundColor = StubTheme.buttonDisabledBackground
        } else if isFocusedState {
            stubBackgroundView.backgroundColor = StubTheme.buttonFocusedBackground
        } else {
            stubBackgroundView.backgroundColor = StubTheme.buttonBackground
        }
    }

    private func applyFocusAppearance(_ focused: Bool) {
        if plainStyle {
            backgroundColor = focused ? StubTheme.plainFocusedBackground : .clear
            setTitleColor(StubTheme.buttonTitle, for: .normal)
        } else {
            updateVisualState()
        }
    }
}

/// A conversation with the Android `ScrollView` on the Scroller screen: it can
/// take focus, and D-pad up/down scroll it instead of moving focus away.
final class FocusableScrollView: UIScrollView {
    override var canBecomeFocused: Bool { true }

    #if os(tvOS)
    override func pressesBegan(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
        var handled = false
        for press in presses {
            switch press.type {
            case .upArrow:
                scrollBy(-StubTheme.dp(120))
                handled = true
            case .downArrow:
                scrollBy(StubTheme.dp(120))
                handled = true
            default:
                break
            }
        }
        if !handled {
            super.pressesBegan(presses, with: event)
        }
    }

    private func scrollBy(_ delta: CGFloat) {
        let maximum = max(0, contentSize.height - bounds.height)
        let target = min(max(0, contentOffset.y + delta), maximum)
        setContentOffset(CGPoint(x: 0, y: target), animated: true)
    }
    #endif
}
