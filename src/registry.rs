//! `oxideav-core` integration layer for `oxideav-pict`.
//!
//! Gated behind the default-on `registry` feature so image-library
//! consumers can depend on `oxideav-pict` with `default-features = false`
//! and skip the `oxideav-core` dependency entirely.
//!
//! The module exposes:
//! * [`register`] / [`register_codecs`] — the `CodecRegistry` entry
//!   point the umbrella `oxideav` crate calls during framework
//!   initialisation. PICT has no separate container format (the file
//!   IS the PICT body, with an optional 512-byte launch-stub prefix
//!   that we sniff for); container registration is therefore a no-op.
//! * The `From<PictError> for oxideav_core::Error` conversion that
//!   lets the trait-side `Decoder` impl in `decoder.rs` bubble
//!   bitstream errors up through the framework error type.

use oxideav_core::{CodecCapabilities, CodecId, PixelFormat};
use oxideav_core::{CodecInfo, CodecRegistry, ContainerRegistry};

use crate::error::PictError;

/// Convert a [`PictError`] into the framework-shared
/// `oxideav_core::Error` so trait impls in this crate can use `?` on
/// errors returned by the framework-free decode function.
impl From<PictError> for oxideav_core::Error {
    fn from(e: PictError) -> Self {
        match e {
            PictError::InvalidData(s) => oxideav_core::Error::InvalidData(s),
            PictError::Unsupported(s) => oxideav_core::Error::Unsupported(s),
            PictError::NoRaster => oxideav_core::Error::InvalidData(
                "no raster opcode (PackBitsRect / DirectBitsRect) in PICT stream".into(),
            ),
        }
    }
}

/// Register the PICT codec into the supplied [`CodecRegistry`].
pub fn register_codecs(reg: &mut CodecRegistry) {
    let caps = CodecCapabilities::video("pict_sw")
        .with_intra_only(true)
        .with_lossless(true)
        .with_max_size(32767, 32767)
        .with_pixel_formats(vec![PixelFormat::Rgba]);
    reg.register(
        CodecInfo::new(CodecId::new(crate::CODEC_ID_STR))
            .capabilities(caps)
            .decoder(crate::decoder::make_decoder),
    );
}

/// Register PICT containers into the supplied [`ContainerRegistry`].
///
/// PICT has no container layer of its own — the file IS the picture
/// body (optionally prefixed by a 512-byte launch stub that the
/// decoder sniffs). This function is therefore a no-op; it exists
/// only so callers can use the same `register_containers(...)` API
/// shape as other codec crates.
pub fn register_containers(_reg: &mut ContainerRegistry) {}

/// Combined registration for callers that just want everything wired up
/// in one call.
pub fn register(codecs: &mut CodecRegistry, containers: &mut ContainerRegistry) {
    register_codecs(codecs);
    register_containers(containers);
}
