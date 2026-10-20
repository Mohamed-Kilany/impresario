package com.impresario.stub

import android.content.pm.PackageManager
import android.content.res.Configuration
import android.view.View
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
