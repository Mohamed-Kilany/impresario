package com.impresario.stub

import android.view.View
import android.widget.LinearLayout
import android.widget.TextView

/** Flags: enabled/disabled buttons and a visible/invisible label. */
class FlagsActivity : ScenarioActivity() {
    override fun scenarioTitle(): String = "Flags"

    override fun buildScenario(column: LinearLayout) {
        val enabled = actionButton(R.id.stub_enabled, "Enabled").apply {
            setOnClickListener { status.text = "status=enabled-clicked" }
        }
        val disabled = actionButton(R.id.stub_disabled, "Disabled").apply {
            isEnabled = false
        }
        column.addStacked(enabled)
        column.addStacked(disabled)
        column.addStacked(
            TextView(this).apply {
                id = R.id.stub_visible
                text = "visible text"
                textSize = 18f
            },
        )
        column.addStacked(
            TextView(this).apply {
                id = R.id.stub_hidden
                text = "hidden text"
                textSize = 18f
                visibility = View.INVISIBLE
            },
        )
        enabled.requestFocus()
    }
}
