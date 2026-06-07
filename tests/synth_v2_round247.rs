//! Round 247 — `PnMode` Boolean pattern transfer modes.
//!
//! Inside Macintosh: Imaging With QuickDraw §3 ("QuickDraw Drawing
//! Reference") `PenMode` procedure (book page 3-44) defines eight
//! Boolean pattern modes (`patCopy = 8` … `notPatBic = 15`) consumed by
//! every pattern-fill verb (frame / paint / erase / fill of rect /
//! round-rect / oval / poly / region). The round-230 dispatcher
//! captures the mode integer into `PictTextState::pn_mode`; round 247
//! wires the rasteriser to honour it.
//!
//! Each test below builds a minimal v2 PICT that:
//! 1. paints a known destination colour over the whole canvas (the
//!    "destination" the transfer mode reads from);
//! 2. sets `PnMode` to one of the eight Boolean codes;
//! 3. sets a known pen pattern (a horizontal-stripe `[0xFF, 0x00, …]`
//!    so on-bits land on even rows and off-bits on odd rows);
//! 4. paints a single rect with foreground `fg` and background `bg`.
//!
//! The expected canvas at row 0 (pattern-bit-1) and row 1 (pattern-bit-
//! 0) follows the §3-44 table verbatim.

use oxideav_pict::ops::{PictBuilder, Verb};
use oxideav_pict::{parse_pict, PatternMode, PictImage};

const HSTRIPE: [u8; 8] = [0xFF, 0x00, 0xFF, 0x00, 0xFF, 0x00, 0xFF, 0x00];

/// Build a 2×2 PICT that paints a "destination" rect of colour `dst`,
/// then sets `PnMode` to `mode` + the horizontal-stripe pen pattern,
/// then paints a 2×2 rect with fg = pattern-bit-1 colour, bg = pattern-
/// bit-0 colour.
fn build_pn_mode_paint(
    mode: i16,
    dst: (u8, u8, u8),
    fg: (u8, u8, u8),
    bg: (u8, u8, u8),
) -> Vec<u8> {
    let mut b = PictBuilder::new(0, 0, 2, 2);
    // Establish destination — use erase verb (uses bg_pattern) with a
    // pn_mode that's irrelevant for `srcCopy`-style colour wash. Easier
    // to just use a paint verb with the default patCopy + solid black
    // pen pattern to write `dst`.
    b.fg_color(dst.0, dst.1, dst.2)
        .bg_color(dst.0, dst.1, dst.2)
        .pn_mode(8) // patCopy — solid colour wash
        .pen_pattern([0xFF; 8])
        .rect(Verb::Paint, 0, 0, 2, 2);
    // Now switch to the actual mode under test + stripe pattern + fg/bg
    // colour pair, and emit the paint we're measuring.
    b.fg_color(fg.0, fg.1, fg.2)
        .bg_color(bg.0, bg.1, bg.2)
        .pn_mode(mode)
        .pen_pattern(HSTRIPE)
        .rect(Verb::Paint, 0, 0, 2, 2);
    b.finish()
}

/// Read pixel `(x, y)` as `(r, g, b)`.
fn pix(img: &PictImage, x: u32, y: u32) -> (u8, u8, u8) {
    let off = ((y * img.width + x) * 4) as usize;
    (img.data[off], img.data[off + 1], img.data[off + 2])
}

// ---------------------------------------------------------------------------
// PatternMode mapping table.
// ---------------------------------------------------------------------------

#[test]
fn pn_mode_integer_codes_map_to_pattern_modes() {
    // Inside Macintosh §3-44 PenMode catalog (page 3-44 / 3-45).
    assert_eq!(PatternMode::from_pn_mode(8), PatternMode::PatCopy);
    assert_eq!(PatternMode::from_pn_mode(9), PatternMode::PatOr);
    assert_eq!(PatternMode::from_pn_mode(10), PatternMode::PatXor);
    assert_eq!(PatternMode::from_pn_mode(11), PatternMode::PatBic);
    assert_eq!(PatternMode::from_pn_mode(12), PatternMode::NotPatCopy);
    assert_eq!(PatternMode::from_pn_mode(13), PatternMode::NotPatOr);
    assert_eq!(PatternMode::from_pn_mode(14), PatternMode::NotPatXor);
    assert_eq!(PatternMode::from_pn_mode(15), PatternMode::NotPatBic);
}

#[test]
fn pn_mode_unknown_codes_fall_back_to_pat_copy() {
    // Source modes (0..=7) and arithmetic transfer modes (32..=49) fall
    // back to the §3-44 default rather than wedging the rasteriser.
    for code in [-1, 0, 1, 7, 16, 31, 32, 49, 50, 100, i16::MAX, i16::MIN] {
        assert_eq!(
            PatternMode::from_pn_mode(code),
            PatternMode::PatCopy,
            "code = {code}"
        );
    }
}

#[test]
fn pat_copy_is_default_for_default_state() {
    assert_eq!(PatternMode::default(), PatternMode::PatCopy);
    assert!(PatternMode::default().is_pat_copy());
}

// ---------------------------------------------------------------------------
// Boolean modes 8..=15 — per-mode rasteriser coverage.
//
// Conventions used in the assertions below (all match §3-44):
//
// * `dst` = pre-existing destination colour at every cell (we paint it
//   first with a patCopy solid-colour wash).
// * `fg` = foreground colour the §3-44 table references.
// * `bg` = background colour.
// * Pattern: HSTRIPE = [0xFF, 0x00, …] → row 0 (and every even row) is
//   "pattern-bit-1" (on); row 1 (every odd row) is "pattern-bit-0"
//   (off).
//
// Distinct r/g/b values for `dst`, `fg`, `bg` make the inverted-
// destination cell distinguishable from every input.
// ---------------------------------------------------------------------------

const DST: (u8, u8, u8) = (0x10, 0x20, 0x30);
const FG: (u8, u8, u8) = (0xFF, 0x00, 0x00);
const BG: (u8, u8, u8) = (0x00, 0xFF, 0x00);

#[test]
fn pat_copy_writes_fg_on_bg_off() {
    let bytes = build_pn_mode_paint(8, DST, FG, BG);
    let img = parse_pict(&bytes).expect("decode");
    // row 0 (on) -> fg.
    assert_eq!(pix(&img, 0, 0), FG);
    // row 1 (off) -> bg.
    assert_eq!(pix(&img, 0, 1), BG);
}

#[test]
fn pat_or_writes_fg_on_unchanged_off() {
    let bytes = build_pn_mode_paint(9, DST, FG, BG);
    let img = parse_pict(&bytes).expect("decode");
    // row 0 (on) -> fg.
    assert_eq!(pix(&img, 0, 0), FG);
    // row 1 (off) -> destination unchanged.
    assert_eq!(pix(&img, 0, 1), DST);
}

#[test]
fn pat_xor_inverts_on_unchanged_off() {
    let bytes = build_pn_mode_paint(10, DST, FG, BG);
    let img = parse_pict(&bytes).expect("decode");
    // row 0 (on) -> inverted destination.
    let inv = (!DST.0, !DST.1, !DST.2);
    assert_eq!(pix(&img, 0, 0), inv);
    // row 1 (off) -> destination unchanged.
    assert_eq!(pix(&img, 0, 1), DST);
}

#[test]
fn pat_bic_writes_bg_on_unchanged_off() {
    let bytes = build_pn_mode_paint(11, DST, FG, BG);
    let img = parse_pict(&bytes).expect("decode");
    // row 0 (on) -> bg.
    assert_eq!(pix(&img, 0, 0), BG);
    // row 1 (off) -> destination unchanged.
    assert_eq!(pix(&img, 0, 1), DST);
}

#[test]
fn not_pat_copy_swaps_fg_and_bg() {
    let bytes = build_pn_mode_paint(12, DST, FG, BG);
    let img = parse_pict(&bytes).expect("decode");
    // row 0 (on) -> bg.
    assert_eq!(pix(&img, 0, 0), BG);
    // row 1 (off) -> fg.
    assert_eq!(pix(&img, 0, 1), FG);
}

#[test]
fn not_pat_or_writes_fg_on_off_only() {
    let bytes = build_pn_mode_paint(13, DST, FG, BG);
    let img = parse_pict(&bytes).expect("decode");
    // row 0 (on) -> destination unchanged.
    assert_eq!(pix(&img, 0, 0), DST);
    // row 1 (off) -> fg.
    assert_eq!(pix(&img, 0, 1), FG);
}

#[test]
fn not_pat_xor_inverts_off_unchanged_on() {
    let bytes = build_pn_mode_paint(14, DST, FG, BG);
    let img = parse_pict(&bytes).expect("decode");
    // row 0 (on) -> destination unchanged.
    assert_eq!(pix(&img, 0, 0), DST);
    // row 1 (off) -> inverted destination.
    let inv = (!DST.0, !DST.1, !DST.2);
    assert_eq!(pix(&img, 0, 1), inv);
}

#[test]
fn not_pat_bic_writes_bg_on_off_unchanged_on() {
    let bytes = build_pn_mode_paint(15, DST, FG, BG);
    let img = parse_pict(&bytes).expect("decode");
    // row 0 (on) -> destination unchanged.
    assert_eq!(pix(&img, 0, 0), DST);
    // row 1 (off) -> bg.
    assert_eq!(pix(&img, 0, 1), BG);
}

// ---------------------------------------------------------------------------
// Mode application across shape verbs.
// ---------------------------------------------------------------------------

#[test]
fn pn_mode_applies_to_oval_paint() {
    // Centre of the oval bounding box must follow the §3-44 rule; we
    // only check the cell at (1, 1) of a 3×3 canvas since the ellipse
    // sampler guarantees the centre row hits.
    let mut b = PictBuilder::new(0, 0, 3, 3);
    b.fg_color(DST.0, DST.1, DST.2)
        .bg_color(DST.0, DST.1, DST.2)
        .pn_mode(8)
        .pen_pattern([0xFF; 8])
        .rect(Verb::Paint, 0, 0, 3, 3);
    // Switch to notPatOr + HSTRIPE: row 1 (off-bit) should write fg.
    b.fg_color(FG.0, FG.1, FG.2)
        .bg_color(BG.0, BG.1, BG.2)
        .pn_mode(13)
        .pen_pattern(HSTRIPE);
    // Oval paint verb (0x0051 = paintOval).
    b.oval(Verb::Paint, 0, 0, 3, 3);
    let img = parse_pict(&b.finish()).expect("decode");
    // Row 1 cell 1 is inside the ellipse and on a pattern-off row → fg.
    assert_eq!(pix(&img, 1, 1), FG);
    // Row 0 cell 1 is inside the ellipse but on a pattern-on row →
    // notPatOr leaves it as destination.
    assert_eq!(pix(&img, 1, 0), DST);
}

#[test]
fn pn_mode_applies_to_polygon_paint() {
    // Square polygon covering the full 4×4 canvas — every cell is
    // inside, so the §3-44 mode reads back the same shape as the rect
    // verb. notPatCopy (= 12) swaps fg / bg: pattern-on rows write bg,
    // pattern-off rows write fg.
    let mut b = PictBuilder::new(0, 0, 4, 4);
    b.fg_color(DST.0, DST.1, DST.2)
        .bg_color(DST.0, DST.1, DST.2)
        .pn_mode(8)
        .pen_pattern([0xFF; 8])
        .rect(Verb::Paint, 0, 0, 4, 4);
    b.fg_color(FG.0, FG.1, FG.2)
        .bg_color(BG.0, BG.1, BG.2)
        .pn_mode(12)
        .pen_pattern(HSTRIPE);
    b.poly(Verb::Paint, &[(0, 0), (4, 0), (4, 4), (0, 4)])
        .expect("poly");
    let img = parse_pict(&b.finish()).expect("decode");
    // Row 0 (even = on) → notPatCopy writes bg.
    assert_eq!(pix(&img, 1, 0), BG);
    // Row 1 (odd = off) → notPatCopy writes fg.
    assert_eq!(pix(&img, 1, 1), FG);
}

#[test]
fn pn_mode_default_state_unchanged_by_round8_pattern_tests() {
    // Round-8 (and 91 / 95) tests pre-date round 247 and rely on the
    // fresh-GrafPort default `pn_mode = 8 (patCopy)`. Just confirm the
    // default still resolves to PatCopy through PatternMode::from_pn_mode.
    use oxideav_pict::PictTextState;
    let ts = PictTextState::fresh_graf_port();
    assert_eq!(ts.pn_mode, 8);
    assert_eq!(PatternMode::from_pn_mode(ts.pn_mode), PatternMode::PatCopy,);
}

#[test]
fn fill_verb_honours_pn_mode_too() {
    // fillRect uses state.fill_pat (default = Pattern::BLACK) — same
    // §3-44 modes apply (PenMode controls the pen path; `fillVerb`
    // historically used a separate `patMode` but Imaging With
    // QuickDraw collapsed both to the `PnMode` field for picture-time
    // playback).
    let mut b = PictBuilder::new(0, 0, 2, 2);
    b.fg_color(DST.0, DST.1, DST.2)
        .bg_color(DST.0, DST.1, DST.2)
        .pn_mode(8)
        .pen_pattern([0xFF; 8])
        .rect(Verb::Paint, 0, 0, 2, 2);
    // patBic + HSTRIPE fill pattern: on rows write bg, off rows
    // unchanged.
    b.fg_color(FG.0, FG.1, FG.2)
        .bg_color(BG.0, BG.1, BG.2)
        .pn_mode(11)
        .fill_pattern(HSTRIPE)
        .rect(Verb::Fill, 0, 0, 2, 2);
    let img = parse_pict(&b.finish()).expect("decode");
    // Row 0 (on) -> bg.
    assert_eq!(pix(&img, 0, 0), BG);
    // Row 1 (off) -> destination unchanged.
    assert_eq!(pix(&img, 0, 1), DST);
}

#[test]
fn erase_verb_honours_pn_mode_with_bk_pattern() {
    // EraseRect uses state.back_pat. §3-44 PnMode applies.
    // The erase verb in the round-2 dispatch swaps fg/bg roles
    // (on-bits select background, off-bits select foreground) so
    // mode-honoured arithmetic still maps cleanly:
    //
    //   patOr on an erase verb: on-bits write `bg` (the swapped fg),
    //   off-bits leave destination unchanged.
    let mut b = PictBuilder::new(0, 0, 2, 2);
    b.fg_color(DST.0, DST.1, DST.2)
        .bg_color(DST.0, DST.1, DST.2)
        .pn_mode(8)
        .pen_pattern([0xFF; 8])
        .rect(Verb::Paint, 0, 0, 2, 2);
    b.fg_color(FG.0, FG.1, FG.2)
        .bg_color(BG.0, BG.1, BG.2)
        .pn_mode(9) // patOr
        .bg_pattern(HSTRIPE)
        .rect(Verb::Erase, 0, 0, 2, 2);
    let img = parse_pict(&b.finish()).expect("decode");
    // Erase verb swap: state passes (back_pat, bg, fg). With patOr the
    // pattern-on cells (row 0) write the first colour-arg (bg). Off
    // cells (row 1) leave destination unchanged.
    assert_eq!(pix(&img, 0, 0), BG);
    assert_eq!(pix(&img, 0, 1), DST);
}
