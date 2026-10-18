package com.impresario.agent

import impresario.v1.Impresario.ErrorCode
import impresario.v1.Impresario.ErrorInfo

/**
 * An executor-side failure that maps onto the wire `ErrorInfo`.
 *
 * Instances only ever surface as gRPC not-ok states for transport-level
 * problems (authentication, wrong session); everything that happens inside a
 * session is answered with `ok=false` in the `CommandResult` (protocol rule 6).
 */
class AgentException(
    val code: ErrorCode,
    message: String,
    val details: Map<String, String> = emptyMap(),
) : Exception(message) {

    fun toProto(): ErrorInfo {
        val builder = ErrorInfo.newBuilder().setCode(code).setMessage(message ?: "")
        details.forEach { (key, value) -> builder.putDetails(key, value) }
        return builder.build()
    }

    companion object {
        fun elementNotFound(message: String, selector: String? = null): AgentException {
            val details = if (selector == null) emptyMap() else mapOf("selector" to selector)
            return AgentException(ErrorCode.ELEMENT_NOT_FOUND, message, details)
        }

        fun invalidSelector(message: String): AgentException =
            AgentException(ErrorCode.INVALID_SELECTOR, message)

        fun notInteractable(message: String, details: Map<String, String> = emptyMap()): AgentException =
            AgentException(ErrorCode.NOT_INTERACTABLE, message, details)

        fun unsupportedAction(message: String): AgentException =
            AgentException(ErrorCode.UNSUPPORTED_ACTION, message)

        fun focusUnreachable(message: String, details: Map<String, String> = emptyMap()): AgentException =
            AgentException(ErrorCode.FOCUS_UNREACHABLE, message, details)

        fun checkFailed(
            message: String,
            expected: String,
            observed: String,
            matchedCount: Int,
            selector: String?,
        ): AgentException {
            val details = linkedMapOf(
                "expected" to expected,
                "observed" to observed,
                "matched_count" to matchedCount.toString(),
            )
            if (selector != null) details["selector"] = selector
            return AgentException(ErrorCode.CHECK_FAILED, message, details)
        }

        fun cancelled(message: String): AgentException =
            AgentException(ErrorCode.CANCELLED, message)

        fun protocol(message: String): AgentException =
            AgentException(ErrorCode.PROTOCOL, message)

        fun internal(message: String): AgentException =
            AgentException(ErrorCode.INTERNAL, message)

        fun appCrashed(message: String): AgentException =
            AgentException(ErrorCode.APP_CRASHED, message)

        fun timeout(message: String): AgentException =
            AgentException(ErrorCode.TIMEOUT, message)

        fun staleElement(message: String): AgentException =
            AgentException(ErrorCode.STALE_ELEMENT, message)
    }
}
