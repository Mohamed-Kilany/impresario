package com.impresario.stub

import android.app.Activity
import android.content.Intent
import android.os.Bundle
import android.widget.Button
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView

/**
 * The stub home screen: one row per test scenario, each opening its own
 * dedicated screen. Every row is a focusable button, so the same APK works
 * under touch and under TV D-pad navigation (the first row takes focus).
 */
class MainActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val density = resources.displayMetrics.density
        fun dp(value: Int): Int = (value * density).toInt()

        val column = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(dp(48), dp(36), dp(48), dp(36))
        }
        column.addView(
            TextView(this).apply {
                id = R.id.stub_title
                text = "Impresario stub"
                textSize = 22f
                contentDescription = "stub badge"
            },
        )
        column.addView(
            TextView(this).apply {
                text = "One screen per test scenario."
                textSize = 15f
                setPadding(0, 0, 0, dp(12))
            },
        )
        var first: Button? = null
        for ((id, label, screen) in SCENARIOS) {
            val row = Button(this).apply {
                this.id = id
                text = label
                textSize = 18f
                setPadding(dp(12), dp(6), dp(12), dp(6))
                // Stable fixture text on every theme (docs/DECISIONS.md I-64).
                isAllCaps = false
                setOnClickListener { startActivity(Intent(this@MainActivity, screen)) }
            }
            if (first == null) first = row
            column.addView(row)
        }
        setContentView(ScrollView(this).apply { addView(column) })
        first?.requestFocus()
    }

    private companion object {
        val SCENARIOS = listOf(
            Triple(R.id.stub_scenario_basic, "Basic controls", BasicActivity::class.java),
            Triple(R.id.stub_scenario_text, "Text entry", TextActivity::class.java),
            Triple(R.id.stub_scenario_scroll, "Scroller", ScrollActivity::class.java),
            Triple(R.id.stub_scenario_select, "Selection", SelectActivity::class.java),
            Triple(R.id.stub_scenario_flags, "Flags", FlagsActivity::class.java),
            Triple(R.id.stub_scenario_keys, "Key log", KeysActivity::class.java),
            Triple(R.id.stub_scenario_info, "Device info", InfoActivity::class.java),
        )
    }
}
