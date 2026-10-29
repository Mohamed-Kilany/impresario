package com.impresario.agent

import impresario.v1.Impresario.Capability
import impresario.v1.Impresario.ErrorCode
import impresario.v1.Impresario.HandshakeRequest
import impresario.v1.Impresario.Platform
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test

class AgentStateTest {
    private fun request(
        platform: Platform = Platform.ANDROID,
        appId: String = "com.example.app",
        protocol: String = "1.0.0",
        token: String = "",
        options: Map<String, String> = emptyMap(),
    ): HandshakeRequest = HandshakeRequest.newBuilder()
        .setProtocolVersion(protocol)
        .setMasterVersion("1.0.0")
        .setDeviceName("device")
        .setPlatform(platform)
        .setAppId(appId)
        .setAuthToken(token)
        .putAllOptions(options)
        .build()

    @Test
    fun acceptsAndroidAndReportsCapabilities() {
        val state = AgentState(FakeDriver())
        val response = state.handshake(request())
        assertTrue(response.accepted)
        assertTrue(response.sessionId.isNotEmpty())
        assertEquals("1.0.0", response.protocolVersion)
        assertEquals(AgentVersion.VERSION, response.agentVersion)
        assertTrue(response.capabilitiesList.contains(Capability.TOUCH_GESTURES))
        assertTrue(response.capabilitiesList.contains(Capability.SCREENSHOT))
        assertTrue(response.capabilitiesList.contains(Capability.LOGS))
        assertTrue(response.capabilitiesList.contains(Capability.APP_CONTROL))
        assertFalse(response.capabilitiesList.contains(Capability.DPAD_FOCUS))
        assertFalse(response.capabilitiesList.contains(Capability.MULTI_SESSION))
        assertEquals("UiAutomator2", response.device.runtime)
    }

    @Test
    fun tvAdvertisesDpadInsteadOfTouch() {
        val state = AgentState(FakeDriver())
        val response = state.handshake(request(platform = Platform.ANDROID_TV))
        assertTrue(response.accepted)
        assertTrue(response.capabilitiesList.contains(Capability.DPAD_FOCUS))
        assertFalse(response.capabilitiesList.contains(Capability.TOUCH_GESTURES))
    }

    @Test
    fun rejectsIncompatibleProtocolMajor() {
        val state = AgentState(FakeDriver())
        val response = state.handshake(request(protocol = "2.0.0"))
        assertFalse(response.accepted)
        assertEquals(ErrorCode.PROTOCOL, response.error.code)
        assertTrue(response.error.message.contains("2.0.0"))
    }

    @Test
    fun rejectsPlatformsTheAgentDoesNotServe() {
        val state = AgentState(FakeDriver())
        val response = state.handshake(request(platform = Platform.WEB))
        assertFalse(response.accepted)
        assertEquals(ErrorCode.PROTOCOL, response.error.code)
    }

    @Test
    fun rejectsASecondSession() {
        val state = AgentState(FakeDriver())
        assertTrue(state.handshake(request()).accepted)
        val second = state.handshake(request())
        assertFalse(second.accepted)
        assertEquals(ErrorCode.ALREADY_IN_USE, second.error.code)
    }

    @Test
    fun tokenIsRequiredWhenConfigured() {
        val state = AgentState(FakeDriver(), expectedToken = "s3cr3t")
        val missing = state.handshake(request())
        assertFalse(missing.accepted)
        assertEquals(ErrorCode.UNAUTHENTICATED, missing.error.code)

        val wrong = state.handshake(request(token = "nope"))
        assertFalse(wrong.accepted)
        assertEquals(ErrorCode.UNAUTHENTICATED, wrong.error.code)

        val accepted = state.handshake(request(token = "s3cr3t"))
        assertTrue(accepted.accepted)
    }

    @Test
    fun handshakeOptionsReachTheExecutor() {
        val state = AgentState(FakeDriver())
        val response = state.handshake(
            request(
                options = mapOf(
                    "poll_interval_ms" to "42",
                    "scroll_max_attempts" to "7",
                    "focus_max_steps" to "3",
                    "clear_app_data" to "true",
                ),
            ),
        )
        assertTrue(response.accepted)
        val session = state.sessionFor(response.sessionId)
        assertNotNull(session)
        assertEquals(42, session!!.options.pollIntervalMs)
        assertEquals(7, session.options.scrollMaxAttempts)
        assertEquals(3, session.options.focusMaxSteps)
        assertTrue(session.options.clearAppDataOnRestart)
    }

    @Test
    fun streamFlagIsExclusive() {
        val state = AgentState(FakeDriver())
        val response = state.handshake(request())
        val session = state.sessionFor(response.sessionId)!!
        assertTrue(state.beginStream(session))
        assertFalse(state.beginStream(session))
        state.endStream(session)
        assertTrue(state.beginStream(session))
    }
}
