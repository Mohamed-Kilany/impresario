package com.impresario.stub

import android.view.KeyEvent
import android.widget.LinearLayout
import android.widget.TextView

/**
 * Key log: three focusable buttons to move focus across with the D-pad and a
 * label that records the last key (`key=DPAD_DOWN`, `key=MEDIA_PLAY_PAUSE`,
 * `key=PROG_RED`, ...) so TV key handling is observable.
 */
class KeysActivity : ScenarioActivity() {
    private lateinit var log: TextView

    override fun scenarioTitle(): String = "Key log"

    override fun buildScenario(column: LinearLayout) {
        log = TextView(this).apply {
            id = R.id.stub_key_log
            text = "key=none"
            textSize = 18f
        }
        val alpha = actionButton(R.id.stub_key_a, "Alpha").apply {
            setOnClickListener { log.text = "clicked=Alpha" }
        }
        val bravo = actionButton(R.id.stub_key_b, "Bravo").apply {
            setOnClickListener { log.text = "clicked=Bravo" }
        }
        val charlie = actionButton(R.id.stub_key_c, "Charlie").apply {
            setOnClickListener { log.text = "clicked=Charlie" }
        }
        column.addStacked(alpha)
        column.addStacked(bravo)
        column.addStacked(charlie)
        column.addStacked(log)
        alpha.requestFocus()
    }

    override fun onKeyDown(keyCode: Int, event: KeyEvent): Boolean {
        if (::log.isInitialized) {
            log.text = "key=" + KeyEvent.keyCodeToString(keyCode).removePrefix("KEYCODE_")
        }
        return super.onKeyDown(keyCode, event)
    }
}
