//! Decoding hooks for the image data carried by the QuickTime picture
//! opcodes (`$8200` `CompressedQuickTime`, and the matte of `$8201`).
//!
//! Inside Macintosh: QuickTime (1993) makes the `$8200` payload a
//! **codec boundary**: the `cType` FourCC of the embedded
//! [`ImageDescription`] names the decompressor component that turns
//! the image data into pixels (page 3-50), exactly like a QuickTime
//! movie's sample description does. `oxideav-pict` therefore never
//! implements a compressor itself — it hands the description + bytes
//! to a [`QuickTimeImageDecoder`] and composites whatever RGBA comes
//! back (see [`crate::parse_pict_with`]).
//!
//! Three decoders ship with the crate:
//!
//! * [`RawQuickTimeDecoder`] — the `'raw '` "compressor" (Table 3-3,
//!   book page 3-64: the *raw* compressor "does not compress"): the
//!   image data is the source pixel map itself, laid out by the
//!   description's `depth`. Framework-free.
//! * [`DefaultQuickTimeDecoder`] — what [`crate::parse_pict`] uses:
//!   `'raw '` through the built-in path and, with the `registry`
//!   feature, `'jpeg'` (the "Photo - JPEG" compressor) through the
//!   sibling `oxideav-mjpeg` decoder — the same arrangement the
//!   workspace's other embedded-codec image formats use.
//! * `RegistryQuickTimeDecoder` (`registry` feature, in
//!   [`crate::registry`]) — resolves the FourCC through a caller's
//!   `oxideav_core::CodecRegistry` first and falls back to the
//!   default chain.
//!
//! A compressor nobody can decode is **not** an error for the
//! picture: the opcode's wrapper stays typed, the canvas keeps
//! whatever the earlier opcodes drew, and the outcome is recorded as
//! [`QuickTimeRender::Unsupported`] on
//! [`PictQuickTime::render`](crate::PictQuickTime::render) — the
//! page 3-26 rule that a reader without the decompressor still
//! honours the `Size` field and carries on.

use crate::error::{PictError, Result};
use crate::quicktime::ImageDescription;
use crate::state::RectI32;

/// Pixels produced by a [`QuickTimeImageDecoder`]: packed RGBA8,
/// row-major, `width × height`, no row padding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedQuickTimeImage {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `width × height × 4` bytes, R G B A per pixel.
    pub rgba: Vec<u8>,
}

impl DecodedQuickTimeImage {
    /// Wrap an RGBA buffer, checking that it is exactly
    /// `width × height × 4` bytes and within the crate's raster
    /// budget ([`crate::MAX_RASTER_BYTES`]).
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Result<Self> {
        let expected = (width as usize)
            .checked_mul(height as usize)
            .and_then(|n| n.checked_mul(4))
            .ok_or_else(|| PictError::invalid("decoded QuickTime image dimensions overflow"))?;
        if expected > crate::MAX_RASTER_BYTES {
            return Err(PictError::invalid(format!(
                "decoded QuickTime image {width}×{height} exceeds the raster budget"
            )));
        }
        if rgba.len() != expected {
            return Err(PictError::invalid(format!(
                "decoded QuickTime image {width}×{height} needs {expected} RGBA bytes, got {}",
                rgba.len()
            )));
        }
        Ok(Self {
            width,
            height,
            rgba,
        })
    }
}

/// Turns the image data of a QuickTime picture opcode into pixels.
///
/// Implementations receive the embedded [`ImageDescription`] (the
/// compressor FourCC, declared dimensions, depth, colour-table id and
/// any extension bytes) plus the image data bytes exactly as stored,
/// and return packed RGBA. Return [`PictError::Unsupported`] for a
/// compressor you do not handle — the decoder records that as
/// [`QuickTimeRender::Unsupported`] and keeps going; any other error
/// is recorded as [`QuickTimeRender::Failed`]. Neither fails the
/// picture.
pub trait QuickTimeImageDecoder {
    /// Decode one image (or matte) described by `description`.
    fn decode_image(
        &mut self,
        description: &ImageDescription,
        data: &[u8],
    ) -> Result<DecodedQuickTimeImage>;
}

/// What happened to a QuickTime picture opcode's pixels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuickTimeRender {
    /// The image reached the canvas. `dst` is the destination
    /// bounding rectangle in picture coordinates — the source
    /// rectangle mapped through the opcode's matrix (Inside
    /// Macintosh: QuickTime `StdPix`, page 3-138: the matrix
    /// "specifies the mapping of the source rectangle to the
    /// destination"), before clipping.
    Rendered {
        /// Destination bounding box in picture coordinates.
        dst: RectI32,
        /// `Some(reason)` when the opcode carried a matte that could
        /// not be decoded (unsupported matte compressor, short data):
        /// the image was drawn **unblended** — the page 3-26 posture
        /// of drawing what can be drawn — and the reason is kept here.
        /// `None` when there was no matte or it was applied.
        matte_skipped: Option<String>,
        /// How many text-drawing opcodes of the emitter's *default
        /// warning placeholder* were skipped after this image drew.
        /// Inside Macintosh: QuickTime page 3-139: `StdPix` appends
        /// "default picture opcodes (for displaying a warning when
        /// QuickTime is not installed)" — the "QuickTime™ and a
        /// <name> decompressor are needed to see this picture" text
        /// — after the `$8200`; a reader that *did* decode the image
        /// is the QuickTime-installed case and must not draw them.
        /// The books do not state the suppression mechanism, so the
        /// decoder skips a run consisting solely of pen / text-state
        /// and text-drawing opcodes that ends at the `NOP` the
        /// emitter leaves before `OpEndPic`, and reports the count
        /// here (`0` = nothing skipped).
        placeholder_skipped: u32,
    },
    /// No decoder for the compressor named by the image description
    /// (or a depth / layout the `'raw '` path does not cover). The
    /// wrapper and image bytes stay available typed on
    /// [`PictQuickTime::image`](crate::PictQuickTime::image).
    Unsupported {
        /// The `cType` FourCC that was not handled.
        codec: [u8; 4],
        /// Human-readable reason.
        reason: String,
    },
    /// A decoder was found but rejected the data, or the decoded
    /// image could not be composited (singular matrix, empty crop).
    Failed(String),
    /// Nothing was attempted: the payload interior did not match the
    /// published layout (`image == None`) — page 3-26 degradation.
    NotAttempted,
}

impl QuickTimeRender {
    /// `true` when pixels landed on the canvas.
    pub fn is_rendered(&self) -> bool {
        matches!(self, Self::Rendered { .. })
    }
}

/// Classify a decoder error into the render outcome it maps to.
pub(crate) fn render_outcome(codec: [u8; 4], err: PictError) -> QuickTimeRender {
    match err {
        PictError::Unsupported(reason) => QuickTimeRender::Unsupported { codec, reason },
        other => QuickTimeRender::Failed(other.to_string()),
    }
}

/// The QuickTime `'raw '` compressor: image data **is** the pixel
/// map (Table 3-3, book page 3-64 — the raw compressor "does not
/// compress"), rows in order, pixels packed per the description's
/// `depth` (page 3-51):
///
/// * `32` — four bytes per pixel, pad/alpha byte first then R, G, B
///   (the Color QuickDraw 32-bit direct layout); the leading byte is
///   not consulted, pixels are opaque.
/// * `24` — three bytes per pixel, R, G, B.
/// * `16` — one big-endian word per pixel, `0RRRRRGGGGGBBBBB` (the
///   16-bit direct layout, 5 bits per component).
///
/// Indexed depths (1 / 2 / 4 / 8) and the grayscale encodings (34 /
/// 36 / 40) need the colour table named by `clutID` — either a
/// system `'clut'` resource or a custom table stored in the image
/// description's extension bytes, whose on-disk layout is outside
/// the staged documentation — and are reported as unsupported.
#[derive(Debug, Default, Clone, Copy)]
pub struct RawQuickTimeDecoder;

/// The `'raw '` compressor FourCC.
pub const RAW_CODEC: [u8; 4] = *b"raw ";
/// The `'jpeg'` ("Photo - JPEG") compressor FourCC.
pub const JPEG_CODEC: [u8; 4] = *b"jpeg";

impl QuickTimeImageDecoder for RawQuickTimeDecoder {
    fn decode_image(
        &mut self,
        description: &ImageDescription,
        data: &[u8],
    ) -> Result<DecodedQuickTimeImage> {
        if description.codec != RAW_CODEC {
            return Err(PictError::unsupported(format!(
                "no decoder for QuickTime compressor '{}'",
                description.codec_str()
            )));
        }
        let width = description.width as u32;
        let height = description.height as u32;
        if width == 0 || height == 0 {
            return Err(PictError::invalid(
                "'raw ' QuickTime image with a zero dimension",
            ));
        }
        let bytes_per_pixel = match description.depth {
            32 => 4usize,
            24 => 3,
            16 => 2,
            other => {
                return Err(PictError::unsupported(format!(
                "'raw ' QuickTime image at depth {other} needs the colour table named by clutID {}",
                description.clut_id
            )))
            }
        };
        let row_len = width as usize * bytes_per_pixel;
        let needed = row_len
            .checked_mul(height as usize)
            .ok_or_else(|| PictError::invalid("'raw ' QuickTime image size overflows"))?;
        if data.len() < needed {
            return Err(PictError::invalid(format!(
                "'raw ' QuickTime image {width}×{height} at depth {} needs {needed} bytes, payload has {}",
                description.depth,
                data.len()
            )));
        }
        let pixels = width as usize * height as usize;
        let rgba_len = pixels
            .checked_mul(4)
            .filter(|&n| n <= crate::MAX_RASTER_BYTES)
            .ok_or_else(|| {
                PictError::invalid("'raw ' QuickTime image exceeds the raster budget")
            })?;
        let mut rgba = vec![0u8; rgba_len];
        for (i, px) in rgba.chunks_exact_mut(4).enumerate() {
            let src = &data[i * bytes_per_pixel..(i + 1) * bytes_per_pixel];
            match bytes_per_pixel {
                4 => {
                    px[0] = src[1];
                    px[1] = src[2];
                    px[2] = src[3];
                }
                3 => {
                    px[0] = src[0];
                    px[1] = src[1];
                    px[2] = src[2];
                }
                _ => {
                    let p = u16::from_be_bytes([src[0], src[1]]);
                    let r5 = ((p >> 10) & 0x1F) as u8;
                    let g5 = ((p >> 5) & 0x1F) as u8;
                    let b5 = (p & 0x1F) as u8;
                    px[0] = (r5 << 3) | (r5 >> 2);
                    px[1] = (g5 << 3) | (g5 >> 2);
                    px[2] = (b5 << 3) | (b5 >> 2);
                }
            }
            px[3] = 0xFF;
        }
        DecodedQuickTimeImage::new(width, height, rgba)
    }
}

/// The decoder chain [`crate::parse_pict`] uses when the caller
/// supplies none: `'raw '` through [`RawQuickTimeDecoder`] and — with
/// the `registry` feature — `'jpeg'` through `oxideav-mjpeg`. Every
/// other compressor is reported unsupported.
#[derive(Debug, Default, Clone, Copy)]
pub struct DefaultQuickTimeDecoder {
    raw: RawQuickTimeDecoder,
}

impl QuickTimeImageDecoder for DefaultQuickTimeDecoder {
    fn decode_image(
        &mut self,
        description: &ImageDescription,
        data: &[u8],
    ) -> Result<DecodedQuickTimeImage> {
        #[cfg(feature = "registry")]
        if description.codec == JPEG_CODEC {
            return decode_jpeg_image(description, data);
        }
        self.raw.decode_image(description, data)
    }
}

/// Decode a `'jpeg'` ("Photo - JPEG") payload through the sibling
/// `oxideav-mjpeg` decoder. The image data is one complete T.81
/// interchange stream (`FFD8 … FFD9`) — the real-world `$8200`
/// samples carry exactly that — so it goes to the framework-free
/// `decode_jpeg` entry point unchanged; the decoded frame's planes
/// are then folded to RGBA by [`video_frame_to_rgba`].
#[cfg(feature = "registry")]
pub fn decode_jpeg_image(
    description: &ImageDescription,
    data: &[u8],
) -> Result<DecodedQuickTimeImage> {
    let info = oxideav_mjpeg::jpeg::inspect::inspect_jpeg(data)
        .map_err(|e| PictError::invalid(format!("'jpeg' QuickTime image: {e}")))?;
    let frame = oxideav_mjpeg::decoder::decode_jpeg(data, None)
        .map_err(|e| PictError::invalid(format!("'jpeg' QuickTime image: {e}")))?;
    // The frame header (SOF) is authoritative for the pixel geometry;
    // the ImageDescription's width/height are what the emitter
    // *declared*, and a disagreement is the JPEG's to win.
    let _ = description;
    video_frame_to_rgba(
        &frame,
        info.width as u32,
        info.height as u32,
        PackedQuad::Cmyk,
    )
}

/// How a single 4-bytes-per-pixel plane is interpreted by
/// [`video_frame_to_rgba`] — the framework's `VideoFrame` carries no
/// pixel-format tag, so the caller states which packed quad layout
/// its producer emits.
#[cfg(feature = "registry")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackedQuad {
    /// R, G, B, A.
    Rgba,
    /// C, M, Y, K (what a four-component JPEG decodes to).
    Cmyk,
}

/// Fold an `oxideav_core::VideoFrame` of `width × height` pixels into
/// packed RGBA.
///
/// The framework frame carries plane bytes and strides but no pixel
/// format, so the layout is inferred from the plane geometry:
///
/// * one plane of `width × height` bytes — grayscale, or a palette
///   image when the frame carries a palette side-channel;
/// * one plane of `3 × width × height` bytes — packed RGB;
/// * one plane of `4 × width × height` bytes — packed quad per `quad`;
/// * three (or four, with alpha) planes — planar Y′CbCr, with the
///   chroma subsampling read off each chroma plane's stride relative
///   to the luma width, upsampled by sample replication and converted
///   with the JFIF full-range matrix.
#[cfg(feature = "registry")]
pub fn video_frame_to_rgba(
    frame: &oxideav_core::VideoFrame,
    width: u32,
    height: u32,
    quad: PackedQuad,
) -> Result<DecodedQuickTimeImage> {
    let w = width as usize;
    let h = height as usize;
    if w == 0 || h == 0 {
        return Err(PictError::invalid("decoded frame has a zero dimension"));
    }
    let pixels = w
        .checked_mul(h)
        .ok_or_else(|| PictError::invalid("decoded frame dimensions overflow"))?;
    let planes = frame.image_planes();
    let mut rgba =
        vec![
            0xFFu8;
            pixels
                .checked_mul(4)
                .filter(|&n| n <= crate::MAX_RASTER_BYTES)
                .ok_or_else(|| PictError::invalid("decoded frame exceeds the raster budget"))?
        ];
    fn row(p: &oxideav_core::VideoPlane, y: usize, min_len: usize) -> Result<&[u8]> {
        let start = y * p.stride;
        p.data
            .get(start..)
            .filter(|r| r.len() >= min_len)
            .ok_or_else(|| PictError::invalid("decoded frame plane shorter than its geometry"))
    }
    match planes.len() {
        1 => {
            let p = &planes[0];
            let bpp = if p.stride >= w * 4 && p.data.len() >= w * 4 * h {
                4
            } else if p.stride >= w * 3 && p.data.len() >= w * 3 * h {
                3
            } else if p.stride >= w && p.data.len() >= w * h {
                1
            } else {
                return Err(PictError::invalid(
                    "decoded single-plane frame does not match its declared geometry",
                ));
            };
            let palette = frame.palette();
            for y in 0..h {
                let src = row(p, y, w * bpp)?;
                let dst = &mut rgba[y * w * 4..(y + 1) * w * 4];
                for x in 0..w {
                    let d = &mut dst[x * 4..x * 4 + 4];
                    match bpp {
                        4 => {
                            let s = &src[x * 4..x * 4 + 4];
                            match quad {
                                PackedQuad::Rgba => d.copy_from_slice(s),
                                PackedQuad::Cmyk => {
                                    let k = 255 - s[3] as u32;
                                    d[0] = ((255 - s[0] as u32) * k / 255) as u8;
                                    d[1] = ((255 - s[1] as u32) * k / 255) as u8;
                                    d[2] = ((255 - s[2] as u32) * k / 255) as u8;
                                    d[3] = 0xFF;
                                }
                            }
                        }
                        3 => {
                            d[..3].copy_from_slice(&src[x * 3..x * 3 + 3]);
                            d[3] = 0xFF;
                        }
                        _ => {
                            let v = src[x];
                            match palette.and_then(|_| frame.palette_rgb(v)) {
                                Some([r, g, b]) => {
                                    d[0] = r;
                                    d[1] = g;
                                    d[2] = b;
                                }
                                None => {
                                    d[0] = v;
                                    d[1] = v;
                                    d[2] = v;
                                }
                            }
                            d[3] = 0xFF;
                        }
                    }
                }
            }
        }
        3 | 4 => {
            let (py, pcb, pcr) = (&planes[0], &planes[1], &planes[2]);
            // Chroma geometry: a chroma plane narrower than the luma
            // width is horizontally subsampled by 2; fewer rows than
            // the luma height means vertical subsampling by 2.
            let cw = if pcb.stride >= w { w } else { w.div_ceil(2) };
            let chroma_rows =
                |p: &oxideav_core::VideoPlane| p.data.len().checked_div(p.stride).unwrap_or(0);
            let ch = if chroma_rows(pcb) >= h && chroma_rows(pcr) >= h {
                h
            } else {
                h.div_ceil(2)
            };
            let hshift = usize::from(cw < w);
            let vshift = usize::from(ch < h);
            for y in 0..h {
                let ys = row(py, y, w)?;
                let cy = y >> vshift;
                let cbs = row(pcb, cy, cw)?;
                let crs = row(pcr, cy, cw)?;
                let alpha = if planes.len() == 4 {
                    Some(row(&planes[3], y, w)?)
                } else {
                    None
                };
                let dst = &mut rgba[y * w * 4..(y + 1) * w * 4];
                for x in 0..w {
                    let cx = x >> hshift;
                    let (r, g, b) = ycbcr_to_rgb(ys[x], cbs[cx], crs[cx]);
                    let d = &mut dst[x * 4..x * 4 + 4];
                    d[0] = r;
                    d[1] = g;
                    d[2] = b;
                    d[3] = alpha.map_or(0xFF, |a| a[x]);
                }
            }
        }
        n => {
            return Err(PictError::unsupported(format!(
                "decoded frame with {n} planes has no known RGBA mapping"
            )))
        }
    }
    DecodedQuickTimeImage::new(width, height, rgba)
}

/// JFIF full-range Y′CbCr → RGB (T.871 §7), fixed-point with 16
/// fractional bits and round-to-nearest.
#[cfg(feature = "registry")]
fn ycbcr_to_rgb(y: u8, cb: u8, cr: u8) -> (u8, u8, u8) {
    const ONE: i64 = 1 << 16;
    const HALF: i64 = 1 << 15;
    // 1.402, 0.344136, 0.714136, 1.772 in 16.16.
    const CR_R: i64 = 91_881;
    const CB_G: i64 = 22_554;
    const CR_G: i64 = 46_802;
    const CB_B: i64 = 116_130;
    let y = y as i64 * ONE;
    let cb = cb as i64 - 128;
    let cr = cr as i64 - 128;
    let clamp = |v: i64| ((v + HALF) >> 16).clamp(0, 255) as u8;
    (
        clamp(y + CR_R * cr),
        clamp(y - CB_G * cb - CR_G * cr),
        clamp(y + CB_B * cb),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::header::Fixed;

    fn desc(codec: [u8; 4], width: u16, height: u16, depth: u16) -> ImageDescription {
        ImageDescription {
            id_size: 86,
            codec,
            version: 1,
            revision_level: 1,
            vendor: *b"appl",
            temporal_quality: 0,
            spatial_quality: 0x0300,
            width,
            height,
            h_res: Fixed::SEVENTY_TWO_DPI,
            v_res: Fixed::SEVENTY_TWO_DPI,
            data_size: 0,
            frame_count: 1,
            name_raw: [0u8; 32],
            depth,
            clut_id: -1,
            extension: Vec::new(),
        }
    }

    #[test]
    fn raw_32_bit_takes_rgb_after_the_pad_byte() {
        let d = desc(RAW_CODEC, 2, 1, 32);
        let data = [0xFF, 0x08, 0x05, 0x01, 0x00, 0x10, 0x20, 0x30];
        let img = RawQuickTimeDecoder.decode_image(&d, &data).unwrap();
        assert_eq!(img.rgba, [8, 5, 1, 255, 0x10, 0x20, 0x30, 255]);
    }

    #[test]
    fn raw_24_and_16_bit_layouts() {
        let d = desc(RAW_CODEC, 1, 2, 24);
        let img = RawQuickTimeDecoder
            .decode_image(&d, &[1, 2, 3, 4, 5, 6])
            .unwrap();
        assert_eq!(img.rgba, [1, 2, 3, 255, 4, 5, 6, 255]);
        let d = desc(RAW_CODEC, 1, 1, 16);
        // 0 11111 00000 00000 = pure red at 5 bits.
        let img = RawQuickTimeDecoder.decode_image(&d, &[0x7C, 0x00]).unwrap();
        assert_eq!(img.rgba, [255, 0, 0, 255]);
    }

    #[test]
    fn raw_short_payload_and_indexed_depth_are_typed_errors() {
        let d = desc(RAW_CODEC, 4, 4, 32);
        assert!(matches!(
            RawQuickTimeDecoder.decode_image(&d, &[0u8; 10]),
            Err(PictError::InvalidData(_))
        ));
        let d = desc(RAW_CODEC, 4, 4, 8);
        assert!(matches!(
            RawQuickTimeDecoder.decode_image(&d, &[0u8; 16]),
            Err(PictError::Unsupported(_))
        ));
        let d = desc(*b"rpza", 4, 4, 16);
        assert!(matches!(
            DefaultQuickTimeDecoder::default().decode_image(&d, &[0u8; 64]),
            Err(PictError::Unsupported(_))
        ));
    }

    #[test]
    fn decoded_image_length_is_checked() {
        assert!(DecodedQuickTimeImage::new(2, 2, vec![0; 15]).is_err());
        assert!(DecodedQuickTimeImage::new(2, 2, vec![0; 16]).is_ok());
        assert!(DecodedQuickTimeImage::new(u32::MAX, u32::MAX, Vec::new()).is_err());
    }

    #[cfg(feature = "registry")]
    #[test]
    fn ycbcr_matrix_hits_the_primaries() {
        assert_eq!(ycbcr_to_rgb(128, 128, 128), (128, 128, 128));
        assert_eq!(ycbcr_to_rgb(255, 128, 128), (255, 255, 255));
        assert_eq!(ycbcr_to_rgb(0, 128, 128), (0, 0, 0));
        // JFIF encode of pure red (255,0,0): Y=76, Cb=85, Cr=255.
        let (r, g, b) = ycbcr_to_rgb(76, 85, 255);
        assert!(r >= 253 && g <= 2 && b <= 2, "{r},{g},{b}");
    }

    #[cfg(feature = "registry")]
    #[test]
    fn frame_geometry_inference_covers_gray_rgb_and_planar() {
        use oxideav_core::{VideoFrame, VideoPlane};
        let gray = VideoFrame {
            pts: None,
            planes: vec![VideoPlane {
                stride: 2,
                data: vec![10, 20, 30, 40],
            }],
        };
        let img = video_frame_to_rgba(&gray, 2, 2, PackedQuad::Rgba).unwrap();
        assert_eq!(&img.rgba[..8], &[10, 10, 10, 255, 20, 20, 20, 255]);

        let rgb = VideoFrame {
            pts: None,
            planes: vec![VideoPlane {
                stride: 6,
                data: vec![1, 2, 3, 4, 5, 6],
            }],
        };
        let img = video_frame_to_rgba(&rgb, 2, 1, PackedQuad::Rgba).unwrap();
        assert_eq!(img.rgba, [1, 2, 3, 255, 4, 5, 6, 255]);

        // 4:2:0 planar, 2×2 luma, one chroma sample.
        let yuv = VideoFrame {
            pts: None,
            planes: vec![
                VideoPlane {
                    stride: 2,
                    data: vec![128; 4],
                },
                VideoPlane {
                    stride: 1,
                    data: vec![128],
                },
                VideoPlane {
                    stride: 1,
                    data: vec![128],
                },
            ],
        };
        let img = video_frame_to_rgba(&yuv, 2, 2, PackedQuad::Rgba).unwrap();
        assert!(img.rgba.chunks_exact(4).all(|p| p == [128, 128, 128, 255]));
    }
}
