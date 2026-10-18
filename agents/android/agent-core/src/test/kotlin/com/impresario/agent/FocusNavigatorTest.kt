package com.impresario.agent

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class FocusNavigatorTest {
    private class Probe(var current: Rect?, var target: Rect?) : FocusProbe {
        var focused = false
        val presses = mutableListOf<Direction>()
        var onPress: ((Direction) -> Unit)? = null

        override fun currentFocusLabel(): String? = "node"
        override fun currentFocusFrame(): Rect? = current
        override fun targetFrame(): Rect? = target
        override fun press(direction: Direction) {
            presses.add(direction)
            onPress?.invoke(direction)
        }

        override fun isTargetFocused(): Boolean = focused
    }

    private fun unreachable(result: FocusResult): FocusResult.Unreachable {
        assertTrue("expected an unreachable result", result is FocusResult.Unreachable)
        return result as FocusResult.Unreachable
    }

    @Test
    fun movesTowardsTheTargetAndSucceeds() {
        val probe = Probe(Rect(0, 0, 10, 10), Rect(100, 0, 110, 10))
        probe.onPress = { if (it == Direction.RIGHT) probe.focused = true }
        val result = FocusNavigator(probe, 50).ensureTargetFocused()
        assertEquals(FocusResult.Focused, result)
        assertEquals(listOf(Direction.RIGHT), probe.presses)
    }

    @Test
    fun prefersTheVerticalAxisOnDiagonalTies() {
        val probe = Probe(Rect(0, 0, 10, 10), Rect(20, 20, 30, 30))
        probe.onPress = { if (it == Direction.DOWN) probe.focused = true }
        val result = FocusNavigator(probe, 50).ensureTargetFocused()
        assertEquals(FocusResult.Focused, result)
        assertEquals(Direction.DOWN, probe.presses.first())
    }

    @Test
    fun blockedWhenNeitherAxisMovesFocus() {
        val probe = Probe(Rect(0, 0, 10, 10), Rect(500, 0, 510, 10))
        val result = unreachable(FocusNavigator(probe, 50).ensureTargetFocused())
        assertEquals("blocked", result.reason)
        assertEquals(listOf(Direction.RIGHT, Direction.DOWN), probe.presses)
        assertEquals("blocked", result.trail.last().direction)
    }

    @Test
    fun detectsCyclesBetweenFrames() {
        val a = Rect(0, 0, 10, 10)
        val b = Rect(100, 0, 110, 10)
        val probe = Probe(a, Rect(500, 0, 510, 10))
        probe.onPress = { probe.current = if (probe.current == a) b else a }
        val result = unreachable(FocusNavigator(probe, 50).ensureTargetFocused())
        assertEquals("cycle", result.reason)
    }

    @Test
    fun givesUpAfterTheStepBudget() {
        val probe = Probe(Rect(0, 0, 10, 10), Rect(1000, 0, 1010, 10))
        probe.onPress = {
            val current = probe.current!!
            probe.current = Rect(current.left + 1, current.top, current.right + 1, current.bottom)
        }
        val result = unreachable(FocusNavigator(probe, 5).ensureTargetFocused())
        assertEquals("budget", result.reason)
        assertEquals(5, result.steps)
        assertEquals(5, probe.presses.size)
    }

    @Test
    fun unreadableFocusPressesDownOnce() {
        val probe = Probe(null, Rect(0, 0, 10, 10))
        probe.onPress = {
            if (probe.current == null) {
                probe.current = Rect(0, 0, 10, 10)
            } else {
                probe.focused = true
            }
        }
        val result = FocusNavigator(probe, 50).ensureTargetFocused()
        assertEquals(FocusResult.Focused, result)
        assertEquals(listOf(Direction.DOWN, Direction.DOWN), probe.presses)
    }

    @Test
    fun trailJsonEscapesLabels() {
        val json = FocusNavigator.trailJson(
            listOf(FocusTrailEntry(1, "down", "a\"b\\c", "[0,0][1,1]")),
        )
        assertEquals(
            "[{\"step\":1,\"direction\":\"down\",\"focused_label\":\"a\\\"b\\\\c\",\"frame\":\"[0,0][1,1]\"}]",
            json,
        )
    }
}
