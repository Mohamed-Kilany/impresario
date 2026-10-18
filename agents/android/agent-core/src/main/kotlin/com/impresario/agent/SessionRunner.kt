package com.impresario.agent

import impresario.v1.Impresario.Command
import impresario.v1.Impresario.CommandResult
import impresario.v1.Impresario.ErrorCode
import impresario.v1.Impresario.Event
import impresario.v1.Impresario.Heartbeat
import impresario.v1.Impresario.SessionClosed
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Deferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.launch
import kotlinx.coroutines.selects.select
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import java.util.concurrent.atomic.AtomicLong
import java.util.concurrent.atomic.AtomicReference

/** Marks a deferred that a client `Cancel` command cancelled. */
class CommandCancelledException : CancellationException("cancelled by client")

/**
 * Drives one session stream (spec 11.2): strict per-command ordering, one
 * result per command, immediate `Cancel` handling, 5 s heartbeats, and a
 * `SessionClosed` event on shutdown or client disconnect.
 *
 * Cancel contract (mirrors the connector, decisions I-36):
 * - a cancel of the in-flight command answers `ok=true` for its own id; the
 *   cancelled command finishes with `CANCELLED`;
 * - a cancel of an unknown or finished id answers `PROTOCOL` for its own id.
 */
class SessionRunner(
    private val executor: CommandExecutor,
    private val heartbeatIntervalMs: Long = 5_000,
    private val commandDispatcher: CoroutineDispatcher = Dispatchers.IO,
    private val now: () -> Long = System::currentTimeMillis,
) {
    private class Active(val id: String, val job: Deferred<CommandResult>)

    suspend fun run(
        commands: Flow<Command>,
        shutdown: Deferred<Unit>,
        send: suspend (Event) -> Unit,
    ) {
        val emitMutex = Mutex()
        val emit: suspend (Event) -> Unit = { event -> emitMutex.withLock { send(event) } }

        coroutineScope {
            val scope = this
            val queue = Channel<Command>(Channel.UNLIMITED)
            val producer = launch {
                try {
                    commands.collect { queue.send(it) }
                } finally {
                    queue.close()
                }
            }
            val activeId = AtomicReference<String?>(null)
            val heartbeatSeq = AtomicLong(0)
            val heartbeats = launch {
                while (true) {
                    delay(heartbeatIntervalMs)
                    emit(
                        Event.newBuilder()
                            .setHeartbeat(
                                Heartbeat.newBuilder()
                                    .setSeq(heartbeatSeq.incrementAndGet())
                                    .setUnixMs(now())
                                    .setCurrentCommandId(activeId.get() ?: ""),
                            )
                            .build(),
                    )
                }
            }

            try {
                runLoop(queue, shutdown, activeId, emit, scope)
            } finally {
                heartbeats.cancel()
                producer.cancel()
            }
        }
    }

    private suspend fun runLoop(
        queue: Channel<Command>,
        shutdown: Deferred<Unit>,
        activeId: AtomicReference<String?>,
        emit: suspend (Event) -> Unit,
        scope: CoroutineScope,
    ) {
        val pending = ArrayDeque<Command>()
        val startedIds = mutableSetOf<String>()
        var active: Active? = null

        suspend fun emitResult(result: CommandResult) = emit(
            Event.newBuilder().setResult(result).build(),
        )

        fun start(command: Command): Active {
            val id = command.commandId
            startedIds.add(id)
            activeId.set(id)
            return Active(
                id,
                scope.async(commandDispatcher) { executor.execute(command) },
            )
        }

        suspend fun finish(activeCommand: Active) {
            val result = try {
                activeCommand.job.await()
            } catch (cancelled: CommandCancelledException) {
                cancelledResult(activeCommand.id)
            }
            emitResult(result)
            activeId.set(null)
        }

        while (true) {
            if (active == null) {
                val next = if (pending.isNotEmpty()) {
                    pending.removeFirst()
                } else {
                    when (
                        val outcome = select<IdleOutcome> {
                            queue.onReceiveCatching { result ->
                                if (result.isClosed) IdleOutcome.Closed
                                else result.getOrNull()?.let { IdleOutcome.Next(it) } ?: IdleOutcome.Closed
                            }
                            shutdown.onAwait { IdleOutcome.Stopped }
                        }
                    ) {
                        is IdleOutcome.Next -> outcome.command
                        IdleOutcome.Closed -> null
                        IdleOutcome.Stopped -> {
                            emit(closedEvent("shutdown"))
                            return
                        }
                    }
                }
                if (next == null) {
                    emit(closedEvent("client_closed"))
                    return
                }
                when (val decision = admit(next, active, startedIds)) {
                    is Admission.Start -> active = start(decision.command)
                    is Admission.RespondImmediately -> emitResult(decision.result)
                }
                continue
            }

            val outcome = try {
                select<BusyOutcome> {
                    active.job.onAwait { result -> BusyOutcome.Completed(result) }
                    queue.onReceiveCatching { result ->
                        if (result.isClosed) BusyOutcome.Closed
                        else result.getOrNull()?.let { BusyOutcome.Next(it) } ?: BusyOutcome.Closed
                    }
                    shutdown.onAwait { BusyOutcome.Stopped }
                }
            } catch (cancelled: CommandCancelledException) {
                BusyOutcome.Cancelled
            }

            when (outcome) {
                is BusyOutcome.Completed -> {
                    emitResult(outcome.result)
                    activeId.set(null)
                    active = null
                }
                BusyOutcome.Cancelled -> {
                    emitResult(cancelledResult(active.id))
                    activeId.set(null)
                    active = null
                }
                is BusyOutcome.Next -> {
                    when (val decision = admit(outcome.command, active, startedIds)) {
                        is Admission.Start -> {
                            // A command received while another runs is queued;
                            // admission only ever starts when the runner is idle.
                            pending.addLast(decision.command)
                        }
                        is Admission.RespondImmediately -> emitResult(decision.result)
                    }
                }
                BusyOutcome.Closed -> {
                    finish(active)
                    active = null
                    // The input stream ended: flush the queue we already hold.
                    while (pending.isNotEmpty()) {
                        val next = pending.removeFirst()
                        when (val decision = admit(next, null, startedIds)) {
                            is Admission.Start -> {
                                active = start(decision.command)
                                while (active != null) {
                                    val raced = select<BusyOutcome> {
                                        active.job.onAwait { result -> BusyOutcome.Completed(result) }
                                        shutdown.onAwait { BusyOutcome.Stopped }
                                    }
                                    when (raced) {
                                        is BusyOutcome.Completed -> {
                                            emitResult(raced.result)
                                            activeId.set(null)
                                            active = null
                                        }
                                        BusyOutcome.Stopped -> {
                                            active.job.cancel(CommandCancelledException())
                                            finish(active)
                                            active = null
                                            emit(closedEvent("shutdown"))
                                            return
                                        }
                                        else -> Unit
                                    }
                                }
                            }
                            is Admission.RespondImmediately -> emitResult(decision.result)
                        }
                    }
                    emit(closedEvent("client_closed"))
                    return
                }
                BusyOutcome.Stopped -> {
                    active.job.cancel(CommandCancelledException())
                    finish(active)
                    active = null
                    emit(closedEvent("shutdown"))
                    return
                }
            }
        }
    }

    private sealed class Admission {
        data class Start(val command: Command) : Admission()
        data class RespondImmediately(val result: CommandResult) : Admission()
    }

    /** Whether to start, queue, or answer the command right away. */
    private fun admit(
        command: Command,
        active: Active?,
        startedIds: MutableSet<String>,
    ): Admission {
        val id = command.commandId
        if (id.isEmpty()) {
            return Admission.RespondImmediately(
                errorResult(
                    id,
                    ErrorCode.PROTOCOL,
                    "the command id is empty",
                ),
            )
        }
        if (command.bodyCase == Command.BodyCase.CANCEL) {
            val target = command.cancel.targetCommandId
            if (active != null && target == active.id) {
                active.job.cancel(CommandCancelledException())
                return Admission.RespondImmediately(cancelAck(id, stopped = true))
            }
            return Admission.RespondImmediately(
                cancelAck(
                    id,
                    stopped = false,
                    message = "the target command '$target' is not in flight",
                ),
            )
        }
        if (id in startedIds) {
            return Admission.RespondImmediately(
                errorResult(id, ErrorCode.PROTOCOL, "duplicate command id '$id'"),
            )
        }
        return Admission.Start(command)
    }

    private fun cancelAck(id: String, stopped: Boolean, message: String = ""): CommandResult {
        val builder = CommandResult.newBuilder().setCommandId(id).setOk(stopped)
        if (!stopped) {
            builder.setError(
                AgentException.protocol(
                    message.ifEmpty { "no command is in flight" },
                ).toProto(),
            )
        }
        return builder.build()
    }

    private fun errorResult(id: String, code: ErrorCode, message: String): CommandResult =
        CommandResult.newBuilder()
            .setCommandId(id)
            .setOk(false)
            .setError(AgentException(code, message).toProto())
            .build()

    private fun cancelledResult(id: String): CommandResult =
        CommandResult.newBuilder()
            .setCommandId(id)
            .setOk(false)
            .setError(AgentException.cancelled("the command was cancelled").toProto())
            .build()

    private fun closedEvent(reason: String): Event =
        Event.newBuilder()
            .setClosed(SessionClosed.newBuilder().setReason(reason))
            .build()

    private sealed class IdleOutcome {
        data class Next(val command: Command) : IdleOutcome()
        object Closed : IdleOutcome()
        object Stopped : IdleOutcome()
    }

    private sealed class BusyOutcome {
        data class Completed(val result: CommandResult) : BusyOutcome()
        object Cancelled : BusyOutcome()
        data class Next(val command: Command) : BusyOutcome()
        object Closed : BusyOutcome()
        object Stopped : BusyOutcome()
    }
}
