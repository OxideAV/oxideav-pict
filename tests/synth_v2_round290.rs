//! Round 290 — the `hilite = 50` highlighting transfer mode.
//!
//! Inside Macintosh: Imaging With QuickDraw §4 ("Color QuickDraw"),
//! section "Highlighting" (book pages 4-41..4-43), defines the
//! `hilite = 50` transfer-mode constant: *"add to source or pattern
//! mode for highlighting."* The spec contract:
//!
//! * *"When highlighting, Color QuickDraw replaces the background color
//!   with the highlight color when your application draws or copies
//!   images between graphics ports."* (4-41)
//! * *"Highlighting uses the pattern or source image to decide which
//!   bits to exchange; only bits that are on in the pattern or source
//!   image can be highlighted in the destination."* (4-43)
//! * §4-40 Table 4-2 ("Arithmetic modes in a 1-bit environment"):
//!   `hilite` reverts to `srcXor` — so with no explicit `HiliteColor`
//!   the mode falls back to the channel-wise invert the round-252 /
//!   round-282 XOR paths already use.
//!
//! The highlight colour comes from the `HiliteColor` opcode (`$001D`),
//! captured into `PictTextState::hilite_color` since round 230. The
//! exchange is reversible (its own inverse on the `{bg, hilite}` pair),
//! matching the §4-40 `srcXor` revert: applying hilite twice restores
//! the destination.

use oxideav_pict::ops::{PictBuilder, Verb};
use oxideav_pict::{
    blend_source, parse_pict, PackType, PatternMode, PictImage, Rgba, SourceMode, HILITE_MODE,
};

/// Read pixel `(x, y)` as `(r, g, b)`.
fn pix(img: &PictImage, x: u32, y: u32) -> (u8, u8, u8) {
    let off = ((y * img.width + x) * 4) as usize;
    (img.data[off], img.data[off + 1], img.data[off + 2])
}

// ---------------------------------------------------------------------------
// Resolver unit tests.
// ---------------------------------------------------------------------------

#[test]
fn pn_mode_50_with_hilite_colour_resolves_hilite() {
    let bg = Rgba::new(0xA0, 0xFF, 0xE0, 255);
    let hilite = Rgba::new(0xFF, 0x00, 0x00, 255);
    let m = PatternMode::from_pn_mode_with(HILITE_MODE, None, bg, Some(hilite));
    assert_eq!(m, PatternMode::Hilite { hilite, bg_key: bg });
}

#[test]
fn pn_mode_50_without_hilite_colour_reverts_to_patxor() {
    // §4-40 Table 4-2: hilite → srcXor / patXor when no HiliteColor.
    let bg = Rgba::new(0xA0, 0xFF, 0xE0, 255);
    let m = PatternMode::from_pn_mode_with(HILITE_MODE, None, bg, None);
    assert_eq!(m, PatternMode::PatXor);
}

#[test]
fn source_mode_50_with_hilite_colour_resolves_hilite() {
    let bg = Rgba::new(0x10, 0x20, 0x30, 255);
    let hilite = Rgba::new(0x00, 0xFF, 0x00, 255);
    let m = SourceMode::from_mode_word(HILITE_MODE, None, bg, Some(hilite));
    assert_eq!(m, SourceMode::Hilite { hilite, bg_key: bg });
}

#[test]
fn source_mode_50_without_hilite_colour_reverts_to_srcxor() {
    let bg = Rgba::new(0x10, 0x20, 0x30, 255);
    let m = SourceMode::from_mode_word(HILITE_MODE, None, bg, None);
    assert_eq!(m, SourceMode::SrcXor);
}

#[test]
fn source_mode_50_strips_dither_copy_bit() {
    // ditherCopy = 64 is additive; 50 | 64 = 114 still resolves hilite.
    let bg = Rgba::new(0x10, 0x20, 0x30, 255);
    let hilite = Rgba::new(0x00, 0xFF, 0x00, 255);
    let m = SourceMode::from_mode_word(HILITE_MODE | 64, None, bg, Some(hilite));
    assert_eq!(m, SourceMode::Hilite { hilite, bg_key: bg });
}

// ---------------------------------------------------------------------------
// Pure-combiner exchange semantics (raster path).
// ---------------------------------------------------------------------------

#[test]
fn blend_source_hilite_exchanges_bg_for_hilite() {
    let bg = Rgba::new(0xA0, 0xFF, 0xE0, 255);
    let hilite = Rgba::new(0xFF, 0x00, 0x00, 255);
    let mode = SourceMode::Hilite { hilite, bg_key: bg };
    // A non-white "on" source pixel over a background-coloured dst →
    // highlight colour.
    let src_on = Rgba::new(0x00, 0x00, 0x00, 255);
    let out = blend_source(mode, src_on, bg, Rgba::BLACK, bg);
    assert_eq!((out.r, out.g, out.b), (0xFF, 0x00, 0x00));
    // The reverse: a dst already in the highlight colour reverts to bg.
    let out2 = blend_source(mode, src_on, hilite, Rgba::BLACK, bg);
    assert_eq!((out2.r, out2.g, out2.b), (0xA0, 0xFF, 0xE0));
    // A third colour is left unchanged.
    let other = Rgba::new(0x12, 0x34, 0x56, 255);
    let out3 = blend_source(mode, src_on, other, Rgba::BLACK, bg);
    assert_eq!((out3.r, out3.g, out3.b), (0x12, 0x34, 0x56));
}

#[test]
fn blend_source_hilite_leaves_white_source_pixels_alone() {
    // §4-43: only on-bits of the source participate; a white source
    // pixel is "off" and leaves the destination unchanged.
    let bg = Rgba::new(0xA0, 0xFF, 0xE0, 255);
    let hilite = Rgba::new(0xFF, 0x00, 0x00, 255);
    let mode = SourceMode::Hilite { hilite, bg_key: bg };
    let src_off = Rgba::WHITE;
    let out = blend_source(mode, src_off, bg, Rgba::BLACK, bg);
    assert_eq!((out.r, out.g, out.b), (0xA0, 0xFF, 0xE0));
}

#[test]
fn blend_source_hilite_is_self_inverse() {
    // Applying hilite twice restores the destination (§4-40 srcXor
    // revert property).
    let bg = Rgba::new(0x33, 0x66, 0x99, 255);
    let hilite = Rgba::new(0xEE, 0xDD, 0xCC, 255);
    let mode = SourceMode::Hilite { hilite, bg_key: bg };
    let src_on = Rgba::BLACK;
    for &start in &[bg, hilite] {
        let once = blend_source(mode, src_on, start, Rgba::BLACK, bg);
        let twice = blend_source(mode, src_on, once, Rgba::BLACK, bg);
        assert_eq!((twice.r, twice.g, twice.b), (start.r, start.g, start.b));
    }
}

// ---------------------------------------------------------------------------
// End-to-end pattern-fill decode (PnMode = hilite).
// ---------------------------------------------------------------------------

#[test]
fn pattern_fill_hilite_highlights_background_cells() {
    // Listing 4-7 shape: wash the canvas a light-blue background, set
    // the highlight colour to red, then paint over it with the hilite
    // pen mode. Every on-bit cell whose destination is the background
    // colour takes the highlight colour.
    let bg = (0xA0, 0xFF, 0xE0);
    let mut b = PictBuilder::new(0, 0, 4, 4);
    // Wash background: patCopy + solid-fg pen over fg = bg colour.
    b.fg_color(bg.0, bg.1, bg.2)
        .bg_color(bg.0, bg.1, bg.2)
        .pn_mode(8) // patCopy
        .pen_pattern([0xFF; 8])
        .rect(Verb::Paint, 0, 0, 4, 4);
    // Highlight pass: hilite colour = red, background key = the wash.
    b.hilite_color(0xFF, 0x00, 0x00)
        .bg_color(bg.0, bg.1, bg.2)
        .pn_mode(HILITE_MODE)
        .pen_pattern([0xFF; 8]) // all on-bits
        .rect(Verb::Paint, 0, 0, 4, 4);
    let img = parse_pict(&b.finish()).expect("decode");
    // Whole rect was the background colour → now the highlight colour.
    assert_eq!(pix(&img, 0, 0), (0xFF, 0x00, 0x00));
    assert_eq!(pix(&img, 3, 3), (0xFF, 0x00, 0x00));
}

#[test]
fn pattern_fill_hilite_only_touches_on_bits() {
    // A horizontal-stripe pen pattern: on-rows highlight, off-rows are
    // left at the background colour.
    const HSTRIPE: [u8; 8] = [0xFF, 0x00, 0xFF, 0x00, 0xFF, 0x00, 0xFF, 0x00];
    let bg = (0xA0, 0xFF, 0xE0);
    let mut b = PictBuilder::new(0, 0, 2, 2);
    b.fg_color(bg.0, bg.1, bg.2)
        .bg_color(bg.0, bg.1, bg.2)
        .pn_mode(8)
        .pen_pattern([0xFF; 8])
        .rect(Verb::Paint, 0, 0, 2, 2);
    b.hilite_color(0x00, 0x00, 0xFF)
        .bg_color(bg.0, bg.1, bg.2)
        .pn_mode(HILITE_MODE)
        .pen_pattern(HSTRIPE)
        .rect(Verb::Paint, 0, 0, 2, 2);
    let img = parse_pict(&b.finish()).expect("decode");
    // Row 0 (on bits) → highlight blue; row 1 (off bits) → background.
    assert_eq!(pix(&img, 0, 0), (0x00, 0x00, 0xFF));
    assert_eq!(pix(&img, 0, 1), (0xA0, 0xFF, 0xE0));
}

#[test]
fn pattern_fill_hilite_without_colour_inverts() {
    // No HiliteColor opcode → §4-40 patXor revert (channel-wise NOT of
    // the destination at on-bit cells).
    let dst = (0x30, 0x60, 0x90);
    let mut b = PictBuilder::new(0, 0, 2, 2);
    b.fg_color(dst.0, dst.1, dst.2)
        .bg_color(dst.0, dst.1, dst.2)
        .pn_mode(8)
        .pen_pattern([0xFF; 8])
        .rect(Verb::Paint, 0, 0, 2, 2);
    // hilite mode, but no hilite_color set → invert.
    b.pn_mode(HILITE_MODE)
        .pen_pattern([0xFF; 8])
        .rect(Verb::Paint, 0, 0, 2, 2);
    let img = parse_pict(&b.finish()).expect("decode");
    assert_eq!(pix(&img, 0, 0), (!0x30, !0x60, !0x90));
}

// ---------------------------------------------------------------------------
// End-to-end raster-blit decode (CopyBits mode word = hilite).
// ---------------------------------------------------------------------------

#[test]
fn raster_blit_hilite_highlights_background_pixels() {
    // Wash the 2×1 canvas (frame top=0 left=0 bottom=1 right=2) the
    // background colour, then blit a two-pixel source (one black "on"
    // pixel, one white "off" pixel) through a DirectBitsRect whose mode
    // word is `hilite`. Only the on pixel's destination — which is the
    // background colour — is highlighted.
    let bg = (0xA0, 0xFF, 0xE0);
    let mut b = PictBuilder::new(0, 0, 1, 2);
    b.fg_color(bg.0, bg.1, bg.2)
        .bg_color(bg.0, bg.1, bg.2)
        .pn_mode(8)
        .pen_pattern([0xFF; 8])
        .rect(Verb::Paint, 0, 0, 1, 2);
    b.hilite_color(0xFF, 0x00, 0x00).bg_color(bg.0, bg.1, bg.2);
    // Source: x=0 = black (on), x=1 = white (off).
    let src = [
        0u8, 0, 0, 255, // black → on
        255, 255, 255, 255, // white → off
    ];
    b.raster_with_mode(0, 0, 1, 2, &src, PackType::Raw, HILITE_MODE as u16)
        .expect("raster");
    let img = parse_pict(&b.finish()).expect("decode");
    // x=0 (on pixel over bg) → highlight red.
    assert_eq!(pix(&img, 0, 0), (0xFF, 0x00, 0x00));
    // x=1 (off / white source pixel) → unchanged background.
    assert_eq!(pix(&img, 1, 0), (0xA0, 0xFF, 0xE0));
}
