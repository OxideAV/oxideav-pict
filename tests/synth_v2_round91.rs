//! Round 91 tests — PixPat (multi-colour 8×8 pixel pattern) opcodes.
//!
//! Inside Macintosh: Imaging With QuickDraw §A-3 Listing A-1 — the
//! `BkPixPat 0x0012`, `PnPixPat 0x0013` and `FillPixPat 0x0014`
//! opcodes each carry a `PixPat` record. The `patType=1` colour-pixmap
//! sub-type carries an 8×8 RGBA tile that the decoder folds onto the
//! rasteriser; the `patType=2` dither sub-type is parsed-and-skipped
//! (round 91 falls back to the `Pat1Data` 8-byte monochrome field for
//! the dither case — see crate README for the follow-up).
//!
//! These tests build PixPat PICTs via [`PictBuilder::pen_pix_pattern`]
//! / `bg_pix_pattern` / `fill_pix_pattern` and verify the round-trip
//! decode lands the right RGBA grid into the canvas.

use oxideav_pict::ops::{PictBuilder, Verb};
use oxideav_pict::{
    build_pix_pat_op, parse_pict, probe_pict, PictImage, PixPatSlot, ProbeTermination,
};

fn at(img: &PictImage, x: u32, y: u32) -> [u8; 4] {
    let off = ((y * img.width + x) * 4) as usize;
    [
        img.data[off],
        img.data[off + 1],
        img.data[off + 2],
        img.data[off + 3],
    ]
}

/// Tile that paints horizontal red / green stripes — even rows red,
/// odd rows green. Useful because the result is independent of the
/// active fg / bg state.
fn rg_stripe_tile() -> [[u8; 4]; 64] {
    let mut tile = [[0u8; 4]; 64];
    for y in 0..8 {
        for x in 0..8 {
            let cell = if y % 2 == 0 {
                [0xFF, 0x00, 0x00, 0xFF]
            } else {
                [0x00, 0xFF, 0x00, 0xFF]
            };
            tile[y * 8 + x] = cell;
        }
    }
    tile
}

#[test]
fn pen_pix_pattern_paint_rect_applies_colour_tile() {
    let tile = rg_stripe_tile();
    let mut b = PictBuilder::new(0, 0, 16, 16);
    // Set an obviously-wrong fg so we can see PixPat overrides it.
    b.fg_color(0x00, 0x00, 0xFF);
    b.pen_pix_pattern([0xFF; 8], &tile).unwrap();
    b.rect(Verb::Paint, 0, 0, 16, 16);
    let img = parse_pict(&b.finish()).expect("decode");

    // Row 0 → red (from tile row 0).
    assert_eq!(at(&img, 0, 0), [0xFF, 0x00, 0x00, 0xFF]);
    assert_eq!(at(&img, 7, 0), [0xFF, 0x00, 0x00, 0xFF]);
    // Row 1 → green.
    assert_eq!(at(&img, 0, 1), [0x00, 0xFF, 0x00, 0xFF]);
    // Row 8 → wraps back to red (tile is 8 rows tall).
    assert_eq!(at(&img, 0, 8), [0xFF, 0x00, 0x00, 0xFF]);
    // Row 9 → green.
    assert_eq!(at(&img, 0, 9), [0x00, 0xFF, 0x00, 0xFF]);
}

#[test]
fn fill_pix_pattern_routes_to_fill_verb() {
    // Distinct tiles in pen + fill slots — paint should pick pen, fill
    // should pick fill. Inside Macintosh §A-3 verb / pattern routing.
    let mut pen_tile = [[0u8; 4]; 64];
    let mut fill_tile = [[0u8; 4]; 64];
    for cell in pen_tile.iter_mut() {
        *cell = [0xFF, 0xAA, 0x00, 0xFF]; // orange
    }
    for cell in fill_tile.iter_mut() {
        *cell = [0x00, 0xAA, 0xFF, 0xFF]; // cyan
    }
    let mut b = PictBuilder::new(0, 0, 16, 16);
    b.pen_pix_pattern([0xFF; 8], &pen_tile).unwrap();
    b.fill_pix_pattern([0xFF; 8], &fill_tile).unwrap();
    b.rect(Verb::Paint, 0, 0, 8, 8); // paint → pen tile
    b.rect(Verb::Fill, 8, 8, 16, 16); // fill → fill tile
    let img = parse_pict(&b.finish()).expect("decode");
    assert_eq!(at(&img, 4, 4), [0xFF, 0xAA, 0x00, 0xFF], "paint = pen");
    assert_eq!(at(&img, 12, 12), [0x00, 0xAA, 0xFF, 0xFF], "fill = fill");
}

#[test]
fn bg_pix_pattern_routes_to_erase_verb() {
    let mut bg_tile = [[0u8; 4]; 64];
    for cell in bg_tile.iter_mut() {
        *cell = [0xAA, 0x00, 0xAA, 0xFF]; // magenta-ish
    }
    let mut b = PictBuilder::new(0, 0, 8, 8);
    b.bg_pix_pattern([0xFF; 8], &bg_tile).unwrap();
    b.rect(Verb::Erase, 0, 0, 8, 8);
    let img = parse_pict(&b.finish()).expect("decode");
    assert_eq!(at(&img, 4, 4), [0xAA, 0x00, 0xAA, 0xFF]);
}

#[test]
fn pen_pix_pattern_oval_fills_with_tile() {
    let tile = rg_stripe_tile();
    let mut b = PictBuilder::new(0, 0, 16, 16);
    b.pen_pix_pattern([0xFF; 8], &tile).unwrap();
    b.oval(Verb::Paint, 0, 0, 16, 16);
    let img = parse_pict(&b.finish()).expect("decode");
    // Centre of the oval — row 8, col 8.
    assert_eq!(at(&img, 8, 8), [0xFF, 0x00, 0x00, 0xFF]); // even row → red
    assert_eq!(at(&img, 8, 9), [0x00, 0xFF, 0x00, 0xFF]); // odd row → green
}

#[test]
fn pen_pix_pattern_polygon_fills_with_tile() {
    let tile = rg_stripe_tile();
    let mut b = PictBuilder::new(0, 0, 16, 16);
    b.pen_pix_pattern([0xFF; 8], &tile).unwrap();
    b.poly(Verb::Paint, &[(2, 2), (14, 2), (8, 14)]).unwrap();
    let img = parse_pict(&b.finish()).expect("decode");
    // Centroid of the triangle (8, 6) — even row → red.
    assert_eq!(at(&img, 8, 6), [0xFF, 0x00, 0x00, 0xFF]);
    // (8, 7) → odd row → green.
    assert_eq!(at(&img, 8, 7), [0x00, 0xFF, 0x00, 0xFF]);
}

#[test]
fn pen_pix_pattern_region_fills_with_tile() {
    let tile = rg_stripe_tile();
    let mut b = PictBuilder::new(0, 0, 16, 16);
    b.pen_pix_pattern([0xFF; 8], &tile).unwrap();
    b.region_rect(Verb::Paint, 2, 2, 14, 14);
    let img = parse_pict(&b.finish()).expect("decode");
    // Inside region: cell (8, 8) → even row → red.
    assert_eq!(at(&img, 8, 8), [0xFF, 0x00, 0x00, 0xFF]);
    assert_eq!(at(&img, 8, 9), [0x00, 0xFF, 0x00, 0xFF]);
    // Outside region: paper (white).
    assert_eq!(at(&img, 0, 0), [0xFF, 0xFF, 0xFF, 0xFF]);
}

#[test]
fn pen_pix_pattern_round_rect_fills_with_tile() {
    let tile = rg_stripe_tile();
    let mut b = PictBuilder::new(0, 0, 32, 32);
    b.oval_size(8, 8);
    b.pen_pix_pattern([0xFF; 8], &tile).unwrap();
    b.round_rect(Verb::Paint, 4, 4, 28, 28);
    let img = parse_pict(&b.finish()).expect("decode");
    // Centre of the round-rect — should pick up the tile (row 16, even
    // → red).
    assert_eq!(at(&img, 16, 16), [0xFF, 0x00, 0x00, 0xFF]);
    // One row down → odd row → green.
    assert_eq!(at(&img, 16, 17), [0x00, 0xFF, 0x00, 0xFF]);
}

#[test]
fn pix_pat_palette_dedup_round_trip() {
    // Build a tile with exactly 4 distinct colours — the encoder's
    // ColorTable should dedupe down to 4 entries (ctSize=3) and the
    // decode round-trip should still produce per-cell-correct RGB.
    let red = [0xFF, 0x00, 0x00, 0xFF];
    let green = [0x00, 0xFF, 0x00, 0xFF];
    let blue = [0x00, 0x00, 0xFF, 0xFF];
    let yellow = [0xFF, 0xFF, 0x00, 0xFF];
    let mut tile = [[0u8; 4]; 64];
    for y in 0..8 {
        for x in 0..8 {
            tile[y * 8 + x] = match (y / 4, x / 4) {
                (0, 0) => red,
                (0, _) => green,
                (_, 0) => blue,
                _ => yellow,
            };
        }
    }
    let mut b = PictBuilder::new(0, 0, 8, 8);
    b.pen_pix_pattern([0xFF; 8], &tile).unwrap();
    b.rect(Verb::Paint, 0, 0, 8, 8);
    let img = parse_pict(&b.finish()).expect("decode");
    // Spot-check every quadrant.
    assert_eq!(at(&img, 1, 1), red);
    assert_eq!(at(&img, 5, 1), green);
    assert_eq!(at(&img, 1, 5), blue);
    assert_eq!(at(&img, 5, 5), yellow);
}

#[test]
fn pn_pix_pat_then_pn_pat_falls_back_to_mono() {
    // Set a PixPat then override with a plain mono PnPat — the mono
    // should win (classic "most-recent-pattern-wins" QuickDraw
    // semantics).
    let tile = rg_stripe_tile();
    let mut b = PictBuilder::new(0, 0, 8, 8);
    b.fg_color(0x00, 0x00, 0xFF); // blue fg
    b.pen_pix_pattern([0xFF; 8], &tile).unwrap();
    b.pen_pattern([0xFF; 8]); // solid foreground stipple → blue fill
    b.rect(Verb::Paint, 0, 0, 8, 8);
    let img = parse_pict(&b.finish()).expect("decode");
    // Should be solid blue (the mono PnPat collapsed to solid-fg).
    assert_eq!(at(&img, 4, 4), [0x00, 0x00, 0xFF, 0xFF]);
}

#[test]
fn probe_counts_pix_pat_opcodes() {
    let tile = rg_stripe_tile();
    let mut b = PictBuilder::new(0, 0, 8, 8);
    b.pen_pix_pattern([0xFF; 8], &tile).unwrap();
    b.bg_pix_pattern([0xFF; 8], &tile).unwrap();
    b.fill_pix_pattern([0xFF; 8], &tile).unwrap();
    b.rect(Verb::Paint, 0, 0, 8, 8);
    let bytes = b.finish();
    let p = probe_pict(&bytes).expect("probe");
    assert_eq!(p.pix_pattern_set_count, 3, "3 PixPat opcodes emitted");
    assert_eq!(p.pattern_set_count, 0, "no mono pattern ops");
    assert!(p.has_visible_content());
    assert_eq!(p.termination, ProbeTermination::EndPic);
}

#[test]
fn build_pix_pat_op_emits_correct_opcode_word() {
    let tile = [[0xFFu8; 4]; 64];
    let pen = build_pix_pat_op(PixPatSlot::Pen, [0xFF; 8], &tile).unwrap();
    let bg = build_pix_pat_op(PixPatSlot::Background, [0xFF; 8], &tile).unwrap();
    let fill = build_pix_pat_op(PixPatSlot::Fill, [0xFF; 8], &tile).unwrap();
    assert_eq!(u16::from_be_bytes([pen[0], pen[1]]), 0x0013);
    assert_eq!(u16::from_be_bytes([bg[0], bg[1]]), 0x0012);
    assert_eq!(u16::from_be_bytes([fill[0], fill[1]]), 0x0014);
    // patType = 1 (colour-pixmap).
    assert_eq!(u16::from_be_bytes([pen[2], pen[3]]), 1);
}

#[test]
fn solid_colour_pix_pat_renders_uniform() {
    // Single-colour tile — every cell is sky blue. Should produce a
    // uniform paint regardless of fg / bg.
    let sky = [0x66, 0xCC, 0xFF, 0xFF];
    let tile = [sky; 64];
    let mut b = PictBuilder::new(0, 0, 4, 4);
    b.fg_color(0x00, 0xFF, 0x00); // green fg (should be ignored)
    b.pen_pix_pattern([0xFF; 8], &tile).unwrap();
    b.rect(Verb::Paint, 0, 0, 4, 4);
    let img = parse_pict(&b.finish()).expect("decode");
    for y in 0..4 {
        for x in 0..4 {
            assert_eq!(at(&img, x, y), sky, "({x},{y}) should be sky");
        }
    }
}
