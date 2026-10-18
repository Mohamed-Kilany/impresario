package com.impresario.agent

import impresario.v1.Impresario.HandshakeRequest
import impresario.v1.Impresario.LogLevel
import impresario.v1.Impresario.LogRequest
import impresario.v1.Impresario.Platform
import io.grpc.Context
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.junit.Assert.assertTrue
import org.junit.Test

class AgentServiceImplTest {
    @Test
    fun streamLogsOpensWithAnEntryBeforeAnyActivity() = runBlocking {
        val state = AgentState(FakeDriver())
        val response = state.handshake(
            HandshakeRequest.newBuilder()
                .setProtocolVersion("1.0.0")
                .setMasterVersion("0.1.0")
                .setDeviceName("device")
                .setPlatform(Platform.ANDROID)
                .setAppId("com.example.app")
                .build(),
        )
        assertTrue("handshake rejected", response.accepted)

        val service = AgentServiceImpl(state)
        val context = Context.current()
            .withValue(AuthInterceptor.CONTEXT_SESSION_ID, response.sessionId)
        val flow = context.call {
            service.streamLogs(LogRequest.newBuilder().setMinLevel(LogLevel.DEBUG).build())
        }

        // The opening entry must arrive without any log-producing RPC:
        // grpc-java sends response headers only with the first message, and
        // the master awaits the open before it sends its first command
        // (docs/DECISIONS.md I-74).
        val first = withTimeout(1_000) { flow.first() }
        assertTrue(
            "unexpected opening entry: ${first.message}",
            first.message.contains("log stream opened"),
        )
    }
}
