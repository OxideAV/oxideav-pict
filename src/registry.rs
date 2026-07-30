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
///
/// Also wired into [`oxideav_meta::register_all`] via the
/// [`oxideav_core::register!`] macro below.
pub fn register(ctx: &mut RuntimeContext) {
    register_codecs(&mut ctx.codecs);
    register_containers(&mut ctx.containers);
}

oxideav_core::register!("pict", register);

/// Resolve the codec named by a QuickTime picture opcode's
/// [`ImageDescription`](crate::quicktime::ImageDescription) through
/// the framework's [`CodecResolver`].
///
/// The `$8200` `CompressedQuickTime` opcode is a CODEC-tag boundary:
/// the compressor FourCC in `cType` (Inside Macintosh: QuickTime,
/// page 3-50) names the decompressor, exactly like a container's
/// sample-entry tag — so `oxideav-pict` never decodes the embedded
/// image itself. This helper hands the FourCC (plus the
/// description's width / height hints) to the resolver; `Some(id)`
/// means the registry carries a matching codec and the caller can
/// construct its decoder (see [`quicktime_codec_parameters`]);
/// `None` means the payload has no workspace implementation and
/// stays available as typed bytes on
/// [`QuickTimeCompressed::image_data`](crate::quicktime::QuickTimeCompressed::image_data).
pub fn resolve_quicktime_codec(
    desc: &crate::quicktime::ImageDescription,
    resolver: &dyn oxideav_core::CodecResolver,
) -> Option<CodecId> {
    let tag = oxideav_core::CodecTag::fourcc(&desc.codec);
    let ctx = oxideav_core::ProbeContext::new(&tag)
        .width(desc.width as u32)
        .height(desc.height as u32);
    resolver.resolve_tag(&ctx)
}

/// Build the [`CodecParameters`](oxideav_core::CodecParameters) for a
/// resolved QuickTime payload codec, ready to hand to
/// `CodecRegistry::first_decoder` (or `decoder_by_impl`) together
/// with the payload bytes as a packet.
///
/// Populates the video dimensions from the image description and
/// preserves the on-wire FourCC via `with_tag` so a consumer
/// re-muxing the stream round-trips the original tag. The
/// description's extension bytes (`idSize > 86` tail) are *not*
/// copied into `extradata` — their layout is per-extension, and the
/// caller holding the [`ImageDescription`](crate::quicktime::ImageDescription)
/// keeps them on
/// [`extension`](crate::quicktime::ImageDescription::extension).
///
/// Returns `None` when the resolver knows no codec for the FourCC.
pub fn quicktime_codec_parameters(
    desc: &crate::quicktime::ImageDescription,
    resolver: &dyn oxideav_core::CodecResolver,
) -> Option<oxideav_core::CodecParameters> {
    let id = resolve_quicktime_codec(desc, resolver)?;
    let mut params = oxideav_core::CodecParameters::video(id)
        .with_tag(oxideav_core::CodecTag::fourcc(&desc.codec));
    params.width = Some(desc.width as u32);
    params.height = Some(desc.height as u32);
    Some(params)
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

    fn qt_desc(codec: [u8; 4]) -> crate::quicktime::ImageDescription {
        crate::quicktime::ImageDescription {
            id_size: 86,
            codec,
            version: 1,
            revision_level: 1,
            vendor: *b"appl",
            temporal_quality: 0,
            spatial_quality: 0x0200,
            width: 64,
            height: 48,
            h_res: crate::header::Fixed::SEVENTY_TWO_DPI,
            v_res: crate::header::Fixed::SEVENTY_TWO_DPI,
            data_size: 0,
            frame_count: 1,
            name_raw: [0u8; 32],
            depth: 24,
            clut_id: -1,
            extension: Vec::new(),
        }
    }

    #[test]
    fn resolve_quicktime_codec_routes_fourcc_through_registry() {
        use oxideav_core::CodecTag;
        let mut reg = CodecRegistry::new();
        reg.register(CodecInfo::new(CodecId::new("jpeg")).tag(CodecTag::fourcc(b"jpeg")));
        let desc = qt_desc(*b"jpeg");
        assert_eq!(
            resolve_quicktime_codec(&desc, &reg),
            Some(CodecId::new("jpeg"))
        );
        // FourCC matching is case-insensitive through CodecTag::fourcc.
        let desc_upper = qt_desc(*b"JPEG");
        assert_eq!(
            resolve_quicktime_codec(&desc_upper, &reg),
            Some(CodecId::new("jpeg"))
        );
    }

    #[test]
    fn resolve_quicktime_codec_without_workspace_impl_is_none() {
        // A codec with no workspace implementation stays unresolved —
        // decoding it is out of scope; the payload remains typed
        // bytes on QuickTimeCompressed::image_data.
        let reg = CodecRegistry::new();
        assert_eq!(resolve_quicktime_codec(&qt_desc(*b"rpza"), &reg), None);
        assert_eq!(
            resolve_quicktime_codec(&qt_desc(*b"jpeg"), &oxideav_core::NullCodecResolver),
            None
        );
    }

    #[test]
    fn quicktime_codec_parameters_carry_dims_and_wire_tag() {
        use oxideav_core::CodecTag;
        let mut reg = CodecRegistry::new();
        reg.register(CodecInfo::new(CodecId::new("jpeg")).tag(CodecTag::fourcc(b"jpeg")));
        let desc = qt_desc(*b"jpeg");
        let params = quicktime_codec_parameters(&desc, &reg).expect("resolved");
        assert_eq!(params.codec_id, CodecId::new("jpeg"));
        assert_eq!(params.width, Some(64));
        assert_eq!(params.height, Some(48));
        assert_eq!(params.tag, Some(CodecTag::fourcc(b"jpeg")));
        assert!(params.extradata.is_empty());
        assert!(quicktime_codec_parameters(&qt_desc(*b"rpza"), &reg).is_none());
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
