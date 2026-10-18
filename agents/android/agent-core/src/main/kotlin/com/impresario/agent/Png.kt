package com.impresario.agent

/** PNG header parsing for `ScreenshotResponse.width/height`. */
object Png {
    private val SIGNATURE = intArrayOf(0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A)

    /**
     * Width and height from the IHDR chunk, or null when [bytes] is not a PNG
     * (or is truncated). Never throws: a raw screenshot goes over the wire
     * even when the header cannot be read.
     */
    fun dimensions(bytes: ByteArray): Pair<Int, Int>? {
        if (bytes.size < 24) return null
        for (index in SIGNATURE.indices) {
            if ((bytes[index].toInt() and 0xFF) != SIGNATURE[index]) return null
        }
        // Bytes 12..15 are the first chunk type; a PNG starts with IHDR.
        if (!(bytes[12] == 'I'.code.toByte() &&
                bytes[13] == 'H'.code.toByte() &&
                bytes[14] == 'D'.code.toByte() &&
                bytes[15] == 'R'.code.toByte())
        ) {
            return null
        }
        fun be32(offset: Int): Int =
            ((bytes[offset].toInt() and 0xFF) shl 24) or
                ((bytes[offset + 1].toInt() and 0xFF) shl 16) or
                ((bytes[offset + 2].toInt() and 0xFF) shl 8) or
                (bytes[offset + 3].toInt() and 0xFF)

        val width = be32(16)
        val height = be32(20)
        if (width <= 0 || height <= 0) return null
        return width to height
    }
}
