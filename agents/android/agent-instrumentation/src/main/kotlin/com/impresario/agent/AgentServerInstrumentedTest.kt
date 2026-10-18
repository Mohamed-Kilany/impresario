package com.impresario.agent

import android.os.ParcelFileDescriptor
import androidx.test.platform.app.InstrumentationRegistry
import com.impresario.agent.driver.UiAutomatorDriver
import impresario.v1.AgentGrpc
import impresario.v1.Impresario.ActionType
import impresario.v1.Impresario.AppControl
import impresario.v1.Impresario.AppOp
import impresario.v1.Impresario.Cancel
import impresario.v1.Impresario.CheckKind
import impresario.v1.Impresario.Command
import impresario.v1.Impresario.CommandResult
import impresario.v1.Impresario.Direction as PbDirection
import impresario.v1.Impresario.ErrorCode
import impresario.v1.Impresario.Event
import impresario.v1.Impresario.HandshakeRequest
import impresario.v1.Impresario.KeyPress
import impresario.v1.Impresario.LogEntry
import impresario.v1.Impresario.LogLevel
import impresario.v1.Impresario.LogRequest
import impresario.v1.Impresario.Platform
import impresario.v1.Impresario.RemoteKey
import impresario.v1.Impresario.RunAction
import impresario.v1.Impresario.Screenshot
import impresario.v1.Impresario.ScreenshotRequest
import impresario.v1.Impresario.Selector
import impresario.v1.Impresario.SelectorStrategy
import impresario.v1.Impresario.ShutdownRequest
import impresario.v1.Impresario.ValidateCheck
import io.grpc.ManagedChannel
import io.grpc.Metadata
import io.grpc.Status
import io.grpc.netty.shaded.io.grpc.netty.NettyChannelBuilder
import io.grpc.stub.MetadataUtils
import io.grpc.stub.StreamObserver
import java.util.concurrent.CountDownLatch
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The real gRPC server (grpc-netty-shaded) running in the agent app's own
 * process on a real device, driven over a real channel by a real client, with
 * every command flowing through the dispatcher onto the stub app's UI
 * cross-app (docs/DECISIONS.md I-62).
 *
 * Platform branches: on Android TV the same commands run with the TV
 * semantics the executor implements (click = focus + OK, scroll = D-pad,
 * `ok`/color keys supported, swipe/focus differences), so one suite covers
 * both device classes.
 */
class AgentServerInstrumentedTest {
    private class EventCollector : StreamObserver<Event> {
        private val queue = LinkedBlockingQueue<Event>()
        private val completed = CountDownLatch(1)

        @Volatile
        private var failure: Throwable? = null

        override fun onNext(value: Event) {
            queue.add(value)
        }

        override fun onError(t: Throwable) {
            failure = t
            completed.countDown()
        }

        override fun onCompleted() {
            completed.countDown()
        }

        fun awaitResult(commandId: String, timeoutMs: Long = 20_000): CommandResult {
            val deadline = System.currentTimeMillis() + timeoutMs
            while (System.currentTimeMillis() < deadline) {
                failure?.let { throw AssertionError("the session stream failed", it) }
                val event = queue.poll(200, TimeUnit.MILLISECONDS) ?: continue
                if (event.hasResult() && event.result.commandId == commandId) return event.result
            }
            throw AssertionError("no result for '$commandId' within ${timeoutMs}ms")
        }

        fun awaitClosed(timeoutMs: Long = 20_000): String {
            val deadline = System.currentTimeMillis() + timeoutMs
            while (System.currentTimeMillis() < deadline) {
                val event = queue.poll(200, TimeUnit.MILLISECONDS) ?: continue
                if (event.hasClosed()) return event.closed.reason
            }
            throw AssertionError("no SessionClosed event within ${timeoutMs}ms (failure: $failure)")
        }

        fun awaitError(timeoutMs: Long = 20_000): Throwable {
            if (completed.await(timeoutMs, TimeUnit.MILLISECONDS)) {
                return failure ?: throw AssertionError("the stream completed instead of failing")
            }
            throw AssertionError("no stream error within ${timeoutMs}ms")
        }
    }

    /**
     * The spike shape: handshake, one command stream, a few commands,
     * screenshots (unary + in-command), logs, the second-stream precondition
     * and shutdown.
     */
    @Test
    fun grpcSessionDrivesTheStubAppOnDevice() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        ProcessGuard.ensure(instrumentation.targetContext)
        val driver = UiAutomatorDriver(instrumentation)
        val isTv = deviceIsTv()
        driver.terminate(STUB_PACKAGE)
        val state = AgentState(driver)
        val server = AgentServer(AgentServiceImpl(state), requestedPort = 0)
        server.start()
        val channel: ManagedChannel = NettyChannelBuilder
            .forAddress("127.0.0.1", server.port)
            .usePlaintext()
            .build()
        try {
            val blocking = AgentGrpc.newBlockingStub(channel)
            val handshake = blocking.handshake(
                HandshakeRequest.newBuilder()
                    .setProtocolVersion(PROTOCOL_VERSION)
                    .setMasterVersion("instrumented-test")
                    .setDeviceName(if (isTv) "android-tv" else "phone")
                    .setPlatform(if (isTv) Platform.ANDROID_TV else Platform.ANDROID)
                    .setAppId(STUB_PACKAGE)
                    .build(),
            )
            assertTrue("handshake rejected: ${handshake.error.message}", handshake.accepted)
            assertEquals("UiAutomator2", handshake.device.runtime)

            val headers = Metadata().apply {
                put(SESSION_ID_KEY, handshake.sessionId)
            }
            val attach = MetadataUtils.newAttachHeadersInterceptor(headers)
            val async = AgentGrpc.newStub(channel).withInterceptors(attach)
            val authenticatedBlocking = blocking.withInterceptors(attach)

            val collector = EventCollector()
            val requests = async.session(collector)

            requests.onNext(launchCommand("c1"))
            assertOk(collector.awaitResult("c1"), "launch")

            requests.onNext(checkCommand("c2", CheckKind.VISIBLE, androidId("stub_title")))
            assertOk(collector.awaitResult("c2"), "title visible")

            requests.onNext(clickCommand("c3", androidId("stub_scenario_basic")))
            assertOk(collector.awaitResult("c3"), "open Basic controls")

            requests.onNext(clickCommand("c4", androidId("stub_button")))
            assertOk(collector.awaitResult("c4"), "click")

            requests.onNext(
                checkCommand(
                    "c5",
                    CheckKind.TEXT_CONTAINS_CHECK,
                    androidId("stub_counter"),
                    expectedText = "taps=",
                ),
            )
            assertOk(collector.awaitResult("c5"), "counter text")

            requests.onNext(typeCommand("c6", androidId("stub_input"), "grpc-e2e", timeoutMs = 1_500))
            assertError(collector.awaitResult("c6"), ErrorCode.ELEMENT_NOT_FOUND, "input is not on this screen")

            requests.onNext(
                command("c7") {
                    setScreenshot(Screenshot.newBuilder().setLabel("instrumented"))
                },
            )
            val shot = collector.awaitResult("c7")
            assertOk(shot, "screenshot command")
            assertTrue("empty screenshot PNG", shot.screenshotPng.size() > 1_000)

            // Logs: the stream opens with its own marker entry (grpc-java
            // sends response headers only with the first message, so a lazy
            // client awaits that marker — docs/DECISIONS.md I-74), then it
            // carries entries from later RPCs.
            val logEntries = LinkedBlockingQueue<LogEntry>()
            async.streamLogs(
                LogRequest.newBuilder().setMinLevel(LogLevel.DEBUG).build(),
                object : StreamObserver<LogEntry> {
                    override fun onNext(value: LogEntry) {
                        logEntries.add(value)
                    }

                    override fun onError(t: Throwable) = Unit

                    override fun onCompleted() = Unit
                },
            )

            val opened = logEntries.poll(20, TimeUnit.SECONDS)
            assertNotNull("no log entries arrived through StreamLogs", opened)
            assertTrue(
                "stream did not open with its marker entry: ${opened!!.message}",
                opened.message.contains("log stream opened"),
            )

            val direct = authenticatedBlocking.captureScreenshot(ScreenshotRequest.getDefaultInstance())
            assertEquals("unary screenshot failed: ${direct.error.message}", 0, direct.error.codeValue)
            assertTrue("empty unary screenshot PNG", direct.png.size() > 1_000)
            assertTrue(direct.width > 0 && direct.height > 0)

            val logEntry = logEntries.poll(20, TimeUnit.SECONDS)
            assertNotNull("no log entries arrived through StreamLogs", logEntry)
            assertTrue(logEntry!!.message.contains("screenshot"))

            // A second command stream on the same session is a precondition failure.
            val secondCollector = EventCollector()
            async.session(secondCollector)
            val streamFailure = secondCollector.awaitError()
            assertEquals(
                Status.Code.FAILED_PRECONDITION,
                Status.fromThrowable(streamFailure).code,
            )

            authenticatedBlocking.shutdown(ShutdownRequest.getDefaultInstance())
            assertEquals("shutdown", collector.awaitClosed())
            requests.onCompleted()
        } finally {
            channel.shutdownNow()
            server.shutdown()
        }
    }

    /**
     * The full action/check matrix over a real gRPC session on a real device
     * (spec 6.1): every action the platform supports, every check kind, all
     * Android selector strategies, index selection, the whole app-control
     * lifecycle (launch/terminate/restart/clear-data/background — the agent
     * survives all of them), error mapping and cancellation.
     */
    @Test
    @Suppress("LongMethod")
    fun everyActionAndCheckRunsOverTheWire() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        ProcessGuard.ensure(instrumentation.targetContext)
        val driver = UiAutomatorDriver(instrumentation)
        val isTv = deviceIsTv()
        driver.terminate(STUB_PACKAGE)
        val state = AgentState(driver)
        val server = AgentServer(AgentServiceImpl(state), requestedPort = 0)
        server.start()
        val channel: ManagedChannel = NettyChannelBuilder
            .forAddress("127.0.0.1", server.port)
            .usePlaintext()
            .build()
        try {
            // The 30-row scroller fixture needs more than the default 10-step
            // budget: the TV path splits the budget in half (docs/DECISIONS.md
            // I-45/I-70), and reaching row 0 from the end takes ~9 up-presses.
            val handshake = AgentGrpc.newBlockingStub(channel).handshake(
                HandshakeRequest.newBuilder()
                    .setProtocolVersion(PROTOCOL_VERSION)
                    .setMasterVersion("instrumented-test")
                    .setDeviceName(if (isTv) "android-tv" else "phone")
                    .setPlatform(if (isTv) Platform.ANDROID_TV else Platform.ANDROID)
                    .setAppId(STUB_PACKAGE)
                    .putOptions("scroll_max_attempts", "30")
                    .build(),
            )
            assertTrue("handshake rejected: ${handshake.error.message}", handshake.accepted)

            val headers = Metadata().apply {
                put(SESSION_ID_KEY, handshake.sessionId)
            }
            val attach = MetadataUtils.newAttachHeadersInterceptor(headers)
            val async = AgentGrpc.newStub(channel).withInterceptors(attach)
            val collector = EventCollector()
            val requests = async.session(collector)
            val auth = AgentGrpc.newBlockingStub(channel).withInterceptors(attach)

            // -- app lifecycle: terminate (agent survives), then launch ----
            requests.onNext(terminateCommand("a1"))
            assertOk(collector.awaitResult("a1"), "terminate")
            requests.onNext(launchCommand("a2"))
            assertOk(collector.awaitResult("a2"), "launch")
            requests.onNext(checkCommand("a3", CheckKind.VISIBLE, androidId("stub_title")))
            assertOk(collector.awaitResult("a3"), "home title visible")
            requests.onNext(
                checkCommand("a4", CheckKind.TEXT_EQUALS, androidId("stub_title"), expectedText = "Impresario stub"),
            )
            assertOk(collector.awaitResult("a4"), "home title text")
            requests.onNext(
                checkCommand("a5", CheckKind.COUNT, clazz("android.widget.Button"), expectedCount = 7),
            )
            assertOk(collector.awaitResult("a5"), "seven scenario rows")

            // -- basic screen: click, long press, counter, status ----------
            requests.onNext(clickCommand("a6", androidId("stub_scenario_basic")))
            assertOk(collector.awaitResult("a6"), "open Basic controls")
            requests.onNext(
                checkCommand("a7", CheckKind.TEXT_EQUALS, androidId("stub_title"), expectedText = "Basic controls"),
            )
            assertOk(collector.awaitResult("a7"), "basic title")
            requests.onNext(clickCommand("a8", androidId("stub_button")))
            assertOk(collector.awaitResult("a8"), "click")
            requests.onNext(
                checkCommand("a9", CheckKind.TEXT_EQUALS, androidId("stub_counter"), expectedText = "taps=1"),
            )
            assertOk(collector.awaitResult("a9"), "tap counter")
            requests.onNext(
                checkCommand("a10", CheckKind.TEXT_EQUALS, androidId("stub_status"), expectedText = "status=clicked"),
            )
            assertOk(collector.awaitResult("a10"), "status after click")
            requests.onNext(checkCommand("a11", CheckKind.COUNT, textContains("taps="), expectedCount = 1))
            assertOk(collector.awaitResult("a11"), "counter count")
            requests.onNext(longPressCommand("a12", androidId("stub_button"), 1_200))
            assertOk(collector.awaitResult("a12"), "long press")
            requests.onNext(
                checkCommand("a13", CheckKind.TEXT_EQUALS, androidId("stub_status"), expectedText = "status=long-pressed"),
            )
            assertOk(collector.awaitResult("a13"), "status after long press")
            requests.onNext(backCommand("a14"))
            assertOk(collector.awaitResult("a14"), "back to home")
            requests.onNext(
                checkCommand("a15", CheckKind.TEXT_EQUALS, androidId("stub_title"), expectedText = "Impresario stub"),
            )
            assertOk(collector.awaitResult("a15"), "home after back")

            // -- flags screen: enabled/disabled/hidden, counts, indexes ----
            requests.onNext(clickCommand("a16", androidId("stub_scenario_flags")))
            assertOk(collector.awaitResult("a16"), "open Flags")
            requests.onNext(
                checkCommand("a17", CheckKind.TEXT_EQUALS, androidId("stub_title"), expectedText = "Flags"),
            )
            assertOk(collector.awaitResult("a17"), "flags title")
            requests.onNext(checkCommand("a18", CheckKind.ENABLED, androidId("stub_enabled")))
            assertOk(collector.awaitResult("a18"), "enabled button")
            requests.onNext(checkCommand("a19", CheckKind.DISABLED, androidId("stub_disabled")))
            assertOk(collector.awaitResult("a19"), "disabled button")
            requests.onNext(checkCommand("a20", CheckKind.VISIBLE, androidId("stub_visible")))
            assertOk(collector.awaitResult("a20"), "visible label")
            requests.onNext(checkCommand("a21", CheckKind.HIDDEN, androidId("stub_hidden")))
            assertOk(collector.awaitResult("a21"), "invisible label is hidden")
            requests.onNext(
                checkCommand("a22", CheckKind.VISIBLE, androidId("stub_hidden"), timeoutMs = 1_500),
            )
            assertError(collector.awaitResult("a22"), ErrorCode.CHECK_FAILED, "invisible label visible")
            requests.onNext(
                checkCommand("a23", CheckKind.COUNT, clazz("android.widget.Button"), expectedCount = 2),
            )
            assertOk(collector.awaitResult("a23"), "button count on the flags screen")
            requests.onNext(
                checkCommand("a24", CheckKind.TEXT_EQUALS, textContains("able", index = 0), expectedText = "Enabled"),
            )
            assertOk(collector.awaitResult("a24"), "first 'able' match by index")
            requests.onNext(
                checkCommand("a25", CheckKind.TEXT_EQUALS, textContains("able", index = 1), expectedText = "Disabled"),
            )
            assertOk(collector.awaitResult("a25"), "second 'able' match by index")
            requests.onNext(
                checkCommand("a26", CheckKind.COUNT, androidId("stub_missing"), expectedCount = 0),
            )
            assertOk(collector.awaitResult("a26"), "absent selector counts zero")
            requests.onNext(clickCommand("a27", androidId("stub_disabled"), timeoutMs = 1_500))
            assertError(collector.awaitResult("a27"), ErrorCode.NOT_INTERACTABLE, "click the disabled button")
            requests.onNext(clickCommand("a28", androidId("stub_enabled")))
            assertOk(collector.awaitResult("a28"), "click the enabled button")
            requests.onNext(
                checkCommand("a29", CheckKind.TEXT_EQUALS, androidId("stub_status"), expectedText = "status=enabled-clicked"),
            )
            assertOk(collector.awaitResult("a29"), "status after enabled click")
            requests.onNext(backCommand("a30"))
            assertOk(collector.awaitResult("a30"), "back from Flags")

            // -- scroller: scroll, scroll_to, swipes (phones only) ---------
            requests.onNext(clickCommand("a31", androidId("stub_scenario_scroll")))
            assertOk(collector.awaitResult("a31"), "open Scroller")
            requests.onNext(
                checkCommand("a32", CheckKind.TEXT_EQUALS, androidId("stub_title"), expectedText = "Scroller"),
            )
            assertOk(collector.awaitResult("a32"), "scroller title")
            requests.onNext(scrollCommand("a33", androidId("stub_scroller"), PbDirection.DOWN, count = 2))
            assertOk(collector.awaitResult("a33"), "scroll down")
            requests.onNext(scrollToCommand("a34", textSelector("row 29")))
            assertOk(collector.awaitResult("a34"), "scroll_to the last row")
            requests.onNext(checkCommand("a35", CheckKind.VISIBLE, textSelector("row 29")))
            assertOk(collector.awaitResult("a35"), "last row visible")
            requests.onNext(scrollToCommand("a36", textSelector("row 0")))
            assertOk(collector.awaitResult("a36"), "scroll_to the first row")
            requests.onNext(checkCommand("a37", CheckKind.VISIBLE, textSelector("row 0")))
            assertOk(collector.awaitResult("a37"), "first row visible")
            if (isTv) {
                requests.onNext(swipeCommand("a38", androidId("stub_scroller"), PbDirection.DOWN))
                assertError(collector.awaitResult("a38"), ErrorCode.UNSUPPORTED_ACTION, "swipe on TV")
                requests.onNext(swipeCommand("a39", null, PbDirection.UP))
                assertError(collector.awaitResult("a39"), ErrorCode.UNSUPPORTED_ACTION, "screen swipe on TV")
            } else {
                requests.onNext(swipeCommand("a38", androidId("stub_scroller"), PbDirection.DOWN))
                assertOk(collector.awaitResult("a38"), "swipe the scroller down")
                requests.onNext(swipeCommand("a39", null, PbDirection.UP))
                assertOk(collector.awaitResult("a39"), "screen swipe")
                requests.onNext(scrollToCommand("a40", textSelector("row 29")))
                assertOk(collector.awaitResult("a40"), "scroll_to after swipes")
            }
            requests.onNext(backCommand("a41"))
            assertOk(collector.awaitResult("a41"), "back from Scroller")

            // -- text entry: type, append, clear, backspace ----------------
            requests.onNext(clickCommand("a42", androidId("stub_scenario_text")))
            assertOk(collector.awaitResult("a42"), "open Text entry")
            requests.onNext(
                checkCommand("a43", CheckKind.TEXT_EQUALS, androidId("stub_title"), expectedText = "Text entry"),
            )
            assertOk(collector.awaitResult("a43"), "text title")
            requests.onNext(typeCommand("a44", androidId("stub_input"), "abc"))
            assertOk(collector.awaitResult("a44"), "type")
            requests.onNext(
                checkCommand("a45", CheckKind.TEXT_EQUALS, androidId("stub_echo"), expectedText = "echo:abc"),
            )
            assertOk(collector.awaitResult("a45"), "echo after type")
            requests.onNext(appendCommand("a46", androidId("stub_input"), "def"))
            assertOk(collector.awaitResult("a46"), "append")
            requests.onNext(
                checkCommand("a47", CheckKind.TEXT_EQUALS, androidId("stub_echo"), expectedText = "echo:abcdef"),
            )
            assertOk(collector.awaitResult("a47"), "echo after append")
            requests.onNext(clearCommand("a48", androidId("stub_input")))
            assertOk(collector.awaitResult("a48"), "clear")
            requests.onNext(
                checkCommand("a49", CheckKind.TEXT_EQUALS, androidId("stub_echo"), expectedText = "echo:"),
            )
            assertOk(collector.awaitResult("a49"), "echo after clear")
            requests.onNext(typeCommand("a50", androidId("stub_input"), "xy"))
            assertOk(collector.awaitResult("a50"), "type again")
            requests.onNext(keyPressCommand("a51", RemoteKey.KEY_BACKSPACE))
            assertOk(collector.awaitResult("a51"), "backspace")
            requests.onNext(
                checkCommand("a52", CheckKind.TEXT_EQUALS, androidId("stub_input"), expectedText = "x"),
            )
            assertOk(collector.awaitResult("a52"), "field text after backspace")
            requests.onNext(checkCommand("a53", CheckKind.FOCUSED, androidId("stub_input")))
            assertOk(collector.awaitResult("a53"), "field focused after typing")
            dismissIme()
            requests.onNext(backCommand("a54"))
            assertOk(collector.awaitResult("a54"), "back from Text entry")

            // -- select: dialog trigger plus option pick -------------------
            requests.onNext(clickCommand("a55", androidId("stub_scenario_select")))
            assertOk(collector.awaitResult("a55"), "open Selection")
            requests.onNext(selectCommand("a56", androidId("stub_select"), "Option B"))
            assertOk(collector.awaitResult("a56"), "select")
            requests.onNext(
                checkCommand("a57", CheckKind.TEXT_EQUALS, androidId("stub_status"), expectedText = "chosen: Option B"),
            )
            assertOk(collector.awaitResult("a57"), "status after select")
            requests.onNext(backCommand("a58"))
            assertOk(collector.awaitResult("a58"), "back from Selection")

            // -- keys screen: D-pad focus moves, media keys, TV-only keys --
            requests.onNext(clickCommand("a59", androidId("stub_scenario_keys")))
            assertOk(collector.awaitResult("a59"), "open Key log")
            requests.onNext(checkCommand("a60", CheckKind.FOCUSED, androidId("stub_key_a")))
            assertOk(collector.awaitResult("a60"), "Alpha focused on entry")
            requests.onNext(keyPressCommand("a61", RemoteKey.KEY_DOWN))
            assertOk(collector.awaitResult("a61"), "down")
            requests.onNext(checkCommand("a62", CheckKind.FOCUSED, androidId("stub_key_b")))
            assertOk(collector.awaitResult("a62"), "Bravo focused after down")
            requests.onNext(
                checkCommand("a63", CheckKind.TEXT_CONTAINS_CHECK, androidId("stub_key_log"), expectedText = "key=DPAD_DOWN"),
            )
            assertOk(collector.awaitResult("a63"), "key log shows the down press")
            requests.onNext(keyPressCommand("a64", RemoteKey.KEY_UP))
            assertOk(collector.awaitResult("a64"), "up")
            requests.onNext(checkCommand("a65", CheckKind.FOCUSED, androidId("stub_key_a")))
            assertOk(collector.awaitResult("a65"), "Alpha focused after up")
            requests.onNext(keyPressCommand("a66", RemoteKey.KEY_PLAYPAUSE))
            assertOk(collector.awaitResult("a66"), "playpause")
            requests.onNext(
                checkCommand("a67", CheckKind.TEXT_CONTAINS_CHECK, androidId("stub_key_log"), expectedText = "key=MEDIA_PLAY_PAUSE"),
            )
            assertOk(collector.awaitResult("a67"), "key log shows the media key")
            if (isTv) {
                requests.onNext(keyPressCommand("a68", RemoteKey.KEY_OK))
                assertOk(collector.awaitResult("a68"), "ok key on tv")
                requests.onNext(
                    checkCommand("a69", CheckKind.TEXT_CONTAINS_CHECK, androidId("stub_key_log"), expectedText = "clicked=Alpha"),
                )
                assertOk(collector.awaitResult("a69"), "ok activates the focused button")
                requests.onNext(keyPressCommand("a70", RemoteKey.KEY_RED))
                assertOk(collector.awaitResult("a70"), "red key on tv")
                requests.onNext(
                    checkCommand("a71", CheckKind.TEXT_CONTAINS_CHECK, androidId("stub_key_log"), expectedText = "key=PROG_RED"),
                )
                assertOk(collector.awaitResult("a71"), "key log shows the color key")
            } else {
                requests.onNext(keyPressCommand("a68", RemoteKey.KEY_OK))
                assertError(collector.awaitResult("a68"), ErrorCode.UNSUPPORTED_ACTION, "ok key on a phone")
                requests.onNext(keyPressCommand("a70", RemoteKey.KEY_RED))
                assertError(collector.awaitResult("a70"), ErrorCode.UNSUPPORTED_ACTION, "color key on a phone")
            }
            requests.onNext(backCommand("a72"))
            assertOk(collector.awaitResult("a72"), "back from Key log")

            // -- focus action (TV) and negative cases ----------------------
            if (isTv) {
                requests.onNext(focusCommand("a73", androidId("stub_scenario_flags")))
                assertOk(collector.awaitResult("a73"), "focus on tv")
                requests.onNext(checkCommand("a74", CheckKind.FOCUSED, androidId("stub_scenario_flags")))
                assertOk(collector.awaitResult("a74"), "row focused after the focus action")
            } else {
                requests.onNext(focusCommand("a73", androidId("stub_scenario_flags")))
                assertError(collector.awaitResult("a73"), ErrorCode.UNSUPPORTED_ACTION, "focus on a phone")
            }
            requests.onNext(
                checkCommand("a75", CheckKind.VISIBLE, css(".anything"), timeoutMs = 1_500),
            )
            assertError(collector.awaitResult("a75"), ErrorCode.INVALID_SELECTOR, "css selector on android")
            requests.onNext(clickCommand("a76", androidId("stub_missing"), timeoutMs = 1_500))
            assertError(collector.awaitResult("a76"), ErrorCode.ELEMENT_NOT_FOUND, "click a missing element")

            // -- background, restart, clear-data (the agent survives) ------
            requests.onNext(backgroundCommand("a77"))
            assertOk(collector.awaitResult("a77"), "background")
            requests.onNext(
                checkCommand("a78", CheckKind.VISIBLE, androidId("stub_title"), timeoutMs = 1_500),
            )
            assertError(collector.awaitResult("a78"), ErrorCode.CHECK_FAILED, "stub visible while backgrounded")
            requests.onNext(launchCommand("a79"))
            assertOk(collector.awaitResult("a79"), "relaunch after background")
            requests.onNext(
                checkCommand("a80", CheckKind.TEXT_EQUALS, androidId("stub_title"), expectedText = "Impresario stub"),
            )
            assertOk(collector.awaitResult("a80"), "home after relaunch")
            requests.onNext(clickCommand("a81", androidId("stub_scenario_basic")))
            assertOk(collector.awaitResult("a81"), "open Basic controls again")
            requests.onNext(clickCommand("a82", androidId("stub_button")))
            assertOk(collector.awaitResult("a82"), "click again")
            requests.onNext(
                checkCommand("a83", CheckKind.TEXT_EQUALS, androidId("stub_counter"), expectedText = "taps=1"),
            )
            assertOk(collector.awaitResult("a83"), "counter before restart")
            requests.onNext(restartCommand("a84"))
            assertOk(collector.awaitResult("a84"), "restart (the agent survives)")
            requests.onNext(
                checkCommand("a85", CheckKind.TEXT_EQUALS, androidId("stub_title"), expectedText = "Impresario stub"),
            )
            assertOk(collector.awaitResult("a85"), "home after restart")
            requests.onNext(clickCommand("a86", androidId("stub_scenario_basic")))
            assertOk(collector.awaitResult("a86"), "open Basic controls after restart")
            requests.onNext(
                checkCommand("a87", CheckKind.TEXT_EQUALS, androidId("stub_counter"), expectedText = "taps=0"),
            )
            assertOk(collector.awaitResult("a87"), "restart reset the counter")
            requests.onNext(backCommand("a88"))
            assertOk(collector.awaitResult("a88"), "back before the clear-data restart")
            requests.onNext(restartCommand("a89", clearData = true))
            assertOk(collector.awaitResult("a89"), "restart with clear-data")
            requests.onNext(
                checkCommand("a90", CheckKind.TEXT_EQUALS, androidId("stub_title"), expectedText = "Impresario stub"),
            )
            assertOk(collector.awaitResult("a90"), "home after clear-data restart")

            // -- cancel: ack the cancel, the target reports CANCELLED ------
            requests.onNext(clickCommand("a91", androidId("stub_missing"), timeoutMs = 30_000))
            requests.onNext(cancelCommand("a92", "a91"))
            assertOk(collector.awaitResult("a92", 10_000), "cancel ack")
            assertError(collector.awaitResult("a91", 20_000), ErrorCode.CANCELLED, "cancelled long wait")
            requests.onNext(checkCommand("a93", CheckKind.VISIBLE, androidId("stub_title")))
            assertOk(collector.awaitResult("a93"), "session alive after cancel")

            auth.shutdown(ShutdownRequest.getDefaultInstance())
            assertEquals("shutdown", collector.awaitClosed())
            requests.onCompleted()
        } finally {
            channel.shutdownNow()
            server.shutdown()
        }
    }

    // ------------------------------------------------------------------
    // helpers
    // ------------------------------------------------------------------

    private fun deviceIsTv(): Boolean {
        val descriptor = InstrumentationRegistry.getInstrumentation().uiAutomation
            .executeShellCommand("getprop ro.build.characteristics")
        return ParcelFileDescriptor.AutoCloseInputStream(descriptor).bufferedReader()
            .use { it.readText() }
            .contains("tv")
    }

    private fun command(
        id: String,
        timeoutMs: Int = 15_000,
        body: Command.Builder.() -> Unit,
    ): Command {
        val builder = Command.newBuilder().setCommandId(id).setTimeoutMs(timeoutMs)
        builder.body()
        return builder.build()
    }

    private fun launchCommand(id: String): Command = command(id) {
        setAppControl(AppControl.newBuilder().setOp(AppOp.LAUNCH))
    }

    private fun terminateCommand(id: String): Command = command(id) {
        setAppControl(AppControl.newBuilder().setOp(AppOp.TERMINATE))
    }

    private fun restartCommand(id: String, clearData: Boolean = false): Command = command(id) {
        setAppControl(AppControl.newBuilder().setOp(AppOp.RESTART).setClearData(clearData))
    }

    private fun backgroundCommand(id: String): Command = command(id) {
        setAppControl(AppControl.newBuilder().setOp(AppOp.BACKGROUND))
    }

    private fun backCommand(id: String): Command = command(id) {
        setRunAction(RunAction.newBuilder().setType(ActionType.BACK))
    }

    private fun focusCommand(id: String, selector: Selector): Command = command(id) {
        setRunAction(RunAction.newBuilder().setType(ActionType.FOCUS).setSelector(selector))
    }

    private fun cancelCommand(id: String, target: String): Command = command(id) {
        setCancel(Cancel.newBuilder().setTargetCommandId(target))
    }

    private fun longPressCommand(id: String, selector: Selector, durationMs: Int): Command =
        command(id) {
            setRunAction(
                RunAction.newBuilder().setType(ActionType.LONG_PRESS)
                    .setSelector(selector)
                    .setDurationMs(durationMs),
            )
        }

    private fun scrollCommand(
        id: String,
        container: Selector,
        direction: PbDirection,
        count: Int,
    ): Command = command(id) {
        setRunAction(
            RunAction.newBuilder().setType(ActionType.SCROLL)
                .setSelector(container)
                .setDirection(direction)
                .setCount(count),
        )
    }

    private fun scrollToCommand(id: String, selector: Selector): Command = command(id) {
        setRunAction(RunAction.newBuilder().setType(ActionType.SCROLL_TO).setSelector(selector))
    }

    private fun swipeCommand(id: String, selector: Selector?, direction: PbDirection): Command =
        command(id) {
            val action = RunAction.newBuilder().setType(ActionType.SWIPE).setDirection(direction)
            if (selector != null) action.setSelector(selector)
            setRunAction(action)
        }

    private fun appendCommand(id: String, selector: Selector, text: String): Command = command(id) {
        setRunAction(
            RunAction.newBuilder().setType(ActionType.APPEND).setSelector(selector).setText(text),
        )
    }

    private fun clearCommand(id: String, selector: Selector): Command = command(id) {
        setRunAction(RunAction.newBuilder().setType(ActionType.CLEAR).setSelector(selector))
    }

    private fun selectCommand(id: String, selector: Selector, option: String): Command = command(id) {
        setRunAction(
            RunAction.newBuilder().setType(ActionType.SELECT).setSelector(selector).setText(option),
        )
    }

    private fun keyPressCommand(id: String, key: RemoteKey): Command = command(id) {
        setKeyPress(KeyPress.newBuilder().setKey(key))
    }

    private fun clickCommand(id: String, selector: Selector, timeoutMs: Int = 15_000): Command =
        command(id, timeoutMs) {
            setRunAction(RunAction.newBuilder().setType(ActionType.CLICK).setSelector(selector))
        }

    private fun typeCommand(
        id: String,
        selector: Selector,
        text: String,
        timeoutMs: Int = 15_000,
    ): Command = command(id, timeoutMs) {
        setRunAction(RunAction.newBuilder().setType(ActionType.TYPE).setSelector(selector).setText(text))
    }

    private fun checkCommand(
        id: String,
        kind: CheckKind,
        selector: Selector,
        expectedText: String = "",
        expectedCount: Int = 0,
        timeoutMs: Int = 15_000,
    ): Command = command(id, timeoutMs) {
        val check = ValidateCheck.newBuilder().setKind(kind).setSelector(selector)
        if (expectedText.isNotEmpty()) check.setExpectedText(expectedText)
        if (expectedCount != 0) check.setExpectedCount(expectedCount)
        setValidateCheck(check)
    }

    private fun assertOk(result: CommandResult, what: String) {
        assertTrue("$what failed: ${result.error.code} ${result.error.message}", result.ok)
    }

    private fun assertError(result: CommandResult, code: ErrorCode, what: String) {
        assertTrue("$what should fail but passed", !result.ok)
        assertEquals("$what error code", code, result.error.code)
    }

    /**
     * Dismisses the soft keyboard so the following `back` reaches the activity.
     * On this TV the leanback IME ignores ESC (111) and consumes the *first*
     * BACK (I-73); BACK here mirrors the user's first press, the command's own
     * `back` becomes the second.
     */
    private fun dismissIme() {
        InstrumentationRegistry.getInstrumentation().uiAutomation
            .executeShellCommand("input keyevent 4")
            .close()
    }

    /** Resource ids at runtime are `applicationId:id/name` (docs/DECISIONS.md I-58). */
    private fun androidId(name: String, index: Int = 0): Selector =
        selector(SelectorStrategy.ANDROID_ID, "$STUB_PACKAGE:id/$name", index)

    private fun textSelector(value: String, index: Int = 0): Selector =
        selector(SelectorStrategy.TEXT, value, index)

    private fun textContains(value: String, index: Int = 0): Selector =
        selector(SelectorStrategy.TEXT_CONTAINS, value, index)

    private fun clazz(name: String, index: Int = 0): Selector =
        selector(SelectorStrategy.ANDROID_CLASS, name, index)

    private fun css(value: String): Selector =
        selector(SelectorStrategy.CSS, value, 0)

    private fun selector(strategy: SelectorStrategy, value: String, index: Int): Selector =
        Selector.newBuilder()
            .setStrategy(strategy)
            .setValue(value)
            .setIndex(index)
            .build()

    private companion object {
        /** The installed stub app id (`applicationId`; `launch` takes it, ids live under it). */
        const val STUB_PACKAGE = "com.impresario.stub"

        val SESSION_ID_KEY: Metadata.Key<String> =
            Metadata.Key.of("x-impresario-session-id", Metadata.ASCII_STRING_MARSHALLER)
    }
}
