package com.impresario.agent.app

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.content.Intent
import android.os.IBinder
import android.util.Log

/**
 * Keeps the agent process in the foreground while a session runs.
 *
 * Vendor power managers on TV hardware freeze background processes on a
 * timer — on the TCL TV, `FreezeManager` froze the agent every ~40 s even
 * with the screen on (docs/DECISIONS.md I-68), so a freeze landing on an
 * in-flight UiAutomation/binder call regularly wedged the instrumentation at
 * a random test. A foreground service lifts the process out of the
 * freezable background set. It is started by the instrumentation
 * (`com.impresario.agent.ProcessGuard`, same process) and by
 * `AgentRunner.serve`; the teardown path (`am force-stop`) removes it.
 */
class ProcessGuardService : Service() {

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        try {
            val manager = getSystemService(NotificationManager::class.java)
                ?: return START_NOT_STICKY
            manager.createNotificationChannel(
                NotificationChannel(CHANNEL_ID, "Agent session", NotificationManager.IMPORTANCE_LOW),
            )
            val notification = Notification.Builder(this, CHANNEL_ID)
                .setSmallIcon(android.R.drawable.stat_notify_sync)
                .setContentTitle("Impresario agent")
                .setContentText("Session running")
                .build()
            startForeground(NOTIFICATION_ID, notification)
        } catch (error: RuntimeException) {
            // The guard must never take the agent process down with it.
            Log.w(TAG, "session guard failed: ${error.message}")
            stopSelf()
        }
        return START_NOT_STICKY
    }

    private companion object {
        const val TAG = "ImpGuard"
        const val CHANNEL_ID = "impresario-agent"
        const val NOTIFICATION_ID = 1
    }
}
