//! Round 401 — encoder-parity sweep for the walker's remaining
//! decodable-but-not-emittable opcodes.
//!
//! * `ShortLine $0022` / `ShortLineFrom $0023` — compact SignedByte
//!   line forms (`build_short_line` / `build_short_line_from` plus the
//!   `PictBuilder::short_line` / `short_line_from` / `line_from`
//!   methods).
//! * `Origin $000C` — coordinate-origin delta (`build_origin` /
//!   `PictBuilder::origin`), which also pins the round-401 decoder
//!   sign fix: per the `SetOrigin` semantics (Inside Macintosh:
//!   Imaging With QuickDraw §2, book pages 2-23 f.) *increasing* the
//!   origin moves subsequently drawn shapes up / left.
//! * The same-shape verb families (`0x0038` / `0x0048` / `0x0058` /
//!   `0x0068` + verb nibble) — `build_same_rect_op` /
//!   `build_same_round_rect_op` / `build_same_oval_op` /
//!   `build_same_arc_op` and the matching builder methods.
//! * `ClipRgn $0001` — rectangular clip via `PictBuilder::clip_rect`.
//!
//! Every behavioural test asserts pixel-exact equality between the
//! compact form and the equivalent explicit-coordinate stream.

use oxideav_pict::ops::PictBuilder;
use oxideav_pict::{
    build_origin, build_same_arc_op, build_same_oval_op, build_same_rect_op,
    build_same_round_rect_op, build_short_line, build_short_line_from, parse_pict, PictImage, Verb,
};

/// Count non-white pixels.
fn ink_count(img: &PictImage) -> usize {
    img.data
        .chunks_exact(4)
        .filter(|px| px[0] < 240 || px[1] < 240 || px[2] < 240)
        .count()
}

// ---------------------------------------------------------------------------
// Wire layouts pinned against §A-3 Table A-2.
// ---------------------------------------------------------------------------

#[test]
fn parity_builders_pin_the_wire_layout() {
    // ShortLine: opcode, pnLoc (v, h), dh, dv as SignedBytes.
    assert_eq!(
        build_short_line(10, 12, -3, 5),
        vec![0x00, 0x22, 0x00, 12, 0x00, 10, 0xFD, 5],
    );
    // ShortLineFrom: opcode, dh, dv.
    assert_eq!(build_short_line_from(7, -1), vec![0x00, 0x23, 7, 0xFF]);
    // Origin: opcode, dh, dv as Integers.
    assert_eq!(
        build_origin(-5, 260),
        vec![0x00, 0x0C, 0xFF, 0xFB, 0x01, 0x04],
    );
    // Same-shape families: base | verb nibble, no rect payload.
    assert_eq!(build_same_rect_op(Verb::Frame), vec![0x00, 0x38]);
    assert_eq!(build_same_rect_op(Verb::Fill), vec![0x00, 0x3C]);
    assert_eq!(build_same_round_rect_op(Verb::Paint), vec![0x00, 0x49]);
    assert_eq!(build_same_oval_op(Verb::Invert), vec![0x00, 0x5B]);
    // SameArc carries fresh startAngle / arcAngle words.
    assert_eq!(
        build_same_arc_op(Verb::Erase, 90, -45),
        vec![0x00, 0x6A, 0x00, 90, 0xFF, 0xD3],
    );
}

// ---------------------------------------------------------------------------
// ShortLine == Line with the same endpoints, pixel for pixel.
// ---------------------------------------------------------------------------

#[test]
fn short_line_matches_explicit_line() {
    let mut b1 = PictBuilder::new(0, 0, 40, 30);
    b1.short_line(10, 12, 15, 5);
    let mut b2 = PictBuilder::new(0, 0, 40, 30);
    b2.line(10, 12, 25, 17);
    let i1 = parse_pict(&b1.finish()).unwrap();
    let i2 = parse_pict(&b2.finish()).unwrap();
    assert_eq!(i1.data, i2.data, "ShortLine must equal the explicit Line");
    assert!(ink_count(&i1) >= 10);
}

#[test]
fn short_line_from_continues_a_polyline() {
    // line + short_line_from(-3, 7) == line + line_from at the summed
    // endpoint; negative SignedByte dh exercises the sign extension.
    let mut b1 = PictBuilder::new(0, 0, 40, 30);
    b1.line(5, 5, 10, 8).short_line_from(-3, 7);
    let mut b2 = PictBuilder::new(0, 0, 40, 30);
    b2.line(5, 5, 10, 8).line_from(7, 15);
    let i1 = parse_pict(&b1.finish()).unwrap();
    let i2 = parse_pict(&b2.finish()).unwrap();
    assert_eq!(i1.data, i2.data);
    assert!(ink_count(&i1) >= 10);
}

// ---------------------------------------------------------------------------
// Origin: positive deltas move subsequent shapes up / left (SetOrigin,
// book pages 2-23 f.) — pinned against the explicitly-shifted rect.
// ---------------------------------------------------------------------------

#[test]
fn origin_moves_subsequent_shapes_up_left() {
    let mut with_origin = PictBuilder::new(0, 0, 40, 40);
    with_origin.origin(5, 3);
    with_origin.rect(Verb::Paint, 10, 10, 20, 20);
    let mut explicit = PictBuilder::new(0, 0, 40, 40);
    explicit.rect(Verb::Paint, 10 - 3, 10 - 5, 20 - 3, 20 - 5);
    let i1 = parse_pict(&with_origin.finish()).unwrap();
    let i2 = parse_pict(&explicit.finish()).unwrap();
    assert_eq!(
        i1.data, i2.data,
        "Origin(5, 3) must equal drawing at (left − 5, top − 3)"
    );
    assert_eq!(ink_count(&i1), 100);
}

#[test]
fn origin_deltas_accumulate_and_negative_deltas_move_down_right() {
    // Two Origin opcodes accumulate; a negative delta shifts the other
    // way. Net shift here: dh = 4 − 10 = −6 (right 6), dv = −2 (down 2).
    let mut b1 = PictBuilder::new(0, 0, 40, 40);
    b1.origin(4, 0).origin(-10, -2);
    b1.rect(Verb::Paint, 8, 8, 16, 16);
    let mut b2 = PictBuilder::new(0, 0, 40, 40);
    b2.rect(Verb::Paint, 10, 14, 18, 22);
    let i1 = parse_pict(&b1.finish()).unwrap();
    let i2 = parse_pict(&b2.finish()).unwrap();
    assert_eq!(i1.data, i2.data);
    assert_eq!(ink_count(&i1), 64);
}

// ---------------------------------------------------------------------------
// Same-shape verbs replay the previous shape's geometry.
// ---------------------------------------------------------------------------

#[test]
fn same_rect_replays_the_previous_rect() {
    // Paint then invert the SAME rect: interior flips white-on-black →
    // back to… black XOR = white? No — invert NOTs the painted black
    // to white, leaving zero ink. That pins both the replayed geometry
    // and the verb dispatch.
    let mut b1 = PictBuilder::new(0, 0, 30, 30);
    b1.rect(Verb::Paint, 5, 5, 15, 15).same_rect(Verb::Invert);
    let mut b2 = PictBuilder::new(0, 0, 30, 30);
    b2.rect(Verb::Paint, 5, 5, 15, 15)
        .rect(Verb::Invert, 5, 5, 15, 15);
    let i1 = parse_pict(&b1.finish()).unwrap();
    let i2 = parse_pict(&b2.finish()).unwrap();
    assert_eq!(i1.data, i2.data);
    assert_eq!(ink_count(&i1), 0, "paint + invert of the same rect cancels");
}

#[test]
fn same_oval_and_same_round_rect_replay_geometry() {
    let mut b1 = PictBuilder::new(0, 0, 60, 30);
    b1.oval_size(4, 4);
    b1.oval(Verb::Paint, 2, 2, 14, 14).same_oval(Verb::Invert);
    b1.round_rect(Verb::Paint, 2, 30, 14, 50)
        .same_round_rect(Verb::Invert);
    let mut b2 = PictBuilder::new(0, 0, 60, 30);
    b2.oval_size(4, 4);
    b2.oval(Verb::Paint, 2, 2, 14, 14)
        .oval(Verb::Invert, 2, 2, 14, 14);
    b2.round_rect(Verb::Paint, 2, 30, 14, 50)
        .round_rect(Verb::Invert, 2, 30, 14, 50);
    let i1 = parse_pict(&b1.finish()).unwrap();
    let i2 = parse_pict(&b2.finish()).unwrap();
    assert_eq!(i1.data, i2.data);
    assert_eq!(ink_count(&i1), 0);
}

#[test]
fn same_arc_shares_the_rect_but_takes_fresh_angles() {
    // A fan of two quarter-wedges over one enclosing rect.
    let mut b1 = PictBuilder::new(0, 0, 40, 40);
    b1.arc(Verb::Paint, 4, 4, 36, 36, 0, 90)
        .same_arc(Verb::Paint, 90, 90);
    let mut b2 = PictBuilder::new(0, 0, 40, 40);
    b2.arc(Verb::Paint, 4, 4, 36, 36, 0, 90)
        .arc(Verb::Paint, 4, 4, 36, 36, 90, 90);
    let i1 = parse_pict(&b1.finish()).unwrap();
    let i2 = parse_pict(&b2.finish()).unwrap();
    assert_eq!(i1.data, i2.data);
    assert!(ink_count(&i1) > 100, "two wedges should ink a lot");
}

#[test]
fn same_rect_without_a_prior_rect_is_a_no_op() {
    let mut b = PictBuilder::new(0, 0, 20, 20);
    b.same_rect(Verb::Paint); // nothing recorded yet — must not draw
    b.rect(Verb::Paint, 0, 0, 1, 1);
    let img = parse_pict(&b.finish()).unwrap();
    assert_eq!(ink_count(&img), 1, "only the 1×1 anchor paints");
}

// ---------------------------------------------------------------------------
// ClipRgn via the builder masks subsequent drawing.
// ---------------------------------------------------------------------------

#[test]
fn clip_rect_masks_subsequent_paint() {
    let mut b = PictBuilder::new(0, 0, 30, 30);
    b.clip_rect(0, 0, 10, 10);
    b.rect(Verb::Paint, 0, 0, 30, 30);
    let img = parse_pict(&b.finish()).unwrap();
    // Only the 10×10 clip window carries ink.
    assert_eq!(ink_count(&img), 100);
    for y in 0..30u32 {
        for x in 0..30u32 {
            let off = ((y * 30 + x) * 4) as usize;
            let inked = img.data[off] < 240;
            assert_eq!(inked, x < 10 && y < 10, "clip breach at ({x},{y})");
        }
    }
}
