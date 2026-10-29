package com.impresario.agent

import impresario.v1.Impresario.RemoteKey
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

/** The android/android-tv key table of spec 6.2, verbatim. */
class KeyTableTest {
    @Test
    fun everyKnownKeyIsMapped() {
        RemoteKey.values()
            .filter { it != RemoteKey.REMOTE_KEY_UNSPECIFIED && it != RemoteKey.UNRECOGNIZED }
            .forEach { key ->
                assertTrue("missing mapping for $key", KeyTable.mapping(key) != null)
            }
    }

    @Test
    fun specTableValuesMatch() {
        fun code(key: RemoteKey, isTv: Boolean): Int = KeyTable.resolve(key, isTv)
        assertEquals(66, code(RemoteKey.KEY_ENTER, isTv = false))
        assertEquals(66, code(RemoteKey.KEY_ENTER, isTv = true))
        assertEquals(4, code(RemoteKey.KEY_BACK, isTv = false))
        assertEquals(19, code(RemoteKey.KEY_UP, isTv = false))
        assertEquals(20, code(RemoteKey.KEY_DOWN, isTv = true))
        assertEquals(21, code(RemoteKey.KEY_LEFT, isTv = false))
        assertEquals(22, code(RemoteKey.KEY_RIGHT, isTv = true))
        assertEquals(3, code(RemoteKey.KEY_HOME, isTv = false))
        assertEquals(82, code(RemoteKey.KEY_MENU, isTv = true))
        assertEquals(126, code(RemoteKey.KEY_PLAY, isTv = false))
        assertEquals(127, code(RemoteKey.KEY_PAUSE, isTv = true))
        assertEquals(85, code(RemoteKey.KEY_PLAYPAUSE, isTv = false))
        assertEquals(89, code(RemoteKey.KEY_REWIND, isTv = true))
        assertEquals(90, code(RemoteKey.KEY_FASTFORWARD, isTv = false))
        assertEquals(67, code(RemoteKey.KEY_BACKSPACE, isTv = true))
        assertEquals(23, code(RemoteKey.KEY_OK, isTv = true))
        assertEquals(183, code(RemoteKey.KEY_RED, isTv = true))
        assertEquals(184, code(RemoteKey.KEY_GREEN, isTv = true))
        assertEquals(185, code(RemoteKey.KEY_YELLOW, isTv = true))
        assertEquals(186, code(RemoteKey.KEY_BLUE, isTv = true))
        assertEquals(7, code(RemoteKey.KEY_NUM0, isTv = false))
        assertEquals(16, code(RemoteKey.KEY_NUM9, isTv = true))
        assertEquals(24, code(RemoteKey.KEY_VOLUMEUP, isTv = false))
        assertEquals(25, code(RemoteKey.KEY_VOLUMEDOWN, isTv = true))
        assertEquals(164, code(RemoteKey.KEY_MUTE, isTv = false))
        assertEquals(166, code(RemoteKey.KEY_CHANNELUP, isTv = true))
        assertEquals(167, code(RemoteKey.KEY_CHANNELDOWN, isTv = false))
        assertEquals(86, code(RemoteKey.KEY_STOP, isTv = true))
        assertEquals(130, code(RemoteKey.KEY_RECORD, isTv = false))
        assertEquals(165, code(RemoteKey.KEY_INFO, isTv = true))
        assertEquals(172, code(RemoteKey.KEY_GUIDE, isTv = false))
        assertEquals(111, code(RemoteKey.KEY_EXIT, isTv = true))
        assertEquals(175, code(RemoteKey.KEY_SUBTITLE, isTv = false))
        assertEquals(222, code(RemoteKey.KEY_AUDIO, isTv = true))
        assertEquals(84, code(RemoteKey.KEY_SEARCH, isTv = false))
        assertEquals(176, code(RemoteKey.KEY_SETTINGS, isTv = true))
    }

    @Test
    fun phoneOnlyKeysRejectTvAndViceVersa() {
        assertNull(KeyTable.mapping(RemoteKey.REMOTE_KEY_UNSPECIFIED))
        val okOnPhone = assertThrows(AgentException::class.java) {
            KeyTable.resolve(RemoteKey.KEY_OK, isTv = false)
        }
        assertEquals(impresario.v1.Impresario.ErrorCode.UNSUPPORTED_ACTION, okOnPhone.code)

        val redOnPhone = assertThrows(AgentException::class.java) {
            KeyTable.resolve(RemoteKey.KEY_RED, isTv = false)
        }
        assertEquals(impresario.v1.Impresario.ErrorCode.UNSUPPORTED_ACTION, redOnPhone.code)
    }

    @Test
    fun unspecifiedKeyIsAProtocolError() {
        val error = assertThrows(AgentException::class.java) {
            KeyTable.resolve(RemoteKey.REMOTE_KEY_UNSPECIFIED, isTv = false)
        }
        assertEquals(impresario.v1.Impresario.ErrorCode.PROTOCOL, error.code)
    }
}
