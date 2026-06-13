//! Round 273 — Color QuickDraw arithmetic transfer modes honoured on
//! patterned shape fills.
//!
//! Inside Macintosh: Imaging With QuickDraw §4 ("Color QuickDraw")
//! pages 4-38..4-40 define eight arithmetic transfer modes
//! (`blend = 32`, `addPin = 33`, `addOver = 34`, `subPin = 35`,
//! `transparent = 36`, `addMax = 37`, `subOver = 38`, `adMin = 39`).
//! Round 230 captured the `PnMode` code + `OpColor` into
//! `PictTextState`; round 247 honoured the Boolean pattern modes
//! (`8..=15`). Round 273 wires the arithmetic modes into every
//! patterned shape fill via the same per-cell dispatch path.
//!
//! Each test builds a minimal v2 PICT that:
//! 1. paints a known destination colour over the whole canvas;
//! 2. optionally sets `OpColor` (the per-channel pin / blend weight);
//! 3. sets `PnMode` to one of the arithmetic codes;
//! 4. sets a *solid-foreground* pen pattern (`[0xFF; 8]`) so every cell
//!    sources the foreground colour, then paints a rect with that fg.
//!
//! The expected canvas follows the §4 per-channel formulas verbatim.

use oxideav_pict::ops::{PictBuilder, Verb};
use oxideav_pict::{blend_arith, parse_pict, ArithMode, PatternMode, PictImage, Rgba};

/// Build a 2×2 PICT painting `dst`, then `OpColor`, then `PnMode` =
/// `mode`, a solid-fg pen pattern, and a paint with foreground `src`.
fn build_arith_paint(
    mode: i16,
    op_color: Option<(u8, u8, u8)>,
    dst: (u8, u8, u8),
    src: (u8, u8, u8),
) -> Vec<u8> {
    let mut b = PictBuilder::new(0, 0, 2, 2);
    // Destination wash: patCopy + solid-fg pattern collapses to a solid
    // fill at `dst`.
    b.fg_color(dst.0, dst.1, dst.2)
        .bg_color(dst.0, dst.1, dst.2)
        .pn_mode(8)
        .pen_pattern([0xFF; 8])
        .rect(Verb::Paint, 0, 0, 2, 2);
    if let Some((r, g, bl)) = op_color {
        b.op_color(r, g, bl);
    }
    // The measured paint: arithmetic mode, solid-fg pattern so every
    // cell sources `src`.
    b.fg_color(src.0, src.1, src.2)
        .bg_color(src.0, src.1, src.2)
        .pn_mode(mode)
        .pen_pattern([0xFF; 8])
        .rect(Verb::Paint, 0, 0, 2, 2);
    b.finish()
}

/// Read pixel `(x, y)` as `(r, g, b)`.
fn pix(img: &PictImage, x: u32, y: u32) -> (u8, u8, u8) {
    let off = ((y * img.width + x) * 4) as usize;
    (img.data[off], img.data[off + 1], img.data[off + 2])
}

#[test]
fn add_over_wraps_per_channel() {
    // addOver = 34: dst = (src + dst) mod 256.
    // dst = (200, 100, 50), src = (100, 200, 250).
    // → (300%256, 300%256, 300%256) = (44, 44, 44).
    let bytes = build_arith_paint(34, None, (200, 100, 50), (100, 200, 250));
    let img = parse_pict(&bytes).expect("decode");
    assert_eq!(pix(&img, 0, 0), (44, 44, 44));
    assert_eq!(pix(&img, 1, 1), (44, 44, 44));
}

#[test]
fn add_pin_default_white_is_saturating_add() {
    // addPin = 33 with no OpColor → max pin = white (255): saturating add.
    // dst = (200, 100, 50), src = (100, 200, 250) → (255, 255, 255).
    let bytes = build_arith_paint(33, None, (200, 100, 50), (100, 200, 250));
    let img = parse_pict(&bytes).expect("decode");
    assert_eq!(pix(&img, 0, 0), (255, 255, 255));
}

#[test]
fn add_pin_honours_op_color_max() {
    // addPin = 33 with OpColor = (128, 128, 128) clamps each sum to 128.
    // dst = (40, 40, 40), src = (50, 100, 200) → (90, 128, 128).
    let bytes = build_arith_paint(33, Some((128, 128, 128)), (40, 40, 40), (50, 100, 200));
    let img = parse_pict(&bytes).expect("decode");
    assert_eq!(pix(&img, 0, 0), (90, 128, 128));
}

#[test]
fn sub_pin_default_black_is_saturating_sub() {
    // subPin = 35 with no OpColor → min pin = black (0): saturating sub.
    // dst = (100, 100, 100), src = (50, 150, 100) → (50, 0, 0).
    let bytes = build_arith_paint(35, None, (100, 100, 100), (50, 150, 100));
    let img = parse_pict(&bytes).expect("decode");
    assert_eq!(pix(&img, 0, 0), (50, 0, 0));
}

#[test]
fn sub_pin_honours_op_color_min() {
    // subPin = 35 with OpColor = (60, 60, 60) clamps each difference up
    // to 60. dst = (200, 200, 200), src = (100, 200, 10) → (100, 60, 190).
    let bytes = build_arith_paint(35, Some((60, 60, 60)), (200, 200, 200), (100, 200, 10));
    let img = parse_pict(&bytes).expect("decode");
    assert_eq!(pix(&img, 0, 0), (100, 60, 190));
}

#[test]
fn sub_over_wraps_negative() {
    // subOver = 38: dst = (dst - src) mod 256, negatives wrap up.
    // dst = (50, 100, 200), src = (100, 50, 50) → (-50%256, 50, 150)
    // = (206, 50, 150).
    let bytes = build_arith_paint(38, None, (50, 100, 200), (100, 50, 50));
    let img = parse_pict(&bytes).expect("decode");
    assert_eq!(pix(&img, 0, 0), (206, 50, 150));
}

#[test]
fn add_max_takes_greater_saturation() {
    // addMax = 37: dst = max(src, dst) per channel.
    // dst = (200, 50, 100), src = (100, 150, 100) → (200, 150, 100).
    let bytes = build_arith_paint(37, None, (200, 50, 100), (100, 150, 100));
    let img = parse_pict(&bytes).expect("decode");
    assert_eq!(pix(&img, 0, 0), (200, 150, 100));
}

#[test]
fn ad_min_takes_lesser_saturation() {
    // adMin = 39: dst = min(src, dst) per channel.
    // dst = (200, 50, 100), src = (100, 150, 100) → (100, 50, 100).
    let bytes = build_arith_paint(39, None, (200, 50, 100), (100, 150, 100));
    let img = parse_pict(&bytes).expect("decode");
    assert_eq!(pix(&img, 0, 0), (100, 50, 100));
}

#[test]
fn blend_default_is_fifty_percent() {
    // blend = 32, no OpColor → weight = 50% gray (128).
    // dst = (0, 200, 100), src = (200, 0, 100).
    // per channel: (s*128 + d*127 + 127)/255.
    // r: (200*128 + 0*127 + 127)/255 = 25727/255 = 100.
    // g: (0*128 + 200*127 + 127)/255 = 25527/255 = 100.
    // b: (100*128 + 100*127 + 127)/255 = 25627/255 = 100.
    let bytes = build_arith_paint(32, None, (0, 200, 100), (200, 0, 100));
    let img = parse_pict(&bytes).expect("decode");
    assert_eq!(pix(&img, 0, 0), (100, 100, 100));
}

#[test]
fn blend_honours_op_color_weight() {
    // blend = 32 with OpColor = (255, 0, 0): full source on red, full
    // destination on green/blue.
    // dst = (10, 20, 30), src = (200, 100, 50).
    // r: weight 255 → src = 200. g: weight 0 → dst = 20. b: weight 0 →
    // dst = 30.
    let bytes = build_arith_paint(32, Some((255, 0, 0)), (10, 20, 30), (200, 100, 50));
    let img = parse_pict(&bytes).expect("decode");
    assert_eq!(pix(&img, 0, 0), (200, 20, 30));
}

#[test]
fn transparent_copies_unless_source_equals_background() {
    // transparent = 36: dst = src unless src == background colour.
    // The pen pattern is solid-fg so every cell sources `src`; the
    // background colour is set equal to `src` in build_arith_paint, so
    // the source pixel matches the background key and the destination is
    // left UNCHANGED.
    let bytes = build_arith_paint(36, None, (123, 45, 67), (200, 200, 200));
    let img = parse_pict(&bytes).expect("decode");
    // src == bg key ⇒ hole ⇒ destination unchanged.
    assert_eq!(pix(&img, 0, 0), (123, 45, 67));
}

#[test]
fn transparent_copies_when_source_differs_from_background() {
    // Hand-build so the foreground (source) differs from the background
    // key — then transparent copies the source through.
    let mut b = PictBuilder::new(0, 0, 2, 2);
    // Destination wash = (10, 20, 30).
    b.fg_color(10, 20, 30)
        .bg_color(10, 20, 30)
        .pn_mode(8)
        .pen_pattern([0xFF; 8])
        .rect(Verb::Paint, 0, 0, 2, 2);
    // transparent paint: fg = source = (90, 80, 70), bg key = (5, 5, 5)
    // (differs from source) ⇒ source copies through.
    b.fg_color(90, 80, 70)
        .bg_color(5, 5, 5)
        .pn_mode(36)
        .pen_pattern([0xFF; 8])
        .rect(Verb::Paint, 0, 0, 2, 2);
    let img = parse_pict(&b.finish()).expect("decode");
    assert_eq!(pix(&img, 0, 0), (90, 80, 70));
}

#[test]
fn arith_mode_applies_across_shapes() {
    // The arithmetic dispatch goes through the shared per-cell path, so
    // an oval fill honours addOver identically to a rect. Wash mid-grey,
    // then addOver-paint an oval over the interior.
    let mut b = PictBuilder::new(0, 0, 8, 8);
    b.fg_color(100, 100, 100)
        .bg_color(100, 100, 100)
        .pn_mode(8)
        .pen_pattern([0xFF; 8])
        .rect(Verb::Paint, 0, 0, 8, 8);
    b.fg_color(50, 50, 50)
        .bg_color(50, 50, 50)
        .pn_mode(34) // addOver
        .pen_pattern([0xFF; 8])
        .oval(Verb::Paint, 0, 0, 8, 8);
    let img = parse_pict(&b.finish()).expect("decode");
    // Centre of the oval: (100 + 50) mod 256 = 150.
    assert_eq!(pix(&img, 4, 4), (150, 150, 150));
}

#[test]
fn pattern_mode_resolves_arith_variant() {
    // from_pn_mode_with maps 32..=39 to the Arith variant; the bare
    // from_pn_mode folds them to patCopy.
    let m = PatternMode::from_pn_mode_with(34, None, Rgba::BLACK, None);
    assert!(matches!(
        m,
        PatternMode::Arith {
            mode: ArithMode::AddOver,
            ..
        }
    ));
    assert_eq!(PatternMode::from_pn_mode(34), PatternMode::PatCopy);
    assert_eq!(ArithMode::from_code(40), None);
    assert_eq!(ArithMode::from_code(32), Some(ArithMode::Blend));
}

#[test]
fn blend_arith_pure_function_matches_formula() {
    // Direct check of the pure combiner used by the rasteriser.
    let src = Rgba::new(200, 0, 100, 255);
    let dst = Rgba::new(0, 200, 100, 255);
    // addOver wraps.
    let r = blend_arith(ArithMode::AddOver, src, dst, Rgba::WHITE, Rgba::BLACK);
    assert_eq!((r.r, r.g, r.b), (200, 200, 200));
    // addMax.
    let r = blend_arith(ArithMode::AddMax, src, dst, Rgba::WHITE, Rgba::BLACK);
    assert_eq!((r.r, r.g, r.b), (200, 200, 100));
    // adMin.
    let r = blend_arith(ArithMode::AdMin, src, dst, Rgba::WHITE, Rgba::BLACK);
    assert_eq!((r.r, r.g, r.b), (0, 0, 100));
    // Alpha preserved from destination.
    assert_eq!(r.a, 255);
}
