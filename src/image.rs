//! Standalone image container returned by `oxideav-pict`'s framework-free
//! decode API.
//!
//! Defined here (rather than reusing `oxideav_core::VideoFrame`) so the
//! crate can be built with the default `registry` feature off — i.e.
//! without depending on `oxideav-core` at all. When the `registry`
//! feature is on the [`crate::registry`] module provides the matching
//! [`PictPixelFormat`] -> `oxideav_core::PixelFormat` mapping so the
//! trait-side `Decoder` impl keeps working unchanged.

use crate::header::PictHeader;

/// One Picture Comment captured from the opcode stream.
///
/// Inside Macintosh: Imaging With QuickDraw §A-3 Table A-2 lists two
/// comment opcodes that carry application-defined metadata alongside
/// the drawing-state stream:
///
/// * `ShortComment` (`$00A0` v2 / `$A0` v1) — 2-byte `Kind (Integer)`
///   payload, no associated data block.
/// * `LongComment` (`$00A1` v2 / `$A1` v1) — 2-byte `Kind (Integer)` +
///   2-byte `size (Integer)` byte count + `size` raw bytes.
///
/// The decoder records the on-disk `kind` word verbatim (the spec
/// reserves the integer space for Apple-internal and registered
/// third-party identifiers) and, for `LongComment`, owns the data slice
/// so the picture's comment annotations survive the rasterisation
/// step. The drawing-state machine itself ignores comment payloads —
/// they exist purely as a passive metadata channel.
///
/// PICT generators historically used Picture Comments to annotate the
/// drawing stream with PostScript fragments, application-specific
/// drawing hints, page breaks, and font / line-style overrides; a
/// `LongComment` data block holds whatever bytes the producing
/// application chose to write, so the decoder leaves interpretation up
/// to the consumer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PictComment {
    /// `Kind` word as written on disk (16-bit signed in the spec, but
    /// always serialised as an unsigned 16-bit pattern in PICT files
    /// — we expose the raw u16 so callers can compare against the
    /// in-spec `Kind` values without sign-conversion).
    pub kind: u16,
    /// Raw `LongComment` data payload. Empty for `ShortComment` (which
    /// has no data block per §A-3) and for `LongComment` with a zero
    /// `size` field.
    pub data: Vec<u8>,
    /// `true` when the comment was emitted as `LongComment`
    /// (`$00A1` / `$A1`); `false` for `ShortComment` (`$00A0` / `$A0`).
    /// Carries the v2-vs-v1 distinction *only* via this flag — the
    /// kind / data round-trip is identical across the two framings.
    pub is_long: bool,
}

impl PictComment {
    /// Construct a `ShortComment` record (kind only, no data block).
    pub fn short(kind: u16) -> Self {
        Self {
            kind,
            data: Vec::new(),
            is_long: false,
        }
    }

    /// Construct a `LongComment` record from `kind` + an owned data
    /// slice. The spec's `size` word is implicit in `data.len()` and
    /// must fit in a `u16` (the encoder errors on overflow).
    pub fn long(kind: u16, data: Vec<u8>) -> Self {
        Self {
            kind,
            data,
            is_long: true,
        }
    }
}

/// Pixel layout used by [`PictImage`].
///
/// The decoder always normalises to [`PictPixelFormat::Rgba`]: 1-bit
/// PackBitsRect bitmaps are expanded to black / white RGBA, 16-bit
/// pixels (Apple A1R5G5B5) are expanded to 8-bit RGBA, and 32-bit
/// pixels (XRGB on disk) are repacked as RGBA. Consumers therefore
/// never need to know the on-disk pixel layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PictPixelFormat {
    /// 8-bit packed RGBA, 4 bytes per pixel.
    Rgba,
}

/// One decoded PICT raster, framework-free shape.
///
/// `pts` is `None` for the standalone [`crate::parse_pict`] entry
/// point. The registry-backed `Decoder` impl still passes `pts`
/// through from the surrounding `Packet`.
#[derive(Debug, Clone)]
pub struct PictImage {
    /// Picture width in pixels (right - left of the PixMap bounds, not
    /// of `picFrame`; the two can differ because `picFrame` is the
    /// drawing-rectangle while the PixMap has its own bounds).
    pub width: u32,
    /// Picture height in pixels (bottom - top of the PixMap bounds).
    pub height: u32,
    /// Pixel layout the `data` carries. Always
    /// [`PictPixelFormat::Rgba`] in round 1.
    pub pixel_format: PictPixelFormat,
    /// Row-major pixel bytes,
    /// `width × bytes_per_pixel(pixel_format) × height` long.
    pub data: Vec<u8>,
    /// Optional presentation timestamp. Always `None` from the
    /// standalone decode path.
    pub pts: Option<i64>,
    /// Decoded form of the 24-byte v2 `HeaderOp` (`0x0C00`) payload
    /// (Inside Macintosh: Imaging With QuickDraw §A-3 / §A-22 Listing
    /// A-5 + A-6).
    ///
    /// * `Some(PictHeader::ExtendedV2 { .. })` — `OpenCPicture` PICTs
    ///   (`version=-2`), carrying explicit hRes / vRes / optimal-source
    ///   rectangle.
    /// * `Some(PictHeader::V2 { .. })` — `OpenPicture`-in-CGrafPort
    ///   PICTs (`version=-1`), carrying a fixed-point bounding box.
    /// * `None` — v1 PICTs (no `HeaderOp` per §A-25) and v2 PICTs whose
    ///   header version word doesn't match either of the §A-3 values.
    pub header: Option<PictHeader>,
    /// Picture Comments captured during the opcode walk, in stream
    /// order. Inside Macintosh: Imaging With QuickDraw §A-3 — `$00A0`
    /// `ShortComment` / `$00A1` `LongComment` for v2 and `$A0` /
    /// `$A1` for v1 share the same record layout via [`PictComment`].
    /// Empty for PICTs that emit no comment opcodes.
    pub comments: Vec<PictComment>,
}

impl PictImage {
    /// Bytes-per-pixel implied by `pixel_format`.
    pub fn bytes_per_pixel(&self) -> usize {
        match self.pixel_format {
            PictPixelFormat::Rgba => 4,
        }
    }

    /// Bytes per row (width × bpp).
    pub fn stride(&self) -> usize {
        self.width as usize * self.bytes_per_pixel()
    }
}
