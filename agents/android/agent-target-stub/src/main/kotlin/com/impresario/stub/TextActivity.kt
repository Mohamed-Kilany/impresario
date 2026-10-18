package com.impresario.stub

import android.text.Editable
import android.text.TextWatcher
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.TextView

/** Text entry: EditText with an echo label (type/append/clear/backspace). */
class TextActivity : ScenarioActivity() {
    override fun scenarioTitle(): String = "Text entry"

    override fun buildScenario(column: LinearLayout) {
        val echo = TextView(this).apply {
            id = R.id.stub_echo
            text = "echo:"
            textSize = 18f
        }
        val input = EditText(this).apply {
            id = R.id.stub_input
            hint = "type here"
            textSize = 18f
            isSingleLine = true
            setPadding(dp(12), dp(6), dp(12), dp(6))
            addTextChangedListener(object : TextWatcher {
                override fun beforeTextChanged(s: CharSequence?, start: Int, count: Int, after: Int) = Unit
                override fun onTextChanged(s: CharSequence?, start: Int, before: Int, count: Int) = Unit
                override fun afterTextChanged(s: Editable?) {
                    echo.text = "echo:" + (s?.toString() ?: "")
                }
            })
        }
        column.addView(input)
        column.addView(echo)
        input.requestFocus()
    }
}
