//! Round 252 — `Invert` verbs on round-rect / oval / arc / polygon.
//!
//! Inside Macintosh: Imaging With QuickDraw §3 ("QuickDraw Drawing
//! Reference") and §A-3 Table A-2 define the five `Invert*` verbs:
//! `InvertRect` (`$0033`), `InvertRRect` (`$0043`), `InvertOval`
//! (`$0053`), `InvertArc` (`$0063`), `InvertPoly` (`$0073`) — plus
//! their Same-shape companions (`$003B`, `$004B`, `$005B`, `$006B`,
//! `$007B`). Per §3, the invert verb is *"to invert the destination
//! pixel"*: on a 1-bit display the literal Boolean NOT; on the true-
//! colour pipeline of round 2 this crate has settled on channel-wise
//! NOT (alpha preserved) — matching the round-2 `invert_rect` helper
//! that handles the rectangle verb.
//!
//! Pre-r252 the rounded-rect / oval / arc / polygon dispatcher routed
//! verb 3 through the *frame* helper rather than inverting the shape's
//! interior. This file pins the round-252 spec-correct shape:
//!
//! 1. paint a known destination colour over the canvas;
//! 2. emit the `Invert*` opcode covering the shape's bounding box;
//! 3. assert the pixel under the shape's geometric centre is the
//!    channel-wise NOT of the destination colour, and that the corners
//!    *outside* the shape's interior are still the destination colour.
//!
//! A second pass (invert twice) should restore the canvas pixel-for-
//! pixel — the §3 self-inverse contract.

use oxideav_pict::ops::{PictBuilder, Verb};
use oxideav_pict::{parse_pict, PictImage};

/// Read pixel `(x, y)` as `(r, g, b)`.
fn pix(img: &PictImage, x: u32, y: u32) -> (u8, u8, u8) {
    let off = ((y * img.width + x) * 4) as usize;
    (img.data[off], img.data[off + 1], img.data[off + 2])
}

/// Channel-wise NOT of an RGB triple.
fn not_rgb((r, g, b): (u8, u8, u8)) -> (u8, u8, u8) {
    (!r, !g, !b)
}

const DST: (u8, u8, u8) = (0x10, 0x40, 0x80); // arbitrary non-paper colour

/// Paint the full canvas with `DST` so we have a known destination
/// for the invert verb to read from.
fn prime_canvas(b: &mut PictBuilder, w: i16, h: i16) {
    b.fg_color(DST.0, DST.1, DST.2)
        .bg_color(DST.0, DST.1, DST.2)
        .pn_mode(8) // patCopy — solid colour wash
        .pen_pattern([0xFF; 8])
        .rect(Verb::Paint, 0, 0, h, w);
}

#[test]
fn invert_oval_toggles_interior() {
    // 20×20 canvas. Oval inscribed in (2, 2, 18, 18); centre at (10, 10).
    let mut b = PictBuilder::new(0, 0, 20, 20);
    prime_canvas(&mut b, 20, 20);
    b.oval(Verb::Invert, 2, 2, 18, 18);
    let img = parse_pict(&b.finish()).expect("decode");
    // Centre of the ellipse — well inside the filled interior.
    assert_eq!(pix(&img, 10, 10), not_rgb(DST));
    // Bounding-box corner — outside the ellipse (oval doesn't fill the
    // square corners). Should still be DST.
    assert_eq!(pix(&img, 2, 2), DST);
}

#[test]
fn invert_oval_self_inverse() {
    // Two `InvertOval` opcodes back-to-back must restore the canvas.
    let mut b = PictBuilder::new(0, 0, 20, 20);
    prime_canvas(&mut b, 20, 20);
    b.oval(Verb::Invert, 2, 2, 18, 18);
    b.oval(Verb::Invert, 2, 2, 18, 18);
    let img = parse_pict(&b.finish()).expect("decode");
    // Every pixel — interior + corner — should be the original DST.
    assert_eq!(pix(&img, 10, 10), DST);
    assert_eq!(pix(&img, 2, 2), DST);
    assert_eq!(pix(&img, 5, 5), DST);
}

#[test]
fn invert_round_rect_toggles_interior() {
    // 20×20 canvas, oval corner = (6, 6). InvertRRect (2, 2, 18, 18).
    let mut b = PictBuilder::new(0, 0, 20, 20);
    prime_canvas(&mut b, 20, 20);
    b.oval_size(6, 6);
    b.round_rect(Verb::Invert, 2, 2, 18, 18);
    let img = parse_pict(&b.finish()).expect("decode");
    // Centre — interior of the round-rect.
    assert_eq!(pix(&img, 10, 10), not_rgb(DST));
    // Mid-edge — also interior, since rounded corners affect only
    // the four extremes.
    assert_eq!(pix(&img, 10, 3), not_rgb(DST));
    // Outside the round-rect (above the top edge).
    assert_eq!(pix(&img, 10, 0), DST);
}

#[test]
fn invert_round_rect_self_inverse() {
    let mut b = PictBuilder::new(0, 0, 20, 20);
    prime_canvas(&mut b, 20, 20);
    b.oval_size(6, 6);
    b.round_rect(Verb::Invert, 2, 2, 18, 18);
    b.round_rect(Verb::Invert, 2, 2, 18, 18);
    let img = parse_pict(&b.finish()).expect("decode");
    assert_eq!(pix(&img, 10, 10), DST);
    assert_eq!(pix(&img, 10, 3), DST);
    assert_eq!(pix(&img, 0, 0), DST);
}

#[test]
fn invert_arc_toggles_wedge() {
    // 20×20 canvas. Full-circle arc (start 0, sweep 360) should match
    // the InvertOval shape over the same bounding box.
    let mut b = PictBuilder::new(0, 0, 20, 20);
    prime_canvas(&mut b, 20, 20);
    b.arc(Verb::Invert, 2, 2, 18, 18, 0, 360);
    let img = parse_pict(&b.finish()).expect("decode");
    // Centre of the wedge — interior.
    assert_eq!(pix(&img, 10, 10), not_rgb(DST));
    // Corner — outside the ellipse.
    assert_eq!(pix(&img, 2, 2), DST);
}

#[test]
fn invert_arc_quarter_wedge() {
    // 20×20 canvas. Quarter-arc 0..90 = the north-east quadrant of the
    // ellipse (QuickDraw: 0° = 12 o'clock, +90° = 3 o'clock).
    let mut b = PictBuilder::new(0, 0, 20, 20);
    prime_canvas(&mut b, 20, 20);
    b.arc(Verb::Invert, 2, 2, 18, 18, 0, 90);
    let img = parse_pict(&b.finish()).expect("decode");
    // Inside the NE wedge — slightly above-right of centre.
    assert_eq!(pix(&img, 13, 7), not_rgb(DST));
    // Outside the wedge (SW quadrant).
    assert_eq!(pix(&img, 6, 14), DST);
}

#[test]
fn invert_polygon_toggles_interior() {
    // Triangle (5, 5), (15, 5), (10, 15).
    let mut b = PictBuilder::new(0, 0, 20, 20);
    prime_canvas(&mut b, 20, 20);
    let verts = [(5i16, 5i16), (15, 5), (10, 15)];
    b.poly(Verb::Invert, &verts).expect("poly");
    let img = parse_pict(&b.finish()).expect("decode");
    // (10, 8) sits squarely inside the triangle.
    assert_eq!(pix(&img, 10, 8), not_rgb(DST));
    // Outside the triangle (top-left).
    assert_eq!(pix(&img, 0, 0), DST);
}

#[test]
fn invert_polygon_self_inverse() {
    let mut b = PictBuilder::new(0, 0, 20, 20);
    prime_canvas(&mut b, 20, 20);
    let verts = [(5i16, 5i16), (15, 5), (10, 15)];
    b.poly(Verb::Invert, &verts).expect("poly");
    b.poly(Verb::Invert, &verts).expect("poly");
    let img = parse_pict(&b.finish()).expect("decode");
    assert_eq!(pix(&img, 10, 8), DST);
    assert_eq!(pix(&img, 0, 0), DST);
}

#[test]
fn invert_same_oval_uses_last_geometry() {
    // §A-3 Same-shape opcode `$005B` (`InvertSameOval`) reuses the
    // most-recent oval geometry from `state.last_oval`. Round 252 routes
    // it through `apply_oval_verb` with `opcode - 8`, so it picks up the
    // new InvertOval behaviour.
    let mut b = PictBuilder::new(0, 0, 20, 20);
    prime_canvas(&mut b, 20, 20);
    // Paint a yellow oval (2, 2, 18, 18); centre (10, 10) becomes
    // yellow. Then `$005B` on the same geometry should invert the
    // centre to ~blue (NOT yellow).
    const YELLOW: (u8, u8, u8) = (0xFF, 0xFF, 0x00);
    b.fg_color(YELLOW.0, YELLOW.1, YELLOW.2)
        .pen_pattern([0xFF; 8])
        .pn_mode(8) // patCopy
        .oval(Verb::Paint, 2, 2, 18, 18);
    // Raw 2-byte InvertSameOval opcode (`$005B`). PictBuilder's
    // `push` handles word-alignment for us.
    b.push(&[0x00, 0x5B]);
    let img = parse_pict(&b.finish()).expect("decode");
    let p = pix(&img, 10, 10);
    assert_eq!(p, not_rgb(YELLOW));
}
