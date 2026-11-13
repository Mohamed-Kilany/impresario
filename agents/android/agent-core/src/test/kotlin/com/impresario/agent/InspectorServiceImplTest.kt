package com.impresario.agent

import impresario.v1.Impresario.Direction as PbDirection
import impresario.v1.Impresario.HandshakeRequest
import impresario.v1.Impresario.InspectorInputKind
import impresario.v1.Impresario.InspectorInputRequest
import impresario.v1.Impresario.InspectorNode
import impresario.v1.Impresario.InspectorRect
import impresario.v1.Impresario.InspectorSnapshotRequest
import impresario.v1.Impresario.Platform
import impresario.v1.Impresario.RemoteKey
import io.grpc.Context
import io.grpc.Status
import io.grpc.StatusRuntimeException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test

class InspectorServiceImplTest {
    private class FakeInspectorDriver : InspectorDriver {
        val calls = mutableListOf<String>()
        var capture = InspectorCapture(
            nodes = listOf(
                InspectorNode.newBuilder()
                    .setId(7)
                    .setRole("button")
                    .setName("Basic controls")
                    .setText("Basic controls")
                    .setFocused(true)
                    .setEnabled(true)
                    .setBounds(InspectorRect.newBuilder().setX(10f).setY(20f).setW(100f).setH(40f))
                    .build(),
            ),
            focusedId = 7,
            truncated = false,
        )

        override fun capture(options: InspectorCaptureOptions): InspectorCapture {
            calls += "capture:${options.appId}:${options.maxNodes}:${options.maxDepth}"
            return capture
        }

        override fun tap(nodeId: Long, x: Float, y: Float) {
            calls += "tap:$nodeId:$x:$y"
        }

        override fun scroll(direction: Direction, count: Int, nodeId: Long, x: Float, y: Float) {
            calls += "scroll:$direction:$count:$nodeId:$x:$y"
        }

        override fun pressKey(code: Int) {
            calls += "key:$code"
        }

        override fun type(nodeId: Long, text: String) {
            calls += "type:$nodeId:$text"
        }

        override fun swipe(x1: Float, y1: Float, x2: Float, y2: Float, durationMs: Long) {
            calls += "swipe:$x1:$y1:$x2:$y2:$durationMs"
        }
    }

    private fun state(driver: InspectorDriver, token: String = ""): AgentState {
        val state = AgentState(FakeDriver(), expectedToken = token, inspectorDriver = driver)
        val response = state.handshake(
            HandshakeRequest.newBuilder()
                .setProtocolVersion("1.0.0")
                .setMasterVersion("test")
                .setDeviceName("phone")
                .setPlatform(Platform.ANDROID)
                .setAppId("com.example.app")
                .setAuthToken(token)
                .build(),
        )
        assertTrue("handshake rejected: ${response.error.message}", response.accepted)
        return state
    }

    private fun Context.callBlocking(block: suspend () -> Unit) {
        call {
            runBlocking { block() }
        }
    }

    @Test
    fun snapshotNormalizesCaptureAndReportsLogicalGeometry() {
        val driver = FakeInspectorDriver()
        val state = state(driver)
        val service = InspectorServiceImpl(state, Dispatchers.Unconfined)
        val context = Context.current()
            .withValue(AuthInterceptor.CONTEXT_SESSION_ID, state.currentSession!!.id)
        lateinit var response: impresario.v1.Impresario.InspectorSnapshotResponse
        context.callBlocking {
            response = service.snapshot(InspectorSnapshotRequest.getDefaultInstance())
        }
        assertTrue(response.rev > 0)
        assertEquals(1, response.nodesCount)
        assertEquals("button", response.nodesList[0].role)
        assertEquals(7L, response.focusedId)
        assertEquals(1, driver.calls.size)
        assertEquals("capture:com.example.app:2000:64", driver.calls[0])
        // 1080 / 2.75 and 2400 / 2.75 from FakeDriver's device info.
        assertEquals(392.727f, response.logicalWidth, 0.01f)
        assertEquals(872.727f, response.logicalHeight, 0.01f)
        assertEquals(2.75f, response.pixelRatio, 0.001f)
        assertTrue(response.settled)
    }

    @Test
    fun inputMapsEveryKindOntoTheDriver() {
        val driver = FakeInspectorDriver()
        val state = state(driver)
        val service = InspectorServiceImpl(state, Dispatchers.Unconfined)
        val context = Context.current()
            .withValue(AuthInterceptor.CONTEXT_SESSION_ID, state.currentSession!!.id)
        context.callBlocking {
            assertTrue(
                service.input(
                    InspectorInputRequest.newBuilder()
                        .setKind(InspectorInputKind.INSPECTOR_INPUT_KIND_TAP)
                        .setNodeId(7)
                        .build(),
                ).ok,
            )
            assertTrue(
                service.input(
                    InspectorInputRequest.newBuilder()
                        .setKind(InspectorInputKind.INSPECTOR_INPUT_KIND_SCROLL)
                        .setDirection(PbDirection.DOWN)
                        .setCount(2)
                        .setNodeId(7)
                        .build(),
                ).ok,
            )
            assertTrue(
                service.input(
                    InspectorInputRequest.newBuilder()
                        .setKind(InspectorInputKind.INSPECTOR_INPUT_KIND_KEY)
                        .setKey(RemoteKey.KEY_ENTER)
                        .build(),
                ).ok,
            )
            assertTrue(
                service.input(
                    InspectorInputRequest.newBuilder()
                        .setKind(InspectorInputKind.INSPECTOR_INPUT_KIND_TYPE)
                        .setNodeId(7)
                        .setText("hello")
                        .build(),
                ).ok,
            )
            assertTrue(
                service.input(
                    InspectorInputRequest.newBuilder()
                        .setKind(InspectorInputKind.INSPECTOR_INPUT_KIND_SWIPE)
                        .setX(1f)
                        .setY(2f)
                        .setEndX(3f)
                        .setEndY(4f)
                        .setDurationMs(200)
                        .build(),
                ).ok,
            )
        }
        assertEquals(
            listOf(
                "tap:7:0.0:0.0",
                "scroll:DOWN:2:7:0.0:0.0",
                "key:66",
                "type:7:hello",
                "swipe:1.0:2.0:3.0:4.0:200",
            ),
            driver.calls,
        )
    }

    @Test
    fun inputFailuresAreReportedInTheResponse() {
        val driver = object : InspectorDriver by FakeInspectorDriver() {
            override fun tap(nodeId: Long, x: Float, y: Float) {
                throw AgentException.notInteractable("the element is covered")
            }
        }
        val state = state(driver)
        val service = InspectorServiceImpl(state, Dispatchers.Unconfined)
        val context = Context.current()
            .withValue(AuthInterceptor.CONTEXT_SESSION_ID, state.currentSession!!.id)
        lateinit var response: impresario.v1.Impresario.InspectorInputResponse
        context.callBlocking {
            response = service.input(
                InspectorInputRequest.newBuilder()
                    .setKind(InspectorInputKind.INSPECTOR_INPUT_KIND_TAP)
                    .setNodeId(1)
                    .build(),
            )
        }
        assertFalse(response.ok)
        assertEquals(
            impresario.v1.Impresario.ErrorCode.NOT_INTERACTABLE,
            response.error.code,
        )
    }

    @Test
    fun missingSessionAndBadTokenAreRejected() {
        val driver = FakeInspectorDriver()
        val unauthenticatedState = state(driver)
        val unauthenticated = InspectorServiceImpl(unauthenticatedState, Dispatchers.Unconfined)
        try {
            runBlocking { unauthenticated.snapshot(InspectorSnapshotRequest.getDefaultInstance()) }
            fail("a missing session must be rejected")
        } catch (error: StatusRuntimeException) {
            assertEquals(Status.Code.INVALID_ARGUMENT, error.status.code)
        }
        val tokenState = state(driver, token = "secret")
        val service = InspectorServiceImpl(tokenState, Dispatchers.Unconfined)
        val noToken = Context.current()
            .withValue(AuthInterceptor.CONTEXT_SESSION_ID, tokenState.currentSession!!.id)
        try {
            noToken.callBlocking {
                service.snapshot(InspectorSnapshotRequest.getDefaultInstance())
            }
            fail("a missing token must be rejected")
        } catch (error: StatusRuntimeException) {
            assertEquals(Status.Code.UNAUTHENTICATED, error.status.code)
        }
    }
}
