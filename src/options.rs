//! Decode-side limits ([`DecodeOptions`]) and encode-side behaviour
//! ([`EncodeOptions`]) for the `IMAGE_CRATE_API` root functions.

use crate::encoder::PackType;
use crate::error::{PictError, Result};
use crate::header::Fixed;
use crate::image::PictVersion;

/// Limits and strictness for [`crate::decode_with`].
///
/// Every limit is checked against the picture record header
/// (`picFrame`) **before** the canvas is allocated, so a hostile
/// 12-byte header fails with [`PictError::LimitExceeded`] instead of
/// committing memory. The defaults are: no dimension / pixel-count
/// limit (`picFrame` coordinates are `i16`, so the geometry is bounded
/// by the format itself at 65 535 × 65 535), the canvas capped at
/// [`DecodeOptions::DEFAULT_MAX_BYTES`] (256 MiB — the crate's
/// long-standing [`crate::MAX_RASTER_BYTES`] budget), `strict = false`.
///
/// `max_bytes` bounds the decoded canvas (`width × height × 4`). The
/// intermediate buffers a raster opcode allocates from its own PixMap
/// fields (`bounds × rowBytes`, the unpacked RGBA of one sub-image)
/// stay bounded by [`crate::MAX_RASTER_BYTES`] whatever `max_bytes`
/// says — that budget is per opcode, not per picture.
///
/// `strict` governs what the lenient walker tolerates from real-world
/// emitters:
///
/// * always (both modes): a picture record at offset 0 or 512, a
///   `$0011` / `$1101` version stanza, a `$0C00` `HeaderOp` after the
///   v2 sentinel, a non-degenerate `picFrame`, every opcode body
///   within the stream, every PackBits packet within its row;
/// * `strict = false` (default): a `HeaderOp` payload whose version
///   word is neither `$FFFE` nor `$FFFF` is skipped (24 bytes) and the
///   header reported as `None`; a stream that ends without `OpEndPic`
///   is accepted with whatever was drawn (real-world generators
///   truncate after the last raster); bytes after `OpEndPic` are
///   ignored;
/// * `strict = true`: each of those is [`PictError::InvalidData`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct DecodeOptions {
    /// Reject pictures wider than this (pixels).
    pub max_width: Option<u32>,
    /// Reject pictures taller than this (pixels).
    pub max_height: Option<u32>,
    /// Reject pictures with more than this many pixels (`width ×
    /// height`).
    pub max_pixels: Option<u64>,
    /// Reject pictures whose RGBA canvas would exceed this many bytes.
    pub max_bytes: Option<u64>,
    /// Enforce the §A-3 framing rules the lenient walker relaxes (see
    /// the type docs).
    pub strict: bool,
}

impl DecodeOptions {
    /// Default [`Self::max_bytes`]: 256 MiB of canvas
    /// ([`crate::MAX_RASTER_BYTES`]).
    pub const DEFAULT_MAX_BYTES: u64 = crate::decoder::MAX_RASTER_BYTES as u64;

    /// The defaults (see the type docs).
    pub fn new() -> Self {
        Self::default()
    }

    /// Set (or lift with `None`) the width limit.
    pub fn with_max_width(mut self, max_width: impl Into<Option<u32>>) -> Self {
        self.max_width = max_width.into();
        self
    }

    /// Set (or lift with `None`) the height limit.
    pub fn with_max_height(mut self, max_height: impl Into<Option<u32>>) -> Self {
        self.max_height = max_height.into();
        self
    }

    /// Set (or lift with `None`) the pixel-count limit.
    pub fn with_max_pixels(mut self, max_pixels: impl Into<Option<u64>>) -> Self {
        self.max_pixels = max_pixels.into();
        self
    }

    /// Set (or lift with `None`) the canvas-bytes limit.
    pub fn with_max_bytes(mut self, max_bytes: impl Into<Option<u64>>) -> Self {
        self.max_bytes = max_bytes.into();
        self
    }

    /// Set strict mode.
    pub fn with_strict(mut self, strict: bool) -> Self {
        self.strict = strict;
        self
    }

    /// Lift every limit (`max_*` all `None`).
    pub fn unlimited(mut self) -> Self {
        self.max_width = None;
        self.max_height = None;
        self.max_pixels = None;
        self.max_bytes = None;
        self
    }

    /// Check a picture frame against the limits. `bytes` is the canvas
    /// the decode would allocate.
    pub(crate) fn check(&self, width: u32, height: u32, bytes: u64) -> Result<()> {
        if let Some(m) = self.max_width {
            if width > m {
                return Err(PictError::limit(format!(
                    "PICT: width {width} exceeds max_width {m}"
                )));
            }
        }
        if let Some(m) = self.max_height {
            if height > m {
                return Err(PictError::limit(format!(
                    "PICT: height {height} exceeds max_height {m}"
                )));
            }
        }
        let pixels = u64::from(width) * u64::from(height);
        if let Some(m) = self.max_pixels {
            if pixels > m {
                return Err(PictError::limit(format!(
                    "PICT: {pixels} pixels exceed max_pixels {m}"
                )));
            }
        }
        if let Some(m) = self.max_bytes {
            if bytes > m {
                return Err(PictError::limit(format!(
                    "PICT: canvas of {bytes} bytes exceeds max_bytes {m}"
                )));
            }
        }
        Ok(())
    }
}

impl Default for DecodeOptions {
    fn default() -> Self {
        Self {
            max_width: None,
            max_height: None,
            max_pixels: None,
            max_bytes: Some(Self::DEFAULT_MAX_BYTES),
            strict: false,
        }
    }
}

/// Behaviour of [`crate::encode`] / [`crate::encode_rgb8`] /
/// [`crate::encode_rgba8`] / [`crate::encode_to`].
///
/// One struct, every variant a field: the opcode framing (`version`),
/// the `DirectBitsRect` pixel packing (`pack`), the 512-byte launch
/// stub (`launch_stub`), the picture frame origin (`frame_origin`), the
/// declared resolution (`resolution`), the v2 header kind
/// (`extended_header`), an optional rectangular `ClipRgn` (`clip`) and
/// whether the image's Picture Comments are written back (`comments`).
///
/// The raster is always one RGBDirect PixMap in a `DirectBitsRect`
/// (`$009A` v2 / `$9A` v1) opcode: that is how both `Rgb24` and `Rgba`
/// are carried (QuickDraw has no alpha; `packType` 1 writes the pad
/// byte `$FF`, `packType` 3 sets the A1 bit, 2 and 4 write colour
/// only). An image decoded by this crate carries its header kind and
/// resolution on [`PictImage::header`](crate::PictImage::header) and
/// its comments on [`comments`](crate::PictImage::comments); with the
/// defaults they are written back as read, so
/// `decode(encode(img)) == img` holds for `Rgba` images with
/// [`PackType::Raw`], [`PackType::Packed24`] or
/// [`PackType::ComponentPackBits`] ([`PackType::Rle16`] is 5 bits per
/// channel and therefore lossy). The monochrome `BitsRect` /
/// `PackBitsRect` and indexed-PixMap writers (a lossy reduction of
/// RGBA) and the drawing builders stay under their own names
/// ([`crate::encode_pict_bits_rect`], [`crate::encode_pict_indexed_bits_rect`],
/// [`crate::PictBuilder`], …).
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct EncodeOptions {
    /// Opcode framing. Default [`PictVersion::V2`]. Version 1 writes
    /// the de-facto `$9A` `DirectBitsRect` inside the 8-bit-opcode
    /// framing (see [`crate::encode_pict_v1_with`] for the conformance
    /// note); the Table-A-3-only monochrome v1 rasters are
    /// [`crate::encode_pict_v1_bits_rect`] /
    /// [`crate::encode_pict_v1_pack_bits_rect`].
    pub version: PictVersion,
    /// `DirectBitsRect` pixel packing. Default [`PackType::Raw`]
    /// (`packType` 1, 32-bit XRGB, lossless, largest).
    pub pack: PackType,
    /// Prepend the 512-byte Apple launch-stub prefix. `None` (default)
    /// follows the version's convention: `true` for v2 files, `false`
    /// for v1 (which pre-dates the stub).
    pub launch_stub: Option<bool>,
    /// `picFrame` top-left `(top, left)` in picture coordinates;
    /// `bottom` / `right` follow from the image size. Default `(0, 0)`.
    /// `top + height` and `left + width` must fit in `i16`
    /// ([`PictError::InvalidData`] otherwise).
    pub frame_origin: (i16, i16),
    /// Declared `(hRes, vRes)`, written to the extended-v2 `HeaderOp`
    /// and to the PixMap. `None` (default) uses the image header's
    /// resolution when it is `ExtendedV2`, else 72.0 dpi (the
    /// QuickDraw default and the Listing A-5 value).
    pub resolution: Option<(Fixed, Fixed)>,
    /// v2 `HeaderOp` kind: `Some(true)` writes the extended-v2 header
    /// (`version = -2`, Listing A-5), `Some(false)` the plain v2 header
    /// (`version = -1`, Listing A-6). `None` (default) follows the image
    /// header's kind, extended when the image has none. Ignored for v1.
    pub extended_header: Option<bool>,
    /// Rectangular `ClipRgn` `[top, left, bottom, right]` (picture
    /// coordinates) emitted before the raster. Default `None`.
    pub clip: Option<[i16; 4]>,
    /// Write the image's Picture Comments (`ShortComment` /
    /// `LongComment`) after the header, in order. Default `true`.
    pub comments: bool,
}

impl Default for EncodeOptions {
    fn default() -> Self {
        Self {
            version: PictVersion::V2,
            pack: PackType::Raw,
            launch_stub: None,
            frame_origin: (0, 0),
            resolution: None,
            extended_header: None,
            clip: None,
            comments: true,
        }
    }
}

impl EncodeOptions {
    /// The defaults: v2, `packType` 1, stub per version, frame at the
    /// origin, resolution and header kind from the image, no clip,
    /// comments written back.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the opcode framing version.
    pub fn with_version(mut self, version: PictVersion) -> Self {
        self.version = version;
        self
    }

    /// Set the `DirectBitsRect` pixel packing.
    pub fn with_pack(mut self, pack: PackType) -> Self {
        self.pack = pack;
        self
    }

    /// Force (or stop forcing, with `None`) the 512-byte launch stub.
    pub fn with_launch_stub(mut self, launch_stub: impl Into<Option<bool>>) -> Self {
        self.launch_stub = launch_stub.into();
        self
    }

    /// Set the `picFrame` origin `(top, left)`.
    pub fn with_frame_origin(mut self, top: i16, left: i16) -> Self {
        self.frame_origin = (top, left);
        self
    }

    /// Set (or clear) the declared resolution.
    pub fn with_resolution(mut self, resolution: impl Into<Option<(Fixed, Fixed)>>) -> Self {
        self.resolution = resolution.into();
        self
    }

    /// Set the declared resolution from integer dots per inch.
    pub fn with_resolution_dpi(mut self, h_dpi: i16, v_dpi: i16) -> Self {
        self.resolution = Some((Fixed::from_integer(h_dpi), Fixed::from_integer(v_dpi)));
        self
    }

    /// Choose the v2 header kind explicitly (extended `version = -2`
    /// when `Some(true)`, plain `version = -1` when `Some(false)`) or
    /// follow the image's header again (`None`).
    pub fn with_extended_header(mut self, extended: impl Into<Option<bool>>) -> Self {
        self.extended_header = extended.into();
        self
    }

    /// Set (or clear) the rectangular clip.
    pub fn with_clip(mut self, clip: impl Into<Option<[i16; 4]>>) -> Self {
        self.clip = clip.into();
        self
    }

    /// Write (or omit) the image's Picture Comments.
    pub fn with_comments(mut self, comments: bool) -> Self {
        self.comments = comments;
        self
    }

    /// Whether this configuration writes the launch stub.
    pub(crate) fn writes_stub(&self) -> bool {
        self.launch_stub
            .unwrap_or(matches!(self.version, PictVersion::V2))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_fire_in_order() {
        let o = DecodeOptions::default()
            .with_max_width(10u32)
            .with_max_height(10u32)
            .with_max_pixels(50u64)
            .with_max_bytes(100u64);
        assert!(o.check(5, 5, 75).is_ok());
        assert!(matches!(
            o.check(11, 1, 1),
            Err(PictError::LimitExceeded(_))
        ));
        assert!(matches!(
            o.check(1, 11, 1),
            Err(PictError::LimitExceeded(_))
        ));
        assert!(matches!(o.check(8, 8, 1), Err(PictError::LimitExceeded(_))));
        assert!(matches!(
            o.check(5, 5, 101),
            Err(PictError::LimitExceeded(_))
        ));
        assert!(o.unlimited().check(u32::MAX, u32::MAX, u64::MAX).is_ok());
    }

    #[test]
    fn decode_defaults() {
        let o = DecodeOptions::default();
        assert_eq!(o.max_width, None);
        assert_eq!(o.max_height, None);
        assert_eq!(o.max_pixels, None);
        assert_eq!(o.max_bytes, Some(crate::MAX_RASTER_BYTES as u64));
        assert!(!o.strict);
    }

    #[test]
    fn encode_defaults() {
        let o = EncodeOptions::default();
        assert_eq!(o.version, PictVersion::V2);
        assert_eq!(o.pack, PackType::Raw);
        assert_eq!(o.launch_stub, None);
        assert!(o.writes_stub());
        assert!(!o.with_version(PictVersion::V1).writes_stub());
        assert!(EncodeOptions::default()
            .with_version(PictVersion::V1)
            .with_launch_stub(true)
            .writes_stub());
        let o = EncodeOptions::default().with_extended_header(false);
        assert_eq!(o.extended_header, Some(false));
        assert_eq!(EncodeOptions::default().extended_header, None);
    }
}
