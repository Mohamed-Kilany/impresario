import Foundation

/// Executes one command against the `UIDriver` (spec 12.3). Pure of gRPC and
/// XCTest, so every behavior is testable with a fake driver.
///
/// Cancellation: the dispatcher cancels the `Task`; poll loops re-check
/// cancellation at every sleep point, so a cancel takes effect within at most
/// one poll interval (spec 12.3 "Concurrency"). Driver calls are synchronous
/// and not interruptible on the real device — the documented latency bound.
public final class CommandExecutor: @unchecked Sendable {
    private let driver: UIDriver
    private let options: ExecutorOptions
    private let now: () -> Int
    private let sleep: (Int) async throws -> Void
    private let log: (LogLevel, String, String) -> Void
    private let extras: DriverExtras?
    private var warnedAboutClearData = false

    public init(
        driver: UIDriver,
        options: ExecutorOptions,
        now: @escaping () -> Int = { Int(Date().timeIntervalSince1970 * 1000) },
        sleep: @escaping (Int) async throws -> Void = { ms in
            try await Task.sleep(nanoseconds: UInt64(max(ms, 1)) * 1_000_000)
        },
        log: @escaping (LogLevel, String, String) -> Void = { _, _, _ in }
    ) {
        self.driver = driver
        self.options = options
        self.now = now
        self.sleep = sleep
        self.log = log
        self.extras = driver as? DriverExtras
    }

    public var executorOptions: ExecutorOptions { options }

    /// Executes a command and returns its result. Never throws: cancellation
    /// becomes a `CANCELLED` result (spec 7.2 rule 5).
    public func execute(_ command: AgentCommand) async -> CommandResultData {
        let started = now()
        let outcome: Outcome
        do {
            outcome = try await runBody(command)
        } catch let error as AgentError {
            outcome = .failed(error)
        } catch is CancellationError {
            outcome = .failed(.cancelled())
        } catch {
            outcome = .failed(.internalError(String(describing: error)))
        }

        var finalOutcome = outcome
        if case .ok = finalOutcome, crashed() {
            finalOutcome = .failed(.appCrashed("the app under test crashed during the command"))
        }
        return toResult(command.id, finalOutcome, max(0, now() - started))
    }

    // ------------------------------------------------------------------
    // bodies
    // ------------------------------------------------------------------

    private func runBody(_ command: AgentCommand) async throws -> Outcome {
        let timeout = max(1, command.timeoutMs)
        switch command.body {
        case .runAction(let action):
            return try await runAction(action, timeout)
        case .validateCheck(let check):
            return try await validateCheck(check, timeout)
        case .keyPress(let press):
            return try keyPress(press)
        case .screenshot:
            return try screenshot()
        case .appControl(let control):
            return try await appControl(control)
        case .probe(let probe):
            let result = try await ProbeCollector(
                driver: driver,
                extras: extras,
                now: now,
                sleep: sleep
            ).collect(probe, timeoutMs: timeout)
            return .ok(probe: result)
        case .cancel:
            throw AgentError.protocolError("cancel commands are handled by the session dispatcher")
        case .unspecified:
            throw AgentError.unsupportedAction("the command has no body")
        }
    }

    private func runAction(_ action: RunActionSpec, _ timeout: Int) async throws -> Outcome {
        switch action.kind {
        case .click:
            return try await click(action, timeout)
        case .type:
            return try await enterText(.replace, action, timeout)
        case .append:
            return try await enterText(.append, action, timeout)
        case .clear:
            return try await enterText(.clear, action, timeout)
        case .longPress:
            return try await longPress(action, timeout)
        case .focus:
            return try await focus(action, timeout)
        case .select:
            return try await select(action, timeout)
        case .scroll:
            return try await scroll(action, timeout)
        case .scrollTo:
            return try await scrollTo(action, timeout)
        case .swipe:
            return try await swipe(action, timeout)
        case .back:
            return try await back()
        }
    }

    // ------------------------------------------------------------------
    // actions
    // ------------------------------------------------------------------

    private func click(_ action: RunActionSpec, _ timeout: Int) async throws -> Outcome {
        let spec = try requireSelector(action.selector, "click")
        let element = try await waitClickable(spec, timeout)
        try await performClick(spec, element)
        return .ok()
    }

    private func performClick(_ spec: SelectorSpec, _ element: ElementRef) async throws {
        if options.platform.isTv {
            do {
                try await ensureFocused(spec, element)
                try driver.remote(.select, repeat: 1, interval: 0.1)
            } catch let error as AgentError where error.code == .focusUnreachable {
                // tvOS has no touch input: `IMPXCUShim.tap` always reports
                // "tap is unavailable on tvOS; use focus and Select". Keep the
                // focus error (with its trail) instead of masking it with that
                // dead-end tap.
                throw error
            }
        } else {
            try driver.tap(element)
        }
    }

    private enum EnterMode {
        case replace
        case append
        case clear
    }

    private func enterText(_ mode: EnterMode, _ action: RunActionSpec, _ timeout: Int) async throws -> Outcome {
        let spec = try requireSelector(action.selector, "text entry")
        let resolved = try await waitInteractable(spec, timeout)
        let element = resolved.element

        // Read the field *before* touching it (I-71): on TVs the activation
        // OK can reach an already-open keyboard and insert a stray keystroke.
        // The wait already paid for this attribute batch (I-198).
        let currentText = resolved.attributes.text

        // Focus: tap on iOS; D-pad focus then Select (opens the keyboard) on
        // tvOS (spec 12.3). Once the keyboard is open it owns D-pad focus, so
        // append/clear must keep that editing session instead of traversing
        // back to the field (which is blocked by the keyboard).
        if options.platform.isTv {
            if extras?.keyboardVisible() != true {
                try await ensureFocused(spec, element)
                try driver.remote(.select, repeat: 1, interval: 0.1)
            }
        } else {
            try driver.tap(element)
        }

        let next: String
        switch mode {
        case .replace:
            next = action.text
        case .append:
            next = currentText + action.text
        case .clear:
            next = ""
        }

        // Whole-field rewrite (I-87): delete the current content, then type
        // the next value. The spec's tap-position cursor for `append` is not
        // guaranteed; delete + retype is deterministic and verified below.
        if !currentText.isEmpty {
            try driver.deleteAll(element, currentText: currentText)
        }
        if !next.isEmpty {
            try driver.typeText(element, next)
        }

        let readBack = attributes(element).text
        if readBack == next {
            return .ok(observedText: readBack)
        }
        if mode == .clear {
            throw AgentError.notInteractable(
                "clear left '\(readBack)' in the field",
                ["selector": spec.describe(), "observed": readBack]
            )
        }
        throw AgentError.notInteractable(
            "text entry did not take effect (expected '\(next)', observed '\(readBack)')",
            ["selector": spec.describe(), "expected": next, "observed": readBack]
        )
    }

    private func longPress(_ action: RunActionSpec, _ timeout: Int) async throws -> Outcome {
        let spec = try requireSelector(action.selector, "longpress")
        let resolved = try await waitInteractable(spec, timeout)
        let element = resolved.element
        let duration = action.durationMs == 0 ? 1.0 : Double(action.durationMs) / 1000.0
        if options.platform.isTv {
            try await ensureFocused(spec, element)
            let pressed = try extras?.longPressSelect(duration: duration) ?? false
            if !pressed {
                throw AgentError.unsupportedAction(
                    "long-press on tvos needs the driver's XCUIRemote select long-press support"
                )
            }
        } else {
            try driver.press(element, duration: duration)
        }
        return .ok()
    }

    private func focus(_ action: RunActionSpec, _ timeout: Int) async throws -> Outcome {
        guard options.platform.isTv else {
            throw AgentError.unsupportedAction(
                "focus is not supported on ios (out-of-process automation cannot move focus)"
            )
        }
        let spec = try requireSelector(action.selector, "focus")
        let matches = try await awaitMatches(spec, timeout) { pick(spec, $0) != nil }
        guard let element = pick(spec, matches) else {
            throw AgentError.elementNotFound("the element \(spec.describe()) was not found", spec.describe())
        }
        try await ensureFocused(spec, element)
        return .ok()
    }

    private func select(_ action: RunActionSpec, _ timeout: Int) async throws -> Outcome {
        guard !options.platform.isTv else {
            throw AgentError.unsupportedAction("select is not supported on tvos (spec 6.1)")
        }
        let spec = try requireSelector(action.selector, "select")
        let trigger = try await waitInteractable(spec, timeout).element
        if let extras, try extras.selectValue(trigger, action.text) {
            return .ok(observedText: action.text)
        }
        try driver.tap(trigger)

        let optionText = action.text
        let optionSpec = SelectorSpec(strategy: .label, value: optionText, index: 0)
        let matches = try await awaitMatches(optionSpec, timeout) { !$0.isEmpty }
        guard let option = matches.first else {
            throw AgentError(
                code: .elementNotFound,
                message: "the option '\(optionText)' was not found after opening \(spec.describe())",
                details: ["selector": spec.describe(), "option": optionText]
            )
        }
        try driver.tap(option)
        return .ok(observedText: optionText)
    }

    private func scroll(_ action: RunActionSpec, _ timeout: Int) async throws -> Outcome {
        let direction = try requireDirection(action.direction)
        let steps = max(1, action.count)
        let containerSpec = action.selector

        if options.platform.isTv {
            if let containerSpec {
                let container = try await waitExisting(containerSpec, timeout)
                try await ensureFocused(containerSpec, container)
            }
            for _ in 0..<steps {
                try driver.remote(direction.remoteButton, repeat: 1, interval: 0.1)
            }
            return .ok()
        }

        var container: ElementRef?
        if let containerSpec {
            container = try await waitExisting(containerSpec, timeout)
        }
        try driver.scroll(container, direction, steps: steps)
        return .ok()
    }

    private func scrollTo(_ action: RunActionSpec, _ timeout: Int) async throws -> Outcome {
        let spec = try requireSelector(action.selector, "scrollto")
        let deadline = now() + timeout
        var attempts = 0
        var scrollingUp = false
        let screenHeight = CGFloat(driver.deviceInfo().screenHeightPx)

        while true {
            try Task.checkCancellation()
            let found = pick(spec, try driver.find(spec.lookup(), timeout: 0))
            let target = found.map { attributes($0) }
            if let target, target.visible {
                return .ok()
            }
            if now() >= deadline || attempts >= options.scrollMaxAttempts {
                break
            }
            // The target frame is a direction signal when XCUITest can report
            // it (I-86): compare it with the screen and the focused viewport.
            // Without it, run half the budget downwards, then reverse (I-70).
            let fallback: Direction =
                (scrollingUp || attempts >= options.scrollMaxAttempts / 2) ? .up : .down
            var direction = fallback
            if let target, !target.frame.isEmpty {
                if target.frame.maxY <= 0 {
                    direction = .up
                } else if screenHeight > 0, target.frame.minY >= screenHeight {
                    direction = .down
                } else if let focused = driver.focusedFrame(), !focused.isEmpty {
                    if target.frame.maxY <= focused.minY {
                        direction = .up
                    } else if target.frame.minY >= focused.maxY {
                        direction = .down
                    }
                }
            }
            scrollingUp = direction == .up
            if options.platform.isTv {
                try driver.remote(direction.remoteButton, repeat: 1, interval: 0.1)
            } else {
                try driver.scroll(nil, direction, steps: 1)
            }
            attempts += 1
            // Apple has no "did the content move" signal (I-86), so the loop
            // pauses a poll interval: the deadline stays meaningful, the
            // content settles before the next visibility check, and the
            // device is not hammered.
            try await sleep(options.pollIntervalMs)
        }
        throw AgentError(
            code: .elementNotFound,
            message: "the element \(spec.describe()) was not reached after \(attempts) scroll steps",
            details: ["selector": spec.describe(), "scroll_attempts": String(attempts)]
        )
    }

    private func swipe(_ action: RunActionSpec, _ timeout: Int) async throws -> Outcome {
        guard !options.platform.isTv else {
            throw AgentError.unsupportedAction("swipe is not supported on tvos (no touch)")
        }
        let direction = try requireDirection(action.direction)
        var element: ElementRef?
        if let spec = action.selector {
            element = try await waitExisting(spec, timeout)
        }
        try driver.swipe(element, direction)
        return .ok()
    }

    private func back() async throws -> Outcome {
        if options.platform.isTv {
            // tvOS's full-screen keyboard consumes the first Menu press. After
            // text entry, dismiss that keyboard and then send the Menu the
            // screen expects, otherwise `back` leaves the scene in place.
            if extras?.keyboardVisible() == true {
                try driver.remote(.menu, repeat: 1, interval: 0.1)
                try await sleep(300)
            }
            try driver.remote(.menu, repeat: 1, interval: 0.1)
            return .ok()
        }
        let performed = try extras?.pressBack() ?? false
        if !performed {
            throw AgentError.unsupportedAction(
                "back is not supported by this driver (no navigation-bar back button and no edge swipe)"
            )
        }
        return .ok()
    }

    // ------------------------------------------------------------------
    // checks
    // ------------------------------------------------------------------

    private func validateCheck(_ check: ValidateCheckSpec, _ timeout: Int) async throws -> Outcome {
        guard let spec = check.selector else {
            throw AgentError.protocolError("the check requires a selector")
        }
        let expected: String
        switch check.kind {
        case .visible: expected = "visible"
        case .hidden: expected = "hidden"
        case .enabled: expected = "enabled"
        case .disabled: expected = "disabled"
        case .focused: expected = "focused"
        case .textEquals, .textContains: expected = check.expectedText
        case .count: expected = String(check.expectedCount)
        }

        let deadline = now() + timeout
        while true {
            try Task.checkCancellation()
            let matches = try driver.find(spec.lookup(), timeout: 0)
            let element = pick(spec, matches)
            let evaluation = evaluate(check, element: element, matchedCount: matches.count)
            if evaluation.satisfied {
                return .ok(observedText: evaluation.observed, observedCount: evaluation.matchedCount)
            }
            if now() >= deadline {
                throw AgentError.checkFailed(
                    "check failed for \(spec.describe()) (expected \(expected), observed \(evaluation.observed))",
                    expected: expected,
                    observed: evaluation.observed,
                    matchedCount: evaluation.matchedCount,
                    selector: spec.describe()
                )
            }
            try await sleep(options.pollIntervalMs)
        }
    }

    private struct CheckEvaluation {
        let satisfied: Bool
        let observed: String
        let matchedCount: Int
    }

    private func evaluate(
        _ check: ValidateCheckSpec,
        element: ElementRef?,
        matchedCount: Int
    ) -> CheckEvaluation {
        switch check.kind {
        case .visible, .hidden:
            let visible = element.map { attributes($0).visible } ?? false
            let observed: String
            if element == nil {
                observed = "missing"
            } else if visible {
                observed = "visible"
            } else {
                observed = "hidden"
            }
            return CheckEvaluation(
                satisfied: check.kind == .visible ? visible : !visible,
                observed: observed,
                matchedCount: matchedCount
            )
        case .enabled, .disabled:
            let enabled = element.map { attributes($0).enabled } ?? false
            let observed: String
            if element == nil {
                observed = "missing"
            } else if enabled {
                observed = "enabled"
            } else {
                observed = "disabled"
            }
            return CheckEvaluation(
                satisfied: check.kind == .enabled ? enabled : !enabled,
                observed: observed,
                matchedCount: matchedCount
            )
        case .focused:
            let focused = element.map { attributes($0).focused } ?? false
            let observed: String
            if element == nil {
                observed = "missing"
            } else if focused {
                observed = "focused"
            } else {
                observed = "not focused"
            }
            return CheckEvaluation(satisfied: focused, observed: observed, matchedCount: matchedCount)
        case .textEquals:
            let text = element.map { attributes($0).text }
            return CheckEvaluation(
                satisfied: element != nil && text == check.expectedText,
                observed: text ?? "missing",
                matchedCount: matchedCount
            )
        case .textContains:
            let text = element.map { attributes($0).text }
            return CheckEvaluation(
                satisfied: element != nil && (text ?? "").contains(check.expectedText),
                observed: text ?? "missing",
                matchedCount: matchedCount
            )
        case .count:
            return CheckEvaluation(
                satisfied: matchedCount == check.expectedCount,
                observed: String(matchedCount),
                matchedCount: matchedCount
            )
        }
    }

    // ------------------------------------------------------------------
    // keys, screenshots, app control
    // ------------------------------------------------------------------

    private func keyPress(_ press: KeyPressSpec) throws -> Outcome {
        switch KeyTable.resolve(press.key, platform: options.platform) {
        case .remote(let button):
            let repeatCount = max(1, press.repeatCount)
            let interval = TimeInterval(press.intervalMs == 0 ? 100 : press.intervalMs) / 1000.0
            try driver.remote(button, repeat: repeatCount, interval: interval)
        case .home:
            try driver.pressHome()
        case .unsupported(let reason):
            throw AgentError.unsupportedAction(reason)
        }
        return .ok()
    }

    private func screenshot() throws -> Outcome {
        let png = try driver.screenshotPNG()
        if png.isEmpty {
            throw AgentError.internalError("the driver returned an empty screenshot")
        }
        return .ok(png: png)
    }

    /// The Apple analogue of `pm clear` (I-82/I-84): the agent cannot wipe
    /// another app's sandbox. The host transport reinstalls the app under test
    /// when `options.app_path` is set (`docs/DEVICES.md` §3); with no reinstall
    /// the launch still becomes a cold start and a WARN is logged once.
    private func clearDataIfRequested(_ control: AppControlSpec) -> Bool {
        guard control.clearData || options.clearAppDataOnRestart else { return true }
        if !warnedAboutClearData {
            warnedAboutClearData = true
            log(
                .warn,
                "agent",
                "cannot clear app data for '\(options.appId)' from inside the app under test on " +
                    "\(options.platform.rawValue): the sandbox belongs to the device. Configure " +
                    "options.app_path so the host transport reinstalls the app under test " +
                    "(docs/DECISIONS.md I-84); this launch is a cold start with the old container."
            )
        }
        return false
    }

    private func appControl(_ control: AppControlSpec) async throws -> Outcome {
        switch control.op {
        case .launch:
            if !clearDataIfRequested(control) {
                try driver.terminate(options.appId)
            }
            try driver.launch(options.appId)
            return .ok()
        case .terminate:
            try driver.terminate(options.appId)
            return .ok()
        case .restart:
            try driver.terminate(options.appId)
            _ = clearDataIfRequested(control)
            try driver.launch(options.appId)
            return .ok()
        case .background:
            try driver.pressHome()
            return .ok()
        }
    }

    // ------------------------------------------------------------------
    // helpers
    // ------------------------------------------------------------------

    private func requireSelector(_ selector: SelectorSpec?, _ what: String) throws -> SelectorSpec {
        guard let selector else {
            throw AgentError.protocolError("the \(what) action requires a selector")
        }
        return selector
    }

    private func requireDirection(_ direction: Direction?) throws -> Direction {
        guard let direction else {
            throw AgentError.protocolError("the direction is unspecified")
        }
        return direction
    }

    private func pick(_ spec: SelectorSpec, _ matches: [ElementRef]) -> ElementRef? {
        if spec.index >= 0 {
            return spec.index < matches.count ? matches[spec.index] : nil
        }
        let fromEnd = matches.count + spec.index
        return fromEnd >= 0 && fromEnd < matches.count ? matches[fromEnd] : nil
    }

    /// The batch attribute read for one element. The real Apple driver reads
    /// one `XCUIElementSnapshot`; the fakes and any driver without the
    /// optional capability fall back to the individual protocol calls.
    private func attributes(_ element: ElementRef) -> ElementAttributes {
        if let extras, let batch = extras.attributes(element) {
            return batch
        }
        return ElementAttributes(
            exists: true,
            visible: driver.isVisible(element),
            enabled: driver.isEnabled(element),
            focused: driver.hasFocus(element),
            frame: driver.frame(element),
            text: driver.text(element)
        )
    }

    private struct ResolvedElement {
        let element: ElementRef
        let attributes: ElementAttributes
    }

    /// Waits until the indexed element exists, is visible and is enabled.
    private func waitInteractable(_ spec: SelectorSpec, _ timeout: Int) async throws -> ResolvedElement {
        let deadline = now() + max(1, timeout)
        while true {
            try Task.checkCancellation()
            let matches = try driver.find(spec.lookup(), timeout: 0)
            if let element = pick(spec, matches) {
                let attrs = attributes(element)
                if attrs.visible && attrs.enabled {
                    return ResolvedElement(element: element, attributes: attrs)
                }
                if now() >= deadline {
                    let reason = !attrs.visible ? "not visible" : "disabled"
                    throw AgentError.notInteractable(
                        "the element \(spec.describe()) is \(reason)",
                        ["selector": spec.describe(), "reason": reason]
                    )
                }
            } else if now() >= deadline {
                throw AgentError.elementNotFound(
                    "the element \(spec.describe()) was not found",
                    spec.describe()
                )
            }
            try await sleep(options.pollIntervalMs)
        }
    }

    /// `click` additionally needs hit-testability on iOS (spec 6.4).
    private func waitClickable(_ spec: SelectorSpec, _ timeout: Int) async throws -> ElementRef {
        let requireHittable = !options.platform.isTv
        let deadline = now() + max(1, timeout)
        while true {
            try Task.checkCancellation()
            let matches = try driver.find(spec.lookup(), timeout: 0)
            if let element = pick(spec, matches) {
                let attrs = attributes(element)
                // Only pay for the separate hittability read when the cheaper
                // attributes already pass; a hidden or disabled element has a
                // more specific error than "not hittable".
                if attrs.visible, attrs.enabled,
                   !requireHittable || driver.isHittable(element) {
                    return element
                }
                if now() >= deadline {
                    let reason = !attrs.visible
                        ? "not visible"
                        : (!attrs.enabled ? "disabled" : "not hittable")
                    throw AgentError.notInteractable(
                        "the element \(spec.describe()) is \(reason)",
                        ["selector": spec.describe(), "reason": reason]
                    )
                }
            } else if now() >= deadline {
                throw AgentError.elementNotFound(
                    "the element \(spec.describe()) was not found",
                    spec.describe()
                )
            }
            try await sleep(options.pollIntervalMs)
        }
    }

    /// Waits until the indexed element exists (containers and swipes).
    private func waitExisting(_ spec: SelectorSpec, _ timeout: Int) async throws -> ElementRef {
        let matches = try await awaitMatches(spec, timeout) { pick(spec, $0) != nil }
        guard let element = pick(spec, matches) else {
            throw AgentError.elementNotFound("the element \(spec.describe()) was not found", spec.describe())
        }
        return element
    }

    private func awaitMatches(
        _ spec: SelectorSpec,
        _ timeoutMs: Int,
        _ predicate: ([ElementRef]) -> Bool
    ) async throws -> [ElementRef] {
        let deadline = now() + max(1, timeoutMs)
        while true {
            try Task.checkCancellation()
            let matches = try driver.find(spec.lookup(), timeout: 0)
            if predicate(matches) { return matches }
            if now() >= deadline { return matches }
            try await sleep(options.pollIntervalMs)
        }
    }

    private func ensureFocused(_ spec: SelectorSpec, _ element: ElementRef) async throws {
        // The previous action (often `back`) may still be applying the
        // destination screen's focus reset. Let that animation settle before
        // reading the focus frame; otherwise the navigator can chase a stale
        // frame and overshoot the target.
        try await sleep(450)
        let probe = ExecutorFocusProbe(driver: driver, element: element, extras: extras)
        let result = FocusNavigator(probe: probe, maxSteps: options.focusMaxSteps).ensureTargetFocused()
        switch result {
        case .focused:
            return
        case .unreachable(let reason, let trail, let steps):
            throw AgentError.focusUnreachable(
                "could not reach \(spec.describe()) by D-pad after \(steps) steps (\(reason))",
                [
                    "selector": spec.describe(),
                    "reason": reason,
                    "steps": String(steps),
                    "focus_trail": FocusNavigator.trailJson(trail),
                ]
            )
        }
    }

    private func crashed() -> Bool {
        guard !options.appId.isEmpty else { return false }
        return extras?.targetCrashed(appId: options.appId) == true
    }

    private func toResult(_ id: String, _ outcome: Outcome, _ durationMs: Int) -> CommandResultData {
        switch outcome {
        case .ok(let observedText, let observedCount, let png, let probe):
            return CommandResultData(
                commandId: id,
                ok: true,
                durationMs: durationMs,
                observedText: observedText.isEmpty ? nil : observedText,
                observedCount: observedCount > 0 ? observedCount : nil,
                screenshotPng: png,
                probe: probe
            )
        case .failed(let error):
            return CommandResultData(commandId: id, ok: false, durationMs: durationMs, error: error.report)
        }
    }

    private enum Outcome {
        case ok(observedText: String = "", observedCount: Int = 0, png: Data? = nil, probe: ProbeResultSpec? = nil)
        case failed(AgentError)
    }
}

/// `FocusProbe` over the executor's driver (spec 6.3).
///
/// One focused-identity snapshot per press replaces the separate "before"
/// frame, "after" frame, target frame and `hasFocus` reads the navigator used
/// to pay for at every D-pad step (I-201). A tvOS home screen with eight rows
/// otherwise spends the whole 20 s click budget before reaching the last row.
private final class ExecutorFocusProbe: FocusProbe {
    private let driver: UIDriver
    private let element: ElementRef
    private let extras: DriverExtras?
    private var target: FocusIdentity?
    private var focused: FocusIdentity?

    init(driver: UIDriver, element: ElementRef, extras: DriverExtras?) {
        self.driver = driver
        self.element = element
        self.extras = extras
    }

    func currentFocusLabel() -> String? { focused?.label }

    func currentFocusFrame() -> CGRect? {
        if let focused {
            return focused.frame
        }
        return refreshFocused()?.frame
    }

    func targetFrame() -> CGRect? {
        guard let target = targetIdentity(), !target.frame.isEmpty else {
            return nil
        }
        return target.frame
    }

    func press(_ direction: Direction) -> Bool {
        do {
            let before = currentFocusFrame()
            try driver.remote(direction.remoteButton, repeat: 1, interval: 0.1)
            // Focus movement and the following scroll are animated. Querying
            // immediately sees the pre-press focus and makes the navigator
            // issue an extra step, which overshoots the last row.
            Thread.sleep(forTimeInterval: 0.45)
            let after = refreshFocused()?.frame

            // A scrollable focus container consumes the first press to scroll
            // an off-screen row into view. When the focus frame did not move
            // but the target is still that way, repeat once so the focus
            // engine can actually land on the now-visible row. Without this,
            // the last home row (Playback) was visible after the scroll but
            // never focused, and the navigator tried a perpendicular axis.
            if let before,
               let after,
               after == before,
               targetIsInDirection(direction, from: before) {
                try driver.remote(direction.remoteButton, repeat: 1, interval: 0.1)
                Thread.sleep(forTimeInterval: 0.45)
                _ = refreshFocused()
            }
            return true
        } catch {
            return false
        }
    }

    func isTargetFocused() -> Bool {
        guard let target = targetIdentity() else {
            return driver.hasFocus(element)
        }
        if focused == nil {
            _ = refreshFocused()
        }
        guard let focused else {
            return false
        }
        if focused.matches(target) {
            return true
        }
        // The focus engine may report a child node. Only then is `hasFocus` on
        // the target worth another round trip; disjoint frames cannot be it.
        if focused.frame.intersects(target.frame) {
            return driver.hasFocus(element)
        }
        return false
    }

    private func refreshFocused() -> FocusIdentity? {
        if let identity = extras?.focusedIdentity() {
            focused = identity
        } else if let frame = driver.focusedFrame() {
            focused = FocusIdentity(identifier: "", label: "", frame: frame)
        } else {
            focused = nil
        }
        return focused
    }

    private func targetIdentity() -> FocusIdentity? {
        if let target {
            return target
        }
        if let identity = extras?.elementIdentity(element) {
            target = identity
        } else {
            target = FocusIdentity(
                identifier: "",
                label: driver.text(element),
                frame: driver.frame(element)
            )
        }
        return target
    }

    private func targetIsInDirection(_ direction: Direction, from current: CGRect) -> Bool {
        guard let target = targetIdentity(), !target.frame.isEmpty else {
            return false
        }
        switch direction {
        case .up: return target.frame.midY < current.midY
        case .down: return target.frame.midY > current.midY
        case .left: return target.frame.midX < current.midX
        case .right: return target.frame.midX > current.midX
        }
    }
}
