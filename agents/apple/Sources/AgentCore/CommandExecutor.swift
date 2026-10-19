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
        case .cancel:
            throw AgentError.protocolError("cancel commands are handled by the session runner")
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
                // Fallback (I-86): some tvOS system surfaces (alerts, focus
                // traps) cannot be reached by D-pad traversal. If XCTest
                // reports the element hittable, tap it directly; otherwise
                // keep the honest focus error.
                if driver.isHittable(element) {
                    try driver.tap(element)
                } else {
                    throw error
                }
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
        let element = try await waitInteractable(spec, timeout)

        // Read the field *before* touching it (I-71): on TVs the activation
        // OK can reach an already-open keyboard and insert a stray keystroke.
        let currentText = driver.text(element)

        // Focus: tap on iOS; D-pad focus then Select (opens the keyboard) on
        // tvOS (spec 12.3).
        if options.platform.isTv {
            try await ensureFocused(spec, element)
            try driver.remote(.select, repeat: 1, interval: 0.1)
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
            try driver.deleteAll(element)
        }
        if !next.isEmpty {
            try driver.typeText(element, next)
        }

        let readBack = driver.text(element)
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
        let element = try await waitInteractable(spec, timeout)
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
        let trigger = try await waitInteractable(spec, timeout)
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

        while true {
            try Task.checkCancellation()
            if let element = pick(spec, try driver.find(spec.lookup(), timeout: 0)), driver.isVisible(element) {
                return .ok()
            }
            if now() >= deadline || attempts >= options.scrollMaxAttempts {
                break
            }
            // No movement signal exists on the Apple platforms (I-86): run
            // half the budget downwards, then reverse upwards (I-70).
            let direction: Direction =
                (scrollingUp || attempts >= options.scrollMaxAttempts / 2) ? .up : .down
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
            let visible = element.map { driver.isVisible($0) } ?? false
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
            let enabled = element.map { driver.isEnabled($0) } ?? false
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
            let focused = element.map { driver.hasFocus($0) } ?? false
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
            let text = element.map { driver.text($0) }
            return CheckEvaluation(
                satisfied: element != nil && text == check.expectedText,
                observed: text ?? "missing",
                matchedCount: matchedCount
            )
        case .textContains:
            let text = element.map { driver.text($0) }
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

    /// The Apple analogue of `pm clear` (I-82/I-84): the runner cannot wipe
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

    /// Waits until the indexed element exists, is visible and is enabled.
    private func waitInteractable(_ spec: SelectorSpec, _ timeout: Int) async throws -> ElementRef {
        let matches = try await awaitMatches(spec, timeout) { list in
            guard let element = pick(spec, list) else { return false }
            return driver.isVisible(element) && driver.isEnabled(element)
        }
        guard let element = pick(spec, matches) else {
            throw AgentError.elementNotFound("the element \(spec.describe()) was not found", spec.describe())
        }
        let visible = driver.isVisible(element)
        let enabled = driver.isEnabled(element)
        if !visible || !enabled {
            let reason = !visible ? "not visible" : "disabled"
            throw AgentError.notInteractable(
                "the element \(spec.describe()) is \(reason)",
                ["selector": spec.describe(), "reason": reason]
            )
        }
        return element
    }

    /// `click` additionally needs hit-testability on iOS (spec 6.4).
    private func waitClickable(_ spec: SelectorSpec, _ timeout: Int) async throws -> ElementRef {
        let requireHittable = !options.platform.isTv
        let matches = try await awaitMatches(spec, timeout) { list in
            guard let element = pick(spec, list) else { return false }
            let base = driver.isVisible(element) && driver.isEnabled(element)
            return base && (!requireHittable || driver.isHittable(element))
        }
        guard let element = pick(spec, matches) else {
            throw AgentError.elementNotFound("the element \(spec.describe()) was not found", spec.describe())
        }
        let visible = driver.isVisible(element)
        let enabled = driver.isEnabled(element)
        if !visible || !enabled {
            let reason = !visible ? "not visible" : "disabled"
            throw AgentError.notInteractable(
                "the element \(spec.describe()) is \(reason)",
                ["selector": spec.describe(), "reason": reason]
            )
        }
        if requireHittable && !driver.isHittable(element) {
            throw AgentError.notInteractable(
                "the element \(spec.describe()) is not hittable",
                ["selector": spec.describe(), "reason": "not hittable"]
            )
        }
        return element
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
        let probe = ExecutorFocusProbe(driver: driver, element: element)
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
        case .ok(let observedText, let observedCount, let png):
            return CommandResultData(
                commandId: id,
                ok: true,
                durationMs: durationMs,
                observedText: observedText.isEmpty ? nil : observedText,
                observedCount: observedCount > 0 ? observedCount : nil,
                screenshotPng: png
            )
        case .failed(let error):
            return CommandResultData(commandId: id, ok: false, durationMs: durationMs, error: error.report)
        }
    }

    private enum Outcome {
        case ok(observedText: String = "", observedCount: Int = 0, png: Data? = nil)
        case failed(AgentError)
    }
}

/// `FocusProbe` over the executor's driver (spec 6.3).
private final class ExecutorFocusProbe: FocusProbe {
    private let driver: UIDriver
    private let element: ElementRef

    init(driver: UIDriver, element: ElementRef) {
        self.driver = driver
        self.element = element
    }

    func currentFocusLabel() -> String? { nil }

    func currentFocusFrame() -> CGRect? { driver.focusedFrame() }

    func targetFrame() -> CGRect? {
        driver.isVisible(element) ? driver.frame(element) : nil
    }

    func press(_ direction: Direction) -> Bool {
        do {
            try driver.remote(direction.remoteButton, repeat: 1, interval: 0.1)
            return true
        } catch {
            return false
        }
    }

    func isTargetFocused() -> Bool { driver.hasFocus(element) }
}
