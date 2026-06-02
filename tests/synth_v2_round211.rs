//! Round 211 — indexed-PixMap encoder (`BitsRect 0x0090`,
//! `PackBitsRect 0x0098`, `BitsRgn 0x0091`, `PackBitsRgn 0x0099` with
//! the `rowBytes` high bit set per Inside Macintosh §A-3 footnote `§`).
//!
//! Pairs with the round-186 indexed-PixMap decoder. All four opcodes
//! and all four indexed pixelSize values (1 / 2 / 4 / 8 bpp) round-trip
//! to the same RGBA output, with the embedded ColorTable resolving each
//! pixel index to its palette colour.

use oxideav_pict::{
    encode_pict_indexed_bits_rect, encode_pict_indexed_bits_rgn,
    encode_pict_indexed_pack_bits_rect, encode_pict_indexed_pack_bits_rgn, parse_pict, probe_pict,
    IndexedPixelSize, PictError, PictPixelFormat, PictProbe, ProbeTermination, ProbeVersion,
};

const RED: [u8; 4] = [0xFF, 0x00, 0x00, 0xFF];
const GRN: [u8; 4] = [0x00, 0xFF, 0x00, 0xFF];
const BLU: [u8; 4] = [0x00, 0x00, 0xFF, 0xFF];
const YEL: [u8; 4] = [0xFF, 0xFF, 0x00, 0xFF];
const BLK: [u8; 4] = [0x00, 0x00, 0x00, 0xFF];
const WHT: [u8; 4] = [0xFF, 0xFF, 0xFF, 0xFF];

fn assert_pixel(img_data: &[u8], width: u32, x: usize, y: usize, expected: [u8; 4], note: &str) {
    let off = (y * width as usize + x) * 4;
    assert_eq!(
        &img_data[off..off + 4],
        &expected[..],
        "{note} pixel ({x},{y}) — expected {expected:02X?}, got {:02X?}",
        &img_data[off..off + 4]
    );
}

// ---------------------------------------------------------------------------
// 8-bpp BitsRect / PackBitsRect: smallest case fitting all four colours.
// ---------------------------------------------------------------------------

#[test]
fn eight_bpp_bits_rect_4x4_roundtrip() {
    // 4×4 indexed image: each column carries a different palette index.
    //   col 0 → red, col 1 → green, col 2 → blue, col 3 → yellow.
    let palette = vec![RED, GRN, BLU, YEL];
    let indices: Vec<u8> = (0..16).map(|i| (i % 4) as u8).collect();
    let pict = encode_pict_indexed_bits_rect(4, 4, &indices, &palette, IndexedPixelSize::EightBpp)
        .expect("encode");
    let img = parse_pict(&pict).expect("decode");
    assert_eq!(img.width, 4);
    assert_eq!(img.height, 4);
    assert_eq!(img.pixel_format, PictPixelFormat::Rgba);
    for y in 0..4 {
        assert_pixel(&img.data, 4, 0, y, RED, "BitsRect 8bpp");
        assert_pixel(&img.data, 4, 1, y, GRN, "BitsRect 8bpp");
        assert_pixel(&img.data, 4, 2, y, BLU, "BitsRect 8bpp");
        assert_pixel(&img.data, 4, 3, y, YEL, "BitsRect 8bpp");
    }
}

#[test]
fn eight_bpp_pack_bits_rect_8x8_roundtrip() {
    // 8×8 image — rowBytes = 8 exactly, the smallest size at which the
    // PackBitsRect arm takes the per-row PackBits path (§A-3 carves out
    // rowBytes < 8 as raw).
    let palette = vec![RED, GRN];
    let mut indices = vec![0u8; 64];
    for y in 0..8 {
        for x in 0..8 {
            indices[y * 8 + x] = if (x + y) % 2 == 0 { 0 } else { 1 };
        }
    }
    let pict =
        encode_pict_indexed_pack_bits_rect(8, 8, &indices, &palette, IndexedPixelSize::EightBpp)
            .expect("encode");
    let img = parse_pict(&pict).expect("decode");
    assert_eq!(img.width, 8);
    for y in 0..8 {
        for x in 0..8 {
            let expected = if (x + y) % 2 == 0 { RED } else { GRN };
            assert_pixel(&img.data, 8, x, y, expected, "PackBitsRect 8bpp");
        }
    }
}

#[test]
fn eight_bpp_pack_bits_rect_4x4_falls_back_to_raw() {
    // rowBytes = 4 < 8 → carve-out → raw rows even on PackBitsRect.
    let palette = vec![BLK, WHT];
    let indices: Vec<u8> = (0..16).map(|i| (i & 1) as u8).collect();
    let pict =
        encode_pict_indexed_pack_bits_rect(4, 4, &indices, &palette, IndexedPixelSize::EightBpp)
            .expect("encode");
    let img = parse_pict(&pict).expect("decode");
    assert_eq!(img.width, 4);
    for y in 0..4 {
        for x in 0..4 {
            let expected = if (y * 4 + x) & 1 == 0 { BLK } else { WHT };
            assert_pixel(&img.data, 4, x, y, expected, "PackBitsRect 4×4 carve-out");
        }
    }
}

// ---------------------------------------------------------------------------
// 1 / 2 / 4 bpp BitsRect — exercise every supported indexed pixel size.
// ---------------------------------------------------------------------------

#[test]
fn one_bpp_bits_rect_8x4_roundtrip() {
    // 8×4 image, 1 bpp → rowBytes = 1. Vertical stripe: col 0/1 = black,
    // col 2/3 = white, …, alternating.
    let palette = vec![BLK, WHT];
    let mut indices = vec![0u8; 32];
    for y in 0..4 {
        for x in 0..8 {
            indices[y * 8 + x] = ((x / 2) & 1) as u8;
        }
    }
    let pict = encode_pict_indexed_bits_rect(8, 4, &indices, &palette, IndexedPixelSize::OneBpp)
        .expect("encode");
    let img = parse_pict(&pict).expect("decode");
    assert_eq!(img.width, 8);
    for y in 0..4 {
        for x in 0..8 {
            let expected = if ((x / 2) & 1) == 0 { BLK } else { WHT };
            assert_pixel(&img.data, 8, x, y, expected, "BitsRect 1bpp");
        }
    }
}

#[test]
fn two_bpp_bits_rect_4x4_roundtrip() {
    // 4×4, 2 bpp → 4 entries × distinct quadrants. Top-left red, top-right
    // green, bottom-left blue, bottom-right yellow.
    let palette = vec![RED, GRN, BLU, YEL];
    let mut indices = vec![0u8; 16];
    for y in 0..4 {
        for x in 0..4 {
            let q = match (x < 2, y < 2) {
                (true, true) => 0,
                (false, true) => 1,
                (true, false) => 2,
                (false, false) => 3,
            };
            indices[y * 4 + x] = q;
        }
    }
    let pict = encode_pict_indexed_bits_rect(4, 4, &indices, &palette, IndexedPixelSize::TwoBpp)
        .expect("encode");
    let img = parse_pict(&pict).expect("decode");
    assert_pixel(&img.data, 4, 0, 0, RED, "2bpp TL");
    assert_pixel(&img.data, 4, 3, 0, GRN, "2bpp TR");
    assert_pixel(&img.data, 4, 0, 3, BLU, "2bpp BL");
    assert_pixel(&img.data, 4, 3, 3, YEL, "2bpp BR");
}

#[test]
fn four_bpp_bits_rect_8x2_roundtrip() {
    // 8×2 at 4 bpp → rowBytes = 4. Use 8 palette entries (within 16 cap).
    let palette = vec![
        RED,
        GRN,
        BLU,
        YEL,
        BLK,
        WHT,
        [0x80, 0, 0, 0xFF],
        [0, 0x80, 0, 0xFF],
    ];
    let indices: Vec<u8> = (0..16).map(|i| (i % 8) as u8).collect();
    let pict = encode_pict_indexed_bits_rect(8, 2, &indices, &palette, IndexedPixelSize::FourBpp)
        .expect("encode");
    let img = parse_pict(&pict).expect("decode");
    assert_pixel(&img.data, 8, 0, 0, RED, "4bpp col0");
    assert_pixel(&img.data, 8, 4, 0, BLK, "4bpp col4");
    assert_pixel(&img.data, 8, 7, 0, [0, 0x80, 0, 0xFF], "4bpp col7");
}

// ---------------------------------------------------------------------------
// PackBitsRect at wider rowBytes — confirms the byteCount prefix logic.
// ---------------------------------------------------------------------------

#[test]
fn eight_bpp_pack_bits_rect_16x4_packbits_path() {
    // rowBytes = 16 (>= 8 carve-out threshold, <= 250 → 1-byte byteCount).
    let palette = vec![RED, GRN, BLU, BLK];
    let mut indices = vec![0u8; 64];
    for y in 0..4 {
        for x in 0..16 {
            indices[y * 16 + x] = ((x / 4) % 4) as u8;
        }
    }
    let pict =
        encode_pict_indexed_pack_bits_rect(16, 4, &indices, &palette, IndexedPixelSize::EightBpp)
            .expect("encode");
    let img = parse_pict(&pict).expect("decode");
    assert_eq!(img.width, 16);
    for y in 0..4 {
        for x in 0..16 {
            let expected = match (x / 4) % 4 {
                0 => RED,
                1 => GRN,
                2 => BLU,
                _ => BLK,
            };
            assert_pixel(&img.data, 16, x, y, expected, "PackBitsRect 16×4");
        }
    }
}

// ---------------------------------------------------------------------------
// BitsRgn / PackBitsRgn — region-clipped indexed PixMap.
// ---------------------------------------------------------------------------

#[test]
fn eight_bpp_bits_rgn_4x4_roundtrip() {
    // Full-frame clip — every pixel survives the mask.
    let palette = vec![RED, GRN];
    let indices: Vec<u8> = (0..16).map(|i| ((i / 4) & 1) as u8).collect();
    let pict = encode_pict_indexed_bits_rgn(
        4,
        4,
        &indices,
        &palette,
        IndexedPixelSize::EightBpp,
        [0, 0, 4, 4],
    )
    .expect("encode");
    let img = parse_pict(&pict).expect("decode");
    for y in 0..4 {
        let expected = if (y & 1) == 0 { RED } else { GRN };
        for x in 0..4 {
            assert_pixel(&img.data, 4, x, y, expected, "BitsRgn full clip");
        }
    }
}

#[test]
fn eight_bpp_pack_bits_rgn_8x8_with_clip() {
    // Clip 2..6 × 2..6 — outside-clip pixels stay paper-white.
    let palette = vec![BLU, RED];
    let indices = vec![1u8; 64]; // all red
    let pict = encode_pict_indexed_pack_bits_rgn(
        8,
        8,
        &indices,
        &palette,
        IndexedPixelSize::EightBpp,
        [2, 2, 6, 6],
    )
    .expect("encode");
    let img = parse_pict(&pict).expect("decode");
    // Inside the clip box: red.
    assert_pixel(&img.data, 8, 3, 3, RED, "inside clip");
    assert_pixel(&img.data, 8, 5, 5, RED, "inside clip");
    // Outside the clip box: paper white (the canvas default).
    assert_pixel(&img.data, 8, 0, 0, WHT, "outside clip TL");
    assert_pixel(&img.data, 8, 7, 7, WHT, "outside clip BR");
}

// ---------------------------------------------------------------------------
// Probe — confirm the indexed-PixMap rasters are surfaced in
// `indexed_raster_count` and not the `raster_count` direct counter.
// ---------------------------------------------------------------------------

#[test]
fn probe_surfaces_indexed_raster_count() {
    let palette = vec![RED, GRN];
    let indices = vec![0u8; 32];
    let pict =
        encode_pict_indexed_pack_bits_rect(8, 4, &indices, &palette, IndexedPixelSize::EightBpp)
            .expect("encode");
    let p: PictProbe = probe_pict(&pict).expect("probe");
    assert_eq!(p.version, ProbeVersion::V2);
    assert_eq!(p.width, 8);
    assert_eq!(p.height, 4);
    assert_eq!(p.termination, ProbeTermination::EndPic);
    // indexed_raster_count is a sub-count of raster_count: every
    // raster increments raster_count; the indexed-PixMap variants
    // additionally bump indexed_raster_count.
    assert_eq!(p.indexed_raster_count, 1);
    assert_eq!(p.raster_count, 1);
}

// ---------------------------------------------------------------------------
// Validation — every error path is exercised.
// ---------------------------------------------------------------------------

#[test]
fn rejects_zero_dim() {
    let err =
        encode_pict_indexed_bits_rect(0, 4, &[], &[RED], IndexedPixelSize::EightBpp).unwrap_err();
    assert!(matches!(err, PictError::InvalidData(_)));
}

#[test]
fn rejects_size_mismatch() {
    let err = encode_pict_indexed_bits_rect(
        4,
        4,
        &[0u8; 8], // expected 16
        &[RED, GRN],
        IndexedPixelSize::EightBpp,
    )
    .unwrap_err();
    assert!(matches!(err, PictError::InvalidData(_)));
}

#[test]
fn rejects_empty_palette() {
    let err = encode_pict_indexed_bits_rect(2, 2, &[0u8; 4], &[], IndexedPixelSize::EightBpp)
        .unwrap_err();
    assert!(matches!(err, PictError::InvalidData(_)));
}

#[test]
fn rejects_palette_overflow_for_pixel_size() {
    // 1 bpp caps at 2 entries; 3-entry palette must be rejected.
    let err =
        encode_pict_indexed_bits_rect(2, 2, &[0u8; 4], &[RED, GRN, BLU], IndexedPixelSize::OneBpp)
            .unwrap_err();
    assert!(matches!(err, PictError::InvalidData(_)));

    // 2 bpp caps at 4; 5 entries must reject.
    let err2 = encode_pict_indexed_bits_rect(
        2,
        2,
        &[0u8; 4],
        &[RED, GRN, BLU, BLK, WHT],
        IndexedPixelSize::TwoBpp,
    )
    .unwrap_err();
    assert!(matches!(err2, PictError::InvalidData(_)));

    // 4 bpp caps at 16; 17 entries must reject.
    let big: Vec<[u8; 4]> = (0..17).map(|i| [i as u8, 0, 0, 0xFF]).collect();
    let err4 = encode_pict_indexed_bits_rect(2, 2, &[0u8; 4], &big, IndexedPixelSize::FourBpp)
        .unwrap_err();
    assert!(matches!(err4, PictError::InvalidData(_)));
}

#[test]
fn eight_bpp_accepts_full_256_entry_palette() {
    let palette: Vec<[u8; 4]> = (0..256).map(|i| [i as u8, 0, 0, 0xFF]).collect();
    let indices: Vec<u8> = (0..16).map(|i| (i * 17) as u8).collect();
    let pict = encode_pict_indexed_bits_rect(4, 4, &indices, &palette, IndexedPixelSize::EightBpp)
        .expect("encode");
    let img = parse_pict(&pict).expect("decode");
    assert_eq!(img.width, 4);
    // First pixel index = 0 → palette[0] = [0,0,0,0xFF]; last pixel
    // index = 15 * 17 = 255 → palette[255] = [255,0,0,0xFF].
    assert_pixel(&img.data, 4, 0, 0, [0, 0, 0, 0xFF], "first");
    assert_pixel(&img.data, 4, 3, 3, [0xFF, 0, 0, 0xFF], "last");
}
