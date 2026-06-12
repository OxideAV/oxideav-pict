//! Synthetic v2 PICT round-trip integration tests.
//!
//! Round 1 doesn't ship a public encoder (PICT writing is round-2),
//! but the decoder needs *something* to decode against. So this test
//! file builds tiny synthetic PICT v2 byte streams by emitting the
//! exact opcodes our decoder is expected to understand and asserts
//! the decoded raster matches the original pixel data byte-for-byte.
//!
//! All synthesised streams here use the canonical layout per Inside
//! Macintosh: Imaging With QuickDraw §A-3.

use oxideav_pict::{blend_source, parse_pict, PictError, PictPixelFormat, Rgba, SourceMode};

/// Build a v2 PICT body (no 512-byte launch-stub prefix) containing a
/// single `DirectBitsRect` (`0x009A`) that holds `width × height`
/// 32-bit pixels in `0xFF R G B` interleaved layout.
fn build_v2_directbits_32bpp(width: u16, height: u16, rgba: &[u8]) -> Vec<u8> {
    assert_eq!(rgba.len(), (width as usize) * (height as usize) * 4);
    let row_bytes = (width as usize) * 4;
    assert!(row_bytes <= 0x3FFF, "rowBytes must fit in 14 bits");
    let mut buf = Vec::new();

    // Picture record header.
    buf.extend_from_slice(&0u16.to_be_bytes()); // picSize (ignored)
    buf.extend_from_slice(&0i16.to_be_bytes()); // picFrame top
    buf.extend_from_slice(&0i16.to_be_bytes()); // picFrame left
    buf.extend_from_slice(&(height as i16).to_be_bytes()); // bottom
    buf.extend_from_slice(&(width as i16).to_be_bytes()); // right

    // v2 version stanza: 0x0011 0x02FF.
    buf.extend_from_slice(&0x0011u16.to_be_bytes());
    buf.extend_from_slice(&0x02FFu16.to_be_bytes());
    // headerOp 0x0C00 + 24-byte payload (irrelevant content).
    buf.extend_from_slice(&0x0C00u16.to_be_bytes());
    buf.extend_from_slice(&[0u8; 24]);

    // DirectBitsRect opcode.
    buf.extend_from_slice(&0x009Au16.to_be_bytes());
    // baseAddr placeholder.
    buf.extend_from_slice(&0xFFu32.to_be_bytes());
    // rowBytes with top bit set (PixMap flag).
    let rb = (row_bytes as u16) | 0x8000;
    buf.extend_from_slice(&rb.to_be_bytes());
    // bounds.
    buf.extend_from_slice(&0i16.to_be_bytes());
    buf.extend_from_slice(&0i16.to_be_bytes());
    buf.extend_from_slice(&(height as i16).to_be_bytes());
    buf.extend_from_slice(&(width as i16).to_be_bytes());
    // pmVersion 0, packType 1 (uncompressed), packSize 0.
    buf.extend_from_slice(&0u16.to_be_bytes());
    buf.extend_from_slice(&1u16.to_be_bytes());
    buf.extend_from_slice(&0u32.to_be_bytes());
    // hRes / vRes = 72.0 fixed-point.
    buf.extend_from_slice(&0x00480000u32.to_be_bytes());
    buf.extend_from_slice(&0x00480000u32.to_be_bytes());
    // pixelType = 16 (RGBDirect), pixelSize = 32, cmpCount = 3, cmpSize = 8.
    buf.extend_from_slice(&16u16.to_be_bytes());
    buf.extend_from_slice(&32u16.to_be_bytes());
    buf.extend_from_slice(&3u16.to_be_bytes());
    buf.extend_from_slice(&8u16.to_be_bytes());
    // planeBytes, pmTable handle, pmReserved.
    buf.extend_from_slice(&0u32.to_be_bytes());
    buf.extend_from_slice(&0u32.to_be_bytes());
    buf.extend_from_slice(&0u32.to_be_bytes());
    // srcRect, dstRect, mode.
    buf.extend_from_slice(&0i16.to_be_bytes());
    buf.extend_from_slice(&0i16.to_be_bytes());
    buf.extend_from_slice(&(height as i16).to_be_bytes());
    buf.extend_from_slice(&(width as i16).to_be_bytes());
    buf.extend_from_slice(&0i16.to_be_bytes());
    buf.extend_from_slice(&0i16.to_be_bytes());
    buf.extend_from_slice(&(height as i16).to_be_bytes());
    buf.extend_from_slice(&(width as i16).to_be_bytes());
    buf.extend_from_slice(&0u16.to_be_bytes()); // mode = srcCopy

    // Pixel data: row_bytes per row, 0xFF R G B per pixel.
    for y in 0..height as usize {
        for x in 0..width as usize {
            let off = (y * width as usize + x) * 4;
            buf.push(0xFF); // alpha (ignored on disk for cmpCount=3)
            buf.push(rgba[off]);
            buf.push(rgba[off + 1]);
            buf.push(rgba[off + 2]);
        }
    }

    // Word-align before terminator if odd.
    if buf.len() % 2 != 0 {
        buf.push(0);
    }
    // OpEndPic.
    buf.extend_from_slice(&0x00FFu16.to_be_bytes());
    buf
}

/// Build a v2 PICT containing a single `PackBitsRect` (`0x0098`) with
/// 1-bit row data, PackBits-compressed.
fn build_v2_packbits_1bpp(width: u16, height: u16, bits_msb_first: &[u8]) -> Vec<u8> {
    let row_bytes = (width as usize).div_ceil(8);
    assert_eq!(bits_msb_first.len(), row_bytes * height as usize);
    assert!(
        row_bytes >= 8,
        "use rowBytes >= 8 to exercise the PackBits path"
    );

    let mut buf = Vec::new();
    buf.extend_from_slice(&0u16.to_be_bytes()); // picSize
    buf.extend_from_slice(&0i16.to_be_bytes());
    buf.extend_from_slice(&0i16.to_be_bytes());
    buf.extend_from_slice(&(height as i16).to_be_bytes());
    buf.extend_from_slice(&(width as i16).to_be_bytes());
    buf.extend_from_slice(&0x0011u16.to_be_bytes());
    buf.extend_from_slice(&0x02FFu16.to_be_bytes());
    buf.extend_from_slice(&0x0C00u16.to_be_bytes());
    buf.extend_from_slice(&[0u8; 24]);

    // PackBitsRect.
    buf.extend_from_slice(&0x0098u16.to_be_bytes());
    // rowBytes (top bit clear — BitMap, not PixMap).
    buf.extend_from_slice(&(row_bytes as u16).to_be_bytes());
    // bounds.
    buf.extend_from_slice(&0i16.to_be_bytes());
    buf.extend_from_slice(&0i16.to_be_bytes());
    buf.extend_from_slice(&(height as i16).to_be_bytes());
    buf.extend_from_slice(&(width as i16).to_be_bytes());
    // srcRect, dstRect.
    buf.extend_from_slice(&0i16.to_be_bytes());
    buf.extend_from_slice(&0i16.to_be_bytes());
    buf.extend_from_slice(&(height as i16).to_be_bytes());
    buf.extend_from_slice(&(width as i16).to_be_bytes());
    buf.extend_from_slice(&0i16.to_be_bytes());
    buf.extend_from_slice(&0i16.to_be_bytes());
    buf.extend_from_slice(&(height as i16).to_be_bytes());
    buf.extend_from_slice(&(width as i16).to_be_bytes());
    buf.extend_from_slice(&0u16.to_be_bytes()); // mode = srcCopy

    // Per-row PackBits-compressed data.
    for y in 0..height as usize {
        let row = &bits_msb_first[y * row_bytes..(y + 1) * row_bytes];
        let encoded = packbits_encode(row);
        if row_bytes > 250 {
            buf.extend_from_slice(&(encoded.len() as u16).to_be_bytes());
        } else {
            buf.push(encoded.len() as u8);
        }
        buf.extend_from_slice(&encoded);
    }

    if buf.len() % 2 != 0 {
        buf.push(0);
    }
    buf.extend_from_slice(&0x00FFu16.to_be_bytes());
    buf
}

/// Test-local PackBits encoder. Mirrors the one in `packbits.rs`'s
/// test module so this integration test is self-contained.
fn packbits_encode(src: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(src.len() + src.len() / 64);
    let mut i = 0usize;
    while i < src.len() {
        let mut run = 1usize;
        while run < 128 && i + run < src.len() && src[i + run] == src[i] {
            run += 1;
        }
        if run >= 3 {
            out.push((1i32 - run as i32) as i8 as u8);
            out.push(src[i]);
            i += run;
            continue;
        }
        let mut raw = 1usize;
        while raw < 128 && i + raw < src.len() {
            let s = i + raw;
            if s + 2 < src.len() && src[s] == src[s + 1] && src[s + 1] == src[s + 2] {
                break;
            }
            raw += 1;
        }
        out.push((raw - 1) as u8);
        out.extend_from_slice(&src[i..i + raw]);
        i += raw;
    }
    out
}

#[test]
fn directbits_32bpp_roundtrip() {
    let width = 4u16;
    let height = 3u16;
    let rgba: Vec<u8> = (0..(width as usize) * (height as usize))
        .flat_map(|i| {
            let i = i as u8;
            [
                i.wrapping_mul(17),
                i.wrapping_mul(31),
                i.wrapping_mul(53),
                0xFF,
            ]
        })
        .collect();
    let pict = build_v2_directbits_32bpp(width, height, &rgba);
    let img = parse_pict(&pict).expect("decode failed");
    assert_eq!(img.width, width as u32);
    assert_eq!(img.height, height as u32);
    assert_eq!(img.pixel_format, PictPixelFormat::Rgba);
    assert_eq!(img.data.len(), rgba.len());
    assert_eq!(img.data, rgba);
}

#[test]
fn directbits_with_512_byte_prefix() {
    let width = 2u16;
    let height = 2u16;
    let rgba = vec![
        0u8, 64, 128, 255, 200, 100, 50, 255, 1, 2, 3, 255, 4, 5, 6, 255,
    ];
    let body = build_v2_directbits_32bpp(width, height, &rgba);
    let mut prefixed = vec![0u8; 512];
    prefixed.extend_from_slice(&body);
    let img = parse_pict(&prefixed).expect("decode-with-prefix failed");
    assert_eq!(img.width, 2);
    assert_eq!(img.height, 2);
    assert_eq!(img.data, rgba);
}

#[test]
fn packbits_1bpp_roundtrip() {
    // 64 px wide × 4 rows = 8 bytes per row × 4 = 32 bytes of 1-bit data.
    // Row 0: alternating 0xAA 0x55 ... -> stipple.
    // Row 1: all 0xFF -> all black.
    // Row 2: all 0x00 -> all white.
    // Row 3: 0xF0 first byte, 0x0F rest -> mixed for compressibility check.
    let width = 64u16;
    let height = 4u16;
    let mut bits = Vec::new();
    bits.extend_from_slice(&[0xAA, 0x55, 0xAA, 0x55, 0xAA, 0x55, 0xAA, 0x55]);
    bits.extend_from_slice(&[0xFF; 8]);
    bits.extend_from_slice(&[0x00; 8]);
    bits.extend_from_slice(&[0xF0, 0x0F, 0x0F, 0x0F, 0x0F, 0x0F, 0x0F, 0x0F]);
    let pict = build_v2_packbits_1bpp(width, height, &bits);
    let img = parse_pict(&pict).expect("packbits decode failed");
    assert_eq!(img.width, 64);
    assert_eq!(img.height, 4);
    assert_eq!(img.pixel_format, PictPixelFormat::Rgba);
    // Spot check: row 1 is all-black pixels = (0,0,0,255).
    for x in 0..64usize {
        let off = (64 + x) * 4;
        assert_eq!(img.data[off], 0x00);
        assert_eq!(img.data[off + 1], 0x00);
        assert_eq!(img.data[off + 2], 0x00);
        assert_eq!(img.data[off + 3], 0xFF);
    }
    // Row 2: all-white = (255,255,255,255).
    for x in 0..64usize {
        let off = (2 * 64 + x) * 4;
        assert_eq!(img.data[off], 0xFF);
        assert_eq!(img.data[off + 3], 0xFF);
    }
    // Row 0 first pixel: bit 7 of 0xAA = 1 -> black.
    assert_eq!(img.data[0], 0x00);
    assert_eq!(img.data[3], 0xFF);
    // Row 0 second pixel: bit 6 of 0xAA = 0 -> white.
    assert_eq!(img.data[4], 0xFF);
}

#[test]
fn opcode_stream_with_state_then_raster() {
    // Synthesise a stream that emits a bunch of pen / colour state
    // opcodes BEFORE the DirectBitsRect, exercising the
    // fixed_operand_size skip table.
    let width = 2u16;
    let height = 2u16;
    let rgba = vec![
        10, 20, 30, 255, 40, 50, 60, 255, 70, 80, 90, 255, 100, 110, 120, 255,
    ];
    let raster = build_v2_directbits_32bpp(width, height, &rgba);
    // raster contains: 18-byte header + opcodes ending in 0x00FF.
    // We need to splice in extra opcodes BETWEEN the headerOp payload
    // (ends at offset 10 + 4 + 2 + 24 = 40) and the DirectBitsRect at
    // offset 40. The simpler approach: rebuild with prepended opcodes.
    let mut buf = Vec::new();
    buf.extend_from_slice(&raster[..40]);
    // Insert: NOP + RGBFgCol + LongComment + ClipRgn + ShortLine + OvSize.
    buf.extend_from_slice(&0x0000u16.to_be_bytes()); // NOP
    buf.extend_from_slice(&0x001Au16.to_be_bytes()); // RGBFgCol
    buf.extend_from_slice(&[0xFF, 0x00, 0xAA, 0xBB, 0x12, 0x34]);
    buf.extend_from_slice(&0x00A1u16.to_be_bytes()); // LongComment
    buf.extend_from_slice(&0x0007u16.to_be_bytes()); // kind = 7
    buf.extend_from_slice(&0x0004u16.to_be_bytes()); // size = 4
    buf.extend_from_slice(&[1, 2, 3, 4]); // payload
    buf.extend_from_slice(&0x0001u16.to_be_bytes()); // ClipRgn
    buf.extend_from_slice(&0x000Au16.to_be_bytes()); // rgn size = 10
    buf.extend_from_slice(&0i16.to_be_bytes());
    buf.extend_from_slice(&0i16.to_be_bytes());
    buf.extend_from_slice(&(height as i16).to_be_bytes());
    buf.extend_from_slice(&(width as i16).to_be_bytes());
    buf.extend_from_slice(&0x0022u16.to_be_bytes()); // ShortLine (6 bytes)
    buf.extend_from_slice(&[0, 0, 0, 0, 1, 1]);
    buf.extend_from_slice(&0x000Bu16.to_be_bytes()); // OvSize (4 bytes)
    buf.extend_from_slice(&[0, 5, 0, 5]);
    // Now the actual DirectBitsRect (everything from offset 40 onwards).
    buf.extend_from_slice(&raster[40..]);
    let img = parse_pict(&buf).expect("state+raster decode failed");
    // The spliced `RGBFgCol` set the foreground to (0xFF, 0xAA, 0x12)
    // (high byte of each 16-bit channel), so the DirectBitsRect's
    // `mode = 0` (srcCopy) word colorizes the blit per Inside
    // Macintosh Â§4 Table 4-1 / Â§4-33: each source channel's
    // closeness to black applies that portion of the foreground, the
    // remainder applies the (default white) background.
    let fg = Rgba::new(0xFF, 0xAA, 0x12, 0xFF);
    for px in 0..4usize {
        let src = Rgba::new(
            rgba[px * 4],
            rgba[px * 4 + 1],
            rgba[px * 4 + 2],
            rgba[px * 4 + 3],
        );
        let want = blend_source(SourceMode::SrcCopy, src, Rgba::WHITE, fg, Rgba::WHITE);
        assert_eq!(
            &img.data[px * 4..px * 4 + 4],
            &[want.r, want.g, want.b, want.a],
            "pixel {px}"
        );
    }
}

#[test]
fn no_raster_returns_no_raster_error() {
    // Picture whose only opcode is OpEndPic.
    let mut buf = Vec::new();
    buf.extend_from_slice(&0u16.to_be_bytes());
    buf.extend_from_slice(&0i16.to_be_bytes());
    buf.extend_from_slice(&0i16.to_be_bytes());
    buf.extend_from_slice(&100i16.to_be_bytes());
    buf.extend_from_slice(&100i16.to_be_bytes());
    buf.extend_from_slice(&0x0011u16.to_be_bytes());
    buf.extend_from_slice(&0x02FFu16.to_be_bytes());
    buf.extend_from_slice(&0x0C00u16.to_be_bytes());
    buf.extend_from_slice(&[0u8; 24]);
    buf.extend_from_slice(&0x00FFu16.to_be_bytes());
    let err = parse_pict(&buf).unwrap_err();
    assert_eq!(err, PictError::NoRaster);
}

#[test]
fn truncated_stream_errors() {
    // Header only, opcode stream truncated mid-headerOp payload.
    let mut buf = Vec::new();
    buf.extend_from_slice(&0u16.to_be_bytes());
    buf.extend_from_slice(&0i16.to_be_bytes());
    buf.extend_from_slice(&0i16.to_be_bytes());
    buf.extend_from_slice(&100i16.to_be_bytes());
    buf.extend_from_slice(&100i16.to_be_bytes());
    buf.extend_from_slice(&0x0011u16.to_be_bytes());
    buf.extend_from_slice(&0x02FFu16.to_be_bytes());
    buf.extend_from_slice(&0x0C00u16.to_be_bytes());
    // Only 4 bytes of the 24-byte headerOp payload.
    buf.extend_from_slice(&[0u8; 4]);
    let err = parse_pict(&buf).unwrap_err();
    assert!(matches!(err, PictError::InvalidData(_)));
}

#[test]
fn rejects_garbage() {
    let err = parse_pict(&[0xDE, 0xAD, 0xBE, 0xEF]).unwrap_err();
    assert!(matches!(err, PictError::InvalidData(_)));
}
