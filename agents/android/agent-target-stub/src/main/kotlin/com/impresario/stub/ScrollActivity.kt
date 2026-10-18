package com.impresario.stub

import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView

/** Scrollable list with 30 `row N` entries (scroll/scrollto/swipe target). */
class ScrollActivity : ScenarioActivity() {
    override fun scenarioTitle(): String = "Scroller"

    override fun buildScenario(column: LinearLayout) {
        val list = LinearLayout(this).apply {
            id = R.id.stub_list
            orientation = LinearLayout.VERTICAL
        }
        for (index in 0 until 30) {
            list.addView(
                TextView(this).apply {
                    text = "row $index"
                    textSize = 18f
                    setPadding(0, dp(12), 0, dp(12))
                },
            )
        }
        val scroller = ScrollView(this).apply {
            id = R.id.stub_scroller
            isFocusable = true
            addView(list)
        }
        column.addView(
            scroller,
            LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT,
                0,
                1f,
            ).apply { topMargin = (STACK_GAP_DP * resources.displayMetrics.density).toInt() },
        )
        scroller.requestFocus()
    }
}
