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
