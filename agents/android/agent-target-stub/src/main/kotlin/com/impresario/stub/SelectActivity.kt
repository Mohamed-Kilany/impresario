package com.impresario.stub

import android.app.AlertDialog
import android.widget.LinearLayout

/** Selection: a dialog trigger and options A/B/C; the status shows the pick. */
class SelectActivity : ScenarioActivity() {
    override fun scenarioTitle(): String = "Selection"

    override fun buildScenario(column: LinearLayout) {
        val options = arrayOf("Option A", "Option B", "Option C")
        val trigger = actionButton(R.id.stub_select, "Choose").apply {
            setOnClickListener {
                AlertDialog.Builder(this@SelectActivity)
                    .setTitle("Pick one")
                    .setItems(options) { _, which -> status.text = "chosen: ${options[which]}" }
                    .show()
            }
        }
        column.addStacked(trigger)
        trigger.requestFocus()
    }
}
