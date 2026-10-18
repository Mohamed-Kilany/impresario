package com.impresario.stub

import android.view.View
import android.widget.LinearLayout

/**
 * Shared vertical rhythm for the fixture screens. Without a gap every row
 * butts against the next one and the whole screen reads as one wall of
 * buttons; the stub is a test fixture, but a human still has to read its
 * failure screenshots.
 */
internal const val STACK_GAP_DP = 10

/**
 * Adds `view` under the previous row with the shared gap: full width,
 * wrapping height, [STACK_GAP_DP] of space above. Ids, texts and the
 * top-to-bottom focus order are untouched (docs/DECISIONS.md I-58/I-64).
 */
internal fun LinearLayout.addStacked(view: View): LinearLayout {
    val gap = (STACK_GAP_DP * resources.displayMetrics.density).toInt()
    view.layoutParams = LinearLayout.LayoutParams(
        LinearLayout.LayoutParams.MATCH_PARENT,
        LinearLayout.LayoutParams.WRAP_CONTENT,
    ).apply { topMargin = gap }
    addView(view)
    return this
}
