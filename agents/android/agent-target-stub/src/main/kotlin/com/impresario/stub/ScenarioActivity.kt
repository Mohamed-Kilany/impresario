package com.impresario.stub

import android.app.Activity
import android.graphics.Typeface
import android.os.Bundle
import android.widget.Button
import android.widget.LinearLayout
import android.widget.TextView

/**
 * Shared scaffold for scenario screens: TV-safe padding, a title (stable id
 * `stub_title` + content description `stub badge`) and a status line the
 * scenario updates so tests can assert what happened.
 */
abstract class ScenarioActivity : Activity() {
    protected lateinit var status: TextView

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val column = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(dp(48), dp(36), dp(48), dp(36))
        }
        column.addView(
            TextView(this).apply {
                id = R.id.stub_title
                text = scenarioTitle()
                textSize = 24f
                setTypeface(typeface, Typeface.BOLD)
                contentDescription = "stub badge"
            },
        )
        buildScenario(column)
        status = TextView(this).apply {
            id = R.id.stub_status
            text = "status=idle"
            textSize = 16f
            alpha = 0.85f
        }
        column.addStacked(status)
        setContentView(column)
    }

    protected abstract fun scenarioTitle(): String

    protected abstract fun buildScenario(column: LinearLayout)

    protected fun dp(value: Int): Int = (value * resources.displayMetrics.density).toInt()

    protected fun actionButton(id: Int, label: String): Button = Button(this).apply {
        this.id = id
        text = label
        textSize = 18f
        minHeight = dp(48)
        setPadding(dp(12), dp(6), dp(12), dp(6))
        // TV themes force button labels to ALL CAPS; `getText()` then reports
        // the uppercase form in the accessibility tree, which breaks exact
        // `text` selectors that work on phones. Keep the fixture text stable
        // across platforms (docs/DECISIONS.md I-64).
        isAllCaps = false
    }
}
