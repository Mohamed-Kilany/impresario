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
    const val KEYCODE_0 = 7
    const val KEYCODE_1 = 8
    const val KEYCODE_2 = 9
    const val KEYCODE_3 = 10
    const val KEYCODE_4 = 11
    const val KEYCODE_5 = 12
    const val KEYCODE_6 = 13
    const val KEYCODE_7 = 14
    const val KEYCODE_8 = 15
    const val KEYCODE_9 = 16
    const val KEYCODE_DPAD_UP = 19
    const val KEYCODE_DPAD_DOWN = 20
    const val KEYCODE_DPAD_LEFT = 21
    const val KEYCODE_DPAD_RIGHT = 22
    const val KEYCODE_DPAD_CENTER = 23
    const val KEYCODE_VOLUME_UP = 24
    const val KEYCODE_VOLUME_DOWN = 25
    const val KEYCODE_ENTER = 66
    const val KEYCODE_DEL = 67
    const val KEYCODE_MENU = 82
    const val KEYCODE_SEARCH = 84
    const val KEYCODE_MEDIA_PLAY_PAUSE = 85
    const val KEYCODE_MEDIA_STOP = 86
    const val KEYCODE_MEDIA_REWIND = 89
    const val KEYCODE_MEDIA_FAST_FORWARD = 90
    const val KEYCODE_ESCAPE = 111
    const val KEYCODE_MEDIA_PLAY = 126
    const val KEYCODE_MEDIA_PAUSE = 127
    const val KEYCODE_MEDIA_RECORD = 130
    const val KEYCODE_MUTE = 164
    const val KEYCODE_INFO = 165
    const val KEYCODE_CHANNEL_UP = 166
    const val KEYCODE_CHANNEL_DOWN = 167
    const val KEYCODE_GUIDE = 172
    const val KEYCODE_CAPTIONS = 175
    const val KEYCODE_SETTINGS = 176
    const val KEYCODE_PROG_RED = 183
    const val KEYCODE_PROG_GREEN = 184
    const val KEYCODE_PROG_YELLOW = 185
    const val KEYCODE_PROG_BLUE = 186
    const val KEYCODE_MEDIA_AUDIO_TRACK = 222

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
        RemoteKey.KEY_NUM0 to Mapping(KEYCODE_0, phone = true, tv = true),
        RemoteKey.KEY_NUM1 to Mapping(KEYCODE_1, phone = true, tv = true),
        RemoteKey.KEY_NUM2 to Mapping(KEYCODE_2, phone = true, tv = true),
        RemoteKey.KEY_NUM3 to Mapping(KEYCODE_3, phone = true, tv = true),
        RemoteKey.KEY_NUM4 to Mapping(KEYCODE_4, phone = true, tv = true),
        RemoteKey.KEY_NUM5 to Mapping(KEYCODE_5, phone = true, tv = true),
        RemoteKey.KEY_NUM6 to Mapping(KEYCODE_6, phone = true, tv = true),
        RemoteKey.KEY_NUM7 to Mapping(KEYCODE_7, phone = true, tv = true),
        RemoteKey.KEY_NUM8 to Mapping(KEYCODE_8, phone = true, tv = true),
        RemoteKey.KEY_NUM9 to Mapping(KEYCODE_9, phone = true, tv = true),
        RemoteKey.KEY_VOLUMEUP to Mapping(KEYCODE_VOLUME_UP, phone = true, tv = true),
        RemoteKey.KEY_VOLUMEDOWN to Mapping(KEYCODE_VOLUME_DOWN, phone = true, tv = true),
        RemoteKey.KEY_MUTE to Mapping(KEYCODE_MUTE, phone = true, tv = true),
        RemoteKey.KEY_CHANNELUP to Mapping(KEYCODE_CHANNEL_UP, phone = true, tv = true),
        RemoteKey.KEY_CHANNELDOWN to Mapping(KEYCODE_CHANNEL_DOWN, phone = true, tv = true),
        RemoteKey.KEY_STOP to Mapping(KEYCODE_MEDIA_STOP, phone = true, tv = true),
        RemoteKey.KEY_RECORD to Mapping(KEYCODE_MEDIA_RECORD, phone = true, tv = true),
        RemoteKey.KEY_INFO to Mapping(KEYCODE_INFO, phone = true, tv = true),
        RemoteKey.KEY_GUIDE to Mapping(KEYCODE_GUIDE, phone = true, tv = true),
        RemoteKey.KEY_EXIT to Mapping(KEYCODE_ESCAPE, phone = true, tv = true),
        RemoteKey.KEY_SUBTITLE to Mapping(KEYCODE_CAPTIONS, phone = true, tv = true),
        RemoteKey.KEY_AUDIO to Mapping(KEYCODE_MEDIA_AUDIO_TRACK, phone = true, tv = true),
        RemoteKey.KEY_SEARCH to Mapping(KEYCODE_SEARCH, phone = true, tv = true),
        RemoteKey.KEY_SETTINGS to Mapping(KEYCODE_SETTINGS, phone = true, tv = true),
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
