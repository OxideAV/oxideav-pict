//! Round 5 encoder tests.
//!
//! Three new encoder paths land this round:
//!
//! 1. [`encode_pict_v1_with`] — v1 PICT emit with a [`PackType`]
//!    selector (packType 1 / 2 / 3 / 4), bringing v1 emit to parity
//!    with v2.
//! 2. [`encode_pict_bits_rect`] / [`encode_pict_pack_bits_rect`] —
//!    1-bpp BitMap encoders emitting `BitsRect` (`0x0090`) and
//!    `PackBitsRect` (`0x0098`) opcodes for monochrome images.
//! 3. [`PictBuilder::raster`] — append a DirectBitsRect raster chunk
//!    to a builder so callers can mix drawing primitives + raster in
//!    the same v2 stream.
//!
//! Every test self-roundtrips through [`parse_pict`].

use oxideav_pict::ops::{PictBuilder, Verb};
use oxideav_pict::{
    encode_pict_bits_rect, encode_pict_pack_bits_rect, encode_pict_v1_with, parse_pict, PackType,
};

// ---------------------------------------------------------------------------
// v1 with PackType selector — round-trip per pack mode.
// ---------------------------------------------------------------------------

#[test]
fn v1_packtype_raw_roundtrip() {
    let width = 4u32;
    let height = 4u32;
    let rgba: Vec<u8> = (0..(width * height * 4)).map(|i| (i * 23) as u8).collect();
    let enc = encode_pict_v1_with(width, height, &rgba, PackType::Raw).unwrap();
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

#[test]
fn v1_packtype_packed24_roundtrip() {
    let width = 4u32;
    let height = 4u32;
    let rgba: Vec<u8> = (0..(width * height * 4)).map(|i| (i * 41) as u8).collect();
    let enc = encode_pict_v1_with(width, height, &rgba, PackType::Packed24).unwrap();
    let img = parse_pict(&enc).unwrap();
    assert_eq!(img.width, width);
    assert_eq!(img.height, height);
    for y in 0..height as usize {
        for x in 0..width as usize {
            let off = (y * width as usize + x) * 4;
            assert_eq!(img.data[off], rgba[off], "R ({x},{y})");
            assert_eq!(img.data[off + 1], rgba[off + 1], "G ({x},{y})");
            assert_eq!(img.data[off + 2], rgba[off + 2], "B ({x},{y})");
        }
    }
}

#[test]
fn v1_packtype_rle16_roundtrip() {
    // 5-bit-per-channel quantisation: encode A1R5G5B5 then decode.
    // Test with values that divide cleanly by 8 so 5-bit replication
    // round-trips bit-exactly.
    let width = 8u32;
    let height = 4u32;
    let mut rgba = vec![0u8; (width * height * 4) as usize];
    for y in 0..height as usize {
        for x in 0..width as usize {
            let off = (y * width as usize + x) * 4;
            rgba[off] = ((x as u8) << 5) | 0x1F; // 0x1F, 0x3F, ..., 0xFF
            rgba[off + 1] = ((y as u8) << 5) | 0x1F;
            rgba[off + 2] = 0xFF;
            rgba[off + 3] = 0xFF;
        }
    }
    let enc = encode_pict_v1_with(width, height, &rgba, PackType::Rle16).unwrap();
    let img = parse_pict(&enc).unwrap();
    assert_eq!(img.width, width);
    assert_eq!(img.height, height);
    // 5-bit replicate: top 5 bits preserved, low 3 bits = top 3 of
    // top 5. With low 5 bits = 0x1F, expansion produces 0xXX | (0xXX
    // >> 5), which is the original value modulo replication.
    for y in 0..height as usize {
        for x in 0..width as usize {
            let off = (y * width as usize + x) * 4;
            // After 5-bit quantisation the encoder produces (px[0] >> 3)
            // << 3 | (px[0] >> 3) >> 2 — round-trip preserves only the
            // top 5 bits.  Check the channel value is at most 7 off
            // from the input (one quantum).
            let diff_r = (img.data[off] as i32 - rgba[off] as i32).abs();
            let diff_g = (img.data[off + 1] as i32 - rgba[off + 1] as i32).abs();
            let diff_b = (img.data[off + 2] as i32 - rgba[off + 2] as i32).abs();
            assert!(
                diff_r <= 7,
                "R ({x},{y}): {} vs {}",
                img.data[off],
                rgba[off]
            );
            assert!(diff_g <= 7, "G ({x},{y})");
            assert!(diff_b <= 7, "B ({x},{y})");
        }
    }
}

#[test]
fn v1_packtype_component_packbits_roundtrip() {
    let width = 8u32;
    let height = 4u32;
    let mut rgba = vec![0u8; (width * height * 4) as usize];
    // Solid image — packType 4 should still round-trip exactly.
    for y in 0..height as usize {
        for x in 0..width as usize {
            let off = (y * width as usize + x) * 4;
            rgba[off] = 0x77;
            rgba[off + 1] = 0x88;
            rgba[off + 2] = 0x99;
            rgba[off + 3] = 0xFF;
        }
    }
    let enc = encode_pict_v1_with(width, height, &rgba, PackType::ComponentPackBits).unwrap();
    let img = parse_pict(&enc).unwrap();
    for y in 0..height as usize {
        for x in 0..width as usize {
            let off = (y * width as usize + x) * 4;
            assert_eq!(img.data[off], 0x77);
            assert_eq!(img.data[off + 1], 0x88);
            assert_eq!(img.data[off + 2], 0x99);
            assert_eq!(img.data[off + 3], 0xFF);
        }
    }
}

#[test]
fn v1_packtype3_smaller_than_raw_for_solid() {
    // A 32×32 solid image: packType 3 (16-bit + RLE) should be
    // dramatically smaller than packType 1 (32-bit raw).
    let width = 32u32;
    let height = 32u32;
    let rgba = vec![0xAAu8; (width * height * 4) as usize];
    let enc1 = encode_pict_v1_with(width, height, &rgba, PackType::Raw).unwrap();
    let enc3 = encode_pict_v1_with(width, height, &rgba, PackType::Rle16).unwrap();
    assert!(
        enc3.len() < enc1.len() / 2,
        "v1 packType 3 should at least halve solid image: raw={}, rle16={}",
        enc1.len(),
        enc3.len()
    );
}

#[test]
fn v1_no_512_stub_regardless_of_packtype() {
    let rgba = vec![0u8; 4 * 4 * 4];
    for pack in [
        PackType::Raw,
        PackType::Packed24,
        PackType::Rle16,
        PackType::ComponentPackBits,
    ] {
        let enc = encode_pict_v1_with(4, 4, &rgba, pack).unwrap();
        // First 10 bytes are picSize + picFrame; bytes 10/11 are 0x11
        // 0x01 (v1 sentinel).
        assert_eq!(enc[10], 0x11, "v1 sentinel high byte ({pack:?})");
        assert_eq!(enc[11], 0x01, "v1 sentinel low byte ({pack:?})");
        assert!(enc.len() < 512, "v1 should not include stub ({pack:?})");
    }
}

// ---------------------------------------------------------------------------
// 1-bpp BitMap encoders.
// ---------------------------------------------------------------------------

#[test]
fn bits_rect_all_white_roundtrip() {
    // 8×8 all-white image. 1-bpp threshold maps Y >= 128 → bit 0 →
    // white. Decoder turns bit 0 → 0xFF white pixel.
    let width = 8u32;
    let height = 8u32;
    let rgba = vec![0xFFu8; (width * height * 4) as usize];
    let enc = encode_pict_bits_rect(width, height, &rgba).unwrap();
    let img = parse_pict(&enc).unwrap();
    assert_eq!(img.width, width);
    assert_eq!(img.height, height);
    for y in 0..height as usize {
        for x in 0..width as usize {
            let off = (y * width as usize + x) * 4;
            assert_eq!(img.data[off], 0xFF, "white R ({x},{y})");
            assert_eq!(img.data[off + 1], 0xFF, "white G ({x},{y})");
            assert_eq!(img.data[off + 2], 0xFF, "white B ({x},{y})");
        }
    }
}

#[test]
fn bits_rect_all_black_roundtrip() {
    let width = 8u32;
    let height = 8u32;
    let mut rgba = vec![0u8; (width * height * 4) as usize];
    for px in rgba.chunks_exact_mut(4) {
        px[3] = 0xFF;
    }
    let enc = encode_pict_bits_rect(width, height, &rgba).unwrap();
    let img = parse_pict(&enc).unwrap();
    for y in 0..height as usize {
        for x in 0..width as usize {
            let off = (y * width as usize + x) * 4;
            assert_eq!(img.data[off], 0x00, "black R ({x},{y})");
            assert_eq!(img.data[off + 1], 0x00, "black G ({x},{y})");
            assert_eq!(img.data[off + 2], 0x00, "black B ({x},{y})");
        }
    }
}

#[test]
fn bits_rect_checkerboard_roundtrip() {
    // 8×8 checkerboard. Even (x+y) → black, odd → white. Tests the
    // bit-packing direction (MSB-first within each byte).
    let width = 8u32;
    let height = 8u32;
    let mut rgba = vec![0u8; (width * height * 4) as usize];
    for y in 0..height as usize {
        for x in 0..width as usize {
            let off = (y * width as usize + x) * 4;
            let v = if (x + y) % 2 == 0 { 0x00 } else { 0xFF };
            rgba[off] = v;
            rgba[off + 1] = v;
            rgba[off + 2] = v;
            rgba[off + 3] = 0xFF;
        }
    }
    let enc = encode_pict_bits_rect(width, height, &rgba).unwrap();
    let img = parse_pict(&enc).unwrap();
    for y in 0..height as usize {
        for x in 0..width as usize {
            let off = (y * width as usize + x) * 4;
            let expected = if (x + y) % 2 == 0 { 0x00 } else { 0xFF };
            assert_eq!(img.data[off], expected, "({x},{y})");
        }
    }
}

#[test]
fn pack_bits_rect_wide_image_compresses() {
    // Wide solid image: rowBytes = 64 / 8 = 8 (≥ 8, so PackBitsRect
    // takes the RLE path). PackBits should be much smaller than the
    // raw BitsRect output.
    let width = 64u32;
    let height = 16u32;
    let rgba = vec![0xFFu8; (width * height * 4) as usize]; // all white
    let raw = encode_pict_bits_rect(width, height, &rgba).unwrap();
    let packed = encode_pict_pack_bits_rect(width, height, &rgba).unwrap();
    assert!(
        packed.len() < raw.len(),
        "PackBitsRect ({}) should compress vs BitsRect ({}) for solid image",
        packed.len(),
        raw.len()
    );
    // Round-trip must still be lossless.
    let img = parse_pict(&packed).unwrap();
    assert_eq!(img.width, width);
    assert_eq!(img.height, height);
    for y in 0..height as usize {
        for x in 0..width as usize {
            let off = (y * width as usize + x) * 4;
            assert_eq!(img.data[off], 0xFF, "white R ({x},{y})");
        }
    }
}

#[test]
fn pack_bits_rect_narrow_image_falls_through_to_raw() {
    // rowBytes = ceil(7 / 8) = 1, which is < 8: per Inside Macintosh
    // §A-3 the PackBitsRect special-cases tiny rowBytes to raw rows.
    // Round-trip must succeed.
    let width = 7u32;
    let height = 7u32;
    let rgba = vec![0xFFu8; (width * height * 4) as usize];
    let enc = encode_pict_pack_bits_rect(width, height, &rgba).unwrap();
    let img = parse_pict(&enc).unwrap();
    assert_eq!(img.width, width);
    assert_eq!(img.height, height);
}

#[test]
fn bits_rect_luma_threshold() {
    // Mixed luminance pixels: a pure red pixel (Y ≈ 76) below 128 →
    // black; a pure green pixel (Y ≈ 150) above 128 → white.
    let width = 4u32;
    let height = 1u32;
    let rgba = vec![
        0xFF, 0x00, 0x00, 0xFF, // pure red    → Y ≈ 76  → bit=1 → black
        0x00, 0xFF, 0x00, 0xFF, // pure green  → Y ≈ 150 → bit=0 → white
        0x00, 0x00, 0xFF, 0xFF, // pure blue   → Y ≈ 29  → bit=1 → black
        0xC0, 0xC0, 0xC0, 0xFF, // light grey  → Y ≈ 192 → bit=0 → white
    ];
    let enc = encode_pict_bits_rect(width, height, &rgba).unwrap();
    let img = parse_pict(&enc).unwrap();
    assert_eq!(img.data[0], 0x00, "red → black"); // x=0
    assert_eq!(img.data[4], 0xFF, "green → white"); // x=1
    assert_eq!(img.data[8], 0x00, "blue → black"); // x=2
    assert_eq!(img.data[12], 0xFF, "grey → white"); // x=3
}

// ---------------------------------------------------------------------------
// PictBuilder::raster — drawing + raster combined.
// ---------------------------------------------------------------------------

#[test]
fn builder_raster_alone_decodes() {
    // 4×4 solid red raster pushed through the builder.
    let width = 4u32;
    let height = 4u32;
    let mut rgba = vec![0u8; (width * height * 4) as usize];
    for px in rgba.chunks_exact_mut(4) {
        px[0] = 0xCC;
        px[3] = 0xFF;
    }
    let mut b = PictBuilder::new(0, 0, 4, 4);
    b.raster(0, 0, 4, 4, &rgba, PackType::Raw).unwrap();
    let bytes = b.finish();
    let img = parse_pict(&bytes).expect("decode failed");
    assert_eq!(img.width, 4);
    assert_eq!(img.height, 4);
    for y in 0..height as usize {
        for x in 0..width as usize {
            let off = (y * width as usize + x) * 4;
            assert_eq!(img.data[off], 0xCC, "R ({x},{y})");
            assert_eq!(img.data[off + 1], 0x00, "G ({x},{y})");
        }
    }
}

#[test]
fn builder_drawing_under_raster() {
    // A green-painted rectangle then a smaller raster on top should
    // yield: green where the raster doesn't overwrite, the raster's
    // own colour where it does.
    let mut b = PictBuilder::new(0, 0, 16, 16);
    b.fg_color(0x00, 0xFF, 0x00);
    b.rect(Verb::Paint, 0, 0, 16, 16);
    // 4×4 yellow raster at (4,4)..(8,8).
    let mut raster_rgba = vec![0u8; 4 * 4 * 4];
    for px in raster_rgba.chunks_exact_mut(4) {
        px[0] = 0xFF;
        px[1] = 0xFF;
        px[2] = 0x00;
        px[3] = 0xFF;
    }
    b.raster(4, 4, 8, 8, &raster_rgba, PackType::Raw).unwrap();
    let bytes = b.finish();
    let img = parse_pict(&bytes).expect("decode failed");
    // Inside the raster window: yellow.
    let off = (5 * 16 + 5) * 4;
    assert_eq!(img.data[off], 0xFF, "yellow R inside raster");
    assert_eq!(img.data[off + 1], 0xFF, "yellow G inside raster");
    assert_eq!(img.data[off + 2], 0x00, "yellow B inside raster");
    // Outside the raster window but inside the green paint: green.
    let off = (16 + 1) * 4; // (y=1, x=1)
    assert_eq!(img.data[off], 0x00, "green R outside raster");
    assert_eq!(img.data[off + 1], 0xFF, "green G outside raster");
    assert_eq!(img.data[off + 2], 0x00, "green B outside raster");
}

#[test]
fn builder_raster_then_drawing_overlays() {
    // Drawing AFTER the raster overlays it.
    let mut b = PictBuilder::new(0, 0, 16, 16);
    let mut raster_rgba = vec![0u8; 16 * 16 * 4];
    for px in raster_rgba.chunks_exact_mut(4) {
        px[2] = 0xFF; // pure blue
        px[3] = 0xFF;
    }
    b.raster(0, 0, 16, 16, &raster_rgba, PackType::Raw).unwrap();
    b.fg_color(0xFF, 0x00, 0x00);
    b.rect(Verb::Paint, 4, 4, 12, 12); // red paint over the centre
    let bytes = b.finish();
    let img = parse_pict(&bytes).expect("decode failed");
    // Centre pixel: red (overlay wins).
    let off = (8 * 16 + 8) * 4;
    assert_eq!(img.data[off], 0xFF, "centre overlay R");
    assert_eq!(img.data[off + 1], 0x00, "centre overlay G");
    assert_eq!(img.data[off + 2], 0x00, "centre overlay B");
    // Edge pixel: blue (raster shows through outside the overlay).
    let off = 0; // (y=0, x=0) flat-index 0
    assert_eq!(img.data[off], 0x00, "edge raster R");
    assert_eq!(img.data[off + 2], 0xFF, "edge raster B");
}

#[test]
fn builder_raster_rejects_degenerate_rect() {
    let mut b = PictBuilder::new(0, 0, 4, 4);
    match b.raster(0, 0, 0, 0, &[], PackType::Raw) {
        Err(e) => assert!(format!("{e:?}").contains("degenerate")),
        Ok(_) => panic!("degenerate rect should have errored"),
    }
}

#[test]
fn builder_raster_rejects_data_size_mismatch() {
    let mut b = PictBuilder::new(0, 0, 4, 4);
    match b.raster(0, 0, 4, 4, &[0u8; 5], PackType::Raw) {
        Err(e) => assert!(format!("{e:?}").contains("data.len()")),
        Ok(_) => panic!("data.len() mismatch should have errored"),
    }
}

#[test]
fn builder_drawing_and_packtype3_raster() {
    // Round 5: PictBuilder::raster also accepts compressed pack
    // types.
    let mut b = PictBuilder::new(0, 0, 8, 8);
    let mut raster_rgba = vec![0u8; 8 * 8 * 4];
    for px in raster_rgba.chunks_exact_mut(4) {
        px[0] = 0xFF;
        px[1] = 0xC0;
        px[2] = 0x00;
        px[3] = 0xFF;
    }
    b.raster(0, 0, 8, 8, &raster_rgba, PackType::Rle16).unwrap();
    let bytes = b.finish();
    let img = parse_pict(&bytes).expect("decode failed");
    // 5-bit quantisation: top 5 bits preserved.  The encoded
    // (0xFF, 0xC0, 0x00) becomes A1R5G5B5 = (1, 31, 24, 0) and
    // decodes to (~0xFF, ~0xC6, 0x00).  Allow ±8 quantisation slop.
    let off = (4 * 8 + 4) * 4;
    let diff_r = (img.data[off] as i32 - 0xFF).abs();
    let diff_g = (img.data[off + 1] as i32 - 0xC0).abs();
    let diff_b = img.data[off + 2] as i32; // expected 0
    let diff_b = diff_b.abs();
    assert!(
        diff_r <= 8 && diff_g <= 8 && diff_b <= 8,
        "5-bit quant slop: ({:#x}, {:#x}, {:#x})",
        img.data[off],
        img.data[off + 1],
        img.data[off + 2]
    );
}
