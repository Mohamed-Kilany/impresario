package com.impresario.agent

import android.os.SystemClock
import androidx.test.platform.app.InstrumentationRegistry
import com.impresario.agent.driver.UiAutomatorDriver
import impresario.v1.AgentGrpc
import impresario.v1.Impresario.Direction
import impresario.v1.Impresario.HandshakeRequest
import impresario.v1.Impresario.InspectorInputKind
import impresario.v1.Impresario.InspectorInputRequest
import impresario.v1.Impresario.InspectorSnapshotRequest
import impresario.v1.Impresario.InspectorSnapshotResponse
import impresario.v1.Impresario.Platform
import impresario.v1.Impresario.RemoteKey
import impresario.v1.InspectorGrpc
import io.grpc.ManagedChannel
import io.grpc.Metadata
import io.grpc.netty.shaded.io.grpc.netty.NettyChannelBuilder
import io.grpc.stub.MetadataUtils
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test
import kotlin.math.roundToInt

/**
 * The additive Inspector service on a real device: capture the stub's
 * accessibility tree, tap a scenario row by snapshot id, then append text to
 * the entry field through the same unary input path the Web UI uses.
 */
class InspectorInstrumentedTest {
    @Test
    fun inspectorCapturesAndDrivesTheStubApp() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        ProcessGuard.ensure(instrumentation.targetContext)
        val driver = UiAutomatorDriver(instrumentation)
        val isTv = deviceIsTv()
        driver.terminate(STUB_PACKAGE)
        val state = AgentState(driver)
        val server = AgentServer(
            AgentServiceImpl(state),
            requestedPort = 0,
            inspectorService = InspectorServiceImpl(state),
        )
        server.start()
        val channel: ManagedChannel = NettyChannelBuilder
            .forAddress("127.0.0.1", server.port)
            .usePlaintext()
            .build()
        try {
            val handshake = AgentGrpc.newBlockingStub(channel).handshake(
                HandshakeRequest.newBuilder()
                    .setProtocolVersion(PROTOCOL_VERSION)
                    .setMasterVersion("instrumented-test")
                    .setDeviceName(if (isTv) "android-tv" else "phone")
                    .setPlatform(if (isTv) Platform.ANDROID_TV else Platform.ANDROID)
                    .setAppId(STUB_PACKAGE)
                    .build(),
            )
            assertTrue("handshake rejected: ${handshake.error.message}", handshake.accepted)
            assertTrue(
                "INSPECTOR was not advertised",
                handshake.capabilitiesList.contains(impresario.v1.Impresario.Capability.INSPECTOR),
            )
            val headers = Metadata().apply {
                put(SESSION_ID_KEY, handshake.sessionId)
            }
            val inspector = InspectorGrpc.newBlockingStub(channel)
                .withInterceptors(MetadataUtils.newAttachHeadersInterceptor(headers))

            driver.launch(STUB_PACKAGE, isTv)
            val home = awaitSnapshot(inspector) { snapshot ->
                snapshot.nodesList.any { it.testId == "stub_scenario_text" }
            }
            val textRow = home.nodesList.first { it.testId == "stub_scenario_text" }
            assertTrue("the scenario row has no bounds", textRow.bounds.w > 0f && textRow.bounds.h > 0f)
            assertEquals("button", textRow.role)
            val tapped = inspector.input(
                InspectorInputRequest.newBuilder()
                    .setKind(InspectorInputKind.INSPECTOR_INPUT_KIND_TAP)
                    .setNodeId(textRow.id)
                    .build(),
            )
            assertTrue("tap failed: ${tapped.error.message}", tapped.ok)

            val textScreen = awaitSnapshot(inspector) { snapshot ->
                snapshot.nodesList.any { it.testId == "stub_input" }
            }
            val input = textScreen.nodesList.first { it.testId == "stub_input" }
            assertEquals("textbox", input.role)
            val typed = inspector.input(
                InspectorInputRequest.newBuilder()
                    .setKind(InspectorInputKind.INSPECTOR_INPUT_KIND_TYPE)
                    .setNodeId(input.id)
                    .setText("hello inspector")
                    .build(),
            )
            assertTrue("type failed: ${typed.error.message}", typed.ok)
            val after = awaitSnapshot(inspector) { snapshot ->
                snapshot.nodesList.any {
                    it.testId == "stub_input" && it.value.contains("hello inspector")
                }
            }
            val field = after.nodesList.first { it.testId == "stub_input" }
            assertEquals("hello inspector", field.value)
            assertNotNull("the echo label did not update", after.nodesList.firstOrNull {
                it.testId == "stub_echo" && it.text.contains("hello inspector")
            })

            // Screenshot-only must return a frame without acquiring a tree and
            // must honor the requested scale.
            val halfFrame = inspector.snapshot(
                InspectorSnapshotRequest.newBuilder()
                    .setScreenshotOnly(true)
                    .setScreenshotScale(50)
                    .build(),
            )
            assertEquals("screenshot-only failed: ${halfFrame.error.message}", 0, halfFrame.error.codeValue)
            assertEquals("screenshot-only returned nodes", 0, halfFrame.nodesCount)
            assertTrue("screenshot-only returned no PNG", !halfFrame.screenshotPng.isEmpty)
            val expectedWidth = (home.logicalWidth * home.pixelRatio * 0.5f).roundToInt()
            val expectedHeight = (home.logicalHeight * home.pixelRatio * 0.5f).roundToInt()
            assertTrue(
                "unexpected screenshot size ${halfFrame.screenshotWidth}x${halfFrame.screenshotHeight}, " +
                    "expected ${expectedWidth}x$expectedHeight",
                kotlin.math.abs(halfFrame.screenshotWidth - expectedWidth) <= 2 &&
                    kotlin.math.abs(halfFrame.screenshotHeight - expectedHeight) <= 2,
            )

            // KEY_BACK returns to the scenario list through the same input path.
            val back = inspector.input(
                InspectorInputRequest.newBuilder()
                    .setKind(InspectorInputKind.INSPECTOR_INPUT_KIND_KEY)
                    .setKey(RemoteKey.KEY_BACK)
                    .build(),
            )
            assertTrue("back failed: ${back.error.message}", back.ok)
            awaitSnapshot(inspector) { snapshot ->
                snapshot.nodesList.any { it.testId == "stub_scenario_text" }
            }

            // Scroll a real container by snapshot id and observe the content move.
            val scrollHome = awaitSnapshot(inspector) { snapshot ->
                snapshot.nodesList.any { it.testId == "stub_scenario_scroll" }
            }
            val scrollRow = scrollHome.nodesList.first { it.testId == "stub_scenario_scroll" }
            val opened = inspector.input(
                InspectorInputRequest.newBuilder()
                    .setKind(InspectorInputKind.INSPECTOR_INPUT_KIND_TAP)
                    .setNodeId(scrollRow.id)
                    .build(),
            )
            assertTrue("opening the scroller failed: ${opened.error.message}", opened.ok)
            val scroller = awaitSnapshot(inspector) { snapshot ->
                snapshot.nodesList.any { it.testId == "stub_scroller" }
            }
            val container = scroller.nodesList.first { it.testId == "stub_scroller" }
            val firstRow = firstVisibleRow(scroller)
            assertTrue("no visible row before scrolling", firstRow != null)
            val scrolled = inspector.input(
                InspectorInputRequest.newBuilder()
                    .setKind(InspectorInputKind.INSPECTOR_INPUT_KIND_SCROLL)
                    .setNodeId(container.id)
                    .setDirection(Direction.DOWN)
                    .setCount(2)
                    .build(),
            )
            assertTrue("scroll failed: ${scrolled.error.message}", scrolled.ok)
            val afterScroll = awaitSnapshot(inspector) { snapshot ->
                val next = firstVisibleRow(snapshot) ?: return@awaitSnapshot false
                next > (firstRow ?: 0)
            }
            assertTrue("the scroller did not move", (firstVisibleRow(afterScroll) ?: 0) > (firstRow ?: 0))
        } finally {
            channel.shutdownNow()
            server.shutdown()
        }
    }

    private fun awaitSnapshot(
        inspector: InspectorGrpc.InspectorBlockingStub,
        predicate: (InspectorSnapshotResponse) -> Boolean,
    ): InspectorSnapshotResponse {
        val deadline = System.currentTimeMillis() + 20_000
        var last: InspectorSnapshotResponse? = null
        while (System.currentTimeMillis() < deadline) {
            last = inspector.snapshot(InspectorSnapshotRequest.getDefaultInstance())
            if (last.error.codeValue != 0) {
                throw AssertionError("snapshot failed: ${last.error.message}")
            }
            if (predicate(last)) return last
            SystemClock.sleep(250)
        }
        throw AssertionError(
            "the expected node never appeared; last capture had ${last?.nodesCount ?: 0} nodes " +
                "(${last?.degradedList?.joinToString() ?: "no snapshot"})",
        )
    }

    private fun firstVisibleRow(snapshot: InspectorSnapshotResponse): Int? =
        snapshot.nodesList.mapNotNull { node ->
            Regex("^row (\\d+)$").find(node.text)?.groupValues?.get(1)?.toIntOrNull()
        }.minOrNull()

    private fun deviceIsTv(): Boolean {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        return context.packageManager.hasSystemFeature(android.content.pm.PackageManager.FEATURE_LEANBACK)
    }

    private companion object {
        const val STUB_PACKAGE = "com.impresario.stub"
        val SESSION_ID_KEY: Metadata.Key<String> =
            Metadata.Key.of("x-impresario-session-id", Metadata.ASCII_STRING_MARSHALLER)
    }
}
