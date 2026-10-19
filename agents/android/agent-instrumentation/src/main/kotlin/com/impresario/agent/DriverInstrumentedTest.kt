package com.impresario.agent

import android.os.ParcelFileDescriptor
import android.os.SystemClock
import androidx.test.platform.app.InstrumentationRegistry
import com.impresario.agent.driver.UiAutomatorDriver
import impresario.v1.Impresario.SelectorStrategy
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Assume.assumeFalse
import org.junit.Before
import org.junit.Test

/**
 * Driver-level validation against the stub app on a real device (spec 11.4).
 *
 * The agent lives in its own process (`:agent`) and drives the stub — a
 * separate app — cross-app (docs/DECISIONS.md I-62), so the full lifecycle
 * (terminate, clear-data, background) is exercised from here too.
 *
 * Every UiDriver capability the dispatcher relies on: finding with every
 * Android selector strategy, counting, text and flags, bounds/focus, click
 * and long press, text entry (replace/clear), the shell `input text`
 * fallback, key events incl. backspace and D-pad focus moves, scrolling
 * (implicit, container, D-pad on TV), swipes (touch platforms only),
 * screenshots and device info.
 *
 * The device decides the activation style: phones click; Android TV is
 * driven with the D-pad (the executor's TV click is focus + OK, spec 6.1).
 */
class DriverInstrumentedTest {
    private lateinit var driver: UiAutomatorDriver
    private var isTv = false

    @Before
    fun cleanStubAndOpenHome() {
        ProcessGuard.ensure(InstrumentationRegistry.getInstrumentation().targetContext)
        driver = UiAutomatorDriver(InstrumentationRegistry.getInstrumentation())
        isTv = deviceIsTv()
        dismissIme()
        // Safe cross-app lifecycle: the agent is not in the stub's process.
        driver.terminate(STUB_PACKAGE)
        driver.launch(STUB_PACKAGE, tv = isTv)
        assertEquals("Impresario stub", textOf("stub_title"))
    }

    // ------------------------------------------------------------------
    // helpers
    // ------------------------------------------------------------------

    private fun deviceIsTv(): Boolean {
        val descriptor = InstrumentationRegistry.getInstrumentation().uiAutomation
            .executeShellCommand("getprop ro.build.characteristics")
        return ParcelFileDescriptor.AutoCloseInputStream(descriptor).bufferedReader()
            .use { it.readText() }
            .contains("tv")
    }

    /** ESC dismisses any soft keyboard a previous step left open. */
    private fun dismissIme() {
        InstrumentationRegistry.getInstrumentation().uiAutomation
            .executeShellCommand("input keyevent 111")
            .close()
    }

    private fun spec(strategy: SelectorStrategy, value: String, index: Int = 0): SelectorSpec =
        SelectorSpec(strategy, value, index)

    /**
     * Resource ids at runtime are `applicationId:id/name` — the Kotlin
     * namespace never appears in the accessibility tree (docs/DECISIONS.md
     * I-58; `resource-id="com.impresario.stub:id/stub_title"` on device).
     */
    private fun id(name: String): SelectorSpec =
        spec(SelectorStrategy.ANDROID_ID, "$STUB_PACKAGE:id/$name")

    private fun findOne(selector: SelectorSpec, timeoutMs: Long = 5_000): ElementHandle {
        val found = driver.find(selector, timeoutMs)
        assertEquals("matches for ${selector.describe()}", 1, found.size)
        return found.first()
    }

    private fun textOf(name: String): String = driver.text(findOne(id(name)))

    /** Home rows in tree order; index into [scenarioRows] for [openScenario]. */
    private val scenarioRows = listOf(
        "stub_scenario_basic" to "Basic controls",
        "stub_scenario_text" to "Text entry",
        "stub_scenario_scroll" to "Scroller",
        "stub_scenario_select" to "Selection",
        "stub_scenario_flags" to "Flags",
        "stub_scenario_keys" to "Key log",
        "stub_scenario_info" to "Device info",
    )

    /** Opens scenario [index] from home: touch on phones, D-pad on TVs. */
    private fun openScenario(index: Int) {
        if (isTv) {
            val first = driver.find(id(scenarioRows[0].first), 2_000).firstOrNull()
            if (first == null || !driver.isFocused(first)) {
                // Nothing focused yet: one DOWN focuses the first row.
                driver.pressKey(KeyTable.KEYCODE_DPAD_DOWN, 1, 150)
            }
            repeat(index) { driver.pressKey(KeyTable.KEYCODE_DPAD_DOWN, 1, 150) }
            driver.pressKey(KeyTable.KEYCODE_DPAD_CENTER, 1, 150)
        } else {
            driver.click(findOne(id(scenarioRows[index].first)))
        }
        awaitTitle(scenarioRows[index].second)
    }

    private fun awaitTitle(expected: String, timeoutMs: Long = 8_000) {
        val deadline = SystemClock.uptimeMillis() + timeoutMs
        var seen = ""
        while (SystemClock.uptimeMillis() < deadline) {
            for (title in driver.find(id("stub_title"), 0)) {
                seen = driver.text(title)
                if (seen == expected) return
            }
            SystemClock.sleep(100)
        }
        fail("the title '$expected' did not appear (last seen: '$seen')")
    }

    /** Activates [name] the platform's way (TV: it must already be focused). */
    private fun activate(name: String) {
        if (isTv) {
            driver.pressKey(KeyTable.KEYCODE_DPAD_CENTER, 1, 100)
        } else {
            driver.click(findOne(id(name)))
        }
    }

    // ------------------------------------------------------------------
    // extraction: selector strategies, count, text, flags, bounds
    // ------------------------------------------------------------------

    @Test
    fun homeListsEveryScenario() {
        assertEquals("Impresario stub", textOf("stub_title"))
        for ((rowId, title) in scenarioRows) {
            val row = findOne(id(rowId), 3_000)
            assertTrue("row $rowId not visible", driver.isVisible(row))
            assertEquals(title, driver.text(row))
        }
    }

    @Test
    fun everyAndroidSelectorStrategyResolves() {
        assertEquals("Impresario stub", driver.text(findOne(id("stub_title"))))
        assertEquals(
            "Impresario stub",
            driver.text(findOne(spec(SelectorStrategy.TEXT, "Impresario stub"))),
        )
        assertTrue(
            driver.find(spec(SelectorStrategy.TEXT_CONTAINS, "mpresario stu"), 2_000).isNotEmpty(),
        )
        assertTrue(
            driver.find(spec(SelectorStrategy.ANDROID_DESC, "stub badge"), 2_000).isNotEmpty(),
        )
        assertEquals(
            "Basic controls",
            driver.text(findOne(spec(SelectorStrategy.TEXT, "Basic controls"), 2_000)),
        )
    }

    @Test
    fun countsMatchesAcrossStrategies() {
        assertEquals(7, driver.count(spec(SelectorStrategy.ANDROID_CLASS, "android.widget.Button")))
        assertEquals(1, driver.count(spec(SelectorStrategy.TEXT, "Scroller")))
        assertEquals(0, driver.count(spec(SelectorStrategy.TEXT_CONTAINS, "no such text")))
    }

    @Test
    fun unsupportedStrategyIsRejected() {
        val error = try {
            driver.find(spec(SelectorStrategy.CSS, ".anything"), 0)
            null
        } catch (failure: AgentException) {
            failure
        }
        assertNotNull("css selector on android must throw", error)
    }

    @Test
    fun boundsAndFocusReflectTheRealScreen() {
        openScenario(1) // Text entry: the field takes focus on entry.
        val title = findOne(id("stub_title"))
        val bounds = driver.bounds(title)
        assertFalse("empty bounds", bounds.isEmpty)
        val info = driver.deviceInfo()
        assertTrue("left off screen", bounds.left in 0..info.screenWidthPx)
        assertTrue("top off screen", bounds.top in 0..info.screenHeightPx)

        val input = findOne(id("stub_input"))
        assertTrue("the input did not take focus", driver.isFocused(input))
        assertNotNull("no focused bounds while a field is focused", driver.focusedBounds())
    }

    @Test
    fun flagsScreenReflectsVisibilityAndEnablement() {
        openScenario(4)
        assertTrue(driver.isEnabled(findOne(id("stub_enabled"))))
        assertFalse(driver.isEnabled(findOne(id("stub_disabled"))))
        assertTrue(driver.isVisible(findOne(id("stub_visible"))))
        val hidden = driver.find(id("stub_hidden"), 0)
        assertTrue("the invisible label must not be visible", hidden.none { driver.isVisible(it) })
    }

    @Test
    fun countsOnTheFlagsScreenAreExact() {
        openScenario(4)
        assertEquals(2, driver.count(spec(SelectorStrategy.ANDROID_CLASS, "android.widget.Button")))
        assertEquals(0, driver.count(spec(SelectorStrategy.TEXT_CONTAINS, "taps=")))
    }

    // ------------------------------------------------------------------
    // interactions
    // ------------------------------------------------------------------

    @Test
    fun clickUpdatesTheTapCounter() {
        openScenario(0)
        activate("stub_button")
        assertEquals("taps=1", textOf("stub_counter"))
        assertEquals("status=clicked", textOf("stub_status"))
    }

    @Test
    fun longPressUpdatesTheStatus() {
        openScenario(0)
        if (isTv) {
            assertTrue(
                "the TV long-press key fallback failed",
                driver.longPressKey(KeyTable.KEYCODE_DPAD_CENTER, 1_200),
            )
        } else {
            driver.longClick(findOne(id("stub_button")), 1_200)
        }
        assertEquals("status=long-pressed", textOf("stub_status"))
    }

    @Test
    fun typingIntoTheFieldUpdatesTheEcho() {
        openScenario(1)
        val input = findOne(id("stub_input"))
        driver.setText(input, "driver-e2e")
        assertEquals("driver-e2e", driver.text(input))
        assertEquals("echo:driver-e2e", textOf("stub_echo"))
    }

    @Test
    fun clearingTheFieldEmptiesTheEcho() {
        openScenario(1)
        val input = findOne(id("stub_input"))
        driver.setText(input, "to-clear")
        assertEquals("echo:to-clear", textOf("stub_echo"))
        driver.clear(input)
        assertEquals("echo:", textOf("stub_echo"))
    }

    @Test
    fun backspaceKeyDeletesTheLastCharacter() {
        openScenario(1)
        val input = findOne(id("stub_input"))
        driver.setText(input, "abc")
        driver.pressKey(KeyTable.KEYCODE_DEL, 1, 100)
        assertEquals("ab", driver.text(input))
        assertEquals("echo:ab", textOf("stub_echo"))
    }

    @Test
    fun inputTextFallbackTypesIntoTheFocusedField() {
        openScenario(1)
        val input = findOne(id("stub_input"))
        driver.setText(input, "")
        assertTrue("the input did not take focus", driver.isFocused(input))
        // `input text` is IME-layout dependent — a letters layout may remap to
        // another script (observed with an Arabic keyboard on this TV), so the
        // fallback test types digits, which every layout sends verbatim
        // (docs/DECISIONS.md I-64).
        assertTrue("input text fallback failed", driver.inputText("12345"))
        assertEquals("12345", driver.text(input))
        assertEquals("echo:12345", textOf("stub_echo"))
    }

    @Test
    fun keysMoveFocusAndLogTheKey() {
        openScenario(5)
        assertTrue("Alpha did not take focus", driver.isFocused(findOne(id("stub_key_a"))))
        driver.pressKey(KeyTable.KEYCODE_DPAD_DOWN, 1, 150)
        assertTrue("DOWN did not move focus to Bravo", driver.isFocused(findOne(id("stub_key_b"))))
        assertTrue(
            "the key log missed DPAD_DOWN: ${textOf("stub_key_log")}",
            textOf("stub_key_log").contains("key=DPAD_DOWN"),
        )
        driver.pressKey(KeyTable.KEYCODE_DPAD_UP, 1, 150)
        assertTrue("UP did not move focus back to Alpha", driver.isFocused(findOne(id("stub_key_a"))))
        assertTrue(textOf("stub_key_log").contains("key=DPAD_UP"))
        driver.pressKey(KeyTable.KEYCODE_DPAD_CENTER, 1, 150)
        assertTrue(
            "OK did not activate Alpha: ${textOf("stub_key_log")}",
            textOf("stub_key_log").contains("clicked=Alpha"),
        )
    }

    @Test
    fun selectDialogPicksAnOption() {
        openScenario(3)
        val option = spec(SelectorStrategy.TEXT, "Option B")
        if (isTv) {
            // The trigger already has focus; OK opens the dialog. AlertDialog
            // list rows keep the D-pad focus on the list itself (they never
            // report `focused`), so the option is activated through the
            // accessibility click action — the same fallback the executor's TV
            // click uses (docs/DECISIONS.md I-64).
            driver.pressKey(KeyTable.KEYCODE_DPAD_CENTER, 1, 150)
        } else {
            driver.click(findOne(id("stub_select")))
        }
        driver.click(findOne(option, 5_000))
        val deadline = SystemClock.uptimeMillis() + 5_000
        var status = ""
        while (SystemClock.uptimeMillis() < deadline) {
            status = textOf("stub_status")
            if (status == "chosen: Option B") return
            SystemClock.sleep(100)
        }
        fail("the dialog choice did not land (status: '$status')")
    }

    @Test
    fun backLeavesTheStubAndLaunchReturns() {
        driver.pressBack()
        assertTrue(
            "the stub is still on screen after back",
            driver.find(id("stub_title"), 2_000).isEmpty(),
        )
        driver.launch(STUB_PACKAGE, tv = isTv)
        assertEquals("Impresario stub", textOf("stub_title"))
    }

    // ------------------------------------------------------------------
    // scrolling and swiping
    // ------------------------------------------------------------------

    @Test
    fun scrollReachesLateRows() {
        openScenario(2)
        val late = spec(SelectorStrategy.TEXT, "row 29")
        var attempts = 0
        while (attempts < 16 && driver.find(late, 0).none { driver.isVisible(it) }) {
            if (isTv) {
                driver.pressKey(KeyTable.KEYCODE_DPAD_DOWN, 1, 120)
            } else {
                driver.scroll(null, Direction.DOWN, 1)
            }
            attempts += 1
        }
        assertTrue(
            "row 29 not reached after $attempts steps",
            driver.find(late, 2_000).any { driver.isVisible(it) },
        )
    }

    @Test
    fun containerScrollReachesTheEndAndBack() {
        assumeFalse("touch-only path (swipe)", isTv)
        openScenario(2)
        val scroller = findOne(id("stub_scroller"))
        driver.scroll(scroller, Direction.DOWN, 6)
        val last = driver.find(spec(SelectorStrategy.TEXT, "row 29"), 2_000)
        assertTrue("row 29 not visible after scrolling down", last.any { driver.isVisible(it) })
        driver.scroll(scroller, Direction.UP, 8)
        val first = driver.find(spec(SelectorStrategy.TEXT, "row 0"), 2_000)
        assertTrue("row 0 not visible after scrolling back up", first.any { driver.isVisible(it) })
    }

    @Test
    fun elementSwipeScrollsTheListBothWays() {
        assumeFalse("touch-only path (swipe)", isTv)
        openScenario(2)
        val scroller = findOne(id("stub_scroller"))
        val first = spec(SelectorStrategy.TEXT, "row 0")
        assertTrue(driver.find(first, 1_000).any { driver.isVisible(it) })

        driver.swipe(scroller, Direction.UP)
        val afterUp = driver.find(first, 0)
        assertTrue(
            "row 0 still on screen after swiping up",
            afterUp.none { driver.isVisible(it) },
        )

        driver.swipe(scroller, Direction.DOWN)
        val afterDown = driver.find(first, 1_000)
        assertTrue("row 0 not back on screen after swiping down", afterDown.any { driver.isVisible(it) })
    }

    @Test
    fun screenSwipeKeepsTheAppUp() {
        assumeFalse("touch-only path (swipe)", isTv)
        openScenario(2)
        driver.swipe(null, Direction.UP)
        driver.swipe(null, Direction.DOWN)
        awaitTitle("Scroller")
    }

    // ------------------------------------------------------------------
    // lifecycle, screenshots, device info
    // ------------------------------------------------------------------

    @Test
    fun terminateStopsTheStubAndLaunchRestarts() {
        driver.terminate(STUB_PACKAGE)
        assertTrue(
            "the stub survived terminate",
            driver.find(id("stub_title"), 2_000).isEmpty(),
        )
        driver.launch(STUB_PACKAGE, tv = isTv)
        assertEquals("Impresario stub", textOf("stub_title"))
    }

    @Test
    fun clearDataResetsTheScenarioState() {
        openScenario(0)
        activate("stub_button")
        assertEquals("taps=1", textOf("stub_counter"))
        val cleared = driver.clearData(STUB_PACKAGE)
        if (!cleared) {
            // Some ROMs deny shell `pm clear` (docs/DECISIONS.md I-82); the
            // executor then force-stops so the launch is still a cold start.
            driver.terminate(STUB_PACKAGE)
        }
        driver.launch(STUB_PACKAGE, tv = isTv)
        awaitTitle("Impresario stub")
        openScenario(0)
        assertEquals("taps=0", textOf("stub_counter"))
    }

    @Test
    fun putInBackgroundLeavesTheStub() {
        assertTrue(driver.putInBackground(STUB_PACKAGE))
        assertTrue(
            "the stub is still on screen after background",
            driver.find(id("stub_title"), 2_000).isEmpty(),
        )
        driver.launch(STUB_PACKAGE, tv = isTv)
        assertEquals("Impresario stub", textOf("stub_title"))
    }

    @Test
    fun infoScreenShowsTheStubIdentity() {
        openScenario(6)
        val info = textOf("stub_info")
        assertTrue("info does not name the stub: $info", info.contains("pkg=$STUB_PACKAGE"))
        assertTrue("info does not match this device: $info", info.contains("tv=$isTv"))
    }

    @Test
    fun screenshotIsARealPng() {
        val png = driver.screenshotPng()
        assertTrue("screenshot too small: ${png.size}", png.size > 1_000)
        val dimensions = Png.dimensions(png)
        assertNotNull("screenshot is not a PNG", dimensions)
        assertTrue(dimensions!!.first > 0 && dimensions.second > 0)
    }

    @Test
    fun deviceInfoDescribesTheUiAutomatorRuntime() {
        val info = driver.deviceInfo()
        assertEquals("UiAutomator2", info.runtime)
        assertEquals("Android", info.osName)
        assertTrue(info.screenWidthPx > 0)
        assertTrue(info.screenHeightPx > 0)
        assertTrue(info.pixelRatio > 0f)
    }

    private companion object {
        /** The installed stub app id (applicationId; ids live under it). */
        const val STUB_PACKAGE = "com.impresario.stub"
    }
}
