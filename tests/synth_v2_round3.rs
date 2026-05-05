//! Round-3 integration tests covering new encoder features:
//!
//! * `encode_pict_v2` with packType 2 and packType 4 round-trips.
//! * `encode_pict_v1` produces a decodable v1 stream.
//! * `encode_pict_v2_with_clip` injects a valid `ClipRgn` opcode.
//! * `pixel_data_sizes` measurements: packType 4 compresses solid blocks,
//!   packType 2 is exactly 75 % of raw.
//! * v1 decoder now handles `DirectBitsRect` (opcode 0x9A) in v1 streams.

use oxideav_pict::{
    encode_pict_v1, encode_pict_v2, encode_pict_v2_with_clip, parse_pict, pixel_data_sizes,
    PackType,
};

// ---- packType 2 (Packed24) round-trip ----

#[test]
fn packtype2_roundtrip_16x16() {
    let width = 16u32;
    let height = 16u32;
    let rgba: Vec<u8> = (0..(width * height * 4)).map(|i| (i * 31) as u8).collect();
    let enc = encode_pict_v2(width, height, &rgba, PackType::Packed24).unwrap();
    let img = parse_pict(&enc).unwrap();
    assert_eq!(img.width, width);
    assert_eq!(img.height, height);
    for y in 0..height as usize {
        for x in 0..width as usize {
            let off = (y * width as usize + x) * 4;
            assert_eq!(img.data[off], rgba[off], "R ({x},{y})");
            assert_eq!(img.data[off + 1], rgba[off + 1], "G ({x},{y})");
            assert_eq!(img.data[off + 2], rgba[off + 2], "B ({x},{y})");
            assert_eq!(img.data[off + 3], 0xFF, "A ({x},{y})");
        }
    }
}

// ---- packType 4 (ComponentPackBits) round-trips ----

#[test]
fn packtype4_roundtrip_solid_colour() {
    let width = 32u32;
    let height = 32u32;
    // Solid green: every R=0x00 G=0xFF B=0x00 A=0xFF.
    let mut rgba = vec![0u8; (width * height * 4) as usize];
    for i in 0..width as usize * height as usize {
        rgba[i * 4 + 1] = 0xFF;
        rgba[i * 4 + 3] = 0xFF;
    }
    let enc = encode_pict_v2(width, height, &rgba, PackType::ComponentPackBits).unwrap();
    let img = parse_pict(&enc).unwrap();
    assert_eq!(img.width, width);
    assert_eq!(img.height, height);
    for i in 0..width as usize * height as usize {
        let off = i * 4;
        assert_eq!(img.data[off], 0x00, "R");
        assert_eq!(img.data[off + 1], 0xFF, "G");
        assert_eq!(img.data[off + 2], 0x00, "B");
        assert_eq!(img.data[off + 3], 0xFF, "A");
    }
}

#[test]
fn packtype4_roundtrip_gradient() {
    let width = 16u32;
    let height = 16u32;
    let mut rgba = vec![0u8; (width * height * 4) as usize];
    for y in 0..height as usize {
        for x in 0..width as usize {
            let off = (y * width as usize + x) * 4;
            rgba[off] = (x * 16) as u8;
            rgba[off + 1] = (y * 16) as u8;
            rgba[off + 2] = 0x80;
            rgba[off + 3] = 0xFF;
        }
    }
    let enc = encode_pict_v2(width, height, &rgba, PackType::ComponentPackBits).unwrap();
    let img = parse_pict(&enc).unwrap();
    for y in 0..height as usize {
        for x in 0..width as usize {
            let off = (y * width as usize + x) * 4;
            assert_eq!(img.data[off], rgba[off], "R ({x},{y})");
            assert_eq!(img.data[off + 1], rgba[off + 1], "G ({x},{y})");
            assert_eq!(img.data[off + 2], rgba[off + 2], "B ({x},{y})");
            assert_eq!(img.data[off + 3], 0xFF, "A ({x},{y})");
        }
    }
}

// ---- byte-savings measurements ----

#[test]
fn packtype4_smaller_than_raw_for_solid() {
    let width = 64u32;
    let height = 64u32;
    // Solid colour: every pixel the same → maximum RLE savings.
    let rgba = vec![0x77u8; (width * height * 4) as usize];
    let (raw, packed) = pixel_data_sizes(width, height, &rgba, PackType::ComponentPackBits);
    // Each row: 3 planes × 2 bytes each (1 run packet) + 1 byteCount =
    // 7 bytes, vs 64×4=256 raw. At least 95 % savings.
    assert!(
        packed * 20 < raw,
        "solid packType4 should be > 95% smaller than raw: packed={packed} raw={raw}"
    );
}

#[test]
fn packtype2_is_exactly_75_pct_of_raw() {
    let width = 100u32;
    let height = 50u32;
    let rgba = vec![0u8; (width * height * 4) as usize];
    let (raw, packed) = pixel_data_sizes(width, height, &rgba, PackType::Packed24);
    assert_eq!(packed * 4, raw * 3);
}

// ---- v1 encoder ----

#[test]
fn v1_roundtrip_8x6() {
    let width = 8u32;
    let height = 6u32;
    let mut rgba = vec![0u8; (width * height * 4) as usize];
    for y in 0..height as usize {
        for x in 0..width as usize {
            let off = (y * width as usize + x) * 4;
            rgba[off] = (x * 30) as u8;
            rgba[off + 1] = (y * 40) as u8;
            rgba[off + 2] = 0x55;
            rgba[off + 3] = 0xFF;
        }
    }
    let enc = encode_pict_v1(width, height, &rgba).unwrap();
    // Verify it is NOT a v2 stream (no 512-byte stub, no 0x0011 0x02FF).
    assert!(enc.len() < 512, "v1 must not have 512-byte stub");
    // v1 sentinel at offset 10.
    assert_eq!(enc[10], 0x11, "v1 sentinel byte 0");
    assert_eq!(enc[11], 0x01, "v1 sentinel byte 1");
    // The decode must produce the right image.
    let img = parse_pict(&enc).unwrap();
    assert_eq!(img.width, width);
    assert_eq!(img.height, height);
    for y in 0..height as usize {
        for x in 0..width as usize {
            let off = (y * width as usize + x) * 4;
            assert_eq!(img.data[off], rgba[off], "R ({x},{y})");
            assert_eq!(img.data[off + 1], rgba[off + 1], "G ({x},{y})");
            assert_eq!(img.data[off + 2], rgba[off + 2], "B ({x},{y})");
            assert_eq!(img.data[off + 3], 0xFF, "A ({x},{y})");
        }
    }
}

// ---- ClipRgn injection ----

#[test]
fn clip_rgn_inserted_at_correct_offset() {
    let width = 8u32;
    let height = 8u32;
    let rgba = vec![0u8; (width * height * 4) as usize];
    let enc = encode_pict_v2_with_clip(width, height, &rgba, PackType::Raw, [1, 2, 7, 6]).unwrap();
    // After the 552-byte preamble, the ClipRgn opcode must start.
    let pos = 552usize;
    assert_eq!(&enc[pos..pos + 2], &[0x00, 0x01], "ClipRgn opcode");
    assert_eq!(&enc[pos + 2..pos + 4], &[0x00, 0x0A], "rgnSize=10");
    // bbox: top=1, left=2, bottom=7, right=6.
    assert_eq!(i16::from_be_bytes([enc[pos + 4], enc[pos + 5]]), 1, "top");
    assert_eq!(i16::from_be_bytes([enc[pos + 6], enc[pos + 7]]), 2, "left");
    assert_eq!(
        i16::from_be_bytes([enc[pos + 8], enc[pos + 9]]),
        7,
        "bottom"
    );
    assert_eq!(
        i16::from_be_bytes([enc[pos + 10], enc[pos + 11]]),
        6,
        "right"
    );
}

#[test]
fn clip_rgn_decode_succeeds() {
    // The decoder must not error on a ClipRgn opcode (it parses + drops
    // it today; honoring it is deferred).
    let width = 4u32;
    let height = 4u32;
    let rgba: Vec<u8> = (0..(width * height * 4) as usize)
        .map(|i| (i * 13) as u8)
        .collect();
    let enc = encode_pict_v2_with_clip(width, height, &rgba, PackType::Raw, [0, 0, 4, 4]).unwrap();
    let img = parse_pict(&enc).unwrap();
    assert_eq!(img.width, width);
    assert_eq!(img.height, height);
}
