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
        }
    }
}
