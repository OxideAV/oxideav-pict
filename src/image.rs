//! The standalone image types: the shapes every `oxideav-<format>`
//! image crate shares (`IMAGE_CRATE_API`), specialised for PICT.
//!
//! * [`PictImage`] — the rasterised picture [`crate::decode`] returns
//!   and [`crate::encode`] consumes: dimensions, a [`PixelFormat`]
//!   tag, one packed [`Plane`], [`ColorInfo`], [`Metadata`], plus the
//!   PICT extras — the v2 [`PictHeader`], the Picture Comments, the
//!   embedded QuickTime payloads and the final text / pen state.
//! * [`RgbImage`] / [`RgbaImage`] — the tightly packed 8-bit raw paths
//!   ([`crate::decode_rgb8`] / [`crate::decode_rgba8`],
//!   [`PictImage::to_rgb8`] / [`PictImage::to_rgba8`]).
//! * [`ImageInfo`] — what [`crate::info`] reads from the picture record
//!   header and the version stanza without walking a single drawing
//!   opcode.
//!
//! Defined here (rather than reusing `oxideav_core::VideoFrame`) so the
//! crate builds with the default `registry` feature off — i.e. without
//! depending on `oxideav-core` at all. With `registry` on,
//! `crate::registry` adds the `From<PictImage> for VideoFrame`
//! conversion and its inverse so the framework `Decoder` / `Encoder`
//! are thin adapters over the same functions.

use crate::error::{PictError, Result};
use crate::header::PictHeader;
use crate::state::PictTextState;

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

/// One embedded QuickTime image payload captured from the opcode
/// stream (round 401; typed interior round 435).
///
/// Inside Macintosh: Imaging With QuickDraw §A-3 Table A-2 defines two
/// QuickTime opcodes, each carrying `Data length (Long)` followed by
/// `data length` bytes:
///
/// * `CompressedQuickTime` (`$8200`) — a compressed embedded image
///   (typically JPEG in late-1990s PICT files).
/// * `UncompressedQuickTime` (`$8201`) — the uncompressed variant,
///   wrapping one ordinary `$98`–`$9B` pixel-data subopcode.
///
/// §A-3 calls the bytes "private to QuickTime"; their layout is
/// published in Inside Macintosh: QuickTime (1993) Chapter 3 (see
/// [`crate::quicktime`]). The decoder keeps the verbatim capture in
/// [`Self::data`] *and* attaches the typed view in [`Self::image`]
/// when the interior matches the published layout. Per page 3-26 the
/// `Size` field is authoritative even for a reader that cannot decode
/// the payload, so an interior that fails the typed parse degrades to
/// the verbatim capture (`image == None`) instead of failing the
/// picture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PictQuickTime {
    /// `true` for `CompressedQuickTime` (`$8200`); `false` for
    /// `UncompressedQuickTime` (`$8201`).
    pub compressed: bool,
    /// The raw payload bytes following the `Data length` Long, exactly
    /// as stored.
    pub data: Vec<u8>,
    /// Typed view of `data` per Inside Macintosh: QuickTime Tables
    /// 3-1 / 3-2 — the opcode wrapper fields (display matrix, matte,
    /// mask region, transfer mode, source rect), the embedded
    /// `ImageDescription` with its compressor FourCC, and the image
    /// payload. `None` when the interior did not match the published
    /// layout (the verbatim `data` capture still stands).
    pub image: Option<crate::quicktime::QuickTimePayload>,
    /// What became of the opcode's pixels (round 461): rendered onto
    /// the canvas (with the destination rectangle), unsupported
    /// compressor, decode failure, or not attempted because the
    /// interior did not parse. See [`crate::qtimage::QuickTimeRender`].
    pub render: crate::qtimage::QuickTimeRender,
}

// ---------------------------------------------------------------------------
// Contract types (IMAGE_CRATE_API)
// ---------------------------------------------------------------------------

/// Pixel layouts the standalone `oxideav-pict` API can produce / consume.
///
/// Variant names mirror `oxideav_core::PixelFormat` exactly, so the
/// `crate::registry` conversion layer is a 1:1 match by name.
///
/// [`crate::decode`] always returns [`Rgba`](PictPixelFormat::Rgba):
/// PICT is a drawing-command stream, and the rasteriser's canvas is
/// 8-bit RGBA — 1-bit bitmaps, indexed PixMaps (through their colour
/// table), 16-bit A1R5G5B5 and 32-bit XRGB direct pixels, vector
/// shapes and text all land on that one canvas. QuickDraw carries no
/// alpha, so the decoded alpha byte is always `255`. There is no
/// `Pal8` output: an indexed PixMap is one opcode among many and the
/// canvas it is drawn onto is not indexed.
///
/// [`Rgb24`](PictPixelFormat::Rgb24) is an *input* layout
/// ([`PictImage::from_rgb8`], [`crate::encode_rgb8`]): the encoder
/// writes it as the same RGBDirect PixMap it writes `Rgba` to
/// (`packType` 2 is literally 3 bytes per pixel), and it decodes back
/// as `Rgba` with alpha `255`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PictPixelFormat {
    /// 8-bit packed RGBA, 4 bytes per pixel — the decode layout.
    Rgba,
    /// 8-bit packed RGB, 3 bytes per pixel — accepted by the encoder.
    Rgb24,
}

/// The contract name for [`PictPixelFormat`].
pub type PixelFormat = PictPixelFormat;

impl PictPixelFormat {
    /// Bytes per pixel of the packed layout.
    pub fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Rgba => 4,
            Self::Rgb24 => 3,
        }
    }

    /// `true` when the layout carries an alpha channel.
    pub fn has_alpha(self) -> bool {
        matches!(self, Self::Rgba)
    }
}

/// One plane of pixel data: a row stride plus the row-major bytes.
/// Every PICT layout is packed, so a [`PictImage`] has exactly one.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Plane {
    /// Bytes per row.
    pub stride: usize,
    /// Row-major bytes, at least `stride × height` long.
    pub data: Vec<u8>,
}

impl Plane {
    /// Wrap a plane buffer with its row stride.
    pub fn new(stride: usize, data: Vec<u8>) -> Self {
        Self { stride, data }
    }
}

/// Nominal sample range (H.273 `VideoFullRangeFlag`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum ColorRange {
    /// No range was signalled.
    #[default]
    Unspecified,
    /// Limited (video / studio) range: `VideoFullRangeFlag == 0`.
    Limited,
    /// Full (PC) range: `VideoFullRangeFlag == 1`.
    Full,
}

/// Colour signalling of an image: the sample range plus the H.273
/// `ColourPrimaries` / `TransferCharacteristics` /
/// `MatrixCoefficients` code points (`2` = unspecified).
///
/// QuickDraw's `RGBColor` is device RGB: the picture format signals no
/// primaries, transfer or ICC profile (Inside Macintosh: Imaging With
/// QuickDraw describes colour in terms of the device's colour table
/// and the Color Manager, not a colour space), so [`crate::decode`]
/// always fills [`ColorInfo::pict_default`] — full-range RGB
/// (`matrix` 0) with unspecified primaries and transfer. This is the
/// crate's documented convention, not a value read from the file, and
/// it is therefore not stamped on registry frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct ColorInfo {
    /// Sample range.
    pub range: ColorRange,
    /// H.273 `ColourPrimaries` code point (`1` = BT.709 / sRGB, `2` =
    /// unspecified).
    pub primaries: u8,
    /// H.273 `TransferCharacteristics` code point (`13` = sRGB, `2` =
    /// unspecified).
    pub transfer: u8,
    /// H.273 `MatrixCoefficients` code point (`0` = identity / RGB).
    pub matrix: u8,
}

impl ColorInfo {
    /// H.273 "unspecified" code point.
    pub const UNSPECIFIED: u8 = 2;
    /// H.273 `MatrixCoefficients` identity (RGB / GBR) code point.
    pub const MATRIX_IDENTITY: u8 = 0;
    /// H.273 `ColourPrimaries` BT.709 / sRGB code point.
    pub const PRIMARIES_BT709: u8 = 1;
    /// H.273 `TransferCharacteristics` IEC 61966-2-1 sRGB code point.
    pub const TRANSFER_SRGB: u8 = 13;

    /// Build a description from its four parts.
    pub const fn new(range: ColorRange, primaries: u8, transfer: u8, matrix: u8) -> Self {
        Self {
            range,
            primaries,
            transfer,
            matrix,
        }
    }

    /// Every field unspecified.
    pub const fn unspecified() -> Self {
        Self::new(
            ColorRange::Unspecified,
            Self::UNSPECIFIED,
            Self::UNSPECIFIED,
            Self::UNSPECIFIED,
        )
    }

    /// PICT's documented default (the format signals no colour space):
    /// full-range RGB (`matrix` 0) with unspecified primaries and
    /// transfer.
    pub const fn pict_default() -> Self {
        Self::new(
            ColorRange::Full,
            Self::UNSPECIFIED,
            Self::UNSPECIFIED,
            Self::MATRIX_IDENTITY,
        )
    }

    /// sRGB (IEC 61966-2-1): BT.709 primaries, sRGB transfer, identity
    /// matrix, full range.
    pub const fn srgb() -> Self {
        Self::new(
            ColorRange::Full,
            Self::PRIMARIES_BT709,
            Self::TRANSFER_SRGB,
            Self::MATRIX_IDENTITY,
        )
    }

    /// Set the range.
    pub fn with_range(mut self, range: ColorRange) -> Self {
        self.range = range;
        self
    }

    /// Set the primaries code point.
    pub fn with_primaries(mut self, primaries: u8) -> Self {
        self.primaries = primaries;
        self
    }

    /// Set the transfer code point.
    pub fn with_transfer(mut self, transfer: u8) -> Self {
        self.transfer = transfer;
        self
    }

    /// Set the matrix code point.
    pub fn with_matrix(mut self, matrix: u8) -> Self {
        self.matrix = matrix;
        self
    }

    /// `true` when both primaries and transfer are specified (`!= 2`).
    pub fn is_specified(&self) -> bool {
        self.primaries != Self::UNSPECIFIED && self.transfer != Self::UNSPECIFIED
    }
}

impl Default for ColorInfo {
    /// [`ColorInfo::pict_default`].
    fn default() -> Self {
        Self::pict_default()
    }
}

/// The metadata blobs every image crate surfaces: an ICC profile, an
/// Exif payload, an XMP packet and a file gamma. The PICT opcode set
/// has no carrier for any of them, so all four are always `None` from
/// [`crate::decode`] and ignored by [`crate::encode`]. The format's own
/// annotation channel — Picture Comments (`ShortComment` /
/// `LongComment`, §A-3) — is the typed extra
/// [`PictImage::comments`] instead, and is written back by the encoder.
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct Metadata {
    /// ICC profile bytes. PICT has no carrier; always `None` on decode.
    pub icc: Option<Vec<u8>>,
    /// Exif payload. PICT has no carrier; always `None` on decode.
    pub exif: Option<Vec<u8>>,
    /// XMP packet. PICT has no carrier; always `None` on decode.
    pub xmp: Option<Vec<u8>>,
    /// Encoding gamma. PICT has no carrier; always `None` on decode.
    pub gamma: Option<f32>,
}

impl Metadata {
    /// Empty metadata.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set (or clear) the ICC profile.
    pub fn with_icc(mut self, icc: impl Into<Option<Vec<u8>>>) -> Self {
        self.icc = icc.into();
        self
    }

    /// Set (or clear) the Exif payload.
    pub fn with_exif(mut self, exif: impl Into<Option<Vec<u8>>>) -> Self {
        self.exif = exif.into();
        self
    }

    /// Set (or clear) the XMP packet.
    pub fn with_xmp(mut self, xmp: impl Into<Option<Vec<u8>>>) -> Self {
        self.xmp = xmp.into();
        self
    }

    /// Set (or clear) the file gamma.
    pub fn with_gamma(mut self, gamma: impl Into<Option<f32>>) -> Self {
        self.gamma = gamma.into();
        self
    }

    /// `true` when no field is set.
    pub fn is_empty(&self) -> bool {
        self.icc.is_none() && self.exif.is_none() && self.xmp.is_none() && self.gamma.is_none()
    }
}

/// Which opcode framing a picture uses (Inside Macintosh: Imaging With
/// QuickDraw §A-3): version 1 (8-bit opcodes, `$11 $01` stanza) or
/// version 2 (16-bit word-aligned opcodes, `$0011 $02FF` stanza + the
/// `$0C00` `HeaderOp`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum PictVersion {
    /// Version 1 — 8-bit opcodes, monochrome BitMap rasters (plus the
    /// de-facto `$9A` DirectBitsRect extension this crate reads and
    /// writes).
    V1,
    /// Version 2 — 16-bit opcodes, Color QuickDraw.
    #[default]
    V2,
}

/// A rasterised PICT in its native layout, as returned by
/// [`crate::decode`] and consumed by [`crate::encode`].
///
/// `planes` holds exactly one packed plane whose row stride is
/// `width × bytes_per_pixel`; `format` is [`PictPixelFormat::Rgba`]
/// for every decoded picture (alpha `255` throughout — QuickDraw
/// carries no alpha); `color` is [`ColorInfo::pict_default`];
/// `metadata` is empty (PICT has no carrier). The PICT extras —
/// `header`, `comments`, `quicktime`, `text_state` — are filled from
/// the opcode walk; [`crate::encode`] writes `header` (resolution,
/// header kind) and `comments` back unless the
/// [`EncodeOptions`](crate::EncodeOptions) override them.
///
/// `width` / `height` are the picture frame's (`picFrame`): the canvas
/// every opcode is drawn onto. A vector-only picture (no raster
/// opcode) is rasterised at that frame's pixel size — PICT coordinates
/// are 72-dpi pixels — with white "paper" where nothing was drawn.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct PictImage {
    /// Picture width in pixels (`picFrame.right − picFrame.left`).
    pub width: u32,
    /// Picture height in pixels (`picFrame.bottom − picFrame.top`).
    pub height: u32,
    /// Pixel layout of the plane. Always [`PictPixelFormat::Rgba`] from
    /// the decoder.
    pub format: PixelFormat,
    /// Exactly one packed plane.
    pub planes: Vec<Plane>,
    /// Colour signalling — [`ColorInfo::pict_default`] from the decoder.
    pub color: ColorInfo,
    /// ICC / Exif / XMP / gamma — all `None` from the decoder.
    pub metadata: Metadata,
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
    /// Embedded QuickTime image payloads (`CompressedQuickTime $8200`
    /// / `UncompressedQuickTime $8201`) captured during the opcode
    /// walk, in stream order (round 401; typed interiors round 435).
    /// Each entry carries the verbatim bytes ([`PictQuickTime::data`])
    /// plus the typed [`crate::quicktime::QuickTimePayload`] view
    /// ([`PictQuickTime::image`]) when the interior matched the
    /// Inside Macintosh: QuickTime Table 3-1 / 3-2 layout — including
    /// the compressor FourCC a consumer routes to the matching
    /// decoder. Empty for PICTs without QuickTime opcodes.
    pub quicktime: Vec<PictQuickTime>,
    /// Final tracked text / pen-mode / highlight state as observed by
    /// the opcode walker.
    ///
    /// Inside Macintosh: Imaging With QuickDraw §A-3 Table A-2 / A-3
    /// list a handful of opcodes — `TxFont $0003`, `TxFace $0004`,
    /// `TxMode $0005`, `SpExtra $0006`, `PnMode $0008`,
    /// `TxSize $000D`, `TxRatio $0010`, `PnLocHFrac $0015`,
    /// `ChExtra $0016`, `HiliteMode $001C`, `HiliteColor $001D`,
    /// `DefHilite $001E`, `OpColor $001F` — that carry text-shape,
    /// transfer-mode, highlight-colour and arithmetic-transfer-mode
    /// parameters. Round 230 captures their payloads into
    /// [`PictTextState`] so consumers (and round-trip encoders) can
    /// recover the values the producer declared.
    ///
    /// Defaults to [`PictTextState::fresh_graf_port`] when the picture
    /// emits no state opcode in the corresponding slot.
    pub text_state: PictTextState,
}

impl PictImage {
    /// Assemble an image from its geometry, layout and planes (one for
    /// PICT). Colour is [`ColorInfo::pict_default`], metadata empty, no
    /// header, no comments, fresh-GrafPort text state; the `with_*`
    /// builders fill those in.
    ///
    /// The plane geometry is validated so an invalid image cannot be
    /// built here: non-zero dimensions, exactly one plane, a stride of
    /// at least `width × bytes_per_pixel`, and at least `stride ×
    /// height` bytes of data ([`PictError::InvalidData`] otherwise).
    pub fn new(width: u32, height: u32, format: PixelFormat, planes: Vec<Plane>) -> Result<Self> {
        let img = Self::unchecked(width, height, format, planes);
        img.validate()?;
        Ok(img)
    }

    pub(crate) fn unchecked(
        width: u32,
        height: u32,
        format: PixelFormat,
        planes: Vec<Plane>,
    ) -> Self {
        Self {
            width,
            height,
            format,
            planes,
            color: ColorInfo::pict_default(),
            metadata: Metadata::default(),
            header: None,
            comments: Vec::new(),
            quicktime: Vec::new(),
            text_state: PictTextState::fresh_graf_port(),
        }
    }

    /// A packed image over `data` with the layout's tight stride
    /// (`width × bytes_per_pixel`), validated like [`PictImage::new`].
    pub fn packed(width: u32, height: u32, format: PixelFormat, data: Vec<u8>) -> Result<Self> {
        let stride = (width as usize)
            .checked_mul(format.bytes_per_pixel())
            .ok_or_else(|| PictError::invalid("PICT: row size overflows"))?;
        Self::new(width, height, format, vec![Plane::new(stride, data)])
    }

    /// A packed `Rgb24` image over `data` (`3 × width × height` bytes,
    /// row-major, stride `3 × width`); [`PictError::InvalidData`] when
    /// the buffer is shorter than that.
    pub fn from_rgb8(width: u32, height: u32, data: Vec<u8>) -> Result<Self> {
        Self::packed(width, height, PixelFormat::Rgb24, data)
    }

    /// A packed `Rgba` image over `data` (`4 × width × height` bytes).
    /// PICT carries no alpha: the encoder writes the colour bytes only
    /// and the picture decodes back with alpha `255`.
    pub fn from_rgba8(width: u32, height: u32, data: Vec<u8>) -> Result<Self> {
        Self::packed(width, height, PixelFormat::Rgba, data)
    }

    /// Set the colour signalling.
    pub fn with_color(mut self, color: ColorInfo) -> Self {
        self.color = color;
        self
    }

    /// Set the metadata record.
    pub fn with_metadata(mut self, metadata: Metadata) -> Self {
        self.metadata = metadata;
        self
    }

    /// Set (or clear) the v2 picture header.
    pub fn with_header(mut self, header: impl Into<Option<PictHeader>>) -> Self {
        self.header = header.into();
        self
    }

    /// Set the Picture Comments.
    pub fn with_comments(mut self, comments: Vec<PictComment>) -> Self {
        self.comments = comments;
        self
    }

    /// Set the final text / pen state record.
    pub fn with_text_state(mut self, text_state: PictTextState) -> Self {
        self.text_state = text_state;
        self
    }

    /// Width in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Height in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Native layout.
    pub fn format(&self) -> PixelFormat {
        self.format
    }

    /// Bytes per pixel of the native layout.
    pub fn bytes_per_pixel(&self) -> usize {
        self.format.bytes_per_pixel()
    }

    /// Bytes per row of the single plane (its stride).
    pub fn stride(&self) -> usize {
        self.planes
            .first()
            .map(|p| p.stride)
            .unwrap_or(self.width as usize * self.bytes_per_pixel())
    }

    /// The single packed plane's bytes (`Some` for every PICT layout).
    pub fn as_bytes(&self) -> Option<&[u8]> {
        self.planes.first().map(|p| p.data.as_slice())
    }

    /// The pixel bytes (the single plane; empty if there is none).
    pub fn data(&self) -> &[u8] {
        self.planes
            .first()
            .map(|p| p.data.as_slice())
            .unwrap_or(&[])
    }

    /// Mutable pixel bytes.
    pub fn data_mut(&mut self) -> &mut [u8] {
        match self.planes.first_mut() {
            Some(p) => p.data.as_mut_slice(),
            None => &mut [],
        }
    }

    /// Consume into the pixel bytes (the single plane; planes
    /// concatenated in order if a caller assembled more than one).
    pub fn into_raw(self) -> Vec<u8> {
        let mut planes = self.planes;
        match planes.len() {
            1 => planes.pop().map(|p| p.data).unwrap_or_default(),
            _ => planes.into_iter().flat_map(|p| p.data).collect(),
        }
    }

    /// `true` when the layout carries alpha.
    pub fn has_alpha(&self) -> bool {
        self.format.has_alpha()
    }

    /// Check the plane geometry: non-zero dimensions, exactly one
    /// plane, `stride ≥ width × bytes_per_pixel`, `data.len() ≥ stride
    /// × height`.
    pub fn validate(&self) -> Result<()> {
        if self.width == 0 || self.height == 0 {
            return Err(PictError::invalid(format!(
                "PICT: degenerate image {}×{}",
                self.width, self.height
            )));
        }
        if self.planes.len() != 1 {
            return Err(PictError::invalid(format!(
                "PICT: packed layout needs exactly one plane, got {}",
                self.planes.len()
            )));
        }
        let plane = &self.planes[0];
        let min_stride = (self.width as usize)
            .checked_mul(self.bytes_per_pixel())
            .ok_or_else(|| PictError::invalid("PICT: row size overflows"))?;
        if plane.stride < min_stride {
            return Err(PictError::invalid(format!(
                "PICT: stride {} shorter than {} bytes per row",
                plane.stride, min_stride
            )));
        }
        let need = plane
            .stride
            .checked_mul(self.height as usize)
            .ok_or_else(|| PictError::invalid("PICT: plane size overflows"))?;
        if plane.data.len() < need {
            return Err(PictError::invalid(format!(
                "PICT: plane has {} bytes, geometry needs {need}",
                plane.data.len()
            )));
        }
        Ok(())
    }

    /// Tightly packed 8-bit RGBA, alpha `255` where the layout has
    /// none. Exact for every native layout (`Rgba` is copied with the
    /// stride padding dropped, `Rgb24` gains an opaque alpha byte).
    /// Infallible on any image this crate's decoder or constructors
    /// produced.
    pub fn to_rgba8(&self) -> Vec<u8> {
        let (w, h) = (self.width as usize, self.height as usize);
        let stride = self.stride();
        let data = self.data();
        let mut out = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            let row = &data[y * stride..];
            match self.format {
                PixelFormat::Rgba => out.extend_from_slice(&row[..w * 4]),
                PixelFormat::Rgb24 => {
                    for px in row[..w * 3].chunks_exact(3) {
                        out.extend_from_slice(&[px[0], px[1], px[2], 255]);
                    }
                }
            }
        }
        out
    }

    /// Tightly packed 8-bit RGB, alpha dropped. Exact for every native
    /// layout.
    pub fn to_rgb8(&self) -> Vec<u8> {
        let (w, h) = (self.width as usize, self.height as usize);
        let stride = self.stride();
        let data = self.data();
        let mut out = Vec::with_capacity(w * h * 3);
        for y in 0..h {
            let row = &data[y * stride..];
            match self.format {
                PixelFormat::Rgba => {
                    for px in row[..w * 4].chunks_exact(4) {
                        out.extend_from_slice(&px[..3]);
                    }
                }
                PixelFormat::Rgb24 => out.extend_from_slice(&row[..w * 3]),
            }
        }
        out
    }

    /// [`PictImage::to_rgba8`] after [`PictImage::validate`], for
    /// caller-assembled images built around the constructors.
    pub fn try_to_rgba8(&self) -> Result<Vec<u8>> {
        self.validate()?;
        Ok(self.to_rgba8())
    }

    /// [`PictImage::to_rgb8`] after [`PictImage::validate`].
    pub fn try_to_rgb8(&self) -> Result<Vec<u8>> {
        self.validate()?;
        Ok(self.to_rgb8())
    }

    /// The pixels as a tightly packed `Rgba` plane (a copy of the plane
    /// for an `Rgba` image, the expanded bytes for `Rgb24`).
    pub(crate) fn rgba_bytes(&self) -> std::borrow::Cow<'_, [u8]> {
        if self.format == PixelFormat::Rgba && self.stride() == self.width as usize * 4 {
            let need = self.width as usize * 4 * self.height as usize;
            std::borrow::Cow::Borrowed(&self.data()[..need])
        } else {
            std::borrow::Cow::Owned(self.to_rgba8())
        }
    }
}

/// Tightly packed 8-bit RGB (3 bytes per pixel, row-major), the
/// [`crate::decode_rgb8`] result. Same definition in every image
/// crate.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct RgbImage {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `3 × width × height` bytes.
    pub data: Vec<u8>,
}

impl RgbImage {
    /// Wrap a packed RGB buffer.
    pub fn new(width: u32, height: u32, data: Vec<u8>) -> Self {
        Self {
            width,
            height,
            data,
        }
    }

    /// The pixel bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.data
    }

    /// Consume into the pixel bytes.
    pub fn into_raw(self) -> Vec<u8> {
        self.data
    }

    /// Bytes per row (`3 × width`).
    pub fn stride(&self) -> usize {
        self.width as usize * 3
    }
}

/// Tightly packed 8-bit RGBA (4 bytes per pixel, row-major), the
/// [`crate::decode_rgba8`] result. Same definition in every image
/// crate.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct RgbaImage {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `4 × width × height` bytes.
    pub data: Vec<u8>,
}

impl RgbaImage {
    /// Wrap a packed RGBA buffer.
    pub fn new(width: u32, height: u32, data: Vec<u8>) -> Self {
        Self {
            width,
            height,
            data,
        }
    }

    /// The pixel bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.data
    }

    /// Consume into the pixel bytes.
    pub fn into_raw(self) -> Vec<u8> {
        self.data
    }

    /// Bytes per row (`4 × width`).
    pub fn stride(&self) -> usize {
        self.width as usize * 4
    }
}

/// What [`crate::info`] reads from the picture record header (the
/// `picSize` word and the `picFrame` rectangle) and the version stanza,
/// without walking a drawing opcode or allocating a canvas.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct ImageInfo {
    /// Width in pixels (`picFrame.right − picFrame.left`, clamped at
    /// 0). A zero width or height is a degenerate frame that
    /// [`crate::decode`] rejects.
    pub width: u32,
    /// Height in pixels (`picFrame.bottom − picFrame.top`, clamped at
    /// 0).
    pub height: u32,
    /// The layout [`crate::decode`] would return — always
    /// [`PictPixelFormat::Rgba`].
    pub format: PixelFormat,
    /// Number of images; always `1` (a PICT is one picture).
    pub frames: u32,
    /// QuickDraw carries no alpha; always `false`.
    pub has_alpha: bool,
    /// Colour signalling; always [`ColorInfo::pict_default`].
    pub color: ColorInfo,
    /// PICT has no ICC carrier; always `false`.
    pub has_icc: bool,
    /// PICT has no Exif carrier; always `false`.
    pub has_exif: bool,
    /// PICT has no XMP carrier; always `false`.
    pub has_xmp: bool,
    /// Opcode framing version.
    pub version: PictVersion,
    /// `true` when the picture record starts at byte 512, after the
    /// Apple pre-OS-X launch-stub prefix.
    pub has_launch_stub: bool,
    /// `picFrame` as `(top, left, bottom, right)` in picture
    /// coordinates.
    pub frame: (i16, i16, i16, i16),
    /// The v2 `HeaderOp` payload, when present and well-formed (see
    /// [`PictImage::header`]).
    pub header: Option<PictHeader>,
}

impl ImageInfo {
    /// A picture with the given geometry; every other field at its
    /// "absent" value (v2, no stub, frame at the origin, no header).
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            format: PixelFormat::Rgba,
            frames: 1,
            has_alpha: false,
            color: ColorInfo::pict_default(),
            has_icc: false,
            has_exif: false,
            has_xmp: false,
            version: PictVersion::V2,
            has_launch_stub: false,
            frame: (0, 0, height as i16, width as i16),
            header: None,
        }
    }

    /// Resolution in dots per inch `(horizontal, vertical)` declared by
    /// an extended-v2 header, if any.
    pub fn resolution_dpi(&self) -> Option<(f32, f32)> {
        match self.header {
            Some(PictHeader::ExtendedV2 { hres, vres, .. }) => Some((hres.to_f32(), vres.to_f32())),
            _ => None,
        }
    }
}
