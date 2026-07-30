//! Round 435 — typed QuickTime payload interiors + `$8201` blitting.
//!
//! Inside Macintosh: QuickTime (1993) Chapter 3 "Image Compression
//! Manager" publishes the layout that Imaging With QuickDraw §A-3
//! declares "private to QuickTime": Table 3-1 (page 3-26) for
//! `CompressedQuickTime $8200`, Table 3-2 (page 3-27) for
//! `UncompressedQuickTime $8201`, and the `ImageDescription`
//! structure on pages 3-49 – 3-51. These tests compose conforming
//! payloads by hand from those tables and pin:
//!
//! * the `$8200` typed surface (matrix, mode, srcRect, matte, mask,
//!   `ImageDescription`, compressed image bytes + codec FourCC);
//! * the "`dataSize` may be 0 if the size is unknown" recovery rule;
//! * `$8201` re-entering the normal `$9A` raster dispatch — pixels
//!   actually land on the canvas;
//! * the page 3-26 degradation rule: an interior that doesn't match
//!   the published layout keeps the verbatim capture (`image = None`)
//!   without failing the picture, because the `Size` field is
//!   authoritative even for a reader that cannot decode the payload.

use oxideav_pict::ops::PictBuilder;
use oxideav_pict::state::RectI32;
use oxideav_pict::{
    build_direct_bits_rect_op, parse_pict, Fixed, ImageDescription, PackType, QuickTimeCompressed,
    QuickTimeMatrix, QuickTimePayload, QuickTimeUncompressed, Verb,
};

/// A minimal conforming `ImageDescription` (idSize = 86).
fn desc(codec: [u8; 4], width: u16, height: u16, data_size: u32) -> ImageDescription {
    let mut name_raw = [0u8; 32];
    name_raw[0] = 4;
    name_raw[1..5].copy_from_slice(b"Test");
    ImageDescription {
        id_size: 86,
        codec,
        version: 1,
        revision_level: 1,
        vendor: *b"appl",
        temporal_quality: 0,
        spatial_quality: 0x0200,
        width,
        height,
        h_res: Fixed::SEVENTY_TWO_DPI,
        v_res: Fixed::SEVENTY_TWO_DPI,
        data_size,
        frame_count: 1,
        name_raw,
        depth: 24,
        clut_id: -1,
        extension: Vec::new(),
    }
}

/// Compose a `$8200` payload (the bytes after the `Size` long) per
/// Table 3-1: Version, Matrix, MatteSize, MatteRect, Mode, SrcRect,
/// Accuracy, MaskSize, then the gated variable fields.
fn compressed_payload(d: &ImageDescription, image_data: &[u8]) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&1u16.to_be_bytes());
    p.extend_from_slice(&QuickTimeMatrix::IDENTITY.to_wire());
    p.extend_from_slice(&0u32.to_be_bytes()); // matteSize = 0
    p.extend_from_slice(&[0u8; 8]); // matteRect
    p.extend_from_slice(&0u16.to_be_bytes()); // mode = srcCopy
    for v in [0i16, 0, d.height as i16, d.width as i16] {
        p.extend_from_slice(&v.to_be_bytes()); // srcRect
    }
    p.extend_from_slice(&0x0200u32.to_be_bytes()); // accuracy
    p.extend_from_slice(&0u32.to_be_bytes()); // maskSize = 0
    p.extend_from_slice(&d.to_bytes());
    p.extend_from_slice(image_data);
    p
}

/// Compose a `$8201` payload per Table 3-2: Version, Matrix,
/// MatteSize, MatteRect, then the embedded subopcode chunk (opcode
/// word + data, wholly inside the wrapper's `Size` window).
fn uncompressed_payload(sub_chunk: &[u8]) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&1u16.to_be_bytes());
    p.extend_from_slice(&QuickTimeMatrix::IDENTITY.to_wire());
    p.extend_from_slice(&0u32.to_be_bytes()); // matteSize = 0
    p.extend_from_slice(&[0u8; 8]); // matteRect
    p.extend_from_slice(sub_chunk);
    p
}

// ---------------------------------------------------------------------------
// $8200: typed surface
// ---------------------------------------------------------------------------

#[test]
fn compressed_quicktime_surfaces_typed_payload_and_codec() {
    let jpeg_ish: Vec<u8> = (0..200u32).map(|i| (i * 7) as u8).collect();
    let d = desc(*b"jpeg", 64, 48, jpeg_ish.len() as u32);
    let payload = compressed_payload(&d, &jpeg_ish);

    let mut b = PictBuilder::new(0, 0, 16, 16);
    b.rect(Verb::Paint, 2, 2, 6, 6);
    b.compressed_quicktime(&payload).unwrap();
    let img = parse_pict(&b.finish()).unwrap();

    assert_eq!(img.quicktime.len(), 1);
    let qt = &img.quicktime[0];
    assert!(qt.compressed);
    assert_eq!(qt.data, payload); // verbatim capture still stands
    let Some(QuickTimePayload::Compressed(c)) = &qt.image else {
        panic!("typed $8200 payload expected, got {:?}", qt.image);
    };
    assert_eq!(c.version, 1);
    assert!(c.matrix.is_identity());
    assert_eq!(c.mode, 0);
    assert_eq!(c.src_rect, RectI32::from_be(0, 0, 48, 64));
    assert_eq!(c.accuracy, 0x0200);
    assert!(c.matte.is_none());
    assert!(c.mask_region.is_none());
    assert_eq!(c.image_description.codec, *b"jpeg");
    assert_eq!(c.image_description.codec_str(), "jpeg");
    assert_eq!(c.image_description.width, 64);
    assert_eq!(c.image_description.height, 48);
    assert_eq!(c.image_description.name(), "Test");
    assert_eq!(c.image_data, jpeg_ish);
    assert_eq!(qt.image.as_ref().unwrap().codec(), Some(*b"jpeg"));
}

#[test]
fn compressed_quicktime_zero_data_size_recovers_from_size_window() {
    // Page 3-51: dataSize "Set this field to 0 if the size is
    // unknown" — the image data then runs to the end of the
    // Size-bounded payload.
    let blob = vec![0xC3u8; 77];
    let d = desc(*b"rpza", 32, 32, 0);
    let payload = compressed_payload(&d, &blob);

    let mut b = PictBuilder::new(0, 0, 8, 8);
    b.rect(Verb::Paint, 0, 0, 2, 2);
    b.compressed_quicktime(&payload).unwrap();
    let img = parse_pict(&b.finish()).unwrap();

    let Some(QuickTimePayload::Compressed(c)) = &img.quicktime[0].image else {
        panic!("typed $8200 payload expected");
    };
    assert_eq!(c.image_description.data_size, 0);
    assert_eq!(c.image_data, blob);
    assert_eq!(c.image_description.codec_str(), "rpza");
}

#[test]
fn malformed_compressed_interior_degrades_to_verbatim_capture() {
    // Page 3-26: `Size` is authoritative — a reader that cannot
    // decode the payload must still walk past it. Garbage interior:
    // picture parses, verbatim capture stands, typed view absent.
    let garbage = vec![0xFFu8; 30]; // far too short for the 68-byte fixed part
    let mut b = PictBuilder::new(0, 0, 8, 8);
    b.rect(Verb::Paint, 0, 0, 2, 2);
    b.compressed_quicktime(&garbage).unwrap();
    let img = parse_pict(&b.finish()).unwrap();

    assert_eq!(img.quicktime.len(), 1);
    assert_eq!(img.quicktime[0].data, garbage);
    assert!(img.quicktime[0].image.is_none());
    // The paint before the opcode still landed (walker resumed).
    let off = (8 + 1) * 4;
    assert_eq!(&img.data[off..off + 4], &[0, 0, 0, 255]);
}

// ---------------------------------------------------------------------------
// $8201: embedded subopcode is rasterised
// ---------------------------------------------------------------------------

#[test]
fn uncompressed_quicktime_blits_embedded_direct_bits_rect() {
    // A 4×4 solid-red DirectBitsRect ($9A) at (2,2)→(6,6), wrapped in
    // the $8201 fixed header. Table 3-2: the subopcode "is entirely
    // within the other opcode" — its pixels must land on the canvas.
    let red: Vec<u8> = [255u8, 0, 0, 255].repeat(16);
    let sub = build_direct_bits_rect_op(2, 2, 6, 6, &red, PackType::Raw).unwrap();
    let payload = uncompressed_payload(&sub);

    let mut b = PictBuilder::new(0, 0, 16, 16);
    b.uncompressed_quicktime(&payload).unwrap();
    let img = parse_pict(&b.finish()).unwrap();

    let qt = &img.quicktime[0];
    assert!(!qt.compressed);
    let Some(QuickTimePayload::Uncompressed(u)) = &qt.image else {
        panic!("typed $8201 payload expected, got {:?}", qt.image);
    };
    assert_eq!(u.subopcode, 0x009A);
    assert!(u.subopcode_in_range());
    assert!(u.matte.is_none());
    assert!(u.matrix.is_identity());
    assert_eq!(u.sub_data, sub[2..]); // bytes after the subopcode word

    // Interior of the blit is red; outside stays background.
    for (x, y, inside) in [(3, 3, true), (2, 2, true), (5, 5, true), (7, 7, false)] {
        let off = (y * 16 + x) * 4;
        let px = &img.data[off..off + 4];
        if inside {
            assert_eq!(px, &[255, 0, 0, 255], "pixel ({x},{y})");
        } else {
            assert_ne!(px, &[255, 0, 0, 255], "pixel ({x},{y})");
        }
    }
}

#[test]
fn uncompressed_quicktime_with_truncated_sub_data_degrades() {
    // Wrapper parses but the $9A pixel rows are cut short: the
    // picture must still parse (Size-bounded skip), the canvas stays
    // untouched, and the typed view is withdrawn.
    let red: Vec<u8> = [255u8, 0, 0, 255].repeat(16);
    let sub = build_direct_bits_rect_op(2, 2, 6, 6, &red, PackType::Raw).unwrap();
    let payload = uncompressed_payload(&sub[..sub.len() - 10]);

    let mut b = PictBuilder::new(0, 0, 16, 16);
    b.rect(Verb::Paint, 10, 10, 12, 12);
    b.uncompressed_quicktime(&payload).unwrap();
    let img = parse_pict(&b.finish()).unwrap();

    let qt = &img.quicktime[0];
    assert!(qt.image.is_none());
    assert_eq!(qt.data, payload);
    // No red pixel anywhere.
    assert!(
        !img.data.chunks(4).any(|px| px == [255, 0, 0, 255]),
        "truncated sub-opcode must not blit"
    );
    // The paint opcode before it still landed.
    let off = (11 * 16 + 11) * 4;
    assert_eq!(&img.data[off..off + 4], &[0, 0, 0, 255]);
}

#[test]
fn uncompressed_quicktime_with_matte_still_reaches_subopcode() {
    // MatteSize != 0: the matte ImageDescription + matte data precede
    // the subopcode (Table 3-2 variable fields, in order).
    let matte_data = [0x55u8; 9];
    let matte_desc = desc(*b"raw ", 4, 4, matte_data.len() as u32);
    let red: Vec<u8> = [255u8, 0, 0, 255].repeat(16);
    let sub = build_direct_bits_rect_op(2, 2, 6, 6, &red, PackType::Raw).unwrap();

    let mut p = Vec::new();
    p.extend_from_slice(&1u16.to_be_bytes());
    p.extend_from_slice(&QuickTimeMatrix::IDENTITY.to_wire());
    p.extend_from_slice(&(matte_data.len() as u32).to_be_bytes());
    for v in [0i16, 0, 4, 4] {
        p.extend_from_slice(&v.to_be_bytes()); // matteRect
    }
    p.extend_from_slice(&matte_desc.to_bytes());
    p.extend_from_slice(&matte_data);
    p.extend_from_slice(&sub);

    let mut b = PictBuilder::new(0, 0, 16, 16);
    b.uncompressed_quicktime(&p).unwrap();
    let img = parse_pict(&b.finish()).unwrap();

    let Some(QuickTimePayload::Uncompressed(u)) = &img.quicktime[0].image else {
        panic!("typed $8201 payload expected");
    };
    let matte = u.matte.as_ref().expect("matte present");
    assert_eq!(matte.description.codec, *b"raw ");
    assert_eq!(matte.data, matte_data);
    assert_eq!(u.matte_rect, RectI32::from_be(0, 0, 4, 4));
    assert_eq!(u.subopcode, 0x009A);
    // Blit still happened after the matte fields.
    let off = (3 * 16 + 3) * 4;
    assert_eq!(&img.data[off..off + 4], &[255, 0, 0, 255]);
}

// ---------------------------------------------------------------------------
// $8200 with matte + mask region round-trips through parse_pict
// ---------------------------------------------------------------------------

#[test]
fn compressed_quicktime_matte_and_mask_survive_parse_pict() {
    let matte_data = [0x0Fu8; 6];
    let matte_desc = desc(*b"raw ", 8, 8, matte_data.len() as u32);
    let mut mask = Vec::new();
    mask.extend_from_slice(&10u16.to_be_bytes()); // rgnSize = 10 (rect region)
    for v in [0i16, 0, 8, 8] {
        mask.extend_from_slice(&v.to_be_bytes());
    }
    let img_data = [0x77u8; 12];
    let img_desc = desc(*b"jpeg", 8, 8, img_data.len() as u32);

    let mut p = Vec::new();
    p.extend_from_slice(&1u16.to_be_bytes());
    p.extend_from_slice(&QuickTimeMatrix::IDENTITY.to_wire());
    p.extend_from_slice(&(matte_data.len() as u32).to_be_bytes());
    for v in [0i16, 0, 8, 8] {
        p.extend_from_slice(&v.to_be_bytes()); // matteRect
    }
    p.extend_from_slice(&0u16.to_be_bytes()); // mode
    for v in [0i16, 0, 8, 8] {
        p.extend_from_slice(&v.to_be_bytes()); // srcRect
    }
    p.extend_from_slice(&0u32.to_be_bytes()); // accuracy
    p.extend_from_slice(&(mask.len() as u32).to_be_bytes()); // maskSize
    p.extend_from_slice(&matte_desc.to_bytes());
    p.extend_from_slice(&matte_data);
    p.extend_from_slice(&mask);
    p.extend_from_slice(&img_desc.to_bytes());
    p.extend_from_slice(&img_data);

    let mut b = PictBuilder::new(0, 0, 8, 8);
    b.rect(Verb::Paint, 0, 0, 2, 2);
    b.compressed_quicktime(&p).unwrap();
    let img = parse_pict(&b.finish()).unwrap();

    let Some(QuickTimePayload::Compressed(c)) = &img.quicktime[0].image else {
        panic!("typed $8200 payload expected");
    };
    let matte = c.matte.as_ref().expect("matte present");
    assert_eq!(matte.description, matte_desc);
    assert_eq!(matte.data, matte_data);
    assert_eq!(c.mask_region.as_deref(), Some(mask.as_slice()));
    assert_eq!(c.image_data, img_data);
}

// ---------------------------------------------------------------------------
// Typed emit → parse round-trips
// ---------------------------------------------------------------------------

#[test]
fn typed_compressed_builder_round_trips_through_parse_pict() {
    let blob: Vec<u8> = (0..150u32).map(|i| (i * 3) as u8).collect();
    let qt = QuickTimeCompressed::still(desc(*b"jpeg", 40, 30, 0), blob.clone());
    assert_eq!(qt.image_description.data_size, blob.len() as u32);
    assert_eq!(qt.src_rect, RectI32::from_be(0, 0, 30, 40));

    let mut b = PictBuilder::new(0, 0, 8, 8);
    b.rect(Verb::Paint, 0, 0, 2, 2);
    b.compressed_quicktime_image(&qt).unwrap();
    let img = parse_pict(&b.finish()).unwrap();

    let Some(QuickTimePayload::Compressed(back)) = &img.quicktime[0].image else {
        panic!("typed $8200 payload expected");
    };
    assert_eq!(back, &qt);
}

#[test]
fn typed_uncompressed_builder_round_trips_and_blits() {
    let red: Vec<u8> = [255u8, 0, 0, 255].repeat(16);
    let sub = build_direct_bits_rect_op(2, 2, 6, 6, &red, PackType::Raw).unwrap();
    let qt = QuickTimeUncompressed::wrapping(&sub).unwrap();
    assert_eq!(qt.subopcode, 0x009A);

    let mut b = PictBuilder::new(0, 0, 16, 16);
    b.uncompressed_quicktime_image(&qt).unwrap();
    let img = parse_pict(&b.finish()).unwrap();

    let Some(QuickTimePayload::Uncompressed(back)) = &img.quicktime[0].image else {
        panic!("typed $8201 payload expected");
    };
    assert_eq!(back, &qt);
    let off = (3 * 16 + 3) * 4;
    assert_eq!(&img.data[off..off + 4], &[255, 0, 0, 255]);
}

#[test]
fn typed_builders_reject_inconsistent_structures() {
    // dataSize disagreement (non-zero but wrong).
    let mut qt = QuickTimeCompressed::still(desc(*b"jpeg", 8, 8, 0), vec![1, 2, 3]);
    qt.image_description.data_size = 999;
    assert!(qt.to_payload_bytes().is_err());
    // dataSize = 0 stays legal ("size unknown" form).
    qt.image_description.data_size = 0;
    assert!(qt.to_payload_bytes().is_ok());
    // Undersized mask region.
    qt.mask_region = Some(vec![0u8; 4]);
    assert!(qt.to_payload_bytes().is_err());

    // Sub-chunk with a non-pixel-data opcode.
    assert!(QuickTimeUncompressed::wrapping(&[0x00, 0x90, 0xAA]).is_err());
    // Sub-chunk shorter than the opcode word.
    assert!(QuickTimeUncompressed::wrapping(&[0x00]).is_err());
}

// ---------------------------------------------------------------------------
// Probe surfaces per-opcode QuickTime summaries
// ---------------------------------------------------------------------------

#[test]
fn probe_summarises_quicktime_opcodes() {
    use oxideav_pict::probe_pict;

    let blob = vec![0x5Au8; 33];
    let c = QuickTimeCompressed::still(desc(*b"jpeg", 640, 480, 0), blob);
    let red: Vec<u8> = [255u8, 0, 0, 255].repeat(16);
    let sub = build_direct_bits_rect_op(2, 2, 6, 6, &red, PackType::Raw).unwrap();
    let u = QuickTimeUncompressed::wrapping(&sub).unwrap();

    let mut b = PictBuilder::new(0, 0, 16, 16);
    b.compressed_quicktime_image(&c).unwrap();
    b.uncompressed_quicktime_image(&u).unwrap();
    // A garbage $8200 interior still shows up as a bare-count row.
    b.compressed_quicktime(&[0xEE; 12]).unwrap();
    let p = probe_pict(&b.finish()).unwrap();

    assert!(p.has_quicktime());
    assert_eq!(p.compressed_quicktime_count, 2);
    assert_eq!(p.uncompressed_quicktime_count, 1);
    assert_eq!(p.quicktime.len(), 3);

    let q0 = &p.quicktime[0];
    assert!(q0.compressed);
    assert_eq!(q0.codec, Some(*b"jpeg"));
    assert_eq!(q0.codec_str().as_deref(), Some("jpeg"));
    assert_eq!(
        (q0.width, q0.height, q0.depth),
        (Some(640), Some(480), Some(24))
    );
    assert_eq!(q0.has_matte, Some(false));
    assert_eq!(q0.has_mask, Some(false));
    assert_eq!(q0.subopcode, None);

    let q1 = &p.quicktime[1];
    assert!(!q1.compressed);
    assert_eq!(q1.codec, None);
    assert_eq!(q1.subopcode, Some(0x009A));
    assert_eq!(q1.has_matte, Some(false));

    let q2 = &p.quicktime[2];
    assert!(q2.compressed);
    assert_eq!(q2.payload_len, 12);
    assert_eq!(q2.codec, None); // interior didn't match the layout
    assert_eq!(q2.has_matte, None);
}
