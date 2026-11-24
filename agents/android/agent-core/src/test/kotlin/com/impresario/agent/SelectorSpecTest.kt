package com.impresario.agent

import impresario.v1.Impresario.ErrorCode
import impresario.v1.Impresario.Selector
import impresario.v1.Impresario.SelectorStrategy
import org.junit.Assert.assertEquals
import org.junit.Assert.fail
import org.junit.Test

/** `SelectorSpec` validation, including the executor-internal `NODE_REF`. */
class SelectorSpecTest {
    @Test
    fun nodeRefCarriesRevisionAndNodeId() {
        val spec = SelectorSpec.from(
            Selector.newBuilder()
                .setStrategy(SelectorStrategy.NODE_REF)
                .setSnapshotRev(12)
                .setNodeId(3)
                .build(),
        )
        assertEquals(SelectorStrategy.NODE_REF, spec.strategy)
        assertEquals(12L, spec.snapshotRev)
        assertEquals(3L, spec.nodeId)
        assertEquals("nodeRef:12:3", spec.describe())
    }

    @Test
    fun nodeRefWithoutRevisionIsAProtocolError() {
        try {
            SelectorSpec.from(
                Selector.newBuilder()
                    .setStrategy(SelectorStrategy.NODE_REF)
                    .setNodeId(3)
                    .build(),
            )
            fail("expected a protocol error")
        } catch (error: AgentException) {
            assertEquals(ErrorCode.PROTOCOL, error.code)
        }
    }

    @Test
    fun nodeRefAllowsAnExplicitMiss() {
        val spec = SelectorSpec.from(
            Selector.newBuilder()
                .setStrategy(SelectorStrategy.NODE_REF)
                .setSnapshotRev(4)
                .build(),
        )
        assertEquals(0L, spec.nodeId)
        assertEquals("nodeRef:4:0", spec.describe())
    }

    @Test
    fun locatorStillNeedsTheMasterRewrite() {
        try {
            SelectorSpec.from(
                Selector.newBuilder()
                    .setStrategy(SelectorStrategy.LOCATOR)
                    .setValue("role=button")
                    .build(),
            )
            fail("expected an invalid selector")
        } catch (error: AgentException) {
            assertEquals(ErrorCode.INVALID_SELECTOR, error.code)
        }
    }
}
