package com.impresario.stub

import android.widget.LinearLayout
import android.widget.TextView

/** Click + long press with a tap counter and a status line. */
class BasicActivity : ScenarioActivity() {
    private var taps = 0
    private lateinit var counter: TextView

    override fun scenarioTitle(): String = "Basic controls"

    override fun buildScenario(column: LinearLayout) {
        counter = TextView(this).apply {
            id = R.id.stub_counter
            text = "taps=0"
            textSize = 18f
        }
        val button = actionButton(R.id.stub_button, "Tap me").apply {
            setOnClickListener {
                taps += 1
                counter.text = "taps=$taps"
                status.text = "status=clicked"
            }
            setOnLongClickListener {
                status.text = "status=long-pressed"
                true
            }
        }
        column.addView(button)
        column.addView(counter)
        button.requestFocus()
    }
}
