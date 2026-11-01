package com.impresario.stub

import android.content.Context
import android.content.pm.PackageManager
import android.content.res.ColorStateList
import android.content.res.Configuration
import android.graphics.Color
import android.graphics.drawable.GradientDrawable
import android.graphics.drawable.StateListDrawable
import android.view.Gravity
import android.view.View
import android.widget.Button
import android.widget.LinearLayout

/**
 * Shared vertical rhythm for the fixture screens. Without a gap every row
 * butts against the next one and the whole screen reads as one wall of
 * buttons; the stub is a test fixture, but a human still has to read its
 * failure screenshots.
 *
 * TV screens reported the 10 dp phone rhythm as too airy, so the television
 * profile uses a tighter gap. Phone/tablet spacing stays exactly as verified
 * on the A51 (docs/DECISIONS.md I-81/I-109).
 */
internal const val STACK_GAP_DP = 10
internal const val TV_STACK_GAP_DP = 6

/** True on Android TV (leanback) devices, including TCL/Shield. */
private fun LinearLayout.isTelevision(): Boolean {
    val uiMode = resources.configuration.uiMode and Configuration.UI_MODE_TYPE_MASK
    return uiMode == Configuration.UI_MODE_TYPE_TELEVISION ||
        context.packageManager.hasSystemFeature(PackageManager.FEATURE_LEANBACK)
}

/**
 * Adds `view` under the previous row with the shared gap: full width,
 * wrapping height, [STACK_GAP_DP] (or [TV_STACK_GAP_DP] on television) of
 * space above. Ids, texts and the top-to-bottom focus order are untouched
 * (docs/DECISIONS.md I-58/I-64).
 */
internal fun LinearLayout.addStacked(view: View): LinearLayout {
    val gapDp = if (isTelevision()) TV_STACK_GAP_DP else STACK_GAP_DP
    val gap = (gapDp * resources.displayMetrics.density).toInt()
    view.layoutParams = LinearLayout.LayoutParams(
        LinearLayout.LayoutParams.MATCH_PARENT,
        LinearLayout.LayoutParams.WRAP_CONTENT,
    ).apply { topMargin = gap }
    addView(view)
    return this
}

private val STUB_FOCUS_BACKGROUND = 0xFFFFD54F.toInt()
private val STUB_FOCUS_FOREGROUND = 0xFF111111.toInt()
private val STUB_BUTTON_BACKGROUND = 0xFF3A3A3A.toInt()
private val STUB_BUTTON_BORDER = 0xFF6A6A6A.toInt()
private val STUB_PANEL_BACKGROUND = 0xFF1C1C1C.toInt()
private val STUB_PANEL_BORDER = 0xFF6A6A6A.toInt()

internal fun Context.dp(value: Int): Int = (value * resources.displayMetrics.density).toInt()

private fun Context.roundedBox(
    fill: Int,
    stroke: Int,
    strokeWidthDp: Int,
    radiusDp: Int = 8,
): GradientDrawable = GradientDrawable().apply {
    shape = GradientDrawable.RECTANGLE
    setColor(fill)
    setStroke(dp(strokeWidthDp), stroke)
    cornerRadius = dp(radiusDp).toFloat()
}

/**
 * High-contrast focus ring for every D-pad target. The platform theme's
 * focused/unfocused button colours differ by only a few RGB steps on the
 * reference TV, which is invisible in a screenshot and at couch distance.
 */
internal fun Context.stubFocusBackground(): StateListDrawable = StateListDrawable().apply {
    addState(
        intArrayOf(android.R.attr.state_focused),
        roundedBox(STUB_FOCUS_BACKGROUND, Color.WHITE, 3),
    )
    addState(
        intArrayOf(-android.R.attr.state_enabled),
        roundedBox(0xFF1A1A1A.toInt(), 0xFF333333.toInt(), 2),
    )
    addState(intArrayOf(), roundedBox(STUB_BUTTON_BACKGROUND, STUB_BUTTON_BORDER, 2))
}

/** Darker fill for text fields and scroll panels; only the border changes. */
internal fun Context.stubPanelBackground(): StateListDrawable = StateListDrawable().apply {
    addState(
        intArrayOf(android.R.attr.state_focused),
        roundedBox(STUB_PANEL_BACKGROUND, STUB_FOCUS_BACKGROUND, 4),
    )
    addState(intArrayOf(), roundedBox(STUB_PANEL_BACKGROUND, STUB_PANEL_BORDER, 2))
}

/** Shared focus-first button used by the home list and every scenario row. */
internal fun Context.stubButton(id: Int, label: String): Button = Button(this).apply {
    this.id = id
    text = label
    textSize = 18f
    minHeight = dp(48)
    setPadding(dp(12), dp(6), dp(12), dp(6))
    gravity = Gravity.CENTER
    // TV themes force labels to ALL CAPS; exact `text` selectors rely on the
    // fixture copy staying stable across phone and TV (I-64).
    isAllCaps = false
    isFocusable = true
    isFocusableInTouchMode = true
    backgroundTintList = null
    background = stubFocusBackground()
    setTextColor(
        ColorStateList(
            arrayOf(
                intArrayOf(android.R.attr.state_focused),
                intArrayOf(-android.R.attr.state_enabled),
                intArrayOf(),
            ),
            intArrayOf(STUB_FOCUS_FOREGROUND, 0xFF777777.toInt(), Color.WHITE),
        ),
    )
    stateListAnimator = null
    elevation = 0f
}
