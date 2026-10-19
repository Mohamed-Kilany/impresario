import Foundation

/// Minimal PNG header reader for `ScreenshotResponse.width/height` (spec 7.3:
/// the screenshot carries both, the consumer renders scaled).
public enum Png {
    /// Returns `(width, height)` from the IHDR chunk, or `nil` when the data is
    /// not a PNG with a plausible header.
    public static func dimensions(_ data: Data) -> (width: Int, height: Int)? {
        guard data.count >= 24 else { return nil }
        let signature: [UInt8] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]
        guard Array(data.prefix(8)) == signature else { return nil }
        // First chunk: length(4) type(4) = 8 bytes after the signature, then
        // the IHDR payload starts with width(4) and height(4), both big-endian.
        let width = be32(data, 16)
        let height = be32(data, 20)
        guard width > 0, height > 0 else { return nil }
        return (width, height)
    }

    private static func be32(_ data: Data, _ offset: Int) -> Int {
        let start = data.startIndex + offset
        let end = start + 4
        guard end <= data.endIndex else { return 0 }
        var value = 0
        for byte in data[start..<end] {
            value = (value << 8) | Int(byte)
        }
        return value
    }
}
