package com.impresario.hook

import android.content.Context
import android.util.Log
import org.json.JSONObject

/**
 * Debug-only reference SDK for Impresario's optional in-app test hook (D-41).
 *
 * Call [player], [drm] and [showing] whenever the state changes (or on a short
 * timer) and [report] once per update. The agent reads the latest
 * `IMPRESARIO_HOOK <json>` logcat line and treats it as AUTHORITATIVE
 * evidence. A missing report is never a failure; it leaves a claim
 * UNVERIFIABLE.
 *
 * Keep this out of release builds (debugImplementation). It records state
 * only: no license keys, tokens or media payloads.
 */
object ImpresarioHook {
    const val TAG = "ImpresarioHook"
    const val MARKER = "IMPRESARIO_HOOK "

    data class Media(
        val state: String? = null,
        val positionSeconds: Double? = null,
        val durationSeconds: Double? = null,
        val rate: Double? = null,
        val volume: Double? = null,
        val muted: Boolean? = null,
        val widthPx: Int? = null,
        val heightPx: Int? = null,
        val audioTrack: String? = null,
        val subtitleTrack: String? = null,
        val error: String? = null,
        val decoded: Boolean? = null,
        val source: String? = null,
    )

    data class Drm(
        val enabled: Boolean? = null,
        val provider: String? = null,
        val license: String? = null,
        val keyStatus: String? = null,
        val securityLevel: String? = null,
        val hdcp: String? = null,
    )

    data class Showing(
        val decoded: Boolean? = null,
        val source: String? = null,
        val piece: String? = null,
    )

    private val media = JSONObject()
    private val drm = JSONObject()
    private val showing = JSONObject()

    @Volatile
    private var packageName: String = ""

    /**
     * Binds the SDK to the app. Call once (usually in `Application.onCreate`
     * or the first activity) before reporting; the driver rejects snapshots
     * without a package because it must not attribute another app's state.
     */
    @Synchronized
    fun start(context: Context) {
        packageName = context.applicationContext?.packageName ?: context.packageName
    }

    @Synchronized
    fun player(value: Media) {
        replace(media, encode(value))
        emit()
    }

    fun media(value: Media) = player(value)

    @Synchronized
    fun drm(value: Drm) {
        replace(drm, encode(value))
        emit()
    }

    @Synchronized
    fun showing(value: Showing) {
        replace(showing, encode(value))
        emit()
    }

    /** Clears all fields, e.g. when leaving a player screen. */
    @Synchronized
    fun clear() {
        clear(media)
        clear(drm)
        clear(showing)
    }

    /** Logs the current snapshot; called by the setters, exposed for retries. */
    @Synchronized
    fun report() = emit()

    @Synchronized
    fun snapshot(): JSONObject {
        val root = JSONObject()
        if (media.length() > 0) root.put("media", JSONObject(media.toString()))
        if (drm.length() > 0) root.put("drm", JSONObject(drm.toString()))
        if (showing.length() > 0) root.put("showing", JSONObject(showing.toString()))
        return root
    }

    private fun emit() {
        val root = snapshot()
        if (root.length() == 0) return
        root.put("package", packageName)
        root.put("reportedAtUnixMs", System.currentTimeMillis())
        Log.i(TAG, MARKER + root.toString())
    }

    private fun encode(value: Media): JSONObject = JSONObject().apply {
        putOpt("state", value.state)
        putOpt("positionSeconds", value.positionSeconds)
        putOpt("durationSeconds", value.durationSeconds)
        putOpt("rate", value.rate)
        putOpt("volume", value.volume)
        putOpt("muted", value.muted)
        putOpt("widthPx", value.widthPx)
        putOpt("heightPx", value.heightPx)
        putOpt("audioTrack", value.audioTrack)
        putOpt("subtitleTrack", value.subtitleTrack)
        putOpt("error", value.error)
        putOpt("decoded", value.decoded)
        putOpt("source", value.source)
    }

    private fun encode(value: Drm): JSONObject = JSONObject().apply {
        putOpt("enabled", value.enabled)
        putOpt("provider", value.provider)
        putOpt("license", value.license)
        putOpt("keyStatus", value.keyStatus)
        putOpt("securityLevel", value.securityLevel)
        putOpt("hdcp", value.hdcp)
    }

    private fun encode(value: Showing): JSONObject = JSONObject().apply {
        putOpt("decoded", value.decoded)
        putOpt("source", value.source)
        putOpt("piece", value.piece)
    }

    private fun replace(target: JSONObject, source: JSONObject) {
        val names = target.keys()
        while (names.hasNext()) target.remove(names.next())
        val incoming = source.keys()
        while (incoming.hasNext()) {
            val key = incoming.next()
            target.put(key, source.get(key))
        }
    }

    private fun clear(target: JSONObject) {
        val names = target.keys()
        while (names.hasNext()) target.remove(names.next())
    }
}
