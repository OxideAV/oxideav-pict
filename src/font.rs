//! Built-in clean-room bitmap glyph set + QuickDraw text rasteriser.
//!
//! PICT files do **not** embed font data. A text-drawing opcode
//! (`LongText` `$0028`, `DHText` `$0029`, `DVText` `$002A`, `DHDVText`
//! `$002B`) carries only the text *bytes* plus a pen position; the glyph
//! pixels themselves are supplied at draw time by the classic-Mac Font
//! Manager from whichever installed system font the `txFont` number
//! selects. The reference set carries no per-font glyph artwork (the
//! `NFNT` strike *format* is described in Inside Macintosh Volume I's
//! Font Manager chapter, but the actual system-font bitmaps live in Mac
//! resource files, not in any spec), so pixel-for-pixel reproduction of
//! a particular Mac system font is out of scope.
//!
//! What *is* fully spec-determined — and what this module implements —
//! is QuickDraw's **text-drawing geometry model**:
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
//!
//! ## `txFace` style synthesis (round 407)
//!
//! The re-staged Inside Macintosh Volume I (1985) specifies how
//! QuickDraw synthesises the `Style` variations from the plain glyph
//! bitmap (QuickDraw chapter, pages I-151/I-152):
//!
//! * **bold** — *"each character is repeatedly drawn one bit to the
//!   right an appropriate number of times for extra thickness"*;
//! * **italic** — *"Character bits above the base line are skewed
//!   right; bits below the base line are skewed left"*;
//! * **underline** — *"draws a line below the base line of the
//!   characters. If part of a character descends below the base line
//!   … the underline isn't drawn through the pixel on either side of
//!   the descending part"*;
//! * **outline** — *"makes a hollow, outlined character rather than a
//!   solid one"*;
//! * **shadow** — *"also makes an outlined character, but the outline
//!   is thickened below and to the right of the character to achieve
//!   the effect of a shadow. If you specify bold along with outline or
//!   shadow, the hollow part of the character is widened"*;
//! * **condense / extend** — *"affect the horizontal distance between
//!   all characters, including spaces"* (decrease / increase).
//!
//! The concrete per-style amounts come from the Font Manager chapter's
//! **font characterization table** for the screen device (Volume I,
//! page I-226, Figure 4): bold `0,1,1`; italic `1,8,0`; outline
//! `5,1,1`; shadow `5,2,2`; condensed `0,0,-1`; extended `0,0,1`;
//! underline `1,1,1` — each triplet naming the affected `FMOutput`
//! style field, the value stored into it, and the increment to the
//! `extra` character-widening field (page I-227). See [`StyleParams`]
//! for how this crate applies them.

use crate::raster::{blend_source, Canvas, SourceMode};
use crate::state::{PictTextFace, Rgba};

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

/// Anisotropic glyph scale: `txSize` plus the `TxRatio` (`$0010`)
/// horizontal / vertical numerator-over-denominator pair.
///
/// Imaging With QuickDraw (book page 12-13, `DrawJustified` / `StdText`
/// scaling): *"numer.v over denom.v gives the vertical scaling, and
/// numer.h over denom.h gives the horizontal scaling factor."* So a
/// design-space length `len` maps to
///
/// * horizontal canvas pixels: `len · txSize/DESIGN_EM · numer_h/denom_h`
/// * vertical canvas pixels:   `len · txSize/DESIGN_EM · numer_v/denom_v`
///
/// A `TxRatio` of `1/1` on both axes (the §A-3 fresh-GrafPort default)
/// reduces to the isotropic `txSize / DESIGN_EM` scale. Denominators of
/// zero (a malformed record) are clamped to `1` so the ratio can never
/// divide by zero or collapse a glyph to nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextScale {
    /// Cell height in pixels; the artwork em is [`DESIGN_EM`].
    pub tx_size: i32,
    /// `TxRatio` horizontal numerator (`numer.h`).
    pub numer_h: i32,
    /// `TxRatio` horizontal denominator (`denom.h`).
    pub denom_h: i32,
    /// `TxRatio` vertical numerator (`numer.v`).
    pub numer_v: i32,
    /// `TxRatio` vertical denominator (`denom.v`).
    pub denom_v: i32,
}

impl TextScale {
    /// The `txSize`-only scale used when no `TxRatio` is in force (both
    /// axes `1/1`).
    #[inline]
    pub const fn isotropic(tx_size: i32) -> Self {
        Self {
            tx_size,
            numer_h: 1,
            denom_h: 1,
            numer_v: 1,
            denom_v: 1,
        }
    }

    /// Scale a design-space length along the horizontal axis, in canvas
    /// pixels (`len · txSize/DESIGN_EM · numer_h/denom_h`).
    #[inline]
    pub fn h(&self, len: i32) -> i32 {
        ratio_scale(len, self.tx_size, self.numer_h, self.denom_h)
    }

    /// Scale a design-space length along the vertical axis, in canvas
    /// pixels (`len · txSize/DESIGN_EM · numer_v/denom_v`).
    #[inline]
    pub fn v(&self, len: i32) -> i32 {
        ratio_scale(len, self.tx_size, self.numer_v, self.denom_v)
    }

    /// Scale a horizontal design-space **offset** (a position, not a
    /// block size): identical rounding to [`TextScale::h`] but without
    /// the 1-px floor, so `h_off(0) == 0` and distinct design columns
    /// map to distinct canvas positions. The floor in [`TextScale::h`]
    /// exists so a *block* never collapses to nothing; applied to
    /// offsets it would fold design columns 0 and 1 onto the same
    /// canvas column (round 407 fix).
    #[inline]
    pub fn h_off(&self, len: i32) -> i32 {
        offset_scale(len, self.tx_size, self.numer_h, self.denom_h)
    }

    /// Scale a vertical design-space **offset** — see [`TextScale::h_off`].
    #[inline]
    pub fn v_off(&self, len: i32) -> i32 {
        offset_scale(len, self.tx_size, self.numer_v, self.denom_v)
    }
}

/// Scale `len` design pixels by `txSize/DESIGN_EM · numer/denom`, rounded
/// to nearest with a 1-px floor so a glyph never collapses to nothing.
/// `txSize <= 0` falls back to the raw `len · numer/denom` (no size
/// scaling); a non-positive denominator is treated as `1`.
#[inline]
fn ratio_scale(len: i32, tx_size: i32, numer: i32, denom: i32) -> i32 {
    let denom = if denom <= 0 { 1 } else { denom };
    let numer = numer.max(0);
    let size = if tx_size <= 0 { DESIGN_EM } else { tx_size };
    // Compute in i64 so a large txSize × ratio can't overflow i32.
    let num = len as i64 * size as i64 * numer as i64;
    let den = DESIGN_EM as i64 * denom as i64;
    let scaled = (num + den / 2) / den;
    // Saturating narrow: a hostile txSize × TxRatio product can exceed
    // i32 (round 407 hardening — the wrap would turn a huge cell into a
    // negative offset).
    (scaled.clamp(i32::MIN as i64, i32::MAX as i64) as i32).max(1)
}

/// [`ratio_scale`] without the 1-px floor, for scaling positions rather
/// than block sizes (`offset_scale(0) == 0`). Negative lengths mirror
/// symmetrically.
#[inline]
fn offset_scale(len: i32, tx_size: i32, numer: i32, denom: i32) -> i32 {
    let denom = if denom <= 0 { 1 } else { denom };
    let numer = numer.max(0);
    let size = if tx_size <= 0 { DESIGN_EM } else { tx_size };
    let num = len.unsigned_abs() as i64 * size as i64 * numer as i64;
    let den = DESIGN_EM as i64 * denom as i64;
    // Saturating narrow — see `ratio_scale`.
    let scaled = ((num + den / 2) / den).min(i32::MAX as i64) as i32;
    if len < 0 {
        -scaled
    } else {
        scaled
    }
}

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

/// Concrete `txFace` style-synthesis amounts, derived from the classic
/// Font Manager **font characterization table** for the screen device
/// (Inside Macintosh Volume I, page I-226 Figure 4, applied per the
/// triplet mechanism described on page I-227).
///
/// Each active style bit contributes its table triplet, in table order:
///
/// | style     | triplet   | effect here                                |
/// | --------- | --------- | ------------------------------------------ |
/// | bold      | `0, 1, 1` | `bold = 1` smear pass, `extra += 1`        |
/// | italic    | `1, 8, 0` | `italic = 8` shear factor                  |
/// | outline   | `5, 1, 1` | `shadow = 1` ring, `extra += 1`            |
/// | shadow    | `5, 2, 2` | `shadow = 2` thickened ring, `extra += 2`  |
/// | condensed | `0, 0,-1` | `extra -= 1`                               |
/// | extended  | `0, 0, 1` | `extra += 1`                               |
/// | underline | `1, 1, 1` | `ul_offset/ul_shadow/ul_thick = 1`         |
///
/// Notes on the two table subtleties:
///
/// * Both *outline* and *shadow* store into the `FMOutput` **shadow**
///   field (byte 5 beyond `bold`) — outline stores `1`, shadow stores
///   `2` — so a single `shadow` factor drives both treatments: `1`
///   draws the plain 1-pixel ring, `2` additionally thickens the ring
///   below and to the right (page I-152). When both bits are set the
///   later (shadow) store wins the field while both `extra` increments
///   apply, per the "starting from 0" accumulation rule on page I-227.
/// * *Condensed* / *extended* name field byte `0` with amount `0` — a
///   zero store carries no styling of its own (the table has no other
///   way to express "no field affected"), so this crate treats it as a
///   no-op rather than a destructive overwrite of an active bold
///   factor; only their `extra` increments apply. Volume I explicitly
///   leaves the fields' exact use implementation-defined (page I-227:
///   *"You'll need to experiment with these values"*).
///
/// The `extra` field accumulates across active styles — Volume I's own
/// worked example (page I-227): *"the extra field for bold shadowed
/// characters would be 3."*
///
/// All amounts are in **design pixels** and scale with the glyph cell
/// (`txSize` × `TxRatio`), so styled text stays proportionate at every
/// size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StyleParams {
    /// Bold smear passes (each ORs the glyph shifted one design pixel
    /// right — page I-152).
    pub bold: i32,
    /// Italic shear factor: a row `d` design-rows from the baseline
    /// shifts by `(d · italic) >> 4` design pixels — right above the
    /// baseline, left below it (page I-152). The screen table's factor
    /// of `8` therefore shears one pixel per two rows; Volume I leaves
    /// the factor's exact unit to the implementation (page I-227), and
    /// this crate fixes it as a `>> 4` slope so the table value maps to
    /// a deterministic, monotone shear.
    pub italic: i32,
    /// Underline distance below the baseline, in design rows.
    pub ul_offset: i32,
    /// Underline gap on either side of descender ink, in design pixels.
    pub ul_shadow: i32,
    /// Underline thickness, in design rows.
    pub ul_thick: i32,
    /// Outline/shadow ring factor: `0` solid, `1` hollow 1-pixel ring,
    /// `2` ring additionally thickened below/right (pages I-152/I-226).
    pub shadow: i32,
    /// Extra character-advance widening, in design pixels (may be
    /// negative for condensed text).
    pub extra: i32,
}

impl StyleParams {
    /// Derive the synthesis amounts for a `txFace` style byte per the
    /// Volume I screen characterization table (see type docs).
    pub fn from_face(face: PictTextFace) -> Self {
        let mut p = Self::default();
        // Triplets applied in Figure 4 table order (page I-226).
        if face.bold() {
            p.bold = 1;
            p.extra += 1;
        }
        if face.italic() {
            p.italic = 8;
        }
        if face.outline() {
            p.shadow = 1;
            p.extra += 1;
        }
        if face.shadow() {
            p.shadow = 2;
            p.extra += 2;
        }
        if face.condense() {
            p.extra -= 1;
        }
        if face.extend() {
            p.extra += 1;
        }
        if face.underline() {
            p.ul_offset = 1;
            p.ul_shadow = 1;
            p.ul_thick = 1;
        }
        p
    }

    /// `true` when no synthesis pass would change the plain glyph.
    #[inline]
    pub fn is_plain(&self) -> bool {
        *self == Self::default()
    }
}

/// Width of the per-glyph synthesis mask, in design columns.
const MASK_W: i32 = 16;
/// Design column `-MASK_OX` maps to mask bit 0 (left margin for the
/// outline ring, which can extend one pixel left of the glyph).
const MASK_OX: i32 = 2;
/// Design row `-MASK_OY` maps to mask row 0 (top margin for the ring).
const MASK_OY: i32 = 1;
/// Mask rows: design rows `-1 ..= 10` — the 7 authored rows plus ring /
/// shadow / underline room below the baseline.
const MASK_ROWS: usize = 12;

/// Build the styled ink mask for one glyph, in design space. Bit `c` of
/// row `r` is design pixel `(c - MASK_OX, r - MASK_OY)` relative to the
/// glyph cell's top-left. `advance` is the styled advance width in
/// design pixels (used as the underline span).
///
/// Pipeline order mirrors Volume I pages I-151/I-152: bold smear, then
/// the italic shear, then the outline/shadow ring (so a bolded stroke
/// widens the hollow, as the book requires), then the underline (which
/// must gap around whatever ink ended up below the baseline).
fn synth_glyph_mask(bmp: [u8; 7], style: &StyleParams, advance: i32) -> [u32; MASK_ROWS] {
    let mut m = [0u32; MASK_ROWS];
    for (r, &rowmask) in bmp.iter().enumerate() {
        let mut bits = 0u32;
        for col in 0..GLYPH_W {
            if (rowmask >> (GLYPH_W - 1 - col)) & 1 != 0 {
                bits |= 1 << (col + MASK_OX);
            }
        }
        m[r + MASK_OY as usize] = bits;
    }
    // Bold: "repeatedly drawn one bit to the right" (page I-152).
    for _ in 0..style.bold {
        for row in m.iter_mut() {
            *row |= *row << 1;
        }
    }
    // Italic: skew right above the baseline, left below (page I-152).
    if style.italic > 0 {
        for (i, row) in m.iter_mut().enumerate() {
            let dist = BASELINE_ROW - (i as i32 - MASK_OY);
            if dist > 0 {
                *row <<= ((dist * style.italic) >> 4) as u32;
            } else if dist < 0 {
                *row >>= ((-dist * style.italic) >> 4) as u32;
            }
        }
    }
    // Outline / shadow: hollow ring around the (possibly bolded) solid
    // body; factor 2 thickens the ring below and to the right
    // (page I-152, factors from the I-226 table).
    if style.shadow > 0 {
        let solid = m;
        let mut ring = [0u32; MASK_ROWS];
        for i in 0..MASK_ROWS {
            let above = if i > 0 { solid[i - 1] } else { 0 };
            let below = if i + 1 < MASK_ROWS { solid[i + 1] } else { 0 };
            let dil = solid[i]
                | solid[i] << 1
                | solid[i] >> 1
                | above
                | above << 1
                | above >> 1
                | below
                | below << 1
                | below >> 1;
            ring[i] = dil & !solid[i];
        }
        let base = ring;
        for d in 1..style.shadow {
            let du = d as usize;
            for i in (0..MASK_ROWS).rev() {
                let mut v = ring[i] | (base[i] << d); // thicken rightward
                if i >= du {
                    // … and downward (plus the diagonal join).
                    v |= base[i - du] | (base[i - du] << d);
                }
                ring[i] = v;
            }
        }
        // "hollow, outlined character": the solid body stays unpainted.
        for i in 0..MASK_ROWS {
            m[i] = ring[i] & !solid[i];
        }
    }
    // Underline: `ul_thick` rows starting `ul_offset` below the
    // baseline, spanning the full advance (spaces included — condense /
    // extend are the styles the book scopes to "all characters,
    // including spaces", and the underline is a property of the whole
    // styled run). The line gaps `ul_shadow` pixels on either side of
    // any ink already occupying the underline row (page I-152's
    // descender rule).
    if style.ul_thick > 0 && advance > 0 {
        let span = ((1u32 << advance.min(MASK_W)) - 1) << MASK_OX;
        for t in 0..style.ul_thick {
            let row = BASELINE_ROW + style.ul_offset + t + MASK_OY;
            if !(0..MASK_ROWS as i32).contains(&row) {
                break;
            }
            let i = row as usize;
            let ink = m[i];
            let mut keep_out = ink;
            for s in 1..=style.ul_shadow {
                keep_out |= ink << s | ink >> s;
            }
            m[i] |= span & !keep_out;
        }
    }
    m
}

/// Total advance width, in **canvas pixels**, that drawing `text` at the
/// given [`TextScale`] would consume — the sum of per-glyph horizontal
/// advances plus the `ch_extra` per-character, `sp_extra` per-space and
/// `inter_char` per-character (`lineJustify $002D`) adjustments (§A-3
/// `$0016` / `$0006` / `$002D`; Imaging With QuickDraw book page 2-34).
/// Used to move the running text pen after a draw so successive
/// `DH/DV/DHDVText` opcodes on the same line land correctly.
///
/// The `inter_char` width is the `lineJustify` intercharacter spacing
/// (the Script Manager's "extra character width"); per §A-3 footnote `†`
/// it is added to **every** character in the style run, distinct from the
/// nonspace-only `ch_extra` and space-only `sp_extra`.
///
/// `face` is the active `txFace` style byte: the characterization
/// table's `extra` widening (bold `+1`, outline `+1`, shadow `+2`,
/// condensed `-1`, extended `+1` design pixels — Volume I page I-226)
/// is folded into every character's advance, matching what
/// [`draw_text`] paints.
pub fn measure_text(
    text: &[u8],
    scale: TextScale,
    ch_extra: i32,
    sp_extra: i32,
    inter_char: i32,
    face: PictTextFace,
) -> i32 {
    let style = StyleParams::from_face(face);
    let mut adv = 0i32;
    for &b in text {
        // Saturating: hostile txSize × TxRatio words can push a single
        // advance toward i32 range (round 407 hardening — mirrors
        // `draw_text`'s pen walk so the two stay equal).
        adv = adv
            .saturating_add(scale.h((char_advance_design(b) + style.extra).max(1)))
            .saturating_add(ch_extra)
            .saturating_add(inter_char);
        if b == b' ' {
            adv = adv.saturating_add(sp_extra);
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
/// `pen_y - (BASELINE_ROW - row)·scale_v`, the one descender row below at
/// `pen_y + scale_v`. The cell scales by [`TextScale`] — `txSize` plus the
/// `TxRatio` (`$0010`) horizontal / vertical factors, so a wide or
/// condensed `TxRatio` stretches or squeezes the glyph cells along their
/// respective axes while leaving the baseline anchored on `pen_y`.
///
/// `inter_char` is the `lineJustify` (`$002D`) intercharacter spacing
/// added to every glyph's horizontal advance (§A-3 footnote `†`).
///
/// `face` selects the `txFace` style synthesis (see the module docs and
/// [`StyleParams`]): the glyph mask is bolded / sheared / ringed /
/// underlined in design space per Volume I pages I-151/I-152 before the
/// scaled blocks are painted, and the styled `extra` widening joins the
/// pen advance.
#[allow(clippy::too_many_arguments)]
pub fn draw_text(
    canvas: &mut Canvas,
    text: &[u8],
    pen_x: i32,
    pen_y: i32,
    scale: TextScale,
    ch_extra: i32,
    sp_extra: i32,
    inter_char: i32,
    face: PictTextFace,
    fg: Rgba,
    bg: Rgba,
    mode: SourceMode,
) -> i32 {
    let style = StyleParams::from_face(face);
    let mut x = pen_x;
    // A single design pixel maps to `cw × ch` canvas pixels — the
    // anisotropic cell from the active `txSize` × `TxRatio`.
    let cw = scale.h(1);
    let ch = scale.v(1);
    // srcOr / srcXor / srcBic leave the glyph's off-bits transparent
    // (the visible text modes); the opaque modes paint them too.
    let transparent_off = matches!(
        mode,
        SourceMode::SrcOr | SourceMode::SrcBic | SourceMode::SrcXor
    );
    for &b in text {
        let advance = (char_advance_design(b) + style.extra).max(1);
        let mask = synth_glyph_mask(glyph(b), &style, advance);
        for (i, &rowbits) in mask.iter().enumerate() {
            if rowbits == 0 {
                continue;
            }
            let rd = i as i32 - MASK_OY; // design row within the cell
            let rows_above_baseline = BASELINE_ROW - rd;
            // Vertical placement: the baseline (book page 2-13) sits on
            // `pen_y`, so row `BASELINE_ROW`'s scaled block ends exactly
            // at the pen line. Rows above the baseline stack upward;
            // synthesised rows below it (underline / outline ring) stack
            // downward starting at `pen_y + 1`.
            // Saturating placement arithmetic: `txSize` / `TxRatio` are
            // attacker-controlled i16 words, so a hostile stream can
            // push the scaled offsets toward i32 range (round 407
            // hardening).
            let y_top = if rows_above_baseline >= 0 {
                pen_y
                    .saturating_sub(scale.v(rows_above_baseline + 1))
                    .saturating_add(1)
            } else {
                pen_y
                    .saturating_add(1)
                    .saturating_add(scale.v_off(rd - BASELINE_ROW - 1))
            };
            // Off-bits are only painted by the opaque modes, and only
            // inside the nominal 5×7 character cell (the styled margins
            // around it carry no "source image" of their own).
            let in_cell_row = (0..GLYPH_H).contains(&rd);
            // Clip each scaled block to the canvas *before* iterating:
            // a hostile `txSize` makes `cw × ch` enormous, and walking
            // the off-canvas cells (every write discarded) would be a
            // CPU DoS on a 2-byte field (round 407 hardening).
            let py0 = y_top.max(0);
            let py1 = y_top.saturating_add(ch).min(canvas.height as i32);
            for col in 0..MASK_W {
                let on = rowbits >> col & 1 != 0;
                let cd = col - MASK_OX; // design column within the cell
                if !on && (transparent_off || !in_cell_row || !(0..GLYPH_W).contains(&cd)) {
                    continue;
                }
                // For a glyph the "source image" is black on the on-bits
                // and white on the off-bits; blend_source maps that to the
                // active text mode.
                let src = if on { Rgba::BLACK } else { Rgba::WHITE };
                let bx = x.saturating_add(scale.h_off(cd));
                let px0 = bx.max(0);
                let px1 = bx.saturating_add(cw).min(canvas.width as i32);
                for py in py0..py1 {
                    for px in px0..px1 {
                        let dst = canvas.pixel_at(px, py).unwrap_or(bg);
                        let out = blend_source(mode, src, dst, fg, bg);
                        canvas.put(px, py, out);
                    }
                }
            }
        }
        x = x
            .saturating_add(scale.h(advance))
            .saturating_add(ch_extra)
            .saturating_add(inter_char);
        if b == b' ' {
            x = x.saturating_add(sp_extra);
        }
    }
    x.saturating_sub(pen_x)
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
        let m8 = measure_text(b"AB", TextScale::isotropic(8), 0, 0, 0, PictTextFace::PLAIN);
        let m16 = measure_text(
            b"AB",
            TextScale::isotropic(16),
            0,
            0,
            0,
            PictTextFace::PLAIN,
        );
        assert_eq!(m8, 2 * ADVANCE);
        assert_eq!(m16, 2 * ADVANCE * 2);
    }

    #[test]
    fn measure_includes_extras() {
        let base = measure_text(
            b"a a",
            TextScale::isotropic(8),
            0,
            0,
            0,
            PictTextFace::PLAIN,
        );
        let with_ch = measure_text(
            b"a a",
            TextScale::isotropic(8),
            2,
            0,
            0,
            PictTextFace::PLAIN,
        );
        let with_sp = measure_text(
            b"a a",
            TextScale::isotropic(8),
            0,
            3,
            0,
            PictTextFace::PLAIN,
        );
        let with_ic = measure_text(
            b"a a",
            TextScale::isotropic(8),
            0,
            0,
            4,
            PictTextFace::PLAIN,
        );
        assert_eq!(with_ch, base + 3 * 2); // 3 chars × chExtra 2
        assert_eq!(with_sp, base + 3); // one space × spExtra 3
        assert_eq!(with_ic, base + 3 * 4); // 3 chars × interChar 4
    }

    #[test]
    fn measure_tx_ratio_stretches_horizontally() {
        // A 2/1 horizontal TxRatio doubles the advance; vertical ratio
        // leaves the horizontal advance untouched.
        let base = measure_text(b"AB", TextScale::isotropic(8), 0, 0, 0, PictTextFace::PLAIN);
        let wide = measure_text(
            b"AB",
            TextScale {
                tx_size: 8,
                numer_h: 2,
                denom_h: 1,
                numer_v: 1,
                denom_v: 1,
            },
            0,
            0,
            0,
            PictTextFace::PLAIN,
        );
        assert_eq!(wide, 2 * base);
    }

    #[test]
    fn draw_paints_ink_and_advances() {
        let mut c = Canvas::new(40, 20, Rgba::WHITE);
        let adv = draw_text(
            &mut c,
            b"A",
            2,
            10,
            TextScale::isotropic(8),
            0,
            0,
            0,
            PictTextFace::PLAIN,
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

    /// Collect the set of inked (black) pixels after drawing one glyph.
    fn ink_set(text: &[u8], face: PictTextFace) -> std::collections::BTreeSet<(i32, i32)> {
        let mut c = Canvas::new(48, 24, Rgba::WHITE);
        draw_text(
            &mut c,
            text,
            8,
            12,
            TextScale::isotropic(8),
            0,
            0,
            0,
            face,
            Rgba::BLACK,
            Rgba::WHITE,
            SourceMode::SrcOr,
        );
        let mut s = std::collections::BTreeSet::new();
        for y in 0..24 {
            for x in 0..48 {
                if c.pixel_at(x, y).unwrap() == Rgba::BLACK {
                    s.insert((x, y));
                }
            }
        }
        s
    }

    #[test]
    fn characterization_table_amounts() {
        // Volume I page I-226 Figure 4 (screen column), including the
        // worked bold+shadow extra example on page I-227 (extra = 3).
        let p = StyleParams::from_face(PictTextFace::from(PictTextFace::BOLD));
        assert_eq!((p.bold, p.extra), (1, 1));
        let p = StyleParams::from_face(PictTextFace::from(PictTextFace::ITALIC));
        assert_eq!((p.italic, p.extra), (8, 0));
        let p = StyleParams::from_face(PictTextFace::from(PictTextFace::OUTLINE));
        assert_eq!((p.shadow, p.extra), (1, 1));
        let p = StyleParams::from_face(PictTextFace::from(PictTextFace::SHADOW));
        assert_eq!((p.shadow, p.extra), (2, 2));
        let p = StyleParams::from_face(PictTextFace::from(PictTextFace::CONDENSE));
        assert_eq!(p.extra, -1);
        let p = StyleParams::from_face(PictTextFace::from(PictTextFace::EXTEND));
        assert_eq!(p.extra, 1);
        let p = StyleParams::from_face(PictTextFace::from(PictTextFace::UNDERLINE));
        assert_eq!((p.ul_offset, p.ul_shadow, p.ul_thick), (1, 1, 1));
        let p = StyleParams::from_face(PictTextFace::from(
            PictTextFace::BOLD | PictTextFace::SHADOW,
        ));
        assert_eq!(p.extra, 3);
        assert!(StyleParams::from_face(PictTextFace::PLAIN).is_plain());
    }

    #[test]
    fn measure_widens_per_style_extra() {
        let scale = TextScale::isotropic(8);
        let plain = measure_text(b"AB", scale, 0, 0, 0, PictTextFace::PLAIN);
        let bold = measure_text(
            b"AB",
            scale,
            0,
            0,
            0,
            PictTextFace::from(PictTextFace::BOLD),
        );
        let cond = measure_text(
            b"AB",
            scale,
            0,
            0,
            0,
            PictTextFace::from(PictTextFace::CONDENSE),
        );
        let shad = measure_text(
            b"AB",
            scale,
            0,
            0,
            0,
            PictTextFace::from(PictTextFace::SHADOW),
        );
        assert_eq!(bold, plain + 2); // +1 design px per char
        assert_eq!(cond, plain - 2); // -1 per char
        assert_eq!(shad, plain + 4); // +2 per char
    }

    #[test]
    fn bold_smears_one_pixel_right() {
        // Page I-152: bold = the plain image ORed one pixel right. The
        // '|' glyph (single column) makes the smear directly countable.
        let plain = ink_set(b"|", PictTextFace::PLAIN);
        let bold = ink_set(b"|", PictTextFace::from(PictTextFace::BOLD));
        // Every plain pixel survives …
        assert!(plain.is_subset(&bold));
        // … and the extra ink is exactly the plain set shifted +1 in x.
        let smear: std::collections::BTreeSet<_> = plain.iter().map(|&(x, y)| (x + 1, y)).collect();
        let expect: std::collections::BTreeSet<_> = plain.union(&smear).copied().collect();
        assert_eq!(bold, expect);
    }

    #[test]
    fn italic_shears_top_right_baseline_fixed() {
        // Page I-152: bits above the baseline skew right. '|' is a
        // single column, so each row's shear is directly visible.
        let plain = ink_set(b"|", PictTextFace::PLAIN);
        let italic = ink_set(b"|", PictTextFace::from(PictTextFace::ITALIC));
        assert_eq!(plain.len(), italic.len());
        // The baseline row (y = 12, pen v) is unmoved.
        let base_plain: Vec<_> = plain.iter().filter(|&&(_, y)| y == 12).collect();
        let base_italic: Vec<_> = italic.iter().filter(|&&(_, y)| y == 12).collect();
        assert_eq!(base_plain, base_italic);
        // The top authored row (y = 12 - 6) moved right by (6*8)>>4 = 3.
        let top_plain: Vec<_> = plain.iter().filter(|&&(_, y)| y == 6).copied().collect();
        let top_italic: Vec<_> = italic.iter().filter(|&&(_, y)| y == 6).copied().collect();
        assert_eq!(
            top_italic,
            top_plain
                .iter()
                .map(|&(x, y)| (x + 3, y))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn underline_draws_one_row_below_baseline() {
        // Page I-152 + the I-226 table: offset 1, thickness 1. Pen at
        // (8, 12) → the underline row is y = 13, spanning the advance.
        let plain = ink_set(b"A", PictTextFace::PLAIN);
        let under = ink_set(b"A", PictTextFace::from(PictTextFace::UNDERLINE));
        assert!(plain.iter().all(|&(_, y)| y != 13));
        let row: Vec<_> = under
            .iter()
            .filter(|&&(_, y)| y == 13)
            .map(|&(x, _)| x)
            .collect();
        // Advance is 6 design px starting at pen_x = 8, so the line
        // spans x = 8..=13 — right up to (but not into) the next
        // character cell, keeping run underlines continuous.
        assert_eq!(row, (8..=13).collect::<Vec<_>>());
        // Above the underline the glyph is unchanged.
        let under_glyph: std::collections::BTreeSet<_> =
            under.iter().copied().filter(|&(_, y)| y != 13).collect();
        assert_eq!(under_glyph, plain);
    }

    #[test]
    fn underline_gaps_around_descender_ink() {
        // Outline pushes a ring one pixel below the baseline; the
        // underline (same row) must gap one pixel either side of that
        // ink (page I-152's descender rule) instead of butting into it.
        let outlined = ink_set(b"o", PictTextFace::from(PictTextFace::OUTLINE));
        let both = ink_set(
            b"o",
            PictTextFace::from(PictTextFace::OUTLINE | PictTextFace::UNDERLINE),
        );
        let ring_row: std::collections::BTreeSet<_> = outlined
            .iter()
            .filter(|&&(_, y)| y == 13)
            .map(|&(x, _)| x)
            .collect();
        assert!(!ring_row.is_empty(), "outline ring should reach y = 13");
        let both_row: std::collections::BTreeSet<_> = both
            .iter()
            .filter(|&&(_, y)| y == 13)
            .map(|&(x, _)| x)
            .collect();
        // No underline pixel directly adjacent to ring ink: every added
        // pixel is ≥ 2 away from all ring pixels.
        for &x in both_row.difference(&ring_row) {
            assert!(
                ring_row.iter().all(|&rx| (rx - x).abs() >= 2),
                "underline pixel at x={x} touches the descender ring"
            );
        }
    }

    #[test]
    fn outline_is_hollow() {
        // Page I-152: a hollow, outlined character — the plain strokes
        // are unpainted, and the ink that is painted borders them.
        let plain = ink_set(b"I", PictTextFace::PLAIN);
        let outline = ink_set(b"I", PictTextFace::from(PictTextFace::OUTLINE));
        assert!(plain.is_disjoint(&outline), "outline must be hollow");
        // Every outline pixel is 8-adjacent to a plain stroke pixel.
        for &(x, y) in &outline {
            let touches = plain
                .iter()
                .any(|&(px, py)| (px - x).abs() <= 1 && (py - y).abs() <= 1 && (px, py) != (x, y));
            assert!(touches, "ring pixel ({x},{y}) not adjacent to the stroke");
        }
    }

    #[test]
    fn shadow_thickens_below_and_right() {
        // Page I-152: shadow = the outline ring thickened below and to
        // the right. The shadow set strictly contains the outline set,
        // stays hollow, and every added pixel lies below/right of ring
        // ink.
        let plain = ink_set(b"I", PictTextFace::PLAIN);
        let outline = ink_set(b"I", PictTextFace::from(PictTextFace::OUTLINE));
        let shadow = ink_set(b"I", PictTextFace::from(PictTextFace::SHADOW));
        assert!(outline.is_subset(&shadow));
        assert!(shadow.len() > outline.len());
        assert!(plain.is_disjoint(&shadow), "shadow stays hollow");
        for &(x, y) in shadow.difference(&outline) {
            assert!(
                outline.contains(&(x - 1, y))
                    || outline.contains(&(x, y - 1))
                    || outline.contains(&(x - 1, y - 1)),
                "thickening pixel ({x},{y}) is not below/right of the ring"
            );
        }
    }

    #[test]
    fn bold_widens_the_hollow() {
        // Page I-152: "If you specify bold along with outline or
        // shadow, the hollow part of the character is widened." The
        // hollow (unpainted stroke body) of bold+outline '|' covers the
        // bolded 2-px stroke, not just the plain 1-px one.
        let bold = ink_set(b"|", PictTextFace::from(PictTextFace::BOLD));
        let bold_outline = ink_set(
            b"|",
            PictTextFace::from(PictTextFace::BOLD | PictTextFace::OUTLINE),
        );
        assert!(
            bold.is_disjoint(&bold_outline),
            "the widened (bolded) stroke body stays hollow"
        );
        assert!(!bold_outline.is_empty());
    }

    #[test]
    fn space_paints_no_ink_in_or_mode() {
        let mut c = Canvas::new(40, 20, Rgba::WHITE);
        draw_text(
            &mut c,
            b" ",
            2,
            10,
            TextScale::isotropic(8),
            0,
            0,
            0,
            PictTextFace::PLAIN,
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
