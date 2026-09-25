//! Round 461 — ImageMagick-emitted PICT compatibility.
//!
//! `tests/fixtures/imagemagick_gradient_64x48.pict` (968 bytes) was
//! generated with `magick -size 64x48 gradient:red-blue out.pict`
//! (ImageMagick 7.1.2-29 Q16-HDRI, 2026-09-25) — a tool-generated
//! fixture with no third-party creative content. Its single
//! `DirectBitsRect` is 32-bit `packType = 4` with `rowBytes = 256`
//! and `cmpCount = 3`, and the emitter deviates from §A-3 "PixData" in
//! two ways this crate now tolerates:
//!
//! * the per-scanline byte count is **one byte** although
//!   `rowBytes > 250` calls for a word;
//! * each row's three component planes are packed as **one** PackBits
//!   stream, so a run may span a plane boundary (a red row is
//!   `C1 FF 81 00`: 64 × `FF` for R, then one 128-byte zero run
//!   covering G and B).
//!
//! The expected pixels come from ImageMagick's own black-box render
//! of the file (`magick out.pict out.png`), against which the crate's
//! render is byte-identical (AE 0 over 64×48).

use oxideav_pict::{parse_pict, probe_pict, ProbeTermination};

const FIXTURE: &[u8] = include_bytes!("fixtures/imagemagick_gradient_64x48.pict");

fn px(img: &oxideav_pict::PictImage, x: u32, y: u32) -> [u8; 3] {
    let off = ((y * img.width + x) * 4) as usize;
    [img.data[off], img.data[off + 1], img.data[off + 2]]
}

#[test]
fn imagemagick_gradient_decodes_like_its_emitter_renders_it() {
    let img = parse_pict(FIXTURE).expect("ImageMagick PICT decodes");
    assert_eq!((img.width, img.height), (64, 48));
    assert_eq!(px(&img, 0, 0), [255, 0, 0]);
    assert_eq!(px(&img, 0, 47), [0, 0, 255]);
    assert_eq!(px(&img, 63, 24), [125, 0, 130]);
    // Vertical gradient: every pixel in a row is the same colour and
    // red falls / blue rises monotonically down the image.
    for y in 0..48 {
        let p = px(&img, 0, y);
        assert!((0..64).all(|x| px(&img, x, y) == p), "row {y} is flat");
        if y > 0 {
            let q = px(&img, 0, y - 1);
            assert!(p[0] <= q[0] && p[2] >= q[2], "row {y} monotone");
        }
    }
}

#[test]
fn imagemagick_gradient_probes_to_end_pic() {
    let p = probe_pict(FIXTURE).unwrap();
    assert_eq!(p.raster_count, 1);
    assert_eq!(p.termination, ProbeTermination::EndPic);
}

/// Hand-built 64×1 32-bit `packType = 4` DirectBitsRect whose one row
/// uses a one-byte count under `rowBytes = 256` and a run spanning
/// the G and B planes — the ImageMagick shape, isolated.
#[test]
fn one_byte_count_and_plane_spanning_run_are_tolerated() {
    fn i16be(v: i16) -> [u8; 2] {
        v.to_be_bytes()
    }
    let mut p = Vec::new();
    p.extend_from_slice(&[0u8; 512]);
    let body_start = p.len();
    p.extend_from_slice(&[0, 0]); // picSize (patched below)
    for v in [0i16, 0, 1, 64] {
        p.extend_from_slice(&i16be(v)); // picFrame
    }
    p.extend_from_slice(&[0x00, 0x11, 0x02, 0xFF]); // Version 2
    p.extend_from_slice(&[0x0C, 0x00, 0xFF, 0xFE, 0, 0]); // HeaderOp
    p.extend_from_slice(&[0, 0x48, 0, 0, 0, 0x48, 0, 0]);
    for v in [0i16, 0, 1, 64] {
        p.extend_from_slice(&i16be(v));
    }
    p.extend_from_slice(&[0; 4]);
    p.extend_from_slice(&[0x00, 0x9A]); // DirectBitsRect
    p.extend_from_slice(&[0, 0, 0, 0xFF]); // baseAddr
    p.extend_from_slice(&0x8100u16.to_be_bytes()); // rowBytes 256 | PixMap flag
    for v in [0i16, 0, 1, 64] {
        p.extend_from_slice(&i16be(v)); // bounds
    }
    p.extend_from_slice(&[0, 0]); // pmVersion
    p.extend_from_slice(&[0, 4]); // packType 4
    p.extend_from_slice(&[0; 4]); // packSize
    p.extend_from_slice(&[0, 0x48, 0, 0, 0, 0x48, 0, 0]); // hRes / vRes
    p.extend_from_slice(&[0, 16]); // pixelType RGBDirect
    p.extend_from_slice(&[0, 32]); // pixelSize
    p.extend_from_slice(&[0, 3]); // cmpCount
    p.extend_from_slice(&[0, 8]); // cmpSize
    p.extend_from_slice(&[0; 12]); // planeBytes, pmTable, pmReserved
    for _ in 0..2 {
        for v in [0i16, 0, 1, 64] {
            p.extend_from_slice(&i16be(v)); // srcRect, dstRect
        }
    }
    p.extend_from_slice(&[0, 0]); // mode
                                  // Row: one-byte count 4, then R = 64 × 0xFF, G+B = one 128-byte
                                  // zero run.
    p.extend_from_slice(&[4, 0xC1, 0xFF, 0x81, 0x00]);
    p.push(0); // word alignment
    p.extend_from_slice(&[0x00, 0xFF]); // OpEndPic
    let size = (p.len() - body_start) as u16;
    p[body_start..body_start + 2].copy_from_slice(&size.to_be_bytes());

    let img = parse_pict(&p).expect("tolerated emitter deviation decodes");
    assert!(
        (0..64).all(|x| px(&img, x, 0) == [255, 0, 0]),
        "row decodes as red"
    );
    assert_eq!(
        probe_pict(&p).unwrap().termination,
        ProbeTermination::EndPic
    );
}
