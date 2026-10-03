//! Crate-local error type used by `oxideav-pict`'s standalone (no
//! `oxideav-core`) public API.
//!
//! When the `registry` feature is enabled, [`PictError`] gains a
//! `From<PictError> for oxideav_core::Error` impl (defined in
//! `crate::registry`) so the trait-side surface (`Decoder` /
//! `Encoder`) can keep returning `oxideav_core::Result<T>` while the
//! underlying decode / encode functions stay framework-free.

use core::fmt;

/// `Result` alias scoped to `oxideav-pict`. Standalone (no
/// `oxideav-core`) callers see this; framework callers convert via the
/// gated `From<PictError> for oxideav_core::Error` impl.
pub type Result<T> = core::result::Result<T, PictError>;

/// The contract name for [`PictError`].
pub type Error = PictError;

/// Error variants returned by `oxideav-pict`'s standalone API.
///
/// The variants mirror the subset of `oxideav_core::Error` the codec
/// can hit plus the two the image-crate contract requires
/// (`LimitExceeded`, `Io`). Framework-specific errors
/// (`FormatNotFound`, `CodecNotFound`) originate in callers that are
/// already linking `oxideav-core`.
#[derive(Debug)]
#[non_exhaustive]
pub enum PictError {
    /// The byte stream is malformed (truncated header, opcode runs
    /// past end of stream, PackBits packet runs past end of row, …),
    /// or a caller-assembled image is inconsistent (plane too short,
    /// zero dimensions, …).
    InvalidData(String),
    /// The byte stream uses a feature this codec doesn't implement
    /// (an undocumented-size "Not determined" opcode, an unknown
    /// opcode word, …), or the encoder was asked for something the
    /// format cannot represent.
    Unsupported(String),
    /// A [`crate::DecodeOptions`] limit (dimensions / pixels / bytes)
    /// would be exceeded; nothing was allocated.
    LimitExceeded(String),
    /// A read / write on a caller-supplied stream failed
    /// ([`crate::decode_from`] / [`crate::encode_to`]).
    Io(std::io::Error),
    /// The opcode stream terminated (`OpEndPic` reached or bytes ran
    /// out) without producing any drawing, raster, or captured
    /// QuickTime-payload content — the picture is empty. A PICT whose
    /// only content is a `$8200` / `$8201` QuickTime opcode does
    /// *not* hit this (round 435): the captured payload counts as
    /// content and decode succeeds with the background canvas.
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

    /// Construct a [`PictError::LimitExceeded`] from a stringy message.
    pub fn limit(msg: impl Into<String>) -> Self {
        Self::LimitExceeded(msg.into())
    }
}

impl From<std::io::Error> for PictError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl fmt::Display for PictError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidData(s) => write!(f, "invalid data: {s}"),
            Self::Unsupported(s) => write!(f, "unsupported: {s}"),
            Self::LimitExceeded(s) => write!(f, "limit exceeded: {s}"),
            Self::Io(e) => write!(f, "io: {e}"),
            Self::NoRaster => write!(
                f,
                "no raster opcode (PackBitsRect / DirectBitsRect) in PICT stream"
            ),
        }
    }
}

impl std::error::Error for PictError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}
