//! Round 401 — PictBuilder text-opcode emission.
//!
//! Through round 372 the four §A-3 text opcodes (`LongText $0028`,
//! `DHText $0029`, `DVText $002A`, `DHDVText $002B`) were decodable and
//! rasterised, but the `ops` emit surface had no builders for them —
//! synthetic text streams had to be hand-rolled byte-by-byte. Round 401
//! adds `build_long_text` / `build_dh_text` / `build_dv_text` /
//! `build_dhdv_text` plus chainable `PictBuilder` methods, closing the
//! encoder-parity gap for the text family.
//!
//! The tests pin the wire layout against Inside Macintosh: Imaging With
//! QuickDraw §A-3 Table A-2 (`txLoc` Point is `(v, h)` on disk), the
//! decode round-trip (ink lands where the builder said), and the
//! equivalence relations between the delta forms (`DHText dh` ==
//! `DHDVText (dh, 0)`, `DVText dv` == `DHDVText (0, dv)`).

use oxideav_pict::font::{measure_text, TextScale, GLYPH_H};
use oxideav_pict::ops::PictBuilder;
use oxideav_pict::PictTextFace;
use oxideav_pict::{
    build_dh_text, build_dhdv_text, build_dv_text, build_long_text, build_tx_size, parse_pict,
    probe_pict, PictImage,
};

/// True if pixel `(x, y)` in `img` is (near-)black ink.
fn is_ink(img: &PictImage, x: u32, y: u32) -> bool {
    if x >= img.width || y >= img.height {
        return false;
    }
    let off = ((y * img.width + x) * 4) as usize;
    let (r, g, b) = (img.data[off], img.data[off + 1], img.data[off + 2]);
    r < 40 && g < 40 && b < 40
}

/// Count ink pixels in the rectangle `[x0, x1) × [y0, y1)`.
fn count_ink(img: &PictImage, x0: u32, y0: u32, x1: u32, y1: u32) -> u32 {
    let mut n = 0;
    for y in y0..y1.min(img.height) {
        for x in x0..x1.min(img.width) {
            if is_ink(img, x, y) {
                n += 1;
            }
        }
    }
    n
}

// ---------------------------------------------------------------------------
// Wire layout: the builders emit exactly the §A-3 Table A-2 byte shapes.
// ---------------------------------------------------------------------------

#[test]
fn text_builders_pin_the_wire_layout() {
    // LongText: opcode, txLoc (v, h) — vertical first — count, text.
    assert_eq!(
        build_long_text(6, 14, b"HI").unwrap(),
        vec![0x00, 0x28, 0x00, 14, 0x00, 6, 2, b'H', b'I'],
    );
    // Negative coordinates ride the i16 big-endian encoding.
    assert_eq!(
        build_long_text(-2, -1, b"A").unwrap(),
        vec![0x00, 0x28, 0xFF, 0xFF, 0xFF, 0xFE, 1, b'A'],
    );
    // DHText: opcode, dh, count, text.
    assert_eq!(
        build_dh_text(20, b"B").unwrap(),
        vec![0x00, 0x29, 20, 1, b'B'],
    );
    // DVText: opcode, dv, count, text.
    assert_eq!(
        build_dv_text(9, b"xy").unwrap(),
        vec![0x00, 0x2A, 9, 2, b'x', b'y'],
    );
    // DHDVText: opcode, dh, dv, count, text.
    assert_eq!(
        build_dhdv_text(3, 4, b"Q").unwrap(),
        vec![0x00, 0x2B, 3, 4, 1, b'Q'],
    );
}

// ---------------------------------------------------------------------------
// The 1-byte count field caps a text run at 255 bytes.
// ---------------------------------------------------------------------------

#[test]
fn text_count_overflow_is_rejected() {
    let long = vec![b'a'; 256];
    assert!(build_long_text(0, 0, &long).is_err());
    assert!(build_dh_text(0, &long).is_err());
    assert!(build_dv_text(0, &long).is_err());
    assert!(build_dhdv_text(0, 0, &long).is_err());
    // 255 bytes is exactly representable.
    let max = vec![b'a'; 255];
    assert_eq!(build_long_text(0, 0, &max).unwrap().len(), 2 + 5 + 255);
}

// ---------------------------------------------------------------------------
// Round-trip: builder-emitted LongText inks the canvas at (h, v) and the
// argument order is (h, v) — shifting h moves the ink right, not down.
// ---------------------------------------------------------------------------

#[test]
fn long_text_h_argument_shifts_ink_horizontally() {
    let render = |h: i16| -> PictImage {
        let mut b = PictBuilder::new(0, 0, 80, 32);
        b.push(&build_tx_size(8));
        b.long_text(h, 20, b"X").unwrap();
        parse_pict(&b.finish()).unwrap()
    };
    let a = render(10);
    let b = render(15);
    // Every ink pixel of `a` reappears in `b` shifted right by 5.
    for y in 0..32u32 {
        for x in 0..75u32 {
            assert_eq!(
                is_ink(&a, x, y),
                is_ink(&b, x + 5, y),
                "h-shift mismatch at ({x},{y})"
            );
        }
    }
    // And there is ink to compare at all.
    assert!(count_ink(&a, 0, 0, 80, 32) >= 5);
}

#[test]
fn long_text_v_argument_shifts_ink_vertically() {
    let render = |v: i16| -> PictImage {
        let mut b = PictBuilder::new(0, 0, 80, 40);
        b.push(&build_tx_size(8));
        b.long_text(10, v, b"X").unwrap();
        parse_pict(&b.finish()).unwrap()
    };
    let a = render(16);
    let b = render(23);
    for y in 0..33u32 {
        for x in 0..80u32 {
            assert_eq!(
                is_ink(&a, x, y),
                is_ink(&b, x, y + 7),
                "v-shift mismatch at ({x},{y})"
            );
        }
    }
    assert!(count_ink(&a, 0, 0, 80, 40) >= 5);
}

// ---------------------------------------------------------------------------
// Delta-form equivalences: DHText == DHDVText with dv = 0, and
// DVText == DHDVText with dh = 0, pixel for pixel.
// ---------------------------------------------------------------------------

#[test]
fn dh_text_matches_dhdv_text_with_zero_dv() {
    let base = |b: &mut PictBuilder| {
        b.push(&build_tx_size(8));
        b.long_text(4, 16, b"A").unwrap();
    };
    let mut b1 = PictBuilder::new(0, 0, 120, 32);
    base(&mut b1);
    b1.dh_text(12, b"B").unwrap();
    let mut b2 = PictBuilder::new(0, 0, 120, 32);
    base(&mut b2);
    b2.dhdv_text(12, 0, b"B").unwrap();

    let i1 = parse_pict(&b1.finish()).unwrap();
    let i2 = parse_pict(&b2.finish()).unwrap();
    assert_eq!(i1.data, i2.data, "DHText(dh) must equal DHDVText(dh, 0)");
    assert_eq!(i1.text_state.text_pen, i2.text_state.text_pen);
    // Both really drew a second glyph right of the first.
    assert!(count_ink(&i1, 16, 8, 120, 17) >= 5);
}

#[test]
fn dv_text_matches_dhdv_text_with_zero_dh() {
    let base = |b: &mut PictBuilder| {
        b.push(&build_tx_size(8));
        b.long_text(4, 12, b"A").unwrap();
    };
    let mut b1 = PictBuilder::new(0, 0, 60, 48);
    base(&mut b1);
    b1.dv_text(11, b"B").unwrap();
    let mut b2 = PictBuilder::new(0, 0, 60, 48);
    base(&mut b2);
    b2.dhdv_text(0, 11, b"B").unwrap();

    let i1 = parse_pict(&b1.finish()).unwrap();
    let i2 = parse_pict(&b2.finish()).unwrap();
    assert_eq!(i1.data, i2.data, "DVText(dv) must equal DHDVText(0, dv)");
    assert_eq!(i1.text_state.text_pen, i2.text_state.text_pen);
    // The second glyph landed a line below the first.
    assert!(count_ink(&i1, 0, 15, 60, 24) >= 5);
}

// ---------------------------------------------------------------------------
// Chained builder text methods produce a stream whose final pen matches
// the font-metric prediction, and probe counts every text opcode.
// ---------------------------------------------------------------------------

#[test]
fn chained_text_methods_round_trip_pen_and_probe_counts() {
    let mut b = PictBuilder::new(0, 0, 160, 64);
    b.push(&build_tx_size(8));
    b.long_text(6, 14, b"HI").unwrap();
    b.dh_text(5, b"J").unwrap();
    b.dv_text(10, b"K").unwrap();
    b.dhdv_text(2, 3, b"L").unwrap();
    let bytes = b.finish();

    let scale = TextScale::isotropic(8);
    let adv = |t: &[u8]| measure_text(t, scale, 0, 0, 0, PictTextFace::PLAIN);
    // Pen walk: LongText sets (6, 14) then advances by "HI"; each delta
    // form advances from where the previous op left the pen, then by
    // its own text.
    let mut pen = (6 + adv(b"HI"), 14);
    pen = (pen.0 + 5 + adv(b"J"), pen.1);
    pen = (pen.0 + adv(b"K"), pen.1 + 10);
    pen = (pen.0 + 2 + adv(b"L"), pen.1 + 3);

    let img = parse_pict(&bytes).unwrap();
    assert_eq!(img.text_state.text_pen, Some(pen));
    assert_eq!(img.text_state.text_op_count, 4);

    let p = probe_pict(&bytes).unwrap();
    assert_eq!(p.text_count, 4);
}

// ---------------------------------------------------------------------------
// Odd-length text payloads keep the v2 stream word-aligned: a following
// opcode still decodes (the builder pads before the next push).
// ---------------------------------------------------------------------------

#[test]
fn odd_length_text_keeps_following_opcodes_aligned() {
    let mut b = PictBuilder::new(0, 0, 80, 40);
    b.push(&build_tx_size(8));
    // "A" makes the LongText record 9 bytes — odd.
    b.long_text(4, 12, b"A").unwrap();
    // A second text op must still land on a word boundary and decode.
    b.long_text(4, 30, b"B").unwrap();
    let img = parse_pict(&b.finish()).unwrap();
    assert_eq!(img.text_state.text_op_count, 2);
    // Both glyph boxes carry ink.
    assert!(count_ink(&img, 4, (12 - GLYPH_H) as u32, 16, 13) >= 5);
    assert!(count_ink(&img, 4, (30 - GLYPH_H) as u32, 16, 31) >= 5);
}
