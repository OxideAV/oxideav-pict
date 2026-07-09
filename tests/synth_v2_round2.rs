//! Round-2 integration tests covering the new sub-features:
//!
//! * Drawing-command rasteriser end-to-end (FrameRect / PaintRect /
//!   FillOval / FramePoly / Line) folded onto the canvas.
//! * `DirectBitsRect` packTypes 2, 3 and 4.
//! * Region inversion-data parsing via PaintRgn.
//! * `CompressedQuickTime` opcode parsing (skips embedded payload
//!   without wedging the surrounding decode).
//! * v1 raster opcode (`PackBitsRect 0x98`) decoding.
//! * Encoder round-trip via `encode_pict` -> `parse_pict`.

use oxideav_pict::{encode_pict, parse_pict, PictPixelFormat};

/// Build a v2 PICT body containing exactly the supplied opcode bytes
/// (with a sane picFrame and the standard v2 header stanza).
fn build_v2_with_opcodes(width: u16, height: u16, opcodes: &[u8]) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(&0u16.to_be_bytes()); // picSize
    buf.extend_from_slice(&0i16.to_be_bytes()); // picFrame top
    buf.extend_from_slice(&0i16.to_be_bytes()); // left
    buf.extend_from_slice(&(height as i16).to_be_bytes()); // bottom
    buf.extend_from_slice(&(width as i16).to_be_bytes()); // right
    buf.extend_from_slice(&0x0011u16.to_be_bytes());
    buf.extend_from_slice(&0x02FFu16.to_be_bytes());
    buf.extend_from_slice(&0x0C00u16.to_be_bytes());
    buf.extend_from_slice(&[0u8; 24]);
    buf.extend_from_slice(opcodes);
    if buf.len() % 2 != 0 {
        buf.push(0);
    }
    buf.extend_from_slice(&0x00FFu16.to_be_bytes());
    buf
}

#[test]
fn paintrect_rasterises() {
    // 10x10 picture; PaintRect (0x0031) fills (1,1,9,9) with current
    // fg colour. Default fg = black.
    let mut ops = Vec::new();
    ops.extend_from_slice(&0x0031u16.to_be_bytes());
    ops.extend_from_slice(&1i16.to_be_bytes()); // top
    ops.extend_from_slice(&1i16.to_be_bytes()); // left
    ops.extend_from_slice(&9i16.to_be_bytes()); // bottom
    ops.extend_from_slice(&9i16.to_be_bytes()); // right
    let pict = build_v2_with_opcodes(10, 10, &ops);
    let img = parse_pict(&pict).expect("decode failed");
    assert_eq!(img.width, 10);
    assert_eq!(img.height, 10);
    assert_eq!(img.pixel_format, PictPixelFormat::Rgba);
    // Outside the rect: white paper.
    let off_outside = 0_usize;
    assert_eq!(
        &img.data[off_outside..off_outside + 4],
        &[255, 255, 255, 255]
    );
    // Inside the rect: black ink.
    let off_inside = (5 * 10 + 5) * 4;
    assert_eq!(&img.data[off_inside..off_inside + 4], &[0, 0, 0, 255]);
    // Corner of canvas: white.
    let off_corner = (9 * 10 + 9) * 4;
    assert_eq!(&img.data[off_corner..off_corner + 4], &[255, 255, 255, 255]);
}

#[test]
fn rgb_fg_then_paintrect() {
    // RGBFgCol -> red, then PaintRect (1,1,5,5).
    let mut ops = Vec::new();
    ops.extend_from_slice(&0x001Au16.to_be_bytes()); // RGBFgCol
    ops.extend_from_slice(&0xFFFFu16.to_be_bytes()); // R high byte
    ops.extend_from_slice(&0x0000u16.to_be_bytes()); // G
    ops.extend_from_slice(&0x0000u16.to_be_bytes()); // B
    ops.extend_from_slice(&0x0031u16.to_be_bytes()); // PaintRect
    ops.extend_from_slice(&1i16.to_be_bytes());
    ops.extend_from_slice(&1i16.to_be_bytes());
    ops.extend_from_slice(&5i16.to_be_bytes());
    ops.extend_from_slice(&5i16.to_be_bytes());
    let pict = build_v2_with_opcodes(8, 8, &ops);
    let img = parse_pict(&pict).expect("decode failed");
    let off = (3 * 8 + 3) * 4;
    assert_eq!(&img.data[off..off + 4], &[255, 0, 0, 255]);
}

#[test]
fn line_rasterises() {
    // RGBFgCol -> blue; Line (0,0)->(7,7).
    let mut ops = Vec::new();
    ops.extend_from_slice(&0x001Au16.to_be_bytes());
    ops.extend_from_slice(&0x0000u16.to_be_bytes());
    ops.extend_from_slice(&0x0000u16.to_be_bytes());
    ops.extend_from_slice(&0xFFFFu16.to_be_bytes());
    ops.extend_from_slice(&0x0020u16.to_be_bytes()); // Line
    ops.extend_from_slice(&0i16.to_be_bytes()); // pt0 v
    ops.extend_from_slice(&0i16.to_be_bytes()); // pt0 h
    ops.extend_from_slice(&7i16.to_be_bytes()); // pt1 v
    ops.extend_from_slice(&7i16.to_be_bytes()); // pt1 h
    let pict = build_v2_with_opcodes(8, 8, &ops);
    let img = parse_pict(&pict).expect("decode failed");
    // Diagonal pixels should be blue.
    for i in 0..8 {
        let off = (i * 8 + i) * 4;
        assert_eq!(
            &img.data[off..off + 4],
            &[0, 0, 255, 255],
            "diagonal pixel ({i},{i}) not blue"
        );
    }
}

#[test]
fn fillpoly_rasterises_triangle() {
    // FillPoly (0x0074) with triangle vertices (in v, h order):
    //   (1, 1), (1, 9), (9, 5)
    // poly_size = 10 (header) + 4*3 = 22 bytes.
    let mut ops = Vec::new();
    ops.extend_from_slice(&0x0074u16.to_be_bytes()); // FillPoly
    ops.extend_from_slice(&22u16.to_be_bytes()); // poly_size
    ops.extend_from_slice(&1i16.to_be_bytes()); // bbox top
    ops.extend_from_slice(&1i16.to_be_bytes()); // bbox left
    ops.extend_from_slice(&9i16.to_be_bytes()); // bbox bottom
    ops.extend_from_slice(&9i16.to_be_bytes()); // bbox right
    ops.extend_from_slice(&1i16.to_be_bytes()); // v
    ops.extend_from_slice(&5i16.to_be_bytes()); // h (apex)
    ops.extend_from_slice(&8i16.to_be_bytes());
    ops.extend_from_slice(&1i16.to_be_bytes());
    ops.extend_from_slice(&8i16.to_be_bytes());
    ops.extend_from_slice(&9i16.to_be_bytes());
    let pict = build_v2_with_opcodes(10, 10, &ops);
    let img = parse_pict(&pict).expect("decode failed");
    // Centre of triangle.
    let off = (5 * 10 + 5) * 4;
    assert_eq!(&img.data[off..off + 4], &[0, 0, 0, 255]);
    // Outside the triangle (top corners).
    let off_corner = 0_usize;
    assert_eq!(&img.data[off_corner..off_corner + 4], &[255, 255, 255, 255]);
}

#[test]
fn paintrgn_rectangular_region() {
    // PaintRgn (0x0081) with a rectangular region (rgnSize=10).
    let mut ops = Vec::new();
    ops.extend_from_slice(&0x0081u16.to_be_bytes());
    ops.extend_from_slice(&10u16.to_be_bytes()); // rgnSize
    ops.extend_from_slice(&2i16.to_be_bytes()); // bbox
    ops.extend_from_slice(&2i16.to_be_bytes());
    ops.extend_from_slice(&6i16.to_be_bytes());
    ops.extend_from_slice(&6i16.to_be_bytes());
    let pict = build_v2_with_opcodes(8, 8, &ops);
    let img = parse_pict(&pict).expect("decode failed");
    // (3, 3) inside region.
    let off = (3 * 8 + 3) * 4;
    assert_eq!(&img.data[off..off + 4], &[0, 0, 0, 255]);
    // (1, 1) outside.
    let off_out = (8 + 1) * 4;
    assert_eq!(&img.data[off_out..off_out + 4], &[255, 255, 255, 255]);
}

#[test]
fn directbits_packtype2_24bpp() {
    // 4x2 packType=2 DirectBitsRect: 3 bytes per pixel (R G B), no
    // pad. rowBytes = 12.
    let width = 4u16;
    let height = 2u16;
    let stride = (width as usize) * 3;
    let mut ops = Vec::new();
    ops.extend_from_slice(&0x009Au16.to_be_bytes()); // DirectBitsRect
    ops.extend_from_slice(&0xFFu32.to_be_bytes()); // baseAddr
    ops.extend_from_slice(&((stride as u16) | 0x8000).to_be_bytes()); // rowBytes
    ops.extend_from_slice(&0i16.to_be_bytes()); // bounds top
    ops.extend_from_slice(&0i16.to_be_bytes());
    ops.extend_from_slice(&(height as i16).to_be_bytes());
    ops.extend_from_slice(&(width as i16).to_be_bytes());
    ops.extend_from_slice(&0u16.to_be_bytes()); // pmVersion
    ops.extend_from_slice(&2u16.to_be_bytes()); // packType = 2
    ops.extend_from_slice(&0u32.to_be_bytes()); // packSize
    ops.extend_from_slice(&0x00480000u32.to_be_bytes()); // hRes
    ops.extend_from_slice(&0x00480000u32.to_be_bytes()); // vRes
    ops.extend_from_slice(&16u16.to_be_bytes()); // pixelType
    ops.extend_from_slice(&32u16.to_be_bytes()); // pixelSize
    ops.extend_from_slice(&3u16.to_be_bytes()); // cmpCount
    ops.extend_from_slice(&8u16.to_be_bytes()); // cmpSize
    ops.extend_from_slice(&0u32.to_be_bytes());
    ops.extend_from_slice(&0u32.to_be_bytes());
    ops.extend_from_slice(&0u32.to_be_bytes());
    // srcRect / dstRect / mode.
    for _ in 0..2 {
        ops.extend_from_slice(&0i16.to_be_bytes());
        ops.extend_from_slice(&0i16.to_be_bytes());
        ops.extend_from_slice(&(height as i16).to_be_bytes());
        ops.extend_from_slice(&(width as i16).to_be_bytes());
    }
    ops.extend_from_slice(&0u16.to_be_bytes()); // mode
                                                // Pixel data: row 0 = R G B per pixel.
    for _ in 0..(width * height) {
        ops.extend_from_slice(&[0xFF, 0x80, 0x40]);
    }
    let pict = build_v2_with_opcodes(width, height, &ops);
    let img = parse_pict(&pict).expect("decode failed");
    assert_eq!(img.width, width as u32);
    let off = 0;
    assert_eq!(&img.data[off..off + 4], &[0xFF, 0x80, 0x40, 0xFF]);
}

#[test]
fn directbits_packtype3_16bpp_packbits() {
    // 4x1 packType=3 DirectBitsRect: 16-bit A1R5G5B5, PackBits-RLE
    // with u16 unit replication.
    // We encode 4 pixels as 1 raw packet of 4 u16s.
    // A1R5G5B5 0x7C00 = 11111-00000-00000 = pure red.
    let width = 4u16;
    let height = 1u16;
    let row_bytes = width as usize * 2;
    let mut ops = Vec::new();
    ops.extend_from_slice(&0x009Au16.to_be_bytes());
    ops.extend_from_slice(&0xFFu32.to_be_bytes());
    ops.extend_from_slice(&((row_bytes as u16) | 0x8000).to_be_bytes());
    ops.extend_from_slice(&0i16.to_be_bytes());
    ops.extend_from_slice(&0i16.to_be_bytes());
    ops.extend_from_slice(&(height as i16).to_be_bytes());
    ops.extend_from_slice(&(width as i16).to_be_bytes());
    ops.extend_from_slice(&0u16.to_be_bytes());
    ops.extend_from_slice(&3u16.to_be_bytes()); // packType = 3
    ops.extend_from_slice(&0u32.to_be_bytes());
    ops.extend_from_slice(&0x00480000u32.to_be_bytes());
    ops.extend_from_slice(&0x00480000u32.to_be_bytes());
    ops.extend_from_slice(&16u16.to_be_bytes());
    ops.extend_from_slice(&16u16.to_be_bytes()); // pixelSize=16
    ops.extend_from_slice(&3u16.to_be_bytes());
    ops.extend_from_slice(&5u16.to_be_bytes()); // cmpSize=5
    ops.extend_from_slice(&0u32.to_be_bytes());
    ops.extend_from_slice(&0u32.to_be_bytes());
    ops.extend_from_slice(&0u32.to_be_bytes());
    for _ in 0..2 {
        ops.extend_from_slice(&0i16.to_be_bytes());
        ops.extend_from_slice(&0i16.to_be_bytes());
        ops.extend_from_slice(&(height as i16).to_be_bytes());
        ops.extend_from_slice(&(width as i16).to_be_bytes());
    }
    ops.extend_from_slice(&0u16.to_be_bytes());
    // PackBits-u16 row: byteCount + flag=-3 (run packet, 4 reps of
    // next u16).
    // byteCount = 1 (flag) + 2 (u16) = 3.
    ops.push(3); // byteCount (1 byte since rowBytes < 250)
    ops.push((1i32 - 4i32) as i8 as u8); // flag = -3
    ops.extend_from_slice(&0x7C00u16.to_be_bytes()); // pure red u16
    let pict = build_v2_with_opcodes(width, height, &ops);
    let img = parse_pict(&pict).expect("decode failed");
    // All 4 pixels should decode to red (R5=31 -> 0xFF).
    for x in 0..4 {
        let off = x * 4;
        assert_eq!(img.data[off], 0xFF, "pixel {x} R");
        assert_eq!(img.data[off + 1], 0, "pixel {x} G");
        assert_eq!(img.data[off + 2], 0, "pixel {x} B");
    }
}

#[test]
fn directbits_packtype4_planar_packbits() {
    // 4x1 packType=4 DirectBitsRect: cmpCount=3, R/G/B planes,
    // PackBits per plane.
    let width = 4u16;
    let height = 1u16;
    // For packType 4, rowBytes is the *unpacked* row stride (4 bytes
    // per pixel as if packType=1). So rowBytes = 16.
    let row_bytes = width as usize * 4;
    let mut ops = Vec::new();
    ops.extend_from_slice(&0x009Au16.to_be_bytes());
    ops.extend_from_slice(&0xFFu32.to_be_bytes());
    ops.extend_from_slice(&((row_bytes as u16) | 0x8000).to_be_bytes());
    ops.extend_from_slice(&0i16.to_be_bytes());
    ops.extend_from_slice(&0i16.to_be_bytes());
    ops.extend_from_slice(&(height as i16).to_be_bytes());
    ops.extend_from_slice(&(width as i16).to_be_bytes());
    ops.extend_from_slice(&0u16.to_be_bytes());
    ops.extend_from_slice(&4u16.to_be_bytes()); // packType = 4
    ops.extend_from_slice(&0u32.to_be_bytes());
    ops.extend_from_slice(&0x00480000u32.to_be_bytes());
    ops.extend_from_slice(&0x00480000u32.to_be_bytes());
    ops.extend_from_slice(&16u16.to_be_bytes());
    ops.extend_from_slice(&32u16.to_be_bytes());
    ops.extend_from_slice(&3u16.to_be_bytes());
    ops.extend_from_slice(&8u16.to_be_bytes());
    ops.extend_from_slice(&0u32.to_be_bytes());
    ops.extend_from_slice(&0u32.to_be_bytes());
    ops.extend_from_slice(&0u32.to_be_bytes());
    for _ in 0..2 {
        ops.extend_from_slice(&0i16.to_be_bytes());
        ops.extend_from_slice(&0i16.to_be_bytes());
        ops.extend_from_slice(&(height as i16).to_be_bytes());
        ops.extend_from_slice(&(width as i16).to_be_bytes());
    }
    ops.extend_from_slice(&0u16.to_be_bytes());
    // 3 planes × 4 bytes each. Each plane PackBits-RLE = run of 4
    // copies of one byte: flag=-3, byte. So each plane is 2 bytes.
    // Total packed planes = 6 bytes, plus byteCount prefix.
    ops.push(6); // byteCount
                 // R plane: all 0xFF.
    ops.push((1i32 - 4i32) as i8 as u8);
    ops.push(0xFF);
    // G plane: all 0x80.
    ops.push((1i32 - 4i32) as i8 as u8);
    ops.push(0x80);
    // B plane: all 0x40.
    ops.push((1i32 - 4i32) as i8 as u8);
    ops.push(0x40);
    let pict = build_v2_with_opcodes(width, height, &ops);
    let img = parse_pict(&pict).expect("decode failed");
    for x in 0..4 {
        let off = x * 4;
        assert_eq!(img.data[off], 0xFF, "pixel {x} R");
        assert_eq!(img.data[off + 1], 0x80, "pixel {x} G");
        assert_eq!(img.data[off + 2], 0x40, "pixel {x} B");
        assert_eq!(img.data[off + 3], 0xFF, "pixel {x} A");
    }
}

#[test]
fn compressed_quicktime_skipped_then_paintrect() {
    // CompressedQuickTime opcode followed by PaintRect — verify the
    // walker doesn't wedge on the QT opcode and correctly resumes.
    // §A-3 Table A-2: the Long is the DATA length, excluding itself
    // (round 401 conformance fix) — 12 here for 12 payload bytes.
    let qt_data_length = 12u32;
    let mut ops = Vec::new();
    ops.extend_from_slice(&0x8200u16.to_be_bytes());
    ops.extend_from_slice(&qt_data_length.to_be_bytes());
    ops.extend_from_slice(&[0u8; 12]);
    // Now PaintRect to prove the walker resumed.
    ops.extend_from_slice(&0x0031u16.to_be_bytes());
    ops.extend_from_slice(&1i16.to_be_bytes());
    ops.extend_from_slice(&1i16.to_be_bytes());
    ops.extend_from_slice(&5i16.to_be_bytes());
    ops.extend_from_slice(&5i16.to_be_bytes());
    let pict = build_v2_with_opcodes(8, 8, &ops);
    let img = parse_pict(&pict).expect("decode failed");
    let off = (3 * 8 + 3) * 4;
    assert_eq!(&img.data[off..off + 4], &[0, 0, 0, 255]);
}

#[test]
fn v1_packbits_decode() {
    // Build a v1 PICT with PackBitsRect (0x98). Layout: 10-byte
    // picture header, version stanza (0x11 0x01), then the opcode
    // stream. v1 has 8-bit opcodes, no word alignment.
    let width = 16u16;
    let height = 1u16;
    let row_bytes = (width as usize).div_ceil(8); // = 2
    let mut buf = Vec::new();
    buf.extend_from_slice(&0u16.to_be_bytes());
    buf.extend_from_slice(&0i16.to_be_bytes());
    buf.extend_from_slice(&0i16.to_be_bytes());
    buf.extend_from_slice(&(height as i16).to_be_bytes());
    buf.extend_from_slice(&(width as i16).to_be_bytes());
    // v1 sentinel: 0x11 0x01.
    buf.push(0x11);
    buf.push(0x01);
    // PackBitsRect (0x98).
    buf.push(0x98);
    buf.extend_from_slice(&(row_bytes as u16).to_be_bytes());
    buf.extend_from_slice(&0i16.to_be_bytes());
    buf.extend_from_slice(&0i16.to_be_bytes());
    buf.extend_from_slice(&(height as i16).to_be_bytes());
    buf.extend_from_slice(&(width as i16).to_be_bytes());
    for _ in 0..2 {
        buf.extend_from_slice(&0i16.to_be_bytes());
        buf.extend_from_slice(&0i16.to_be_bytes());
        buf.extend_from_slice(&(height as i16).to_be_bytes());
        buf.extend_from_slice(&(width as i16).to_be_bytes());
    }
    buf.extend_from_slice(&0u16.to_be_bytes()); // mode
                                                // row_bytes=2 < 8, so each row is raw. 2 bytes of 0xFF (all bits
                                                // set -> all black pixels).
    buf.push(0xFF);
    buf.push(0xFF);
    // OpEndPic = 0xFF.
    buf.push(0xFF);
    let img = parse_pict(&buf).expect("v1 decode failed");
    assert_eq!(img.width, 16);
    assert_eq!(img.height, 1);
    // All 16 pixels should be black.
    for x in 0..16 {
        let off = x * 4;
        assert_eq!(&img.data[off..off + 4], &[0, 0, 0, 255]);
    }
}

#[test]
fn encode_decode_roundtrip_8x8() {
    let width = 8u32;
    let height = 8u32;
    let mut rgba = vec![0u8; (width * height * 4) as usize];
    for y in 0..height as usize {
        for x in 0..width as usize {
            let off = (y * width as usize + x) * 4;
            rgba[off] = (x * 32) as u8;
            rgba[off + 1] = (y * 32) as u8;
            rgba[off + 2] = ((x + y) * 16) as u8;
            rgba[off + 3] = 0xFF;
        }
    }
    let pict = encode_pict(width, height, &rgba).expect("encode failed");
    // Roundtrip. Encoder uses cmpCount=3 so alpha is dropped; decoder
    // synthesises 0xFF alpha, which matches our test data.
    let img = parse_pict(&pict).expect("decode roundtrip failed");
    assert_eq!(img.width, width);
    assert_eq!(img.height, height);
    assert_eq!(img.data, rgba);
}
