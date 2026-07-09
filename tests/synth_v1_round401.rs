//! Round 401 — Table-A-3-conformant v1 raster encoders + the
//! `pnLocHFrac` fresh-GrafPort default fix.
//!
//! `encode_pict_v1` emits a v2-style `DirectBitsRect $9A` inside a v1
//! framing — an extension §A-3 Table A-3 does not define (its raster
//! opcodes stop at `$90`/`$91`/`$98`/`$99`). Round 401 adds
//! `encode_pict_v1_bits_rect` / `encode_pict_v1_pack_bits_rect`: 1-bpp
//! BitMap rasters in strict Table-A-3 form, including footnote `‡`
//! (`$90` only when `rowBytes < 8`).

use oxideav_pict::{
    encode_pict_v1_bits_rect, encode_pict_v1_pack_bits_rect, parse_pict, probe_pict, PictTextState,
    ProbeVersion,
};

/// A black/white RGBA checker: pixel (x, y) black when (x + y) even.
fn checker_rgba(w: u32, h: u32) -> Vec<u8> {
    let mut v = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let ink = (x + y) % 2 == 0;
            let c = if ink { 0x00 } else { 0xFF };
            v.extend_from_slice(&[c, c, c, 0xFF]);
        }
    }
    v
}

#[test]
fn v1_bits_rect_round_trips_narrow_bitmap() {
    // 16 columns → rowBytes = 2 < 8: the footnote-‡-legal BitsRect.
    let rgba = checker_rgba(16, 8);
    let bytes = encode_pict_v1_bits_rect(16, 8, &rgba).unwrap();
    // v1 framing: no stub; picSize recorded; $11 $01 stanza.
    assert_eq!(&bytes[10..12], &[0x11, 0x01]);
    assert_eq!(
        u16::from_be_bytes([bytes[0], bytes[1]]) as usize,
        bytes.len()
    );
    let img = parse_pict(&bytes).unwrap();
    assert_eq!((img.width, img.height), (16, 8));
    // 1-bpp threshold reproduces the checker exactly.
    for (i, px) in img.data.chunks_exact(4).enumerate() {
        let (x, y) = (i % 16, i / 16);
        let want = if (x + y) % 2 == 0 { 0x00 } else { 0xFF };
        assert_eq!(px[0], want, "at ({x},{y})");
    }
    let p = probe_pict(&bytes).unwrap();
    assert_eq!(p.version, ProbeVersion::V1);
    assert_eq!(p.raster_count, 1);
}

#[test]
fn v1_bits_rect_rejects_wide_rows_per_footnote() {
    // 64 columns → rowBytes = 8: footnote ‡ says BitsRect is illegal.
    let rgba = checker_rgba(64, 4);
    assert!(encode_pict_v1_bits_rect(64, 4, &rgba).is_err());
    // The packed form handles it.
    let bytes = encode_pict_v1_pack_bits_rect(64, 4, &rgba).unwrap();
    let img = parse_pict(&bytes).unwrap();
    assert_eq!((img.width, img.height), (64, 4));
    for (i, px) in img.data.chunks_exact(4).enumerate() {
        let (x, y) = (i % 64, i / 64);
        let want = if (x + y) % 2 == 0 { 0x00 } else { 0xFF };
        assert_eq!(px[0], want, "at ({x},{y})");
    }
}

#[test]
fn v1_pack_bits_rect_narrow_rows_stay_raw() {
    // 24 columns → rowBytes = 3 < 8: §A-3 carve-out — PackBitsRect
    // rows ship raw with no byteCount prefix. Round-trip proves the
    // encoder/decoder agree on the carve-out inside v1 framing.
    let rgba = checker_rgba(24, 6);
    let bytes = encode_pict_v1_pack_bits_rect(24, 6, &rgba).unwrap();
    let img = parse_pict(&bytes).unwrap();
    assert_eq!((img.width, img.height), (24, 6));
    let p = probe_pict(&bytes).unwrap();
    assert_eq!(p.version, ProbeVersion::V1);
    assert_eq!(p.raster_count, 1);
}

// ---------------------------------------------------------------------------
// pnLocHFrac default: 0.5 = bit pattern 0x8000 (low word of a Fixed),
// per the §4 CGrafPort initial-field table (round 401 fix; the crate
// previously defaulted to 0x4000 = 0.25).
// ---------------------------------------------------------------------------

#[test]
fn pn_loc_h_frac_defaults_to_half() {
    let ts = PictTextState::fresh_graf_port();
    assert_eq!(ts.pn_loc_h_frac as u16, 0x8000);
}
