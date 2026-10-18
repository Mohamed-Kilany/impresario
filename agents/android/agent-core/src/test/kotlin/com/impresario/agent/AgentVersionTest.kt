package com.impresario.agent

import impresario.v1.Impresario.Platform
import org.junit.Assert.assertEquals
import org.junit.Test

/** Toolchain smoke test: proves proto/gRPC generation and Kotlin compilation run. */
class AgentVersionTest {
    @Test
    fun versionAndGeneratedCodeAreReachable() {
        assertEquals("0.1.0", AgentVersion.VERSION)
        assertEquals(4, Platform.ANDROID.number)
    }
}
