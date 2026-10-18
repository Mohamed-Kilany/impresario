package com.impresario.agent

import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.util.Log

/**
 * Starts the agent app's session guard (`ProcessGuardService`) so the vendor
 * power manager does not freeze this process mid-session (docs/DECISIONS.md
 * I-68). The service lives in the agent app, which this module does not
 * compile against, so it is started by component name. Idempotent; never
 * fails the caller.
 */
object ProcessGuard {
    private const val TAG = "ImpGuard"
    private const val SERVICE = "com.impresario.agent.app.ProcessGuardService"

    @Volatile
    private var started = false

    fun ensure(context: Context) {
        if (started) return
        started = true
        try {
            val intent = Intent().setComponent(ComponentName(context.packageName, SERVICE))
            context.startForegroundService(intent)
        } catch (error: RuntimeException) {
            // The guard is an optimization; an OS that refuses the start must
            // not break the session.
            Log.w(TAG, "session guard not started: ${error.message}")
        }
    }
}
