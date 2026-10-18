package com.impresario.agent.driver

import android.app.Instrumentation
import android.content.Intent
import android.graphics.Bitmap
import android.os.Build
import android.os.SystemClock
import android.view.KeyCharacterMap
import android.view.KeyEvent
import androidx.test.uiautomator.By
import androidx.test.uiautomator.BySelector
import androidx.test.uiautomator.StaleObjectException
import androidx.test.uiautomator.UiDevice
import androidx.test.uiautomator.UiObject2
import com.impresario.agent.AgentException
import com.impresario.agent.DeviceInfoData
import com.impresario.agent.Direction
import com.impresario.agent.DriverExtras
import com.impresario.agent.ElementHandle
import com.impresario.agent.Rect
import com.impresario.agent.SelectorSpec
import com.impresario.agent.UiDriver
import impresario.v1.Impresario.SelectorStrategy
import java.io.ByteArrayOutputStream

/** An [ElementHandle] backed by a UiAutomator node snapshot. */
class UiHandle(internal val object2: UiObject2) : ElementHandle

/**
 * The real driver (spec 11.4). All UiAutomator interaction is here; the
 * dispatcher only sees [UiDriver].
 *
 * Notes recorded in docs/DECISIONS (I-49..I-53, I-66, I-71, I-72):
 * - bare `androidId` values are resolved against the last launched package;
 * - `visible` is `getVisibleBounds()` being non-empty (UiObject2 has no
 *   `isVisible`);
 * - `scroll` uses `UiObject2.scroll(dir, 0.8)` on a scrollable (the first
 *   scrollable of the screen when no container is given), falling back to a
 *   screen swipe when no scrollable exists; it reports whether the content
 *   actually moved via uiautomator's scroll-finished signal (I-75);
 * - `input text` escapes spaces as `%s` and only accepts printable ASCII;
 * - `text` treats a field's hint as empty content: an empty EditText reports
 *   its hint through the a11y text (framework behavior), so text that equals
 *   the hint is mapped to `""` (I-72);
 * - crash detection watches `pidof` (after a launch) and the system crash
 *   dialogs.
 */
class UiAutomatorDriver(
    private val instrumentation: Instrumentation,
    private val device: UiDevice = UiDevice.getInstance(instrumentation),
) : UiDriver, DriverExtras {

    @Volatile
    var lastLaunchedPackage: String = ""
        private set

    override fun find(sel: SelectorSpec, timeoutMs: Long): List<ElementHandle> {
        val by = byOf(sel)
        val deadline = SystemClock.uptimeMillis() + timeoutMs
        var matches = device.findObjects(by)
        while (matches.isEmpty() && SystemClock.uptimeMillis() < deadline) {
            SystemClock.sleep(POLL_INTERVAL_MS)
            matches = device.findObjects(by)
        }
        return matches.map { UiHandle(it) }
    }

    override fun count(sel: SelectorSpec): Int = device.findObjects(byOf(sel)).size

    override fun click(e: ElementHandle) = act { obj(e).click() }

    override fun longClick(e: ElementHandle, ms: Long) = act { obj(e).click(ms) }

    override fun setText(e: ElementHandle, text: String) = act { obj(e).setText(text) }

    override fun clear(e: ElementHandle) = act { obj(e).clear() }

    override fun text(e: ElementHandle): String = tolerant("") {
        val node = obj(e)
        val text = node.text ?: ""
        // An empty EditText reports its *hint* as the a11y text (framework
        // behavior); `UiObject2.hint` exposes the hint separately, so text
        // that is nothing but the hint means the field is empty (I-72).
        if (text.isNotEmpty() && text == node.hint) "" else text
    }

    override fun isVisible(e: ElementHandle): Boolean = tolerant(false) { !obj(e).visibleBounds.isEmpty }

    override fun isEnabled(e: ElementHandle): Boolean = tolerant(false) { obj(e).isEnabled }

    override fun isFocused(e: ElementHandle): Boolean = tolerant(false) { obj(e).isFocused }

    override fun bounds(e: ElementHandle): Rect = tolerant(Rect(0, 0, 0, 0)) { rect(obj(e).visibleBounds) }

    override fun focusedBounds(): Rect? =
        device.findObject(By.focused(true))?.visibleBounds?.let { rect(it) }

    override fun pressKey(code: Int, repeat: Int, intervalMs: Long) {
        val times = repeat.coerceAtLeast(1)
        for (index in 0 until times) {
            device.pressKeyCode(code)
            if (index < times - 1 && intervalMs > 0) {
                SystemClock.sleep(intervalMs)
            }
        }
    }

    override fun pressBack() {
        device.pressBack()
    }

    override fun scroll(container: ElementHandle?, dir: Direction, steps: Int): Boolean {
        val target = container?.let { obj(it) } ?: primaryScrollable()
        val times = steps.coerceAtLeast(1)
        var moved = false
        repeat(times) {
            val node = target
            if (node == null || !tolerant(false) { node.isScrollable }) {
                // Nothing scrollable to drive: fall back to a blind screen
                // swipe, whose effect this driver cannot observe (I-75).
                screenSwipe(dir, SCROLL_PERCENT)
                moved = true
                return@repeat
            }
            val scrolled = try {
                node.scroll(direction(dir), SCROLL_PERCENT)
            } catch (error: Exception) {
                false
            }
            if (!scrolled) {
                // `UiObject2.scroll` waits for the accessibility
                // scroll-finished event; none arrived, so the content did
                // not move - the container sits at that end and further
                // steps cannot advance it (I-75).
                return moved
            }
            moved = true
        }
        return moved
    }

    override fun swipe(e: ElementHandle?, dir: Direction) {
        val node = e?.let { obj(it) }
        if (node != null && !node.visibleBounds.isEmpty) {
            node.swipe(direction(dir), SWIPE_PERCENT)
        } else {
            screenSwipe(dir, SWIPE_PERCENT)
        }
    }

    override fun screenshotPng(): ByteArray {
        val bitmap = device.takeScreenshot() ?: return ByteArray(0)
        val stream = ByteArrayOutputStream()
        val compressed = bitmap.compress(Bitmap.CompressFormat.PNG, 100, stream)
        return if (compressed) stream.toByteArray() else ByteArray(0)
    }

    override fun launch(appId: String, tv: Boolean) {
        requirePackageName(appId)
        val context = instrumentation.targetContext
        // `getLaunchIntentForPackage` only matches CATEGORY_LAUNCHER; TV-only
        // apps declare LEANBACK_LAUNCHER alone, so fall back for them. The
        // agent app declares QUERY_ALL_PACKAGES so the target is visible to
        // it at all (docs/DECISIONS.md I-62).
        val intent = context.packageManager.getLaunchIntentForPackage(appId)
            ?: (if (tv) context.packageManager.getLeanbackLaunchIntentForPackage(appId) else null)
            ?: throw AgentException.internal("no launcher activity for package '$appId'")
        intent.addFlags(
            Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_RESET_TASK_IF_NEEDED,
        )
        context.startActivity(intent)
        lastLaunchedPackage = appId
        wasLaunched = true
        val deadline = SystemClock.uptimeMillis() + LAUNCH_TIMEOUT_MS
        while (SystemClock.uptimeMillis() < deadline) {
            if (device.currentPackageName == appId) return
            SystemClock.sleep(POLL_INTERVAL_MS)
        }
        throw AgentException.internal("'$appId' did not reach the foreground within ${LAUNCH_TIMEOUT_MS}ms")
    }

    override fun terminate(appId: String) {
        requirePackageName(appId)
        device.executeShellCommand("am force-stop $appId")
        wasLaunched = false
    }

    override fun clearData(appId: String) {
        requirePackageName(appId)
        device.executeShellCommand("pm clear $appId")
        wasLaunched = false
    }

    override fun deviceInfo(): DeviceInfoData = DeviceInfoData(
        model = device.productName.ifBlank { Build.MODEL },
        osName = "Android",
        osVersion = Build.VERSION.RELEASE,
        screenWidthPx = device.displayWidth,
        screenHeightPx = device.displayHeight,
        pixelRatio = instrumentation.targetContext.resources.displayMetrics.density,
        runtime = "UiAutomator2",
    )

    // ------------------------------------------------------------------
    // DriverExtras
    // ------------------------------------------------------------------

    override fun inputText(text: String): Boolean {
        if (text.isEmpty()) return true
        if (text.any { it.code !in 32..126 }) {
            throw AgentException.unsupportedAction(
                "`input text` can only send printable ASCII",
            )
        }
        // `executeShellCommand` hands the argv tokens over without a shell
        // parse, so quotes would be typed literally; spaces are escaped as
        // `%s`, which `input text` maps back to spaces.
        val escaped = text.replace(" ", "%s")
        device.executeShellCommand("input text $escaped")
        return true
    }

    override fun longPressKey(keyCode: Int, durationMs: Long): Boolean {
        // A real remote "hold": down, sleep past the focused view's own
        // long-press timer (`View.onKeyDown` schedules its 500 ms
        // long-click callback for confirm keys), a repeat flagged
        // `FLAG_LONG_PRESS` where the input system would put it (the marker
        // for apps that track the key via `event.startTracking()` +
        // `onKeyLongPress`), then up. `input keyevent --longpress` cannot
        // express this: it emits down/flagged-repeat/up with the same
        // timestamp, so the timer never elapses and the up degrades into a
        // plain click. Injection goes through `UiAutomation` (the channel
        // behind `UiDevice.pressKeyCode`, which injects with the shell
        // identity): `Instrumentation.sendKeySync` fails on the device with
        // "Injecting to another application requires INJECT_EVENTS
        // permission" (docs/DECISIONS.md I-66).
        val holdMs = durationMs.coerceAtLeast(LONG_PRESS_HOLD_MS)
        val downTime = SystemClock.uptimeMillis()
        val down = KeyEvent(
            downTime, downTime, KeyEvent.ACTION_DOWN, keyCode, 0,
            0, KeyCharacterMap.VIRTUAL_KEYBOARD, 0, 0,
        )
        injectKey(down)
        val repeatAtMs = holdMs.coerceAtMost(LONG_PRESS_REPEAT_MS)
        SystemClock.sleep(repeatAtMs)
        injectKey(
            KeyEvent.changeTimeRepeat(
                down,
                SystemClock.uptimeMillis(),
                1,
                KeyEvent.FLAG_LONG_PRESS,
            ),
        )
        SystemClock.sleep(holdMs - repeatAtMs)
        injectKey(
            KeyEvent(
                downTime, SystemClock.uptimeMillis(), KeyEvent.ACTION_UP, keyCode, 0,
                0, KeyCharacterMap.VIRTUAL_KEYBOARD, 0, 0,
            ),
        )
        return true
    }

    private fun injectKey(event: KeyEvent) {
        if (!instrumentation.uiAutomation.injectInputEvent(event, true)) {
            throw AgentException.internal("key event injection was rejected")
        }
    }

    override fun putInBackground(appId: String): Boolean {
        requirePackageName(appId)
        // The home intent backgrounds whatever is in the foreground; on a TV
        // this is the same gesture as the remote's home key.
        device.pressHome()
        return true
    }

    override fun targetCrashed(appId: String): Boolean {
        if (crashDialogVisible()) return true
        if (!wasLaunched) return false
        return try {
            device.executeShellCommand("pidof $appId").trim().isEmpty()
        } catch (error: Exception) {
            false
        }
    }

    // ------------------------------------------------------------------
    // helpers
    // ------------------------------------------------------------------

    /**
     * Reads tolerate a handle that went stale while querying: real UIs change
     * constantly and checks poll. UiObject2 re-runs its selector on every
     * access, so one retry usually succeeds; otherwise the neutral value
     * stands in for "element not there this instant" (the next poll sees the
     * fresh tree) instead of failing the whole command.
     */
    private inline fun <T> tolerant(fallback: T, block: () -> T): T =
        try {
            block()
        } catch (_: StaleObjectException) {
            SystemClock.sleep(RETRY_PAUSE_MS)
            try {
                block()
            } catch (_: StaleObjectException) {
                fallback
            }
        }

    /**
     * Actions never retry — a repeated click would fire twice — but a stale
     * handle must surface as the wire's `STALE_ELEMENT`, not as a raw
     * UiAutomator exception (docs/DECISIONS.md I-65).
     */
    private inline fun act(block: () -> Unit) {
        try {
            block()
        } catch (_: StaleObjectException) {
            throw AgentException.staleElement("the element became stale before the action could run")
        }
    }

    private fun obj(e: ElementHandle): UiObject2 =
        (e as? UiHandle)?.object2 ?: throw AgentException.staleElement("the element handle is not from this driver")

    private fun rect(bounds: android.graphics.Rect): Rect =
        Rect(bounds.left, bounds.top, bounds.right, bounds.bottom)

    private fun direction(dir: Direction) = when (dir) {
        Direction.UP -> androidx.test.uiautomator.Direction.UP
        Direction.DOWN -> androidx.test.uiautomator.Direction.DOWN
        Direction.LEFT -> androidx.test.uiautomator.Direction.LEFT
        Direction.RIGHT -> androidx.test.uiautomator.Direction.RIGHT
    }

    private fun primaryScrollable(): UiObject2? =
        device.findObjects(By.scrollable(true)).firstOrNull { !it.visibleBounds.isEmpty }

    private fun screenSwipe(dir: Direction, fraction: Float) {
        val width = device.displayWidth
        val height = device.displayHeight
        val centerX = width / 2
        val centerY = height / 2
        val travel = when (dir) {
            Direction.DOWN, Direction.UP -> (height * fraction / 2).toInt()
            Direction.LEFT, Direction.RIGHT -> (width * fraction / 2).toInt()
        }
        val steps = SWIPE_STEPS
        when (dir) {
            Direction.DOWN -> device.swipe(centerX, centerY + travel, centerX, centerY - travel, steps)
            Direction.UP -> device.swipe(centerX, centerY - travel, centerX, centerY + travel, steps)
            Direction.RIGHT -> device.swipe(centerX - travel, centerY, centerX + travel, centerY, steps)
            Direction.LEFT -> device.swipe(centerX + travel, centerY, centerX - travel, centerY, steps)
        }
    }

    private fun crashDialogVisible(): Boolean =
        CRASH_TEXTS.any { device.hasObject(By.textContains(it)) }

    private fun byOf(sel: SelectorSpec): BySelector = when (sel.strategy) {
        SelectorStrategy.ANDROID_ID ->
            if (sel.value.contains(':')) By.res(sel.value)
            else By.res(lastLaunchedPackage.ifEmpty { device.currentPackageName ?: "" }, sel.value)
        SelectorStrategy.ANDROID_DESC -> By.desc(sel.value)
        SelectorStrategy.ANDROID_CLASS -> By.clazz(sel.value)
        SelectorStrategy.TEXT -> By.text(sel.value)
        SelectorStrategy.TEXT_CONTAINS -> By.textContains(sel.value)
        else -> throw AgentException.invalidSelector(
            "selector strategy '${SelectorSpec.strategyName(sel.strategy)}' does not apply to android",
        )
    }

    private fun requirePackageName(appId: String) {
        if (!PACKAGE_NAME.matches(appId)) {
            throw AgentException.protocol("'$appId' is not a valid package name")
        }
    }

    private var wasLaunched: Boolean = false

    private companion object {
        const val POLL_INTERVAL_MS = 50L
        const val RETRY_PAUSE_MS = 60L

        /**
         * Generous window for the app under test to reach the foreground:
         * TV hardware can be slow right after `am force-stop`, especially
         * under vendor power management (docs/DECISIONS.md I-67).
         */
        const val LAUNCH_TIMEOUT_MS = 20_000L

        /** Minimum key hold for a TV long press (view timer is 500 ms). */
        const val LONG_PRESS_HOLD_MS = 600L

        /** Offset of the `FLAG_LONG_PRESS` repeat inside the hold. */
        const val LONG_PRESS_REPEAT_MS = 600L
        const val SCROLL_PERCENT = 0.8f
        const val SWIPE_PERCENT = 0.5f
        const val SWIPE_STEPS = 20
        val PACKAGE_NAME = Regex("^[A-Za-z0-9_]+(\\.[A-Za-z0-9_]+)+$")
        val CRASH_TEXTS = listOf(
            "isn't responding",
            "keeps stopping",
            "has stopped",
            "Close app",
        )
    }
}
