//! The root vocabulary of the image-crate contract (`IMAGE_CRATE_API`):
//! `probe` / `info` / `decode*` / `encode*`, all framework-free.

use std::io::{Read, Write};

use crate::error::{PictError as Error, Result};
use crate::image::{ImageInfo, PictImage, RgbImage, RgbaImage};
use crate::options::{DecodeOptions, EncodeOptions};
use crate::qtimage::{DefaultQuickTimeDecoder, QuickTimeImageDecoder};

/// `true` when `bytes` carry a PICT picture record — the §A-3 version
/// stanza (`$0011 $02FF` for version 2, `$11 $01` for version 1) at
/// offset 10 of a record that starts at byte 0 or, after Apple's
/// 512-byte launch-stub prefix, at byte 512. Allocation-free; `false`
/// on short input.
pub fn probe(bytes: &[u8]) -> bool {
    crate::decoder::probe_bytes(bytes)
}

/// Describe a PICT from its picture record header without walking a
/// drawing opcode: `picFrame` dimensions, the native layout
/// [`decode`] would return (always `Rgba`), `frames` (always 1), the
/// framing [`PictVersion`](crate::PictVersion), whether the file has
/// the launch stub, and the v2 `HeaderOp` payload (resolution,
/// optimal source rectangle) when present.
///
/// A degenerate frame (zero width or height) is reported as such;
/// [`decode`] rejects it.
pub fn info(bytes: &[u8]) -> Result<ImageInfo> {
    crate::decoder::read_info(bytes)
}

/// Rasterise a PICT onto its picture frame with
/// [`DecodeOptions::default`] and the default QuickTime-image decoder
/// chain (`'raw '` built in, `'jpeg'` through `oxideav-mjpeg` with the
/// `registry` feature).
///
/// Accepts both forms real-world generators produce: the raw picture
/// record at offset 0, or the 512-byte launch-stub prefix + record.
/// Returns [`Error::NoRaster`] if the opcode stream terminates without
/// drawing anything (and carries no QuickTime payload).
pub fn decode(bytes: &[u8]) -> Result<PictImage> {
    decode_with(bytes, &DecodeOptions::default())
}

/// [`decode`] under explicit limits / strictness.
pub fn decode_with(bytes: &[u8], opts: &DecodeOptions) -> Result<PictImage> {
    let mut qt = DefaultQuickTimeDecoder::default();
    crate::decoder::decode_image(bytes, opts, &mut qt)
}

/// [`decode_with`] with a caller-supplied decoder for the image data of
/// `CompressedQuickTime` (`$8200`) opcodes and QuickTime mattes.
///
/// The `$8200` payload is a codec boundary (Inside Macintosh:
/// QuickTime, page 3-50: the `cType` FourCC names the decompressor);
/// `qt` is asked for the pixels of every such image and the result is
/// composited per the opcode's matrix / matte / mask / mode. Pass a
/// `RegistryQuickTimeDecoder`
/// (feature `registry`) to route FourCCs through an
/// `oxideav_core::CodecRegistry`, or any [`QuickTimeImageDecoder`] of
/// your own.
pub fn decode_with_quicktime(
    bytes: &[u8],
    opts: &DecodeOptions,
    qt: &mut dyn QuickTimeImageDecoder,
) -> Result<PictImage> {
    crate::decoder::decode_image(bytes, opts, qt)
}

/// Decode straight to tightly packed 8-bit RGB (the canvas with its
/// always-opaque alpha dropped).
pub fn decode_rgb8(bytes: &[u8]) -> Result<RgbImage> {
    let img = decode(bytes)?;
    Ok(RgbImage::new(img.width, img.height, img.to_rgb8()))
}

/// Decode straight to tightly packed 8-bit RGBA (alpha `255`
/// throughout — QuickDraw carries no alpha).
pub fn decode_rgba8(bytes: &[u8]) -> Result<RgbaImage> {
    let img = decode(bytes)?;
    let (width, height) = (img.width, img.height);
    Ok(RgbaImage::new(width, height, img.into_raw()))
}

/// Read `r` to its end and [`decode`] the bytes.
pub fn decode_from<R: Read>(mut r: R) -> Result<PictImage> {
    let mut buf = Vec::new();
    r.read_to_end(&mut buf)?;
    decode(&buf)
}

/// Encode `image` as a PICT file whose drawing is one `DirectBitsRect`
/// raster, per `opts`. See [`EncodeOptions`] for what the image's
/// header and comments become on the wire.
pub fn encode(image: &PictImage, opts: &EncodeOptions) -> Result<Vec<u8>> {
    crate::encoder::encode_image(image, opts)
}

/// Encode tightly packed 8-bit RGB (`3 × width × height` bytes) as a
/// PICT. The pixels are written as an RGBDirect PixMap exactly as
/// [`encode_rgba8`] writes them (PICT has no alpha to drop).
pub fn encode_rgb8(width: u32, height: u32, rgb: &[u8], opts: &EncodeOptions) -> Result<Vec<u8>> {
    check_raw_len(width, height, 3, rgb.len())?;
    encode(&PictImage::from_rgb8(width, height, rgb.to_vec())?, opts)
}

/// Encode tightly packed 8-bit RGBA (`4 × width × height` bytes) as a
/// PICT. The alpha byte is not stored (QuickDraw has none); the picture
/// decodes back with alpha `255`.
pub fn encode_rgba8(width: u32, height: u32, rgba: &[u8], opts: &EncodeOptions) -> Result<Vec<u8>> {
    check_raw_len(width, height, 4, rgba.len())?;
    encode(&PictImage::from_rgba8(width, height, rgba.to_vec())?, opts)
}

/// [`encode`] straight into a writer.
pub fn encode_to<W: Write>(image: &PictImage, opts: &EncodeOptions, mut w: W) -> Result<()> {
    let bytes = encode(image, opts)?;
    w.write_all(&bytes)?;
    Ok(())
}

fn check_raw_len(width: u32, height: u32, bpp: usize, len: usize) -> Result<()> {
    let need = (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(bpp))
        .ok_or_else(|| Error::invalid("PICT encoder: dimensions overflow"))?;
    if len < need {
        return Err(Error::invalid(format!(
            "PICT encoder: {width}x{height} at {bpp} bytes/pixel needs {need} bytes, got {len}"
        )));
    }
    Ok(())
}

// ---- Pre-contract names, kept for one release -------------------------

/// The pre-contract name of [`crate::inspect`].
#[deprecated(
    note = "use oxideav_pict::inspect (IMAGE_CRATE_API naming; `probe` is the bool sniff)"
)]
pub fn probe_pict(bytes: &[u8]) -> Result<crate::probe::PictProbe> {
    crate::probe::inspect(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_is_total_on_short_input() {
        for n in 0..16 {
            assert!(!probe(&vec![0u8; n]));
        }
        assert!(!probe(&[]));
    }

    #[test]
    fn check_raw_len_rejects_short_buffers() {
        assert!(check_raw_len(2, 2, 4, 16).is_ok());
        assert!(matches!(
            check_raw_len(2, 2, 4, 15),
            Err(Error::InvalidData(_))
        ));
        assert!(matches!(
            check_raw_len(u32::MAX, u32::MAX, 4, 0),
            Err(Error::InvalidData(_))
        ));
    }
}
