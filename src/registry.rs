//! `oxideav-core` integration layer for `oxideav-pict`.
//!
//! Gated behind the default-on `registry` feature so image-library
//! consumers can depend on `oxideav-pict` with `default-features = false`
//! and skip the `oxideav-core` dependency entirely.
//!
//! The module exposes:
//! * [`register`] — the unified `RuntimeContext` entry point the
//!   umbrella `oxideav` crate calls during framework initialisation.
//!   Internally calls [`register_codecs`] and [`register_containers`].
//! * [`register_codecs`] — registers the PICT codec (decoder) into a
//!   [`CodecRegistry`].
//! * [`register_containers`] — registers the canonical PICT file
//!   extensions (`.pict`, `.pic`, `.pct`) against the `"pict"` codec
//!   id. PICT has no demuxer of its own (the file IS the picture
//!   body, with an optional 512-byte launch-stub prefix that the
//!   decoder sniffs); only the extension table is populated.
//! * The `From<PictError> for oxideav_core::Error` conversion that
//!   lets the trait-side `Decoder` impl in `decoder.rs` bubble
//!   bitstream errors up through the framework error type.

use oxideav_core::{CodecCapabilities, CodecId, PixelFormat, RuntimeContext};
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

/// Register PICT file extensions into the supplied [`ContainerRegistry`].
///
/// PICT has no container layer of its own — the file IS the picture
/// body (optionally prefixed by a 512-byte launch stub that the
/// decoder sniffs), so no demuxer / probe is registered. We *do*
/// register the canonical PICT file extensions (`.pict`, `.pic`,
/// `.pct`) against the codec id `"pict"` so a caller resolving a path
/// hint via [`ContainerRegistry::container_for_extension`] still gets
/// a useful answer.
pub fn register_containers(reg: &mut ContainerRegistry) {
    for ext in ["pict", "pic", "pct"] {
        reg.register_extension(ext, crate::CODEC_ID_STR);
    }
}

/// Unified entry point: install every codec and container provided by
/// `oxideav-pict` into a [`RuntimeContext`].
pub fn register(ctx: &mut RuntimeContext) {
    register_codecs(&mut ctx.codecs);
    register_containers(&mut ctx.containers);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_containers_maps_pict_extension_to_pict() {
        let mut reg = ContainerRegistry::new();
        register_containers(&mut reg);
        assert_eq!(reg.container_for_extension("pict"), Some("pict"));
    }

    #[test]
    fn register_containers_maps_pic_extension_to_pict() {
        let mut reg = ContainerRegistry::new();
        register_containers(&mut reg);
        assert_eq!(reg.container_for_extension("pic"), Some("pict"));
    }

    #[test]
    fn register_containers_maps_pct_extension_to_pict() {
        let mut reg = ContainerRegistry::new();
        register_containers(&mut reg);
        assert_eq!(reg.container_for_extension("pct"), Some("pict"));
    }

    #[test]
    fn extension_lookup_is_case_insensitive() {
        let mut reg = ContainerRegistry::new();
        register_containers(&mut reg);
        for ext in [
            "PICT", "Pict", "pIcT", "PIC", "Pic", "pIc", "PCT", "Pct", "pCt",
        ] {
            assert_eq!(
                reg.container_for_extension(ext),
                Some("pict"),
                "extension {ext:?} should map to \"pict\""
            );
        }
    }

    #[test]
    fn unknown_extension_returns_none() {
        let mut reg = ContainerRegistry::new();
        register_containers(&mut reg);
        assert_eq!(reg.container_for_extension("png"), None);
        assert_eq!(reg.container_for_extension(""), None);
    }

    #[test]
    fn register_via_runtime_context_installs_factories() {
        let mut ctx = RuntimeContext::new();
        register(&mut ctx);
        assert!(
            ctx.codecs.decoder_ids().next().is_some(),
            "register(ctx) should install codec decoder factories"
        );
        assert_eq!(
            ctx.containers.container_for_extension("pict"),
            Some(crate::CODEC_ID_STR),
            "register(ctx) should install .pict extension hint"
        );
    }
}
