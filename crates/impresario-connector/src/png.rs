//! Minimal PNG dimension parsing (the out-of-band screenshot reply carries
//! width/height alongside the bytes; no image crate needed).

/// PNG signature.
const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

/// `(width, height)` from the IHDR chunk, or `None` for non-PNG data.
#[must_use]
pub fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.get(..8)? != SIGNATURE {
        return None;
    }
    let width = u32::from_be_bytes(bytes.get(16..20)?.try_into().ok()?);
    let height = u32::from_be_bytes(bytes.get(20..24)?.try_into().ok()?);
    Some((width, height))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ihdr_dimensions() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&SIGNATURE);
        bytes.extend_from_slice(&[0, 0, 0, 13]); // chunk length
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&1280u32.to_be_bytes());
        bytes.extend_from_slice(&720u32.to_be_bytes());
        bytes.extend_from_slice(&[8, 6, 0, 0, 0]);
        assert_eq!(png_dimensions(&bytes), Some((1280, 720)));
    }

    #[test]
    fn rejects_short_or_non_png_data() {
        assert_eq!(png_dimensions(b"not a png"), None);
        assert_eq!(png_dimensions(&SIGNATURE), None);
    }
}
