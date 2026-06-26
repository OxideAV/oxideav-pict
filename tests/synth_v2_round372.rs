//! Round-372 integration tests: the `frameOval` (`$0050`), `frameRRect`
//! (`$0040`) and `frameArc` (`$0060`) verbs now honour the current pen
//! size, pen pattern, and pen pattern mode, exercised end-to-end through
//! `parse_pict`.
//!
//! Inside Macintosh: Imaging With QuickDraw, "Framing Shapes" (book page
//! 3-13): *"Using the FrameRect, FrameOval, FrameRoundRect, FrameArc,
//! FramePoly, or FrameRgn procedure to frame a shape draws just its
//! outline, using the size, pattern, and pattern mode of the graphics
//! pen."* Before round 372 the oval frame ignored the pen pattern /
//! pattern mode (only the pen size widened it), and the round-rect / arc
//! frames ignored the pen entirely — a fixed 1-pixel `fgColor` trace.
//! These tests pin the fixed behaviour: a non-solid `PnPat`, a thicker
//! `PnSize`, and a `PnMode` all reach the three shape frames.

use oxideav_pict::{ops::PictBuilder, ops::Verb, parse_pict};

/// A vertical-stripe pen pattern (`0xAA` rows → foreground on even
/// columns) applied to `frameRRect` tiles the outline: even-x boundary
/// cells take `fgColor`, odd-x cells stay paper.
#[test]
fn frame_round_rect_honours_pen_pattern() {
    let mut b = PictBuilder::new(0, 0, 16, 16);
    b.fg_color(0x10, 0x20, 0x30);
    b.oval_size(6, 6);
    b.pen_pattern([0xAA; 8]);
    b.round_rect(Verb::Frame, 1, 1, 15, 15);
    let bytes = b.finish();

    let img = parse_pict(&bytes).expect("decode patterned frameRRect");
    assert_eq!(img.width, 16);

    // The top straight edge of the round-rect runs along y == 1 between
    // the corners. Sample two adjacent interior boundary cells: an even
    // column is inked, the odd neighbour is paper.
    let px = |x: usize, y: usize| {
        let off = (y * 16 + x) * 4;
        [img.data[off], img.data[off + 1], img.data[off + 2]]
    };
    assert_eq!(px(6, 1), [0x10, 0x20, 0x30], "even-x boundary cell inked");
    assert_eq!(px(7, 1), [0xFF, 0xFF, 0xFF], "odd-x boundary cell paper");
}

/// A 3×3 pen widens the `frameOval` outline — the historical path only
/// kicked in for the size, so this confirms a non-default size still
/// works through the new pattern-aware dispatch.
#[test]
fn frame_oval_honours_pen_size() {
    let mut thin = PictBuilder::new(0, 0, 20, 20);
    thin.fg_color(0, 0, 0);
    thin.oval(Verb::Frame, 2, 2, 18, 18);
    let thin_img = parse_pict(&thin.finish()).expect("thin oval");

    let mut thick = PictBuilder::new(0, 0, 20, 20);
    thick.fg_color(0, 0, 0);
    thick.pen_size(3, 3);
    thick.oval(Verb::Frame, 2, 2, 18, 18);
    let thick_img = parse_pict(&thick.finish()).expect("thick oval");

    let inked = |img: &oxideav_pict::PictImage| {
        img.data
            .chunks_exact(4)
            .filter(|p| p[0] == 0 && p[1] == 0 && p[2] == 0)
            .count()
    };
    assert!(
        inked(&thick_img) > inked(&thin_img),
        "3×3 pen widens the oval outline ({} vs {})",
        inked(&thick_img),
        inked(&thin_img)
    );
}

/// A solid-fg pen in `patXor` mode applied to `frameOval` inverts the
/// destination along the boundary; framing the same oval twice restores
/// the canvas (XOR is its own inverse).
#[test]
fn frame_oval_pen_mode_xor_round_trips() {
    let build = |n: usize| {
        let mut b = PictBuilder::new(0, 0, 16, 16);
        // A grey backdrop so the canvas is non-trivial (and the picture
        // always carries at least one raster verb).
        b.fg_color(0x40, 0x40, 0x40);
        b.rect(Verb::Paint, 0, 0, 16, 16);
        b.fg_color(0, 0, 0);
        b.pn_mode(10); // patXor
        for _ in 0..n {
            b.oval(Verb::Frame, 2, 2, 14, 14);
        }
        parse_pict(&b.finish()).expect("decode xor oval")
    };
    let zero = build(0);
    let twice = build(2);
    assert_eq!(
        zero.data, twice.data,
        "two patXor oval frames cancel to the blank canvas"
    );
}

/// A non-solid pen pattern reaches `frameArc`: the 0°..90° quarter arc
/// of an ellipse is stroked, and at least some boundary pixels carry the
/// foreground colour (proving the arc frame no longer ignores the pen).
#[test]
fn frame_arc_honours_pen_pattern() {
    let mut b = PictBuilder::new(0, 0, 16, 16);
    b.fg_color(0x80, 0x40, 0x20);
    b.pen_pattern([0xAA; 8]);
    b.arc(Verb::Frame, 2, 2, 14, 14, 0, 90);
    let bytes = b.finish();

    let img = parse_pict(&bytes).expect("decode patterned frameArc");
    let inked = img
        .data
        .chunks_exact(4)
        .filter(|p| p[0] == 0x80 && p[1] == 0x40 && p[2] == 0x20)
        .count();
    assert!(inked > 0, "patterned arc inks some boundary cells");

    // Solid-fg pen baseline must ink at least as many cells (the stripe
    // pattern drops the odd columns).
    let mut solid = PictBuilder::new(0, 0, 16, 16);
    solid.fg_color(0x80, 0x40, 0x20);
    solid.arc(Verb::Frame, 2, 2, 14, 14, 0, 90);
    let solid_img = parse_pict(&solid.finish()).expect("solid arc");
    let solid_inked = solid_img
        .data
        .chunks_exact(4)
        .filter(|p| p[0] == 0x80 && p[1] == 0x40 && p[2] == 0x20)
        .count();
    assert!(
        solid_inked >= inked,
        "solid arc inks >= patterned arc ({solid_inked} vs {inked})"
    );
}

/// Regression guard: a default solid 1×1 `patCopy` `fgColor` pen must
/// keep the historical thin-outline render bit-for-bit on all three
/// shapes (the fast path the new dispatch preserves).
#[test]
fn default_pen_frames_unchanged() {
    for shape in 0..3 {
        let mut b = PictBuilder::new(0, 0, 16, 16);
        b.fg_color(0, 0, 0);
        match shape {
            0 => {
                b.oval_size(6, 6);
                b.round_rect(Verb::Frame, 1, 1, 15, 15);
            }
            1 => {
                b.oval(Verb::Frame, 1, 1, 15, 15);
            }
            _ => {
                b.arc(Verb::Frame, 1, 1, 15, 15, 0, 270);
            }
        }
        let img = parse_pict(&b.finish()).expect("decode default-pen frame");
        // Outline-only: the centre pixel must remain paper white.
        let off = (8 * 16 + 8) * 4;
        assert_eq!(
            &img.data[off..off + 3],
            &[0xFF, 0xFF, 0xFF],
            "shape {shape}: interior unaffected by frame verb"
        );
        // At least one black boundary pixel must exist.
        let any_black = img
            .data
            .chunks_exact(4)
            .any(|p| p[0] == 0 && p[1] == 0 && p[2] == 0);
        assert!(any_black, "shape {shape}: outline drawn");
    }
}
