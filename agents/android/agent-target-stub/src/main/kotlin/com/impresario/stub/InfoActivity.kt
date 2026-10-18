package com.impresario.stub

import android.app.UiModeManager
import android.content.Context
import android.content.res.Configuration
import android.os.Build
import android.widget.LinearLayout
import android.widget.TextView

/** Device info: what the app sees about itself (checks/textContains target). */
class InfoActivity : ScenarioActivity() {
    override fun scenarioTitle(): String = "Device info"

    override fun buildScenario(column: LinearLayout) {
        val uiMode = getSystemService(Context.UI_MODE_SERVICE) as UiModeManager
        val text = buildString {
            append("pkg=").append(packageName).append("; ")
            append("sdk=").append(Build.VERSION.SDK_INT).append("; ")
            append("model=").append(Build.MODEL).append("; ")
            append("tv=").append(uiMode.currentModeType == Configuration.UI_MODE_TYPE_TELEVISION)
        }
        column.addView(
            TextView(this).apply {
                id = R.id.stub_info
                this.text = text
                textSize = 16f
            },
        )
    }
}
