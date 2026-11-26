package com.impresario.agent.driver

import impresario.v1.Impresario.InspectorNode
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
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

    @Test
    fun flutterEmbeddingMarkerIsDetected() {
        assertTrue(hasFlutterEmbedding(setOf("flutterEmbedding")))
    }

    @Test
    fun otherManifestMetadataIsNotFlutter() {
        assertFalse(hasFlutterEmbedding(setOf("com.google.android.geo.API_KEY")))
    }

    @Test
    fun emptyManifestMetadataIsNotFlutter() {
        assertFalse(hasFlutterEmbedding(emptySet()))
    }

    @Test
    fun aNamedFlutterSemanticsNodeIsUsable() {
        val node = InspectorNode.newBuilder().setName("Login").build()
        assertTrue(hasUsableFlutterSemantics(listOf(node)))
    }

    @Test
    fun aRoleAloneMakesSemanticsUsable() {
        val node = InspectorNode.newBuilder().setRole("button").build()
        assertTrue(hasUsableFlutterSemantics(listOf(node)))
    }

    @Test
    fun aFlutterTestIdMakesSemanticsUsable() {
        val node = InspectorNode.newBuilder().setTestId("login").build()
        assertTrue(hasUsableFlutterSemantics(listOf(node)))
    }

    @Test
    fun aScrollRangeMakesSemanticsUsable() {
        val node = InspectorNode.newBuilder().setCanScrollForward(true).build()
        assertTrue(hasUsableFlutterSemantics(listOf(node)))
    }

    @Test
    fun aBareGenericFlutterViewIsNotUsable() {
        val root = InspectorNode.newBuilder().setRole("generic").build()
        assertFalse(hasUsableFlutterSemantics(listOf(root)))
    }

    @Test
    fun aGenericNodeCarryingTextIsUsable() {
        val node = InspectorNode.newBuilder().setRole("generic").setText("Welcome").build()
        assertTrue(hasUsableFlutterSemantics(listOf(node)))
    }
}
