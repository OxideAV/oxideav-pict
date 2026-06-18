//! Round 333 — `FramePoly` honours the current pen size + pen pattern.
//!
//! Inside Macintosh: Imaging With QuickDraw, "QuickDraw Drawing
//! Reference" (book page 3-81, `FramePoly` procedure):
//!
//! > Using the current graphics port's pen pattern, pattern mode, and
//! > size, the FramePoly procedure plays back the line-drawing commands
//! > that define the polygon… The graphics pen hangs below and to the
//! > right of each point on the boundary of the polygon. Thus, the drawn
//! > polygon extends beyond the right and bottom edges of the polygon's
//! > bounding rectangle … by the pen width and pen height, respectively.
//!
//! Pre-r333 the polygon `Frame` verb drew a fixed 1-pixel Bresenham
//! outline at the foreground colour, ignoring `PnSize` (`$0007`) and the
//! pen pattern (`PnPat` `$0009`) — every other frame verb (rect / round-
//! rect / oval) already honoured the pen size. This file pins the
//! round-333 behaviour:
//!
//! 1. a 1×1 pen frames a 1-pixel-thick boundary (unchanged baseline);
//! 2. a 3×3 pen frames a 3-pixel-thick boundary that hangs below + right
//!    of each boundary point (the spec's pen-hang rule);
//! 3. a non-solid `PnPat` outline stipples foreground / background.

use oxideav_pict::ops::{PictBuilder, Verb};
use oxideav_pict::{parse_pict, PictImage};

/// Read pixel `(x, y)` as `(r, g, b)`.
fn pix(img: &PictImage, x: u32, y: u32) -> (u8, u8, u8) {
    let off = ((y * img.width + x) * 4) as usize;
    (img.data[off], img.data[off + 1], img.data[off + 2])
}

const INK: (u8, u8, u8) = (0xE0, 0x20, 0x40);
const PAPER: (u8, u8, u8) = (0xF0, 0xF0, 0xF0);

/// Prime the whole canvas with `PAPER` so undrawn pixels are known.
fn prime(b: &mut PictBuilder, w: i16, h: i16) {
    b.fg_color(PAPER.0, PAPER.1, PAPER.2)
        .bg_color(PAPER.0, PAPER.1, PAPER.2)
        .pn_mode(8)
        .pen_pattern([0xFF; 8])
        .rect(Verb::Paint, 0, 0, h, w);
}

/// A square polygon (4 corners) framed with a 1×1 pen draws a 1-pixel
/// outline; the interior and the row just inside the top edge stay
/// PAPER.
#[test]
fn frame_poly_thin_pen_is_one_pixel() {
    let mut b = PictBuilder::new(0, 0, 24, 24);
    prime(&mut b, 24, 24);
    b.fg_color(INK.0, INK.1, INK.2)
        .pen_size(1, 1)
        .pen_pattern([0xFF; 8])
        .poly(Verb::Frame, &[(4, 4), (18, 4), (18, 18), (4, 18)])
        .expect("frame poly");
    let img = parse_pict(&b.finish()).expect("decode");
    // Top edge (v == 4) is INK.
    assert_eq!(pix(&img, 8, 4), INK, "top edge should be inked");
    // One row below the top edge is still PAPER (thin pen, no hang).
    assert_eq!(pix(&img, 8, 5), PAPER, "interior just below thin top edge");
    // Interior centre is untouched.
    assert_eq!(pix(&img, 11, 11), PAPER, "interior not painted by Frame");
}

/// The same square framed with a 3×3 pen draws a 3-pixel-thick boundary
/// that hangs *below and to the right* of each boundary point (book page
/// 3-81). The top edge therefore covers rows v = 4, 5, 6.
#[test]
fn frame_poly_thick_pen_hangs_below_and_right() {
    let mut b = PictBuilder::new(0, 0, 24, 24);
    prime(&mut b, 24, 24);
    b.fg_color(INK.0, INK.1, INK.2)
        .pen_size(3, 3)
        .pen_pattern([0xFF; 8])
        .poly(Verb::Frame, &[(4, 4), (18, 4), (18, 18), (4, 18)])
        .expect("frame poly");
    let img = parse_pict(&b.finish()).expect("decode");
    // Top edge band: the boundary point is at v = 4; the pen hangs down
    // so rows 4, 5, 6 are inked above the interior.
    assert_eq!(pix(&img, 9, 4), INK, "top edge row 0");
    assert_eq!(pix(&img, 9, 5), INK, "top edge row 1 (pen hang)");
    assert_eq!(pix(&img, 9, 6), INK, "top edge row 2 (pen hang)");
    // Row 7 — past the 3-pixel band — is the untouched interior.
    assert_eq!(pix(&img, 9, 7), PAPER, "interior below the 3px band");
    // Left edge band hangs to the right: cols 4, 5, 6 at an interior row.
    assert_eq!(pix(&img, 4, 11), INK, "left edge col 0");
    assert_eq!(pix(&img, 5, 11), INK, "left edge col 1 (pen hang)");
    assert_eq!(pix(&img, 6, 11), INK, "left edge col 2 (pen hang)");
    assert_eq!(pix(&img, 7, 11), PAPER, "interior right of the 3px band");
}

/// A non-solid `PnPat` (50 % checker) framing a polygon stipples the
/// outline between foreground (INK) and background (PAPER) instead of a
/// solid INK line.
#[test]
fn frame_poly_honours_pen_pattern() {
    let mut b = PictBuilder::new(0, 0, 24, 24);
    prime(&mut b, 24, 24);
    // 0xAA = 1010_1010 alternating; with a 1×1 pen the top edge toggles
    // INK / PAPER cell-by-cell along x.
    b.fg_color(INK.0, INK.1, INK.2)
        .bg_color(PAPER.0, PAPER.1, PAPER.2)
        .pn_mode(8)
        .pen_size(1, 1)
        .pen_pattern([0xAA; 8])
        .poly(Verb::Frame, &[(4, 4), (18, 4), (18, 18), (4, 18)])
        .expect("frame poly");
    let img = parse_pict(&b.finish()).expect("decode");
    // Along the top edge at v = 4, the pattern row 0xAA selects fg at
    // even x (bit 7,5,3,1 set) and bg at odd x. Find at least one of
    // each so we know the stipple is live (not a solid line).
    let mut saw_ink = false;
    let mut saw_paper = false;
    for x in 4..18u32 {
        match pix(&img, x, 4) {
            INK => saw_ink = true,
            PAPER => saw_paper = true,
            _ => {}
        }
    }
    assert!(saw_ink, "patterned outline should show foreground cells");
    assert!(saw_paper, "patterned outline should show background cells");
}
