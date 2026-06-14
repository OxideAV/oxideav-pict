//! Round 295 — QuickDraw text-drawing pen-location tracking through the
//! `LongText` / `DHText` / `DVText` / `DHDVText` opcodes.
//!
//! Inside Macintosh: Imaging With QuickDraw, "About Basic QuickDraw"
//! (book page 2-13): *"Text is drawn with the base line positioned at
//! the pen location."* — so the location recorded by these opcodes is
//! where the text baseline sits. Appendix A, Table A-2 (and the v1
//! Table A-3) give the on-disk shape:
//!
//! * `LongText  $0028` — `txLoc (Point)`, `count (0..255)`, `text` —
//!   the absolute Point that establishes the text pen.
//! * `DHText    $0029` — `dh (0..255)`, `count`, `text` — advances the
//!   running text pen rightward.
//! * `DVText    $002A` — `dv (0..255)`, `count`, `text` — advances it
//!   downward.
//! * `DHDVText  $002B` — `dh (0..255)`, `dv (0..255)`, `count`, `text` —
//!   advances by both.
//!
//! The compact `DH/DV/DHDV` variants carry positive deltas relative to
//! the position the previous text opcode left, which is precisely why
//! they exist: successive `DrawText` calls on one line record only the
//! increment. The crate has no font rasteriser, so glyph bytes are
//! still walked past — but the spec-determined text origin and the
//! inter-call movement are now tracked into
//! `PictTextState::text_pen` / `text_op_count` (round 295).
//!
//! Point order on disk is `(v, h)`; the crate's pen tuple is `(h, v)`.

use oxideav_pict::ops::PictBuilder;
use oxideav_pict::parse_pict;

/// A `LongText` opcode body: `$0028`, `txLoc (v, h)`, `count`, `text`.
fn long_text(v: i16, h: i16, text: &[u8]) -> Vec<u8> {
    let mut b = vec![0x00, 0x28];
    b.extend_from_slice(&v.to_be_bytes());
    b.extend_from_slice(&h.to_be_bytes());
    b.push(text.len() as u8);
    b.extend_from_slice(text);
    b
}

/// A `DHText` opcode body: `$0029`, `dh`, `count`, `text`.
fn dh_text(dh: u8, text: &[u8]) -> Vec<u8> {
    let mut b = vec![0x00, 0x29, dh, text.len() as u8];
    b.extend_from_slice(text);
    b
}

/// A `DVText` opcode body: `$002A`, `dv`, `count`, `text`.
fn dv_text(dv: u8, text: &[u8]) -> Vec<u8> {
    let mut b = vec![0x00, 0x2A, dv, text.len() as u8];
    b.extend_from_slice(text);
    b
}

/// A `DHDVText` opcode body: `$002B`, `dh`, `dv`, `count`, `text`.
fn dhdv_text(dh: u8, dv: u8, text: &[u8]) -> Vec<u8> {
    let mut b = vec![0x00, 0x2B, dh, dv, text.len() as u8];
    b.extend_from_slice(text);
    b
}

/// A picture that emits text opcodes still has to produce *some* raster
/// or `parse_pict` returns `NoRaster`. We paint a 1×1 rect so the
/// decode succeeds and we can inspect `img.text_state`.
fn paint_dot(b: &mut PictBuilder) {
    use oxideav_pict::ops::Verb;
    b.rect(Verb::Paint, 0, 0, 1, 1);
}

// ---------------------------------------------------------------------------
// LongText establishes the absolute text pen.
// ---------------------------------------------------------------------------

#[test]
fn long_text_sets_absolute_pen() {
    let mut b = PictBuilder::new(0, 0, 64, 64);
    paint_dot(&mut b);
    // txLoc = (v=20, h=10). Pen tuple is (h, v) = (10, 20).
    b.push(&long_text(20, 10, b"hello"));
    let img = parse_pict(&b.finish()).unwrap();
    assert_eq!(img.text_state.text_pen, Some((10, 20)));
    assert_eq!(img.text_state.text_op_count, 1);
}

// ---------------------------------------------------------------------------
// DHText / DVText advance relative to the prior LongText.
// ---------------------------------------------------------------------------

#[test]
fn dh_text_advances_right_from_long_text() {
    let mut b = PictBuilder::new(0, 0, 64, 64);
    paint_dot(&mut b);
    b.push(&long_text(20, 10, b"a")); // pen = (10, 20)
    b.push(&dh_text(7, b"b")); //         pen = (17, 20)
    let img = parse_pict(&b.finish()).unwrap();
    assert_eq!(img.text_state.text_pen, Some((17, 20)));
    assert_eq!(img.text_state.text_op_count, 2);
}

#[test]
fn dv_text_advances_down_from_long_text() {
    let mut b = PictBuilder::new(0, 0, 64, 64);
    paint_dot(&mut b);
    b.push(&long_text(20, 10, b"a")); // pen = (10, 20)
    b.push(&dv_text(5, b"b")); //         pen = (10, 25)
    let img = parse_pict(&b.finish()).unwrap();
    assert_eq!(img.text_state.text_pen, Some((10, 25)));
    assert_eq!(img.text_state.text_op_count, 2);
}

#[test]
fn dhdv_text_advances_both_axes() {
    let mut b = PictBuilder::new(0, 0, 64, 64);
    paint_dot(&mut b);
    b.push(&long_text(20, 10, b"a")); //  pen = (10, 20)
    b.push(&dhdv_text(3, 4, b"b")); //    pen = (13, 24)
    let img = parse_pict(&b.finish()).unwrap();
    assert_eq!(img.text_state.text_pen, Some((13, 24)));
    assert_eq!(img.text_state.text_op_count, 2);
}

// ---------------------------------------------------------------------------
// Successive deltas accumulate (the whole reason the compact forms exist).
// ---------------------------------------------------------------------------

#[test]
fn successive_deltas_accumulate() {
    let mut b = PictBuilder::new(0, 0, 80, 80);
    paint_dot(&mut b);
    b.push(&long_text(30, 5, b"W")); //   pen = (5, 30)
    b.push(&dh_text(8, b"o")); //         pen = (13, 30)
    b.push(&dh_text(8, b"r")); //         pen = (21, 30)
    b.push(&dhdv_text(8, 12, b"d")); //   pen = (29, 42)
    let img = parse_pict(&b.finish()).unwrap();
    assert_eq!(img.text_state.text_pen, Some((29, 42)));
    assert_eq!(img.text_state.text_op_count, 4);
}

// ---------------------------------------------------------------------------
// A delta opcode with no prior LongText advances from the (0, 0) origin.
// ---------------------------------------------------------------------------

#[test]
fn delta_without_long_text_advances_from_origin() {
    let mut b = PictBuilder::new(0, 0, 64, 64);
    paint_dot(&mut b);
    b.push(&dhdv_text(11, 13, b"x")); //  pen = (0, 0) + (11, 13)
    let img = parse_pict(&b.finish()).unwrap();
    assert_eq!(img.text_state.text_pen, Some((11, 13)));
    assert_eq!(img.text_state.text_op_count, 1);
}

// ---------------------------------------------------------------------------
// A picture with no text opcodes leaves the slot at its None default.
// ---------------------------------------------------------------------------

#[test]
fn no_text_leaves_pen_none() {
    let mut b = PictBuilder::new(0, 0, 16, 16);
    paint_dot(&mut b);
    let img = parse_pict(&b.finish()).unwrap();
    assert_eq!(img.text_state.text_pen, None);
    assert_eq!(img.text_state.text_op_count, 0);
}

// ---------------------------------------------------------------------------
// Empty-string text opcodes (count = 0) still move the pen.
// ---------------------------------------------------------------------------

#[test]
fn empty_text_still_moves_pen() {
    let mut b = PictBuilder::new(0, 0, 64, 64);
    paint_dot(&mut b);
    b.push(&long_text(40, 8, b"")); //    pen = (8, 40)
    b.push(&dh_text(6, b"")); //          pen = (14, 40)
    let img = parse_pict(&b.finish()).unwrap();
    assert_eq!(img.text_state.text_pen, Some((14, 40)));
    assert_eq!(img.text_state.text_op_count, 2);
}

// ---------------------------------------------------------------------------
// v1 (8-bit opcode) dispatcher tracks the text pen identically.
// ---------------------------------------------------------------------------

/// A minimal v1 picture: 64×64 frame, `0x11 0x01` version stanza, a
/// `paintRect 0x31` for non-empty raster, then `extra` opcode bytes,
/// closed with `OpEndPic 0xFF`. v1 opcodes are byte-aligned (no word
/// padding), so the text-opcode bodies are emitted with a 1-byte opcode
/// rather than the 2-byte v2 form.
fn v1_pict(extra: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&[0, 0]); // picSize (ignored)
    out.extend_from_slice(&0i16.to_be_bytes()); // top
    out.extend_from_slice(&0i16.to_be_bytes()); // left
    out.extend_from_slice(&64i16.to_be_bytes()); // bottom
    out.extend_from_slice(&64i16.to_be_bytes()); // right
    out.extend_from_slice(&[0x11, 0x01]); // v1 version stanza
    out.extend_from_slice(&[0x31, 0, 0, 0, 0, 0, 1, 0, 1]); // paintRect 0..1
    out.extend_from_slice(extra);
    out.push(0xFF); // OpEndPic
    out
}

#[test]
fn v1_long_text_then_dh_text() {
    // v1 LongText 0x28: txLoc (v=25, h=12), count, text → pen (12, 25).
    let mut ops = vec![0x28];
    ops.extend_from_slice(&25i16.to_be_bytes());
    ops.extend_from_slice(&12i16.to_be_bytes());
    ops.push(2);
    ops.extend_from_slice(b"hi");
    // v1 DHText 0x29: dh=9 → pen (21, 25).
    ops.extend_from_slice(&[0x29, 9, 1, b'!']);
    let img = parse_pict(&v1_pict(&ops)).unwrap();
    assert_eq!(img.text_state.text_pen, Some((21, 25)));
    assert_eq!(img.text_state.text_op_count, 2);
}

#[test]
fn v1_dv_and_dhdv_text() {
    // v1 LongText then DVText 0x2A (dv=7) then DHDVText 0x2B (dh=3, dv=4).
    let mut ops = vec![0x28];
    ops.extend_from_slice(&10i16.to_be_bytes());
    ops.extend_from_slice(&10i16.to_be_bytes());
    ops.push(0); // empty text
    ops.extend_from_slice(&[0x2A, 7, 0]); //        pen (10, 17)
    ops.extend_from_slice(&[0x2B, 3, 4, 0]); //     pen (13, 21)
    let img = parse_pict(&v1_pict(&ops)).unwrap();
    assert_eq!(img.text_state.text_pen, Some((13, 21)));
    assert_eq!(img.text_state.text_op_count, 3);
}
