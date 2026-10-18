package com.impresario.agent

import kotlin.math.abs

/** What the navigator may ask of the device (spec 6.3, `FocusProbe`). */
interface FocusProbe {
    fun currentFocusLabel(): String?
    fun currentFocusFrame(): Rect?
    fun targetFrame(): Rect?
    fun press(direction: Direction)
    fun isTargetFocused(): Boolean
}

sealed class FocusResult {
    object Focused : FocusResult()

    /** `reason` is one of `budget`, `cycle`, `blocked` (spec 6.3). */
    data class Unreachable(
        val reason: String,
        val trail: List<FocusTrailEntry>,
        val steps: Int,
    ) : FocusResult()
}

data class FocusTrailEntry(
    val step: Int,
    val direction: String,
    val focusedLabel: String,
    val frame: String,
)

/**
 * D-pad traversal for TV platforms (spec 6.3): move the focus towards the
 * target along the dominant axis, guard against cycles, and report an honest
 * trail when the target cannot be reached. Pure logic, unit-tested with a fake
 * probe.
 */
class FocusNavigator(private val probe: FocusProbe, private val maxSteps: Int) {

    fun ensureTargetFocused(): FocusResult {
        if (probe.isTargetFocused()) return FocusResult.Focused

        val trail = mutableListOf<FocusTrailEntry>()
        val seen = mutableSetOf<Pair<String, Direction>>()
        var step = 0

        while (step < maxSteps) {
            val current = probe.currentFocusFrame()
            if (current == null) {
                // Unreadable focus: one neutral `down` press, then retry.
                step += 1
                trail.add(entry(step, Direction.DOWN, null))
                if (!seen.add("<none>" to Direction.DOWN)) {
                    return FocusResult.Unreachable("cycle", trail, step)
                }
                probe.press(Direction.DOWN)
                if (probe.isTargetFocused()) return FocusResult.Focused
                continue
            }

            val target = probe.targetFrame()
                ?: return FocusResult.Unreachable("blocked", trail, step)

            val dx = target.centerX - current.centerX
            val dy = target.centerY - current.centerY
            val direction = when {
                dy == 0 && dx == 0 -> Direction.DOWN
                abs(dy) >= abs(dx) -> if (dy > 0) Direction.DOWN else Direction.UP
                dx > 0 -> Direction.RIGHT
                else -> Direction.LEFT
            }

            val frameKey = current.toString()
            if (!seen.add(frameKey to direction)) {
                return FocusResult.Unreachable("cycle", trail, step)
            }
            step += 1
            trail.add(entry(step, direction, current))
            probe.press(direction)

            if (probe.isTargetFocused()) return FocusResult.Focused

            val after = probe.currentFocusFrame()
            if (after != null && after == current) {
                // The dominant axis is blocked: try the other axis once.
                val alternative = when (direction) {
                    Direction.UP, Direction.DOWN -> if (dx >= 0) Direction.RIGHT else Direction.LEFT
                    else -> if (dy >= 0) Direction.DOWN else Direction.UP
                }
                if (!seen.add(frameKey to alternative)) {
                    return FocusResult.Unreachable("cycle", trail, step)
                }
                if (step >= maxSteps) break
                step += 1
                trail.add(entry(step, alternative, current))
                probe.press(alternative)
                if (probe.isTargetFocused()) return FocusResult.Focused
                val afterAlternative = probe.currentFocusFrame()
                if (afterAlternative != null && afterAlternative == current) {
                    trail.add(
                        FocusTrailEntry(
                            step = step,
                            direction = "blocked",
                            focusedLabel = probe.currentFocusLabel() ?: "",
                            frame = current.toString(),
                        ),
                    )
                    return FocusResult.Unreachable("blocked", trail, step)
                }
            }
        }
        return FocusResult.Unreachable("budget", trail, step)
    }

    private fun entry(step: Int, direction: Direction, frame: Rect?): FocusTrailEntry =
        FocusTrailEntry(
            step = step,
            direction = direction.name.lowercase(),
            focusedLabel = probe.currentFocusLabel() ?: "",
            frame = frame?.toString() ?: "",
        )

    companion object {
        /** Serializes a trail into the `details.focus_trail` JSON array. */
        fun trailJson(trail: List<FocusTrailEntry>): String =
            trail.joinToString(prefix = "[", postfix = "]") { entry ->
                "{\"step\":${entry.step}," +
                    "\"direction\":${jsonString(entry.direction)}," +
                    "\"focused_label\":${jsonString(entry.focusedLabel)}," +
                    "\"frame\":${jsonString(entry.frame)}}"
            }

        private fun jsonString(value: String): String {
            val builder = StringBuilder("\"")
            value.forEach { ch ->
                when (ch) {
                    '"' -> builder.append("\\\"")
                    '\\' -> builder.append("\\\\")
                    '\n' -> builder.append("\\n")
                    '\r' -> builder.append("\\r")
                    '\t' -> builder.append("\\t")
                    else ->
                        if (ch.code < 0x20) {
                            builder.append("\\u").append(ch.code.toString(16).padStart(4, '0'))
                        } else {
                            builder.append(ch)
                        }
                }
            }
            return builder.append('"').toString()
        }
    }
}
