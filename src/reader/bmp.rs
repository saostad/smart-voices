//! Tiny 24-bit bottom-up BMP writer for the `reader-preview` dev command.
//!
//! Hand-rolled so the preview needs no image crate.

/// Encode `pixels` (row-major `0x00RRGGBB`, top row first) as a 24-bit
/// bottom-up BMP. A short buffer pads black.
#[must_use]
pub fn encode_bmp_24(width: u32, height: u32, pixels: &[u32]) -> Vec<u8> {
    let w = width.min(i32::MAX as u32);
    let h = height.min(i32::MAX as u32);
    let stride = w as usize * 3;
    let padded = stride.next_multiple_of(4);
    let image_size = padded as u64 * h as u64;
    let file_size = 54 + image_size;
    // `main` bounds the image via `try_reserve` before calling; the header
    // casts below only wrap for absurd sizes no caller passes.
    let mut out = Vec::with_capacity(file_size.min(usize::MAX as u64) as usize);
    // File header (14 bytes).
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&(file_size as u32).to_le_bytes());
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(&54u32.to_le_bytes());
    // DIB header: BITMAPINFOHEADER, positive height = bottom-up.
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(w as i32).to_le_bytes());
    out.extend_from_slice(&(h as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&24u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&(image_size as u32).to_le_bytes());
    out.extend_from_slice(&[0; 16]);
    // Rows bottom-up, BGR, padded to a 4-byte boundary.
    let pad = padded - stride;
    for row in (0..h).rev() {
        for col in 0..w {
            let index = row as usize * w as usize + col as usize;
            let pixel = pixels.get(index).copied().unwrap_or(0);
            out.push((pixel & 0xFF) as u8);
            out.push(((pixel >> 8) & 0xFF) as u8);
            out.push(((pixel >> 16) & 0xFF) as u8);
        }
        out.extend(std::iter::repeat_n(0, pad));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn le_u32(bytes: &[u8]) -> u32 {
        u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
    }

    fn le_i32(bytes: &[u8]) -> i32 {
        i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
    }

    #[test]
    fn header_size_offset_and_padding() {
        let pixels = [0xFF0000, 0x00FF00, 0x0000FF, 0xFFFFFF, 0x000000, 0x123456];
        let bmp = encode_bmp_24(3, 2, &pixels);
        // Row stride 9 + 3 pad = 12; image 24 bytes; file 54 + 24 = 78.
        assert_eq!(bmp.len(), 78);
        assert_eq!(&bmp[0..2], b"BM");
        assert_eq!(le_u32(&bmp[2..6]), 78);
        assert_eq!(le_u32(&bmp[10..14]), 54);
        assert_eq!(le_u32(&bmp[14..18]), 40);
        assert_eq!(le_i32(&bmp[18..22]), 3);
        assert_eq!(le_i32(&bmp[22..26]), 2);
        assert_eq!(u16::from_le_bytes([bmp[28], bmp[29]]), 24);
        // Bottom row first: white, black, 0x123456 in BGR, then padding.
        assert_eq!(&bmp[54..63], &[0xFF, 0xFF, 0xFF, 0, 0, 0, 0x56, 0x34, 0x12]);
        assert_eq!(&bmp[63..66], &[0, 0, 0]);
        // Top row: red, green, blue in BGR, then padding.
        assert_eq!(&bmp[66..75], &[0, 0, 0xFF, 0, 0xFF, 0, 0xFF, 0, 0]);
        assert_eq!(&bmp[75..78], &[0, 0, 0]);
    }
}
