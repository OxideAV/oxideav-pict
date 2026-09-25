//! Round 461 — hostile input through the `$8200` / `$8201` render
//! paths.
//!
//! The compositor (matrix / mask / matte / mode), the built-in
//! `'raw '` decoder, the `'jpeg'` route and the warning-placeholder
//! lookahead all sit behind attacker-controlled lengths and fixed-point
//! cells. Deterministic counterpart of `fuzz/quicktime_8200`: every
//! truncation prefix of a family of QuickTime pictures, seeded byte
//! mutations, and systematic maxing of the matrix cells and the
//! description dimensions — the decoder and probe may return `Err`
//! or a typed non-render, never panic, and never allocate beyond the
//! declared canvas.

use oxideav_pict::ops::PictBuilder;
use oxideav_pict::state::RectI32;
use oxideav_pict::{
    build_direct_bits_rect_op, build_long_text, build_tx_size, parse_pict, probe_pict, Fixed,
    ImageDescription, PackType, QuickTimeCompressed, QuickTimeMatrix, QuickTimeMatte,
    QuickTimeRender, QuickTimeUncompressed,
};

fn desc(codec: [u8; 4], width: u16, height: u16, depth: u16) -> ImageDescription {
    ImageDescription {
        id_size: 86,
        codec,
        version: 1,
        revision_level: 1,
        vendor: *b"appl",
        temporal_quality: 0,
        spatial_quality: 0x0300,
        width,
        height,
        h_res: Fixed::SEVENTY_TWO_DPI,
        v_res: Fixed::SEVENTY_TWO_DPI,
        data_size: 0,
        frame_count: 1,
        name_raw: [0u8; 32],
        depth,
        clut_id: -1,
        extension: Vec::new(),
    }
}

fn raw32(n: usize) -> Vec<u8> {
    (0..n)
        .flat_map(|i| [0xFF, i as u8, (i * 7) as u8, 9])
        .collect()
}

/// The fixture family: identity raw, matte + mask, rotation, a
/// `'jpeg'`-tagged blob, an `$8201` with matrix + matte, and the
/// emitter placeholder shape.
fn corpus() -> Vec<Vec<u8>> {
    let mut out = Vec::new();

    let plain = QuickTimeCompressed::still(desc(*b"raw ", 8, 8, 32), raw32(64));
    let mut b = PictBuilder::new(0, 0, 8, 8);
    b.compressed_quicktime_image(&plain).unwrap();
    b.push(&build_tx_size(12));
    b.push(&build_long_text(0, 7, b"QuickTime and a").unwrap());
    b.push(&[0, 0]);
    out.push(b.finish());

    let mut rich = QuickTimeCompressed::still(desc(*b"raw ", 8, 8, 16), vec![0x7C; 128]);
    rich.matte = Some(QuickTimeMatte {
        description: desc(*b"raw ", 8, 8, 32),
        data: raw32(64),
    });
    rich.matte_rect = RectI32::from_be(0, 0, 8, 8);
    let mut rgn = Vec::new();
    rgn.extend_from_slice(&10u16.to_be_bytes());
    for v in [1i16, 1, 7, 7] {
        rgn.extend_from_slice(&v.to_be_bytes());
    }
    rich.mask_region = Some(rgn);
    rich.mode = 64;
    let mut b = PictBuilder::new(0, 0, 8, 8);
    b.compressed_quicktime_image(&rich).unwrap();
    out.push(b.finish());

    let mut rot = QuickTimeCompressed::still(desc(*b"raw ", 8, 4, 24), vec![0x40; 96]);
    rot.matrix = QuickTimeMatrix([
        Fixed(0),
        Fixed(0x0001_0000),
        Fixed(0),
        Fixed(-0x0001_0000),
        Fixed(0),
        Fixed(0),
        Fixed(0x0004_0000),
        Fixed(0),
        Fixed(0x4000_0000),
    ]);
    let mut b = PictBuilder::new(0, 0, 8, 8);
    b.compressed_quicktime_image(&rot).unwrap();
    out.push(b.finish());

    let jpeg = QuickTimeCompressed::still(
        desc(*b"jpeg", 8, 8, 24),
        [
            0xFFu8, 0xD8, 0xFF, 0xE0, 0x00, 0x10, b'J', b'F', b'I', b'F', 0, 1, 1, 0, 0, 72, 0, 72,
            0, 0,
        ]
        .repeat(3),
    );
    let mut b = PictBuilder::new(0, 0, 8, 8);
    b.compressed_quicktime_image(&jpeg).unwrap();
    out.push(b.finish());

    let sub = build_direct_bits_rect_op(0, 0, 4, 4, &[0x80; 64], PackType::Rle16).unwrap();
    let mut u = QuickTimeUncompressed::wrapping(&sub).unwrap();
    u.matrix = QuickTimeMatrix::scale_translate(
        Fixed(0x0002_0000),
        Fixed(0x0002_0000),
        Fixed(0),
        Fixed(0),
    );
    u.matte = Some(QuickTimeMatte {
        description: desc(*b"raw ", 4, 4, 32),
        data: raw32(16),
    });
    u.matte_rect = RectI32::from_be(0, 0, 4, 4);
    let mut b = PictBuilder::new(0, 0, 8, 8);
    b.uncompressed_quicktime_image(&u).unwrap();
    out.push(b.finish());

    out
}

fn exercise(bytes: &[u8]) {
    let _ = parse_pict(bytes);
    let _ = probe_pict(bytes);
}

#[test]
fn corpus_renders_before_it_is_attacked() {
    for (i, bytes) in corpus().iter().enumerate() {
        let img = parse_pict(bytes).unwrap_or_else(|e| panic!("fixture {i}: {e}"));
        assert_eq!(img.quicktime.len(), 1, "fixture {i}");
        match (i, &img.quicktime[0].render) {
            (3, QuickTimeRender::Failed(_)) => {} // the fake JPEG is rejected, typed
            (_, r) => assert!(r.is_rendered(), "fixture {i}: {r:?}"),
        }
    }
}

#[test]
fn every_truncation_prefix_is_err_or_typed_never_panic() {
    for bytes in corpus() {
        for cut in 0..bytes.len() {
            exercise(&bytes[..cut]);
        }
    }
}

#[test]
fn seeded_byte_mutations_never_panic() {
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = move || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    for bytes in corpus() {
        // Mutations land in the picture body (past the 512-byte pad).
        let body = 512..bytes.len();
        for _ in 0..1500 {
            let mut m = bytes.clone();
            let flips = 1 + (next() % 4) as usize;
            for _ in 0..flips {
                let at = body.start + (next() as usize % body.len());
                m[at] = match next() % 4 {
                    0 => 0x00,
                    1 => 0xFF,
                    2 => m[at] ^ (1 << (next() % 8)),
                    _ => next() as u8,
                };
            }
            exercise(&m);
        }
    }
}

#[test]
fn maxed_matrix_cells_and_dimensions_stay_bounded() {
    let base = QuickTimeCompressed::still(desc(*b"raw ", 8, 8, 32), raw32(64));
    for cell in 0..9 {
        for v in [i32::MAX, i32::MIN, 1, -1, 0, 0x4000_0000, 0x7FFF_FFFF] {
            let mut qt = base.clone();
            qt.matrix.0[cell] = Fixed(v);
            let mut b = PictBuilder::new(0, 0, 8, 8);
            b.compressed_quicktime_image(&qt).unwrap();
            let bytes = b.finish();
            let img = parse_pict(&bytes).expect("wrapper stays valid");
            assert!(
                !matches!(img.quicktime[0].render, QuickTimeRender::NotAttempted),
                "cell {cell} = {v:#x}"
            );
            let _ = probe_pict(&bytes);
        }
    }
    // Declared dimensions far beyond the data: typed failure, no
    // allocation of the declared size.
    for (w, h) in [(u16::MAX, u16::MAX), (u16::MAX, 1), (1, u16::MAX), (0, 0)] {
        let qt = QuickTimeCompressed::still(desc(*b"raw ", w, h, 32), raw32(16));
        let mut b = PictBuilder::new(0, 0, 8, 8);
        b.compressed_quicktime_image(&qt).unwrap();
        let img = parse_pict(&b.finish()).unwrap();
        assert!(
            matches!(img.quicktime[0].render, QuickTimeRender::Failed(_)),
            "{w}×{h}: {:?}",
            img.quicktime[0].render
        );
    }
    // A srcRect / matteRect / mask far outside everything.
    let mut qt = base.clone();
    qt.src_rect = RectI32::from_be(i16::MIN, i16::MIN, i16::MAX, i16::MAX);
    qt.matte = Some(QuickTimeMatte {
        description: desc(*b"raw ", 8, 8, 32),
        data: raw32(64),
    });
    qt.matte_rect = RectI32::from_be(i16::MAX, i16::MAX, i16::MIN, i16::MIN);
    let mut rgn = Vec::new();
    rgn.extend_from_slice(&10u16.to_be_bytes());
    for v in [i16::MIN, i16::MIN, i16::MAX, i16::MAX] {
        rgn.extend_from_slice(&v.to_be_bytes());
    }
    qt.mask_region = Some(rgn);
    let mut b = PictBuilder::new(0, 0, 8, 8);
    b.compressed_quicktime_image(&qt).unwrap();
    let img = parse_pict(&b.finish()).unwrap();
    assert!(img.quicktime[0].render.is_rendered());
}
