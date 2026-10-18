package com.impresario.agent

import impresario.v1.Impresario.Platform
import impresario.v1.Impresario.Selector
import impresario.v1.Impresario.SelectorStrategy

/** Android display-space rectangle, pixels. */
data class Rect(val left: Int, val top: Int, val right: Int, val bottom: Int) {
    val width: Int get() = right - left
    val height: Int get() = bottom - top
    val centerX: Int get() = (left + right) / 2
    val centerY: Int get() = (top + bottom) / 2
    val area: Long get() = width.toLong() * height.toLong()
    val isEmpty: Boolean get() = width <= 0 || height <= 0

    override fun toString(): String = "[$left,$top][$right,$bottom]"
}

enum class Direction {
    UP, DOWN, LEFT, RIGHT;

    val keyCode: Int
        get() = when (this) {
            UP -> KeyTable.KEYCODE_DPAD_UP
            DOWN -> KeyTable.KEYCODE_DPAD_DOWN
            LEFT -> KeyTable.KEYCODE_DPAD_LEFT
            RIGHT -> KeyTable.KEYCODE_DPAD_RIGHT
        }
}

/** A selector as understood by the Android side (spec 5.3). */
data class SelectorSpec(
    val strategy: SelectorStrategy,
    val value: String,
    val index: Int,
) {
    /** Human-readable form for error details, e.g. `androidId:com.example:id/ok`. */
    fun describe(): String = "${strategyName(strategy)}:$value" +
        if (index > 0) "#$index" else ""

    companion object {
        private val supported = setOf(
            SelectorStrategy.ANDROID_ID,
            SelectorStrategy.ANDROID_DESC,
            SelectorStrategy.ANDROID_CLASS,
            SelectorStrategy.TEXT,
            SelectorStrategy.TEXT_CONTAINS,
        )

        fun strategyName(strategy: SelectorStrategy): String = when (strategy) {
            SelectorStrategy.ANDROID_ID -> "androidId"
            SelectorStrategy.ANDROID_DESC -> "androidDesc"
            SelectorStrategy.ANDROID_CLASS -> "androidClass"
            SelectorStrategy.TEXT -> "text"
            SelectorStrategy.TEXT_CONTAINS -> "textContains"
            SelectorStrategy.CSS -> "css"
            SelectorStrategy.XPATH -> "xpath"
            SelectorStrategy.ACCESSIBILITY_ID -> "accessibilityId"
            SelectorStrategy.LABEL -> "label"
            SelectorStrategy.LABEL_CONTAINS -> "labelContains"
            SelectorStrategy.PREDICATE -> "predicate"
            SelectorStrategy.IOS_TYPE -> "iosType"
            else -> "unspecified"
        }

        /**
         * Validates the proto selector for Android platforms.
         * Unspecified enum → PROTOCOL (protocol rule 6); a strategy that does not
         * apply to Android → INVALID_SELECTOR.
         */
        fun from(selector: Selector?): SelectorSpec {
            if (selector == null) {
                throw AgentException.protocol("the command carries no selector")
            }
            val strategy = selector.strategy
            if (strategy == SelectorStrategy.SELECTOR_STRATEGY_UNSPECIFIED) {
                throw AgentException.protocol("the selector strategy is unspecified")
            }
            if (strategy !in supported) {
                throw AgentException.invalidSelector(
                    "selector strategy '${strategyName(strategy)}' does not apply to android",
                )
            }
            return SelectorSpec(strategy, selector.value, selector.index)
        }
    }
}

/** An opaque driver handle for a matched element (spec 11.3). */
interface ElementHandle

/** Everything the dispatcher needs to know about the display (spec 11.3). */
data class DeviceInfoData(
    val model: String,
    val osName: String,
    val osVersion: String,
    val screenWidthPx: Int,
    val screenHeightPx: Int,
    val pixelRatio: Float,
    val runtime: String,
)

/**
 * Everything that touches UiAutomator lives behind this interface so the
 * dispatcher is JVM-testable with a fake (spec 11.3, verbatim).
 */
interface UiDriver {
    fun find(sel: SelectorSpec, timeoutMs: Long): List<ElementHandle>
    fun count(sel: SelectorSpec): Int
    fun click(e: ElementHandle)
    fun longClick(e: ElementHandle, ms: Long)
    fun setText(e: ElementHandle, text: String)
    fun text(e: ElementHandle): String
    fun clear(e: ElementHandle)
    fun isVisible(e: ElementHandle): Boolean
    fun isEnabled(e: ElementHandle): Boolean
    fun isFocused(e: ElementHandle): Boolean
    fun bounds(e: ElementHandle): Rect
    fun focusedBounds(): Rect?
    fun pressKey(code: Int, repeat: Int, intervalMs: Long)
    fun pressBack()

    /**
     * Runs [steps] scroll steps inside `container` (the primary scrollable of
     * the screen when `null`). Returns `false` when no step moved the
     * content - the container already sits at that end - and `true` when it
     * moved or the driver cannot observe movement (docs/DECISIONS.md I-75).
     */
    fun scroll(container: ElementHandle?, dir: Direction, steps: Int): Boolean
    fun swipe(e: ElementHandle?, dir: Direction)
    fun screenshotPng(): ByteArray
    fun launch(appId: String, tv: Boolean)
    fun terminate(appId: String)
    fun clearData(appId: String)
    fun deviceInfo(): DeviceInfoData
}

/**
 * Optional driver capabilities that only the real UiAutomator driver can
 * provide (they need `UiDevice.executeShellCommand` or the accessibility
 * service). Kept out of [UiDriver] so the spec interface stays exact; the
 * dispatcher degrades gracefully when the implementation does not offer them.
 */
interface DriverExtras {
    /** `input text` fallback for the setText mismatch path (spec 11.4). */
    fun inputText(text: String): Boolean = false

    /** Timed key hold for the TV long-press path (spec 11.4, I-66). */
    fun longPressKey(keyCode: Int, durationMs: Long): Boolean = false

    /** Crash dialog visible or the target process gone (spec 11.4). */
    fun targetCrashed(appId: String): Boolean = false

    /** `app background`: sends the app under test to the background. */
    fun putInBackground(appId: String): Boolean = false
}

/** Handshake options and knobs handed to the dispatcher. */
data class ExecutorOptions(
    val platform: Platform,
    val appId: String,
    val pollIntervalMs: Long = 100,
    val scrollMaxAttempts: Int = 10,
    val focusMaxSteps: Int = 50,
    val clearAppDataOnRestart: Boolean = false,
) {
    val isTv: Boolean get() = platform == Platform.ANDROID_TV

    companion object {
        fun fromHandshake(
            platform: Platform,
            appId: String,
            options: Map<String, String>,
        ): ExecutorOptions {
            fun int(key: String, fallback: Int): Int =
                options[key]?.toIntOrNull()?.takeIf { it > 0 } ?: fallback
            return ExecutorOptions(
                platform = platform,
                appId = appId,
                pollIntervalMs = int("poll_interval_ms", 100).toLong(),
                scrollMaxAttempts = int("scroll_max_attempts", 10),
                focusMaxSteps = int("focus_max_steps", 50),
                clearAppDataOnRestart = options["clear_app_data"]?.toBooleanStrictOrNull() ?: false,
            )
        }
    }
}
