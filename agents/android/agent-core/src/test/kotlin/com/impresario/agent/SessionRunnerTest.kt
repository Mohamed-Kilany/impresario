package com.impresario.agent

import impresario.v1.Impresario.ActionType
import impresario.v1.Impresario.Command
import impresario.v1.Impresario.ErrorCode
import impresario.v1.Impresario.Event
import impresario.v1.Impresario.KeyPress
import impresario.v1.Impresario.Platform
import impresario.v1.Impresario.RemoteKey
import impresario.v1.Impresario.RunAction
import impresario.v1.Impresario.Selector
import impresario.v1.Impresario.SelectorStrategy
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.consumeAsFlow
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class SessionRunnerTest {
    private val options = ExecutorOptions(
        platform = Platform.ANDROID,
        appId = "com.example.app",
        pollIntervalMs = 5,
        scrollMaxAttempts = 4,
        focusMaxSteps = 10,
    )

    private fun clickCommand(id: String, timeoutMs: Int = 300): Command =
        Command.newBuilder().setCommandId(id).setTimeoutMs(timeoutMs)
            .setRunAction(
                RunAction.newBuilder().setType(ActionType.CLICK)
                    .setSelector(Selector.newBuilder().setStrategy(SelectorStrategy.TEXT).setValue("OK")),
            )
            .build()

    private fun homeKeyCommand(id: String): Command =
        Command.newBuilder().setCommandId(id)
            .setKeyPress(KeyPress.newBuilder().setKey(RemoteKey.KEY_HOME))
            .build()

    private fun cancelCommand(id: String, target: String): Command =
        Command.newBuilder().setCommandId(id)
            .setCancel(impresario.v1.Impresario.Cancel.newBuilder().setTargetCommandId(target))
            .build()

    private class Harness(val driver: FakeDriver) {
        val events = Channel<Event>(Channel.UNLIMITED)
        val shutdown = CompletableDeferred<Unit>()
    }

    private fun startHarness(
        driver: FakeDriver,
        heartbeatMs: Long,
        commands: Channel<Command>,
    ): Harness {
        val harness = Harness(driver)
        val executor = CommandExecutor(driver, options)
        val runner = SessionRunner(
            executor,
            heartbeatIntervalMs = heartbeatMs,
            commandDispatcher = Dispatchers.Default,
        )
        CoroutineScope(Dispatchers.Default).launch {
            runner.run(commands.consumeAsFlow(), harness.shutdown) { event ->
                harness.events.send(event)
            }
        }
        return harness
    }

    private suspend fun Channel<Event>.resultFor(id: String): impresario.v1.Impresario.CommandResult {
        var found: impresario.v1.Impresario.CommandResult? = null
        withTimeout(10_000) {
            while (found == null) {
                val event = receive()
                if (event.hasResult() && event.result.commandId == id) found = event.result
            }
        }
        return found ?: throw AssertionError("no result for '$id' within 10s")
    }

    private suspend fun Channel<Event>.awaitClosed(): String {
        var reason: String? = null
        withTimeout(10_000) {
            while (reason == null) {
                val event = receive()
                if (event.hasClosed()) reason = event.closed.reason
            }
        }
        return reason ?: throw AssertionError("no SessionClosed event within 10s")
    }

    @Test
    fun resultsAreOrderedAndOnePerCommand() = runBlocking {
        val driver = FakeDriver()
        driver.blockFindMs = 60
        val commands = Channel<Command>(Channel.UNLIMITED)
        val harness = startHarness(driver, heartbeatMs = 60_000, commands = commands)
        commands.send(clickCommand("c1", timeoutMs = 250))
        delay(50)
        commands.send(homeKeyCommand("c2"))

        val first = harness.events.resultFor("c1")
        assertFalse(first.ok)
        assertEquals(ErrorCode.ELEMENT_NOT_FOUND, first.error.code)
        val second = harness.events.resultFor("c2")
        assertTrue(second.ok)

        // c2 only ran after c1 finished.
        val pressIndex = driver.calls.indexOf("pressKey:3:1:100")
        val lastFindIndex = driver.calls.indexOfLast { it.startsWith("find:") }
        assertTrue(pressIndex > lastFindIndex)

        harness.shutdown.complete(Unit)
        assertEquals("shutdown", harness.events.awaitClosed())
    }

    @Test
    fun cancelStopsTheInFlightCommand() = runBlocking {
        val driver = FakeDriver()
        driver.blockFindMs = 100
        val commands = Channel<Command>(Channel.UNLIMITED)
        val harness = startHarness(driver, heartbeatMs = 60_000, commands = commands)
        commands.send(clickCommand("c1", timeoutMs = 5000))
        delay(80)
        commands.send(cancelCommand("c2", "c1"))

        val ack = harness.events.resultFor("c2")
        assertTrue(ack.ok)
        val cancelled = harness.events.resultFor("c1")
        assertFalse(cancelled.ok)
        assertEquals(ErrorCode.CANCELLED, cancelled.error.code)

        harness.shutdown.complete(Unit)
        assertEquals("shutdown", harness.events.awaitClosed())
    }

    @Test
    fun cancelOfAnIdleCommandIsProtocol() = runBlocking {
        val driver = FakeDriver()
        val commands = Channel<Command>(Channel.UNLIMITED)
        val harness = startHarness(driver, heartbeatMs = 60_000, commands = commands)
        commands.send(cancelCommand("c1", "ghost"))
        val result = harness.events.resultFor("c1")
        assertFalse(result.ok)
        assertEquals(ErrorCode.PROTOCOL, result.error.code)
        harness.shutdown.complete(Unit)
        assertEquals("shutdown", harness.events.awaitClosed())
    }

    @Test
    fun cancelOfAFinishedCommandIsProtocol() = runBlocking {
        val driver = FakeDriver()
        driver.addNode("btn", selector = "text:OK")
        val commands = Channel<Command>(Channel.UNLIMITED)
        val harness = startHarness(driver, heartbeatMs = 60_000, commands = commands)
        commands.send(clickCommand("c1"))
        assertTrue(harness.events.resultFor("c1").ok)
        commands.send(cancelCommand("c2", "c1"))
        val result = harness.events.resultFor("c2")
        assertFalse(result.ok)
        assertEquals(ErrorCode.PROTOCOL, result.error.code)
        harness.shutdown.complete(Unit)
        assertEquals("shutdown", harness.events.awaitClosed())
    }

    @Test
    fun duplicateCommandIdIsProtocol() = runBlocking {
        val driver = FakeDriver()
        driver.addNode("btn", selector = "text:OK")
        val commands = Channel<Command>(Channel.UNLIMITED)
        val harness = startHarness(driver, heartbeatMs = 60_000, commands = commands)
        commands.send(clickCommand("c1"))
        commands.send(clickCommand("c1"))
        val results = withTimeout(10_000) {
            val collected = mutableListOf<impresario.v1.Impresario.CommandResult>()
            while (collected.size < 2) {
                val event = harness.events.receive()
                if (event.hasResult() && event.result.commandId == "c1") {
                    collected += event.result
                }
            }
            collected
        }
        assertEquals(1, results.count { it.ok })
        assertEquals(1, results.count { !it.ok && it.error.code == ErrorCode.PROTOCOL })
        harness.shutdown.complete(Unit)
        assertEquals("shutdown", harness.events.awaitClosed())
    }

    @Test
    fun shutdownWhileBusyCancelsTheCommandAndCloses() = runBlocking {
        val driver = FakeDriver()
        driver.blockFindMs = 100
        val commands = Channel<Command>(Channel.UNLIMITED)
        val harness = startHarness(driver, heartbeatMs = 60_000, commands = commands)
        commands.send(clickCommand("c1", timeoutMs = 5000))
        delay(80)
        harness.shutdown.complete(Unit)

        val cancelled = harness.events.resultFor("c1")
        assertFalse(cancelled.ok)
        assertEquals(ErrorCode.CANCELLED, cancelled.error.code)
        assertEquals("shutdown", harness.events.awaitClosed())
    }

    @Test
    fun clientDisconnectFinishesTheActiveCommandThenCloses() = runBlocking {
        val driver = FakeDriver()
        driver.addNode("btn", selector = "text:OK")
        driver.blockFindMs = 50
        val harness = Harness(driver)
        val executor = CommandExecutor(driver, options)
        val runner = SessionRunner(executor, heartbeatIntervalMs = 60_000, commandDispatcher = Dispatchers.Default)
        launch(Dispatchers.Default) {
            runner.run(
                flow {
                    emit(clickCommand("c1"))
                    emit(homeKeyCommand("c2"))
                },
                harness.shutdown,
            ) { event -> harness.events.send(event) }
        }

        assertTrue(harness.events.resultFor("c1").ok)
        assertTrue(harness.events.resultFor("c2").ok)
        assertEquals("client_closed", harness.events.awaitClosed())
    }

    @Test
    fun heartbeatsCarryTheActiveCommandId() = runBlocking {
        val driver = FakeDriver()
        driver.blockFindMs = 200
        val commands = Channel<Command>(Channel.UNLIMITED)
        val harness = startHarness(driver, heartbeatMs = 50, commands = commands)
        commands.send(clickCommand("c1", timeoutMs = 400))

        var heartbeat: impresario.v1.Impresario.Heartbeat? = null
        withTimeout(10_000) {
            while (heartbeat == null) {
                val event = harness.events.receive()
                if (event.hasHeartbeat() && event.heartbeat.currentCommandId == "c1") {
                    heartbeat = event.heartbeat
                }
            }
        }
        val beat = heartbeat ?: throw AssertionError("no heartbeat for c1 within 10s")
        assertTrue(beat.seq >= 1)

        harness.events.resultFor("c1")
        harness.shutdown.complete(Unit)
        assertEquals("shutdown", harness.events.awaitClosed())
    }
}
