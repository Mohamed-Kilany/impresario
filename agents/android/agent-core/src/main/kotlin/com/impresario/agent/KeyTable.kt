package com.impresario.agent

import impresario.v1.Impresario.RemoteKey

/**
 * Table-driven key mapping for Android and Android TV (spec 6.2).
 *
 * `agent-core` is pure Kotlin/JVM, so the Android `KeyEvent` values are named
 * here (with the Android constant in the comment) instead of importing
 * `android.view.KeyEvent`. Values are asserted by unit tests against the spec
 * table.
 */
object KeyTable {
    const val KEYCODE_HOME = 3
    const val KEYCODE_BACK = 4
    const val KEYCODE_DPAD_UP = 19
    const val KEYCODE_DPAD_DOWN = 20
    const val KEYCODE_DPAD_LEFT = 21
    const val KEYCODE_DPAD_RIGHT = 22
    const val KEYCODE_DPAD_CENTER = 23
    const val KEYCODE_ENTER = 66
    const val KEYCODE_DEL = 67
    const val KEYCODE_MENU = 82
    const val KEYCODE_MEDIA_PLAY_PAUSE = 85
    const val KEYCODE_MEDIA_REWIND = 89
    const val KEYCODE_MEDIA_FAST_FORWARD = 90
    const val KEYCODE_MEDIA_PLAY = 126
    const val KEYCODE_MEDIA_PAUSE = 127
    const val KEYCODE_PROG_RED = 183
    const val KEYCODE_PROG_GREEN = 184
    const val KEYCODE_PROG_YELLOW = 185
    const val KEYCODE_PROG_BLUE = 186

    data class Mapping(val keyCode: Int, val phone: Boolean, val tv: Boolean)

    private val mappings: Map<RemoteKey, Mapping> = mapOf(
        RemoteKey.KEY_OK to Mapping(KEYCODE_DPAD_CENTER, phone = false, tv = true),
        RemoteKey.KEY_ENTER to Mapping(KEYCODE_ENTER, phone = true, tv = true),
        RemoteKey.KEY_BACK to Mapping(KEYCODE_BACK, phone = true, tv = true),
        RemoteKey.KEY_UP to Mapping(KEYCODE_DPAD_UP, phone = true, tv = true),
        RemoteKey.KEY_DOWN to Mapping(KEYCODE_DPAD_DOWN, phone = true, tv = true),
        RemoteKey.KEY_LEFT to Mapping(KEYCODE_DPAD_LEFT, phone = true, tv = true),
        RemoteKey.KEY_RIGHT to Mapping(KEYCODE_DPAD_RIGHT, phone = true, tv = true),
        RemoteKey.KEY_HOME to Mapping(KEYCODE_HOME, phone = true, tv = true),
        RemoteKey.KEY_MENU to Mapping(KEYCODE_MENU, phone = true, tv = true),
        RemoteKey.KEY_PLAY to Mapping(KEYCODE_MEDIA_PLAY, phone = true, tv = true),
        RemoteKey.KEY_PAUSE to Mapping(KEYCODE_MEDIA_PAUSE, phone = true, tv = true),
        RemoteKey.KEY_PLAYPAUSE to Mapping(KEYCODE_MEDIA_PLAY_PAUSE, phone = true, tv = true),
        RemoteKey.KEY_REWIND to Mapping(KEYCODE_MEDIA_REWIND, phone = true, tv = true),
        RemoteKey.KEY_FASTFORWARD to Mapping(KEYCODE_MEDIA_FAST_FORWARD, phone = true, tv = true),
        RemoteKey.KEY_RED to Mapping(KEYCODE_PROG_RED, phone = false, tv = true),
        RemoteKey.KEY_GREEN to Mapping(KEYCODE_PROG_GREEN, phone = false, tv = true),
        RemoteKey.KEY_YELLOW to Mapping(KEYCODE_PROG_YELLOW, phone = false, tv = true),
        RemoteKey.KEY_BLUE to Mapping(KEYCODE_PROG_BLUE, phone = false, tv = true),
        RemoteKey.KEY_BACKSPACE to Mapping(KEYCODE_DEL, phone = true, tv = true),
    )

    /** All keys the spec table knows; used by tests to prove table completeness. */
    val knownKeys: Set<RemoteKey> get() = mappings.keys

    fun mapping(key: RemoteKey): Mapping? = mappings[key]

    /** Resolves the Android key code for a platform; throws per protocol rule 6. */
    fun resolve(key: RemoteKey, isTv: Boolean): Int {
        if (key == RemoteKey.REMOTE_KEY_UNSPECIFIED) {
            throw AgentException.protocol("the key is unspecified")
        }
        val mapping = mappings[key]
            ?: throw AgentException.unsupportedAction("key '${key.name}' is not supported on android")
        if (isTv && !mapping.tv) {
            throw AgentException.unsupportedAction("key '${key.name}' is not supported on android tv")
        }
        if (!isTv && !mapping.phone) {
            throw AgentException.unsupportedAction("key '${key.name}' is not supported on android phones")
        }
        return mapping.keyCode
    }
}
