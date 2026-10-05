//! `oxideav-core` integration layer for `oxideav-pict`.
//!
//! Gated behind the default-on `registry` feature so image-library
//! consumers can depend on `oxideav-pict` with `default-features = false`
//! and skip the `oxideav-core` dependency entirely.
//!
//! The module exposes:
//! * [`register`] (the fleet `RuntimeContext` entry point, also what
//!   the `oxideav_core::register!` macro dispatches),
//!   [`register_codecs`] / [`register_containers`] for callers holding
//!   the sub-registries. PICT has no demuxer of its own (the file IS
//!   the picture body, with an optional 512-byte launch-stub prefix
//!   that the decoder sniffs); only the extension table is populated.
//! * [`make_decoder`] / [`make_encoder`] — the codec factories; the
//!   framework `Decoder` / `Encoder` are thin adapters over
//!   [`crate::decode`] / [`crate::encode`] (one implementation).
//! * The frame bridge: `From<PictImage> for VideoFrame`,
//!   [`PictImage::from_video_frame`] and
//!   `TryFrom<(&VideoFrame, &CodecParameters)>`, plus the 1:1
//!   [`PictPixelFormat`] ↔ `oxideav_core::PixelFormat` name mapping.
//! * The QuickTime-payload codec routing
//!   ([`resolve_quicktime_codec`], [`quicktime_codec_parameters`],
//!   [`RegistryQuickTimeDecoder`]) for `$8200` pictures.
//! * The `From<PictError> for oxideav_core::Error` conversion.

use oxideav_core::{
    CodecCapabilities, CodecId, CodecInfo, CodecParameters, CodecRegistry, ColorPrimaries,
    ColorSignal, ContainerRegistry, Decoder, Encoder, Frame, MatrixCoefficients, Packet,
    PixelFormat, RuntimeContext, TimeBase, TransferCharacteristics, VideoFrame, VideoPlane,
};

use crate::error::PictError;
use crate::image::{ColorInfo, ColorRange, PictImage, PictPixelFormat, Plane};
use crate::options::EncodeOptions;

/// Convert a [`PictError`] into the framework-shared
/// `oxideav_core::Error` so trait impls in this crate can use `?` on
/// errors returned by the framework-free decode / encode functions.
impl From<PictError> for oxideav_core::Error {
    fn from(e: PictError) -> Self {
        match e {
            PictError::InvalidData(s) => oxideav_core::Error::InvalidData(s),
            PictError::Unsupported(s) => oxideav_core::Error::Unsupported(s),
            PictError::LimitExceeded(s) => oxideav_core::Error::InvalidData(s),
            PictError::Io(e) => oxideav_core::Error::Io(e),
            PictError::NoRaster => oxideav_core::Error::InvalidData(
                "no raster opcode (PackBitsRect / DirectBitsRect) in PICT stream".into(),
            ),
        }
    }
}

// ---- Pixel-format and colour mapping (1:1 by name) ----

/// The 1:1 name mapping from the framework enum to [`PictPixelFormat`].
pub fn from_core_pixel_format(pf: PixelFormat) -> oxideav_core::Result<PictPixelFormat> {
    Ok(match pf {
        PixelFormat::Rgba => PictPixelFormat::Rgba,
        PixelFormat::Rgb24 => PictPixelFormat::Rgb24,
        other => {
            return Err(oxideav_core::Error::unsupported(format!(
                "PICT: pixel format {other:?} not supported"
            )))
        }
    })
}

/// The 1:1 name mapping from [`PictPixelFormat`] to the framework enum.
pub fn to_core_pixel_format(pf: PictPixelFormat) -> PixelFormat {
    match pf {
        PictPixelFormat::Rgba => PixelFormat::Rgba,
        PictPixelFormat::Rgb24 => PixelFormat::Rgb24,
    }
}

impl From<PictPixelFormat> for PixelFormat {
    fn from(pf: PictPixelFormat) -> Self {
        to_core_pixel_format(pf)
    }
}

impl TryFrom<PixelFormat> for PictPixelFormat {
    type Error = oxideav_core::Error;
    fn try_from(pf: PixelFormat) -> oxideav_core::Result<Self> {
        from_core_pixel_format(pf)
    }
}

/// [`ColorInfo`] as the framework's [`ColorSignal`] (code points map
/// 1:1; `Unspecified` range stays unspecified).
pub fn to_color_signal(c: &ColorInfo) -> ColorSignal {
    let range = match c.range {
        ColorRange::Unspecified => oxideav_core::ColorRange::Unspecified,
        ColorRange::Limited => oxideav_core::ColorRange::Limited,
        ColorRange::Full => oxideav_core::ColorRange::Full,
    };
    ColorSignal::new(
        range,
        ColorPrimaries(c.primaries),
        TransferCharacteristics(c.transfer),
        MatrixCoefficients(c.matrix),
    )
}

/// The inverse of [`to_color_signal`].
pub fn from_color_signal(s: &ColorSignal) -> ColorInfo {
    let range = match s.range {
        oxideav_core::ColorRange::Limited => ColorRange::Limited,
        oxideav_core::ColorRange::Full => ColorRange::Full,
        _ => ColorRange::Unspecified,
    };
    ColorInfo::new(range, s.primaries.0, s.transfer.0, s.matrix.0)
}

// ---- Frame bridge ----

/// [`PictImage`] → `VideoFrame` with `pts` stamped: the single packed
/// plane. PICT signals no colour, so the documented
/// [`ColorInfo::pict_default`] is **not** stamped on the frame; only a
/// caller-supplied signal beyond that default is.
pub fn image_into_video_frame(mut image: PictImage, pts: Option<i64>) -> VideoFrame {
    let stride = image.stride();
    let data = if image.planes.is_empty() {
        Vec::new()
    } else {
        std::mem::take(&mut image.planes[0].data)
    };
    let mut frame = VideoFrame {
        pts,
        planes: vec![VideoPlane { stride, data }],
    };
    let c = image.color;
    if c.primaries != ColorInfo::UNSPECIFIED
        || c.transfer != ColorInfo::UNSPECIFIED
        || c.range == ColorRange::Limited
    {
        frame.set_color_signal(to_color_signal(&c));
    }
    frame
}

impl From<PictImage> for VideoFrame {
    /// The pixel plane (`pts` `None`); see [`image_into_video_frame`].
    fn from(image: PictImage) -> Self {
        image_into_video_frame(image, None)
    }
}

impl From<&PictImage> for VideoFrame {
    fn from(image: &PictImage) -> Self {
        image_into_video_frame(image.clone(), None)
    }
}

impl PictImage {
    /// Rebuild an image from a framework frame and the stream
    /// parameters that describe it (`width`, `height` required;
    /// `pixel_format` defaults to `Rgba`, the PICT decode layout). The
    /// frame's colour-signal side-channel, when attached, becomes
    /// `color`. The geometry is validated.
    pub fn from_video_frame(frame: &VideoFrame, params: &CodecParameters) -> crate::Result<Self> {
        let width = params
            .width
            .ok_or_else(|| PictError::invalid("PICT: missing width"))?;
        let height = params
            .height
            .ok_or_else(|| PictError::invalid("PICT: missing height"))?;
        let pix = from_core_pixel_format(params.pixel_format.unwrap_or(PixelFormat::Rgba))
            .map_err(|e| PictError::unsupported(e.to_string()))?;
        let plane = frame
            .image_planes()
            .first()
            .ok_or_else(|| PictError::invalid("PICT: frame has no planes"))?;
        let mut img = PictImage::new(
            width,
            height,
            pix,
            vec![Plane::new(plane.stride, plane.data.clone())],
        )?;
        if let Some(sig) = frame.color_signal() {
            img.color = from_color_signal(&sig);
        }
        Ok(img)
    }
}

impl TryFrom<(&VideoFrame, &CodecParameters)> for PictImage {
    type Error = PictError;
    fn try_from((frame, params): (&VideoFrame, &CodecParameters)) -> crate::Result<Self> {
        PictImage::from_video_frame(frame, params)
    }
}

// ---- Decoder trait impl + factory ----

/// Factory registered with the codec registry. Consumes one packet per
/// whole PICT file and produces one `Rgba` frame — the native layout
/// [`crate::decode`] returns — through the default QuickTime-image
/// chain (`'raw '` built in, `'jpeg'` via `oxideav-mjpeg`).
pub fn make_decoder(_params: &CodecParameters) -> oxideav_core::Result<Box<dyn Decoder>> {
    Ok(Box::new(PictDecoder {
        codec_id: CodecId::new(crate::CODEC_ID_STR),
        pending: None,
        eof: false,
    }))
}

struct PictDecoder {
    codec_id: CodecId,
    pending: Option<VideoFrame>,
    eof: bool,
}

impl Decoder for PictDecoder {
    fn codec_id(&self) -> &CodecId {
        &self.codec_id
    }
    fn send_packet(&mut self, packet: &Packet) -> oxideav_core::Result<()> {
        let image = crate::decode(&packet.data)?;
        self.pending = Some(image_into_video_frame(image, packet.pts));
        Ok(())
    }
    fn receive_frame(&mut self) -> oxideav_core::Result<Frame> {
        match self.pending.take() {
            Some(f) => Ok(Frame::Video(f)),
            None => {
                if self.eof {
                    Err(oxideav_core::Error::Eof)
                } else {
                    Err(oxideav_core::Error::NeedMore)
                }
            }
        }
    }
    fn flush(&mut self) -> oxideav_core::Result<()> {
        self.eof = true;
        Ok(())
    }
}

// ---- Encoder trait impl + factory ----

/// Factory registered with the codec registry: one frame in, one
/// complete PICT file out through [`crate::encode`] with
/// [`EncodeOptions::default`] (v2, `packType` 1, launch stub).
///
/// Accepted `pixel_format`s: `Rgba` and `Rgb24` map 1:1 onto the
/// [`PictImage`] layouts; `Bgra` / `Bgr24` / `Argb` / `Abgr` are
/// re-ordered to RGB first (QuickDraw stores no alpha, so nothing is
/// lost).
pub fn make_encoder(params: &CodecParameters) -> oxideav_core::Result<Box<dyn Encoder>> {
    let mut out_params = CodecParameters::video(CodecId::new(crate::CODEC_ID_STR));
    out_params.width = params.width;
    out_params.height = params.height;
    out_params.pixel_format = params.pixel_format;
    Ok(Box::new(PictEncoder {
        codec_id: CodecId::new(crate::CODEC_ID_STR),
        out_params,
        opts: EncodeOptions::default(),
        pending: None,
        eof: false,
    }))
}

struct PictEncoder {
    codec_id: CodecId,
    out_params: CodecParameters,
    opts: EncodeOptions,
    pending: Option<Vec<u8>>,
    eof: bool,
}

/// Bring a framework frame in a layout outside the 1:1 set (`Bgr24`,
/// `Bgra`, `Argb`, `Abgr`) into a [`PictImage`]; the 1:1 layouts go
/// through [`PictImage::from_video_frame`] unchanged.
fn frame_to_image(vf: &VideoFrame, params: &CodecParameters) -> oxideav_core::Result<PictImage> {
    let format = params.pixel_format.ok_or_else(|| {
        oxideav_core::Error::invalid("PICT encoder: pixel_format missing in CodecParameters")
    })?;
    let width = params.width.ok_or_else(|| {
        oxideav_core::Error::invalid("PICT encoder: width missing in CodecParameters")
    })?;
    let height = params.height.ok_or_else(|| {
        oxideav_core::Error::invalid("PICT encoder: height missing in CodecParameters")
    })?;
    let plane = vf
        .image_planes()
        .first()
        .ok_or_else(|| oxideav_core::Error::invalid("PICT encoder: empty frame plane"))?;
    let (w, h) = (width as usize, height as usize);
    // Byte positions of (R, G, B) inside one packed pixel.
    let (bpp, order): (usize, [usize; 3]) = match format {
        PixelFormat::Rgba | PixelFormat::Rgb24 => {
            return Ok(PictImage::from_video_frame(vf, params)?);
        }
        PixelFormat::Bgr24 => (3, [2, 1, 0]),
        PixelFormat::Bgra => (4, [2, 1, 0]),
        PixelFormat::Argb => (4, [1, 2, 3]),
        PixelFormat::Abgr => (4, [3, 2, 1]),
        other => {
            return Err(oxideav_core::Error::invalid(format!(
                "PICT encoder: unsupported pixel format {other:?}"
            )))
        }
    };
    let want = w * bpp;
    if plane.stride < want || plane.data.len() < plane.stride * h {
        return Err(oxideav_core::Error::invalid(format!(
            "PICT encoder: plane of {} bytes (stride {}) is short for {w}x{h} at {bpp} bytes/pixel",
            plane.data.len(),
            plane.stride
        )));
    }
    let mut rgb = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        let row = &plane.data[y * plane.stride..y * plane.stride + want];
        for px in row.chunks_exact(bpp) {
            rgb.extend_from_slice(&[px[order[0]], px[order[1]], px[order[2]]]);
        }
    }
    Ok(PictImage::from_rgb8(width, height, rgb)?)
}

impl Encoder for PictEncoder {
    fn codec_id(&self) -> &CodecId {
        &self.codec_id
    }
    fn output_params(&self) -> &CodecParameters {
        &self.out_params
    }
    fn send_frame(&mut self, frame: &Frame) -> oxideav_core::Result<()> {
        let vf = match frame {
            Frame::Video(v) => v,
            _ => {
                return Err(oxideav_core::Error::invalid(
                    "PICT encoder: expected video frame",
                ))
            }
        };
        let image = frame_to_image(vf, &self.out_params)?;
        self.pending = Some(crate::encode(&image, &self.opts)?);
        Ok(())
    }
    fn receive_packet(&mut self) -> oxideav_core::Result<Packet> {
        match self.pending.take() {
            Some(bytes) => {
                let mut pkt = Packet::new(0, TimeBase::new(1, 1), bytes);
                pkt.flags.keyframe = true;
                Ok(pkt)
            }
            None => {
                if self.eof {
                    Err(oxideav_core::Error::Eof)
                } else {
                    Err(oxideav_core::Error::NeedMore)
                }
            }
        }
    }
    fn flush(&mut self) -> oxideav_core::Result<()> {
        self.eof = true;
        Ok(())
    }
}

// ---- Registration ----

/// Register the PICT codec into the supplied [`CodecRegistry`].
pub fn register_codecs(reg: &mut CodecRegistry) {
    let caps = CodecCapabilities::video("pict_sw")
        .with_intra_only(true)
        .with_lossless(true)
        .with_max_size(32767, 32767)
        .with_pixel_formats(vec![PixelFormat::Rgba, PixelFormat::Rgb24]);
    reg.register(
        CodecInfo::new(CodecId::new(crate::CODEC_ID_STR))
            .capabilities(caps)
            .decoder(make_decoder)
            .encoder(make_encoder),
    );
}

/// Register the `pict` container ([`crate::container`]): the structural
/// probe (version stanza + opcode walk — PICT has no magic), the
/// demuxer (the whole file as one packet, `Rgba` stream), the muxer
/// (the encoder's packet written verbatim) and the `.pict` / `.pct` /
/// `.pic` extensions.
pub fn register_containers(reg: &mut ContainerRegistry) {
    crate::container::register(reg);
}

/// Unified entry point: install every codec and container provided by
/// `oxideav-pict` into a [`RuntimeContext`].
///
/// Also wired into `oxideav_meta::register_all` via the
/// [`oxideav_core::register!`] macro below.
pub fn register(ctx: &mut RuntimeContext) {
    register_codecs(&mut ctx.codecs);
    register_containers(&mut ctx.containers);
}

oxideav_core::register!("pict", register);

/// Resolve the codec named by a QuickTime picture opcode's
/// [`ImageDescription`](crate::quicktime::ImageDescription) through
/// the framework's [`oxideav_core::CodecResolver`].
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

/// Build the [`CodecParameters`] for a
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

/// A [`QuickTimeImageDecoder`](crate::qtimage::QuickTimeImageDecoder)
/// that resolves each `$8200` compressor FourCC through a caller's
/// [`CodecRegistry`] (via [`resolve_quicktime_codec`] /
/// [`quicktime_codec_parameters`]), runs the registry's first decoder
/// on the image data as one packet, and folds the resulting frame to
/// RGBA. FourCCs the registry does not know fall back to
/// [`DefaultQuickTimeDecoder`](crate::qtimage::DefaultQuickTimeDecoder)
/// (`'raw '` built in, `'jpeg'` through `oxideav-mjpeg`).
///
/// ```no_run
/// use oxideav_core::RuntimeContext;
/// use oxideav_pict::{decode_with_quicktime, DecodeOptions, RegistryQuickTimeDecoder};
///
/// let mut ctx = RuntimeContext::new();
/// // … register the sibling codecs you want available …
/// let mut qt = RegistryQuickTimeDecoder::new(&ctx.codecs);
/// let bytes = std::fs::read("photo.pict")?;
/// let img = decode_with_quicktime(&bytes, &DecodeOptions::default(), &mut qt)?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub struct RegistryQuickTimeDecoder<'a> {
    registry: &'a CodecRegistry,
    fallback: crate::qtimage::DefaultQuickTimeDecoder,
}

impl<'a> RegistryQuickTimeDecoder<'a> {
    /// Route FourCCs through `registry`.
    pub fn new(registry: &'a CodecRegistry) -> Self {
        Self {
            registry,
            fallback: crate::qtimage::DefaultQuickTimeDecoder::default(),
        }
    }
}

impl crate::qtimage::QuickTimeImageDecoder for RegistryQuickTimeDecoder<'_> {
    fn decode_image(
        &mut self,
        description: &crate::quicktime::ImageDescription,
        data: &[u8],
    ) -> crate::error::Result<crate::qtimage::DecodedQuickTimeImage> {
        let Some(params) = quicktime_codec_parameters(description, self.registry) else {
            return self.fallback.decode_image(description, data);
        };
        let Ok(mut dec) = self.registry.first_decoder(&params) else {
            return self.fallback.decode_image(description, data);
        };
        let wrap = |e: oxideav_core::Error| {
            PictError::invalid(format!(
                "'{}' QuickTime image via {}: {e}",
                description.codec_str(),
                params.codec_id
            ))
        };
        let packet = oxideav_core::Packet::new(0, oxideav_core::TimeBase::new(1, 1), data.to_vec());
        dec.send_packet(&packet).map_err(wrap)?;
        let frame = match dec.receive_frame() {
            Ok(f) => f,
            Err(oxideav_core::Error::NeedMore) => {
                dec.flush().map_err(wrap)?;
                dec.receive_frame().map_err(wrap)?
            }
            Err(e) => return Err(wrap(e)),
        };
        match frame {
            oxideav_core::Frame::Video(v) => crate::qtimage::video_frame_to_rgba(
                &v,
                description.width as u32,
                description.height as u32,
                crate::qtimage::PackedQuad::Rgba,
            ),
            _ => Err(PictError::invalid(format!(
                "'{}' QuickTime image: decoder produced a non-video frame",
                description.codec_str()
            ))),
        }
    }
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
    fn frame_bridge_round_trips_rgba_and_maps_formats() {
        let img = PictImage::from_rgba8(2, 1, vec![1, 2, 3, 255, 4, 5, 6, 255]).unwrap();
        let frame: VideoFrame = img.clone().into();
        assert_eq!(frame.planes.len(), 1);
        assert_eq!(frame.planes[0].stride, 8);
        assert!(
            frame.color_signal().is_none(),
            "pict_default is not stamped"
        );
        let mut params = CodecParameters::video(CodecId::new(crate::CODEC_ID_STR));
        params.width = Some(2);
        params.height = Some(1);
        params.pixel_format = Some(PixelFormat::Rgba);
        let back = PictImage::from_video_frame(&frame, &params).unwrap();
        assert_eq!(back, img);
        let back2 = PictImage::try_from((&frame, &params)).unwrap();
        assert_eq!(back2, img);
        params.pixel_format = Some(PixelFormat::Gray8);
        assert!(matches!(
            PictImage::from_video_frame(&frame, &params),
            Err(PictError::Unsupported(_))
        ));
        assert_eq!(
            PixelFormat::from(PictPixelFormat::Rgb24),
            PixelFormat::Rgb24
        );
        assert!(PictPixelFormat::try_from(PixelFormat::Pal8).is_err());
    }

    #[test]
    fn encoder_factory_writes_a_decodable_pict() {
        let mut params = CodecParameters::video(CodecId::new(crate::CODEC_ID_STR));
        params.width = Some(2);
        params.height = Some(2);
        params.pixel_format = Some(PixelFormat::Bgra);
        let mut enc = make_encoder(&params).unwrap();
        let frame = VideoFrame {
            pts: None,
            planes: vec![VideoPlane {
                stride: 8,
                data: vec![
                    30, 20, 10, 0, 60, 50, 40, 0, //
                    90, 80, 70, 0, 120, 110, 100, 0,
                ],
            }],
        };
        enc.send_frame(&Frame::Video(frame)).unwrap();
        let pkt = enc.receive_packet().unwrap();
        assert!(pkt.flags.keyframe);
        let img = crate::decode(&pkt.data).unwrap();
        assert_eq!(
            img.to_rgb8(),
            vec![10, 20, 30, 40, 50, 60, 70, 80, 90, 100, 110, 120]
        );
        enc.flush().unwrap();
        assert!(matches!(
            enc.receive_packet(),
            Err(oxideav_core::Error::Eof)
        ));
    }

    #[test]
    fn register_via_runtime_context_installs_factories() {
        let mut ctx = RuntimeContext::new();
        register(&mut ctx);
        assert!(
            ctx.codecs.decoder_ids().next().is_some(),
            "register(ctx) should install codec decoder factories"
        );
        let params = CodecParameters::video(CodecId::new(crate::CODEC_ID_STR));
        assert!(ctx.codecs.first_decoder(&params).is_ok());
        assert!(ctx.codecs.first_encoder(&params).is_ok());
        assert_eq!(
            ctx.containers.container_for_extension("pict"),
            Some(crate::CODEC_ID_STR),
            "register(ctx) should install .pict extension hint"
        );
    }
}
