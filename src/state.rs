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

use crate::raster::SourceMode;

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

    /// Per-channel 50/50 average of two colours — the "blend of the
    /// foreground and background colors" that `grayishTextOr = 49`
    /// draws dimmed text with on a colour destination (Inside
    /// Macintosh Volume VI, page 17-17). Truncating average, worked at
    /// 8-bit channel precision like every other resolver in this crate.
    pub const fn blend_half(self, other: Self) -> Self {
        Self {
            r: ((self.r as u16 + other.r as u16) / 2) as u8,
            g: ((self.g as u16 + other.g as u16) / 2) as u8,
            b: ((self.b as u16 + other.b as u16) / 2) as u8,
            a: ((self.a as u16 + other.a as u16) / 2) as u8,
        }
    }

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

/// QuickDraw multi-colour pixel pattern.
///
/// Inside Macintosh: Imaging With QuickDraw §A-3 Listing A-1 — the
/// `BkPixPat` (`0x0012`), `PnPixPat` (`0x0013`) and `FillPixPat`
/// (`0x0014`) opcodes each carry a `PixPat` record whose `patType=1`
/// (colour-pixmap) variant carries:
///
/// 1. `PatType: word`     — type=1 for colour-pixmap variant.
/// 2. `Pat1Data: Pattern` — 8-byte monochrome fallback.
/// 3. `PixMap: PixMap`    — pixel-map header sans baseAddr (matches
///    Listing A-2 convention).
/// 4. `ColorTable`        — palette consumed by indexed-pixel PixData.
/// 5. `PixData: PixData`  — per-row PackBits / raw pixel bytes per §A-3.
///
/// The decoder resolves the indexed-pixel PixData against the
/// `ColorTable` and stores the result as a `width`×`height` [`Rgba`]
/// grid here. Inside Macintosh §3 ("QuickDraw Drawing Reference",
/// book page 3-40) states *"A pixel pattern can use additional colors
/// and can be of any width and height that's a power of 2."* — so the
/// tile is no longer constrained to `8×8` (round 91 only honoured the
/// universal `8×8` tile; round 302 wires up the arbitrary power-of-2
/// `bounds` the spec permits). A pattern whose `bounds` is degenerate
/// (zero width or height) or whose dimensions aren't both powers of two
/// still falls back to the `Pat1Data` monochrome interpretation.
///
/// Sampling semantics mirror [`Pattern::sample`]: the tile wraps on
/// `(x mod width, y mod height)`, with the most-significant byte/row
/// mapping to top-left. Stippling cells take their fully-resolved RGB
/// directly from `pixels`; the current foreground / background colour
/// state is NOT consulted (PixPat is *colour-explicit*, unlike
/// monochrome `Pattern` which selects between fg / bg per cell).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PixPattern {
    /// 8-byte monochrome fallback (the `Pat1Data` field of the on-disk
    /// PixPat record). Used by callers that want a black/white render
    /// of the pattern without consulting the colour grid.
    pub fallback: Pattern,
    /// Tile width in pixels (the `bounds` right − left). Always a power
    /// of two per §3 (book page 3-40); commonly `8`.
    pub width: u16,
    /// Tile height in pixels (the `bounds` bottom − top). Always a power
    /// of two per §3; commonly `8`.
    pub height: u16,
    /// `width` × `height` cells of RGBA, row-major (the top-left cell is
    /// `pixels[0]`). Length is exactly `width as usize * height as usize`.
    pub pixels: Vec<Rgba>,
}

impl PixPattern {
    /// Construct an arbitrary `width`×`height` colour tile.
    ///
    /// `pixels` is row-major and must contain exactly
    /// `width * height` cells; on a length mismatch the constructor
    /// pads with [`Rgba::BLACK`] / truncates so the invariant
    /// `pixels.len() == width * height` always holds (a defensive
    /// posture so a malformed producer can't desync the sampler).
    pub fn new(width: u16, height: u16, mut pixels: Vec<Rgba>, fallback: Pattern) -> Self {
        let want = width as usize * height as usize;
        pixels.resize(want, Rgba::BLACK);
        Self {
            fallback,
            width,
            height,
            pixels,
        }
    }

    /// Sample the pattern at picture-frame coordinates `(x, y)`. The
    /// tile wraps modulo `width` / `height` along the two axes; the
    /// QuickDraw origin corresponds to cell `[0][0]`.
    #[inline]
    pub fn sample(&self, x: i32, y: i32) -> Rgba {
        // A degenerate (zero-dimension) tile can't be sampled; treat it
        // as solid black rather than panicking on a `% 0`.
        if self.width == 0 || self.height == 0 || self.pixels.is_empty() {
            return Rgba::BLACK;
        }
        let row = y.rem_euclid(self.height as i32) as usize;
        let col = x.rem_euclid(self.width as i32) as usize;
        self.pixels[row * self.width as usize + col]
    }

    /// Construct an 8×8 colour tile that approximates the target
    /// `rgb` colour, as `MakeRGBPat` would on an effectively-infinite-
    /// colour-depth GDevice.
    ///
    /// Inside Macintosh: Imaging With QuickDraw §4 ("Color QuickDraw")
    /// describes the [`MakeRGBPat`](https://developer.apple.com/) procedure
    /// behaviour as: *"generates a PixPat record that, when used to draw
    /// a pixel pattern, approximates the color you specify in the
    /// myColor parameter."* — i.e. the algorithmic contract is
    /// **approximate this RGB**, not "produce a specific bit-pattern."
    /// The same chapter is explicit that the implementation **"opted
    /// for a fast pattern selection rather than the best possible
    /// pattern selection,"** and §A-3 Listing A-1 confirms that the
    /// on-disk `ditherPat` record carries **only** an `RGBColor` — the
    /// 8×8 tile itself is computed at draw time against the active
    /// `GDevice` palette.
    ///
    /// Our rasteriser draws to a true-colour RGBA canvas (no indexed
    /// `GDevice` palette in the loop), so the target colour can be
    /// rendered *exactly* by setting every cell to the requested RGB.
    /// This satisfies the §4 spec contract — *"approximate this RGB" *
    /// — with zero approximation error on a 24-bit canvas, and it
    /// preserves the §A-3 luminance guarantee (*"QuickDraw draws pixel
    /// patterns created with the MakeRGBPat procedure as bit patterns
    /// having approximately the same luminance as the pixel
    /// patterns"*) by construction.
    ///
    /// The `fallback` field carries the `Pat1Data` byte stream from
    /// the on-disk PixPat record verbatim so callers that *do* end up
    /// running against a 1-bpp surface (Inside Macintosh §A-3: *"on
    /// computers that have only black-and-white displays, Color
    /// QuickDraw substitutes the bit pattern in pat1Data"*) still pick
    /// up a sensible monochrome stipple.
    pub fn from_dither_rgb(rgb: Rgba, fallback: Pattern) -> Self {
        Self {
            fallback,
            width: 8,
            height: 8,
            pixels: vec![rgb; 64],
        }
    }
}

/// Tagged pattern slot — either the legacy monochrome `Pattern` or the
/// colour-pixmap `PixPattern`. Tracked separately from the monochrome
/// [`Pattern`] state so encoders / inspectors can round-trip the slot
/// without lossy collapse to the `Pat1Data` fallback.
///
/// The colour variant is boxed because a `PixPattern` is 264 bytes
/// while `Pattern` is 8 — the size discrepancy would otherwise inflate
/// every `PictPattern` consumer 33×.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PictPattern {
    /// Monochrome 8×8 stipple — `PnPat 0x0009 / BkPat 0x0002 /
    /// FillPat 0x000A` opcodes (or the `Pat1Data` fallback of a
    /// PixPat record).
    Mono(Pattern),
    /// Multi-colour 8×8 pixel pattern — `PnPixPat 0x0013 / BkPixPat
    /// 0x0012 / FillPixPat 0x0014` opcodes, `patType=1` variant.
    ColourPixmap(Box<PixPattern>),
    /// Dithered 8×8 pixel pattern — `PnPixPat 0x0013 / BkPixPat
    /// 0x0012 / FillPixPat 0x0014` opcodes, `patType=2` variant.
    ///
    /// The on-disk record carries only a target `RGBColor` plus the
    /// `Pat1Data` monochrome fallback (Inside Macintosh §A-3 Listing
    /// A-1). Classic Color QuickDraw expands the tile at draw time
    /// against the active `GDevice` palette via the `MakeRGBPat`
    /// algorithm — see [`PixPattern::from_dither_rgb`] for the
    /// rationale behind our expansion on a true-colour RGBA canvas.
    ///
    /// Stored as a fully-resolved 8×8 [`PixPattern`] so the rasteriser
    /// can sample it identically to the `ColourPixmap` variant; the
    /// `pixels` field of the inner `PixPattern` holds the resolved
    /// 24-bit RGB at every cell, the `fallback` holds the on-disk
    /// `Pat1Data` byte stream verbatim. Round-trip writers that need
    /// to preserve the dither subtype distinctly from
    /// `ColourPixmap` (e.g. for re-emission against a different
    /// GDevice) can recover the source `RGBColor` from any cell of
    /// `pixels` (every cell carries the same RGB by construction).
    DitheredPixmap {
        /// Target `RGBColor` from the on-disk patType=2 record,
        /// packed into 8-bit RGBA. The 16-bit-per-channel precision of
        /// the source `RGBColor` is preserved through `Rgba::from_rgb16`'s
        /// high-byte selection.
        rgb: Rgba,
        /// 8-byte `Pat1Data` monochrome fallback from the on-disk
        /// record. Used by 1-bpp consumers; the colour rasteriser
        /// renders `pixels` instead.
        fallback: Pattern,
        /// 8 rows × 8 columns of RGBA, every cell = `rgb`. Kept
        /// alongside `rgb` so the rasteriser sample path doesn't
        /// branch on the variant tag.
        pixels: Box<[Rgba; 64]>,
    },
}

impl PictPattern {
    /// The monochrome representation. Returns the `Pattern` directly
    /// for `Mono`, the `Pat1Data` fallback for `ColourPixmap`, or the
    /// `fallback` field for `DitheredPixmap`.
    pub fn mono(&self) -> Pattern {
        match self {
            PictPattern::Mono(p) => *p,
            PictPattern::ColourPixmap(pp) => pp.fallback,
            PictPattern::DitheredPixmap { fallback, .. } => *fallback,
        }
    }
}

impl Default for PictPattern {
    fn default() -> Self {
        PictPattern::Mono(Pattern::default())
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

/// Decoded `TxFace` (`$0004` / `$04`) `Style` byte per Inside
/// Macintosh: Imaging With QuickDraw §A-3 Table A-2 / A-3.
///
/// The on-disk payload is a single byte (`0..=255`) describing a classic
/// Mac `Style` bitfield. §A-3's "Text's font style (0..255)" caption
/// pairs each set bit with one of the classic-Mac glyph treatments:
///
/// | Bit  | Mask   | Treatment   |
/// | ---- | ------ | ----------- |
/// | 0    | `0x01` | `bold`      |
/// | 1    | `0x02` | `italic`    |
/// | 2    | `0x04` | `underline` |
/// | 3    | `0x08` | `outline`   |
/// | 4    | `0x10` | `shadow`    |
/// | 5    | `0x20` | `condense`  |
/// | 6    | `0x40` | `extend`    |
/// | 7    | `0x80` | reserved    |
///
/// The fresh-GrafPort default is [`PictTextFace::PLAIN`] (`0x00`),
/// matching Color QuickDraw's plain-text style: no `bold`, no `italic`,
/// etc.
///
/// Round 266 promotes [`PictTextState::tx_face`] from raw `u8` storage
/// to this newtype so consumers can decode the per-bit treatment via
/// named predicates without re-deriving the bit positions at every call
/// site. The wire-format encoder helper [`crate::build_tx_face`] keeps
/// taking a `u8` (the on-disk byte) so existing producers don't have to
/// migrate; the newtype's `From<u8>` / `Into<u8>` round-trip makes the
/// transition mechanical (`PictTextFace::from(0x05)` `==` `0x05u8`).
///
/// Bit 7 (`0x80`) is unassigned in §A-3's caption; the decoder preserves
/// it verbatim through [`PictTextFace::bits`] so round-trip encoders
/// don't drop reserved bits a future producer might set. The named-bit
/// predicates only read bits 0..=6.
///
/// The bit order matches the classic `Style` Pascal set's ordinal order
/// — `(bold, italic, underline, outline, shadow, condense, extend)` —
/// declared in Inside Macintosh Volume I (page I-152). Since round 407
/// the rasteriser *synthesises* these treatments onto every drawn glyph
/// per Volume I's style rules; see [`crate::font::StyleParams`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct PictTextFace(u8);

impl PictTextFace {
    /// Bold-glyph bit (`0x01`).
    pub const BOLD: u8 = 0x01;
    /// Italic-glyph bit (`0x02`).
    pub const ITALIC: u8 = 0x02;
    /// Underline bit (`0x04`).
    pub const UNDERLINE: u8 = 0x04;
    /// Outline-glyph bit (`0x08`).
    pub const OUTLINE: u8 = 0x08;
    /// Shadow-glyph bit (`0x10`).
    pub const SHADOW: u8 = 0x10;
    /// Condense (negative tracking) bit (`0x20`).
    pub const CONDENSE: u8 = 0x20;
    /// Extend (positive tracking) bit (`0x40`).
    pub const EXTEND: u8 = 0x40;

    /// The §A-3 plain-text default: no style bits set. Equivalent to
    /// `PictTextFace::from(0)` and to the Color QuickDraw fresh-GrafPort
    /// `tx_face` slot.
    pub const PLAIN: Self = Self(0);

    /// Wrap a raw on-disk style byte verbatim.
    #[inline]
    pub const fn new(byte: u8) -> Self {
        Self(byte)
    }

    /// Read the raw on-disk style byte back out.
    #[inline]
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// `true` when the `bold` bit (`0x01`) is set.
    #[inline]
    pub const fn bold(self) -> bool {
        self.0 & Self::BOLD != 0
    }

    /// `true` when the `italic` bit (`0x02`) is set.
    #[inline]
    pub const fn italic(self) -> bool {
        self.0 & Self::ITALIC != 0
    }

    /// `true` when the `underline` bit (`0x04`) is set.
    #[inline]
    pub const fn underline(self) -> bool {
        self.0 & Self::UNDERLINE != 0
    }

    /// `true` when the `outline` bit (`0x08`) is set.
    #[inline]
    pub const fn outline(self) -> bool {
        self.0 & Self::OUTLINE != 0
    }

    /// `true` when the `shadow` bit (`0x10`) is set.
    #[inline]
    pub const fn shadow(self) -> bool {
        self.0 & Self::SHADOW != 0
    }

    /// `true` when the `condense` bit (`0x20`) is set.
    #[inline]
    pub const fn condense(self) -> bool {
        self.0 & Self::CONDENSE != 0
    }

    /// `true` when the `extend` bit (`0x40`) is set.
    #[inline]
    pub const fn extend(self) -> bool {
        self.0 & Self::EXTEND != 0
    }

    /// `true` when every named-bit treatment is off (the §A-3
    /// fresh-GrafPort default). Reserved bit 7 (`0x80`) is ignored — a
    /// producer that flips it without flipping any of the named bits
    /// still reports `is_plain() == true`, matching the §A-3 caption
    /// which only names bits 0..=6.
    #[inline]
    pub const fn is_plain(self) -> bool {
        self.0 & 0x7F == 0
    }
}

impl From<u8> for PictTextFace {
    #[inline]
    fn from(byte: u8) -> Self {
        Self(byte)
    }
}

impl From<PictTextFace> for u8 {
    #[inline]
    fn from(face: PictTextFace) -> u8 {
        face.0
    }
}

impl PartialEq<u8> for PictTextFace {
    #[inline]
    fn eq(&self, other: &u8) -> bool {
        self.0 == *other
    }
}

impl PartialEq<PictTextFace> for u8 {
    #[inline]
    fn eq(&self, other: &PictTextFace) -> bool {
        *self == other.0
    }
}

/// Tracked text / pen-mode / highlight state captured from the §A-3
/// opcodes the rasteriser walks past today.
///
/// Inside Macintosh: Imaging With QuickDraw §A-3 Table A-2 (v2) and
/// Table A-3 (v1) list a handful of opcodes that carry pen-mode,
/// text-font, text-size, transfer-mode, highlight-colour and arithmetic
/// transfer-mode parameters which previously had their payload bytes
/// stepped past with no further structure. Round 230 captures the
/// payloads into [`PictTextState`] so consumers (and round-trip
/// encoders) can recover the values that the producer chose to declare,
/// even before the crate grows a font rasteriser or honours the
/// arithmetic transfer modes on the canvas.
///
/// Every field defaults to the Color QuickDraw "fresh GrafPort" value
/// per §A-3 / §4 ("Color QuickDraw and PixMaps"). `None` on the
/// highlight / op-color slots means the producer never emitted the
/// corresponding opcode — distinct from "emitted then reset" because
/// QuickDraw has no reset opcode for these slots (a fresh GrafPort just
/// inherits the system-wide default at draw time).
///
/// Round 236 adds typed capture of the `fontName $002C` /
/// `lineJustify $002D` / `glyphState $002E` payloads — these are the
/// "Pictures-chapter Script-Manager round-trip" opcodes (§A-3 Table A-2
/// footnotes `*` and `†`) that the v2 walker previously stepped past.
/// Each lands in a dedicated `Option<…>` slot so that consumers can
/// recover the producer's last-declared font / justification / glyph-
/// rendering preferences (`Some` ↔ "the producer emitted this opcode at
/// least once"; the slot otherwise stays at `None`, preserving the
/// fresh-GrafPort baseline).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PictTextState {
    /// Font number set by `TxFont` (`$0003` v2 / `$03` v1, `Integer`).
    /// §A-3 Table A-2: 2-byte payload. The numeric ID is the classic
    /// `FOND` resource ID, not yet resolved against a font database by
    /// this crate.
    pub tx_font: i16,
    /// Text face / style flags set by `TxFace` (`$0004` v2 / `$04` v1,
    /// 0..255). §A-3 Table A-2: 1-byte payload. The on-disk byte is a
    /// classic-Mac `Style` bitfield; round 266 promotes the storage from
    /// a raw `u8` to a typed [`PictTextFace`] newtype that decodes the
    /// per-bit predicates ([`PictTextFace::bold`] / `italic` /
    /// `underline` / `outline` / `shadow` / `condense` / `extend`) plus
    /// `From<u8>` / `Into<u8>` round-trip so existing call sites and
    /// encoder helpers keep working unchanged.
    pub tx_face: PictTextFace,
    /// Source-mode raster transfer mode set by `TxMode` (`$0005` v2 /
    /// `$05` v1, `Integer`). §A-3 Table A-2: 2-byte payload, named
    /// `srcCopy = 0`, `srcOr = 1`, …, `notSrcXor = 7`, plus the
    /// arithmetic-mode block (`patCopy = 8` … `grayishTextOr = 49`).
    pub tx_mode: i16,
    /// Extra space-width set by `SpExtra` (`$0006` v2 / `$06` v1,
    /// `Fixed`). §A-3 Table A-2: 4-byte payload. Stored verbatim as the
    /// on-disk i32 16.16 fixed value (use [`crate::header::Fixed::to_f32`]
    /// to convert).
    pub sp_extra: crate::header::Fixed,
    /// Pen transfer mode set by `PnMode` (`$0008` v2 / `$08` v1,
    /// `Integer`). §A-3 Table A-2: 2-byte payload, same numeric mode
    /// catalog as `tx_mode`.
    pub pn_mode: i16,
    /// Text size in points set by `TxSize` (`$000D` v2 / `$0D` v1,
    /// `Integer`). §A-3 Table A-2: 2-byte payload.
    pub tx_size: i16,
    /// Text scaling ratio set by `TxRatio` (`$0010` v2 / `$10` v1).
    /// §A-3 Table A-2: 8-byte payload — `(Point numerator, Point
    /// denominator)` where each `Point` is `(v, h)` i16-pair. Stored
    /// here as four signed 16-bit components in on-disk order to make
    /// the round-trip explicit.
    pub tx_ratio: TextRatio,
    /// Fractional pen position set by `PnLocHFrac` (`$0015` v2 /
    /// `$15` v1, low word of `Fixed`). §A-3 Table A-2: 2-byte payload.
    /// Default = 0.5 per the spec's note: *"if value is not 0.5, pen
    /// position is always set to the picture before each text-drawing
    /// operation."*
    pub pn_loc_h_frac: i16,
    /// Per-character extra-width adjustment set by `ChExtra`
    /// (`$0016` v2, `Integer`). §A-3 Table A-2: 2-byte payload.
    pub ch_extra: i16,
    /// Highlight colour set by `HiliteColor` (`$001D` v2,
    /// `RGBColor`). §A-3 Table A-2: 6-byte payload (3 × i16). `None`
    /// until the picture emits one — `DefHilite` (`$001E`) resets to
    /// the QuickDraw default, which depends on the low-memory globals
    /// at draw time, so we surface that as `None` rather than guess.
    pub hilite_color: Option<Rgba>,
    /// Arithmetic-transfer-mode opcolor set by `OpColor` (`$001F` v2,
    /// `RGBColor`). §A-3 Table A-2: 6-byte payload. Consumed by the
    /// `blend`, `addPin`, `addOver`, `subPin`, `addMax`, `subOver` and
    /// `addMin` arithmetic transfer modes, which the rasteriser honours
    /// on pattern fills and the `CopyBits` blit (see
    /// [`crate::raster::ArithMode`]); defaults per §4-39/4-40 apply
    /// when the picture never declared one.
    pub op_color: Option<Rgba>,
    /// `true` once a `DefHilite` (`$001E` v2) opcode was observed.
    /// Mutually-exclusive with `hilite_color` in practice — a producer
    /// that wants the system default highlight colour emits this
    /// opcode and clears the previously-set colour. We track the flag
    /// separately from `hilite_color: None` (which is also the default
    /// before any opcode is emitted) so encoder-side round-trips can
    /// reproduce the opcode.
    pub hilite_default: bool,
    /// `true` once a `HiliteMode` (`$001C` v2) opcode was observed.
    /// §A-3 Table A-2: 0-byte payload. The opcode is a "flag" that the
    /// next drawing operation uses the highlight mode; the producer
    /// emits it once per draw that needs it.
    pub hilite_mode_flag: bool,
    /// Last `fontName` (`$002C` v2 / `$2C` v1) record observed in the
    /// stream, decoded into a [`PictFontName`]. §A-3 Table A-2 footnote
    /// `*` — the v2 record is `dataLength (Integer)`, `oldFontID
    /// (Integer)`, `nameLength (0..255)`, `fontName (nameLength bytes)`
    /// — declares the font identity that subsequent text-glyph opcodes
    /// (`LongText / DH/DV/DHDVText`) belong to. Round 236.
    pub font_name: Option<PictFontName>,
    /// Last `lineJustify` (`$002D` v2) record observed, decoded into a
    /// [`PictLineJustify`]. §A-3 Table A-2 footnote `†` — Script-Manager
    /// line-layout state: `dataLength = 8`, `intercharacter spacing
    /// (Fixed)`, `total extra space (Fixed)`. Round 236.
    pub line_justify: Option<PictLineJustify>,
    /// Last `glyphState` (`$002E` v2) record observed, decoded into a
    /// [`PictGlyphState`]. §A-3 Table A-2 row 0x002E — the four
    /// preserved-glyph Booleans (`outline preferred`, `preserve glyph`,
    /// `fractional widths`, `scaling disabled`) carried inside a
    /// length-prefixed block. Round 236.
    pub glyph_state: Option<PictGlyphState>,
    /// QuickDraw **text-drawing pen location**, in picture-frame
    /// coordinates, after the most recent text-glyph opcode
    /// (`LongText $0028` / `DHText $0029` / `DVText $002A` /
    /// `DHDVText $002B`). Inside Macintosh: Imaging With QuickDraw, "About
    /// Basic QuickDraw" (book page 2-13): *"Text is drawn with the base
    /// line positioned at the pen location."* — so the `(h, v)` recorded
    /// here is where the text baseline sits.
    ///
    /// `LongText` carries an absolute `txLoc` Point that sets this slot
    /// directly; the compact `DHText` / `DVText` / `DHDVText` variants
    /// carry positive `(0..255)` deltas (§A-3 Table A-2) that advance the
    /// running text pen relative to the position left by the previous
    /// text opcode — which is precisely why the compact forms exist
    /// (successive `DrawText` calls on one line record only the
    /// increment). The first such delta opcode in a picture with no prior
    /// `LongText` advances from the graphics pen origin `(0, 0)`.
    ///
    /// `None` until the picture emits its first text opcode. Since
    /// round 352 the glyph bytes are rasterised (see [`crate::font`])
    /// and the slot also advances by each drawn glyph's width — the
    /// styled advance including the `txFace` `extra` widening (round
    /// 407) plus the `chExtra` / `spExtra` / `lineJustify` adjustments —
    /// so after a draw it reports where the next glyph would begin.
    /// Round 295.
    pub text_pen: Option<(i32, i32)>,
    /// Number of text-glyph opcodes (`LongText` / `DHText` / `DVText` /
    /// `DHDVText`) observed in the stream. `0` for a picture with no
    /// text. Round 295.
    pub text_op_count: u32,
}

impl PictTextState {
    /// Construct a fresh-GrafPort default state.
    ///
    /// `tx_size = 12` and `pn_loc_h_frac = 0x8000` (= 0.5) match the
    /// Color QuickDraw default a `NewWindow` call establishes; every
    /// other field defaults to zero.
    pub const fn fresh_graf_port() -> Self {
        Self {
            tx_font: 0,
            tx_face: PictTextFace::PLAIN,
            tx_mode: 0, // srcCopy
            sp_extra: crate::header::Fixed(0),
            pn_mode: 8, // patCopy — Inside Macintosh default
            tx_size: 12,
            tx_ratio: TextRatio {
                numer_v: 1,
                numer_h: 1,
                denom_v: 1,
                denom_h: 1,
            },
            // 0.5 as the low (fraction) word of a Fixed is the bit
            // pattern 0x8000 (= 0x8000 / 0x10000). The field mirrors
            // the on-disk signed Integer, so the pattern reads back as
            // i16::MIN — compare via `as u16` for the fraction value.
            // Inside Macintosh: Imaging With QuickDraw §4 (CGrafPort
            // initial-field table): "the low word of a Fixed number;
            // in decimal, its initial value is 0.5". (Round 401 fixes
            // the previous 0x4000 = 0.25 default.)
            pn_loc_h_frac: 0x8000u16 as i16,
            ch_extra: 0,
            hilite_color: None,
            op_color: None,
            hilite_default: false,
            hilite_mode_flag: false,
            font_name: None,
            line_justify: None,
            glyph_state: None,
            text_pen: None,
            text_op_count: 0,
        }
    }

    /// Resolve the captured `TxMode` (`$0005`) word into a structured
    /// [`SourceMode`].
    ///
    /// §A-3 Table A-2 describes `TxMode` as a *"Source mode (Integer)"* —
    /// the same numeric transfer-mode catalog the `CopyBits` raster
    /// records carry in their own `mode` word (§3 book pages 3-113..3-114
    /// Boolean source modes `srcCopy = 0` … `notSrcBic = 7`; §4 pages
    /// 4-38..4-43 arithmetic modes `blend = 32` … `adMin = 39` plus
    /// `hilite = 50`). Round 282 resolved that word for the raster blit;
    /// this accessor exposes the identical resolution for the text channel
    /// so round-trip tooling and content inspectors can read the producer's
    /// declared text transfer mode without re-deriving the numeric codes.
    ///
    /// The arithmetic / highlight modes pull their colour context from this
    /// same text state: `op_color` is the captured `OpColor` (`$001F`,
    /// defaulting per §4-39/4-40 when absent — max-pin → white, min-pin →
    /// black, blend → 50 % gray inside [`SourceMode::from_mode_word`]),
    /// `hilite_color` is the captured `HiliteColor` (`$001D`, falling back
    /// to `srcXor` per the §4-40 Table 4-2 revert when absent), and
    /// `bg_key` is supplied by the caller (the active background colour,
    /// the transparent-mode key §4 uses). Codes outside the catalog fold
    /// to `srcCopy`, the total-function posture every other mode resolver
    /// in this crate shares.
    ///
    /// The text rasteriser applies this resolution to every drawn glyph
    /// (round 352 onward), with two text-channel adjustments made at the
    /// draw site rather than here: a `srcCopy` word folds to `srcOr`
    /// (an opaque white box behind inline text is never what a picture
    /// intends), and `grayishTextOr = 49` — text-only, Inside Macintosh
    /// Volume VI page 17-17 — is intercepted before this resolver would
    /// fold it, drawing the glyphs in the fg/bg `blend_half` average
    /// (see [`crate::raster::GRAYISH_TEXT_OR_MODE`]).
    #[inline]
    pub fn tx_source_mode(&self, bg_key: Rgba) -> SourceMode {
        SourceMode::from_mode_word(self.tx_mode, self.op_color, bg_key, self.hilite_color)
    }
}

/// Decoded `fontName` (`$002C`) record per Inside Macintosh: Imaging
/// With QuickDraw §A-3 Table A-2 footnote `*`.
///
/// On-disk layout (after the 2-byte opcode):
///
/// ```text
/// dataLength : u16    // total payload size, including itself (5 + nameLen)
/// oldFontID  : i16    // legacy `FOND` resource ID
/// nameLen    : u8     // 0..255
/// name       : [u8; nameLen]
/// ```
///
/// The "old font ID" pairs with the `TxFont` opcode that the same
/// producer emitted earlier in the stream; the `name` is the
/// human-readable identifier (`Geneva`, `Chicago`, `Helvetica`, …) the
/// host font subsystem resolves it under. Stored byte-for-byte; this
/// crate makes no attempt at MacRoman → UTF-8 conversion because §A-3
/// is silent on the name encoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PictFontName {
    /// Pairs with the `TxFont` (`$0003`) opcode's `fontID`.
    pub old_font_id: i16,
    /// Font name bytes as they appeared on disk — `name_len` bytes long.
    pub name: Vec<u8>,
}

impl PictFontName {
    /// Construct a `PictFontName` from its decoded parts.
    pub fn new(old_font_id: i16, name: Vec<u8>) -> Self {
        Self { old_font_id, name }
    }
}

/// Decoded `lineJustify` (`$002D`) record per Inside Macintosh: Imaging
/// With QuickDraw §A-3 Table A-2 footnote `†`.
///
/// The Script-Manager line-layout state, serialised as two `Fixed`
/// 16.16 values:
///
/// * `inter_char_spacing` — extra width inserted between every glyph
///   pair (§A-3 footnote `†`'s "intercharacter spacing").
/// * `total_extra` — total extra width spread across the style run to
///   reach the justified line length.
///
/// Both are stored as raw `Fixed` (i32 16.16) so callers can convert
/// to `f32` via [`crate::header::Fixed::to_f32`] when needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PictLineJustify {
    pub inter_char_spacing: crate::header::Fixed,
    pub total_extra: crate::header::Fixed,
}

/// Decoded `glyphState` (`$002E`) record per Inside Macintosh: Imaging
/// With QuickDraw §A-3 Table A-2 row 0x002E.
///
/// Four 1-byte Boolean fields the QuickDraw text engine consults during
/// glyph rasterisation. Stored as `bool` values; the on-disk bytes are
/// `0` for `false`, any non-zero for `true`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PictGlyphState {
    /// `outline preferred` — request outline glyph shapes when the font
    /// offers them.
    pub outline_preferred: bool,
    /// `preserve glyph` — preserve the glyph metric across scaling.
    pub preserve_glyph: bool,
    /// `fractional widths` — honour sub-pixel advance widths.
    pub fractional_widths: bool,
    /// `scaling disabled` — skip glyph scaling.
    pub scaling_disabled: bool,
}

/// `TxRatio` payload: numerator and denominator `Point`s.
///
/// Each `Point` in §A-3 is `(v, h)` (vertical then horizontal i16) per
/// Table A-1 / §A-3. We preserve that ordering on the wire so encoder
/// round-trips stay explicit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextRatio {
    pub numer_v: i16,
    pub numer_h: i16,
    pub denom_v: i16,
    pub denom_h: i16,
}

impl Default for TextRatio {
    fn default() -> Self {
        Self {
            numer_v: 1,
            numer_h: 1,
            denom_v: 1,
            denom_h: 1,
        }
    }
}

/// Drawing state carried across the v2 opcode walk.
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
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
    /// Active multi-colour pen pattern (set by `PnPixPat`, opcode
    /// `0x0013`). When `Some`, overrides `pen_pat` for frame / paint
    /// verbs — every cell renders the resolved per-cell RGBA directly
    /// instead of selecting between current fg / bg per the monochrome
    /// stipple convention. Set back to `None` on the next plain `PnPat`
    /// opcode (round 91 mirrors classic QuickDraw's "set most-recent
    /// pattern" semantics).
    pub pen_pix_pat: Option<PixPattern>,
    /// Active multi-colour background pattern (`BkPixPat 0x0012`).
    /// Overrides `back_pat` for erase verbs when `Some`.
    pub back_pix_pat: Option<PixPattern>,
    /// Active multi-colour fill pattern (`FillPixPat 0x0014`). Overrides
    /// `fill_pat` for fill verbs when `Some`.
    pub fill_pix_pat: Option<PixPattern>,
    /// Picture Comments collected during the opcode walk, in stream
    /// order. Inside Macintosh: Imaging With QuickDraw §A-3 Table A-2
    /// (`$00A0` `ShortComment` / `$00A1` `LongComment`) and Table A-3
    /// (`$A0` / `$A1`). Surfaced on the final [`crate::PictImage`] so
    /// callers can recover annotation metadata that doesn't influence
    /// rasterisation.
    pub comments: Vec<crate::image::PictComment>,
    /// Embedded QuickTime image payloads captured from the
    /// `CompressedQuickTime $8200` / `UncompressedQuickTime $8201`
    /// opcodes, in stream order (round 401). Surfaced on the final
    /// [`crate::PictImage`].
    pub quicktime: Vec<crate::image::PictQuickTime>,
    /// Tracked text / pen-mode / highlight state. Updated by the §A-3
    /// state opcodes (`TxFont`, `TxFace`, `TxMode`, `SpExtra`,
    /// `PnMode`, `TxSize`, `TxRatio`, `PnLocHFrac`, `ChExtra`,
    /// `HiliteMode`, `HiliteColor`, `DefHilite`, `OpColor`) instead of
    /// having the payload bytes stepped past. Surfaced on
    /// [`crate::PictImage`] / [`crate::PictProbe`] as the producer's
    /// final declared text + transfer-mode state — round 230.
    pub text_state: PictTextState,
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
            pen_pix_pat: None,
            back_pix_pat: None,
            fill_pix_pat: None,
            comments: Vec::new(),
            quicktime: Vec::new(),
            text_state: PictTextState::fresh_graf_port(),
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
    fn pix_pattern_sample_wraps() {
        let mut pixels = [Rgba::BLACK; 64];
        // Top-left cell = red so we can spot wrapping.
        pixels[0] = Rgba::new(0xFF, 0, 0, 0xFF);
        // Bottom-right cell = green.
        pixels[63] = Rgba::new(0, 0xFF, 0, 0xFF);
        let pp = PixPattern::new(8, 8, pixels.to_vec(), Pattern::BLACK);
        assert_eq!(pp.sample(0, 0).r, 0xFF);
        assert_eq!(pp.sample(8, 8).r, 0xFF, "wraps modulo 8");
        assert_eq!(pp.sample(7, 7).g, 0xFF, "bottom-right is green");
        assert_eq!(pp.sample(-1, -1).g, 0xFF, "negative coords wrap");
    }

    #[test]
    fn pix_pattern_sample_wraps_non_8x8() {
        // A 4×2 tile: row 0 = red, row 1 = green; columns alternate.
        // Confirms the sampler wraps modulo width / height for the
        // arbitrary power-of-2 tile sizes §3 (book page 3-40) permits.
        let red = Rgba::new(0xFF, 0, 0, 0xFF);
        let green = Rgba::new(0, 0xFF, 0, 0xFF);
        let pixels = vec![red, red, red, red, green, green, green, green];
        let pp = PixPattern::new(4, 2, pixels, Pattern::BLACK);
        assert_eq!(pp.sample(0, 0), red);
        assert_eq!(pp.sample(3, 0), red);
        assert_eq!(pp.sample(4, 0), red, "wraps modulo width 4");
        assert_eq!(pp.sample(0, 1), green);
        assert_eq!(pp.sample(0, 2), red, "wraps modulo height 2");
        assert_eq!(pp.sample(-1, -1), green, "negative coords wrap");
    }

    #[test]
    fn pix_pattern_new_pads_short_pixels() {
        // A length mismatch is padded with black so the sampler invariant
        // holds.
        let pp = PixPattern::new(2, 2, vec![Rgba::WHITE], Pattern::BLACK);
        assert_eq!(pp.pixels.len(), 4);
        assert_eq!(pp.sample(0, 0), Rgba::WHITE);
        assert_eq!(pp.sample(1, 0), Rgba::BLACK);
    }

    #[test]
    fn pix_pattern_degenerate_is_black() {
        let pp = PixPattern::new(0, 0, vec![], Pattern::BLACK);
        assert_eq!(pp.sample(0, 0), Rgba::BLACK);
    }

    #[test]
    fn pict_pattern_mono_unwrap() {
        let p = PictPattern::Mono(Pattern([0xAA; 8]));
        assert_eq!(p.mono(), Pattern([0xAA; 8]));
        let cp = PictPattern::ColourPixmap(Box::new(PixPattern::new(
            8,
            8,
            vec![Rgba::BLACK; 64],
            Pattern([0x55; 8]),
        )));
        assert_eq!(cp.mono(), Pattern([0x55; 8]));
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

    #[test]
    fn pict_text_face_plain_default() {
        let f = PictTextFace::default();
        assert_eq!(f, PictTextFace::PLAIN);
        assert_eq!(f.bits(), 0);
        assert!(f.is_plain());
        assert!(!f.bold());
        assert!(!f.italic());
        assert!(!f.underline());
        assert!(!f.outline());
        assert!(!f.shadow());
        assert!(!f.condense());
        assert!(!f.extend());
    }

    #[test]
    fn pict_text_face_named_bit_predicates() {
        // Per §A-3 caption: bold=0x01, italic=0x02, underline=0x04,
        // outline=0x08, shadow=0x10, condense=0x20, extend=0x40.
        assert!(PictTextFace::from(0x01).bold());
        assert!(PictTextFace::from(0x02).italic());
        assert!(PictTextFace::from(0x04).underline());
        assert!(PictTextFace::from(0x08).outline());
        assert!(PictTextFace::from(0x10).shadow());
        assert!(PictTextFace::from(0x20).condense());
        assert!(PictTextFace::from(0x40).extend());

        // 0x05 = bold + underline (the round-230 test value).
        let f = PictTextFace::from(0x05);
        assert!(f.bold());
        assert!(!f.italic());
        assert!(f.underline());
        assert!(!f.outline());
        assert!(!f.is_plain());
    }

    #[test]
    fn pict_text_face_all_named_bits_set() {
        // 0x7F = every named bit (bold..extend) set; bit 7 stays clear.
        let f = PictTextFace::from(0x7F);
        assert!(f.bold());
        assert!(f.italic());
        assert!(f.underline());
        assert!(f.outline());
        assert!(f.shadow());
        assert!(f.condense());
        assert!(f.extend());
        assert!(!f.is_plain());
        assert_eq!(f.bits(), 0x7F);
    }

    #[test]
    fn pict_text_face_reserved_bit7_ignored_by_is_plain() {
        // Reserved bit 7 (`0x80`) is preserved on the raw byte but does
        // not flip `is_plain` — §A-3's caption only names bits 0..=6.
        let f = PictTextFace::from(0x80);
        assert!(f.is_plain());
        assert_eq!(f.bits(), 0x80);
        assert!(!f.bold());
    }

    #[test]
    fn pict_text_face_u8_roundtrip() {
        // `From<u8>` ↔ `Into<u8>` preserves the on-disk byte verbatim
        // for the whole 0..=255 range — no bit is dropped or aliased.
        for byte in 0u8..=255 {
            let f = PictTextFace::from(byte);
            assert_eq!(u8::from(f), byte);
            assert_eq!(f.bits(), byte);
        }
    }

    #[test]
    fn pict_text_face_equality_with_u8() {
        // PartialEq<u8> lets existing call sites that compared
        // `tx_face == 0x05` keep working through the typed migration.
        let f = PictTextFace::from(0x05);
        assert!(f == 0x05u8);
        assert!(0x05u8 == f);
        assert!(f != 0x06u8);
    }

    #[test]
    fn pict_text_face_fresh_graf_port_is_plain() {
        // The §A-3 fresh-GrafPort default is plain text (no style bits).
        let ts = PictTextState::fresh_graf_port();
        assert_eq!(ts.tx_face, PictTextFace::PLAIN);
        assert!(ts.tx_face.is_plain());
        // Round-230 callers comparing the field to a raw byte still work
        // through the `PartialEq<u8>` impl.
        assert!(ts.tx_face == 0u8);
    }

    #[test]
    fn tx_source_mode_fresh_graf_port_is_src_copy() {
        // §A-3: fresh-GrafPort `TxMode` is `srcCopy = 0`.
        let ts = PictTextState::fresh_graf_port();
        assert_eq!(ts.tx_source_mode(Rgba::WHITE), SourceMode::SrcCopy);
    }

    #[test]
    fn tx_source_mode_resolves_boolean_modes() {
        // The eight §3 Boolean source modes map by their numeric code,
        // identical to the `CopyBits` raster `mode` word.
        let mut ts = PictTextState::fresh_graf_port();
        for (code, expect) in [
            (0, SourceMode::SrcCopy),
            (1, SourceMode::SrcOr),
            (2, SourceMode::SrcXor),
            (3, SourceMode::SrcBic),
            (4, SourceMode::NotSrcCopy),
            (5, SourceMode::NotSrcOr),
            (6, SourceMode::NotSrcXor),
            (7, SourceMode::NotSrcBic),
        ] {
            ts.tx_mode = code;
            assert_eq!(ts.tx_source_mode(Rgba::WHITE), expect);
        }
    }

    #[test]
    fn tx_source_mode_arith_uses_captured_op_color() {
        // An arithmetic `TxMode` (`addOver = 34`) carries the captured
        // `OpColor` through to the resolved `SourceMode::Arith`.
        let mut ts = PictTextState::fresh_graf_port();
        ts.tx_mode = 34;
        ts.op_color = Some(Rgba::new(0x10, 0x20, 0x30, 0xFF));
        let bg = Rgba::new(0x40, 0x50, 0x60, 0xFF);
        match ts.tx_source_mode(bg) {
            SourceMode::Arith {
                mode,
                op_color,
                bg_key,
            } => {
                assert_eq!(mode, crate::raster::ArithMode::AddOver);
                assert_eq!(op_color, Rgba::new(0x10, 0x20, 0x30, 0xFF));
                assert_eq!(bg_key, bg);
            }
            other => panic!("expected Arith, got {other:?}"),
        }
    }

    #[test]
    fn tx_source_mode_hilite_falls_back_without_color() {
        // §4-40 Table 4-2: `hilite` with no `HiliteColor` reverts to
        // `srcXor`; with one it resolves to `SourceMode::Hilite`.
        let mut ts = PictTextState::fresh_graf_port();
        ts.tx_mode = crate::raster::HILITE_MODE;
        assert_eq!(ts.tx_source_mode(Rgba::WHITE), SourceMode::SrcXor);
        ts.hilite_color = Some(Rgba::new(0xFF, 0, 0, 0xFF));
        let bg = Rgba::new(0xA0, 0xFF, 0xE0, 0xFF);
        assert_eq!(
            ts.tx_source_mode(bg),
            SourceMode::Hilite {
                hilite: Rgba::new(0xFF, 0, 0, 0xFF),
                bg_key: bg,
            }
        );
    }
}
