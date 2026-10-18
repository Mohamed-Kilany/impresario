package com.impresario.agent

import java.util.Base64
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class PngTest {
    // A real 1x1 transparent PNG.
    private val onePixel = Base64.getDecoder().decode(
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==",
    )

    @Test
    fun readsIhdrDimensions() {
        assertEquals(1 to 1, Png.dimensions(onePixel))
    }

    @Test
    fun rejectsNonPngBytes() {
        assertNull(Png.dimensions(byteArrayOf(1, 2, 3)))
        assertNull(Png.dimensions(ByteArray(64)))
    }
}
