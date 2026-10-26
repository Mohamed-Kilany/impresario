import UIKit

/// Shared visual language for the fixture. The sizes, gaps and copy come from
/// `agents/android/agent-target-stub`; the target is an exact mirror, not a
/// lookalike, so both platforms can be driven by the same plays.
enum StubTheme {
    static let background = UIColor.black
    static let foreground = UIColor.white
    static let buttonBackground = UIColor(white: 0.18, alpha: 1)
    static let buttonDisabledTitle = UIColor(white: 0.45, alpha: 1)
    static let inputBackground = UIColor(white: 0.12, alpha: 1)
    static let inputUnderline = UIColor(white: 0.55, alpha: 1)
    static let dialogBackground = UIColor(white: 0.12, alpha: 1)
    static let dialogSeparator = UIColor(white: 0.30, alpha: 1)
    static let mediaBrokenBackground = UIColor(white: 32.0 / 255.0, alpha: 1)

    /// Android `MainActivity`/`ScenarioActivity` use 48 x 36 dp.
    static let horizontalInset: CGFloat = 48
    static let verticalInset: CGFloat = 36

    /// Android `MediaActivity` uses 24 x 20 dp.
    static let mediaHorizontalInset: CGFloat = 24
    static let mediaVerticalInset: CGFloat = 20

    /// Android `Ui.kt`: 10 dp on phones/tablets, 6 dp on television.
    static var stackGap: CGFloat {
        #if os(tvOS)
        return 6
        #else
        return 10
        #endif
    }

    /// Android `ScrollActivity` hardcodes `STACK_GAP_DP` (10 dp) above the
    /// scroller even on television; the status line below uses `stackGap`.
    static let fixedStackGap: CGFloat = 10

    static func titleLabel(_ text: String) -> UILabel {
        let label = UILabel()
        label.text = text
        label.textColor = foreground
        label.font = .systemFont(ofSize: 24, weight: .bold)
        label.numberOfLines = 0
        return label
    }

    static func label(_ text: String, size: CGFloat = 18) -> UILabel {
        let label = UILabel()
        label.text = text
        label.textColor = foreground
        label.font = .systemFont(ofSize: size)
        label.numberOfLines = 0
        return label
    }

    static func statusLabel(_ text: String = "status=idle") -> UILabel {
        let label = label(text, size: 16)
        label.alpha = 0.85
        return label
    }

    static func button(_ title: String, id: String) -> StubButton {
        let button = StubButton(type: .system)
        button.setTitle(title, for: .normal)
        button.accessibilityIdentifier = id
        button.titleLabel?.font = .systemFont(ofSize: 18)
        button.setTitleColor(foreground, for: .normal)
        button.setTitleColor(buttonDisabledTitle, for: .disabled)
        button.backgroundColor = buttonBackground
        button.layer.cornerRadius = 4
        button.heightAnchor.constraint(greaterThanOrEqualToConstant: 48).isActive = true
        return button
    }

    static func plainButton(_ title: String, id: String? = nil) -> StubButton {
        let button = StubButton(type: .system)
        button.setTitle(title, for: .normal)
        button.accessibilityIdentifier = id
        button.titleLabel?.font = .systemFont(ofSize: 18)
        button.setTitleColor(foreground, for: .normal)
        button.contentHorizontalAlignment = .leading
        button.heightAnchor.constraint(greaterThanOrEqualToConstant: 48).isActive = true
        button.plainStyle = true
        return button
    }
}

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
        coordinator.addCoordinatedAnimations({
            self.applyFocusAppearance(focused)
        }, completion: nil)
        if focused {
            onFocusBecame?()
        }
    }
    #endif

    private func applyFocusAppearance(_ focused: Bool) {
        if plainStyle {
            backgroundColor = focused ? UIColor(white: 0.25, alpha: 1) : .clear
            setTitleColor(.white, for: .normal)
        } else if focused {
            backgroundColor = .white
            setTitleColor(.black, for: .normal)
        } else {
            backgroundColor = StubTheme.buttonBackground
            setTitleColor(StubTheme.foreground, for: .normal)
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
                scrollBy(-120)
                handled = true
            case .downArrow:
                scrollBy(120)
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
