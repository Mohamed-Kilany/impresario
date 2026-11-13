package com.impresario.agent

import impresario.v1.Impresario.InspectorInputKind
import impresario.v1.Impresario.InspectorInputRequest
import impresario.v1.Impresario.InspectorInputResponse
import impresario.v1.Impresario.InspectorNode
import impresario.v1.Impresario.InspectorSnapshotRequest
import impresario.v1.Impresario.InspectorSnapshotResponse
import impresario.v1.InspectorGrpcKt
import io.grpc.Context
import io.grpc.Status
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.util.concurrent.atomic.AtomicLong

/** Defaults match the connector's viewport-only acquisition (TASK-INSPECTOR 6.3). */
const val INSPECTOR_DEFAULT_MAX_NODES: Int = 2_000
const val INSPECTOR_DEFAULT_MAX_DEPTH: Int = 64

/** One capture request, already resolved against the active session. */
data class InspectorCaptureOptions(
    val includeSystemUi: Boolean,
    val minVisibleFraction: Float,
    val maxNodes: Int,
    val maxDepth: Int,
    val includeRaw: Boolean,
    val includeScreenshot: Boolean,
    val appId: String,
    val pixelRatio: Float,
    val displayWidthPx: Int,
    val displayHeightPx: Int,
)

/** A viewport-pruned accessibility tree plus its focused node. */
data class InspectorCapture(
    val nodes: List<InspectorNode>,
    val focusedId: Long,
    val truncated: Boolean,
    val degraded: List<String> = emptyList(),
    val screenshotPng: ByteArray? = null,
)

/**
 * The Android side of the additive `Inspector` service. Implemented by the
 * UiAutomator driver; kept as an interface so agent-core stays JVM-testable.
 */
interface InspectorDriver {
    fun capture(options: InspectorCaptureOptions): InspectorCapture

    /** Capture only the current frame at [scalePercent] (1-100). */
    fun screenshot(scalePercent: Int): ByteArray? = null

    /** Tap a snapshot node, or the logical point when [nodeId] is zero. */
    fun tap(nodeId: Long, x: Float, y: Float)

    /** Scroll the content in [direction] by [count] steps. */
    fun scroll(direction: Direction, count: Int, nodeId: Long, x: Float, y: Float)

    fun pressKey(code: Int)

    /** Focus [nodeId] when set, then replace the focused field's text. */
    fun type(nodeId: Long, text: String)

    /** Swipe between logical points. */
    fun swipe(x1: Float, y1: Float, x2: Float, y2: Float, durationMs: Long)
}

/**
 * `impresario.v1.Inspector` over the same session/token rules as `Agent`
 * (TASK-INSPECTOR 4.2/6.3). The master opens an `Agent` session first, then
 * calls these unary RPCs with `x-impresario-session-id`.
 */
class InspectorServiceImpl(
    private val state: AgentState,
    private val commandDispatcher: CoroutineDispatcher = Dispatchers.IO,
) : InspectorGrpcKt.InspectorCoroutineImplBase() {

    private val revision = AtomicLong(1)

    override suspend fun snapshot(request: InspectorSnapshotRequest): InspectorSnapshotResponse {
        val context = Context.current()
        requireAuthorized(AuthInterceptor.CONTEXT_TOKEN.get(context))
        val session = requireSession(AuthInterceptor.CONTEXT_SESSION_ID.get(context).orEmpty())
        val driver = state.inspectorDriver
            ?: throw Status.UNIMPLEMENTED
                .withDescription("this executor has no inspector driver")
                .asRuntimeException()
        val info = state.driver.deviceInfo()
        val logicalWidth = if (info.pixelRatio > 0f) info.screenWidthPx / info.pixelRatio else 0f
        val logicalHeight = if (info.pixelRatio > 0f) info.screenHeightPx / info.pixelRatio else 0f
        if (request.screenshotOnly) {
            val started = System.nanoTime()
            val png = try {
                withContext(commandDispatcher) {
                    driver.screenshot(
                        if (request.screenshotScale == 0) 100
                        else request.screenshotScale.coerceIn(1, 100),
                    )
                }
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (error: Exception) {
                return InspectorSnapshotResponse.newBuilder()
                    .setRev(revision.incrementAndGet())
                    .setDeviceName(info.model)
                    .setPlatform(session.options.platform)
                    .setLogicalWidth(logicalWidth)
                    .setLogicalHeight(logicalHeight)
                    .setPixelRatio(info.pixelRatio)
                    .setTimestampUnixMs(System.currentTimeMillis())
                    .setError(error.toAgentException().toProto())
                    .build()
            }
            val elapsedMs = ((System.nanoTime() - started) / 1_000_000L).toInt()
            val builder = InspectorSnapshotResponse.newBuilder()
                .setRev(revision.incrementAndGet())
                .setDeviceName(info.model)
                .setPlatform(session.options.platform)
                .setLogicalWidth(logicalWidth)
                .setLogicalHeight(logicalHeight)
                .setViewportWidth(logicalWidth)
                .setViewportHeight(logicalHeight)
                .setPixelRatio(info.pixelRatio)
                .setSettled(true)
                .setAcquisitionMs(elapsedMs)
                .setTimestampUnixMs(System.currentTimeMillis())
            png?.let { bytes ->
                builder.setScreenshotPng(com.google.protobuf.ByteString.copyFrom(bytes))
                Png.dimensions(bytes)?.let { (width, height) ->
                    builder.setScreenshotWidth(width)
                    builder.setScreenshotHeight(height)
                }
            }
            return builder.build()
        }
        val started = System.nanoTime()
        val capture = try {
            withContext(commandDispatcher) {
                driver.capture(
                    InspectorCaptureOptions(
                        includeSystemUi = request.includeSystemUi,
                        minVisibleFraction = request.minVisibleFraction,
                        maxNodes = if (request.maxNodes > 0) request.maxNodes else INSPECTOR_DEFAULT_MAX_NODES,
                        maxDepth = if (request.maxDepth > 0) request.maxDepth else INSPECTOR_DEFAULT_MAX_DEPTH,
                        includeRaw = request.includeRaw,
                        includeScreenshot = request.includeScreenshot,
                        appId = session.options.appId,
                        pixelRatio = info.pixelRatio,
                        displayWidthPx = info.screenWidthPx,
                        displayHeightPx = info.screenHeightPx,
                    ),
                )
            }
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (error: Exception) {
            return InspectorSnapshotResponse.newBuilder()
                .setRev(revision.incrementAndGet())
                .setDeviceName(info.model)
                .setPlatform(session.options.platform)
                .setLogicalWidth(logicalWidth)
                .setLogicalHeight(logicalHeight)
                .setPixelRatio(info.pixelRatio)
                .setViewportWidth(logicalWidth)
                .setViewportHeight(logicalHeight)
                .setTimestampUnixMs(System.currentTimeMillis())
                .setError(error.toAgentException().toProto())
                .build()
        }
        val elapsedMs = ((System.nanoTime() - started) / 1_000_000L).toInt()
        val builder = InspectorSnapshotResponse.newBuilder()
            .setRev(revision.incrementAndGet())
            .setDeviceName(info.model)
            .setPlatform(session.options.platform)
            .setLogicalWidth(logicalWidth)
            .setLogicalHeight(logicalHeight)
            .setViewportWidth(logicalWidth)
            .setViewportHeight(logicalHeight)
            .setPixelRatio(info.pixelRatio)
            .setSettled(true)
            .setAcquisitionMs(elapsedMs)
            .addAllNodes(capture.nodes)
            .addAllDegraded(capture.degraded)
            .setFocusedId(capture.focusedId)
            .setTimestampUnixMs(System.currentTimeMillis())
            .setTruncated(capture.truncated)
        capture.screenshotPng?.let { png ->
            builder.setScreenshotPng(com.google.protobuf.ByteString.copyFrom(png))
            Png.dimensions(png)?.let { (width, height) ->
                builder.setScreenshotWidth(width)
                builder.setScreenshotHeight(height)
            }
        }
        return builder.build()
    }

    override suspend fun input(request: InspectorInputRequest): InspectorInputResponse {
        val context = Context.current()
        requireAuthorized(AuthInterceptor.CONTEXT_TOKEN.get(context))
        val session = requireSession(AuthInterceptor.CONTEXT_SESSION_ID.get(context).orEmpty())
        val driver = state.inspectorDriver
            ?: throw Status.UNIMPLEMENTED
                .withDescription("this executor has no inspector driver")
                .asRuntimeException()
        return try {
            withContext(commandDispatcher) {
                when (request.kind) {
                    InspectorInputKind.INSPECTOR_INPUT_KIND_TAP ->
                        driver.tap(request.nodeId, request.x, request.y)
                    InspectorInputKind.INSPECTOR_INPUT_KIND_SCROLL ->
                        driver.scroll(
                            directionOf(request.direction),
                            request.count.coerceAtLeast(1),
                            request.nodeId,
                            request.x,
                            request.y,
                        )
                    InspectorInputKind.INSPECTOR_INPUT_KIND_KEY ->
                        driver.pressKey(KeyTable.resolve(request.key, session.options.isTv))
                    InspectorInputKind.INSPECTOR_INPUT_KIND_TYPE ->
                        driver.type(request.nodeId, request.text)
                    InspectorInputKind.INSPECTOR_INPUT_KIND_SWIPE ->
                        driver.swipe(request.x, request.y, request.endX, request.endY, request.durationMs.toLong())
                    else -> throw AgentException.protocol("the inspector input kind is unspecified")
                }
            }
            InspectorInputResponse.newBuilder().setOk(true).setRev(revision.get()).build()
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (error: Exception) {
            InspectorInputResponse.newBuilder()
                .setOk(false)
                .setError(error.toAgentException().toProto())
                .setRev(revision.get())
                .build()
        }
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

    private fun directionOf(direction: impresario.v1.Impresario.Direction): Direction = when (direction) {
        impresario.v1.Impresario.Direction.UP -> Direction.UP
        impresario.v1.Impresario.Direction.DOWN -> Direction.DOWN
        impresario.v1.Impresario.Direction.LEFT -> Direction.LEFT
        impresario.v1.Impresario.Direction.RIGHT -> Direction.RIGHT
        else -> throw AgentException.protocol("the scroll direction is unspecified")
    }

    private fun Exception.toAgentException(): AgentException =
        this as? AgentException ?: AgentException.internal(message ?: this::class.java.simpleName)
}
