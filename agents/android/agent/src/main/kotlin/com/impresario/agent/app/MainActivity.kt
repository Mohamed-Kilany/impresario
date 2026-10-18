package com.impresario.agent.app

import android.app.Activity
import android.os.Build
import android.os.Bundle
import android.widget.LinearLayout
import android.widget.TextView

/**
 * A human-visible landing screen only. The agent itself is started by the
 * master through `am instrument` (this app is the instrumentation target),
 * binds its gRPC server on all interfaces and drives whatever app the
 * handshake names. This screen proves the install and shows what it is.
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
                id = R.id.agent_title
                text = "Impresario agent"
                textSize = 22f
            },
        )
        column.addView(
            TextView(this).apply {
                id = R.id.agent_info
                text = buildString {
                    append("pkg=").append(packageName).append('\n')
                    append("sdk=").append(Build.VERSION.SDK_INT).append('\n')
                    append("model=").append(Build.MODEL).append("\n\n")
                    append("The master starts the session with `am instrument`\n")
                    append("and connects to this app's gRPC server.")
                }
                textSize = 15f
                setPadding(0, dp(12), 0, 0)
            },
        )
        setContentView(column)
    }
}
