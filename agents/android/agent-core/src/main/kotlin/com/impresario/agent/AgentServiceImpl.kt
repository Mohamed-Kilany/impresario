package com.impresario.agent

import com.google.protobuf.ByteString
import impresario.v1.AgentGrpcKt
import impresario.v1.Impresario.Capability
import impresario.v1.Impresario.Command
import impresario.v1.Impresario.DeviceInfo
import impresario.v1.Impresario.ErrorCode
import impresario.v1.Impresario.Event
import impresario.v1.Impresario.HandshakeRequest
import impresario.v1.Impresario.HandshakeResponse
import impresario.v1.Impresario.LogEntry
import impresario.v1.Impresario.LogLevel
import impresario.v1.Impresario.LogRequest
import impresario.v1.Impresario.Platform
import impresario.v1.Impresario.ScreenshotRequest
import impresario.v1.Impresario.ScreenshotResponse
import impresario.v1.Impresario.ShutdownRequest
import impresario.v1.Impresario.ShutdownResponse
import io.grpc.Context
import io.grpc.Contexts
import io.grpc.Metadata
import io.grpc.ServerCall
import io.grpc.ServerCallHandler
import io.grpc.ServerInterceptor
import io.grpc.Status
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.channelFlow
import kotlinx.coroutines.flow.emitAll
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.withContext
import java.util.UUID

/** Wire protocol version spoken by the Android agent. */
const val PROTOCOL_VERSION: String = "1.0.0"

/** True when the major part of [version] matches [PROTOCOL_VERSION]. */
fun protocolMajorCompatible(version: String): Boolean {
    val major = version.substringBefore('.').toIntOrNull() ?: return false
    return major == PROTOCOL_VERSION.substringBefore('.').toInt()
}

/**
 * Session bookkeeping for one agent process (spec 11.2): exactly one session,
 * created by a successful handshake, kept until the process exits.
 */
class AgentState(
    val driver: UiDriver,
    val expectedToken: String = "",
    val agentVersion: String = AgentVersion.VERSION,
    val logBus: LogBus = LogBus(),
) {
    class SessionState(
        val id: String,
        val options: ExecutorOptions,
        val executor: CommandExecutor,
        val shutdownSignal: CompletableDeferred<Unit> = CompletableDeferred(),
    ) {
        var streamActive: Boolean = false
    }

    private val lock = Any()

    @Volatile
    private var session: SessionState? = null

    /** Completed by any `Shutdown` RPC, session or not (the runner exits on it). */
    val shutdownRequested: CompletableDeferred<Unit> = CompletableDeferred()

    val currentSession: SessionState? get() = session

    fun sessionFor(id: String): SessionState? = session?.takeIf { it.id == id }

    fun beginStream(session: SessionState): Boolean = synchronized(lock) {
        if (session.streamActive) {
            false
        } else {
            session.streamActive = true
            true
        }
    }

    fun endStream(session: SessionState) {
        synchronized(lock) { session.streamActive = false }
    }

    fun handshake(request: HandshakeRequest): HandshakeResponse {
        fun reject(code: ErrorCode, message: String): HandshakeResponse =
            HandshakeResponse.newBuilder()
                .setAccepted(false)
                .setAgentVersion(agentVersion)
                .setProtocolVersion(PROTOCOL_VERSION)
                .setError(AgentException(code, message).toProto())
                .build()

        if (expectedToken.isNotEmpty() && request.authToken != expectedToken) {
            return reject(ErrorCode.UNAUTHENTICATED, "missing or invalid impresario token (spec 7.2.1)")
        }
        if (!protocolMajorCompatible(request.protocolVersion)) {
            return reject(
                ErrorCode.PROTOCOL,
                "protocol version '${request.protocolVersion}' is not served by this agent " +
                    "(it speaks $PROTOCOL_VERSION)",
            )
        }
        if (request.platform != Platform.ANDROID && request.platform != Platform.ANDROID_TV) {
            return reject(
                ErrorCode.PROTOCOL,
                "the impresario android agent serves android platforms only (android, androidTv); " +
                    "got ${request.platform.name.lowercase()}",
            )
        }
        val device = try {
            driver.deviceInfo()
        } catch (error: Exception) {
            return reject(ErrorCode.INTERNAL, "device info failed: ${error.message}")
        }
        synchronized(lock) {
            if (session != null) {
                return reject(ErrorCode.ALREADY_IN_USE, "this agent already has an active session")
            }
            val options = ExecutorOptions.fromHandshake(request.platform, request.appId, request.optionsMap)
            val created = SessionState(
                id = UUID.randomUUID().toString(),
                options = options,
                executor = CommandExecutor(driver, options),
            )
            session = created
            logBus.log(
                LogLevel.INFO,
                "agent",
                "session ${created.id} accepted for app '${request.appId}' on ${request.platform.name}",
            )
            return HandshakeResponse.newBuilder()
                .setAccepted(true)
                .setSessionId(created.id)
                .setAgentVersion(agentVersion)
                .setProtocolVersion(PROTOCOL_VERSION)
                .addAllCapabilities(capabilities(request.platform))
                .setDevice(
                    DeviceInfo.newBuilder()
                        .setModel(device.model)
                        .setOsName(device.osName)
                        .setOsVersion(device.osVersion)
                        .setScreenWidthPx(device.screenWidthPx)
                        .setScreenHeightPx(device.screenHeightPx)
                        .setPixelRatio(device.pixelRatio)
                        .setRuntime(device.runtime)
                        .build(),
                )
                .build()
        }
    }

    companion object {
        /** Capabilities per platform (spec 6.1/6.2, decision D-13: one agent, runtime platform). */
        fun capabilities(platform: Platform): List<Capability> =
            if (platform == Platform.ANDROID_TV) {
                listOf(
                    Capability.LOGS,
                    Capability.SCREENSHOT,
                    Capability.DPAD_FOCUS,
                    Capability.KEY_EVENTS,
                    Capability.APP_CONTROL,
                )
            } else {
                listOf(
                    Capability.LOGS,
                    Capability.SCREENSHOT,
                    Capability.TOUCH_GESTURES,
                    Capability.KEY_EVENTS,
                    Capability.APP_CONTROL,
                )
            }
    }
}

/**
 * Reads `x-impresario-token` and `x-impresario-session-id` from the call
 * metadata into the gRPC [Context]; the service methods enforce them. Token
 * handling mirrors the connector: the handshake may carry the token in its
 * body field, every other RPC must use metadata (spec 7.2.1).
 */
class AuthInterceptor : ServerInterceptor {
    override fun <ReqT, RespT> interceptCall(
        call: ServerCall<ReqT, RespT>,
        headers: Metadata,
        next: ServerCallHandler<ReqT, RespT>,
    ): ServerCall.Listener<ReqT> {
        val context = Context.current()
            .withValue(CONTEXT_TOKEN, headers.get(TOKEN_KEY))
            .withValue(CONTEXT_SESSION_ID, headers.get(SESSION_KEY).orEmpty())
        return Contexts.interceptCall(context, call, headers, next)
    }

    companion object {
        private val TOKEN_KEY = Metadata.Key.of("x-impresario-token", Metadata.ASCII_STRING_MARSHALLER)
        private val SESSION_KEY = Metadata.Key.of("x-impresario-session-id", Metadata.ASCII_STRING_MARSHALLER)
        val CONTEXT_TOKEN: Context.Key<String> = Context.key("impresario-token")
        val CONTEXT_SESSION_ID: Context.Key<String> = Context.key("impresario-session-id")
    }
}

/**
 * The `impresario.v1.Agent` service over the Android agent state (spec 11.2).
 * Behavior is aligned with the connector so the master treats every executor
 * identically (decisions I-36): unknown session on `Shutdown` answers ok,
 * missing session metadata is `INVALID_ARGUMENT`, unknown session is
 * `NOT_FOUND`, a second command stream is `FAILED_PRECONDITION`.
 */
class AgentServiceImpl(
    private val state: AgentState,
    private val heartbeatIntervalMs: Long = 5_000,
    private val commandDispatcher: CoroutineDispatcher = Dispatchers.IO,
    private val now: () -> Long = System::currentTimeMillis,
) : AgentGrpcKt.AgentCoroutineImplBase() {

    override suspend fun handshake(request: HandshakeRequest): HandshakeResponse = state.handshake(request)

    override fun session(requests: Flow<Command>): Flow<Event> {
        val context = Context.current()
        requireAuthorized(AuthInterceptor.CONTEXT_TOKEN.get(context))
        val session = requireSession(AuthInterceptor.CONTEXT_SESSION_ID.get(context).orEmpty())
        if (!state.beginStream(session)) {
            throw Status.FAILED_PRECONDITION
                .withDescription("this session already has an open command stream")
                .asRuntimeException()
        }
        return channelFlow {
            try {
                SessionRunner(
                    executor = session.executor,
                    heartbeatIntervalMs = heartbeatIntervalMs,
                    commandDispatcher = commandDispatcher,
                    now = now,
                ).run(requests, session.shutdownSignal) { event -> send(event) }
            } finally {
                state.endStream(session)
            }
        }
    }

    override suspend fun captureScreenshot(request: ScreenshotRequest): ScreenshotResponse {
        val context = Context.current()
        requireAuthorized(AuthInterceptor.CONTEXT_TOKEN.get(context))
        val session = requireSession(AuthInterceptor.CONTEXT_SESSION_ID.get(context).orEmpty())
        return try {
            val png = withContext(commandDispatcher) { state.driver.screenshotPng() }
            val dimensions = Png.dimensions(png)
            state.logBus.log(LogLevel.DEBUG, "agent", "screenshot for session ${session.id}")
            ScreenshotResponse.newBuilder()
                .setPng(ByteString.copyFrom(png))
                .setWidth(dimensions?.first ?: 0)
                .setHeight(dimensions?.second ?: 0)
                .build()
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (error: Exception) {
            ScreenshotResponse.newBuilder()
                .setError(
                    AgentException.internal("screenshot failed: ${error.message}").toProto(),
                )
                .build()
        }
    }

    override fun streamLogs(request: LogRequest): Flow<LogEntry> {
        val context = Context.current()
        requireAuthorized(AuthInterceptor.CONTEXT_TOKEN.get(context))
        requireSession(AuthInterceptor.CONTEXT_SESSION_ID.get(context).orEmpty())
        // A marker entry opens the stream before any log-producing RPC runs.
        // It is stream plumbing and bypasses the level filter: grpc-java sends
        // response headers only with the first message, so an executor whose
        // bus is idle would otherwise keep a client that awaits the response
        // (the master) blocked forever (docs/DECISIONS.md I-74).
        val opened = LogEntry.newBuilder()
            .setUnixMs(now())
            .setLevel(LogLevel.INFO)
            .setSource("agent")
            .setMessage("log stream opened at min level ${request.minLevel.name}")
            .build()
        return flow {
            emit(opened)
            emitAll(state.logBus.events(request.minLevel))
        }
    }

    override suspend fun shutdown(request: ShutdownRequest): ShutdownResponse {
        val context = Context.current()
        requireAuthorized(AuthInterceptor.CONTEXT_TOKEN.get(context))
        val sessionId = AuthInterceptor.CONTEXT_SESSION_ID.get(context).orEmpty()
        val session = if (sessionId.isEmpty()) null else state.sessionFor(sessionId)
        if (session != null) {
            if (request.terminateApp && session.options.appId.isNotEmpty()) {
                runCatching {
                    withContext(commandDispatcher) {
                        state.driver.terminate(session.options.appId)
                    }
                }
            }
            state.logBus.log(LogLevel.INFO, "agent", "shutdown requested for session ${session.id}")
            session.shutdownSignal.complete(Unit)
            state.shutdownRequested.complete(Unit)
        }
        // Unknown sessions answer ok: shutdown is idempotent (decision I-36).
        return ShutdownResponse.newBuilder().build()
    }

    private fun requireAuthorized(token: String?) {
        if (state.expectedToken.isEmpty()) return
        if (token != state.expectedToken) {
            throw Status.UNAUTHENTICATED
                .withDescription("missing or invalid impresario token (spec 7.2.1)")
                .asRuntimeException()
        }
    }

    private fun requireSession(id: String): AgentState.SessionState {
        if (id.isEmpty()) {
            throw Status.INVALID_ARGUMENT
                .withDescription("missing x-impresario-session-id metadata")
                .asRuntimeException()
        }
        return state.sessionFor(id)
            ?: throw Status.NOT_FOUND
                .withDescription("unknown session '$id'")
                .asRuntimeException()
    }
}
