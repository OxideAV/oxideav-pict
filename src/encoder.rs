//! PICT writer.
//!
//! Round 2 ships a minimal encoder that emits a *raster-only* PICT v2
//! file: the 512-byte launch-stub prefix, the picture record header,
//! the v2 sentinel + headerOp stanza, a single `DirectBitsRect`
//! (`0x009A`) carrying the input RGBA pixels in packType=1
//! `0xFF R G B` interleaved layout, and the `OpEndPic` terminator.
//!
//! That round-trips losslessly through the matching round-2 decoder
//! (see `decoder::decode_direct_bits_pixels` packType=1 32-bpp arm).
//! It's the smallest output that any PICT consumer (Preview.app,
//! qlmanage, ImageMagick's PICT delegate) can read.
//!
//! A more elaborate writer — emitting drawing commands instead of one
//! big raster, or honouring a target packType to keep file size down
//! — is a future-round refinement.

use crate::error::{PictError, Result};

/// Encode an RGBA8 raster (`width × height × 4 bytes`, row-major) as
/// a PICT v2 byte stream. The output begins with the 512-byte
/// launch-stub prefix that classic Mac apps expect.
///
/// Returns `InvalidData` if `data.len() != width × height × 4` or if
/// `width` / `height` exceed the 14-bit `rowBytes` field PICT v2
/// supports (16,383 bytes per row → 4,095 px wide max for 32-bpp).
pub fn encode_pict(width: u32, height: u32, data: &[u8]) -> Result<Vec<u8>> {
    let row_bytes = (width as usize)
        .checked_mul(4)
        .ok_or_else(|| PictError::invalid("encode: width × 4 overflows usize"))?;
    if row_bytes > 0x3FFF {
        return Err(PictError::invalid(format!(
            "encode: rowBytes {row_bytes} exceeds the 14-bit PICT v2 PixMap limit"
        )));
    }
    let expected = (width as usize) * (height as usize) * 4;
    if data.len() != expected {
        return Err(PictError::invalid(format!(
            "encode: data.len() = {} but width × height × 4 = {expected}",
            data.len()
        )));
    }
    if width == 0 || height == 0 {
        return Err(PictError::invalid(
            "encode: width and height must be non-zero",
        ));
    }

    let mut out = Vec::with_capacity(512 + 80 + expected + 4);
    // 512-byte launch-stub prefix (zero-filled).
    out.extend_from_slice(&[0u8; 512]);
    // Picture record: picSize (ignored — just stays 0), picFrame.
    out.extend_from_slice(&0u16.to_be_bytes()); // picSize
    out.extend_from_slice(&0i16.to_be_bytes()); // top
    out.extend_from_slice(&0i16.to_be_bytes()); // left
    out.extend_from_slice(&(height as i16).to_be_bytes()); // bottom
    out.extend_from_slice(&(width as i16).to_be_bytes()); // right
                                                          // v2 sentinel.
    out.extend_from_slice(&0x0011u16.to_be_bytes());
    out.extend_from_slice(&0x02FFu16.to_be_bytes());
    // headerOp + 24-byte payload (zero-filled).
    out.extend_from_slice(&0x0C00u16.to_be_bytes());
    out.extend_from_slice(&[0u8; 24]);

    // DirectBitsRect.
    out.extend_from_slice(&0x009Au16.to_be_bytes());
    // baseAddr placeholder.
    out.extend_from_slice(&0x000000FFu32.to_be_bytes());
    // rowBytes (top bit = PixMap flag).
    out.extend_from_slice(&((row_bytes as u16) | 0x8000).to_be_bytes());
    // bounds.
    out.extend_from_slice(&0i16.to_be_bytes());
    out.extend_from_slice(&0i16.to_be_bytes());
    out.extend_from_slice(&(height as i16).to_be_bytes());
    out.extend_from_slice(&(width as i16).to_be_bytes());
    // pmVersion = 0, packType = 1 (uncompressed), packSize = 0.
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&0u32.to_be_bytes());
    // hRes / vRes = 72.0 fixed-point.
    out.extend_from_slice(&0x00480000u32.to_be_bytes());
    out.extend_from_slice(&0x00480000u32.to_be_bytes());
    // pixelType = 16 (RGBDirect), pixelSize = 32, cmpCount = 3, cmpSize = 8.
    out.extend_from_slice(&16u16.to_be_bytes());
    out.extend_from_slice(&32u16.to_be_bytes());
    out.extend_from_slice(&3u16.to_be_bytes());
    out.extend_from_slice(&8u16.to_be_bytes());
    // planeBytes / pmTable / pmReserved.
    out.extend_from_slice(&0u32.to_be_bytes());
    out.extend_from_slice(&0u32.to_be_bytes());
    out.extend_from_slice(&0u32.to_be_bytes());
    // srcRect / dstRect.
    out.extend_from_slice(&0i16.to_be_bytes());
    out.extend_from_slice(&0i16.to_be_bytes());
    out.extend_from_slice(&(height as i16).to_be_bytes());
    out.extend_from_slice(&(width as i16).to_be_bytes());
    out.extend_from_slice(&0i16.to_be_bytes());
    out.extend_from_slice(&0i16.to_be_bytes());
    out.extend_from_slice(&(height as i16).to_be_bytes());
    out.extend_from_slice(&(width as i16).to_be_bytes());
    // mode = srcCopy.
    out.extend_from_slice(&0u16.to_be_bytes());

    // Pixel data: 4 bytes per pixel, 0xFF R G B (drop input alpha
    // since cmpCount=3 means there's no alpha plane).
    for px in data.chunks_exact(4) {
        out.push(0xFF);
        out.push(px[0]);
        out.push(px[1]);
        out.push(px[2]);
    }

    // Word-align before terminator.
    if out.len() % 2 != 0 {
        out.push(0);
    }
    // OpEndPic.
    out.extend_from_slice(&0x00FFu16.to_be_bytes());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decoder::parse_pict;

    #[test]
    fn rejects_size_mismatch() {
        let err = encode_pict(2, 2, &[0u8; 8]).unwrap_err();
        assert!(matches!(err, PictError::InvalidData(_)));
    }

    #[test]
    fn rejects_zero_dim() {
        assert!(matches!(
            encode_pict(0, 1, &[]).unwrap_err(),
            PictError::InvalidData(_)
        ));
    }

    #[test]
    fn roundtrip_3x2_rgba() {
        let width = 3u32;
        let height = 2u32;
        let rgba: Vec<u8> = (0..(width * height * 4)).map(|i| (i * 37) as u8).collect();
        let encoded = encode_pict(width, height, &rgba).expect("encode failed");
        let img = parse_pict(&encoded).expect("decode failed");
        assert_eq!(img.width, width);
        assert_eq!(img.height, height);
        // Encoder drops alpha (cmpCount=3, decoder writes 0xFF for
        // those pixels). Round-trip just compares R/G/B bytes and
        // expects A = 0xFF on the decoded side.
        for y in 0..height as usize {
            for x in 0..width as usize {
                let src = (y * width as usize + x) * 4;
                let dst = src;
                assert_eq!(img.data[dst], rgba[src], "R mismatch ({x},{y})");
                assert_eq!(img.data[dst + 1], rgba[src + 1], "G mismatch ({x},{y})");
                assert_eq!(img.data[dst + 2], rgba[src + 2], "B mismatch ({x},{y})");
                assert_eq!(
                    img.data[dst + 3],
                    0xFF,
                    "A should be 0xFF on decode ({x},{y})"
                );
            }
        }
    }
}
