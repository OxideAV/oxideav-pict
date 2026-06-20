//! Round 352 — the QuickDraw text-drawing opcodes now rasterise glyphs.
//!
//! Through round 295 the four text opcodes (`LongText $0028`,
//! `DHText $0029`, `DVText $002A`, `DHDVText $002B`) only tracked the
//! text pen; the glyph bytes were skipped. Round 352 draws them through
//! the crate's built-in clean-room ASCII face ([`oxideav_pict::font`]),
//! placing the baseline at the pen (Imaging With QuickDraw book page
//! 2-13), scaling by `txSize` (book page 2-34), inking in `fgColor`, and
//! advancing the pen by each glyph's width.
//!
//! These tests drive the full `parse_pict` path and inspect the rendered
//! RGBA canvas, asserting that visible ink lands inside the expected
//! glyph box and that whitespace / out-of-text regions stay paper-white.

use oxideav_pict::font::{measure_text, ADVANCE, GLYPH_H};
use oxideav_pict::ops::{PictBuilder, Verb};
use oxideav_pict::{build_rgb_fg_col, build_tx_size, parse_pict, PictImage};

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

/// True if pixel `(x, y)` in `img` is (near-)black ink.
fn is_ink(img: &PictImage, x: u32, y: u32) -> bool {
    if x >= img.width || y >= img.height {
        return false;
    }
    let off = ((y * img.width + x) * 4) as usize;
    let (r, g, b) = (img.data[off], img.data[off + 1], img.data[off + 2]);
    r < 40 && g < 40 && b < 40
}

/// True if pixel `(x, y)` is (near-)white paper.
fn is_paper(img: &PictImage, x: u32, y: u32) -> bool {
    let off = ((y * img.width + x) * 4) as usize;
    let (r, g, b) = (img.data[off], img.data[off + 1], img.data[off + 2]);
    r > 215 && g > 215 && b > 215
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
// LongText paints ink at the declared baseline; default fg is black.
// ---------------------------------------------------------------------------

#[test]
fn long_text_paints_ink_at_baseline() {
    let mut b = PictBuilder::new(0, 0, 80, 32);
    // Native-scale glyphs: txSize == the design em (8).
    b.push(&build_tx_size(8));
    // Draw "HI" with the baseline at (h=6, v=14).
    b.push(&long_text(14, 6, b"HI"));
    let img = parse_pict(&b.finish()).unwrap();

    // Ink must appear in the glyph box: x in [6, 6 + advance("HI")),
    // y in (baseline - cell_height, baseline].
    let adv = measure_text(b"HI", 8, 0, 0) as u32;
    let ink = count_ink(&img, 6, (14 - GLYPH_H) as u32, 6 + adv, 15);
    assert!(ink >= 10, "expected legible ink, got {ink} pixels");

    // The pen ends after the drawn string.
    assert_eq!(img.text_state.text_pen, Some((6 + adv as i32, 14)));
}

// ---------------------------------------------------------------------------
// Text leaves the rest of the canvas as paper-white.
// ---------------------------------------------------------------------------

#[test]
fn text_does_not_smear_the_canvas() {
    let mut b = PictBuilder::new(0, 0, 80, 32);
    b.push(&build_tx_size(8));
    b.push(&long_text(14, 6, b"Hi"));
    let img = parse_pict(&b.finish()).unwrap();

    // A region well clear of the text (bottom-right corner) stays white.
    for y in 24..32 {
        for x in 60..80 {
            assert!(is_paper(&img, x, y), "stray ink at ({x},{y})");
        }
    }
    // The top-left corner, clear of the text, is paper too.
    assert!(is_paper(&img, 0, 0));
}

// ---------------------------------------------------------------------------
// txSize scales the glyphs: a bigger size paints more ink for one glyph.
// ---------------------------------------------------------------------------

#[test]
fn larger_tx_size_paints_more_ink() {
    let small = {
        let mut b = PictBuilder::new(0, 0, 80, 64);
        b.push(&build_tx_size(8));
        b.push(&long_text(40, 4, b"M"));
        parse_pict(&b.finish()).unwrap()
    };
    let large = {
        let mut b = PictBuilder::new(0, 0, 80, 64);
        b.push(&build_tx_size(24)); // 3× the design em
        b.push(&long_text(40, 4, b"M"));
        parse_pict(&b.finish()).unwrap()
    };
    let small_ink = count_ink(&small, 0, 0, 80, 64);
    let large_ink = count_ink(&large, 0, 0, 80, 64);
    assert!(
        large_ink > small_ink * 3,
        "3× size should paint roughly 9× ink: small={small_ink} large={large_ink}"
    );
}

// ---------------------------------------------------------------------------
// fgColor is the ink: a red foreground paints red glyphs.
// ---------------------------------------------------------------------------

#[test]
fn fg_color_inks_the_glyphs() {
    let mut b = PictBuilder::new(0, 0, 80, 32);
    b.push(&build_tx_size(8));
    b.push(&build_rgb_fg_col(0xFF, 0x00, 0x00)); // red ink
    b.push(&long_text(14, 6, b"X"));
    let img = parse_pict(&b.finish()).unwrap();

    // Find a red pixel in the glyph box.
    let mut red = false;
    for y in (14 - GLYPH_H) as u32..15 {
        for x in 6..6 + ADVANCE as u32 {
            let off = ((y * img.width + x) * 4) as usize;
            if img.data[off] > 200 && img.data[off + 1] < 60 && img.data[off + 2] < 60 {
                red = true;
            }
        }
    }
    assert!(red, "fgColor red did not ink the glyph");
    // And no black ink — every painted glyph pixel is red.
    assert_eq!(count_ink(&img, 0, 0, 80, 32), 0);
}

// ---------------------------------------------------------------------------
// Successive DHText opcodes on one line lay glyphs left-to-right.
// ---------------------------------------------------------------------------

#[test]
fn successive_dh_text_lays_a_line() {
    let mut b = PictBuilder::new(0, 0, 120, 32);
    b.push(&build_tx_size(8));
    b.push(&long_text(16, 4, b"A")); // first glyph at x≈4
    b.push(&dh_text(20, b"B")); //      next glyph 20px further right
    let img = parse_pict(&b.finish()).unwrap();

    // Ink in the first glyph's column band …
    let left = count_ink(&img, 4, 8, 4 + ADVANCE as u32, 17);
    // … and ink in the second glyph's band (shifted right by adv+20).
    let second_x = 4 + ADVANCE + 20;
    let right = count_ink(&img, second_x as u32, 8, (second_x + ADVANCE) as u32, 17);
    assert!(left >= 5, "first glyph missing ink ({left})");
    assert!(right >= 5, "second glyph missing ink ({right})");
}

// ---------------------------------------------------------------------------
// A picture-origin shift moves where text lands on the canvas.
// ---------------------------------------------------------------------------

#[test]
fn empty_text_paints_nothing_but_advances_pen() {
    let mut b = PictBuilder::new(0, 0, 40, 20);
    // A visible non-text raster so parse_pict doesn't return NoRaster.
    b.rect(Verb::Paint, 0, 0, 1, 1);
    b.push(&build_tx_size(8));
    b.push(&long_text(10, 5, b"")); // count = 0
    let img = parse_pict(&b.finish()).unwrap();
    // Empty string: pen stays exactly at the declared origin.
    assert_eq!(img.text_state.text_pen, Some((5, 10)));
    // Only the 1×1 paint-dot is ink; the empty text drew nothing.
    assert_eq!(count_ink(&img, 1, 0, 40, 20), 0);
}
