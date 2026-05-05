//! Crate-local error type used by `oxideav-pict`'s standalone (no
//! `oxideav-core`) public API.
//!
//! When the `registry` feature is enabled, [`PictError`] gains a
//! `From<PictError> for oxideav_core::Error` impl (defined in
//! [`crate::registry`]) so the trait-side surface (`Decoder`) can keep
//! returning `oxideav_core::Result<T>` while the underlying decode
//! function stays framework-free.

use core::fmt;

/// `Result` alias scoped to `oxideav-pict`. Standalone (no
/// `oxideav-core`) callers see this; framework callers convert via the
/// gated `From<PictError> for oxideav_core::Error` impl.
pub type Result<T> = core::result::Result<T, PictError>;

/// Error variants returned by `oxideav-pict`'s standalone API.
///
/// The variants mirror the subset of `oxideav_core::Error` the codec
/// can hit. The crate intentionally avoids surfacing transport (`Io`)
/// or framework-specific (`FormatNotFound`, `CodecNotFound`) errors —
/// those originate in callers that are already linking `oxideav-core`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PictError {
    /// The byte stream is malformed (truncated header, opcode runs
    /// past end of stream, PackBits packet runs past end of row, …).
    InvalidData(String),
    /// The byte stream uses a feature this codec doesn't implement
    /// (CompressedQuickTime, region-clipped DirectBitsRgn, packType
    /// 2/3/4, v1 raster opcodes, …).
    Unsupported(String),
    /// The opcode stream terminated (`OpEndPic` reached) before any
    /// `PackBitsRect` / `DirectBitsRect` was seen, so no raster could
    /// be extracted. v2 PICTs that contain only drawing commands hit
    /// this until round 2 adds drawing-command rasterisation.
    NoRaster,
}

impl PictError {
    /// Construct a [`PictError::InvalidData`] from a stringy message.
    pub fn invalid(msg: impl Into<String>) -> Self {
        Self::InvalidData(msg.into())
    }

    /// Construct a [`PictError::Unsupported`] from a stringy message.
    pub fn unsupported(msg: impl Into<String>) -> Self {
        Self::Unsupported(msg.into())
    }
}

impl fmt::Display for PictError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidData(s) => write!(f, "invalid data: {s}"),
            Self::Unsupported(s) => write!(f, "unsupported: {s}"),
            Self::NoRaster => write!(
                f,
                "no raster opcode (PackBitsRect / DirectBitsRect) in PICT stream"
            ),
        }
    }
}

impl std::error::Error for PictError {}
