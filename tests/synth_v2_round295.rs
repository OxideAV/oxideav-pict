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
//! increment.
//!
//! Round 352 turns the text opcodes from walk-past into a real raster:
//! the glyph bytes are now drawn through the crate's built-in clean-room
//! ASCII face, and — per the QuickDraw text-drawing model — the pen
//! advances rightward by each drawn glyph's width as it goes. So after a
//! text opcode the pen sits at the *end* of the drawn string, not at its
//! start. These tests therefore assert
//! `declared_position + measure_text(text)`, where `measure_text` is the
//! same advance the rasteriser uses (default `txSize = 12`, no
//! `chExtra` / `spExtra`). Empty strings advance by zero, so a `count = 0`
//! opcode still leaves the pen exactly at the declared position.
//!
//! Point order on disk is `(v, h)`; the crate's pen tuple is `(h, v)`.

use oxideav_pict::font::{measure_text, TextScale};
use oxideav_pict::ops::PictBuilder;
use oxideav_pict::parse_pict;

/// The default text size a freshly-initialised `PictTextState` carries
/// (`TxSize` defaults to 12 points). The synth pictures below never emit
/// a `TxSize` opcode, so every draw uses this size.
const DEFAULT_TX_SIZE: i32 = 12;

/// Horizontal advance the rasteriser adds for `text` at the default size.
fn adv(text: &[u8]) -> i32 {
    measure_text(text, TextScale::isotropic(DEFAULT_TX_SIZE), 0, 0, 0)
}

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
    // txLoc = (v=20, h=10). Pen tuple is (h, v) = (10, 20); after drawing
    // "hello" the pen has advanced right by the glyph widths.
    b.push(&long_text(20, 10, b"hello"));
    let img = parse_pict(&b.finish()).unwrap();
    assert_eq!(img.text_state.text_pen, Some((10 + adv(b"hello"), 20)));
    assert_eq!(img.text_state.text_op_count, 1);
}

// ---------------------------------------------------------------------------
// DHText / DVText advance relative to the prior LongText.
// ---------------------------------------------------------------------------

#[test]
fn dh_text_advances_right_from_long_text() {
    let mut b = PictBuilder::new(0, 0, 64, 64);
    paint_dot(&mut b);
    b.push(&long_text(20, 10, b"a")); // draw "a" → pen = (10 + adv(a), 20)
    b.push(&dh_text(7, b"b")); //         +dh 7, draw "b"
    let img = parse_pict(&b.finish()).unwrap();
    let h = 10 + adv(b"a") + 7 + adv(b"b");
    assert_eq!(img.text_state.text_pen, Some((h, 20)));
    assert_eq!(img.text_state.text_op_count, 2);
}

#[test]
fn dv_text_advances_down_from_long_text() {
    let mut b = PictBuilder::new(0, 0, 64, 64);
    paint_dot(&mut b);
    b.push(&long_text(20, 10, b"a")); // draw "a" → pen h advanced, v = 20
    b.push(&dv_text(5, b"b")); //         +dv 5, draw "b"
    let img = parse_pict(&b.finish()).unwrap();
    let h = 10 + adv(b"a") + adv(b"b");
    assert_eq!(img.text_state.text_pen, Some((h, 25)));
    assert_eq!(img.text_state.text_op_count, 2);
}

#[test]
fn dhdv_text_advances_both_axes() {
    let mut b = PictBuilder::new(0, 0, 64, 64);
    paint_dot(&mut b);
    b.push(&long_text(20, 10, b"a")); //  draw "a"
    b.push(&dhdv_text(3, 4, b"b")); //    +dh 3, +dv 4, draw "b"
    let img = parse_pict(&b.finish()).unwrap();
    let h = 10 + adv(b"a") + 3 + adv(b"b");
    assert_eq!(img.text_state.text_pen, Some((h, 24)));
    assert_eq!(img.text_state.text_op_count, 2);
}

// ---------------------------------------------------------------------------
// Successive deltas accumulate (the whole reason the compact forms exist).
// ---------------------------------------------------------------------------

#[test]
fn successive_deltas_accumulate() {
    let mut b = PictBuilder::new(0, 0, 80, 80);
    paint_dot(&mut b);
    b.push(&long_text(30, 5, b"W")); //   draw "W"
    b.push(&dh_text(8, b"o")); //         +8, draw "o"
    b.push(&dh_text(8, b"r")); //         +8, draw "r"
    b.push(&dhdv_text(8, 12, b"d")); //   +8, +12, draw "d"
    let img = parse_pict(&b.finish()).unwrap();
    let h = 5 + adv(b"W") + 8 + adv(b"o") + 8 + adv(b"r") + 8 + adv(b"d");
    assert_eq!(img.text_state.text_pen, Some((h, 42)));
    assert_eq!(img.text_state.text_op_count, 4);
}

// ---------------------------------------------------------------------------
// A delta opcode with no prior LongText advances from the (0, 0) origin.
// ---------------------------------------------------------------------------

#[test]
fn delta_without_long_text_advances_from_origin() {
    let mut b = PictBuilder::new(0, 0, 64, 64);
    paint_dot(&mut b);
    b.push(&dhdv_text(11, 13, b"x")); //  (0,0) + (11,13), draw "x"
    let img = parse_pict(&b.finish()).unwrap();
    assert_eq!(img.text_state.text_pen, Some((11 + adv(b"x"), 13)));
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
    // v1 DHText 0x29: dh=9, draw "!".
    ops.extend_from_slice(&[0x29, 9, 1, b'!']);
    let img = parse_pict(&v1_pict(&ops)).unwrap();
    let h = 12 + adv(b"hi") + 9 + adv(b"!");
    assert_eq!(img.text_state.text_pen, Some((h, 25)));
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
