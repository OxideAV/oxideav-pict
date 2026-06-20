//! Built-in clean-room bitmap glyph set + QuickDraw text rasteriser.
//!
//! PICT files do **not** embed font data. A text-drawing opcode
//! (`LongText` `$0028`, `DHText` `$0029`, `DVText` `$002A`, `DHDVText`
//! `$002B`) carries only the text *bytes* plus a pen position; the glyph
//! pixels themselves are supplied at draw time by the classic-Mac Font
//! Manager from whichever installed system font the `txFont` number
//! selects. Inside Macintosh: Imaging With QuickDraw is explicit that the
//! glyph bitmaps,
//! the per-font character widths, and the bold/italic/outline style
//! synthesis all live in a separate book — "the chapter 'Font Manager'
//! in Inside Macintosh: Text" (book page 2-34) — which is **not** part of
//! this crate's clean-room reference set. None of the four Inside
//! Macintosh volumes shipped in `docs/image/quickdraw/` carry a
//! machine-readable Font Manager / `NFNT` strike-format spec.
//!
//! So pixel-for-pixel reproduction of a particular Mac system font is
//! out of scope: there is no public spec for it in-tree. What *is* fully
//! spec-determined — and what this module implements — is QuickDraw's
//! **text-drawing geometry model**:
//!
//! * the baseline sits at the pen location (Imaging With QuickDraw,
//!   "About Basic QuickDraw", book page 2-13);
//! * `txSize` is the cell height in pixels — `point × resolution / 72`
//!   (book page 2-34);
//! * `fgColor` is the ink the glyphs are painted with (book page 2-34);
//! * text honours the Boolean source modes `srcOr` / `srcXor` / `srcBic`
//!   (book page 2-34, "Only three source modes … should be used for
//!   drawing text");
//! * the pen advances rightward by each glyph's width as the string is
//!   drawn, plus the `chExtra` per-character and `spExtra` per-space
//!   adjustments (book pages 2-33/2-34, §A-3 `$0006`/`$0016`).
//!
//! The glyph *shapes* below are an original, crate-authored 5×7 ASCII
//! face — not transcribed from, traced from, or measured against any
//! Apple font or any other implementation. They exist solely so that the
//! spec-determined geometry produces visible, legible pixels rather than
//! being walked past. Each character is one `u8` row mask per scanline
//! (bit 4 = leftmost of the 5 columns); the design cell is 5 wide × 7
//! tall on a 1-pixel right-hand advance gutter (6-px nominal advance at
//! `txSize == DESIGN_EM`).

use crate::raster::{blend_source, Canvas, SourceMode};
use crate::state::Rgba;

/// Design grid: glyph artwork is authored on a 5-wide × 7-tall cell.
pub const GLYPH_W: i32 = 5;
pub const GLYPH_H: i32 = 7;
/// The nominal advance (glyph width + 1-px inter-glyph gutter) at the
/// design size, in design pixels.
pub const ADVANCE: i32 = GLYPH_W + 1;
/// The em the artwork is authored at. A `txSize` equal to this draws the
/// glyphs at native scale; other sizes scale by `txSize / DESIGN_EM`.
/// The cell is 7 tall with the baseline one row above the bottom, so the
/// em (ascender-to-descender box) maps to the full 8-px advance height.
pub const DESIGN_EM: i32 = 8;
/// Rows of the design cell that sit *above* the baseline. The bottom row
/// (index 6) is the baseline row; index 7 (one below) is descender room.
pub const BASELINE_ROW: i32 = 6;

/// 5×7 glyph bitmaps for ASCII `0x20..=0x7E`. Index `c - 0x20`; each
/// entry is 7 row masks, MSB-of-low-5-bits = leftmost column.
///
/// Crate-authored artwork (see module docs): no external font consulted.
#[rustfmt::skip]
const GLYPHS: [[u8; 7]; 0x5F] = [
    [0b00000,0b00000,0b00000,0b00000,0b00000,0b00000,0b00000], // 0x20 space
    [0b00100,0b00100,0b00100,0b00100,0b00100,0b00000,0b00100], // !
    [0b01010,0b01010,0b01010,0b00000,0b00000,0b00000,0b00000], // "
    [0b01010,0b11111,0b01010,0b01010,0b11111,0b01010,0b00000], // #
    [0b00100,0b01111,0b10100,0b01110,0b00101,0b11110,0b00100], // $
    [0b11000,0b11001,0b00010,0b00100,0b01001,0b10011,0b00000], // %
    [0b01100,0b10010,0b10100,0b01000,0b10101,0b10010,0b01101], // &
    [0b00100,0b00100,0b00100,0b00000,0b00000,0b00000,0b00000], // '
    [0b00010,0b00100,0b01000,0b01000,0b01000,0b00100,0b00010], // (
    [0b01000,0b00100,0b00010,0b00010,0b00010,0b00100,0b01000], // )
    [0b00000,0b00100,0b10101,0b01110,0b10101,0b00100,0b00000], // *
    [0b00000,0b00100,0b00100,0b11111,0b00100,0b00100,0b00000], // +
    [0b00000,0b00000,0b00000,0b00000,0b00100,0b00100,0b01000], // ,
    [0b00000,0b00000,0b00000,0b11111,0b00000,0b00000,0b00000], // -
    [0b00000,0b00000,0b00000,0b00000,0b00000,0b00100,0b00100], // .
    [0b00001,0b00010,0b00100,0b00100,0b01000,0b10000,0b00000], // /
    [0b01110,0b10001,0b10011,0b10101,0b11001,0b10001,0b01110], // 0
    [0b00100,0b01100,0b00100,0b00100,0b00100,0b00100,0b01110], // 1
    [0b01110,0b10001,0b00001,0b00110,0b01000,0b10000,0b11111], // 2
    [0b11111,0b00010,0b00100,0b00010,0b00001,0b10001,0b01110], // 3
    [0b00010,0b00110,0b01010,0b10010,0b11111,0b00010,0b00010], // 4
    [0b11111,0b10000,0b11110,0b00001,0b00001,0b10001,0b01110], // 5
    [0b00110,0b01000,0b10000,0b11110,0b10001,0b10001,0b01110], // 6
    [0b11111,0b00001,0b00010,0b00100,0b01000,0b01000,0b01000], // 7
    [0b01110,0b10001,0b10001,0b01110,0b10001,0b10001,0b01110], // 8
    [0b01110,0b10001,0b10001,0b01111,0b00001,0b00010,0b01100], // 9
    [0b00000,0b00100,0b00100,0b00000,0b00100,0b00100,0b00000], // :
    [0b00000,0b00100,0b00100,0b00000,0b00100,0b00100,0b01000], // ;
    [0b00010,0b00100,0b01000,0b10000,0b01000,0b00100,0b00010], // <
    [0b00000,0b00000,0b11111,0b00000,0b11111,0b00000,0b00000], // =
    [0b01000,0b00100,0b00010,0b00001,0b00010,0b00100,0b01000], // >
    [0b01110,0b10001,0b00001,0b00110,0b00100,0b00000,0b00100], // ?
    [0b01110,0b10001,0b10111,0b10101,0b10111,0b10000,0b01110], // @
    [0b01110,0b10001,0b10001,0b11111,0b10001,0b10001,0b10001], // A
    [0b11110,0b10001,0b10001,0b11110,0b10001,0b10001,0b11110], // B
    [0b01110,0b10001,0b10000,0b10000,0b10000,0b10001,0b01110], // C
    [0b11100,0b10010,0b10001,0b10001,0b10001,0b10010,0b11100], // D
    [0b11111,0b10000,0b10000,0b11110,0b10000,0b10000,0b11111], // E
    [0b11111,0b10000,0b10000,0b11110,0b10000,0b10000,0b10000], // F
    [0b01110,0b10001,0b10000,0b10111,0b10001,0b10001,0b01111], // G
    [0b10001,0b10001,0b10001,0b11111,0b10001,0b10001,0b10001], // H
    [0b01110,0b00100,0b00100,0b00100,0b00100,0b00100,0b01110], // I
    [0b00111,0b00010,0b00010,0b00010,0b00010,0b10010,0b01100], // J
    [0b10001,0b10010,0b10100,0b11000,0b10100,0b10010,0b10001], // K
    [0b10000,0b10000,0b10000,0b10000,0b10000,0b10000,0b11111], // L
    [0b10001,0b11011,0b10101,0b10101,0b10001,0b10001,0b10001], // M
    [0b10001,0b11001,0b10101,0b10011,0b10001,0b10001,0b10001], // N
    [0b01110,0b10001,0b10001,0b10001,0b10001,0b10001,0b01110], // O
    [0b11110,0b10001,0b10001,0b11110,0b10000,0b10000,0b10000], // P
    [0b01110,0b10001,0b10001,0b10001,0b10101,0b10010,0b01101], // Q
    [0b11110,0b10001,0b10001,0b11110,0b10100,0b10010,0b10001], // R
    [0b01111,0b10000,0b10000,0b01110,0b00001,0b00001,0b11110], // S
    [0b11111,0b00100,0b00100,0b00100,0b00100,0b00100,0b00100], // T
    [0b10001,0b10001,0b10001,0b10001,0b10001,0b10001,0b01110], // U
    [0b10001,0b10001,0b10001,0b10001,0b10001,0b01010,0b00100], // V
    [0b10001,0b10001,0b10001,0b10101,0b10101,0b10101,0b01010], // W
    [0b10001,0b10001,0b01010,0b00100,0b01010,0b10001,0b10001], // X
    [0b10001,0b10001,0b01010,0b00100,0b00100,0b00100,0b00100], // Y
    [0b11111,0b00001,0b00010,0b00100,0b01000,0b10000,0b11111], // Z
    [0b01110,0b01000,0b01000,0b01000,0b01000,0b01000,0b01110], // [
    [0b10000,0b01000,0b00100,0b00100,0b00010,0b00001,0b00000], // backslash
    [0b01110,0b00010,0b00010,0b00010,0b00010,0b00010,0b01110], // ]
    [0b00100,0b01010,0b10001,0b00000,0b00000,0b00000,0b00000], // ^
    [0b00000,0b00000,0b00000,0b00000,0b00000,0b00000,0b11111], // _
    [0b01000,0b00100,0b00010,0b00000,0b00000,0b00000,0b00000], // `
    [0b00000,0b00000,0b01110,0b00001,0b01111,0b10001,0b01111], // a
    [0b10000,0b10000,0b10110,0b11001,0b10001,0b10001,0b11110], // b
    [0b00000,0b00000,0b01110,0b10000,0b10000,0b10001,0b01110], // c
    [0b00001,0b00001,0b01101,0b10011,0b10001,0b10001,0b01111], // d
    [0b00000,0b00000,0b01110,0b10001,0b11111,0b10000,0b01110], // e
    [0b00110,0b01001,0b01000,0b11100,0b01000,0b01000,0b01000], // f
    [0b00000,0b01111,0b10001,0b10001,0b01111,0b00001,0b01110], // g
    [0b10000,0b10000,0b10110,0b11001,0b10001,0b10001,0b10001], // h
    [0b00100,0b00000,0b01100,0b00100,0b00100,0b00100,0b01110], // i
    [0b00010,0b00000,0b00110,0b00010,0b00010,0b10010,0b01100], // j
    [0b10000,0b10000,0b10010,0b10100,0b11000,0b10100,0b10010], // k
    [0b01100,0b00100,0b00100,0b00100,0b00100,0b00100,0b01110], // l
    [0b00000,0b00000,0b11010,0b10101,0b10101,0b10001,0b10001], // m
    [0b00000,0b00000,0b10110,0b11001,0b10001,0b10001,0b10001], // n
    [0b00000,0b00000,0b01110,0b10001,0b10001,0b10001,0b01110], // o
    [0b00000,0b00000,0b11110,0b10001,0b11110,0b10000,0b10000], // p
    [0b00000,0b00000,0b01101,0b10011,0b01111,0b00001,0b00001], // q
    [0b00000,0b00000,0b10110,0b11001,0b10000,0b10000,0b10000], // r
    [0b00000,0b00000,0b01111,0b10000,0b01110,0b00001,0b11110], // s
    [0b01000,0b01000,0b11100,0b01000,0b01000,0b01001,0b00110], // t
    [0b00000,0b00000,0b10001,0b10001,0b10001,0b10011,0b01101], // u
    [0b00000,0b00000,0b10001,0b10001,0b10001,0b01010,0b00100], // v
    [0b00000,0b00000,0b10001,0b10001,0b10101,0b10101,0b01010], // w
    [0b00000,0b00000,0b10001,0b01010,0b00100,0b01010,0b10001], // x
    [0b00000,0b00000,0b10001,0b10001,0b01111,0b00001,0b01110], // y
    [0b00000,0b00000,0b11111,0b00010,0b00100,0b01000,0b11111], // z
    [0b00010,0b00100,0b00100,0b01000,0b00100,0b00100,0b00010], // {
    [0b00100,0b00100,0b00100,0b00100,0b00100,0b00100,0b00100], // |
    [0b01000,0b00100,0b00100,0b00010,0b00100,0b00100,0b01000], // }
    [0b00000,0b00000,0b01000,0b10101,0b00010,0b00000,0b00000], // ~
];

/// Return the 7-row glyph bitmap for byte `b`. Bytes outside the
/// authored `0x20..=0x7E` range map to a filled box (the QuickDraw
/// "missing symbol" notion — Imaging With QuickDraw, book page 2-33:
/// *"each font contains a missing symbol to be drawn in case of a
/// request to draw a character that's missing from the font"*).
fn glyph(b: u8) -> [u8; 7] {
    if (0x20..=0x7E).contains(&b) {
        GLYPHS[(b - 0x20) as usize]
    } else {
        // Missing-symbol: a hollow box spanning the cell.
        [
            0b11111, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11111,
        ]
    }
}

/// The horizontal advance, in *design* pixels, for byte `b`. Every glyph
/// in this fixed face advances by [`ADVANCE`]; the value is broken out so
/// the renderer's pen-advance maths reads as the spec's "pen advances by
/// the character's width" rather than a magic constant.
#[inline]
pub fn char_advance_design(_b: u8) -> i32 {
    ADVANCE
}

/// Scale a design-space length by the active `txSize`. `txSize` is the
/// cell height in pixels (book page 2-34: `point × resolution / 72`); the
/// artwork's em is [`DESIGN_EM`] pixels, so the scale is
/// `txSize / DESIGN_EM`, rounded to nearest with a 1-px floor so a glyph
/// never collapses to nothing.
#[inline]
fn scale_design(len: i32, tx_size: i32) -> i32 {
    if tx_size <= 0 {
        return len;
    }
    let scaled = (len * tx_size + DESIGN_EM / 2) / DESIGN_EM;
    scaled.max(1)
}

/// Total advance width, in **canvas pixels**, that drawing `text` at the
/// given `tx_size` would consume — the sum of per-glyph advances plus the
/// `ch_extra` per-character and `sp_extra` per-space adjustments (§A-3
/// `$0016` / `$0006`; Imaging With QuickDraw book page 2-34). Used to move
/// the running text pen after a draw so successive `DH/DV/DHDVText`
/// opcodes on the same line land correctly.
pub fn measure_text(text: &[u8], tx_size: i32, ch_extra: i32, sp_extra: i32) -> i32 {
    let mut adv = 0i32;
    for &b in text {
        adv += scale_design(char_advance_design(b), tx_size);
        adv += ch_extra;
        if b == b' ' {
            adv += sp_extra;
        }
    }
    adv
}

/// Draw `text` onto `canvas` with its baseline-left origin at canvas-local
/// `(pen_x, pen_y)`, in foreground ink `fg`, against background `bg`,
/// using the QuickDraw source `mode`. Returns the total advance width in
/// canvas pixels (same value [`measure_text`] would compute), so the
/// caller can move the text pen.
///
/// The baseline rule (book page 2-13) places row [`BASELINE_ROW`] of the
/// design cell on the pen's `y`; rows above the baseline are drawn at
/// `pen_y - (BASELINE_ROW - row)·scale`, the one descender row below at
/// `pen_y + scale`. `txSize` scales the cell isotropically.
#[allow(clippy::too_many_arguments)]
pub fn draw_text(
    canvas: &mut Canvas,
    text: &[u8],
    pen_x: i32,
    pen_y: i32,
    tx_size: i32,
    ch_extra: i32,
    sp_extra: i32,
    fg: Rgba,
    bg: Rgba,
    mode: SourceMode,
) -> i32 {
    let mut x = pen_x;
    let s = |len: i32| scale_design(len, tx_size);
    for &b in text {
        let bmp = glyph(b);
        let scale = s(1);
        for (row_idx, &rowmask) in bmp.iter().enumerate() {
            if rowmask == 0 {
                continue;
            }
            // Vertical placement: the baseline (book page 2-13) sits on
            // `pen_y`, so row `BASELINE_ROW`'s scaled block ends exactly
            // at the pen line. Rows above the baseline stack upward; the
            // single row below (index 7 would be descender, but the cell
            // is 7 rows so the deepest authored row is the baseline) keeps
            // its block bottom on the baseline too.
            let rows_above_baseline = BASELINE_ROW - row_idx as i32;
            let y_top = pen_y - s(rows_above_baseline + 1) + 1;
            for col in 0..GLYPH_W {
                let on = (rowmask >> (GLYPH_W - 1 - col)) & 1 != 0;
                // For a glyph the "source image" is black on the on-bits
                // and white on the off-bits; blend_source maps that to the
                // active text mode. srcOr (the visible text mode) leaves
                // off-bits transparent, so we only need to touch on-bits.
                let src = if on { Rgba::BLACK } else { Rgba::WHITE };
                // Skip pure off-pixels for the common transparent modes to
                // avoid pointless work; srcCopy/notSrc* DO paint them.
                if !on
                    && matches!(
                        mode,
                        SourceMode::SrcOr | SourceMode::SrcBic | SourceMode::SrcXor
                    )
                {
                    continue;
                }
                let bx = x + s(col);
                for dy in 0..scale {
                    let py = y_top + dy;
                    for dx in 0..scale {
                        let px = bx + dx;
                        let dst = canvas.pixel_at(px, py).unwrap_or(bg);
                        let out = blend_source(mode, src, dst, fg, bg);
                        canvas.put(px, py, out);
                    }
                }
            }
        }
        x += s(char_advance_design(b)) + ch_extra;
        if b == b' ' {
            x += sp_extra;
        }
    }
    x - pen_x
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_table_is_complete() {
        // Every printable ASCII byte resolves to a non-trivial glyph
        // (space is intentionally blank; everything else has ink).
        for b in 0x21u8..=0x7E {
            let g = glyph(b);
            assert!(g.iter().any(|&r| r != 0), "glyph {b:#x} is blank");
        }
        assert!(glyph(b' ').iter().all(|&r| r == 0));
    }

    #[test]
    fn missing_symbol_for_out_of_range() {
        // High bytes / control bytes get the missing-symbol box.
        let g = glyph(0xC9);
        assert_eq!(g[0], 0b11111);
        assert_eq!(g[6], 0b11111);
    }

    #[test]
    fn measure_scales_with_size() {
        let m8 = measure_text(b"AB", 8, 0, 0);
        let m16 = measure_text(b"AB", 16, 0, 0);
        assert_eq!(m8, 2 * ADVANCE);
        assert_eq!(m16, 2 * ADVANCE * 2);
    }

    #[test]
    fn measure_includes_extras() {
        let base = measure_text(b"a a", 8, 0, 0);
        let with_ch = measure_text(b"a a", 8, 2, 0);
        let with_sp = measure_text(b"a a", 8, 0, 3);
        assert_eq!(with_ch, base + 3 * 2); // 3 chars × chExtra 2
        assert_eq!(with_sp, base + 3); // one space × spExtra 3
    }

    #[test]
    fn draw_paints_ink_and_advances() {
        let mut c = Canvas::new(40, 20, Rgba::WHITE);
        let adv = draw_text(
            &mut c,
            b"A",
            2,
            10,
            8,
            0,
            0,
            Rgba::BLACK,
            Rgba::WHITE,
            SourceMode::SrcOr,
        );
        assert_eq!(adv, ADVANCE);
        // Some pixel in the glyph cell must be black ink now.
        let mut inked = false;
        for y in 0..20 {
            for x in 0..40 {
                let p = c.pixel_at(x, y).unwrap();
                if p == Rgba::BLACK {
                    inked = true;
                }
            }
        }
        assert!(inked, "draw_text painted no ink");
    }

    #[test]
    fn space_paints_no_ink_in_or_mode() {
        let mut c = Canvas::new(40, 20, Rgba::WHITE);
        draw_text(
            &mut c,
            b" ",
            2,
            10,
            8,
            0,
            0,
            Rgba::BLACK,
            Rgba::WHITE,
            SourceMode::SrcOr,
        );
        // No black pixels — a space in srcOr leaves the canvas untouched.
        for y in 0..20 {
            for x in 0..40 {
                assert_ne!(c.pixel_at(x, y).unwrap(), Rgba::BLACK);
            }
        }
    }
}
