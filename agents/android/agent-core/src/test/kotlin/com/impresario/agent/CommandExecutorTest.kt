package com.impresario.agent

import com.google.protobuf.ByteString
import impresario.v1.Impresario.ActionType
import impresario.v1.Impresario.AppControl
import impresario.v1.Impresario.AppOp
import impresario.v1.Impresario.CheckKind
import impresario.v1.Impresario.Command
import impresario.v1.Impresario.Direction
import impresario.v1.Impresario.ErrorCode
import impresario.v1.Impresario.KeyPress
import impresario.v1.Impresario.Platform
import impresario.v1.Impresario.RemoteKey
import impresario.v1.Impresario.RunAction
import impresario.v1.Impresario.Selector
import impresario.v1.Impresario.SelectorStrategy
import impresario.v1.Impresario.ValidateCheck
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class CommandExecutorTest {
    private val phoneOptions = ExecutorOptions(
        platform = Platform.ANDROID,
        appId = "com.example.app",
        pollIntervalMs = 5,
        scrollMaxAttempts = 4,
        focusMaxSteps = 10,
    )
    private val tvOptions = phoneOptions.copy(platform = Platform.ANDROID_TV)

    private fun selector(strategy: SelectorStrategy, value: String, index: Int = 0): Selector =
        Selector.newBuilder().setStrategy(strategy).setValue(value).setIndex(index).build()

    private val okSelector: Selector get() = selector(SelectorStrategy.TEXT, "OK")

    private fun action(
        type: ActionType,
        selector: Selector? = null,
        text: String = "",
        direction: Direction = Direction.DIRECTION_UNSPECIFIED,
        count: Int = 0,
        durationMs: Int = 0,
    ): RunAction {
        val builder = RunAction.newBuilder().setType(type)
        selector?.let { builder.setSelector(it) }
        if (text.isNotEmpty()) builder.setText(text)
        if (direction != Direction.DIRECTION_UNSPECIFIED) builder.setDirection(direction)
        if (count != 0) builder.setCount(count)
        if (durationMs != 0) builder.setDurationMs(durationMs)
        return builder.build()
    }

    private fun actionCommand(id: String, action: RunAction, timeoutMs: Int = 30): Command =
        Command.newBuilder().setCommandId(id).setTimeoutMs(timeoutMs).setRunAction(action).build()

    // ------------------------------------------------------------------
    // click
    // ------------------------------------------------------------------

    @Test
    fun phoneClickTapsTheElement() = runBlocking {
        val driver = FakeDriver()
        driver.addNode("btn", text = "OK", selector = "text:OK")
        val result = CommandExecutor(driver, phoneOptions).execute(
            actionCommand("c1", action(ActionType.CLICK, okSelector)),
        )
        assertTrue(result.ok)
        assertTrue(driver.calls.contains("click:btn"))
    }

    @Test
    fun clickOnAMissingElementIsElementNotFound() = runBlocking {
        val driver = FakeDriver()
        val result = CommandExecutor(driver, phoneOptions).execute(
            actionCommand("c1", action(ActionType.CLICK, okSelector)),
        )
        assertFalse(result.ok)
        assertEquals(ErrorCode.ELEMENT_NOT_FOUND, result.error.code)
        assertEquals("text:OK", result.error.detailsMap["selector"])
    }

    @Test
    fun clickOnADisabledElementIsNotInteractable() = runBlocking {
        val driver = FakeDriver()
        driver.addNode("btn", enabled = false, selector = "text:OK")
        val result = CommandExecutor(driver, phoneOptions).execute(
            actionCommand("c1", action(ActionType.CLICK, okSelector)),
        )
        assertFalse(result.ok)
        assertEquals(ErrorCode.NOT_INTERACTABLE, result.error.code)
        assertEquals("disabled", result.error.detailsMap["reason"])
    }

    @Test
    fun clickOnAnInvisibleElementIsNotInteractable() = runBlocking {
        val driver = FakeDriver()
        driver.addNode("btn", visible = false, selector = "text:OK")
        val result = CommandExecutor(driver, phoneOptions).execute(
            actionCommand("c1", action(ActionType.CLICK, okSelector)),
        )
        assertFalse(result.ok)
        assertEquals(ErrorCode.NOT_INTERACTABLE, result.error.code)
        assertEquals("not visible", result.error.detailsMap["reason"])
    }

    @Test
    fun elementIndexSelectsAmongMatches() = runBlocking {
        val driver = FakeDriver()
        driver.addNode("first", selector = "text:Item")
        driver.addNode("second", selector = "text:Item")
        val result = CommandExecutor(driver, phoneOptions).execute(
            actionCommand("c1", action(ActionType.CLICK, selector(SelectorStrategy.TEXT, "Item", index = 1))),
        )
        assertTrue(result.ok)
        assertTrue(driver.calls.contains("click:second"))
    }

    @Test
    fun tvClickMovesTheFocusThenPressesOk() = runBlocking {
        val driver = FakeDriver()
        val target = driver.addNode("btn", bounds = Rect(200, 0, 300, 100), selector = "text:OK")
        driver.addNode("current", focused = true, bounds = Rect(0, 0, 100, 100))
        driver.onPressKey = { code -> if (code == KeyTable.KEYCODE_DPAD_RIGHT) target.focused = true }
        val result = CommandExecutor(driver, tvOptions).execute(
            actionCommand("c1", action(ActionType.CLICK, okSelector)),
        )
        assertTrue(result.ok)
        assertTrue(driver.calls.contains("pressKey:22:1:100"))
        assertTrue(driver.calls.contains("pressKey:23:1:100"))
        assertFalse(driver.calls.any { it.startsWith("click:") })
    }

    @Test
    fun tvClickFallsBackToTheAccessibilityClickWhenFocusIsUnreachable() = runBlocking {
        val driver = FakeDriver()
        driver.addNode("btn", bounds = Rect(200, 0, 300, 100), selector = "text:OK")
        driver.addNode("current", focused = true, bounds = Rect(0, 0, 100, 100))
        val result = CommandExecutor(driver, tvOptions).execute(
            actionCommand("c1", action(ActionType.CLICK, okSelector)),
        )
        // The target never reports focused (classic AdapterViews keep the D-pad
        // focus on the list itself), so the executor must not give up: it
        // activates the row through the accessibility click action instead
        // (docs/DECISIONS.md I-64) — and never presses OK, which would hit
        // whatever the list still has selected.
        assertTrue(result.ok)
        assertTrue(driver.calls.contains("click:btn"))
        assertFalse(driver.calls.contains("pressKey:23:1:100"))
    }

    @Test
    fun tvClickReportsFocusUnreachableWhenTheRowVanishes() = runBlocking {
        val driver = FakeDriver()
        driver.addNode("btn", bounds = Rect(200, 0, 300, 100), selector = "text:OK")
        driver.addNode("current", focused = true, bounds = Rect(0, 0, 100, 100))
        // The screen changes while the navigator works: nothing matches anymore,
        // so the accessibility fallback has nothing to click.
        driver.onPressKey = { driver.selectorMatches["text:OK"]?.clear() }
        val result = CommandExecutor(driver, tvOptions).execute(
            actionCommand("c1", action(ActionType.CLICK, okSelector)),
        )
        assertFalse(result.ok)
        assertEquals(ErrorCode.FOCUS_UNREACHABLE, result.error.code)
        assertEquals("blocked", result.error.detailsMap["reason"])
        assertTrue(result.error.detailsMap.containsKey("focus_trail"))
    }

    // ------------------------------------------------------------------
    // text entry
    // ------------------------------------------------------------------

    private fun fieldSelector(): Selector = selector(SelectorStrategy.ANDROID_ID, "field")

    @Test
    fun typeReplacesContent() = runBlocking {
        val driver = FakeDriver()
        driver.addNode("field", text = "seed", selector = "androidId:field")
        val result = CommandExecutor(driver, phoneOptions).execute(
            actionCommand("c1", action(ActionType.TYPE, fieldSelector(), text = "hello")),
        )
        assertTrue(result.ok)
        assertEquals("hello", driver.node("field").text)
        assertEquals("hello", result.observedText)
        assertTrue(driver.calls.contains("setText:field:hello"))
    }

    @Test
    fun appendKeepsContent() = runBlocking {
        val driver = FakeDriver()
        driver.addNode("field", text = "ab", selector = "androidId:field")
        val result = CommandExecutor(driver, phoneOptions).execute(
            actionCommand("c1", action(ActionType.APPEND, fieldSelector(), text = "cd")),
        )
        assertTrue(result.ok)
        assertEquals("abcd", driver.node("field").text)
    }

    @Test
    fun clearLeavesAnEmptyField() = runBlocking {
        val driver = FakeDriver()
        driver.addNode("field", text = "ab", selector = "androidId:field")
        val result = CommandExecutor(driver, phoneOptions).execute(
            actionCommand("c1", action(ActionType.CLEAR, fieldSelector())),
        )
        assertTrue(result.ok)
        assertEquals("", driver.node("field").text)
    }

    @Test
    fun setTextMismatchFallsBackToShellInput() = runBlocking {
        val driver = FakeDriver()
        driver.addNode("field", text = "seed", selector = "androidId:field")
        driver.setTextIgnores = true
        val result = CommandExecutor(driver, phoneOptions).execute(
            actionCommand("c1", action(ActionType.TYPE, fieldSelector(), text = "hello")),
        )
        assertTrue(result.ok)
        assertEquals("hello", driver.node("field").text)
        assertTrue(driver.calls.contains("inputText:hello"))
        assertTrue(driver.calls.contains("clear:field"))
    }

    @Test
    fun fallbackRejectsNonAsciiText() = runBlocking {
        val driver = FakeDriver()
        driver.addNode("field", text = "seed", selector = "androidId:field")
        driver.setTextIgnores = true
        val result = CommandExecutor(driver, phoneOptions).execute(
            actionCommand("c1", action(ActionType.TYPE, fieldSelector(), text = "h\u00e9llo")),
        )
        assertFalse(result.ok)
        assertEquals(ErrorCode.UNSUPPORTED_ACTION, result.error.code)
    }

    @Test
    fun clearMismatchIsNotInteractable() = runBlocking {
        val driver = FakeDriver()
        driver.addNode("field", text = "ab", selector = "androidId:field")
        driver.setTextIgnores = true
        driver.clearWorks = false
        val result = CommandExecutor(driver, phoneOptions).execute(
            actionCommand("c1", action(ActionType.CLEAR, fieldSelector())),
        )
        assertFalse(result.ok)
        assertEquals(ErrorCode.NOT_INTERACTABLE, result.error.code)
    }

    @Test
    fun tvTypeFocusesTheFieldAndEntersEditMode() = runBlocking {
        val driver = FakeDriver()
        val field = driver.addNode("field", text = "seed", bounds = Rect(200, 0, 300, 100), selector = "androidId:field")
        driver.addNode("current", focused = true, bounds = Rect(0, 0, 100, 100))
        driver.onPressKey = { code -> if (code == KeyTable.KEYCODE_DPAD_RIGHT) field.focused = true }
        val result = CommandExecutor(driver, tvOptions).execute(
            actionCommand("c1", action(ActionType.TYPE, fieldSelector(), text = "hello")),
        )
        assertTrue(result.ok)
        assertEquals("hello", driver.node("field").text)
        assertTrue(driver.calls.contains("pressKey:23:1:100"))
    }

    // ------------------------------------------------------------------
    // long press, focus, select
    // ------------------------------------------------------------------

    @Test
    fun longPressUsesTheGivenDuration() = runBlocking {
        val driver = FakeDriver()
        driver.addNode("btn", selector = "text:OK")
        val result = CommandExecutor(driver, phoneOptions).execute(
            actionCommand("c1", action(ActionType.LONG_PRESS, okSelector, durationMs = 1500)),
        )
        assertTrue(result.ok)
        assertTrue(driver.calls.contains("longClick:btn:1500"))
    }

    @Test
    fun longPressDefaultsToOneSecond() = runBlocking {
        val driver = FakeDriver()
        driver.addNode("btn", selector = "text:OK")
        val result = CommandExecutor(driver, phoneOptions).execute(
            actionCommand("c1", action(ActionType.LONG_PRESS, okSelector)),
        )
        assertTrue(result.ok)
        assertTrue(driver.calls.contains("longClick:btn:1000"))
    }

    @Test
    fun focusIsUnsupportedOnPhones() = runBlocking {
        val driver = FakeDriver()
        driver.addNode("btn", selector = "text:OK")
        val result = CommandExecutor(driver, phoneOptions).execute(
            actionCommand("c1", action(ActionType.FOCUS, okSelector)),
        )
        assertFalse(result.ok)
        assertEquals(ErrorCode.UNSUPPORTED_ACTION, result.error.code)
    }

    @Test
    fun tvFocusMovesTheDpadWithoutPressingOk() = runBlocking {
        val driver = FakeDriver()
        val target = driver.addNode("btn", bounds = Rect(200, 0, 300, 100), selector = "text:OK")
        driver.addNode("current", focused = true, bounds = Rect(0, 0, 100, 100))
        driver.onPressKey = { code -> if (code == KeyTable.KEYCODE_DPAD_RIGHT) target.focused = true }
        val result = CommandExecutor(driver, tvOptions).execute(
            actionCommand("c1", action(ActionType.FOCUS, okSelector)),
        )
        assertTrue(result.ok)
        assertTrue(driver.calls.contains("pressKey:22:1:100"))
        assertFalse(driver.calls.contains("pressKey:23:1:100"))
    }

    @Test
    fun selectClicksTheTriggerThenTheOption() = runBlocking {
        val driver = FakeDriver()
        driver.addNode("spinner", selector = "text:Pick")
        driver.addNode("option", selector = "text:Blue")
        val result = CommandExecutor(driver, phoneOptions).execute(
            actionCommand("c1", action(ActionType.SELECT, selector(SelectorStrategy.TEXT, "Pick"), text = "Blue")),
        )
        assertTrue(result.ok)
        assertEquals(listOf("click:spinner", "click:option"), driver.calls.filterNot { it.startsWith("find:") })
    }

    @Test
    fun selectMissingOptionIsElementNotFoundWithOptionDetail() = runBlocking {
        val driver = FakeDriver()
        driver.addNode("spinner", selector = "text:Pick")
        val result = CommandExecutor(driver, phoneOptions).execute(
            actionCommand("c1", action(ActionType.SELECT, selector(SelectorStrategy.TEXT, "Pick"), text = "Nope")),
        )
        assertFalse(result.ok)
        assertEquals(ErrorCode.ELEMENT_NOT_FOUND, result.error.code)
        assertEquals("Nope", result.error.detailsMap["option"])
    }

    // ------------------------------------------------------------------
    // scroll and swipe
    // ------------------------------------------------------------------

    @Test
    fun phoneScrollUsesTheDriver() = runBlocking {
        val driver = FakeDriver()
        val result = CommandExecutor(driver, phoneOptions).execute(
            actionCommand("c1", action(ActionType.SCROLL, direction = Direction.DOWN, count = 3)),
        )
        assertTrue(result.ok)
        assertEquals(listOf("scroll:-:DOWN:3"), driver.calls.toList())
    }

    @Test
    fun tvScrollPressesTheDpadPerStep() = runBlocking {
        val driver = FakeDriver()
        val result = CommandExecutor(driver, tvOptions).execute(
            actionCommand("c1", action(ActionType.SCROLL, direction = Direction.DOWN, count = 2)),
        )
        assertTrue(result.ok)
        assertEquals(listOf("pressKey:20:1:100", "pressKey:20:1:100"), driver.calls.toList())
    }

    @Test
    fun scrollToSucceedsWhenTheElementIsAlreadyVisible() = runBlocking {
        val driver = FakeDriver()
        driver.addNode("row", selector = "text:Row")
        val result = CommandExecutor(driver, phoneOptions).execute(
            actionCommand("c1", action(ActionType.SCROLL_TO, selector(SelectorStrategy.TEXT, "Row")), timeoutMs = 1000),
        )
        assertTrue(result.ok)
        assertTrue(driver.calls.none { it.startsWith("scroll:") })
    }

    @Test
    fun scrollToExhaustsTheBudgetAndTriesUpwards() = runBlocking {
        val driver = FakeDriver()
        val result = CommandExecutor(driver, phoneOptions).execute(
            actionCommand("c1", action(ActionType.SCROLL_TO, selector(SelectorStrategy.TEXT, "Ghost")), timeoutMs = 1000),
        )
        assertFalse(result.ok)
        assertEquals(ErrorCode.ELEMENT_NOT_FOUND, result.error.code)
        assertEquals("4", result.error.detailsMap["scroll_attempts"])
        assertEquals(
            listOf("scroll:-:DOWN:1", "scroll:-:DOWN:1", "scroll:-:UP:1", "scroll:-:UP:1"),
            driver.calls.filterNot { it.startsWith("find:") },
        )
    }

    @Test
    fun swipeIsTouchOnly() = runBlocking {
        val phoneDriver = FakeDriver()
        val phoneResult = CommandExecutor(phoneDriver, phoneOptions).execute(
            actionCommand("c1", action(ActionType.SWIPE, direction = Direction.LEFT)),
        )
        assertTrue(phoneResult.ok)
        assertEquals(listOf("swipe:-:LEFT"), phoneDriver.calls.toList())

        val tvDriver = FakeDriver()
        val tvResult = CommandExecutor(tvDriver, tvOptions).execute(
            actionCommand("c1", action(ActionType.SWIPE, direction = Direction.LEFT)),
        )
        assertFalse(tvResult.ok)
        assertEquals(ErrorCode.UNSUPPORTED_ACTION, tvResult.error.code)
    }

    @Test
    fun backUsesTheDriver() = runBlocking {
        val driver = FakeDriver()
        val result = CommandExecutor(driver, phoneOptions).execute(
            actionCommand("c1", action(ActionType.BACK)),
        )
        assertTrue(result.ok)
        assertEquals(listOf("back"), driver.calls.toList())
    }

    // ------------------------------------------------------------------
    // checks
    // ------------------------------------------------------------------

    private fun checkCommand(
        id: String,
        kind: CheckKind,
        selector: Selector?,
        expectedText: String = "",
        expectedCount: Int = 0,
        timeoutMs: Int = 30,
    ): Command {
        val builder = ValidateCheck.newBuilder().setKind(kind)
        selector?.let { builder.setSelector(it) }
        if (expectedText.isNotEmpty()) builder.setExpectedText(expectedText)
        if (expectedCount != 0) builder.setExpectedCount(expectedCount)
        return Command.newBuilder().setCommandId(id).setTimeoutMs(timeoutMs).setValidateCheck(builder).build()
    }

    @Test
    fun textEqualsCheckPasses() = runBlocking {
        val driver = FakeDriver()
        driver.addNode("t", text = "expected", selector = "text:OK")
        val result = CommandExecutor(driver, phoneOptions).execute(
            checkCommand("c1", CheckKind.TEXT_EQUALS, okSelector, expectedText = "expected"),
        )
        assertTrue(result.ok)
        assertEquals("expected", result.observedText)
    }

    @Test
    fun textEqualsCheckFailsWithExpectedObservedAndCount() = runBlocking {
        val driver = FakeDriver()
        driver.addNode("t", text = "actual", selector = "text:OK")
        val result = CommandExecutor(driver, phoneOptions).execute(
            checkCommand("c1", CheckKind.TEXT_EQUALS, okSelector, expectedText = "expected"),
        )
        assertFalse(result.ok)
        assertEquals(ErrorCode.CHECK_FAILED, result.error.code)
        assertEquals("expected", result.error.detailsMap["expected"])
        assertEquals("actual", result.error.detailsMap["observed"])
        assertEquals("1", result.error.detailsMap["matched_count"])
        assertEquals("text:OK", result.error.detailsMap["selector"])
    }

    @Test
    fun visibleCheckOfAMissingElementReportsMissing() = runBlocking {
        val driver = FakeDriver()
        val result = CommandExecutor(driver, phoneOptions).execute(
            checkCommand("c1", CheckKind.VISIBLE, okSelector),
        )
        assertFalse(result.ok)
        assertEquals(ErrorCode.CHECK_FAILED, result.error.code)
        assertEquals("missing", result.error.detailsMap["observed"])
        assertEquals("0", result.error.detailsMap["matched_count"])
    }

    @Test
    fun hiddenCheckPassesForAMissingElement() = runBlocking {
        val driver = FakeDriver()
        val result = CommandExecutor(driver, phoneOptions).execute(
            checkCommand("c1", CheckKind.HIDDEN, okSelector),
        )
        assertTrue(result.ok)
    }

    @Test
    fun countCheckMatchesTheSelectorArity() = runBlocking {
        val driver = FakeDriver()
        driver.addNode("a", selector = "androidClass:Button")
        driver.addNode("b", selector = "androidClass:Button")
        driver.addNode("c", selector = "androidClass:Button")
        val ok = CommandExecutor(driver, phoneOptions).execute(
            checkCommand("c1", CheckKind.COUNT, selector(SelectorStrategy.ANDROID_CLASS, "Button"), expectedCount = 3),
        )
        assertTrue(ok.ok)
        assertEquals(3, ok.observedCount)

        val failing = CommandExecutor(driver, phoneOptions).execute(
            checkCommand("c2", CheckKind.COUNT, selector(SelectorStrategy.ANDROID_CLASS, "Button"), expectedCount = 2),
        )
        assertFalse(failing.ok)
        assertEquals("3", failing.error.detailsMap["observed"])
        assertEquals("3", failing.error.detailsMap["matched_count"])
    }

    @Test
    fun enabledAndFocusedChecksWork() = runBlocking {
        val driver = FakeDriver()
        driver.addNode("t", text = "OK", enabled = true, focused = true, selector = "text:OK")
        val enabled = CommandExecutor(driver, phoneOptions).execute(
            checkCommand("c1", CheckKind.ENABLED, okSelector),
        )
        assertTrue(enabled.ok)
        val focused = CommandExecutor(driver, phoneOptions).execute(
            checkCommand("c2", CheckKind.FOCUSED, okSelector),
        )
        assertTrue(focused.ok)
    }

    @Test
    fun unspecifiedCheckKindIsAProtocolError() = runBlocking {
        val driver = FakeDriver()
        driver.addNode("t", text = "OK", selector = "text:OK")
        val result = CommandExecutor(driver, phoneOptions).execute(
            checkCommand("c1", CheckKind.CHECK_KIND_UNSPECIFIED, okSelector),
        )
        assertFalse(result.ok)
        assertEquals(ErrorCode.PROTOCOL, result.error.code)
    }

    // ------------------------------------------------------------------
    // keys, screenshots, app control, malformed commands
    // ------------------------------------------------------------------

    @Test
    fun keyPressMapsAndDefaults() = runBlocking {
        val driver = FakeDriver()
        val executor = CommandExecutor(driver, phoneOptions)
        val home = executor.execute(
            Command.newBuilder().setCommandId("k1")
                .setKeyPress(KeyPress.newBuilder().setKey(RemoteKey.KEY_HOME)).build(),
        )
        assertTrue(home.ok)
        assertEquals(listOf("pressKey:3:1:100"), driver.calls.toList())

        driver.calls.clear()
        val repeated = executor.execute(
            Command.newBuilder().setCommandId("k2")
                .setKeyPress(KeyPress.newBuilder().setKey(RemoteKey.KEY_BACKSPACE).setRepeat(3).setIntervalMs(50))
                .build(),
        )
        assertTrue(repeated.ok)
        assertEquals(listOf("pressKey:67:3:50"), driver.calls.toList())
    }

    @Test
    fun keyOkIsUnsupportedOnPhones() = runBlocking {
        val driver = FakeDriver()
        val result = CommandExecutor(driver, phoneOptions).execute(
            Command.newBuilder().setCommandId("k").setKeyPress(KeyPress.newBuilder().setKey(RemoteKey.KEY_OK)).build(),
        )
        assertFalse(result.ok)
        assertEquals(ErrorCode.UNSUPPORTED_ACTION, result.error.code)
    }

    @Test
    fun unspecifiedKeyIsAProtocolError() = runBlocking {
        val driver = FakeDriver()
        val result = CommandExecutor(driver, phoneOptions).execute(
            Command.newBuilder().setCommandId("k").setKeyPress(KeyPress.newBuilder()).build(),
        )
        assertFalse(result.ok)
        assertEquals(ErrorCode.PROTOCOL, result.error.code)
    }

    @Test
    fun screenshotCarriesThePng() = runBlocking {
        val driver = FakeDriver()
        val result = CommandExecutor(driver, phoneOptions).execute(
            Command.newBuilder().setCommandId("s").setScreenshot(
                impresario.v1.Impresario.Screenshot.newBuilder().setLabel("shot"),
            ).build(),
        )
        assertTrue(result.ok)
        assertEquals(ByteString.copyFrom(byteArrayOf(1, 2, 3, 4)), result.screenshotPng)
    }

    @Test
    fun restartTerminatesClearsAndLaunches() = runBlocking {
        val driver = FakeDriver()
        val result = CommandExecutor(driver, phoneOptions).execute(
            Command.newBuilder().setCommandId("a")
                .setAppControl(AppControl.newBuilder().setOp(AppOp.RESTART).setClearData(true))
                .build(),
        )
        assertTrue(result.ok)
        assertEquals(
            listOf("terminate:com.example.app", "clearData:com.example.app", "launch:com.example.app:tv=false"),
            driver.calls.toList(),
        )
    }

    @Test
    fun clearDataOptionAppliesToRestart() = runBlocking {
        val driver = FakeDriver()
        val options = phoneOptions.copy(clearAppDataOnRestart = true)
        CommandExecutor(driver, options).execute(
            Command.newBuilder().setCommandId("a")
                .setAppControl(AppControl.newBuilder().setOp(AppOp.RESTART))
                .build(),
        )
        assertTrue(driver.calls.contains("clearData:com.example.app"))
    }

    @Test
    fun backgroundPutsTheAppInTheBackground() = runBlocking {
        val driver = FakeDriver()
        val result = CommandExecutor(driver, phoneOptions).execute(
            Command.newBuilder().setCommandId("a")
                .setAppControl(AppControl.newBuilder().setOp(AppOp.BACKGROUND))
                .build(),
        )
        assertTrue(result.ok)
        assertEquals(listOf("putInBackground:com.example.app"), driver.calls.toList())
    }

    @Test
    fun backgroundWithoutDriverSupportIsUnsupported() = runBlocking {
        val driver = FakeDriver()
        driver.backgroundSucceeds = false
        val result = CommandExecutor(driver, phoneOptions).execute(
            Command.newBuilder().setCommandId("a")
                .setAppControl(AppControl.newBuilder().setOp(AppOp.BACKGROUND))
                .build(),
        )
        assertFalse(result.ok)
        assertEquals(ErrorCode.UNSUPPORTED_ACTION, result.error.code)
    }

    @Test
    fun crashDuringACommandIsAppCrashed() = runBlocking {
        val driver = FakeDriver()
        driver.addNode("btn", selector = "text:OK")
        driver.crash = true
        val result = CommandExecutor(driver, phoneOptions).execute(
            actionCommand("c1", action(ActionType.CLICK, okSelector)),
        )
        assertFalse(result.ok)
        assertEquals(ErrorCode.APP_CRASHED, result.error.code)
    }

    @Test
    fun cancelBodyIsHandledByTheRunner() = runBlocking {
        val driver = FakeDriver()
        val result = CommandExecutor(driver, phoneOptions).execute(
            Command.newBuilder().setCommandId("c")
                .setCancel(impresario.v1.Impresario.Cancel.newBuilder().setTargetCommandId("x"))
                .build(),
        )
        assertFalse(result.ok)
        assertEquals(ErrorCode.PROTOCOL, result.error.code)
    }

    @Test
    fun commandWithoutABodyIsUnsupported() = runBlocking {
        val driver = FakeDriver()
        val result = CommandExecutor(driver, phoneOptions).execute(
            Command.newBuilder().setCommandId("c").build(),
        )
        assertFalse(result.ok)
        assertEquals(ErrorCode.UNSUPPORTED_ACTION, result.error.code)
    }

    @Test
    fun unspecifiedActionTypeIsAProtocolError() = runBlocking {
        val driver = FakeDriver()
        val result = CommandExecutor(driver, phoneOptions).execute(
            actionCommand("c1", RunAction.newBuilder().setType(ActionType.ACTION_TYPE_UNSPECIFIED).build()),
        )
        assertFalse(result.ok)
        assertEquals(ErrorCode.PROTOCOL, result.error.code)
    }

    @Test
    fun unspecifiedDirectionIsAProtocolError() = runBlocking {
        val driver = FakeDriver()
        val result = CommandExecutor(driver, phoneOptions).execute(
            actionCommand("c1", action(ActionType.SCROLL)),
        )
        assertFalse(result.ok)
        assertEquals(ErrorCode.PROTOCOL, result.error.code)
    }

    @Test
    fun webOnlySelectorStrategyIsInvalidSelector() = runBlocking {
        val driver = FakeDriver()
        val result = CommandExecutor(driver, phoneOptions).execute(
            actionCommand("c1", action(ActionType.CLICK, selector(SelectorStrategy.CSS, "#x"))),
        )
        assertFalse(result.ok)
        assertEquals(ErrorCode.INVALID_SELECTOR, result.error.code)
    }
}
