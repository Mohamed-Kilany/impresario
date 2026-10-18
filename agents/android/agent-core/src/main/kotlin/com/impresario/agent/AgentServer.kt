package com.impresario.agent

import io.grpc.Server
import io.grpc.netty.shaded.io.grpc.netty.NettyServerBuilder
import java.net.InetSocketAddress
import java.util.concurrent.TimeUnit

/**
 * The in-instrumentation gRPC server (spec 11.2, the Phase 0 spike that proves
 * it runs on a device). It binds every interface so the master reaches it over
 * the local network; loopback always works for in-process tests and
 * `adb forward` remains the fallback for devices without a network address
 * (docs/DECISIONS.md I-62).
 */
class AgentServer(
    private val service: AgentServiceImpl,
    /** The requested port: `0` asks the OS for a free ephemeral port. */
    private val requestedPort: Int = 0,
    private val host: String = "0.0.0.0",
) {
    private var server: Server? = null

    /** The bound port (`0` asks the OS for a free one); valid after [start]. */
    val port: Int get() = server?.port ?: -1

    fun start() {
        val started = NettyServerBuilder.forAddress(InetSocketAddress(host, requestedPort))
            .addService(service)
            .intercept(AuthInterceptor())
            .maxInboundMessageSize(16 * 1024 * 1024)
            .keepAliveTime(10, TimeUnit.SECONDS)
            .keepAliveTimeout(5, TimeUnit.SECONDS)
            .permitKeepAliveWithoutCalls(true)
            .build()
            .start()
        server = started
    }

    fun awaitTermination(timeoutMs: Long = 0) {
        server?.awaitTermination(timeoutMs, TimeUnit.MILLISECONDS)
    }

    fun shutdown() {
        val running = server ?: return
        running.shutdown()
        if (!running.awaitTermination(5, TimeUnit.SECONDS)) {
            running.shutdownNow()
            running.awaitTermination(2, TimeUnit.SECONDS)
        }
    }
}
