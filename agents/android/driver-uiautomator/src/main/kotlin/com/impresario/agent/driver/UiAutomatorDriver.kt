package com.impresario.agent.driver

import android.app.Instrumentation
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Rect as AndroidRect
import android.os.Build
import android.os.Bundle
import android.os.SystemClock
import android.view.KeyCharacterMap
import android.view.KeyEvent
import android.view.accessibility.AccessibilityNodeInfo
import android.view.accessibility.AccessibilityWindowInfo
import androidx.test.uiautomator.By
import androidx.test.uiautomator.BySelector
import androidx.test.uiautomator.Configurator
import androidx.test.uiautomator.StaleObjectException
import androidx.test.uiautomator.UiDevice
import androidx.test.uiautomator.UiObject2
import com.impresario.agent.AgentException
import com.impresario.agent.DeviceInfoData
import com.impresario.agent.Direction
import com.impresario.agent.DrmSnapshot
import com.impresario.agent.DriverExtras
import com.impresario.agent.ElementHandle
import com.impresario.agent.InspectorCapture
import com.impresario.agent.InspectorCaptureOptions
import com.impresario.agent.InspectorDriver
import com.impresario.agent.MediaSnapshot
import com.impresario.agent.Rect
import com.impresario.agent.SelectorSpec
import com.impresario.agent.ShowingSnapshot
import com.impresario.agent.UiDriver
import impresario.v1.Impresario.InspectorNode
import impresario.v1.Impresario.InspectorRect
import impresario.v1.Impresario.SelectorStrategy
import java.io.ByteArrayOutputStream
import kotlin.math.roundToInt
import org.json.JSONObject

/** Screen-gesture endpoints in device pixels. */
internal data class SwipePath(
    val startX: Int,
    val startY: Int,
    val endX: Int,
    val endY: Int,
)

/**
 * `swipe <dir>` names the finger direction (docs/DSL.md 5). `UP` therefore
 * starts below the centre and ends above it; the horizontal mapping is the
 * mirror image.
 */
internal fun fingerSwipePath(
    direction: Direction,
    width: Int,
    height: Int,
    fraction: Float,
): SwipePath {
    val centerX = width / 2
    val centerY = height / 2
    val travel = when (direction) {
        Direction.UP, Direction.DOWN -> (height * fraction / 2).toInt()
        Direction.LEFT, Direction.RIGHT -> (width * fraction / 2).toInt()
    }
    return when (direction) {
        Direction.UP -> SwipePath(centerX, centerY + travel, centerX, centerY - travel)
        Direction.DOWN -> SwipePath(centerX, centerY - travel, centerX, centerY + travel)
        Direction.RIGHT -> SwipePath(centerX - travel, centerY, centerX + travel, centerY)
        Direction.LEFT -> SwipePath(centerX + travel, centerY, centerX - travel, centerY)
    }
}

/**
 * `scroll <dir>` names the content direction (docs/DSL.md 5), so its screen
 * fallback moves the finger the opposite way.
 */
internal fun scrollSwipePath(
    direction: Direction,
    width: Int,
    height: Int,
    fraction: Float,
): SwipePath = fingerSwipePath(direction.opposite(), width, height, fraction)

private fun Direction.opposite(): Direction = when (this) {
    Direction.UP -> Direction.DOWN
    Direction.DOWN -> Direction.UP
    Direction.LEFT -> Direction.RIGHT
    Direction.RIGHT -> Direction.LEFT
}

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
) : UiDriver, DriverExtras, InspectorDriver {

    init {
        // uiautomator waits up to 10 s for an idle window before every
        // accessibility query. A playing video never reports idle, so each
        // find() on the Media screen outlived its command deadline. Impresario
        // owns readiness: the master polls bounded checks and the hook reports
        // app state, so implicit idling is disabled here.
        Configurator.getInstance().setWaitForIdleTimeout(0)
    }

    /** Last snapshot's native nodes and pixel rects, used by inspector input. */
    private var inspectorNodes: Map<Long, AccessibilityNodeInfo> = emptyMap()
    private var inspectorRects: Map<Long, Rect> = emptyMap()
    private var inspectorPixelRatio: Float = 1f

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
                // `scroll` names the content direction, so the finger moves
                // the other way.
                screenSwipe(scrollSwipePath(dir, device.displayWidth, device.displayHeight, SCROLL_PERCENT))
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
            // `swipe` names the finger direction (DSL 5); keep the same
            // endpoint mapping as UiObject2.swipe for the screen fallback.
            screenSwipe(fingerSwipePath(dir, device.displayWidth, device.displayHeight, SWIPE_PERCENT))
        }
    }

    override fun screenshotPng(): ByteArray {
        // Let a just-started activity transition settle; with implicit idle
        // waits disabled, a capture immediately after launch can blend frames.
        Thread.sleep(500)
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
        val component = intent.component
            ?: throw AgentException.internal("launcher intent for '$appId' has no component")
        // Start through `am` instead of `context.startActivity`. The agent
        // instrumentation is a background process, so Android 10+ background
        // activity-launch restrictions can silently drop the latter; TCL's
        // freezer can also leave the previous task suspended. The shell start
        // is what the device's own launcher uses and is not subject to either
        // restriction. `-W` waits for the activity manager to finish the
        // launch so the foreground poll starts from a settled state.
        val start = device.executeShellCommand("am start -W -n ${component.flattenToShortString()}")
        if (start.contains("Error") || start.contains("Exception")) {
            throw AgentException.internal("could not launch '$appId': ${start.trim()}")
        }
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

    override fun clearData(appId: String): Boolean {
        requirePackageName(appId)
        val output = device.executeShellCommand("pm clear $appId")
        wasLaunched = false
        // `pm clear` prints `Success`; anything else (including the
        // SecurityException text some ROMs return) means the data survives.
        return output.contains("Success")
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

    /**
     * Reads the platform MediaSession stack. This is the legitimate public
     * state channel: `dumpsys media_session` is the same data the system media
     * notification renders, and needs no privileged permission.
     */
    override fun mediaSnapshot(): MediaSnapshot? {
        hookSnapshot()?.optJSONObject("media")?.let { hook ->
            val state = hook.optString("state").takeIf { it.isNotEmpty() }
            return MediaSnapshot(
                state = state,
                positionSeconds = hook.optDouble("positionSeconds").takeIf { !it.isNaN() },
                durationSeconds = hook.optDouble("durationSeconds").takeIf { !it.isNaN() },
                rate = hook.optDouble("rate").takeIf { !it.isNaN() },
                volume = hook.optDouble("volume").takeIf { !it.isNaN() },
                muted = hook.optBoolean("muted").takeIf { hook.has("muted") },
                widthPx = hook.optInt("widthPx").takeIf { hook.has("widthPx") },
                heightPx = hook.optInt("heightPx").takeIf { hook.has("heightPx") },
                audioTrack = hook.optString("audioTrack").takeIf { it.isNotEmpty() },
                subtitleTrack = hook.optString("subtitleTrack").takeIf { it.isNotEmpty() },
                source = hook.optString("source").takeIf { it.isNotEmpty() },
                error = hook.optString("error").takeIf { it.isNotEmpty() },
                decoded = hook.optBoolean("decoded").takeIf { hook.has("decoded") },
                origin = "app_hook",
            )
        }
        val output = runCatching { device.executeShellCommand("dumpsys media_session") }
            .getOrNull()
            ?: return null
        if (output.isBlank()) return null
        val pkg = lastLaunchedPackage.ifEmpty { device.currentPackageName ?: "" }
        if (pkg.isEmpty()) return null
        val blocks = mediaSessionBlocks(output)
        val block = blocks.firstOrNull { it.contains("package=$pkg") } ?: return null
        val stateNumber = REGEX_PLAYBACK_STATE.find(block)?.groupValues?.get(1)?.toIntOrNull()
        val state = when (stateNumber) {
            0 -> "Idle"
            1 -> "Ended"
            2 -> "Paused"
            3, 4, 5, 9, 10 -> "Playing"
            6 -> "Buffering"
            7 -> "Error"
            8 -> "Loading"
            else -> null
        }
        val speed = REGEX_SPEED.find(block)?.groupValues?.get(1)?.toDoubleOrNull()
        val positionMs = REGEX_POSITION.find(block)?.groupValues?.get(1)?.toLongOrNull()
        val updated = REGEX_UPDATED.find(block)?.groupValues?.get(1)?.toLongOrNull()
        val position = positionMs?.let {
            var seconds = it / 1000.0
            if (updated != null && speed != null && speed > 0.0) {
                val elapsed = (SystemClock.elapsedRealtime() - updated).coerceAtLeast(0)
                seconds += elapsed * speed / 1000.0
            }
            seconds.coerceAtLeast(0.0)
        }
        val duration = REGEX_DURATION.find(block)?.groupValues?.get(1)?.toDoubleOrNull()
            ?.let { if (it > 10_000) it / 1000.0 else it }
        val source = REGEX_MEDIA_URI.find(block)?.groupValues?.get(1)
        return MediaSnapshot(
            state = state,
            positionSeconds = position,
            durationSeconds = duration,
            rate = speed,
            source = source,
            error = if (state == "Error") "media session reported ERROR" else null,
            origin = "media_session",
        )
    }

    /**
     * Reads the optional hook's dedicated `showing` object, preferring it over
     * the legacy `media.decoded/source` fields that early SDK users reported.
     */
    override fun showingSnapshot(): ShowingSnapshot? {
        val hook = hookSnapshot() ?: return null
        val showing = hook.optJSONObject("showing")
        val media = hook.optJSONObject("media")
        val showingDecoded = showing?.takeIf { it.has("decoded") }?.optBoolean("decoded")
        val mediaDecoded = media?.takeIf { it.has("decoded") }?.optBoolean("decoded")
        val decoded = showingDecoded ?: mediaDecoded
        val source = showing?.optString("source")?.takeIf { it.isNotEmpty() }
            ?: media?.optString("source")?.takeIf { it.isNotEmpty() }
        if (decoded == null && source == null) return null
        return ShowingSnapshot(
            decoded = decoded,
            source = source,
            piece = showing?.optString("piece")?.takeIf { it.isNotEmpty() },
        )
    }

    /**
     * Only reports DRM when a DRM service shows an active session; a mere
     * provider being installed is not evidence that this playback uses it.
     */
    override fun drmSnapshot(): DrmSnapshot? {
        hookSnapshot()?.optJSONObject("drm")?.let { hook ->
            return DrmSnapshot(
                enabled = hook.optBoolean("enabled").takeIf { hook.has("enabled") },
                provider = hook.optString("provider").takeIf { it.isNotEmpty() },
                license = hook.optString("license").takeIf { it.isNotEmpty() },
                keyStatus = hook.optString("keyStatus").takeIf { it.isNotEmpty() },
                securityLevel = hook.optString("securityLevel").takeIf { it.isNotEmpty() },
                hdcp = hook.optString("hdcp").takeIf { it.isNotEmpty() },
                origin = "app_hook",
            )
        }
        val output = runCatching { device.executeShellCommand("dumpsys media.drm") }
            .getOrNull()
            ?: return null
        if (output.isBlank()) return null
        // The dump is system-wide. Only attribute an active session when the
        // target package is named in it; another app's DRM session must not
        // make this piece look protected.
        val target = lastLaunchedPackage.ifEmpty { device.currentPackageName ?: "" }
        if (target.isEmpty() || !output.contains(target, ignoreCase = true)) return null
        val provider = when {
            output.contains("Widevine", ignoreCase = true) -> "Widevine"
            output.contains("PlayReady", ignoreCase = true) -> "PlayReady"
            output.contains("ClearKey", ignoreCase = true) -> "ClearKey"
            else -> return null
        }
        val active = REGEX_DRM_SESSIONS.find(output)?.groupValues?.get(1)?.toIntOrNull()?.let { it > 0 }
            ?: false
        if (!active) return null
        return DrmSnapshot(enabled = true, provider = provider)
    }

    /**
     * Latest debug-only in-app hook snapshot. The SDK logs one JSON object per
     * update under `ImpresarioHook`; shell identity can read logcat without
     * touching the app's private files. A snapshot from another package or
     * older than a minute is ignored.
     */
    private fun hookSnapshot(): JSONObject? {
        val output = runCatching {
            device.executeShellCommand("logcat -d -t 400 -s ImpresarioHook:I")
        }.getOrNull() ?: return null
        if (output.isBlank()) return null
        val marker = "IMPRESARIO_HOOK "
        val line = output.lineSequence().lastOrNull { it.contains(marker) } ?: return null
        val json = runCatching {
            JSONObject(line.substringAfter(marker).trim())
        }.getOrNull() ?: return null
        val reportedPackage = json.optString("package")
        val target = lastLaunchedPackage.ifEmpty { device.currentPackageName ?: "" }
        if (target.isEmpty() || reportedPackage != target) {
            return null
        }
        val reportedAt = json.optLong("reportedAtUnixMs")
        if (reportedAt > 0 && System.currentTimeMillis() - reportedAt > 60_000) {
            return null
        }
        return json
    }

    override fun regionCoverage(png: ByteArray, rect: Rect): Double? {
        if (png.isEmpty() || rect.isEmpty) return null
        val bitmap = BitmapFactory.decodeByteArray(png, 0, png.size) ?: return null
        try {
            val width = bitmap.width
            val height = bitmap.height
            if (width <= 0 || height <= 0) return null
            val screenWidth = device.displayWidth.coerceAtLeast(1)
            val screenHeight = device.displayHeight.coerceAtLeast(1)
            val scaleX = width.toDouble() / screenWidth
            val scaleY = height.toDouble() / screenHeight
            val left = (rect.left * scaleX).toInt().coerceIn(0, width - 1)
            val top = (rect.top * scaleY).toInt().coerceIn(0, height - 1)
            val right = (rect.right * scaleX).toInt().coerceIn(left + 1, width)
            val bottom = (rect.bottom * scaleY).toInt().coerceIn(top + 1, height)
            val step = maxOf(1, minOf(right - left, bottom - top) / 200)
            val samples = ArrayList<Double>(256)
            var y = top
            while (y < bottom) {
                var x = left
                while (x < right) {
                    val pixel = bitmap.getPixel(x, y)
                    val alpha = (pixel ushr 24) and 0xFF
                    if (alpha >= 8) {
                        val red = (pixel shr 16) and 0xFF
                        val green = (pixel shr 8) and 0xFF
                        val blue = pixel and 0xFF
                        samples += 0.2126 * red + 0.7152 * green + 0.0722 * blue
                    }
                    x += step
                }
                y += step
            }
            if (samples.isEmpty()) return 0.0
            val mean = samples.average()
            val painted = samples.count { kotlin.math.abs(it - mean) > 6.0 }
            return painted * 100.0 / samples.size
        } finally {
            bitmap.recycle()
        }
    }

    private fun mediaSessionBlocks(output: String): List<String> {
        val blocks = mutableListOf<String>()
        val current = StringBuilder()
        for (line in output.lineSequence()) {
            if (line.contains("package=") && current.isNotEmpty()) {
                blocks += current.toString()
                current.clear()
            }
            if (current.isNotEmpty() || line.contains("package=") || line.contains("PlaybackState")) {
                current.appendLine(line)
            }
        }
        if (current.isNotEmpty()) blocks += current.toString()
        return blocks
    }

    // ------------------------------------------------------------------
    // InspectorDriver (TASK-INSPECTOR 6.3)
    // ------------------------------------------------------------------

    /**
     * Captures the active app window's accessibility tree, normalizes it to the
     * shared role/rect model and prunes it to the viewport. The algorithm
     * mirrors the connector's `prune_nodes` so both platforms produce the same
     * shape for the Web UI and MCP views.
     */
    override fun capture(options: InspectorCaptureOptions): InspectorCapture {
        val pixelRatio = if (options.pixelRatio > 0f) options.pixelRatio else 1f
        val viewportWidth = options.displayWidthPx / pixelRatio
        val viewportHeight = options.displayHeightPx / pixelRatio
        val nodes = ArrayList<InspectorNode>(256)
        val nativeNodes = HashMap<Long, AccessibilityNodeInfo>(256)
        val nativeRects = HashMap<Long, Rect>(256)
        var traversalTruncated = false

        fun walk(node: AccessibilityNodeInfo?, parentId: Long, depth: Int, systemUi: Boolean) {
            if (node == null) return
            if (nodes.size >= INSPECTOR_TRAVERSAL_BUDGET) {
                traversalTruncated = true
                return
            }
            if (depth > options.maxDepth) return
            val id = nodes.size.toLong() + 1
            val pixelRect = AndroidRect()
            try {
                node.getBoundsInScreen(pixelRect)
            } catch (_: Exception) {
                return
            }
            val class_name = node.className?.toString().orEmpty()
            val editable = try {
                node.isEditable
            } catch (_: Exception) {
                false
            }
            val rawText = node.text?.toString().orEmpty()
            val hint = try {
                node.hintText?.toString().orEmpty()
            } catch (_: Exception) {
                ""
            }
            val description = node.contentDescription?.toString().orEmpty()
            val capped = capText(rawText, INSPECTOR_TEXT_LIMIT)
            val visibleText = if (editable) "" else capped.first
            val value = if (editable) capped.first else ""
            val checked = try {
                if (node.isCheckable) node.isChecked else null
            } catch (_: Exception) {
                null
            }
            val scrollable = try {
                node.isScrollable
            } catch (_: Exception) {
                false
            }
            val actions = try {
                node.actionList
            } catch (_: Exception) {
                emptyList()
            }
            val canScrollBackward = actions.any {
                it.id == AccessibilityNodeInfo.ACTION_SCROLL_BACKWARD
            }
            val canScrollForward = actions.any {
                it.id == AccessibilityNodeInfo.ACTION_SCROLL_FORWARD
            }
            val resourceId = node.viewIdResourceName.orEmpty()
            val builder = InspectorNode.newBuilder()
                .setId(id)
                .setParentId(parentId)
                .setRole(roleFor(class_name, editable))
                .setName(capText(description.ifEmpty { if (editable) hint else rawText }, INSPECTOR_NAME_LIMIT).first)
                .setText(visibleText)
                .setValue(value)
                .setLabel(hint)
                .setDescription(description)
                .setPlaceholder(hint)
                .setTestId(resourceId.substringAfter("id/", resourceId))
                .setRawId(resourceId)
                .setBounds(
                    InspectorRect.newBuilder()
                        .setX(pixelRect.left / pixelRatio)
                        .setY(pixelRect.top / pixelRatio)
                        .setW(pixelRect.width() / pixelRatio)
                        .setH(pixelRect.height() / pixelRatio)
                        .build(),
                )
                .setFocused(node.isFocused)
                .setEnabled(node.isEnabled)
                .setSelected(node.isSelected)
                .setReadonly(editable && !node.isEditable)
                .setHidden(!node.isVisibleToUser)
                .setSecure(node.isPassword)
                .setSystemUi(systemUi)
                .setScrollable(scrollable)
                .setCanScrollBackward(canScrollBackward)
                .setCanScrollForward(canScrollForward)
                .setScrollAxis(
                    when {
                        !scrollable -> ""
                        pixelRect.height() >= pixelRect.width() -> "vertical"
                        else -> "horizontal"
                    },
                )
                .setVisibleFraction(visibleFraction(pixelRect, options.displayWidthPx, options.displayHeightPx))
                .setDepth(depth)
                .setTextTruncated(capped.second)
            if (checked != null) builder.setChecked(checked)
            if (options.includeRaw) {
                builder.setRaw(
                    JSONObject()
                        .put("class", class_name)
                        .put("package", node.packageName?.toString().orEmpty())
                        .put("resource_id", resourceId)
                        .put("clickable", node.isClickable)
                        .put("long_clickable", node.isLongClickable)
                        .put("focusable", node.isFocusable)
                        .put("checkable", node.isCheckable)
                        .put("checked", checked)
                        .put("bounds", pixelRect.toString())
                        .toString(),
                )
            }
            nodes += builder.build()
            nativeNodes[id] = node
            // Input APIs take device pixels; the wire rects stay logical.
            nativeRects[id] = Rect(pixelRect.left, pixelRect.top, pixelRect.right, pixelRect.bottom)
            val childCount = try {
                node.childCount
            } catch (_: Exception) {
                0
            }
            for (index in 0 until childCount) {
                val child = try {
                    node.getChild(index)
                } catch (_: Exception) {
                    null
                }
                walk(child, id, depth + 1, systemUi)
            }
        }

        for (root in inspectorRoots(options)) {
            walk(root.first, 0, 0, root.second)
        }

        val pruned = pruneInspectorNodes(nodes, options, viewportWidth, viewportHeight)
        val keptIds = pruned.nodes.mapTo(HashSet()) { it.id }
        inspectorNodes = nativeNodes.filterKeys { it in keptIds }
        inspectorRects = nativeRects.filterKeys { it in keptIds }
        inspectorPixelRatio = pixelRatio
        val degraded = ArrayList<String>()
        if (nodes.isEmpty()) {
            degraded += "UNAVAILABLE: the active window has no accessibility root"
        }
        if (traversalTruncated) {
            degraded += "PARTIAL: the accessibility tree hit the traversal budget; some nodes may be missing"
        }
        if (pruned.capped) {
            degraded += "PARTIAL: max_nodes truncated the snapshot"
        }
        return InspectorCapture(
            nodes = pruned.nodes,
            focusedId = pruned.focusedId,
            truncated = traversalTruncated || pruned.capped,
            degraded = degraded,
            screenshotPng = if (options.includeScreenshot) inspectorScreenshot() else null,
        )
    }

    override fun tap(nodeId: Long, x: Float, y: Float) {
        val node = if (nodeId != 0L) inspectorNodes[nodeId] else null
        // Accessibility ACTION_CLICK is the same path a screen reader uses and
        // is reliable on windows that swallow synthetic touch streams. Try the
        // node itself, then its clickable ancestors: rows are often a
        // non-clickable container around a clickable child.
        val clickable = clickableAncestor(node)
        if (clickable != null && clickable.performAction(AccessibilityNodeInfo.ACTION_CLICK)) return
        val rect = if (nodeId != 0L) inspectorRects[nodeId] else null
        val point = rect?.let { it.centerX to it.centerY }
            ?: logicalPoint(x, y)
        if (!device.click(point.first, point.second)) {
            throw AgentException.notInteractable("the tap did not reach the screen")
        }
    }

    private fun clickableAncestor(node: AccessibilityNodeInfo?): AccessibilityNodeInfo? {
        var current = node
        var guard = 0
        while (current != null && guard < INSPECTOR_MAX_ANCESTOR_WALK) {
            if (current.isClickable) return current
            current = try {
                current.parent
            } catch (_: Exception) {
                null
            }
            guard++
        }
        return null
    }

    override fun scroll(direction: Direction, count: Int, nodeId: Long, x: Float, y: Float) {
        val node = if (nodeId != 0L) inspectorNodes[nodeId] else null
        val rect = if (nodeId != 0L) inspectorRects[nodeId] else null
        repeat(count.coerceAtLeast(1)) {
            val scrolled = node?.let { scrollNode(it, direction) } ?: false
            if (!scrolled) {
                val area = rect?.let { AndroidRect(it.left, it.top, it.right, it.bottom) }
                    ?: AndroidRect(0, 0, device.displayWidth, device.displayHeight)
                val path = scrollPathIn(direction, area)
                device.swipe(path.startX, path.startY, path.endX, path.endY, SWIPE_STEPS)
            }
        }
    }

    override fun pressKey(code: Int) {
        device.pressKeyCode(code)
    }

    override fun type(nodeId: Long, text: String) {
        val node = if (nodeId != 0L) inspectorNodes[nodeId] else null
        if (node == null) {
            inputText(text)
            return
        }
        node.performAction(AccessibilityNodeInfo.ACTION_FOCUS)
        if (text.isEmpty()) return
        // The connector appends at the caret (`insert_text`); ACTION_SET_TEXT
        // replaces, so append to the node's current text. An empty EditText
        // reports its hint as text, which must not become field content.
        val raw = node.text?.toString().orEmpty()
        val hint = try {
            node.hintText?.toString().orEmpty()
        } catch (_: Exception) {
            ""
        }
        val current = if (raw.isNotEmpty() && raw == hint) "" else raw
        val arguments = Bundle().apply {
            putCharSequence(
                AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE,
                current + text,
            )
        }
        if (!node.performAction(AccessibilityNodeInfo.ACTION_SET_TEXT, arguments)) {
            inputText(text)
        }
    }

    override fun swipe(x1: Float, y1: Float, x2: Float, y2: Float, durationMs: Long) {
        val start = logicalPoint(x1, y1)
        val end = logicalPoint(x2, y2)
        val steps = if (durationMs > 0) {
            (durationMs / INSPECTOR_SWIPE_STEP_MS).coerceIn(2, 120).toInt()
        } else {
            SWIPE_STEPS
        }
        device.swipe(start.first, start.second, end.first, end.second, steps)
    }

    /** Roots to traverse: the app window, plus system windows when opted in. */
    private fun inspectorRoots(options: InspectorCaptureOptions): List<Pair<AccessibilityNodeInfo, Boolean>> {
        val roots = ArrayList<Pair<AccessibilityNodeInfo, Boolean>>(2)
        val windows = try {
            instrumentation.uiAutomation.windows
        } catch (_: Exception) {
            emptyList()
        }
        val appWindow = windows.firstOrNull { window ->
            window.type == AccessibilityWindowInfo.TYPE_APPLICATION &&
                window.root?.packageName?.toString() == options.appId
        } ?: windows.firstOrNull { it.type == AccessibilityWindowInfo.TYPE_APPLICATION }
        appWindow?.root?.let { roots += it to false }
        if (options.includeSystemUi) {
            for (window in windows) {
                if (window.type != AccessibilityWindowInfo.TYPE_SYSTEM) continue
                window.root?.let { roots += it to true }
            }
        }
        if (roots.isEmpty()) {
            try {
                instrumentation.uiAutomation.rootInActiveWindow?.let { roots += it to false }
            } catch (_: Exception) {
                // A vanished window leaves the capture empty; the service
                // reports the degraded reason.
            }
        }
        return roots
    }

    private data class PrunedCapture(
        val nodes: List<InspectorNode>,
        val focusedId: Long,
        val capped: Boolean,
    )

    private fun pruneInspectorNodes(
        nodes: List<InspectorNode>,
        options: InspectorCaptureOptions,
        viewportWidth: Float,
        viewportHeight: Float,
    ): PrunedCapture {
        if (nodes.isEmpty()) return PrunedCapture(emptyList(), 0L, false)
        val mutable = nodes.map { it.toBuilder() }.toMutableList()
        val byId = HashMap<Long, Int>(mutable.size)
        mutable.forEachIndexed { index, node -> byId[node.id] = index }
        val keep = BooleanArray(mutable.size)
        val minFraction = options.minVisibleFraction.coerceIn(0f, 1f)
        for ((index, node) in mutable.withIndex()) {
            if (node.systemUi && !options.includeSystemUi) continue
            if (node.depth > options.maxDepth) continue
            if (node.hidden && !node.focused) continue
            val bounds = node.bounds
            if (bounds.w <= 0f || bounds.h <= 0f) continue
            if (node.focused) {
                keep[index] = true
                continue
            }
            val fraction = node.visibleFraction
            if (fraction > 0f && (minFraction <= 0f || fraction >= minFraction)) {
                keep[index] = true
            }
        }
        // Keep the ancestor chain of every retained node so parent ids resolve.
        for (index in mutable.indices) {
            if (!keep[index]) continue
            var parent = mutable[index].parentId
            var guard = 0
            while (parent != 0L && guard < INSPECTOR_MAX_ANCESTOR_WALK) {
                val parentIndex = byId[parent] ?: break
                if (keep[parentIndex]) break
                keep[parentIndex] = true
                parent = mutable[parentIndex].parentId
                guard++
            }
        }
        var keptCount = keep.count { it }
        var capped = false
        val maxNodes = options.maxNodes.coerceAtLeast(1)
        if (keptCount > maxNodes) {
            capped = true
            val priority = BooleanArray(mutable.size)
            for ((index, node) in mutable.withIndex()) {
                if (node.focused) markAncestors(index, mutable, byId, priority)
            }
            for (index in mutable.indices.reversed()) {
                if (keptCount <= maxNodes) break
                if (keep[index] && !priority[index]) {
                    keep[index] = false
                    keptCount--
                }
            }
            for (index in mutable.indices.reversed()) {
                if (keptCount <= maxNodes) break
                if (keep[index] && !mutable[index].focused) {
                    keep[index] = false
                    keptCount--
                }
            }
        }
        val hiddenCounts = HashMap<Long, Int>()
        for ((index, node) in mutable.withIndex()) {
            if (keep[index]) continue
            val parent = node.parentId
            if (parent != 0L && byId[parent]?.let { keep[it] } == true) {
                hiddenCounts[parent] = (hiddenCounts[parent] ?: 0) + 1
            }
        }
        val out = ArrayList<InspectorNode>(keptCount)
        for ((index, node) in mutable.withIndex()) {
            if (!keep[index]) continue
            val builder = node
            builder.setChildrenHiddenCount(
                builder.childrenHiddenCount + (hiddenCounts[node.id] ?: 0),
            )
            if (builder.parentId != 0L && byId[builder.parentId]?.let { keep[it] } != true) {
                builder.setParentId(0L)
            }
            builder.setViewportBounds(clipInspectorRect(node.bounds, viewportWidth, viewportHeight))
            out += builder.build()
        }
        return PrunedCapture(
            nodes = out,
            focusedId = out.firstOrNull { it.focused }?.id ?: 0L,
            capped = capped,
        )
    }

    private fun markAncestors(
        index: Int,
        nodes: List<InspectorNode.Builder>,
        byId: Map<Long, Int>,
        priority: BooleanArray,
    ) {
        var current = index
        var guard = 0
        while (guard < INSPECTOR_MAX_ANCESTOR_WALK) {
            priority[current] = true
            val parent = nodes[current].parentId
            if (parent == 0L) break
            current = byId[parent] ?: break
            guard++
        }
    }

    private fun clipInspectorRect(rect: InspectorRect, width: Float, height: Float): InspectorRect {
        val left = rect.x.coerceIn(0f, width)
        val top = rect.y.coerceIn(0f, height)
        val right = (rect.x + rect.w).coerceIn(left, width)
        val bottom = (rect.y + rect.h).coerceIn(top, height)
        return InspectorRect.newBuilder()
            .setX(left)
            .setY(top)
            .setW(right - left)
            .setH(bottom - top)
            .build()
    }

    private fun visibleFraction(rect: AndroidRect, displayWidth: Int, displayHeight: Int): Float {
        if (rect.width() <= 0 || rect.height() <= 0) return 0f
        val left = rect.left.coerceAtLeast(0)
        val top = rect.top.coerceAtLeast(0)
        val right = rect.right.coerceAtMost(displayWidth)
        val bottom = rect.bottom.coerceAtMost(displayHeight)
        if (right <= left || bottom <= top) return 0f
        val visible = (right - left).toLong() * (bottom - top).toLong()
        val total = rect.width().toLong() * rect.height().toLong()
        return (visible.toDouble() / total.toDouble()).toFloat().coerceIn(0f, 1f)
    }

    private fun scrollNode(node: AccessibilityNodeInfo, direction: Direction): Boolean {
        val action = when (direction) {
            Direction.DOWN, Direction.RIGHT -> AccessibilityNodeInfo.ACTION_SCROLL_FORWARD
            Direction.UP, Direction.LEFT -> AccessibilityNodeInfo.ACTION_SCROLL_BACKWARD
        }
        return try {
            node.performAction(action)
        } catch (_: Exception) {
            false
        }
    }

    private fun scrollPathIn(direction: Direction, area: AndroidRect): SwipePath {
        val centerX = area.centerX()
        val centerY = area.centerY()
        val travelX = (area.width() * INSPECTOR_SCROLL_FRACTION / 2f).toInt()
        val travelY = (area.height() * INSPECTOR_SCROLL_FRACTION / 2f).toInt()
        return when (direction) {
            Direction.UP -> SwipePath(centerX, centerY + travelY, centerX, centerY - travelY)
            Direction.DOWN -> SwipePath(centerX, centerY - travelY, centerX, centerY + travelY)
            Direction.LEFT -> SwipePath(centerX + travelX, centerY, centerX - travelX, centerY)
            Direction.RIGHT -> SwipePath(centerX - travelX, centerY, centerX + travelX, centerY)
        }
    }

    private fun logicalPoint(x: Float, y: Float): Pair<Int, Int> {
        val ratio = if (inspectorPixelRatio > 0f) inspectorPixelRatio else 1f
        return (x * ratio).roundToInt() to (y * ratio).roundToInt()
    }

    override fun screenshot(scalePercent: Int): ByteArray? = inspectorScreenshot(scalePercent)

    private fun inspectorScreenshot(scalePercent: Int = 100): ByteArray? {
        val bitmap = try {
            device.takeScreenshot()
        } catch (_: Exception) {
            return null
        } ?: return null
        val scale = scalePercent.coerceIn(1, 100)
        val output = if (scale < 100) {
            try {
                Bitmap.createScaledBitmap(
                    bitmap,
                    (bitmap.width * scale / 100).coerceAtLeast(1),
                    (bitmap.height * scale / 100).coerceAtLeast(1),
                    true,
                )
            } catch (_: Exception) {
                bitmap
            }
        } else {
            bitmap
        }
        return try {
            val out = ByteArrayOutputStream()
            output.compress(Bitmap.CompressFormat.PNG, 100, out)
            out.toByteArray()
        } catch (_: Exception) {
            null
        } finally {
            if (output !== bitmap) output.recycle()
            bitmap.recycle()
        }
    }

    private fun roleFor(class_name: String, editable: Boolean): String {
        val lower = class_name.lowercase()
        return when {
            lower.contains("imagebutton") -> "button"
            lower.contains("button") -> "button"
            lower.contains("edittext") || lower.contains("textfield") -> "textbox"
            editable -> "textbox"
            lower.contains("checkbox") -> "checkbox"
            lower.contains("switch") || lower.contains("togglebutton") -> "switch"
            lower.contains("radiobutton") -> "radio"
            lower.contains("seekbar") || lower.contains("slider") -> "slider"
            lower.contains("spinner") -> "combobox"
            lower.contains("recyclerview") || lower.contains("listview") ||
                lower.contains("gridview") -> "list"
            lower.contains("image") -> "image"
            lower.contains("textview") -> "text"
            lower.contains("progressbar") -> "progressbar"
            lower.contains("tab") -> "tab"
            else -> "generic"
        }
    }

    private fun capText(value: String, limit: Int): Pair<String, Boolean> =
        if (value.length > limit) value.substring(0, limit) to true else value to false

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

    private fun screenSwipe(path: SwipePath) {
        device.swipe(path.startX, path.startY, path.endX, path.endY, SWIPE_STEPS)
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
        const val INSPECTOR_TRAVERSAL_BUDGET = 20_000
        const val INSPECTOR_TEXT_LIMIT = 1_000
        const val INSPECTOR_NAME_LIMIT = 200
        const val INSPECTOR_MAX_ANCESTOR_WALK = 1_024
        const val INSPECTOR_SCROLL_FRACTION = 0.8f
        const val INSPECTOR_SWIPE_STEP_MS = 16L
        val PACKAGE_NAME = Regex("^[A-Za-z0-9_]+(\\.[A-Za-z0-9_]+)+$")
        val REGEX_PLAYBACK_STATE = Regex("PlaybackState \\{[^}]*state=(-?\\d+)")
        val REGEX_SPEED = Regex("speed=(-?\\d+(?:\\.\\d+)?)")
        val REGEX_POSITION = Regex("position=(-?\\d+)")
        val REGEX_UPDATED = Regex("updated=(-?\\d+)")
        val REGEX_DURATION = Regex("DURATION=(-?\\d+)", RegexOption.IGNORE_CASE)
        val REGEX_MEDIA_URI = Regex("MEDIA_URI=([^,\\s}]+)")
        val REGEX_DRM_SESSIONS = Regex("(?:open|active)?\\s*sessions?\\s*[:=]\\s*(\\d+)", RegexOption.IGNORE_CASE)
        val CRASH_TEXTS = listOf(
            "isn't responding",
            "keeps stopping",
            "has stopped",
            "Close app",
        )
    }
}
