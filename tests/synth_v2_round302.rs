//! Round 302 — arbitrary power-of-2 PixPat tiles.
//!
//! Inside Macintosh: Imaging With QuickDraw §3 ("QuickDraw Drawing
//! Reference", book page 3-40): *"A pixel pattern … can be of any width
//! and height that's a power of 2."* Round 91 only honoured the
//! universal 8×8 tile; round 302 wires up the arbitrary power-of-2
//! `bounds` the spec permits on the decoder and exposes the matching
//! `build_pix_pat_op_sized` / `PictBuilder::pen_pix_pattern_sized`
//! encoder helpers so the path is round-trip tested.

use oxideav_pict::ops::{PictBuilder, Verb};
use oxideav_pict::{build_pix_pat_op_sized, parse_pict, probe_pict, PictPixelFormat, PixPatSlot};

const RED: [u8; 4] = [0xFF, 0x00, 0x00, 0xFF];
const GREEN: [u8; 4] = [0x00, 0xFF, 0x00, 0xFF];
const BLUE: [u8; 4] = [0x00, 0x00, 0xFF, 0xFF];
const YELLOW: [u8; 4] = [0xFF, 0xFF, 0x00, 0xFF];

/// A 4×2 tile (row 0 red, row 1 green) paints onto a 8×8 canvas so the
/// 4-wide tile wraps twice across and the 2-tall tile wraps four times
/// down. Every even row is red, every odd row green.
#[test]
fn pen_pix_pattern_4x2_tiles() {
    let pixels = vec![RED, RED, RED, RED, GREEN, GREEN, GREEN, GREEN];
    let mut b = PictBuilder::new(0, 0, 8, 8);
    b.pen_pix_pattern_sized([0xFF; 8], 4, 2, &pixels).unwrap();
    b.rect(Verb::Paint, 0, 0, 8, 8);
    let img = parse_pict(&b.finish()).unwrap();

    assert_eq!(img.width, 8);
    assert_eq!(img.height, 8);
    assert_eq!(img.pixel_format, PictPixelFormat::Rgba);

    let px = |x: usize, y: usize| {
        let off = (y * img.width as usize + x) * 4;
        &img.data[off..off + 4]
    };

    // Row 0 (tile row 0) — red across the whole width, including the
    // wrapped columns 4..8.
    for x in 0..8 {
        assert_eq!(px(x, 0), &RED, "row 0 col {x} should be red");
    }
    // Row 1 (tile row 1) — green.
    for x in 0..8 {
        assert_eq!(px(x, 1), &GREEN, "row 1 col {x} should be green");
    }
    // Row 2 wraps back to tile row 0 → red again.
    assert_eq!(px(0, 2), &RED, "row 2 wraps to tile row 0");
    assert_eq!(px(0, 3), &GREEN, "row 3 wraps to tile row 1");
}

/// A 2×4 tile with a distinct colour in each of its 8 cells confirms
/// per-cell colour resolution + wrapping on both axes for a tile whose
/// width and height differ.
#[test]
fn pen_pix_pattern_2x4_per_cell() {
    // Tile (row-major, width 2, height 4):
    //   (R G)
    //   (B Y)
    //   (R G)
    //   (B Y)
    let pixels = vec![RED, GREEN, BLUE, YELLOW, RED, GREEN, BLUE, YELLOW];
    // Canvas 4 wide × 8 tall — PictBuilder::new is (top, left, bottom, right).
    let mut b = PictBuilder::new(0, 0, 8, 4);
    b.pen_pix_pattern_sized([0xFF; 8], 2, 4, &pixels).unwrap();
    b.rect(Verb::Paint, 0, 0, 8, 4);
    let img = parse_pict(&b.finish()).unwrap();
    assert_eq!(img.width, 4);
    assert_eq!(img.height, 8);

    let px = |x: usize, y: usize| {
        let off = (y * img.width as usize + x) * 4;
        &img.data[off..off + 4]
    };

    assert_eq!(px(0, 0), &RED);
    assert_eq!(px(1, 0), &GREEN);
    assert_eq!(px(2, 0), &RED, "col 2 wraps to tile col 0");
    assert_eq!(px(3, 0), &GREEN, "col 3 wraps to tile col 1");
    assert_eq!(px(0, 1), &BLUE);
    assert_eq!(px(1, 1), &YELLOW);
    assert_eq!(px(0, 3), &BLUE, "tile row 3 col 0 is blue");
    assert_eq!(px(1, 3), &YELLOW, "tile row 3 col 1 is yellow");
    assert_eq!(px(0, 4), &RED, "row 4 wraps to tile row 0");
}

/// A 16×16 tile (larger than 8, exercising the rowBytes ≥ 8 PackBits
/// PixData path on encode AND decode) round-trips its corner colours.
#[test]
fn pen_pix_pattern_16x16_packbits_rows() {
    let mut pixels = vec![BLUE; 16 * 16];
    // Stamp the four corners with distinct colours.
    pixels[0] = RED; // top-left
    pixels[15] = GREEN; // top-right
    pixels[15 * 16] = YELLOW; // bottom-left
    let mut b = PictBuilder::new(0, 0, 16, 16);
    b.pen_pix_pattern_sized([0xFF; 8], 16, 16, &pixels).unwrap();
    b.rect(Verb::Paint, 0, 0, 16, 16);
    let img = parse_pict(&b.finish()).unwrap();

    let px = |x: usize, y: usize| {
        let off = (y * img.width as usize + x) * 4;
        &img.data[off..off + 4]
    };
    assert_eq!(px(0, 0), &RED);
    assert_eq!(px(15, 0), &GREEN);
    assert_eq!(px(0, 15), &YELLOW);
    assert_eq!(px(8, 8), &BLUE, "interior is the fill colour");
}

/// Probe surfaces the same arbitrary-size PixPat as a colour pattern
/// without rasterising. (The pix-pattern record is walked identically
/// by the probe and the decoder.)
#[test]
fn probe_walks_non_8x8_pix_pat() {
    let pixels = vec![RED, RED, RED, RED, GREEN, GREEN, GREEN, GREEN];
    let mut b = PictBuilder::new(0, 0, 8, 8);
    b.pen_pix_pattern_sized([0xFF; 8], 4, 2, &pixels).unwrap();
    b.rect(Verb::Paint, 0, 0, 8, 8);
    let bytes = b.finish();

    // Both walkers accept the stream cleanly.
    let _img = parse_pict(&bytes).unwrap();
    let p = probe_pict(&bytes).unwrap();
    assert!(p.end_pic_seen);
    assert_eq!(p.drawing_count, 1, "one Paint rect");
}

/// The encoder rejects dimensions that aren't both powers of two — §3
/// constrains pixel-pattern tiles to power-of-2 width and height.
#[test]
fn encoder_rejects_non_power_of_two() {
    let pixels = vec![RED; 3 * 2];
    let err = build_pix_pat_op_sized(PixPatSlot::Pen, [0xFF; 8], 3, 2, &pixels);
    assert!(err.is_err(), "width 3 is not a power of two");

    let pixels = vec![RED; 4 * 4];
    let err = build_pix_pat_op_sized(PixPatSlot::Pen, [0xFF; 8], 4, 2, &pixels);
    assert!(err.is_err(), "cell count mismatch (4*4 vs 4*2)");
}

/// A 1×1 tile is a degenerate-but-valid power-of-2 case — every cell of
/// the painted region takes the single colour.
#[test]
fn pen_pix_pattern_1x1_solid() {
    let pixels = vec![YELLOW];
    let mut b = PictBuilder::new(0, 0, 4, 4);
    b.pen_pix_pattern_sized([0xFF; 8], 1, 1, &pixels).unwrap();
    b.rect(Verb::Paint, 0, 0, 4, 4);
    let img = parse_pict(&b.finish()).unwrap();
    for y in 0..4 {
        for x in 0..4 {
            let off = (y * 4 + x) * 4;
            assert_eq!(&img.data[off..off + 4], &YELLOW);
        }
    }
}
