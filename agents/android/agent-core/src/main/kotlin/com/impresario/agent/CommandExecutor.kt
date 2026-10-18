package com.impresario.agent

import impresario.v1.Impresario.ActionType
import impresario.v1.Impresario.CheckKind
import impresario.v1.Impresario.Command
import impresario.v1.Impresario.CommandResult
import impresario.v1.Impresario.Direction as PbDirection
import impresario.v1.Impresario.ErrorCode
import impresario.v1.Impresario.SelectorStrategy
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.delay

/**
 * Executes one command against the [UiDriver] (spec 11.4). Pure of any gRPC
 * or Android type, so every behavior is JVM-testable with a fake driver.
 *
 * Cancellation: the session runner cancels the coroutine; `delay` points make
 * the cancellation take effect within at most one poll interval (spec 11.2).
 * Driver calls themselves are blocking and not interruptible on the real
 * device — that is the documented latency bound.
 */
class CommandExecutor(
    private val driver: UiDriver,
    private val options: ExecutorOptions,
    private val now: () -> Long = System::currentTimeMillis,
) {
    private val extras: DriverExtras? = driver as? DriverExtras

    /** Executes a command; throws [CancellationException] only when cancelled. */
    suspend fun execute(command: Command): CommandResult {
        val started = now()
        val outcome = try {
            runBody(command)
        } catch (cancellation: CancellationException) {
            throw cancellation
        } catch (agent: AgentException) {
            Outcome.Failed(agent)
        } catch (error: Exception) {
            Outcome.Failed(
                AgentException.internal(error.message ?: error.javaClass.simpleName),
            )
        }

        val finalOutcome = if (outcome is Outcome.Ok && crashed()) {
            Outcome.Failed(AgentException.appCrashed("the app under test crashed during the command"))
        } else {
            outcome
        }

        return toResult(command.commandId, finalOutcome, now() - started)
    }

    private fun crashed(): Boolean =
        options.appId.isNotEmpty() && extras?.targetCrashed(options.appId) == true

    // ------------------------------------------------------------------
    // bodies
    // ------------------------------------------------------------------

    private suspend fun runBody(command: Command): Outcome {
        val timeout = command.timeoutMs.toLong().coerceAtLeast(1)
        return when (command.bodyCase) {
            Command.BodyCase.RUN_ACTION -> runAction(command.runAction, timeout)
            Command.BodyCase.VALIDATE_CHECK -> validateCheck(command.validateCheck, timeout)
            Command.BodyCase.KEY_PRESS -> keyPress(command.keyPress)
            Command.BodyCase.SCREENSHOT -> screenshot()
            Command.BodyCase.APP_CONTROL -> appControl(command.appControl)
            Command.BodyCase.CANCEL ->
                throw AgentException.protocol("cancel commands are handled by the session runner")
            Command.BodyCase.BODY_NOT_SET ->
                throw AgentException.unsupportedAction("the command has no body")
        }
    }

    private suspend fun runAction(action: impresario.v1.Impresario.RunAction, timeout: Long): Outcome {
        val type = action.type
        if (type == ActionType.ACTION_TYPE_UNSPECIFIED) {
            throw AgentException.protocol("the action type is unspecified")
        }
        return when (type) {
            ActionType.CLICK -> click(requireSelector(action.hasSelector(), action.selector), timeout)
            ActionType.TYPE -> enterText(EnterMode.REPLACE, action, timeout)
            ActionType.APPEND -> enterText(EnterMode.APPEND, action, timeout)
            ActionType.CLEAR -> enterText(EnterMode.CLEAR, action, timeout)
            ActionType.LONG_PRESS -> longPress(action, timeout)
            ActionType.FOCUS -> focus(action, timeout)
            ActionType.SELECT -> select(action, timeout)
            ActionType.SCROLL -> scroll(action, timeout)
            ActionType.SCROLL_TO -> scrollTo(action, timeout)
            ActionType.SWIPE -> swipe(action, timeout)
            ActionType.BACK -> {
                driver.pressBack()
                Outcome.Ok()
            }
            else -> throw AgentException.protocol("unknown action type '${type.name}'")
        }
    }

    // ------------------------------------------------------------------
    // actions
    // ------------------------------------------------------------------

    private suspend fun click(spec: SelectorSpec, timeout: Long): Outcome {
        val element = waitInteractable(spec, timeout)
        performClick(spec, element)
        return Outcome.Ok()
    }

    private suspend fun performClick(spec: SelectorSpec, element: ElementHandle) {
        if (options.isTv) {
            try {
                ensureFocused(spec, element)
                driver.pressKey(KeyTable.KEYCODE_DPAD_CENTER, repeat = 1, intervalMs = 100)
            } catch (unreachable: AgentException) {
                if (unreachable.code != ErrorCode.FOCUS_UNREACHABLE) throw unreachable
                // Classic AdapterViews (e.g. `AlertDialog.setItems` lists) keep
                // the D-pad focus on the list itself, so rows never report
                // `focused` and the focus navigator cannot land on them. The
                // accessibility click action still activates the row
                // (docs/DECISIONS.md I-64).
                val fresh = pick(spec, awaitMatches(spec, 0) { pick(spec, it) != null })
                    ?: throw unreachable
                driver.click(fresh)
            }
        } else {
            driver.click(element)
        }
    }

    private enum class EnterMode { REPLACE, APPEND, CLEAR }

    @Suppress("LongMethod")
    private suspend fun enterText(
        mode: EnterMode,
        action: impresario.v1.Impresario.RunAction,
        timeout: Long,
    ): Outcome {
        val spec = requireSelector(action.hasSelector(), action.selector)
        val element = waitInteractable(spec, timeout)

        // Read the field *before* touching it: on TVs the activation OK can
        // reach an already-open IME keyboard and insert a stray keystroke
        // (observed: type 'abc', then append 'def' => 'abc1def', I-71). Every
        // mode ends in a whole-field `setText(next)`, so a pre-activation read
        // is the stable definition of "current content".
        val currentText = driver.text(element)

        // Focus: click on phones; D-pad focus then OK on TVs (enters edit mode).
        if (options.isTv) {
            ensureFocused(spec, element)
            driver.pressKey(KeyTable.KEYCODE_DPAD_CENTER, repeat = 1, intervalMs = 100)
        } else {
            driver.click(element)
        }

        val next = when (mode) {
            EnterMode.REPLACE -> action.text
            EnterMode.APPEND -> currentText + action.text
            EnterMode.CLEAR -> ""
        }

        driver.setText(element, next)
        val readBack = driver.text(element)
        if (readBack == next) {
            return Outcome.Ok(observedText = readBack)
        }

        // Fallback (spec 11.4): clear + `input text` (ASCII only). `clear()`
        // deletes all content, which is the select-all + delete step.
        if (mode == EnterMode.CLEAR) {
            driver.clear(element)
            val cleared = driver.text(element)
            if (cleared.isEmpty()) return Outcome.Ok(observedText = "")
            throw AgentException.notInteractable(
                "clear left '${cleared}' in the field",
                mapOf("selector" to spec.describe(), "observed" to cleared),
            )
        }
        if (!isAsciiPrintable(next)) {
            throw AgentException.unsupportedAction(
                "text entry fell back to `input text`, which cannot send non-ASCII content",
            )
        }
        val fallbackWorked = extras?.let { driverExtras ->
            driver.clear(element)
            val sent = driverExtras.inputText(next)
            sent && driver.text(element) == next
        } ?: false
        if (fallbackWorked) {
            return Outcome.Ok(observedText = next)
        }
        throw AgentException.notInteractable(
            "text entry did not take effect (expected '${next}', observed '${driver.text(element)}')",
            mapOf(
                "selector" to spec.describe(),
                "expected" to next,
                "observed" to driver.text(element),
            ),
        )
    }

    private suspend fun longPress(
        action: impresario.v1.Impresario.RunAction,
        timeout: Long,
    ): Outcome {
        val spec = requireSelector(action.hasSelector(), action.selector)
        val element = waitInteractable(spec, timeout)
        val duration = if (action.durationMs == 0) 1000L else action.durationMs.toLong()
        if (options.isTv) {
            ensureFocused(spec, element)
            val pressed = extras?.longPressKey(KeyTable.KEYCODE_DPAD_CENTER, duration) ?: false
            if (!pressed) {
                throw AgentException.unsupportedAction(
                    "long-press on android tv needs the key-injection fallback",
                )
            }
        } else {
            driver.longClick(element, duration)
        }
        return Outcome.Ok()
    }

    private suspend fun focus(
        action: impresario.v1.Impresario.RunAction,
        timeout: Long,
    ): Outcome {
        if (!options.isTv) {
            throw AgentException.unsupportedAction(
                "focus is not supported on android phones (out-of-process automation cannot move focus)",
            )
        }
        val spec = requireSelector(action.hasSelector(), action.selector)
        val matches = awaitMatches(spec, timeout) { pick(spec, it) != null }
        val element = pick(spec, matches)
            ?: throw AgentException.elementNotFound(
                "the element ${spec.describe()} was not found",
                spec.describe(),
            )
        ensureFocused(spec, element)
        return Outcome.Ok()
    }

    private suspend fun select(
        action: impresario.v1.Impresario.RunAction,
        timeout: Long,
    ): Outcome {
        val spec = requireSelector(action.hasSelector(), action.selector)
        val trigger = waitInteractable(spec, timeout)
        performClick(spec, trigger)

        val optionText = action.text
        val optionSpec = SelectorSpec(SelectorStrategy.TEXT, optionText, 0)
        val matches = awaitMatches(optionSpec, timeout) { it.isNotEmpty() }
        val option = matches.firstOrNull()
            ?: throw AgentException.elementNotFound(
                "the option '${optionText}' was not found after opening ${spec.describe()}",
                spec.describe(),
            ).let {
                AgentException(
                    it.code,
                    it.message.orEmpty(),
                    it.details + mapOf("option" to optionText),
                )
            }
        performClick(optionSpec, option)
        return Outcome.Ok(observedText = optionText)
    }

    private suspend fun scroll(
        action: impresario.v1.Impresario.RunAction,
        timeout: Long,
    ): Outcome {
        val direction = direction(action.direction)
        val steps = action.count.coerceAtLeast(1)
        val containerSpec = if (action.hasSelector()) SelectorSpec.from(action.selector) else null

        if (options.isTv) {
            if (containerSpec != null) {
                val container = waitExisting(containerSpec, timeout)
                ensureFocused(containerSpec, container)
            }
            repeat(steps) {
                driver.pressKey(direction.keyCode, repeat = 1, intervalMs = 100)
            }
            return Outcome.Ok()
        }

        val container = if (containerSpec != null) {
            waitExisting(containerSpec, timeout)
        } else {
            null
        }
        driver.scroll(container, direction, steps)
        return Outcome.Ok()
    }

    private suspend fun scrollTo(
        action: impresario.v1.Impresario.RunAction,
        timeout: Long,
    ): Outcome {
        val spec = requireSelector(action.hasSelector(), action.selector)
        val deadline = now() + timeout
        var attempts = 0
        var scrollingUp = false
        while (now() < deadline && attempts < options.scrollMaxAttempts) {
            val element = pick(spec, driver.find(lookup(spec), 0))
            if (element != null && driver.isVisible(element)) {
                return Outcome.Ok()
            }
            val direction = if (scrollingUp) Direction.UP else Direction.DOWN
            if (options.isTv) {
                driver.pressKey(direction.keyCode, repeat = 1, intervalMs = 100)
            } else {
                driver.scroll(null, direction, 1)
            }
            attempts += 1
            if (!scrollingUp && attempts >= options.scrollMaxAttempts / 2) {
                // Down phase exhausted: try upwards for the rest of the budget.
                scrollingUp = true
            }
        }
        throw AgentException(
            impresario.v1.Impresario.ErrorCode.ELEMENT_NOT_FOUND,
            "the element ${spec.describe()} was not reached after $attempts scroll steps",
            mapOf("selector" to spec.describe(), "scroll_attempts" to attempts.toString()),
        )
    }

    private suspend fun swipe(
        action: impresario.v1.Impresario.RunAction,
        timeout: Long,
    ): Outcome {
        if (options.isTv) {
            throw AgentException.unsupportedAction("swipe is not supported on android tv (no touch)")
        }
        val direction = direction(action.direction)
        val element = if (action.hasSelector()) {
            val spec = SelectorSpec.from(action.selector)
            waitExisting(spec, timeout)
        } else {
            null
        }
        driver.swipe(element, direction)
        return Outcome.Ok()
    }

    // ------------------------------------------------------------------
    // checks
    // ------------------------------------------------------------------

    private suspend fun validateCheck(
        check: impresario.v1.Impresario.ValidateCheck,
        timeout: Long,
    ): Outcome {
        val kind = check.kind
        if (kind == CheckKind.CHECK_KIND_UNSPECIFIED) {
            throw AgentException.protocol("the check kind is unspecified")
        }
        val spec = SelectorSpec.from(check.selector)
        val expected = when (kind) {
            CheckKind.TEXT_EQUALS, CheckKind.TEXT_CONTAINS_CHECK -> check.expectedText
            CheckKind.COUNT -> check.expectedCount.toString()
            CheckKind.VISIBLE -> "visible"
            CheckKind.HIDDEN -> "hidden"
            CheckKind.ENABLED -> "enabled"
            CheckKind.DISABLED -> "disabled"
            CheckKind.FOCUSED -> "focused"
            else -> ""
        }
        val deadline = now() + timeout
        while (true) {
            val matches = driver.find(lookup(spec), 0)
            val element = pick(spec, matches)
            val evaluation = evaluateCheck(kind, check, spec, element, matches.size)
            if (evaluation.satisfied) {
                return Outcome.Ok(
                    observedText = evaluation.observed,
                    observedCount = evaluation.matchedCount,
                )
            }
            if (now() >= deadline) {
                throw AgentException.checkFailed(
                    "check failed for ${spec.describe()} (expected ${expected}, observed ${evaluation.observed})",
                    expected,
                    evaluation.observed,
                    evaluation.matchedCount,
                    spec.describe(),
                )
            }
            delay(options.pollIntervalMs)
        }
    }

    private data class CheckEvaluation(val satisfied: Boolean, val observed: String, val matchedCount: Int)

    private fun evaluateCheck(
        kind: CheckKind,
        check: impresario.v1.Impresario.ValidateCheck,
        spec: SelectorSpec,
        element: ElementHandle?,
        matchedCount: Int,
    ): CheckEvaluation = when (kind) {
        CheckKind.VISIBLE, CheckKind.HIDDEN -> {
            val visible = element != null && driver.isVisible(element)
            val observed = when {
                element == null -> "missing"
                visible -> "visible"
                else -> "hidden"
            }
            CheckEvaluation(if (kind == CheckKind.VISIBLE) visible else !visible, observed, matchedCount)
        }
        CheckKind.ENABLED, CheckKind.DISABLED -> {
            val enabled = element != null && driver.isEnabled(element)
            val observed = when {
                element == null -> "missing"
                enabled -> "enabled"
                else -> "disabled"
            }
            CheckEvaluation(if (kind == CheckKind.ENABLED) enabled else !enabled, observed, matchedCount)
        }
        CheckKind.FOCUSED -> {
            val focused = element != null && driver.isFocused(element)
            val observed = when {
                element == null -> "missing"
                focused -> "focused"
                else -> "not focused"
            }
            CheckEvaluation(focused, observed, matchedCount)
        }
        CheckKind.TEXT_EQUALS -> {
            val text = element?.let { driver.text(it) }
            CheckEvaluation(
                element != null && text == check.expectedText,
                text ?: "missing",
                matchedCount,
            )
        }
        CheckKind.TEXT_CONTAINS_CHECK -> {
            val text = element?.let { driver.text(it) }
            CheckEvaluation(
                element != null && text != null && text.contains(check.expectedText),
                text ?: "missing",
                matchedCount,
            )
        }
        CheckKind.COUNT -> CheckEvaluation(
            matchedCount == check.expectedCount,
            matchedCount.toString(),
            matchedCount,
        )
        else -> throw AgentException.protocol("unsupported check kind ${kind.name}")
    }.also { /* spec: the selector is only informational here */ }

    // ------------------------------------------------------------------
    // keys, screenshots, app control
    // ------------------------------------------------------------------

    private fun keyPress(keyPress: impresario.v1.Impresario.KeyPress): Outcome {
        val code = KeyTable.resolve(keyPress.key, options.isTv)
        val repeat = keyPress.repeat.coerceAtLeast(1)
        val interval = if (keyPress.intervalMs == 0) 100L else keyPress.intervalMs.toLong()
        driver.pressKey(code, repeat, interval)
        return Outcome.Ok()
    }

    private fun screenshot(): Outcome {
        val png = driver.screenshotPng()
        if (png.isEmpty()) {
            throw AgentException.internal("the driver returned an empty screenshot")
        }
        return Outcome.Ok(png = png)
    }

    private fun appControl(control: impresario.v1.Impresario.AppControl): Outcome {
        return when (control.op) {
            impresario.v1.Impresario.AppOp.LAUNCH -> {
                driver.launch(options.appId, options.isTv)
                Outcome.Ok()
            }
            impresario.v1.Impresario.AppOp.TERMINATE -> {
                driver.terminate(options.appId)
                Outcome.Ok()
            }
            impresario.v1.Impresario.AppOp.RESTART -> {
                driver.terminate(options.appId)
                if (control.clearData || options.clearAppDataOnRestart) {
                    driver.clearData(options.appId)
                }
                driver.launch(options.appId, options.isTv)
                Outcome.Ok()
            }
            impresario.v1.Impresario.AppOp.BACKGROUND -> {
                val backgrounded = extras?.putInBackground(options.appId) == true
                if (!backgrounded) {
                    throw AgentException.unsupportedAction(
                        "putting the app under test in the background is not supported by this driver",
                    )
                }
                Outcome.Ok()
            }
            else -> throw AgentException.protocol("the app op is unspecified")
        }
    }

    // ------------------------------------------------------------------
    // helpers
    // ------------------------------------------------------------------

    private fun requireSelector(hasSelector: Boolean, selector: impresario.v1.Impresario.Selector?): SelectorSpec {
        if (!hasSelector) {
            throw AgentException.protocol("the action requires a selector")
        }
        return SelectorSpec.from(selector)
    }

    private fun direction(direction: PbDirection): Direction {
        if (direction == PbDirection.DIRECTION_UNSPECIFIED) {
            throw AgentException.protocol("the direction is unspecified")
        }
        return when (direction) {
            PbDirection.UP -> Direction.UP
            PbDirection.DOWN -> Direction.DOWN
            PbDirection.LEFT -> Direction.LEFT
            PbDirection.RIGHT -> Direction.RIGHT
            else -> throw AgentException.protocol("unsupported direction ${direction.name}")
        }
    }

    private fun pick(spec: SelectorSpec, matches: List<ElementHandle>): ElementHandle? =
        matches.getOrNull(spec.index)

    /**
     * The driver always sees the raw selector: `index` selects among matches in
     * the dispatcher (spec 11.3), so drivers return *all* matches.
     */
    private fun lookup(spec: SelectorSpec): SelectorSpec = spec.copy(index = 0)

    /** Waits until the indexed element exists, is visible and is enabled. */
    private suspend fun waitInteractable(spec: SelectorSpec, timeout: Long): ElementHandle {
        val matches = awaitMatches(spec, timeout) { list ->
            val element = pick(spec, list) ?: return@awaitMatches false
            driver.isVisible(element) && driver.isEnabled(element)
        }
        val element = pick(spec, matches)
            ?: throw AgentException.elementNotFound(
                "the element ${spec.describe()} was not found",
                spec.describe(),
            )
        val visible = driver.isVisible(element)
        val enabled = driver.isEnabled(element)
        if (!visible || !enabled) {
            val reason = if (!visible) "not visible" else "disabled"
            throw AgentException.notInteractable(
                "the element ${spec.describe()} is $reason",
                mapOf("selector" to spec.describe(), "reason" to reason),
            )
        }
        return element
    }

    /** Waits until the indexed element exists (used for containers and swipes). */
    private suspend fun waitExisting(spec: SelectorSpec, timeout: Long): ElementHandle {
        val matches = awaitMatches(spec, timeout) { pick(spec, it) != null }
        return pick(spec, matches)
            ?: throw AgentException.elementNotFound(
                "the element ${spec.describe()} was not found",
                spec.describe(),
            )
    }

    private suspend fun ensureFocused(spec: SelectorSpec, element: ElementHandle) {
        val probe = object : FocusProbe {
            override fun currentFocusLabel(): String? = null
            override fun currentFocusFrame(): Rect? = driver.focusedBounds()
            override fun targetFrame(): Rect? =
                if (driver.isVisible(element)) driver.bounds(element) else null

            override fun press(direction: Direction) {
                driver.pressKey(direction.keyCode, repeat = 1, intervalMs = 100)
            }

            override fun isTargetFocused(): Boolean = driver.isFocused(element)
        }
        when (val result = FocusNavigator(probe, options.focusMaxSteps).ensureTargetFocused()) {
            is FocusResult.Focused -> Unit
            is FocusResult.Unreachable -> throw AgentException.focusUnreachable(
                "could not reach ${spec.describe()} by D-pad after ${result.steps} steps (${result.reason})",
                mapOf(
                    "selector" to spec.describe(),
                    "reason" to result.reason,
                    "steps" to result.steps.toString(),
                    "focus_trail" to FocusNavigator.trailJson(result.trail),
                ),
            )
        }
    }

    private suspend fun awaitMatches(
        spec: SelectorSpec,
        timeoutMs: Long,
        predicate: (List<ElementHandle>) -> Boolean,
    ): List<ElementHandle> {
        val deadline = now() + timeoutMs.coerceAtLeast(1)
        while (true) {
            val matches = driver.find(lookup(spec), 0)
            if (predicate(matches)) return matches
            if (now() >= deadline) return matches
            delay(options.pollIntervalMs)
        }
    }

    private fun toResult(id: String, outcome: Outcome, durationMs: Long): CommandResult {
        val builder = CommandResult.newBuilder().setCommandId(id).setDurationMs(durationMs.coerceIn(0, Int.MAX_VALUE.toLong()).toInt())
        return when (outcome) {
            is Outcome.Ok -> {
                builder.setOk(true)
                if (outcome.observedText.isNotEmpty()) builder.setObservedText(outcome.observedText)
                if (outcome.observedCount > 0) builder.setObservedCount(outcome.observedCount)
                outcome.png?.let { builder.setScreenshotPng(com.google.protobuf.ByteString.copyFrom(it)) }
                builder.build()
            }
            is Outcome.Failed -> builder.setOk(false).setError(outcome.error.toProto()).build()
        }
    }

    private sealed class Outcome {
        data class Ok(
            val observedText: String = "",
            val observedCount: Int = 0,
            val png: ByteArray? = null,
        ) : Outcome() {
            override fun equals(other: Any?): Boolean = this === other
            override fun hashCode(): Int = System.identityHashCode(this)
        }

        data class Failed(val error: AgentException) : Outcome()
    }

    private companion object {
        fun isAsciiPrintable(text: String): Boolean = text.all { it.code in 32..126 }
    }
}
