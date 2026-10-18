package com.impresario.agent

import androidx.test.platform.app.InstrumentationRegistry
import com.impresario.agent.driver.UiAutomatorDriver
import impresario.v1.Impresario.LogLevel
import kotlinx.coroutines.runBlocking
import org.junit.Test

/**
 * The entry point the master's `AdbTransport` starts (spec 10.3, 11.2):
 *
 * ```
 * adb -s <serial> shell am instrument -w -r \
 *   -e class com.impresario.agent.AgentRunner \
 *   -e port <agent_port> \
 *   com.impresario.agent.test/androidx.test.runner.AndroidJUnitRunner
 * ```
 *
 * Starts the gRPC server over the UiAutomator driver and blocks until a
 * `Shutdown` RPC arrives (or the host kills the instrumentation).
 */
class AgentRunner {
    @Test
    fun serve() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        // Keep vendor power managers away from this process for the whole
        // session (docs/DECISIONS.md I-68).
        ProcessGuard.ensure(instrumentation.targetContext)
        val arguments = InstrumentationRegistry.getArguments()
        val port = arguments.getString("port")?.toIntOrNull() ?: DEFAULT_PORT
        val token = arguments.getString("token") ?: ""
        // Binds every interface by default (the master dials the device over
        // the network); `-e bind 127.0.0.1` restricts it if ever needed.
        val bind = arguments.getString("bind") ?: "0.0.0.0"

        val driver = UiAutomatorDriver(instrumentation)
        val state = AgentState(driver, expectedToken = token)
        val server = AgentServer(AgentServiceImpl(state), port, bind)

        state.logBus.log(LogLevel.INFO, "agent", "impresario android agent starting on $bind:$port")
        server.start()
        state.logBus.log(LogLevel.INFO, "agent", "listening on $bind:${server.port}")
        try {
            runBlocking { state.shutdownRequested.await() }
        } finally {
            state.logBus.log(LogLevel.INFO, "agent", "shutting down")
            server.shutdown()
        }
    }

    private companion object {
        const val DEFAULT_PORT = 50061
    }
}
