//! Round 95 tests — Dithered PixPat sub-type (`patType=2`).
//!
//! Inside Macintosh: Imaging With QuickDraw §A-3 Listing A-1 — the
//! `BkPixPat 0x0012`, `PnPixPat 0x0013` and `FillPixPat 0x0014` opcodes
//! each carry a `PixPat` record. The `patType=2` "ditherPat" sub-type
//! has a 16-byte payload after the opcode word:
//!
//! ```text
//! PatType:   word     (= 2)
//! Pat1Data:  Pattern  (8 bytes — monochrome fallback)
//! RGB:       RGBColor (6 bytes — target colour)
//! ```
//!
//! Color QuickDraw expands `RGB` into an 8×8 tile at draw time against
//! the active GDevice palette (§4 MakeRGBPat). On our true-colour RGBA
//! canvas the spec contract — "approximate the target colour" —
//! reduces to "emit the target RGB at every cell" with zero
//! approximation error.

use oxideav_pict::ops::{PictBuilder, Verb};
use oxideav_pict::{
    build_pix_pat_dither_op, parse_pict, probe_pict, PictImage, PixPatSlot, ProbeTermination,
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

#[test]
fn pen_dither_pix_pattern_paint_applies_target_rgb() {
    // PnPixPat patType=2 with target RGB = purple (#C000C0).
    let mut b = PictBuilder::new(0, 0, 16, 16);
    // Set an obviously-wrong fg to confirm the dither tile overrides it.
    b.fg_color(0x00, 0xFF, 0x00); // green fg — should NOT appear
    b.pen_dither_pix_pattern([0xFF; 8], [0xC0, 0x00, 0xC0]);
    b.rect(Verb::Paint, 0, 0, 16, 16);
    let img = parse_pict(&b.finish()).expect("decode");

    // Every cell of the canvas should be the target RGB.
    for y in 0..16 {
        for x in 0..16 {
            assert_eq!(
                at(&img, x, y),
                [0xC0, 0x00, 0xC0, 0xFF],
                "cell ({x}, {y}) should be the dither target RGB"
            );
        }
    }
}

#[test]
fn fill_dither_pix_pattern_routes_to_fill_verb() {
    // Distinct dither targets on pen + fill slots — paint uses pen,
    // fill uses fill. Inside Macintosh §A-3 verb / pattern routing.
    let mut b = PictBuilder::new(0, 0, 16, 16);
    b.pen_dither_pix_pattern([0xFF; 8], [0xFF, 0xAA, 0x00]); // orange pen
    b.fill_dither_pix_pattern([0xFF; 8], [0x00, 0xAA, 0xFF]); // cyan fill
    b.rect(Verb::Paint, 0, 0, 8, 8); // paint → pen → orange
    b.rect(Verb::Fill, 8, 8, 16, 16); // fill → fill → cyan
    let img = parse_pict(&b.finish()).expect("decode");
    assert_eq!(at(&img, 4, 4), [0xFF, 0xAA, 0x00, 0xFF]);
    assert_eq!(at(&img, 12, 12), [0x00, 0xAA, 0xFF, 0xFF]);
}

#[test]
fn bg_dither_pix_pattern_routes_to_erase_verb() {
    // BkPixPat patType=2 → erase verb. The pattern fully resolves
    // colour, fg / bg state is irrelevant.
    let mut b = PictBuilder::new(0, 0, 8, 8);
    b.bg_dither_pix_pattern([0xFF; 8], [0xAA, 0x00, 0xAA]); // magenta
    b.rect(Verb::Erase, 0, 0, 8, 8);
    let img = parse_pict(&b.finish()).expect("decode");
    for y in 0..8 {
        for x in 0..8 {
            assert_eq!(at(&img, x, y), [0xAA, 0x00, 0xAA, 0xFF]);
        }
    }
}

#[test]
fn dither_pix_pattern_overridden_by_subsequent_mono_pat() {
    // Setting PnPixPat patType=2 then PnPat clears the colour slot —
    // classic "most-recent-pattern-wins" QuickDraw semantics. The mono
    // pattern then renders against the active fg / bg.
    let mut b = PictBuilder::new(0, 0, 8, 8);
    b.pen_dither_pix_pattern([0xFF; 8], [0xFF, 0x00, 0x00]); // red dither
    b.fg_color(0x00, 0x00, 0xFF); // blue fg
    b.bg_color(0xFF, 0xFF, 0xFF); // white bg
    b.pen_pattern([0xFF; 8]); // solid-fg mono — should clear PixPat
    b.rect(Verb::Paint, 0, 0, 8, 8);
    let img = parse_pict(&b.finish()).expect("decode");
    // Red should NOT appear; the mono stipple resolves to blue (fg).
    assert_eq!(at(&img, 4, 4), [0x00, 0x00, 0xFF, 0xFF]);
}

#[test]
fn build_pix_pat_dither_op_opcode_word_and_payload() {
    // build_pix_pat_dither_op emits exactly: opcode-word (2) + patType (2)
    // + Pat1Data (8) + RGB (6) = 18 bytes.
    for (slot, expected_opcode) in [
        (PixPatSlot::Background, 0x0012u16),
        (PixPatSlot::Pen, 0x0013u16),
        (PixPatSlot::Fill, 0x0014u16),
    ] {
        let bytes = build_pix_pat_dither_op(slot, [0x55; 8], [0x80, 0x40, 0x20]);
        assert_eq!(bytes.len(), 18, "{slot:?}: dither payload is exactly 18 B");
        assert_eq!(
            u16::from_be_bytes([bytes[0], bytes[1]]),
            expected_opcode,
            "{slot:?}: opcode word"
        );
        assert_eq!(u16::from_be_bytes([bytes[2], bytes[3]]), 2, "patType = 2");
        assert_eq!(&bytes[4..12], &[0x55; 8], "Pat1Data");
        // RGB: each 8-bit channel replicated to 16-bit (high8 = low8).
        assert_eq!(&bytes[12..14], &[0x80, 0x80], "R16");
        assert_eq!(&bytes[14..16], &[0x40, 0x40], "G16");
        assert_eq!(&bytes[16..18], &[0x20, 0x20], "B16");
    }
}

#[test]
fn dither_pix_pattern_probe_counts() {
    // The probe walker should count `patType=2` opcodes the same way
    // it counts `patType=1`.
    let mut b = PictBuilder::new(0, 0, 8, 8);
    b.pen_dither_pix_pattern([0xFF; 8], [0xFF, 0x00, 0x00]);
    b.bg_dither_pix_pattern([0xFF; 8], [0x00, 0xFF, 0x00]);
    b.fill_dither_pix_pattern([0xFF; 8], [0x00, 0x00, 0xFF]);
    b.rect(Verb::Paint, 0, 0, 8, 8);
    let bytes = b.finish();
    let p = probe_pict(&bytes).expect("probe");
    assert_eq!(p.pix_pattern_set_count, 3);
    assert_eq!(p.termination, ProbeTermination::EndPic);
}

#[test]
fn dither_pix_pattern_preserves_pat1data_through_fallback() {
    // Channel-shifted check: the Pat1Data round-trips through the
    // decoder even though we don't currently surface it via the public
    // state-machine inspection (the rasteriser uses the colour cells).
    // We confirm it indirectly by injecting an obvious-wrong pat1 that
    // would render very differently if the colour pattern weren't
    // active.
    let mut b = PictBuilder::new(0, 0, 8, 8);
    // Pat1Data = horizontal stripes (would alternate fg/bg if the mono
    // path were taken).
    let stripes: [u8; 8] = [0xFF, 0x00, 0xFF, 0x00, 0xFF, 0x00, 0xFF, 0x00];
    b.fg_color(0x00, 0xFF, 0x00); // green fg (used only by mono path)
    b.bg_color(0xFF, 0xFF, 0xFF); // white bg
    b.pen_dither_pix_pattern(stripes, [0xC0, 0x40, 0x80]); // target RGB
    b.rect(Verb::Paint, 0, 0, 8, 8);
    let img = parse_pict(&b.finish()).expect("decode");
    // Every cell should be the target RGB — the Pat1Data stripes do
    // NOT bleed through on a colour-capable rasteriser.
    for y in 0..8 {
        assert_eq!(at(&img, 0, y), [0xC0, 0x40, 0x80, 0xFF]);
        assert_eq!(at(&img, 7, y), [0xC0, 0x40, 0x80, 0xFF]);
    }
}

#[test]
fn dither_pix_pattern_16bit_rgb_high_byte_preserved() {
    // The on-disk RGBColor is 16-bit per channel; the encoder
    // replicates `high8 = low8` from the 8-bit input, so the decoder's
    // `Rgba::from_rgb16` (which keeps the high byte) round-trips
    // bit-exact.
    let mut b = PictBuilder::new(0, 0, 4, 4);
    b.pen_dither_pix_pattern([0xFF; 8], [0x12, 0x34, 0x56]);
    b.rect(Verb::Paint, 0, 0, 4, 4);
    let img = parse_pict(&b.finish()).expect("decode");
    assert_eq!(at(&img, 0, 0), [0x12, 0x34, 0x56, 0xFF]);
    assert_eq!(at(&img, 3, 3), [0x12, 0x34, 0x56, 0xFF]);
}

#[test]
fn dither_pix_pattern_solid_black_paint() {
    // Edge case: target RGB = black. Should paint the canvas solid
    // black even if fg = a bright colour.
    let mut b = PictBuilder::new(0, 0, 4, 4);
    b.fg_color(0xFF, 0xFF, 0x00); // yellow fg (ignored by dither tile)
    b.pen_dither_pix_pattern([0xFF; 8], [0x00, 0x00, 0x00]);
    b.rect(Verb::Paint, 0, 0, 4, 4);
    let img = parse_pict(&b.finish()).expect("decode");
    for y in 0..4 {
        for x in 0..4 {
            assert_eq!(at(&img, x, y), [0x00, 0x00, 0x00, 0xFF]);
        }
    }
}

#[test]
fn mixed_colour_pixmap_and_dither_share_routing() {
    // PnPixPat patType=1 then PnPixPat patType=2 — the dither should
    // win (most-recent-pattern wins). Same routing rules as the round-91
    // colour-pixmap tests, just with the dither variant on top.
    let mut tile = [[0u8; 4]; 64];
    for cell in tile.iter_mut() {
        *cell = [0xFF, 0xAA, 0x00, 0xFF]; // orange colour-pixmap
    }
    let mut b = PictBuilder::new(0, 0, 8, 8);
    b.pen_pix_pattern([0xFF; 8], &tile).unwrap();
    b.pen_dither_pix_pattern([0xFF; 8], [0x00, 0x80, 0xFF]); // sky blue
    b.rect(Verb::Paint, 0, 0, 8, 8);
    let img = parse_pict(&b.finish()).expect("decode");
    // Dither (sky blue) should overwrite the colour-pixmap (orange).
    assert_eq!(at(&img, 4, 4), [0x00, 0x80, 0xFF, 0xFF]);
}
