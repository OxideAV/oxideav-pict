//! Round 6 (workspace round 42) tests — clipping by region, pen-size
//! aware drawing, and `BitsRgn` / `PackBitsRgn` encoders.
//!
//! Three new subsystems land:
//!
//! 1. **Drawing-clipping by region.** The decoder honours `ClipRgn`
//!    (v2 `0x0001`, v1 `0x01`) by materialising the region into a
//!    canvas-local boolean mask. Subsequent drawing primitives and
//!    raster blits are gated through that mask.
//! 2. **Pen-size aware drawing.** `Line` / `LineFrom` /
//!    `Short(Line)From` / `Frame(Rect|Oval)` honour the `PnSize`
//!    state, drawing with a `pen_h × pen_v` brush instead of the
//!    1×1 default.
//! 3. **`BitsRgn` / `PackBitsRgn` encoders.** Mirrors round 5's
//!    `encode_pict_bits_rect` / `encode_pict_pack_bits_rect` but
//!    appends a region after the rect/mode header — the masked
//!    1-bpp BitMap variants.
//!
//! Every test self-roundtrips through `parse_pict`.

use oxideav_pict::ops::{PictBuilder, Verb};
use oxideav_pict::{
    encode_pict_bits_rgn, encode_pict_pack_bits_rgn, encode_pict_v2_with_clip, parse_pict, PackType,
};

// ---------------------------------------------------------------------------
// Drawing-clipping by region.
// ---------------------------------------------------------------------------

/// `encode_pict_v2_with_clip` with a clip strictly smaller than the
/// picture frame — pixels outside the clip should remain at the
/// "paper" colour (white).
#[test]
fn clip_rgn_masks_raster_outside() {
    // 8×8 raster filled with red; clip = inner [2,2,6,6].
    let width = 8u32;
    let height = 8u32;
    let mut rgba = vec![0u8; (width * height * 4) as usize];
    for px in rgba.chunks_exact_mut(4) {
        px[0] = 0xFF;
        px[1] = 0x00;
        px[2] = 0x00;
        px[3] = 0xFF;
    }
    let enc = encode_pict_v2_with_clip(width, height, &rgba, PackType::Raw, [2, 2, 6, 6]).unwrap();
    let img = parse_pict(&enc).unwrap();
    assert_eq!(img.width, width);
    assert_eq!(img.height, height);
    // Inside the clip — red.
    let off = (3 * 8 + 3) * 4;
    assert_eq!(img.data[off], 0xFF, "R inside clip");
    assert_eq!(img.data[off + 1], 0x00, "G inside clip");
    assert_eq!(img.data[off + 2], 0x00, "B inside clip");
    // Outside the clip (top-left corner) — paper white.
    let off = 0;
    assert_eq!(img.data[off], 0xFF, "paper R");
    assert_eq!(img.data[off + 1], 0xFF, "paper G");
    assert_eq!(img.data[off + 2], 0xFF, "paper B");
    // Outside the clip (bottom-right corner) — also paper.
    let off = (7 * 8 + 7) * 4;
    assert_eq!(img.data[off], 0xFF, "br paper R");
    assert_eq!(img.data[off + 1], 0xFF, "br paper G");
    assert_eq!(img.data[off + 2], 0xFF, "br paper B");
}

/// `ClipRgn` followed by drawing primitives — the rectangle's parts
/// outside the clip should remain paper.
#[test]
fn clip_rgn_masks_drawing_primitives() {
    // 16×16 frame; clip = [4,4,12,12]. Draw a paint-rect that spans
    // the entire canvas — only the central 8×8 should turn red.
    let mut b = PictBuilder::new(0, 0, 16, 16);
    b.push(&oxideav_pict::build_clip_rgn_rect(4, 4, 12, 12));
    b.fg_color(0xFF, 0x00, 0x00);
    b.rect(Verb::Paint, 0, 0, 16, 16);
    let bytes = b.finish();
    let img = parse_pict(&bytes).unwrap();
    // Inside clip — red.
    let off = (8 * 16 + 8) * 4;
    assert_eq!(img.data[off], 0xFF);
    assert_eq!(img.data[off + 1], 0x00);
    assert_eq!(img.data[off + 2], 0x00);
    // Outside clip (corner) — paper.
    let off = 0;
    assert_eq!(img.data[off], 0xFF);
    assert_eq!(img.data[off + 1], 0xFF);
    assert_eq!(img.data[off + 2], 0xFF);
    // Just outside the clip's right edge — paper.
    let off = (8 * 16 + 13) * 4;
    assert_eq!(img.data[off], 0xFF);
    assert_eq!(img.data[off + 1], 0xFF);
    assert_eq!(img.data[off + 2], 0xFF);
}

/// Clip narrower than the line should mask the line ends.
#[test]
fn clip_rgn_masks_line() {
    // PictBuilder::new takes (top, left, bottom, right). For a 16×3
    // picture: bottom = 3 (height), right = 16 (width).
    let mut b = PictBuilder::new(0, 0, 3, 16);
    // Clip: rows [0, 3), cols [4, 12).
    b.push(&oxideav_pict::build_clip_rgn_rect(0, 4, 3, 12));
    b.fg_color(0x00, 0x00, 0x00);
    // Line from (h=0, v=1) to (h=15, v=1) — horizontal, mid-row.
    b.line(0, 1, 15, 1);
    let bytes = b.finish();
    let img = parse_pict(&bytes).unwrap();
    let row = 1usize;
    // Inside the clip [4..12) — black.
    for x in 4..12 {
        let off = (row * 16 + x) * 4;
        assert_eq!(img.data[off], 0x00, "x={x} should be inked");
    }
    // Outside the clip — paper.
    for x in 0..4 {
        let off = (row * 16 + x) * 4;
        assert_eq!(img.data[off], 0xFF, "x={x} should be paper");
    }
    for x in 12..16 {
        let off = (row * 16 + x) * 4;
        assert_eq!(img.data[off], 0xFF, "x={x} should be paper");
    }
}

// ---------------------------------------------------------------------------
// Pen-size aware drawing.
// ---------------------------------------------------------------------------

/// `PnSize` of (3, 3) — a horizontal line should ink a 3-pixel-tall
/// ribbon, not a single row.
#[test]
fn pen_size_3x3_horizontal_line() {
    let mut b = PictBuilder::new(0, 0, 8, 16);
    b.fg_color(0x00, 0x00, 0x00);
    b.pen_size(3, 3);
    b.line(2, 4, 14, 4);
    let bytes = b.finish();
    let img = parse_pict(&bytes).unwrap();
    // Rows 4..7 should have black ink across columns 2..14 (the line
    // origin pixel + 3-pixel pen extent).
    let y_lit = 4usize;
    for dy in 0..3 {
        let off = ((y_lit + dy) * 16 + 8) * 4;
        assert_eq!(img.data[off], 0x00, "row {} should be inked", y_lit + dy);
    }
    // Row 3 (above) should still be paper.
    let off = (3 * 16 + 8) * 4;
    assert_eq!(img.data[off], 0xFF, "row 3 should be paper");
    // Row 7 (below 3-pixel extent) should also be paper.
    let off = (7 * 16 + 8) * 4;
    assert_eq!(img.data[off], 0xFF, "row 7 should be paper");
}

/// `PnSize` of (2, 2) — a frame-rect should produce a 2-pixel-thick
/// border instead of the default 1-pixel border.
#[test]
fn pen_size_2x2_frame_rect() {
    let mut b = PictBuilder::new(0, 0, 16, 16);
    b.fg_color(0xFF, 0x00, 0xFF);
    b.pen_size(2, 2);
    b.rect(Verb::Frame, 4, 4, 12, 12);
    let bytes = b.finish();
    let img = parse_pict(&bytes).unwrap();
    // Outer edge (row 4, row 5) should be inked along [4, 12).
    for y in [4, 5] {
        for x in 4..12 {
            let off = (y * 16 + x) * 4;
            assert_eq!(img.data[off], 0xFF, "row={y} col={x} R");
            assert_eq!(img.data[off + 2], 0xFF, "row={y} col={x} B");
        }
    }
    // Interior (row 6, col 6) should still be paper.
    let off = (6 * 16 + 6) * 4;
    assert_eq!(img.data[off], 0xFF);
    assert_eq!(img.data[off + 1], 0xFF);
    assert_eq!(img.data[off + 2], 0xFF);
}

/// Pen size of (1, 1) — output must be identical to a default-pen
/// (no `PnSize` opcode) frame-rect.
#[test]
fn pen_size_1x1_matches_default() {
    let mut a = PictBuilder::new(0, 0, 8, 8);
    a.fg_color(0x00, 0x00, 0xFF);
    a.rect(Verb::Frame, 1, 1, 7, 7);
    let bytes_default = a.finish();
    let img_default = parse_pict(&bytes_default).unwrap();

    let mut b = PictBuilder::new(0, 0, 8, 8);
    b.fg_color(0x00, 0x00, 0xFF);
    b.pen_size(1, 1);
    b.rect(Verb::Frame, 1, 1, 7, 7);
    let bytes_pen = b.finish();
    let img_pen = parse_pict(&bytes_pen).unwrap();

    assert_eq!(img_default.data, img_pen.data);
}

// ---------------------------------------------------------------------------
// BitsRgn / PackBitsRgn encoders.
// ---------------------------------------------------------------------------

/// `BitsRgn` opcode at offset 552 (after stub + headerOp).
#[test]
fn bits_rgn_emits_bits_rgn_opcode() {
    let rgba = vec![0u8; 8 * 8 * 4];
    let enc = encode_pict_bits_rgn(8, 8, &rgba, [0, 0, 8, 8]).unwrap();
    let pos = 552usize;
    assert_eq!(enc[pos], 0x00, "high byte of BitsRgn opcode");
    assert_eq!(enc[pos + 1], 0x91, "low byte of BitsRgn opcode");
}

/// `PackBitsRgn` opcode at offset 552.
#[test]
fn pack_bits_rgn_emits_pack_bits_rgn_opcode() {
    let rgba = vec![0u8; 64 * 8 * 4]; // wide enough for RLE path
    let enc = encode_pict_pack_bits_rgn(64, 8, &rgba, [0, 0, 8, 64]).unwrap();
    let pos = 552usize;
    assert_eq!(enc[pos], 0x00, "high byte of PackBitsRgn opcode");
    assert_eq!(enc[pos + 1], 0x99, "low byte of PackBitsRgn opcode");
}

/// `BitsRgn` round-trip with the clip = full canvas.
#[test]
fn bits_rgn_full_clip_roundtrip() {
    // 8×8 black image (all bits set).
    let rgba = vec![0u8; 8 * 8 * 4];
    let enc = encode_pict_bits_rgn(8, 8, &rgba, [0, 0, 8, 8]).unwrap();
    let img = parse_pict(&enc).unwrap();
    assert_eq!(img.width, 8);
    assert_eq!(img.height, 8);
    // All pixels should be black (bit=1 → 0x00).
    for i in 0..64 {
        let off = i * 4;
        assert_eq!(img.data[off], 0x00);
        assert_eq!(img.data[off + 1], 0x00);
        assert_eq!(img.data[off + 2], 0x00);
    }
}

/// `PackBitsRgn` round-trip with a wide image — exercises the RLE
/// branch (rowBytes >= 8 → rowBytes = 8 here) and the clip handler.
#[test]
fn pack_bits_rgn_wide_clip_roundtrip() {
    // 64×8 white image (all bits clear → all white).
    let rgba = vec![0xFFu8; 64 * 8 * 4];
    let enc = encode_pict_pack_bits_rgn(64, 8, &rgba, [0, 0, 8, 64]).unwrap();
    let img = parse_pict(&enc).unwrap();
    assert_eq!(img.width, 64);
    assert_eq!(img.height, 8);
    // All pixels should be white.
    for i in 0..(64 * 8) {
        let off = i * 4;
        assert_eq!(img.data[off], 0xFF);
        assert_eq!(img.data[off + 1], 0xFF);
        assert_eq!(img.data[off + 2], 0xFF);
    }
}

/// `BitsRgn` with a clip narrower than the bitmap masks pixels
/// outside.
#[test]
fn bits_rgn_narrow_clip_masks() {
    // 8×8 black image; clip = inner [2,2,6,6]. Pixels outside the
    // clip should be paper white; inside should be black.
    let rgba = vec![0u8; 8 * 8 * 4];
    let enc = encode_pict_bits_rgn(8, 8, &rgba, [2, 2, 6, 6]).unwrap();
    let img = parse_pict(&enc).unwrap();
    // Inside the clip — black.
    let off = (3 * 8 + 3) * 4;
    assert_eq!(img.data[off], 0x00, "inside clip should be black");
    // Outside the clip — paper white.
    let off = 0;
    assert_eq!(img.data[off], 0xFF, "outside clip should be paper");
    let off = (7 * 8 + 7) * 4;
    assert_eq!(img.data[off], 0xFF, "br outside clip should be paper");
}

/// `BitsRgn` with degenerate input — size mismatch must reject.
#[test]
fn bits_rgn_rejects_size_mismatch() {
    assert!(encode_pict_bits_rgn(2, 2, &[0u8; 7], [0, 0, 2, 2]).is_err());
}

#[test]
fn pack_bits_rgn_rejects_size_mismatch() {
    assert!(encode_pict_pack_bits_rgn(2, 2, &[0u8; 7], [0, 0, 2, 2]).is_err());
}
