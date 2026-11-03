package com.impresario.agent.driver

import com.impresario.agent.Direction
import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * The screen-level fallback must preserve the two direction conventions from
 * docs/DSL.md 5: `swipe` is finger direction, `scroll` is content direction.
 */
class ScreenSwipeTest {
    private val width = 1000
    private val height = 2000
    private val fraction = 0.5f

    @Test
    fun fingerSwipeUpStartsBelowCentreAndEndsAboveIt() {
        assertEquals(
            SwipePath(500, 1500, 500, 500),
            fingerSwipePath(Direction.UP, width, height, fraction),
        )
    }

    @Test
    fun fingerSwipeDownStartsAboveCentreAndEndsBelowIt() {
        assertEquals(
            SwipePath(500, 500, 500, 1500),
            fingerSwipePath(Direction.DOWN, width, height, fraction),
        )
    }

    @Test
    fun horizontalFingerSwipesFollowTheNamedDirection() {
        assertEquals(
            SwipePath(250, 1000, 750, 1000),
            fingerSwipePath(Direction.RIGHT, width, height, fraction),
        )
        assertEquals(
            SwipePath(750, 1000, 250, 1000),
            fingerSwipePath(Direction.LEFT, width, height, fraction),
        )
    }

    @Test
    fun scrollFallbackReversesOnlyForTheScreenGesture() {
        assertEquals(
            fingerSwipePath(Direction.UP, width, height, fraction),
            scrollSwipePath(Direction.DOWN, width, height, fraction),
        )
        assertEquals(
            fingerSwipePath(Direction.DOWN, width, height, fraction),
            scrollSwipePath(Direction.UP, width, height, fraction),
        )
        assertEquals(
            fingerSwipePath(Direction.LEFT, width, height, fraction),
            scrollSwipePath(Direction.RIGHT, width, height, fraction),
        )
        assertEquals(
            fingerSwipePath(Direction.RIGHT, width, height, fraction),
            scrollSwipePath(Direction.LEFT, width, height, fraction),
        )
    }
}
