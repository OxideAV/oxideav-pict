//! QuickDraw drawing-state machine.
//!
//! PICT opcodes are interpreted by stepping a small state machine
//! that tracks pen position / size / colour, foreground / background
//! colours, oval-corner size for round-rects, the current text
//! position, and the rectangles last passed to each "verb-rect"
//! family (frame, paint, erase, invert, fill of rect / round-rect /
//! oval / arc / poly / region) so the *SameRect* / *SameOval*
//! opcodes (low-byte nibble `8`) can re-draw without needing the
//! geometry repeated. Inside Macintosh: Imaging With QuickDraw §A-3.

/// 8-bit RGBA colour. Decoder normalises `RGBColor` (Mac u16-per-
/// channel) and Pascal 32-bit colour codes to this layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }
    pub const WHITE: Self = Self::new(255, 255, 255, 255);
    pub const BLACK: Self = Self::new(0, 0, 0, 255);

    /// Pack a Mac `RGBColor` (16-bit per channel) into 8-bit RGBA.
    /// QuickDraw stores the most-significant byte of each channel
    /// first; the low byte is just colour resolution that doesn't
    /// affect display on an 8-bit channel.
    pub fn from_rgb16(r: u16, g: u16, b: u16) -> Self {
        Self {
            r: (r >> 8) as u8,
            g: (g >> 8) as u8,
            b: (b >> 8) as u8,
            a: 0xFF,
        }
    }

    /// Pack a Pascal 32-bit colour code (`fgColor` / `bgColor`
    /// opcodes) into RGBA. The low byte is a colour-class index in
    /// classic QuickDraw (blackColor = 33, whiteColor = 30, redColor
    /// = 209, etc); pre-Color QuickDraw apps always set this. Modern
    /// PICTs use `RGBFgCol` / `RGBBkCol` instead, so the mapping here
    /// only needs to cover the eight classic colours.
    pub fn from_pascal_colour(code: u32) -> Self {
        match code {
            30 => Self::WHITE,
            33 => Self::BLACK,
            69 => Self::new(255, 255, 0, 255),  // yellow
            137 => Self::new(255, 0, 255, 255), // magenta
            205 => Self::new(255, 0, 0, 255),   // red
            273 => Self::new(0, 255, 255, 255), // cyan
            341 => Self::new(0, 255, 0, 255),   // green
            409 => Self::new(0, 0, 255, 255),   // blue
            // Unknown — just use the low 24 bits as RGB. Real-world
            // bgColor often emits 0 (black) or 0xFFFFFF (white) here
            // and that maps fine.
            _ => Self {
                r: ((code >> 16) & 0xFF) as u8,
                g: ((code >> 8) & 0xFF) as u8,
                b: (code & 0xFF) as u8,
                a: 0xFF,
            },
        }
    }
}

/// QuickDraw monochrome 8×8 bit pattern.
///
/// Inside Macintosh: Imaging With QuickDraw §A-3 — the `PnPat` /
/// `BkPat` / `FillPat` opcodes each carry an 8-byte payload that
/// represents an 8 row × 8 column on/off bitmap. The most significant
/// bit of byte 0 is the top-left pixel; the least significant bit of
/// byte 7 is the bottom-right pixel.
///
/// Stippling semantics: a `1` bit selects the *foreground* colour for
/// that cell, a `0` bit selects the *background* colour. So an all-ones
/// pattern (`[0xFF; 8]`, the QuickDraw `black()` constant) collapses to
/// a solid foreground fill — and an all-zeros pattern (`[0x00; 8]`, the
/// QuickDraw `white()` constant) collapses to a solid background fill.
/// Intermediate patterns produce the classic "50 % grey", "horizontal
/// stripes", "diagonal hatch" etc. textures Mac apps draw with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pattern(pub [u8; 8]);

impl Pattern {
    /// All-ones pattern — "solid black ink" in classic QuickDraw, i.e.
    /// stipple selects foreground at every cell. This is the default
    /// `PnPat` and `FillPat` (Inside Macintosh: `qd.black`).
    pub const BLACK: Self = Self([0xFF; 8]);
    /// All-zeros pattern — "solid white paper" in classic QuickDraw,
    /// i.e. stipple selects background at every cell. This is the
    /// default `BkPat` (Inside Macintosh: `qd.white`).
    pub const WHITE: Self = Self([0x00; 8]);

    /// Returns `true` if every bit is set (foreground everywhere).
    pub fn is_solid_fg(&self) -> bool {
        self.0.iter().all(|&b| b == 0xFF)
    }

    /// Returns `true` if every bit is clear (background everywhere).
    pub fn is_solid_bg(&self) -> bool {
        self.0.iter().all(|&b| b == 0x00)
    }

    /// Sample the pattern at picture-frame coordinates `(x, y)`.
    /// Returns `true` if the cell is a foreground bit. The pattern
    /// tiles every 8 pixels along both axes; the QuickDraw origin
    /// corresponds to byte-0 bit-7 of the pattern.
    #[inline]
    pub fn sample(&self, x: i32, y: i32) -> bool {
        let row = self.0[(y.rem_euclid(8)) as usize];
        let bit_index = 7 - (x.rem_euclid(8)) as usize;
        ((row >> bit_index) & 1) != 0
    }
}

impl Default for Pattern {
    fn default() -> Self {
        Self::BLACK
    }
}

/// QuickDraw `Rect` (top, left, bottom, right) — same layout we read
/// off disk. Stored as i32 internally so the rasteriser can use
/// signed arithmetic without risking i16 overflow on out-of-bounds
/// PICTs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RectI32 {
    pub top: i32,
    pub left: i32,
    pub bottom: i32,
    pub right: i32,
}

impl RectI32 {
    pub fn from_be(top: i16, left: i16, bottom: i16, right: i16) -> Self {
        Self {
            top: top as i32,
            left: left as i32,
            bottom: bottom as i32,
            right: right as i32,
        }
    }
}

/// Drawing state carried across the v2 opcode walk.
#[derive(Debug, Clone)]
pub struct PictState {
    /// Current pen position, in picture-frame coordinates.
    pub pen: (i32, i32),
    /// Pen size (h, v). Round 2 honours line + frame ops at 1-pixel
    /// pen size only — we still track it so future rounds can.
    pub pen_size: (i32, i32),
    /// Current foreground colour. Set by `RGBFgCol` (`0x001A`),
    /// `FgColor` (`0x000E`). Initial Mac default = black ink.
    pub fg: Rgba,
    /// Current background colour. Set by `RGBBkCol` (`0x001B`),
    /// `BgColor` (`0x000F`). Initial Mac default = white paper.
    pub bg: Rgba,
    /// Round-rect corner size set by `OvSize` (`0x000B`), in pixels.
    pub oval_size: (i32, i32),
    /// Origin offset set by `Origin` (`0x000C`). All drawing
    /// coordinates are translated by this.
    pub origin: (i32, i32),
    /// Last rectangle passed to a rect verb (frame/paint/erase/
    /// invert/fill rect). Consumed by the *SameRect* opcodes.
    pub last_rect: Option<RectI32>,
    /// Last rectangle passed to a round-rect verb.
    pub last_rrect: Option<RectI32>,
    /// Last rectangle passed to an oval verb.
    pub last_oval: Option<RectI32>,
    /// Last rectangle passed to an arc verb.
    pub last_arc_rect: Option<RectI32>,
    /// True once the canvas has been written to via the rasteriser.
    /// Used by the decoder to distinguish "produced a picture" vs
    /// "no drawing happened, no raster found" (NoRaster).
    pub touched: bool,
    /// Current pen pattern (set by `PnPat`, opcode `0x0009`).
    /// Default = `Pattern::BLACK` (solid foreground), matching the
    /// `qd.black` Inside Macintosh default. Honoured by frame / paint
    /// verbs of rect / round-rect / oval / arc / poly / region.
    pub pen_pat: Pattern,
    /// Current background pattern (set by `BkPat`, opcode `0x0002`).
    /// Default = `Pattern::WHITE` (solid background), the `qd.white`
    /// default. Honoured by erase verbs.
    pub back_pat: Pattern,
    /// Current fill pattern (set by `FillPat`, opcode `0x000A`).
    /// Default = `Pattern::BLACK` (solid foreground), the `qd.black`
    /// default. Honoured by fill verbs (low-nibble `4`).
    pub fill_pat: Pattern,
}

impl Default for PictState {
    fn default() -> Self {
        Self {
            pen: (0, 0),
            pen_size: (1, 1),
            fg: Rgba::BLACK,
            bg: Rgba::WHITE,
            oval_size: (16, 16),
            origin: (0, 0),
            last_rect: None,
            last_rrect: None,
            last_oval: None,
            last_arc_rect: None,
            touched: false,
            pen_pat: Pattern::BLACK,
            back_pat: Pattern::WHITE,
            fill_pat: Pattern::BLACK,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pattern_solid_collapses() {
        assert!(Pattern::BLACK.is_solid_fg());
        assert!(!Pattern::BLACK.is_solid_bg());
        assert!(Pattern::WHITE.is_solid_bg());
        assert!(!Pattern::WHITE.is_solid_fg());
    }

    #[test]
    fn pattern_sample_all_ones() {
        for x in 0..16 {
            for y in 0..16 {
                assert!(Pattern::BLACK.sample(x, y), "{x},{y}");
            }
        }
    }

    #[test]
    fn pattern_sample_all_zeros() {
        for x in 0..16 {
            for y in 0..16 {
                assert!(!Pattern::WHITE.sample(x, y), "{x},{y}");
            }
        }
    }

    #[test]
    fn pattern_sample_horizontal_stripes() {
        // Alternating rows of 0xFF / 0x00 → even rows foreground, odd rows
        // background. Tile wraps every 8 rows.
        let pat = Pattern([0xFF, 0x00, 0xFF, 0x00, 0xFF, 0x00, 0xFF, 0x00]);
        for x in 0..16 {
            assert!(pat.sample(x, 0));
            assert!(!pat.sample(x, 1));
            assert!(pat.sample(x, 2));
            assert!(!pat.sample(x, 9)); // wraps: row 9 -> 1 -> 0x00
            assert!(pat.sample(x, 16)); // wraps: row 16 -> 0 -> 0xFF
        }
    }

    #[test]
    fn pattern_sample_vertical_stripes() {
        // 0xAA = 0b10101010 → even columns foreground, odd background.
        let pat = Pattern([0xAA; 8]);
        for y in 0..16 {
            assert!(pat.sample(0, y));
            assert!(!pat.sample(1, y));
            assert!(pat.sample(8, y)); // wraps to col 0
            assert!(!pat.sample(9, y)); // wraps to col 1
        }
    }

    #[test]
    fn pattern_sample_negative_coordinates() {
        // rem_euclid handles negative coordinates — pattern must still
        // tile cleanly when picture-frame coords go below zero (e.g. an
        // Origin offset that shifts pixels into negative space).
        let pat = Pattern([0xAA; 8]);
        assert_eq!(pat.sample(-1, 0), pat.sample(7, 0));
        assert_eq!(pat.sample(-8, 0), pat.sample(0, 0));
        assert_eq!(pat.sample(-9, 0), pat.sample(7, 0));
    }
}
