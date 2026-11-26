package com.impresario.agent.driver

import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * INS-R8: Compose `testTag` values surface as resource ids only when the app
 * opts in to `testTagsAsResourceId`, so the inspector keeps the segment after
 * `id/` as the test id and never invents one.
 */
class InspectorNodeTest {
    @Test
    fun testIdUsesTheSegmentAfterTheResourceIdPrefix() {
        assertEquals("play-button", testIdFromResourceId("com.example.app:id/play-button"))
    }

    @Test
    fun testIdKeepsAPlainResourceId() {
        assertEquals("plain-id", testIdFromResourceId("plain-id"))
    }

    @Test
    fun absentResourceIdStaysEmpty() {
        assertEquals("", testIdFromResourceId(""))
    }
}
