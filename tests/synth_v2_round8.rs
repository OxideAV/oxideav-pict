//! Round 8 (workspace round 81) tests — monochrome pattern opcodes
//! (`PnPat` / `BkPat` / `FillPat`).
//!
//! The decoder now honours the three pattern-set opcodes on both v1
//! and v2 streams and stipples frame / paint / erase / fill verbs of
//! rect / round-rect / oval / poly / region accordingly:
//!
//! * **Paint** / **Frame** consume the **pen pattern** (`PnPat`,
//!   `0x0009` / v1 `0x09`). The pattern's on-bits select the current
//!   foreground colour; off-bits select the background colour.
//! * **Erase** consumes the **background pattern** (`BkPat`,
//!   `0x0002` / v1 `0x02`). On-bits select background, off-bits select
//!   foreground (the inverted convention from Inside Macintosh §A-3).
//! * **Fill** consumes the **fill pattern** (`FillPat`, `0x000A` /
//!   v1 `0x0A`).
//! * **Invert** ignores patterns entirely.
//!
//! Patterns that collapse to a single colour (`Pattern::BLACK` =
//! `[0xFF; 8]`, all foreground; `Pattern::WHITE` = `[0x00; 8]`, all
//! background) take a solid-colour fast path that delegates to the
//! existing primitives, so default-pattern PICTs are byte-identical to
//! the round-7 behaviour.
//!
//! The probe walker also counts pattern-set occurrences via the new
//! `PictProbe::pattern_set_count` field.

use oxideav_pict::ops::{PictBuilder, Verb};
use oxideav_pict::{parse_pict, probe_pict, Pattern, ProbeTermination};

// 50 % grey stipple — alternates foreground / background on a 1-pixel
// checkerboard. Each row is 0b10101010 / 0b01010101.
const GRAY_50: [u8; 8] = [0xAA, 0x55, 0xAA, 0x55, 0xAA, 0x55, 0xAA, 0x55];

// Horizontal stripes — even rows foreground, odd rows background.
const HSTRIPE: [u8; 8] = [0xFF, 0x00, 0xFF, 0x00, 0xFF, 0x00, 0xFF, 0x00];

// Vertical stripes — even columns foreground, odd columns background.
const VSTRIPE: [u8; 8] = [0xAA; 8];

fn at(img: &oxideav_pict::PictImage, x: u32, y: u32) -> [u8; 4] {
    let off = ((y * img.width + x) * 4) as usize;
    [
        img.data[off],
        img.data[off + 1],
        img.data[off + 2],
        img.data[off + 3],
    ]
}

// ---------------------------------------------------------------------------
// Pattern public type behaviour (re-export sanity).
// ---------------------------------------------------------------------------

#[test]
fn pattern_constants_collapse() {
    assert!(Pattern::BLACK.is_solid_fg());
    assert!(Pattern::WHITE.is_solid_bg());
}

#[test]
fn pattern_sample_50pct_grey() {
    let p = Pattern(GRAY_50);
    // Row 0 = 0xAA = 0b10101010 → even columns fg, odd bg.
    assert!(p.sample(0, 0));
    assert!(!p.sample(1, 0));
    // Row 1 = 0x55 = 0b01010101 → flipped.
    assert!(!p.sample(0, 1));
    assert!(p.sample(1, 1));
}

// ---------------------------------------------------------------------------
// PnPat — paint verb stipples between fg + bg.
// ---------------------------------------------------------------------------

#[test]
fn pn_pat_paint_rect_stipples_horizontal_stripes() {
    // 8×8 frame; bg = white (paper default), fg = red. Paint a full-
    // canvas rect with the horizontal-stripes pen pattern: even rows
    // come out red, odd rows come out white.
    let mut b = PictBuilder::new(0, 0, 8, 8);
    b.fg_color(0xFF, 0x00, 0x00);
    b.pen_pattern(HSTRIPE);
    b.rect(Verb::Paint, 0, 0, 8, 8);
    let bytes = b.finish();
    let img = parse_pict(&bytes).unwrap();
    assert_eq!(img.width, 8);
    assert_eq!(img.height, 8);
    // Row 0 (even — fg = red).
    for x in 0..8 {
        assert_eq!(at(&img, x, 0), [0xFF, 0x00, 0x00, 0xFF], "row0 x={x}");
    }
    // Row 1 (odd — bg = white).
    for x in 0..8 {
        assert_eq!(at(&img, x, 1), [0xFF, 0xFF, 0xFF, 0xFF], "row1 x={x}");
    }
    // Row 2 (even — fg).
    for x in 0..8 {
        assert_eq!(at(&img, x, 2), [0xFF, 0x00, 0x00, 0xFF], "row2 x={x}");
    }
}

#[test]
fn pn_pat_paint_rect_vertical_stripes() {
    // Vertical pattern: even columns fg, odd columns bg.
    let mut b = PictBuilder::new(0, 0, 8, 8);
    b.fg_color(0x00, 0x00, 0xFF);
    b.pen_pattern(VSTRIPE);
    b.rect(Verb::Paint, 0, 0, 8, 8);
    let bytes = b.finish();
    let img = parse_pict(&bytes).unwrap();
    for y in 0..8 {
        for x in 0..8 {
            let expected = if x % 2 == 0 {
                [0x00, 0x00, 0xFF, 0xFF]
            } else {
                [0xFF, 0xFF, 0xFF, 0xFF]
            };
            assert_eq!(at(&img, x, y), expected, "x={x} y={y}");
        }
    }
}

// ---------------------------------------------------------------------------
// FillPat — fill verb stipples between fg + bg.
// ---------------------------------------------------------------------------

#[test]
fn fill_pat_fill_rect_uses_fill_slot_not_pen() {
    // Set PnPat to all-fg (would normally make a paint a solid block)
    // and FillPat to grey-50 — the Fill verb must consult FillPat, not
    // PnPat.
    let mut b = PictBuilder::new(0, 0, 8, 8);
    b.fg_color(0x00, 0xFF, 0x00);
    b.pen_pattern([0xFF; 8]); // would collapse to solid green if used
    b.fill_pattern(GRAY_50);
    b.rect(Verb::Fill, 0, 0, 8, 8);
    let bytes = b.finish();
    let img = parse_pict(&bytes).unwrap();
    // 50 %-grey stipple: (0,0) is fg=green, (1,0) is bg=white.
    assert_eq!(at(&img, 0, 0), [0x00, 0xFF, 0x00, 0xFF]);
    assert_eq!(at(&img, 1, 0), [0xFF, 0xFF, 0xFF, 0xFF]);
}

// ---------------------------------------------------------------------------
// BkPat — erase verb uses the inverted convention (on=bg, off=fg).
// ---------------------------------------------------------------------------

#[test]
fn bk_pat_erase_rect_stipples_with_inverted_convention() {
    // bg = red, fg = blue. Erase with HSTRIPE pattern → on-bits map to
    // bg=red, off-bits to fg=blue. So even rows are red, odd rows are
    // blue.
    let mut b = PictBuilder::new(0, 0, 8, 8);
    b.fg_color(0x00, 0x00, 0xFF); // blue
    b.bg_color(0xFF, 0x00, 0x00); // red
    b.bg_pattern(HSTRIPE);
    b.rect(Verb::Erase, 0, 0, 8, 8);
    let bytes = b.finish();
    let img = parse_pict(&bytes).unwrap();
    for x in 0..8 {
        assert_eq!(at(&img, x, 0), [0xFF, 0x00, 0x00, 0xFF], "row0");
        assert_eq!(at(&img, x, 1), [0x00, 0x00, 0xFF, 0xFF], "row1");
    }
}

// ---------------------------------------------------------------------------
// Solid pattern collapses cleanly to the round-7 (pre-pattern) path.
// ---------------------------------------------------------------------------

#[test]
fn solid_fg_pattern_collapses_to_solid_fg() {
    // Pattern::BLACK (all-ones) under Paint must be byte-identical to
    // a no-pattern Paint with the same fg colour.
    let mut a = PictBuilder::new(0, 0, 16, 16);
    a.fg_color(0x80, 0x40, 0x20);
    a.rect(Verb::Paint, 0, 0, 16, 16);
    let bytes_a = a.finish();
    let img_a = parse_pict(&bytes_a).unwrap();

    let mut b = PictBuilder::new(0, 0, 16, 16);
    b.fg_color(0x80, 0x40, 0x20);
    b.pen_pattern([0xFF; 8]); // collapses to solid fg
    b.rect(Verb::Paint, 0, 0, 16, 16);
    let bytes_b = b.finish();
    let img_b = parse_pict(&bytes_b).unwrap();
    assert_eq!(
        img_a.data, img_b.data,
        "solid-fg pattern must match no-pattern"
    );
}

#[test]
fn solid_bg_pattern_paints_background_everywhere() {
    // Pattern::WHITE (all-zeros) under Paint stipples bg everywhere —
    // canvas should be flooded with the bg colour.
    let mut b = PictBuilder::new(0, 0, 8, 8);
    b.fg_color(0xFF, 0x00, 0x00); // red (would be ignored — all bg)
    b.bg_color(0x00, 0xFF, 0x00); // green
    b.pen_pattern([0x00; 8]);
    b.rect(Verb::Paint, 0, 0, 8, 8);
    let bytes = b.finish();
    let img = parse_pict(&bytes).unwrap();
    for x in 0..8 {
        for y in 0..8 {
            assert_eq!(at(&img, x, y), [0x00, 0xFF, 0x00, 0xFF], "x={x} y={y}");
        }
    }
}

// ---------------------------------------------------------------------------
// Frame verb stipples around the rect outline.
// ---------------------------------------------------------------------------

#[test]
fn pn_pat_frame_rect_stipples_outline() {
    // Frame is 1-pixel pen — the outline pixels are stippled with
    // PnPat. We frame a 4×4 rect with HSTRIPE so the top/bottom edges
    // come out fg/bg alternating per-row.
    let mut b = PictBuilder::new(0, 0, 8, 8);
    b.fg_color(0xFF, 0x00, 0x00); // red
    b.pen_pattern(HSTRIPE);
    b.rect(Verb::Frame, 2, 2, 6, 6);
    let bytes = b.finish();
    let img = parse_pict(&bytes).unwrap();
    // Top edge is row 2 = even → fg=red.
    for x in 2..6 {
        assert_eq!(at(&img, x, 2), [0xFF, 0x00, 0x00, 0xFF], "top edge x={x}");
    }
    // Bottom edge is row 5 = odd → bg=white.
    for x in 2..6 {
        assert_eq!(at(&img, x, 5), [0xFF, 0xFF, 0xFF, 0xFF], "bot edge x={x}");
    }
    // Interior unfilled (paper).
    assert_eq!(at(&img, 3, 3), [0xFF, 0xFF, 0xFF, 0xFF], "interior 3,3");
}

// ---------------------------------------------------------------------------
// Pattern applies to oval / poly / region too.
// ---------------------------------------------------------------------------

#[test]
fn pn_pat_paint_oval_stipples_interior() {
    let mut b = PictBuilder::new(0, 0, 16, 16);
    b.fg_color(0x00, 0x00, 0xFF); // blue
    b.pen_pattern(GRAY_50);
    b.oval(Verb::Paint, 0, 0, 16, 16);
    let bytes = b.finish();
    let img = parse_pict(&bytes).unwrap();
    // Centre of oval (8, 8) is inside; row 8 = 0xAA, col 8 = bit 7 of
    // 0xAA = 1 → fg = blue.
    assert_eq!(at(&img, 8, 8), [0x00, 0x00, 0xFF, 0xFF]);
    // (9, 8): col 9 → bit 6 of 0xAA = 0 → bg = white.
    assert_eq!(at(&img, 9, 8), [0xFF, 0xFF, 0xFF, 0xFF]);
}

#[test]
fn fill_pat_fill_polygon_stipples_interior() {
    let mut b = PictBuilder::new(0, 0, 12, 12);
    b.fg_color(0xFF, 0x80, 0x00); // orange
    b.fill_pattern(HSTRIPE);
    let _ = b.poly(Verb::Fill, &[(2, 2), (10, 2), (6, 10)]).unwrap();
    let bytes = b.finish();
    let img = parse_pict(&bytes).unwrap();
    // (6, 4) inside triangle, row 4 = even → fg = orange.
    assert_eq!(at(&img, 6, 4), [0xFF, 0x80, 0x00, 0xFF]);
    // (6, 5) inside triangle, row 5 = odd → bg = white (paper).
    assert_eq!(at(&img, 6, 5), [0xFF, 0xFF, 0xFF, 0xFF]);
}

// ---------------------------------------------------------------------------
// v1 PICT pattern opcodes (0x02 / 0x09 / 0x0A).
// ---------------------------------------------------------------------------

#[test]
fn v1_pn_pat_paint_rect() {
    // Hand-assemble a v1 stream: 10-byte header, v1 sentinel
    // (0x11 0x01), then state opcodes + a paint rect, then OpEndPic.
    let mut bytes = Vec::new();
    // picture record: picSize (placeholder) + picFrame (0,0,8,8)
    bytes.extend_from_slice(&[0x00, 0x00]); // picSize
    bytes.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x08, 0x00, 0x08]); // top/left/bot/right
                                                                                // v1 sentinel (1-byte opcode 0x11 + version 0x01)
    bytes.push(0x11);
    bytes.push(0x01);
    // FgColor (v1 opcode 0x0E) = Pascal redColor (205)
    bytes.push(0x0E);
    bytes.extend_from_slice(&205u32.to_be_bytes());
    // PnPat (v1 opcode 0x09) = HSTRIPE
    bytes.push(0x09);
    bytes.extend_from_slice(&HSTRIPE);
    // PaintRect (v1 opcode 0x31) + rect (0,0,8,8)
    bytes.push(0x31);
    bytes.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x08, 0x00, 0x08]);
    // OpEndPic (v1 opcode 0xFF)
    bytes.push(0xFF);

    let img = parse_pict(&bytes).unwrap();
    // Row 0 → fg = red (Pascal redColor).
    assert_eq!(at(&img, 0, 0), [0xFF, 0x00, 0x00, 0xFF]);
    // Row 1 → bg = paper (white).
    assert_eq!(at(&img, 0, 1), [0xFF, 0xFF, 0xFF, 0xFF]);
}

#[test]
fn v1_bk_pat_erase_rect() {
    // bg colour = greenColor (Pascal 341), bg pattern = HSTRIPE.
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&[0x00, 0x00]);
    bytes.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x08, 0x00, 0x08]);
    bytes.push(0x11);
    bytes.push(0x01);
    // BgColor (0x0F) = greenColor 341
    bytes.push(0x0F);
    bytes.extend_from_slice(&341u32.to_be_bytes());
    // FgColor (0x0E) = blueColor 409
    bytes.push(0x0E);
    bytes.extend_from_slice(&409u32.to_be_bytes());
    // BkPat (0x02) = HSTRIPE
    bytes.push(0x02);
    bytes.extend_from_slice(&HSTRIPE);
    // EraseRect (0x32) + rect (0,0,8,8)
    bytes.push(0x32);
    bytes.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x08, 0x00, 0x08]);
    bytes.push(0xFF);

    let img = parse_pict(&bytes).unwrap();
    // Erase + HSTRIPE: row 0 (on-bits) → bg=green; row 1 (off-bits) →
    // fg=blue.
    assert_eq!(at(&img, 0, 0), [0x00, 0xFF, 0x00, 0xFF]);
    assert_eq!(at(&img, 0, 1), [0x00, 0x00, 0xFF, 0xFF]);
}

// ---------------------------------------------------------------------------
// Probe counts pattern-set opcodes.
// ---------------------------------------------------------------------------

#[test]
fn probe_counts_v2_pattern_sets() {
    let mut b = PictBuilder::new(0, 0, 4, 4);
    b.pen_pattern(HSTRIPE);
    b.bg_pattern(VSTRIPE);
    b.fill_pattern(GRAY_50);
    b.rect(Verb::Paint, 0, 0, 4, 4);
    let bytes = b.finish();
    let p = probe_pict(&bytes).unwrap();
    assert_eq!(p.pattern_set_count, 3, "3 pattern-set opcodes");
    assert_eq!(p.drawing_count, 1);
    assert_eq!(p.termination, ProbeTermination::EndPic);
}

#[test]
fn probe_counts_v1_pattern_sets() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&[0x00, 0x00]);
    bytes.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0x00, 0x04]);
    bytes.push(0x11);
    bytes.push(0x01);
    // PnPat (0x09)
    bytes.push(0x09);
    bytes.extend_from_slice(&HSTRIPE);
    // BkPat (0x02)
    bytes.push(0x02);
    bytes.extend_from_slice(&VSTRIPE);
    // FillPat (0x0A)
    bytes.push(0x0A);
    bytes.extend_from_slice(&GRAY_50);
    // PaintRect
    bytes.push(0x31);
    bytes.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0x00, 0x04]);
    bytes.push(0xFF);

    let p = probe_pict(&bytes).unwrap();
    assert_eq!(p.pattern_set_count, 3, "3 v1 pattern-set opcodes");
    assert_eq!(p.drawing_count, 1);
    assert_eq!(p.termination, ProbeTermination::EndPic);
}

// ---------------------------------------------------------------------------
// Region paint uses pen_pat.
// ---------------------------------------------------------------------------

#[test]
fn pn_pat_paint_region_stipples_rect_region() {
    let mut b = PictBuilder::new(0, 0, 8, 8);
    b.fg_color(0xFF, 0x00, 0x00);
    b.pen_pattern(HSTRIPE);
    b.region_rect(Verb::Paint, 0, 0, 8, 8);
    let bytes = b.finish();
    let img = parse_pict(&bytes).unwrap();
    // Row 0 → fg = red.
    assert_eq!(at(&img, 4, 0), [0xFF, 0x00, 0x00, 0xFF]);
    // Row 1 → bg = white.
    assert_eq!(at(&img, 4, 1), [0xFF, 0xFF, 0xFF, 0xFF]);
}

// ---------------------------------------------------------------------------
// Pattern survives across multiple draws (state persistence).
// ---------------------------------------------------------------------------

#[test]
fn pen_pat_persists_across_multiple_paints() {
    // Set PnPat once, then paint three disjoint rectangles. Every
    // rectangle should have the same stipple.
    // picFrame = (top=0, left=0, bottom=8, right=16) → 16 wide × 8 tall.
    let mut b = PictBuilder::new(0, 0, 8, 16);
    b.fg_color(0x00, 0x00, 0x80);
    b.pen_pattern(HSTRIPE);
    b.rect(Verb::Paint, 0, 0, 4, 4);
    b.rect(Verb::Paint, 0, 6, 4, 10);
    b.rect(Verb::Paint, 0, 12, 4, 16);
    let bytes = b.finish();
    let img = parse_pict(&bytes).unwrap();
    // All three rectangles: row 0 = fg dark blue, row 1 = bg white.
    for &x in &[1u32, 7, 13] {
        assert_eq!(at(&img, x, 0), [0x00, 0x00, 0x80, 0xFF], "x={x} row 0");
        assert_eq!(at(&img, x, 1), [0xFF, 0xFF, 0xFF, 0xFF], "x={x} row 1");
    }
}
