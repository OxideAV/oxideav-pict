//! Round-366 integration tests for the pen-aware `FrameRgn` outline.
//!
//! Inside Macintosh: Imaging With QuickDraw, "Framing Shapes" (book page
//! 3-13): *"Using the … FrameRgn procedure to frame a shape draws just
//! its outline, using the size, pattern, and pattern mode of the
//! graphics pen."* Earlier rounds drew a fixed 1-pixel `fgColor` trace,
//! ignoring `PnSize` / `PnPat` / `PnMode` — the same gap `FramePoly`
//! closed in round 333. These tests confirm the region outline now
//! honours the pen size (which hangs below and to the right), the pen
//! pattern, and leaves the region interior untouched.

use oxideav_pict::{ops::PictBuilder, ops::Verb, parse_pict};

/// A thin (1×1) pen frames a rectangular region with a 1-pixel border;
/// the interior shows through (paper).
#[test]
fn frame_rect_region_thin_pen_is_one_pixel_border() {
    let mut b = PictBuilder::new(0, 0, 8, 8);
    b.fg_color(0x20, 0x40, 0x80);
    b.region_rect(Verb::Frame, 1, 1, 7, 7);
    let bytes = b.finish();
    let img = parse_pict(&bytes).expect("decode frameRgn");

    let px = |x: usize, y: usize| {
        let off = (y * 8 + x) * 4;
        (img.data[off], img.data[off + 1], img.data[off + 2])
    };
    // Border pixels (region bbox [1,1,7,7] → outline at row/col 1 and 6).
    assert_eq!(px(1, 1), (0x20, 0x40, 0x80), "top-left corner inked");
    assert_eq!(px(6, 6), (0x20, 0x40, 0x80), "bottom-right corner inked");
    assert_eq!(px(3, 1), (0x20, 0x40, 0x80), "top edge inked");
    // Interior shows through (paper white).
    assert_eq!(px(3, 3), (0xFF, 0xFF, 0xFF), "interior is paper");
    // Outside the region untouched.
    assert_eq!(px(0, 0), (0xFF, 0xFF, 0xFF), "outside is paper");
}

/// A thick (2×2) pen widens the outline; the pen hangs below and to the
/// right, so the inked border is two pixels deep on the top/left edges.
#[test]
fn frame_rect_region_thick_pen_hangs_below_and_right() {
    let mut b = PictBuilder::new(0, 0, 10, 10);
    b.fg_color(0xFF, 0x00, 0x00);
    b.pen_size(2, 2);
    b.region_rect(Verb::Frame, 2, 2, 8, 8);
    let bytes = b.finish();
    let img = parse_pict(&bytes).expect("decode thick frameRgn");

    let is_red = |x: usize, y: usize| {
        let off = (y * 10 + x) * 4;
        img.data[off] == 0xFF && img.data[off + 1] == 0x00 && img.data[off + 2] == 0x00
    };
    // Top edge: rows 2 and 3 are inked (2-pixel pen hanging down).
    assert!(is_red(3, 2), "top edge row 2 inked");
    assert!(is_red(3, 3), "top edge row 3 inked (pen hangs down)");
    // Left edge: cols 2 and 3 inked.
    assert!(is_red(2, 4), "left edge col 2 inked");
    assert!(is_red(3, 4), "left edge col 3 inked (pen hangs right)");
    // Deep interior is paper.
    let off = (5 * 10 + 5) * 4;
    assert_eq!(img.data[off], 0xFF, "interior R paper");
    assert_eq!(img.data[off + 1], 0xFF, "interior G paper");
}

/// A non-rectangular (masked) region's outline is pen-stamped at the
/// boundary cells; the interior shows through.
#[test]
fn frame_masked_region_outlines_boundary_only() {
    let mut b = PictBuilder::new(0, 0, 8, 8);
    b.fg_color(0x11, 0x22, 0x33);
    // Region: rows 1..7 cover columns [1, 7) — a 6×6 square block, but
    // encoded as a masked region (carries inversion data).
    let scanlines = [
        (1i16, [1i16, 7i16].as_slice()),
        (7i16, [1i16, 7i16].as_slice()),
    ];
    b.region(Verb::Frame, 0, 0, 8, 8, &scanlines)
        .expect("encode masked region frame");
    let bytes = b.finish();
    let img = parse_pict(&bytes).expect("decode masked frameRgn");

    let inked = |x: usize, y: usize| {
        let off = (y * 8 + x) * 4;
        img.data[off] == 0x11 && img.data[off + 1] == 0x22 && img.data[off + 2] == 0x33
    };
    // Boundary cells (perimeter of the [1,7)×[1,7) block) are inked.
    assert!(inked(1, 1), "boundary corner");
    assert!(inked(6, 6), "boundary opposite corner");
    assert!(inked(3, 1), "boundary top edge");
    assert!(inked(1, 4), "boundary left edge");
    // Interior cell (not on the boundary) shows through as paper.
    let off = (4 * 8 + 4) * 4;
    assert_eq!(img.data[off], 0xFF, "interior paper R");
    // Truly outside the region is paper too.
    let off = 0;
    assert_eq!(img.data[off], 0xFF, "outside paper R");
}

/// The pen pattern is honoured: an all-off pen pattern paints nothing on
/// the outline (every cell selects the background), confirming `PnPat`
/// reaches the region-frame path.
#[test]
fn frame_region_honours_all_off_pen_pattern() {
    let mut b = PictBuilder::new(0, 0, 8, 8);
    b.fg_color(0x00, 0x00, 0x00);
    // All-off pattern: every cell selects bg (paper white default).
    b.pen_pattern([0x00; 8]);
    b.region_rect(Verb::Frame, 1, 1, 7, 7);
    let bytes = b.finish();
    let img = parse_pict(&bytes).expect("decode patterned frameRgn");

    // The outline must NOT be black — an all-off pattern leaves the
    // border background (paper) since on-bits select fg and there are
    // none. (This is the round-8 monochrome pattern convention.)
    let off = (8 + 3) * 4; // a top-edge border cell at row 1, col 3
    assert_eq!(img.data[off], 0xFF, "all-off pen pattern leaves paper R");
    assert_eq!(
        img.data[off + 1],
        0xFF,
        "all-off pen pattern leaves paper G"
    );
    assert_eq!(
        img.data[off + 2],
        0xFF,
        "all-off pen pattern leaves paper B"
    );
}
