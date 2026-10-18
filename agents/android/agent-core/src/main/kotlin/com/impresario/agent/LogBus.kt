package com.impresario.agent

import impresario.v1.Impresario.LogEntry
import impresario.v1.Impresario.LogLevel
import kotlinx.coroutines.channels.BufferOverflow
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.filter

/**
 * Agent-side log fan-out, surfaced through `StreamLogs` (spec 11.2). Entries
 * are dropped when nobody listens; the buffer never blocks the caller.
 */
class LogBus(private val now: () -> Long = System::currentTimeMillis) {
    private val bus = MutableSharedFlow<LogEntry>(
        replay = 0,
        extraBufferCapacity = 256,
        onBufferOverflow = BufferOverflow.DROP_OLDEST,
    )

    fun log(level: LogLevel, source: String, message: String) {
        bus.tryEmit(
            LogEntry.newBuilder()
                .setUnixMs(now())
                .setLevel(level)
                .setSource(source)
                .setMessage(message)
                .build(),
        )
    }

    /** Entries at or above [minLevel]; `LOG_LEVEL_UNSPECIFIED` means everything. */
    fun events(minLevel: LogLevel): Flow<LogEntry> {
        val minimum = if (minLevel == LogLevel.LOG_LEVEL_UNSPECIFIED) 0 else minLevel.number
        return bus.filter { it.levelValue >= minimum }
    }
}
