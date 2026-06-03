//! Round 217 — v2 / extended-v2 `HeaderOp` (`0x0C00`) 24-byte payload
//! is parsed into a structured [`PictHeader`] and surfaced on both
//! [`PictImage::header`] and [`PictProbe::header`].
//!
//! Inside Macintosh: Imaging With QuickDraw §A-3 ("Version and Header
//! Opcodes") + §A-22 Listings A-5 (extended v2) and A-6 (v2) define the
//! on-disk shape. The encoder side now emits a canonical Listing-A-5
//! header (`version=-2`, `hRes=vRes=72.0` dpi, `optimal_source_rect =
//! picFrame`) so a `parse_pict(encode_pict(…))` round-trip recovers the
//! header.

use oxideav_pict::header::{Fixed, PictHeader};
use oxideav_pict::ops::{PictBuilder, Verb};
use oxideav_pict::{
    encode_pict, encode_pict_bits_rect, encode_pict_bits_rgn, encode_pict_indexed_bits_rect,
    encode_pict_pack_bits_rect, encode_pict_pack_bits_rgn, encode_pict_v2,
    encode_pict_v2_with_clip, parse_pict, probe_pict, IndexedPixelSize, PackType,
};

// ---------------------------------------------------------------------------
// Direct-bytes parse (no decoder roundtrip): the Listing-A-5 and
// Listing-A-6 sample payloads must yield the §A-3 expected variants.
// ---------------------------------------------------------------------------

#[test]
fn extended_v2_listing_a5_bytes_parse_through_decoder() {
    // Build a 1×1 PICT whose v2 header is the Listing-A-5 payload
    // verbatim. The smallest legal raster we can wrap it around is the
    // round-2 packType-1 encoder output, with the header bytes patched
    // in over the encoder's emitted header.
    let rgba = vec![0xFFu8; 4];
    let mut pict = encode_pict(1, 1, &rgba).expect("encode_pict 1x1");

    // The header payload starts after stub(512) + picSize(2) +
    // picFrame(8) + versionOp(2) + version(2) + headerOp(2) = byte 528.
    let payload_off = 512 + 2 + 8 + 2 + 2 + 2;
    let listing_a5: [u8; 24] = [
        0xFF, 0xFE, 0x00, 0x00, 0x00, 0x48, 0x00, 0x00, 0x00, 0x48, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00,
    ];
    pict[payload_off..payload_off + 24].copy_from_slice(&listing_a5);

    let img = parse_pict(&pict).expect("decode");
    let header = img.header.expect("header parsed");
    match header {
        PictHeader::ExtendedV2 {
            hres,
            vres,
            optimal_source_rect,
        } => {
            assert_eq!(hres, Fixed::SEVENTY_TWO_DPI);
            assert_eq!(vres, Fixed::SEVENTY_TWO_DPI);
            assert_eq!(optimal_source_rect.top, 0);
            assert_eq!(optimal_source_rect.left, 0);
            assert_eq!(optimal_source_rect.bottom, 1);
            assert_eq!(optimal_source_rect.right, 1);
        }
        other => panic!("expected ExtendedV2, got {other:?}"),
    }
}

#[test]
fn v2_listing_a6_bytes_parse_through_decoder() {
    let rgba = vec![0xFFu8; 4];
    let mut pict = encode_pict(1, 1, &rgba).expect("encode_pict 1x1");
    let payload_off = 512 + 2 + 8 + 2 + 2 + 2;
    let listing_a6: [u8; 24] = [
        0xFF, 0xFF, 0xFF, 0xFF, // version = -1 (long)
        0x00, 0x00, 0x00, 0x00, // top  = 0.0
        0x00, 0x00, 0x00, 0x00, // left = 0.0
        0x00, 0x01, 0x00, 0x00, // bot  = 1.0
        0x00, 0x01, 0x00, 0x00, // rgt  = 1.0
        0x00, 0x00, 0x00, 0x00, // reserved
    ];
    pict[payload_off..payload_off + 24].copy_from_slice(&listing_a6);

    let img = parse_pict(&pict).expect("decode");
    let header = img.header.expect("header parsed");
    match header {
        PictHeader::V2 { fixed_bounds } => {
            assert_eq!(fixed_bounds[0].integer_part(), 0);
            assert_eq!(fixed_bounds[1].integer_part(), 0);
            assert_eq!(fixed_bounds[2].integer_part(), 1);
            assert_eq!(fixed_bounds[3].integer_part(), 1);
        }
        other => panic!("expected V2, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Encoder → decoder roundtrip: every v2 emitter must now produce a
// recognisable Listing-A-5 extended-v2 header.
// ---------------------------------------------------------------------------

#[test]
fn encode_pict_emits_extended_v2_header_72dpi() {
    let rgba = vec![0x80u8; 4 * 4 * 4];
    let pict = encode_pict(4, 4, &rgba).expect("encode");
    let img = parse_pict(&pict).expect("decode");
    let header = img.header.expect("header parsed");
    match header {
        PictHeader::ExtendedV2 {
            hres,
            vres,
            optimal_source_rect,
        } => {
            assert_eq!(hres.integer_part(), 72);
            assert_eq!(vres.integer_part(), 72);
            assert_eq!(optimal_source_rect.top, 0);
            assert_eq!(optimal_source_rect.left, 0);
            assert_eq!(optimal_source_rect.bottom, 4);
            assert_eq!(optimal_source_rect.right, 4);
        }
        other => panic!("expected ExtendedV2, got {other:?}"),
    }
}

#[test]
fn encode_pict_v2_with_clip_emits_extended_v2_header() {
    let rgba = vec![0u8; 4 * 4 * 4];
    let pict = encode_pict_v2_with_clip(4, 4, &rgba, PackType::Raw, [1, 1, 3, 3])
        .expect("encode_pict_v2_with_clip");
    let img = parse_pict(&pict).expect("decode");
    assert!(img.header.is_some());
    assert!(img.header.unwrap().is_extended());
}

#[test]
fn encode_pict_bits_rect_emits_extended_v2_header() {
    let pict = encode_pict_bits_rect(8, 8, &vec![0xFFu8; 8 * 8 * 4]).expect("bits_rect");
    let img = parse_pict(&pict).expect("decode");
    assert!(img.header.unwrap().is_extended());
}

#[test]
fn encode_pict_pack_bits_rect_emits_extended_v2_header() {
    let pict =
        encode_pict_pack_bits_rect(64, 16, &vec![0xFFu8; 64 * 16 * 4]).expect("pack_bits_rect");
    let img = parse_pict(&pict).expect("decode");
    assert!(img.header.unwrap().is_extended());
}

#[test]
fn encode_pict_bits_rgn_emits_extended_v2_header() {
    let pict =
        encode_pict_bits_rgn(8, 8, &vec![0xFFu8; 8 * 8 * 4], [0, 0, 8, 8]).expect("bits_rgn");
    let img = parse_pict(&pict).expect("decode");
    assert!(img.header.unwrap().is_extended());
}

#[test]
fn encode_pict_pack_bits_rgn_emits_extended_v2_header() {
    let pict = encode_pict_pack_bits_rgn(64, 16, &vec![0xFFu8; 64 * 16 * 4], [0, 0, 16, 64])
        .expect("pack_bits_rgn");
    let img = parse_pict(&pict).expect("decode");
    assert!(img.header.unwrap().is_extended());
}

#[test]
fn encode_pict_indexed_bits_rect_emits_extended_v2_header() {
    let palette = vec![[0xFF, 0, 0, 0xFF]];
    let pict =
        encode_pict_indexed_bits_rect(8, 8, &[0u8; 64], &palette, IndexedPixelSize::EightBpp)
            .expect("indexed");
    let img = parse_pict(&pict).expect("decode");
    assert!(img.header.unwrap().is_extended());
}

#[test]
fn encode_pict_v2_packtype_rle16_emits_extended_v2_header() {
    let rgba = vec![0x40u8; 4 * 4 * 4];
    let pict = encode_pict_v2(4, 4, &rgba, PackType::Rle16).expect("rle16");
    let img = parse_pict(&pict).expect("decode");
    assert!(img.header.unwrap().is_extended());
}

#[test]
fn encode_pict_v2_packtype_componentpackbits_emits_extended_v2_header() {
    let rgba = vec![0x60u8; 4 * 4 * 4];
    let pict = encode_pict_v2(4, 4, &rgba, PackType::ComponentPackBits).expect("comp");
    let img = parse_pict(&pict).expect("decode");
    assert!(img.header.unwrap().is_extended());
}

// ---------------------------------------------------------------------------
// PictBuilder also emits a Listing-A-5 header per its `new()` rewrite.
// ---------------------------------------------------------------------------

#[test]
fn pict_builder_emits_extended_v2_header() {
    let mut b = PictBuilder::new(0, 0, 16, 16);
    b.fg_color(0xFF, 0, 0);
    b.rect(Verb::Paint, 0, 0, 16, 16);
    let pict = b.finish();
    let img = parse_pict(&pict).expect("decode builder PICT");
    let header = img.header.expect("PictBuilder header");
    match header {
        PictHeader::ExtendedV2 {
            optimal_source_rect,
            ..
        } => {
            assert_eq!(optimal_source_rect.top, 0);
            assert_eq!(optimal_source_rect.left, 0);
            assert_eq!(optimal_source_rect.bottom, 16);
            assert_eq!(optimal_source_rect.right, 16);
        }
        other => panic!("expected ExtendedV2, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Probe surfaces the same header (no rasterisation needed).
// ---------------------------------------------------------------------------

#[test]
fn probe_surfaces_extended_v2_header() {
    let rgba = vec![0u8; 4 * 4 * 4];
    let pict = encode_pict(4, 4, &rgba).expect("encode");
    let probe = probe_pict(&pict).expect("probe");
    let header = probe.header.expect("probe header");
    assert!(header.is_extended());
    if let PictHeader::ExtendedV2 {
        optimal_source_rect,
        ..
    } = header
    {
        assert_eq!(optimal_source_rect.right, 4);
        assert_eq!(optimal_source_rect.bottom, 4);
    }
}

#[test]
fn probe_v1_picture_has_no_header() {
    // Minimal v1 stream: 10-byte picture record + version stanza
    // (0x1101) + a single-byte ClipRgn (0x01) with a trivial 10-byte
    // rectangular region + OpEndPic (0xFF).
    //
    // v1 has no HeaderOp per §A-25 / Listing A-7, so `header` must be
    // `None`.
    let mut pict: Vec<u8> = Vec::new();
    pict.extend_from_slice(&[0x00, 0x1F]); // picSize placeholder
    pict.extend_from_slice(&[0, 0, 0, 0, 0, 1, 0, 1]); // picFrame 0,0,1,1
    pict.extend_from_slice(&[0x11, 0x01]); // versionOp + version
    pict.push(0x01); // ClipRgn opcode
    pict.extend_from_slice(&[0x00, 0x0A, 0, 0, 0, 0, 0, 1, 0, 1]); // 10-byte rect region
    pict.push(0xFF); // OpEndPic

    let probe = probe_pict(&pict).expect("probe v1");
    assert!(probe.header.is_none(), "v1 has no HeaderOp");
}

// ---------------------------------------------------------------------------
// Tolerant fall-through: a v2 PICT whose 24-byte header payload starts
// with neither 0xFFFE nor 0xFFFF (e.g. pre-r217 oxideav-pict output that
// emitted [0u8; 24]) must still decode — `header` reports `None`.
// ---------------------------------------------------------------------------

#[test]
fn non_canonical_zeroed_header_is_tolerated() {
    let rgba = vec![0u8; 4 * 4 * 4];
    let mut pict = encode_pict(4, 4, &rgba).expect("encode");
    // Zero-out the 24-byte HeaderOp payload to simulate the pre-r217
    // encoder output.
    let payload_off = 512 + 2 + 8 + 2 + 2 + 2;
    for byte in &mut pict[payload_off..payload_off + 24] {
        *byte = 0;
    }
    let img = parse_pict(&pict).expect("decode tolerant");
    assert!(img.header.is_none());
    // Probe path tolerates it too.
    let probe = probe_pict(&pict).expect("probe tolerant");
    assert!(probe.header.is_none());
}

// ---------------------------------------------------------------------------
// Bit-exact verification of the emitted header bytes.
// ---------------------------------------------------------------------------

#[test]
fn emitted_header_bytes_match_listing_a5_structure_at_72dpi() {
    let rgba = vec![0u8; 8 * 8 * 4];
    let pict = encode_pict(8, 8, &rgba).expect("encode");
    let payload_off = 512 + 2 + 8 + 2 + 2 + 2;
    let payload = &pict[payload_off..payload_off + 24];
    // First two bytes = 0xFFFE — extended-v2 version.
    assert_eq!(&payload[0..2], &[0xFF, 0xFE]);
    // Bytes 4..8 = hRes = 0x00480000 (72 dpi).
    assert_eq!(&payload[4..8], &[0x00, 0x48, 0x00, 0x00]);
    // Bytes 8..12 = vRes = 0x00480000.
    assert_eq!(&payload[8..12], &[0x00, 0x48, 0x00, 0x00]);
    // Bytes 12..20 = optimal source rect = picFrame = (0, 0, 8, 8).
    assert_eq!(
        &payload[12..20],
        &[0x00, 0x00, 0x00, 0x00, 0x00, 0x08, 0x00, 0x08]
    );
}
