//! PICT writer — rounds 3 / 4 / 5.
//!
//! Round 2 shipped a minimal packType=1 v2-only encoder. Round 3 added
//! v2 packTypes 2/4 + v1 emit + ClipRgn injection. Round 4 added v2
//! packType 3 (16-bpp PackBits) + the [`crate::ops::PictBuilder`] for
//! drawing-only PICT streams. Round 5 brings:
//!
//! * **`encode_pict_v1` with [`PackType`] selector** — v1 streams
//!   gain packType 2/3/4 emit, identical layout to v2 but inside the
//!   8-bit-opcode v1 wrapper (no headerOp stanza, no 512-byte stub).
//!   The previous behaviour (packType 1 only) is preserved because
//!   [`encode_pict_v1`] now defaults to `PackType::Raw`.
//!
//! * **1-bpp BitMap encoders** ([`encode_pict_bits_rect`] /
//!   [`encode_pict_pack_bits_rect`]) — emit `BitsRect` (`0x0090`) or
//!   `PackBitsRect` (`0x0098`) opcodes for monochrome images. The
//!   input is RGBA8 (per the rest of the encoder API); pixels are
//!   reduced to 1 bpp via a 50 %-luminance threshold (Y =
//!   0.299 R + 0.587 G + 0.114 B, threshold 128). PackBitsRect uses
//!   the same RLE algorithm as packType 1 BitMap rows.
//!
//! * **[`crate::ops::PictBuilder::raster`]** — append a
//!   DirectBitsRect raster chunk to a drawing-builder so callers can
//!   mix drawing primitives + raster in the same v2 stream.
//!
//! Cross-validation: every output produced by this module decodes
//! cleanly via [`crate::decoder::parse_pict`].
//!
//! Round 211 adds the **indexed-PixMap** variants of the four BitMap /
//! PackBitsRect / region opcodes (Inside Macintosh §A-3 footnote `§`:
//! "If the high bit of rowBytes is set, then it is a pixel map containing
//! multiple bits per pixel"). The round-186 indexed decoder already
//! consumes them; the encoder side now emits 1/2/4/8-bpp PixData rows
//! plus the embedded ColorTable across all four
//! `BitsRect`/`PackBitsRect`/`BitsRgn`/`PackBitsRgn` opcodes — closing
//! the indexed-PixMap round-trip the README flagged as the next
//! follow-up.

use crate::error::{PictError, Result};
use crate::header::PictHeader;
use crate::packbits;
use crate::state::RectI32;

/// Build the 24-byte `HeaderOp` payload for an extended-v2 PICT covering
/// `(0, 0)..(width, height)` at 72.0 dpi. Matches the Listing A-5
/// canonical form (`version=-2`, `hRes=vRes=$00480000`,
/// `optimal_source_rect=picFrame`, reserved fields zero).
fn extended_v2_header_payload(width: u32, height: u32) -> [u8; 24] {
    let pf = RectI32::from_be(0, 0, height as i16, width as i16);
    PictHeader::extended_v2_72dpi(pf).to_wire()
}

// ---------------------------------------------------------------------------
// Public pack-type selector.
// ---------------------------------------------------------------------------

/// Selection of DirectBitsRect on-disk encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackType {
    /// packType 1 — raw 4 bytes per pixel (`0xFF R G B`). Largest, always
    /// readable.
    Raw,
    /// packType 2 — 3 bytes per pixel (`R G B`), no pad/fill byte.
    /// 25 % smaller than `Raw` for opaque images.
    Packed24,
    /// packType 3 — 16-bit-per-pixel (A1R5G5B5), each row PackBits-RLE
    /// at u16 unit size. Typically 30–60 % smaller than `Raw` and the
    /// pixel format Mac Color QuickDraw used for video memory until
    /// the 32-bit transition. Slight quality loss vs `Raw` (5 bits per
    /// channel instead of 8); the alpha bit is always set.
    Rle16,
    /// packType 4 — component-separated PackBits. Each row's R, G, B
    /// planes encoded independently by [`packbits::encode`]. Typically
    /// 20–40 % smaller than `Raw` for photographic content; larger than
    /// `Raw` for random noise.
    ComponentPackBits,
}

// ---------------------------------------------------------------------------
// v2 encoder (default public API).
// ---------------------------------------------------------------------------

/// Encode an RGBA8 raster (`width × height × 4 bytes`, row-major) as a
/// PICT **v2** byte stream using `packType=1` raw pixel data.
///
/// Equivalent to `encode_pict_v2(width, height, data, PackType::Raw)`.
/// Kept for API symmetry with round 2.
pub fn encode_pict(width: u32, height: u32, data: &[u8]) -> Result<Vec<u8>> {
    encode_pict_v2(width, height, data, PackType::Raw)
}

/// Encode an RGBA8 raster as a PICT **v2** byte stream using the chosen
/// [`PackType`].
///
/// Returns `InvalidData` if:
/// * `data.len() != width × height × 4`
/// * `width` or `height` is 0
/// * `width × 4` (or `width × 3` for `Packed24`) exceeds the 14-bit
///   PICT v2 rowBytes limit (16 383 bytes per row).
pub fn encode_pict_v2(width: u32, height: u32, data: &[u8], pack: PackType) -> Result<Vec<u8>> {
    validate_dims(width, height, data)?;

    // For packType 3 the on-disk pixel is a 16-bit u16; for the other
    // packTypes it's 32 bits. `row_bytes_raw` is the rowBytes value
    // we write into the PixMap header — the *uncompressed* byte
    // stride per row, in pixel-size units.
    let row_bytes_raw: usize = match pack {
        PackType::Rle16 => width as usize * 2,
        _ => width as usize * 4,
    };
    let row_bytes_disk: usize = match pack {
        PackType::Raw => width as usize * 4,
        PackType::Packed24 => width as usize * 3,
        PackType::Rle16 => width as usize * 2, // post-PackBits byte count varies
        PackType::ComponentPackBits => width as usize * 4,
    };
    if row_bytes_disk > 0x3FFF {
        return Err(PictError::invalid(format!(
            "encode: rowBytes {row_bytes_disk} exceeds the 14-bit PICT v2 PixMap limit"
        )));
    }

    let pack_type_word: u16 = match pack {
        PackType::Raw => 1,
        PackType::Packed24 => 2,
        PackType::Rle16 => 3,
        PackType::ComponentPackBits => 4,
    };

    // Conservative capacity (exact for raw, larger than needed for
    // compressed — Vec will shrink via push).
    let mut out: Vec<u8> = Vec::with_capacity(512 + 80 + row_bytes_disk * height as usize + 4);

    // 512-byte launch-stub prefix.
    out.extend_from_slice(&[0u8; 512]);

    // Picture record: picSize (0) + picFrame.
    write_u16(&mut out, 0);
    write_i16(&mut out, 0); // top
    write_i16(&mut out, 0); // left
    write_i16(&mut out, height as i16); // bottom
    write_i16(&mut out, width as i16); // right

    // v2 sentinel + headerOp stanza.
    write_u16(&mut out, 0x0011);
    write_u16(&mut out, 0x02FF);
    write_u16(&mut out, 0x0C00);
    out.extend_from_slice(&extended_v2_header_payload(width, height));

    // DirectBitsRect opcode.
    write_u16(&mut out, 0x009A);
    write_u32(&mut out, 0x000000FF); // baseAddr placeholder
    write_u16(&mut out, (row_bytes_raw as u16) | 0x8000); // rowBytes with PixMap flag

    // bounds.
    write_i16(&mut out, 0);
    write_i16(&mut out, 0);
    write_i16(&mut out, height as i16);
    write_i16(&mut out, width as i16);

    // pmVersion, packType, packSize.
    write_u16(&mut out, 0);
    write_u16(&mut out, pack_type_word);
    write_u32(&mut out, 0);

    // hRes / vRes = 72 dpi.
    write_u32(&mut out, 0x00480000);
    write_u32(&mut out, 0x00480000);

    // pixelType, pixelSize, cmpCount, cmpSize.
    let (pixel_size, cmp_size) = match pack {
        PackType::Rle16 => (16u16, 5u16),
        _ => (32u16, 8u16),
    };
    write_u16(&mut out, 16); // RGBDirect
    write_u16(&mut out, pixel_size);
    write_u16(&mut out, 3); // cmpCount=3 (no alpha plane)
    write_u16(&mut out, cmp_size);

    // planeBytes, pmTable, pmReserved.
    write_u32(&mut out, 0);
    write_u32(&mut out, 0);
    write_u32(&mut out, 0);

    // srcRect / dstRect.
    for _ in 0..2 {
        write_i16(&mut out, 0);
        write_i16(&mut out, 0);
        write_i16(&mut out, height as i16);
        write_i16(&mut out, width as i16);
    }

    // mode = srcCopy.
    write_u16(&mut out, 0);

    // Pixel data per row.
    write_pixel_rows(&mut out, width, height, data, pack, row_bytes_raw)?;

    // Word-align before terminator.
    if out.len() % 2 != 0 {
        out.push(0);
    }
    write_u16(&mut out, 0x00FF); // OpEndPic
    Ok(out)
}

/// Emit per-row PixMap pixel data for a DirectBitsRect-style opcode
/// (shared by [`encode_pict_v2`] and [`encode_pict_v1_with`]).
///
/// `row_bytes_raw` is the *uncompressed* byte stride; the byteCount
/// prefix size for packTypes 3/4 is `1` if `row_bytes_raw <= 250`,
/// otherwise `2` (Inside Macintosh §A-3).
fn write_pixel_rows(
    out: &mut Vec<u8>,
    width: u32,
    height: u32,
    data: &[u8],
    pack: PackType,
    row_bytes_raw: usize,
) -> Result<()> {
    let w = width as usize;
    for y in 0..height as usize {
        let row_in = &data[y * w * 4..(y + 1) * w * 4];
        match pack {
            PackType::Raw => {
                // 0xFF R G B per pixel.
                for px in row_in.chunks_exact(4) {
                    out.push(0xFF);
                    out.push(px[0]);
                    out.push(px[1]);
                    out.push(px[2]);
                }
            }
            PackType::Packed24 => {
                // R G B per pixel (no pad byte).
                for px in row_in.chunks_exact(4) {
                    out.push(px[0]);
                    out.push(px[1]);
                    out.push(px[2]);
                }
            }
            PackType::Rle16 => {
                // Pack each pixel as A1R5G5B5 BE, then u16-PackBits.
                let mut row_u16: Vec<u16> = Vec::with_capacity(w);
                for px in row_in.chunks_exact(4) {
                    let r5 = (px[0] >> 3) as u16 & 0x1F;
                    let g5 = (px[1] >> 3) as u16 & 0x1F;
                    let b5 = (px[2] >> 3) as u16 & 0x1F;
                    let a1 = 0x8000u16; // alpha bit always set
                    row_u16.push(a1 | (r5 << 10) | (g5 << 5) | b5);
                }
                let encoded = packbits::encode_u16(&row_u16);
                let total = encoded.len();
                // byteCount prefix: 1 byte if rowBytes <= 250, else 2.
                if row_bytes_raw > 250 {
                    write_u16(out, total as u16);
                } else {
                    out.push(total as u8);
                }
                out.extend_from_slice(&encoded);
            }
            PackType::ComponentPackBits => {
                // Separate R, G, B planes then PackBits-encode each.
                let mut r_plane = vec![0u8; w];
                let mut g_plane = vec![0u8; w];
                let mut b_plane = vec![0u8; w];
                for (x, px) in row_in.chunks_exact(4).enumerate() {
                    r_plane[x] = px[0];
                    g_plane[x] = px[1];
                    b_plane[x] = px[2];
                }
                let rc = packbits::encode(&r_plane);
                let gc = packbits::encode(&g_plane);
                let bc = packbits::encode(&b_plane);
                let total = rc.len() + gc.len() + bc.len();
                // byteCount prefix: 1 byte if row_bytes_raw ≤ 250,
                // else 2 bytes. Inside Macintosh §A-3: threshold is
                // 250 (not 256).
                if row_bytes_raw > 250 {
                    write_u16(out, total as u16);
                } else {
                    out.push(total as u8);
                }
                out.extend_from_slice(&rc);
                out.extend_from_slice(&gc);
                out.extend_from_slice(&bc);
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// PICT v1 encoder.
// ---------------------------------------------------------------------------

/// Encode an RGBA8 raster as a **PICT v1** byte stream using
/// [`PackType::Raw`] pixel data.
///
/// Equivalent to `encode_pict_v1_with(width, height, data, PackType::Raw)`.
/// Preserves the round-3 API behaviour (packType 1 only). Use
/// [`encode_pict_v1_with`] for the round-5 [`PackType`] selector.
pub fn encode_pict_v1(width: u32, height: u32, data: &[u8]) -> Result<Vec<u8>> {
    encode_pict_v1_with(width, height, data, PackType::Raw)
}

/// Encode an RGBA8 raster as a **PICT v1** byte stream using the
/// chosen [`PackType`].
///
/// v1 PICTs use 8-bit opcodes, no word alignment between opcodes, and a
/// simpler 10-byte picture-record header (no v2 headerOp stanza). The
/// pixel data is emitted as a v1 `DirectBitsRect` (opcode `0x9A`),
/// identical PixMap-header layout to the v2 `0x009A` opcode.
///
/// **Conformance caveat (round 401):** Inside Macintosh: Imaging With
/// QuickDraw §A-3 Table A-3 defines no `$9A` opcode for version 1
/// pictures — the v1 raster opcodes stop at the BitMap-based
/// `$90`/`$91`/`$98`/`$99`, and a version-1 walker has no skip rule
/// for unknown opcodes. Streams from this function therefore rely on
/// the consumer accepting the v2-style `DirectBitsRect` body inside a
/// v1 framing (this crate's decoder does; a strict Table-A-3-only
/// reader will not). For a fully Table-A-3-conformant v1 raster use
/// the 1-bpp [`encode_pict_v1_bits_rect`] /
/// [`encode_pict_v1_pack_bits_rect`] pair, or draw through
/// [`crate::ops::PictV1Builder`].
///
/// The v1 wire shape pre-dates System 7 but is still in wide use for
/// legacy interchange.
///
/// **Note:** This function does NOT emit a 512-byte launch-stub prefix
/// because v1 files pre-date the stub convention. If a consuming
/// application requires the stub, prepend 512 zero bytes manually.
pub fn encode_pict_v1_with(
    width: u32,
    height: u32,
    data: &[u8],
    pack: PackType,
) -> Result<Vec<u8>> {
    validate_dims(width, height, data)?;

    // For packType 3 the on-disk pixel is a 16-bit u16; for the other
    // packTypes it's 32 bits. `row_bytes_raw` is the rowBytes value
    // we write into the PixMap header — the *uncompressed* byte
    // stride per row, in pixel-size units.
    let row_bytes_raw: usize = match pack {
        PackType::Rle16 => width as usize * 2,
        _ => width as usize * 4,
    };
    if row_bytes_raw > 0x3FFF {
        return Err(PictError::invalid(format!(
            "encode_pict_v1: rowBytes {row_bytes_raw} exceeds 14-bit limit"
        )));
    }

    let pack_type_word: u16 = match pack {
        PackType::Raw => 1,
        PackType::Packed24 => 2,
        PackType::Rle16 => 3,
        PackType::ComponentPackBits => 4,
    };

    let (pixel_size, cmp_size) = match pack {
        PackType::Rle16 => (16u16, 5u16),
        _ => (32u16, 8u16),
    };

    let mut out: Vec<u8> = Vec::with_capacity(10 + 4 + row_bytes_raw * height as usize + 4);

    // 10-byte picture record header (NO 512-byte stub for v1).
    write_u16(&mut out, 0); // picSize
    write_i16(&mut out, 0); // picFrame top
    write_i16(&mut out, 0); // left
    write_i16(&mut out, height as i16); // bottom
    write_i16(&mut out, width as i16); // right

    // v1 version stanza: opcode 0x11, version 0x01.
    out.push(0x11);
    out.push(0x01);

    // v1 DirectBitsRect opcode: single byte 0x9A.
    out.push(0x9A);

    // PixMap header (same layout as v2 0x009A).
    write_u32(&mut out, 0x000000FF); // baseAddr
    write_u16(&mut out, (row_bytes_raw as u16) | 0x8000); // rowBytes + PixMap flag

    // bounds.
    write_i16(&mut out, 0);
    write_i16(&mut out, 0);
    write_i16(&mut out, height as i16);
    write_i16(&mut out, width as i16);

    // pmVersion, packType, packSize.
    write_u16(&mut out, 0);
    write_u16(&mut out, pack_type_word);
    write_u32(&mut out, 0);

    // hRes / vRes = 72 dpi.
    write_u32(&mut out, 0x00480000);
    write_u32(&mut out, 0x00480000);

    // pixelType, pixelSize, cmpCount, cmpSize.
    write_u16(&mut out, 16);
    write_u16(&mut out, pixel_size);
    write_u16(&mut out, 3);
    write_u16(&mut out, cmp_size);

    // planeBytes, pmTable, pmReserved.
    write_u32(&mut out, 0);
    write_u32(&mut out, 0);
    write_u32(&mut out, 0);

    // srcRect / dstRect / mode.
    for _ in 0..2 {
        write_i16(&mut out, 0);
        write_i16(&mut out, 0);
        write_i16(&mut out, height as i16);
        write_i16(&mut out, width as i16);
    }
    write_u16(&mut out, 0); // mode = srcCopy

    // Pixel data per row.
    write_pixel_rows(&mut out, width, height, data, pack, row_bytes_raw)?;

    // v1 OpEndPic: single byte 0xFF.
    out.push(0xFF);
    Ok(out)
}

// ---------------------------------------------------------------------------
// DirectBitsRect opcode-bytes builder (used by PictBuilder::raster).
// ---------------------------------------------------------------------------

/// Build the bytes for a single PICT v2 `DirectBitsRect` (`0x009A`)
/// opcode + payload at picture-frame coordinates `(top, left, bottom,
/// right)`. No 512-byte stub, no headerOp, no `OpEndPic` — just the
/// opcode word and the PixMap header + pixel data.
///
/// The image is RGBA8 row-major; the `(top, left, bottom, right)`
/// coordinate quadruple specifies the dst rect in picture-frame coords.
/// The raster's intrinsic width and height are derived from
/// `right - left` and `bottom - top`; the input data buffer must be
/// `width × height × 4` bytes in size.
///
/// This is the building block [`crate::ops::PictBuilder::raster`] uses
/// to fold a raster into a drawing-only stream.
///
/// The record's `mode` (transfer mode) word is `0` (`srcCopy`); use
/// [`build_direct_bits_rect_op_with_mode`] to emit one of the other
/// §3-113 Boolean source modes or a §4 arithmetic transfer mode.
pub fn build_direct_bits_rect_op(
    top: i16,
    left: i16,
    bottom: i16,
    right: i16,
    data: &[u8],
    pack: PackType,
) -> Result<Vec<u8>> {
    build_direct_bits_rect_op_with_mode(top, left, bottom, right, data, pack, 0)
}

/// [`build_direct_bits_rect_op`] with an explicit transfer-mode word.
///
/// `mode` is written verbatim into the record's `mode` field (§A-3
/// Listing A-2 — *"mode: Mode; {transfer mode}"*): `0..=7` are the
/// §3-113 Boolean source modes (`srcCopy` … `notSrcBic`), `32..=39`
/// the §4 arithmetic transfer modes (`blend` … `adMin`), and `+ 64`
/// requests dithering (`ditherCopy`, additive per §3-114).
#[allow(clippy::too_many_arguments)]
pub fn build_direct_bits_rect_op_with_mode(
    top: i16,
    left: i16,
    bottom: i16,
    right: i16,
    data: &[u8],
    pack: PackType,
    mode: u16,
) -> Result<Vec<u8>> {
    if bottom <= top || right <= left {
        return Err(PictError::invalid(format!(
            "build_direct_bits_rect_op: degenerate rect ({top},{left})→({bottom},{right})"
        )));
    }
    let width = (right - left) as u32;
    let height = (bottom - top) as u32;
    let expected = width as usize * height as usize * 4;
    if data.len() != expected {
        return Err(PictError::invalid(format!(
            "build_direct_bits_rect_op: data.len() = {} but width × height × 4 = {expected}",
            data.len()
        )));
    }

    let row_bytes_raw: usize = match pack {
        PackType::Rle16 => width as usize * 2,
        _ => width as usize * 4,
    };
    if row_bytes_raw > 0x3FFF {
        return Err(PictError::invalid(format!(
            "build_direct_bits_rect_op: rowBytes {row_bytes_raw} exceeds the 14-bit limit"
        )));
    }

    let pack_type_word: u16 = match pack {
        PackType::Raw => 1,
        PackType::Packed24 => 2,
        PackType::Rle16 => 3,
        PackType::ComponentPackBits => 4,
    };
    let (pixel_size, cmp_size) = match pack {
        PackType::Rle16 => (16u16, 5u16),
        _ => (32u16, 8u16),
    };

    let mut buf = Vec::with_capacity(2 + 50 + row_bytes_raw * height as usize);
    write_u16(&mut buf, 0x009A); // DirectBitsRect
    write_u32(&mut buf, 0x000000FF); // baseAddr
    write_u16(&mut buf, (row_bytes_raw as u16) | 0x8000); // rowBytes + PixMap flag
                                                          // bounds: 0..height, 0..width (the bitmap's own coordinate frame).
    write_i16(&mut buf, 0);
    write_i16(&mut buf, 0);
    write_i16(&mut buf, height as i16);
    write_i16(&mut buf, width as i16);
    // pmVersion, packType, packSize.
    write_u16(&mut buf, 0);
    write_u16(&mut buf, pack_type_word);
    write_u32(&mut buf, 0);
    // hRes / vRes = 72 dpi.
    write_u32(&mut buf, 0x00480000);
    write_u32(&mut buf, 0x00480000);
    // pixelType, pixelSize, cmpCount, cmpSize.
    write_u16(&mut buf, 16); // RGBDirect
    write_u16(&mut buf, pixel_size);
    write_u16(&mut buf, 3);
    write_u16(&mut buf, cmp_size);
    // planeBytes, pmTable, pmReserved.
    write_u32(&mut buf, 0);
    write_u32(&mut buf, 0);
    write_u32(&mut buf, 0);
    // srcRect: bitmap-local 0..width × 0..height.
    write_i16(&mut buf, 0);
    write_i16(&mut buf, 0);
    write_i16(&mut buf, height as i16);
    write_i16(&mut buf, width as i16);
    // dstRect: caller-supplied picture-frame coordinates.
    write_i16(&mut buf, top);
    write_i16(&mut buf, left);
    write_i16(&mut buf, bottom);
    write_i16(&mut buf, right);
    // mode — the record's transfer-mode word.
    write_u16(&mut buf, mode);
    // Pixel rows.
    write_pixel_rows(&mut buf, width, height, data, pack, row_bytes_raw)?;
    Ok(buf)
}

// ---------------------------------------------------------------------------
// PixPat opcode-bytes builders (round 91 — colour 8×8 pixel pattern).
// ---------------------------------------------------------------------------

/// Which PixPat slot to emit — `BkPixPat 0x0012`, `PnPixPat 0x0013` or
/// `FillPixPat 0x0014`. Mirrors the three monochrome `BkPat / PnPat /
/// FillPat` opcodes, just with a multi-colour 8×8 tile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixPatSlot {
    /// Background pattern — consumed by erase verbs.
    Background,
    /// Pen pattern — consumed by frame / paint verbs.
    Pen,
    /// Fill pattern — consumed by fill verbs.
    Fill,
}

impl PixPatSlot {
    /// PICT opcode word for this slot.
    pub fn opcode(self) -> u16 {
        match self {
            PixPatSlot::Background => 0x0012,
            PixPatSlot::Pen => 0x0013,
            PixPatSlot::Fill => 0x0014,
        }
    }
}

/// Build the bytes for a single PICT v2 PixPat opcode (`0x0012` /
/// `0x0013` / `0x0014`) carrying a **colour-pixmap** (`patType=1`)
/// 8×8 pixel pattern. Inside Macintosh: Imaging With QuickDraw §A-3
/// Listing A-1.
///
/// * `slot` — selects `BkPixPat` / `PnPixPat` / `FillPixPat`.
/// * `fallback` — the 8-byte monochrome `Pat1Data` field; classic
///   QuickDraw consults this when the colour pixmap can't be honoured
///   (typically on b/w screens). Round-trips through the decoder as
///   the `Pattern` portion of the resulting `PictPattern::ColourPixmap`.
/// * `pixels` — 8 rows × 8 columns of RGBA, row-major. The on-disk
///   representation packs them into 8 bpp indexed pixels against a
///   ColorTable holding every distinct colour seen (≤ 64 entries).
///
/// The emitted PixData uses unpacked 8 bpp (Inside Macintosh §A-3 PixData
/// case `rowBytes < 8`: 8 row bytes × 8 rows = 64 bytes flat — no
/// PackBits prefix). pixelSize = 8, cmpCount = 1, cmpSize = 8;
/// packType = 0 (default = no packing) per §4.
///
/// Returns `InvalidData` if more than 256 distinct colours are present
/// (the indexed PixData uses an 8 bpp palette, capping at 256 entries).
/// 64 distinct colours is the theoretical maximum for an 8×8 tile, so
/// this can't fire in practice.
pub fn build_pix_pat_op(
    slot: PixPatSlot,
    fallback: [u8; 8],
    pixels: &[[u8; 4]; 64],
) -> Result<Vec<u8>> {
    build_pix_pat_op_sized(slot, fallback, 8, 8, pixels)
}

/// Build the bytes for a single PICT v2 PixPat opcode (`0x0012` /
/// `0x0013` / `0x0014`) carrying a **colour-pixmap** (`patType=1`)
/// pixel pattern of an arbitrary power-of-2 `width`×`height` tile.
///
/// Inside Macintosh §3 (book page 3-40): *"A pixel pattern … can be of
/// any width and height that's a power of 2."* — round 302 exposes the
/// arbitrary-tile encoder so the decoder's new power-of-2 path can be
/// round-trip tested; [`build_pix_pat_op`] is the 8×8 special case.
///
/// * `width` / `height` must both be powers of two and `width * height`
///   must equal `pixels.len()`, else `InvalidData`.
/// * `pixels` — `width * height` RGBA cells, row-major.
///
/// The on-disk PixData uses 8 bpp indexed pixels against a deduplicated
/// ColorTable (≤ 256 entries). Per Inside Macintosh §A-3 "PixData":
/// rows with `rowBytes < 8` are emitted unpacked (flat), wider rows are
/// emitted as per-row `byteCount` + PackBits (matching the decoder's
/// `decode_pix_pat` reader).
pub fn build_pix_pat_op_sized(
    slot: PixPatSlot,
    fallback: [u8; 8],
    width: u16,
    height: u16,
    pixels: &[[u8; 4]],
) -> Result<Vec<u8>> {
    if width == 0 || height == 0 || !width.is_power_of_two() || !height.is_power_of_two() {
        return Err(PictError::invalid(format!(
            "build_pix_pat_op_sized: width/height must be non-zero powers of two (got {width}×{height})"
        )));
    }
    let cells = width as usize * height as usize;
    if pixels.len() != cells {
        return Err(PictError::invalid(format!(
            "build_pix_pat_op_sized: pixels.len()={} != width*height={cells}",
            pixels.len()
        )));
    }

    // Build a deduplicated palette + per-cell indices.
    let mut palette: Vec<[u8; 4]> = Vec::new();
    let mut indices = vec![0u8; cells];
    for (i, px) in pixels.iter().enumerate() {
        let idx = match palette.iter().position(|p| p == px) {
            Some(j) => j,
            None => {
                if palette.len() >= 256 {
                    return Err(PictError::invalid(format!(
                        "build_pix_pat_op_sized: palette overflowed 256 entries at cell {i}"
                    )));
                }
                palette.push(*px);
                palette.len() - 1
            }
        };
        indices[i] = idx as u8;
    }

    let row_bytes = width as usize; // 8 bpp → 1 byte per pixel.
    let mut buf: Vec<u8> =
        Vec::with_capacity(2 + 10 + 46 + 8 + palette.len() * 8 + row_bytes * height as usize);

    // Opcode word + patType + Pat1Data.
    write_u16(&mut buf, slot.opcode());
    write_u16(&mut buf, 1); // patType = 1 (colour-pixmap)
    buf.extend_from_slice(&fallback);

    // PixMap (sans baseAddr) — 46 bytes.
    // rowBytes (PixMap flag set + row stride).
    write_u16(&mut buf, (row_bytes as u16) | 0x8000);
    // bounds: 0,0,height,width.
    write_i16(&mut buf, 0);
    write_i16(&mut buf, 0);
    write_i16(&mut buf, height as i16);
    write_i16(&mut buf, width as i16);
    // pmVersion, packType, packSize.
    write_u16(&mut buf, 0);
    write_u16(&mut buf, 0);
    write_u32(&mut buf, 0);
    // hRes / vRes = 72 dpi.
    write_u32(&mut buf, 0x00480000);
    write_u32(&mut buf, 0x00480000);
    // pixelType = 0 (indexed), pixelSize = 8, cmpCount = 1, cmpSize = 8.
    write_u16(&mut buf, 0);
    write_u16(&mut buf, 8);
    write_u16(&mut buf, 1);
    write_u16(&mut buf, 8);
    // planeBytes, pmTable, pmReserved.
    write_u32(&mut buf, 0);
    write_u32(&mut buf, 0);
    write_u32(&mut buf, 0);

    // ColorTable: ctSeed (4) + ctFlags (2) + ctSize (2) + (ctSize + 1) entries.
    write_u32(&mut buf, 0); // ctSeed (synth)
    write_u16(&mut buf, 0); // ctFlags (clear → PixMap, not device)
    let ct_size = (palette.len() as i16) - 1;
    write_i16(&mut buf, ct_size);
    for (i, rgba) in palette.iter().enumerate() {
        write_u16(&mut buf, i as u16); // value (= index)
                                       // RGBColor: 16-bit per channel, high byte = colour data.
        write_u16(&mut buf, ((rgba[0] as u16) << 8) | rgba[0] as u16);
        write_u16(&mut buf, ((rgba[1] as u16) << 8) | rgba[1] as u16);
        write_u16(&mut buf, ((rgba[2] as u16) << 8) | rgba[2] as u16);
    }

    // PixData per Inside Macintosh §A-3 "PixData": rowBytes < 8 → flat
    // (unpacked); otherwise per-row byteCount + PackBits (1-byte prefix
    // when rowBytes ≤ 250, else 2-byte).
    if row_bytes < 8 {
        buf.extend_from_slice(&indices);
    } else {
        for y in 0..height as usize {
            let row = &indices[y * row_bytes..(y + 1) * row_bytes];
            let enc = packbits::encode(row);
            if row_bytes > 250 {
                write_u16(&mut buf, enc.len() as u16);
            } else {
                buf.push(enc.len() as u8);
            }
            buf.extend_from_slice(&enc);
        }
    }

    Ok(buf)
}

/// Build the bytes for a single PICT v2 PixPat opcode (`0x0012` /
/// `0x0013` / `0x0014`) carrying a **dither** (`patType=2`) sub-type
/// record. Inside Macintosh: Imaging With QuickDraw §A-3 Listing A-1.
///
/// * `slot` — selects `BkPixPat` / `PnPixPat` / `FillPixPat`.
/// * `fallback` — the 8-byte monochrome `Pat1Data` field; classic
///   QuickDraw consults this when the colour pattern can't be honoured
///   (typically on b/w screens).
/// * `rgb` — the target `RGBColor` (R, G, B) the dither tile should
///   approximate. Each channel is replicated to 16-bit precision on
///   disk (`high8 = low8 = channel`), the format Color QuickDraw
///   stores for true 24-bit colour input.
///
/// The on-disk layout per Listing A-1 patType=2 branch:
///
/// ```text
/// opcode-word: 2 bytes ($0012 / $0013 / $0014)
/// PatType:     word    (= 2, "ditherPat")
/// Pat1Data:    Pattern (8 bytes — monochrome fallback)
/// RGB:         RGBColor (6 bytes — desired RGB at 16-bit precision)
/// ```
///
/// Total opcode payload: 18 bytes (16 + opcode word).
pub fn build_pix_pat_dither_op(slot: PixPatSlot, fallback: [u8; 8], rgb: [u8; 3]) -> Vec<u8> {
    let mut buf: Vec<u8> = Vec::with_capacity(18);
    write_u16(&mut buf, slot.opcode());
    write_u16(&mut buf, 2); // patType = 2 (ditherPat)
    buf.extend_from_slice(&fallback);
    // RGBColor: 16-bit per channel, replicate the 8-bit input across
    // both bytes so `high8 = colour data, low8 = colour data` — same
    // convention Color QuickDraw uses when storing 8-bit input as a
    // 16-bit `RGBColor`.
    let r16 = ((rgb[0] as u16) << 8) | rgb[0] as u16;
    let g16 = ((rgb[1] as u16) << 8) | rgb[1] as u16;
    let b16 = ((rgb[2] as u16) << 8) | rgb[2] as u16;
    write_u16(&mut buf, r16);
    write_u16(&mut buf, g16);
    write_u16(&mut buf, b16);
    buf
}

// ---------------------------------------------------------------------------
// 1-bpp BitMap encoders (BitsRect / PackBitsRect).
// ---------------------------------------------------------------------------

/// Reduce an RGBA8 raster to a 1-bit-per-pixel BitMap row buffer,
/// `row_bytes` bytes per row (rounded up from `ceil(width / 8)`).
///
/// Bits are packed MSB-first within each byte (column 0 is bit 7 of
/// byte 0). Per the decoder convention (`expand_1bpp_to_rgba`), bit
/// `1` represents black and bit `0` represents white. The encoder
/// uses a 50 %-luminance threshold (Y = 0.299 R + 0.587 G + 0.114 B,
/// `Y < 128` → ink/black/bit=1).
fn rgba_to_1bpp(width: u32, height: u32, data: &[u8], row_bytes: usize) -> Vec<u8> {
    let w = width as usize;
    let mut bitmap = vec![0u8; row_bytes * height as usize];
    for y in 0..height as usize {
        let row_in = &data[y * w * 4..(y + 1) * w * 4];
        for (x, px) in row_in.chunks_exact(4).enumerate() {
            // ITU-R BT.601 luminance approx; round to nearest with
            // integer arithmetic (multiply numerators × 1000, divide by
            // 1000 at the end).
            let y_val = (299 * px[0] as u32 + 587 * px[1] as u32 + 114 * px[2] as u32) / 1000;
            if y_val < 128 {
                bitmap[y * row_bytes + (x >> 3)] |= 0x80 >> (x & 7);
            }
        }
    }
    bitmap
}

/// Encode an RGBA8 raster as a v2 PICT containing a single
/// **`BitsRect`** (`0x0090`) opcode — 1-bpp BitMap, raw rows (no
/// PackBits).
///
/// The raster is reduced to 1 bpp via a 50 %-luminance threshold.
///
/// Returns `InvalidData` if the per-row stride exceeds the 14-bit
/// PICT v2 rowBytes limit (`0x3FFE`, since the top bit is reserved
/// for the PixMap flag).
pub fn encode_pict_bits_rect(width: u32, height: u32, data: &[u8]) -> Result<Vec<u8>> {
    encode_pict_bitmap(
        width, height, data, /* pack_bits = */ false, /* v1 = */ false,
    )
}

/// Encode an RGBA8 raster as a **PICT v1** containing a single
/// `BitsRect` (`$90`) opcode — the fully §A-3 Table-A-3-conformant v1
/// raster form (round 401). Same 1-bpp 50 %-luminance reduction and
/// BitMap body layout as [`encode_pict_bits_rect`], framed as a
/// version 1 picture: 10-byte record header, `$11 $01` stanza, 1-byte
/// opcodes, `$FF` terminator, no launch stub. Table A-3 footnote `‡`
/// notes `$90` "can only be used when rowBytes is less than 8", i.e.
/// images up to 63 columns; wider images must use
/// [`encode_pict_v1_pack_bits_rect`] (this function errors on them).
pub fn encode_pict_v1_bits_rect(width: u32, height: u32, data: &[u8]) -> Result<Vec<u8>> {
    if width.div_ceil(8) >= 8 {
        return Err(PictError::invalid(format!(
            "encode_pict_v1_bits_rect: width {width} gives rowBytes >= 8; §A-3 Table A-3 \
             footnote ‡ limits BitsRect to rowBytes < 8 — use encode_pict_v1_pack_bits_rect"
        )));
    }
    encode_pict_bitmap(
        width, height, data, /* pack_bits = */ false, /* v1 = */ true,
    )
}

/// Encode an RGBA8 raster as a **PICT v1** containing a single
/// `PackBitsRect` (`$98`) opcode — PackBits-RLE rows when
/// `rowBytes >= 8`, raw narrow rows below that (§A-3 carve-out). The
/// Table-A-3-conformant packed v1 raster form (round 401); see
/// [`encode_pict_v1_bits_rect`] for the framing notes.
pub fn encode_pict_v1_pack_bits_rect(width: u32, height: u32, data: &[u8]) -> Result<Vec<u8>> {
    encode_pict_bitmap(
        width, height, data, /* pack_bits = */ true, /* v1 = */ true,
    )
}

/// Encode an RGBA8 raster as a v2 PICT containing a single
/// **`PackBitsRect`** (`0x0098`) opcode — 1-bpp BitMap, PackBits-RLE
/// rows (only when `rowBytes >= 8`, per Inside Macintosh §A-3).
///
/// The raster is reduced to 1 bpp via a 50 %-luminance threshold.
/// For images narrower than 64 columns (`rowBytes < 8`), the opcode's
/// per-row data is laid out raw with no byteCount prefix and no
/// PackBits compression — same encoding as `BitsRect`.
pub fn encode_pict_pack_bits_rect(width: u32, height: u32, data: &[u8]) -> Result<Vec<u8>> {
    encode_pict_bitmap(
        width, height, data, /* pack_bits = */ true, /* v1 = */ false,
    )
}

fn encode_pict_bitmap(
    width: u32,
    height: u32,
    data: &[u8],
    pack_bits: bool,
    v1: bool,
) -> Result<Vec<u8>> {
    validate_dims(width, height, data)?;
    let row_bytes = width.div_ceil(8) as usize;
    if row_bytes > 0x3FFE {
        return Err(PictError::invalid(format!(
            "encode_pict_bits_rect: rowBytes {row_bytes} exceeds the 14-bit limit"
        )));
    }
    let bitmap = rgba_to_1bpp(width, height, data, row_bytes);

    let mut out: Vec<u8> = Vec::with_capacity(560 + row_bytes * height as usize + 4);
    if !v1 {
        // 512-byte launch stub (v2 file convention; v1 pre-dates it).
        out.extend_from_slice(&[0u8; 512]);
    }
    // Picture record: picSize + picFrame.
    write_u16(&mut out, 0);
    write_i16(&mut out, 0);
    write_i16(&mut out, 0);
    write_i16(&mut out, height as i16);
    write_i16(&mut out, width as i16);
    if v1 {
        // v1 version stanza: opcode $11, version $01 — then 1-byte
        // opcodes with no word alignment (§A-3 Table A-3).
        out.push(0x11);
        out.push(0x01);
    } else {
        // v2 sentinel + headerOp stanza.
        write_u16(&mut out, 0x0011);
        write_u16(&mut out, 0x02FF);
        write_u16(&mut out, 0x0C00);
        out.extend_from_slice(&extended_v2_header_payload(width, height));
    }

    // Opcode: BitsRect (0x0090) or PackBitsRect (0x0098) — the same
    // numbering in Table A-3, one byte wide for v1. For BitMap
    // opcodes the rowBytes top bit must stay clear (the decoder
    // explicitly rejects rowBytes & 0x8000 != 0 here).
    let opcode: u16 = if pack_bits { 0x0098 } else { 0x0090 };
    if v1 {
        out.push(opcode as u8);
    } else {
        write_u16(&mut out, opcode);
    }

    // BitMap header: rowBytes, bounds, srcRect, dstRect, mode.
    write_u16(&mut out, row_bytes as u16);
    for _ in 0..3 {
        write_i16(&mut out, 0);
        write_i16(&mut out, 0);
        write_i16(&mut out, height as i16);
        write_i16(&mut out, width as i16);
    }
    write_u16(&mut out, 0); // mode = srcCopy

    // Per-row data.
    let h = height as usize;
    if !pack_bits || row_bytes < 8 {
        // BitsRect: always raw rows. PackBitsRect with rowBytes < 8:
        // also raw, no byteCount prefix (Inside Macintosh §A-3 carves
        // out this special case so very narrow bitmaps don't pay the
        // PackBits overhead).
        for y in 0..h {
            out.extend_from_slice(&bitmap[y * row_bytes..(y + 1) * row_bytes]);
        }
    } else {
        // PackBitsRect with rowBytes >= 8: per-row PackBits-encode the
        // raw scanline, prefix with byteCount (1 byte if rowBytes <=
        // 250, else 2).
        for y in 0..h {
            let raw = &bitmap[y * row_bytes..(y + 1) * row_bytes];
            let enc = packbits::encode(raw);
            let total = enc.len();
            if row_bytes > 250 {
                write_u16(&mut out, total as u16);
            } else {
                out.push(total as u8);
            }
            out.extend_from_slice(&enc);
        }
    }

    // Word-align before terminator.
    if v1 {
        // v1 terminator: 1-byte $FF, no alignment. Patch picSize (the
        // record starts at offset 0 — no stub) when it fits, per the
        // Table A-3 32 KB v1 sizing.
        out.push(0xFF);
        if let Ok(size) = u16::try_from(out.len()) {
            let size = size.to_be_bytes();
            out[0..2].copy_from_slice(&size);
        }
        return Ok(out);
    }
    if out.len() % 2 != 0 {
        out.push(0);
    }
    write_u16(&mut out, 0x00FF); // OpEndPic
    Ok(out)
}

/// Encode an RGBA8 raster as a v2 PICT containing a single
/// **`BitsRgn`** (`0x0091`) opcode — 1-bpp BitMap with a clipping
/// region attached just after the rect/mode header.
///
/// `clip` is the rectangular clip-region bbox in picture-frame
/// coordinates `(top, left, bottom, right)`. The region is emitted in
/// its trivial 10-byte form (no inversion data). The raster is
/// reduced to 1 bpp via the same 50 %-luminance threshold used by
/// [`encode_pict_bits_rect`].
///
/// Round 42: pairs with the existing `decode_bits_rect_v2(_,
/// with_region=true)` decoder path.
pub fn encode_pict_bits_rgn(
    width: u32,
    height: u32,
    data: &[u8],
    clip: [i16; 4],
) -> Result<Vec<u8>> {
    encode_pict_bitmap_with_region(width, height, data, /* pack_bits = */ false, clip)
}

/// Encode an RGBA8 raster as a v2 PICT containing a single
/// **`PackBitsRgn`** (`0x0099`) opcode — 1-bpp BitMap with a clipping
/// region. PackBits-RLE rows when `rowBytes >= 8`, otherwise raw rows
/// (Inside Macintosh §A-3 narrow-row carve-out).
///
/// `clip` is `[top, left, bottom, right]` in picture-frame coords.
pub fn encode_pict_pack_bits_rgn(
    width: u32,
    height: u32,
    data: &[u8],
    clip: [i16; 4],
) -> Result<Vec<u8>> {
    encode_pict_bitmap_with_region(width, height, data, /* pack_bits = */ true, clip)
}

fn encode_pict_bitmap_with_region(
    width: u32,
    height: u32,
    data: &[u8],
    pack_bits: bool,
    clip: [i16; 4],
) -> Result<Vec<u8>> {
    validate_dims(width, height, data)?;
    let row_bytes = width.div_ceil(8) as usize;
    if row_bytes > 0x3FFE {
        return Err(PictError::invalid(format!(
            "encode_pict_bits_rgn: rowBytes {row_bytes} exceeds the 14-bit limit"
        )));
    }
    let bitmap = rgba_to_1bpp(width, height, data, row_bytes);

    let mut out: Vec<u8> = Vec::with_capacity(560 + 12 + row_bytes * height as usize + 4);
    out.extend_from_slice(&[0u8; 512]);
    write_u16(&mut out, 0);
    write_i16(&mut out, 0);
    write_i16(&mut out, 0);
    write_i16(&mut out, height as i16);
    write_i16(&mut out, width as i16);
    write_u16(&mut out, 0x0011);
    write_u16(&mut out, 0x02FF);
    write_u16(&mut out, 0x0C00);
    out.extend_from_slice(&extended_v2_header_payload(width, height));

    let opcode = if pack_bits { 0x0099 } else { 0x0091 };
    write_u16(&mut out, opcode);

    write_u16(&mut out, row_bytes as u16);
    for _ in 0..3 {
        write_i16(&mut out, 0);
        write_i16(&mut out, 0);
        write_i16(&mut out, height as i16);
        write_i16(&mut out, width as i16);
    }
    write_u16(&mut out, 0); // mode = srcCopy

    // Region bytes: rgnSize=10 + bbox.
    write_u16(&mut out, 10);
    write_i16(&mut out, clip[0]);
    write_i16(&mut out, clip[1]);
    write_i16(&mut out, clip[2]);
    write_i16(&mut out, clip[3]);

    let h = height as usize;
    if !pack_bits || row_bytes < 8 {
        for y in 0..h {
            out.extend_from_slice(&bitmap[y * row_bytes..(y + 1) * row_bytes]);
        }
    } else {
        for y in 0..h {
            let raw = &bitmap[y * row_bytes..(y + 1) * row_bytes];
            let enc = packbits::encode(raw);
            let total = enc.len();
            if row_bytes > 250 {
                write_u16(&mut out, total as u16);
            } else {
                out.push(total as u8);
            }
            out.extend_from_slice(&enc);
        }
    }

    if out.len() % 2 != 0 {
        out.push(0);
    }
    write_u16(&mut out, 0x00FF);
    Ok(out)
}

// ---------------------------------------------------------------------------
// Indexed-PixMap encoder (round 211 — `BitsRect` / `PackBitsRect` PixMap
// variant per Inside Macintosh §A-3 footnote `§`).
// ---------------------------------------------------------------------------

/// 1, 2, 4 or 8 bits per pixel for an indexed PixMap. Inside Macintosh §4
/// ("Color QuickDraw and PixMaps") enumerates exactly these four indexed
/// pixel-size values; round-trip-pair to the decoder's
/// `read_indexed_pixel` switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexedPixelSize {
    /// 1 bit per pixel — 2-entry palette, 8 pixels per byte (MSB-first).
    OneBpp,
    /// 2 bits per pixel — 4-entry palette, 4 pixels per byte.
    TwoBpp,
    /// 4 bits per pixel — 16-entry palette, 2 pixels per byte.
    FourBpp,
    /// 8 bits per pixel — 256-entry palette, 1 pixel per byte. The most
    /// common indexed colour-QuickDraw mode.
    EightBpp,
}

impl IndexedPixelSize {
    /// Pixel-size field as written into the PixMap header.
    fn bits(self) -> u16 {
        match self {
            IndexedPixelSize::OneBpp => 1,
            IndexedPixelSize::TwoBpp => 2,
            IndexedPixelSize::FourBpp => 4,
            IndexedPixelSize::EightBpp => 8,
        }
    }

    /// Maximum palette entries this pixel size can address.
    fn max_palette_entries(self) -> usize {
        1 << self.bits()
    }

    /// `rowBytes` (header value) for a row of `width` pixels at this depth.
    /// Per Inside Macintosh §4 the row stride rounds up to the next byte.
    fn row_bytes(self, width: u32) -> usize {
        let w = width as usize;
        match self {
            IndexedPixelSize::OneBpp => w.div_ceil(8),
            IndexedPixelSize::TwoBpp => w.div_ceil(4),
            IndexedPixelSize::FourBpp => w.div_ceil(2),
            IndexedPixelSize::EightBpp => w,
        }
    }

    /// Pack a row of u8 indices into the on-disk bit-stream for this depth.
    /// MSB-first per QuickDraw convention. The output length matches
    /// [`Self::row_bytes`]; out-of-range indices are silently masked to
    /// the depth's bit width.
    fn pack_row(self, indices: &[u8], row_bytes: usize) -> Vec<u8> {
        let mut out = vec![0u8; row_bytes];
        match self {
            IndexedPixelSize::OneBpp => {
                for (x, &v) in indices.iter().enumerate() {
                    if v & 0x01 != 0 {
                        out[x >> 3] |= 0x80 >> (x & 7);
                    }
                }
            }
            IndexedPixelSize::TwoBpp => {
                for (x, &v) in indices.iter().enumerate() {
                    let shift = (3 - (x & 3)) * 2;
                    out[x >> 2] |= (v & 0x03) << shift;
                }
            }
            IndexedPixelSize::FourBpp => {
                for (x, &v) in indices.iter().enumerate() {
                    let shift = (1 - (x & 1)) * 4;
                    out[x >> 1] |= (v & 0x0F) << shift;
                }
            }
            IndexedPixelSize::EightBpp => {
                let n = indices.len().min(out.len());
                out[..n].copy_from_slice(&indices[..n]);
            }
        }
        out
    }
}

/// Encode an indexed image (one byte per pixel index, row-major) as a v2
/// PICT containing a single **`BitsRect`** (`0x0090`) opcode in its
/// **indexed-PixMap** variant (Inside Macintosh §A-3 footnote `§`:
/// "If the high bit of rowBytes is set, then it is a pixel map containing
/// multiple bits per pixel").
///
/// PixData rows are emitted raw (no PackBits prefix) — the `BitsRect`
/// opcode forces the unpacked PixData path regardless of `rowBytes`
/// (matching the decoder's `decode_bits_rect_v2` / `packed=false` arm).
/// For PackBits-RLE rows use [`encode_pict_indexed_pack_bits_rect`].
///
/// * `width` / `height` — picture-frame dimensions; must be non-zero.
/// * `indices` — `width × height` bytes; each byte is a palette index in
///   `0..palette.len()`. Indices outside that range are silently masked
///   to the chosen pixel size's bit width on disk; the decoder treats
///   out-of-range indices as black.
/// * `palette` — `RGBA8` colour table; up to `pixel_size.max_palette_entries()`
///   entries (`alpha` byte is ignored — QuickDraw ColorTable entries
///   are opaque RGB only). At least one entry is required.
/// * `pixel_size` — 1 / 2 / 4 / 8 bpp.
///
/// Returns `InvalidData` if:
/// * dimensions are zero or `indices.len() != width × height`,
/// * `palette` is empty or longer than `pixel_size.max_palette_entries()`,
/// * `rowBytes` exceeds the 14-bit PICT v2 limit (`0x3FFE`).
pub fn encode_pict_indexed_bits_rect(
    width: u32,
    height: u32,
    indices: &[u8],
    palette: &[[u8; 4]],
    pixel_size: IndexedPixelSize,
) -> Result<Vec<u8>> {
    encode_pict_indexed_pixmap(width, height, indices, palette, pixel_size, false, None)
}

/// Encode an indexed image as a v2 PICT containing a single
/// **`PackBitsRect`** (`0x0098`) opcode in its **indexed-PixMap** variant.
///
/// PixData rows are PackBits-RLE encoded when `rowBytes >= 8` (Inside
/// Macintosh §A-3 narrow-row carve-out: rows narrower than 8 bytes ship
/// raw with no byteCount prefix). The decoder is the matching
/// `decode_pack_bits_rect` / `packed=true` arm.
///
/// See [`encode_pict_indexed_bits_rect`] for parameter semantics.
pub fn encode_pict_indexed_pack_bits_rect(
    width: u32,
    height: u32,
    indices: &[u8],
    palette: &[[u8; 4]],
    pixel_size: IndexedPixelSize,
) -> Result<Vec<u8>> {
    encode_pict_indexed_pixmap(width, height, indices, palette, pixel_size, true, None)
}

/// Encode an indexed image as a v2 PICT containing a single **`BitsRgn`**
/// (`0x0091`) opcode in its **indexed-PixMap** variant — `BitsRect` plus a
/// rectangular clip region inserted just before the per-row pixel data.
///
/// `clip` is `[top, left, bottom, right]` in picture-frame coords. See
/// [`encode_pict_indexed_bits_rect`] for the rest of the parameter
/// semantics.
pub fn encode_pict_indexed_bits_rgn(
    width: u32,
    height: u32,
    indices: &[u8],
    palette: &[[u8; 4]],
    pixel_size: IndexedPixelSize,
    clip: [i16; 4],
) -> Result<Vec<u8>> {
    encode_pict_indexed_pixmap(
        width,
        height,
        indices,
        palette,
        pixel_size,
        false,
        Some(clip),
    )
}

/// Encode an indexed image as a v2 PICT containing a single
/// **`PackBitsRgn`** (`0x0099`) opcode in its **indexed-PixMap** variant —
/// `PackBitsRect` plus a rectangular clip region.
pub fn encode_pict_indexed_pack_bits_rgn(
    width: u32,
    height: u32,
    indices: &[u8],
    palette: &[[u8; 4]],
    pixel_size: IndexedPixelSize,
    clip: [i16; 4],
) -> Result<Vec<u8>> {
    encode_pict_indexed_pixmap(
        width,
        height,
        indices,
        palette,
        pixel_size,
        true,
        Some(clip),
    )
}

fn validate_indexed_dims(
    width: u32,
    height: u32,
    indices: &[u8],
    palette: &[[u8; 4]],
    pixel_size: IndexedPixelSize,
) -> Result<()> {
    if width == 0 || height == 0 {
        return Err(PictError::invalid(
            "encode_pict_indexed: width and height must be non-zero",
        ));
    }
    let expected = width as usize * height as usize;
    if indices.len() != expected {
        return Err(PictError::invalid(format!(
            "encode_pict_indexed: indices.len() = {} but width × height = {expected}",
            indices.len()
        )));
    }
    if palette.is_empty() {
        return Err(PictError::invalid(
            "encode_pict_indexed: palette must contain at least one entry",
        ));
    }
    let cap = pixel_size.max_palette_entries();
    if palette.len() > cap {
        return Err(PictError::invalid(format!(
            "encode_pict_indexed: palette has {} entries but pixelSize = {} bpp caps at {cap}",
            palette.len(),
            pixel_size.bits(),
        )));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn encode_pict_indexed_pixmap(
    width: u32,
    height: u32,
    indices: &[u8],
    palette: &[[u8; 4]],
    pixel_size: IndexedPixelSize,
    pack_bits: bool,
    clip: Option<[i16; 4]>,
) -> Result<Vec<u8>> {
    validate_indexed_dims(width, height, indices, palette, pixel_size)?;

    let row_bytes = pixel_size.row_bytes(width);
    if row_bytes > 0x3FFE {
        return Err(PictError::invalid(format!(
            "encode_pict_indexed: rowBytes {row_bytes} exceeds the 14-bit limit"
        )));
    }

    // Pack every row up front; we may reuse the buffer for both the
    // payload-size estimate and the actual write.
    let w = width as usize;
    let h = height as usize;
    let mut packed_rows: Vec<Vec<u8>> = Vec::with_capacity(h);
    for y in 0..h {
        let row = &indices[y * w..(y + 1) * w];
        packed_rows.push(pixel_size.pack_row(row, row_bytes));
    }

    let opcode: u16 = match (pack_bits, clip.is_some()) {
        (false, false) => 0x0090, // BitsRect
        (true, false) => 0x0098,  // PackBitsRect
        (false, true) => 0x0091,  // BitsRgn
        (true, true) => 0x0099,   // PackBitsRgn
    };

    // Header: stub + picSize + picFrame + v2 sentinel + headerOp.
    let mut out: Vec<u8> = Vec::with_capacity(560 + 80 + row_bytes * h + 4);
    out.extend_from_slice(&[0u8; 512]);
    write_u16(&mut out, 0); // picSize
    write_i16(&mut out, 0);
    write_i16(&mut out, 0);
    write_i16(&mut out, height as i16);
    write_i16(&mut out, width as i16);
    write_u16(&mut out, 0x0011);
    write_u16(&mut out, 0x02FF);
    write_u16(&mut out, 0x0C00);
    out.extend_from_slice(&extended_v2_header_payload(width, height));

    // Opcode + indexed PixMap header.
    write_u16(&mut out, opcode);

    // BitsRect / PackBitsRect / *Rgn variants of the indexed PixMap do
    // NOT carry a baseAddr field — Inside Macintosh §A-3 footnote `§`:
    // "PixMap data structure (excluding baseAddr) is included as data".
    // Only DirectBitsRect / DirectBitsRgn (0x009A / 0x009B) carry baseAddr.
    write_u16(&mut out, (row_bytes as u16) | 0x8000); // rowBytes + PixMap flag
    write_i16(&mut out, 0);
    write_i16(&mut out, 0);
    write_i16(&mut out, height as i16);
    write_i16(&mut out, width as i16);
    // pmVersion, packType, packSize.
    write_u16(&mut out, 0);
    write_u16(&mut out, 0); // packType = 0 (indexed PixData is per-row, not pmHeader-packed)
    write_u32(&mut out, 0);
    // hRes / vRes = 72 dpi (0x00480000 fixed-point).
    write_u32(&mut out, 0x00480000);
    write_u32(&mut out, 0x00480000);
    // pixelType = 0 (indexed), pixelSize, cmpCount = 1, cmpSize = pixelSize.
    write_u16(&mut out, 0);
    write_u16(&mut out, pixel_size.bits());
    write_u16(&mut out, 1);
    write_u16(&mut out, pixel_size.bits());
    // planeBytes, pmTable, pmReserved.
    write_u32(&mut out, 0);
    write_u32(&mut out, 0);
    write_u32(&mut out, 0);

    // ColorTable: ctSeed (4) + ctFlags (2) + ctSize (2) + entries (8 each).
    write_u32(&mut out, 0); // ctSeed (synth — decoder ignores)
    write_u16(&mut out, 0); // ctFlags (clear → PixMap, not device)
    let ct_size = (palette.len() as i16) - 1;
    write_i16(&mut out, ct_size);
    for (i, rgba) in palette.iter().enumerate() {
        // value: sequential index per §A-3 ColorSpec layout. The
        // decoder maps by position (palette[0] = entry 0), so this is
        // metadata only.
        write_u16(&mut out, i as u16);
        // RGBColor: 16-bit per channel; replicate the 8-bit input across
        // both bytes so `from_rgb16` recovers the 8-bit value exactly
        // (`high8 = colour data`, same convention used by `build_pix_pat_op`).
        write_u16(&mut out, ((rgba[0] as u16) << 8) | rgba[0] as u16);
        write_u16(&mut out, ((rgba[1] as u16) << 8) | rgba[1] as u16);
        write_u16(&mut out, ((rgba[2] as u16) << 8) | rgba[2] as u16);
    }

    // srcRect / dstRect.
    for _ in 0..2 {
        write_i16(&mut out, 0);
        write_i16(&mut out, 0);
        write_i16(&mut out, height as i16);
        write_i16(&mut out, width as i16);
    }
    // mode = srcCopy.
    write_u16(&mut out, 0);

    // Rectangular clip region (rgnSize = 10 + bbox) if requested.
    if let Some(bbox) = clip {
        write_u16(&mut out, 10);
        write_i16(&mut out, bbox[0]);
        write_i16(&mut out, bbox[1]);
        write_i16(&mut out, bbox[2]);
        write_i16(&mut out, bbox[3]);
    }

    // PixData rows — raw for the BitsRect / BitsRgn opcodes (or the
    // PackBitsRect narrow-row carve-out when rowBytes < 8). Otherwise
    // per-row PackBits with byteCount prefix (1 byte if rowBytes ≤ 250,
    // else 2 bytes).
    if !pack_bits || row_bytes < 8 {
        for row in &packed_rows {
            out.extend_from_slice(row);
        }
    } else {
        for row in &packed_rows {
            let enc = packbits::encode(row);
            let total = enc.len();
            if row_bytes > 250 {
                write_u16(&mut out, total as u16);
            } else {
                out.push(total as u8);
            }
            out.extend_from_slice(&enc);
        }
    }

    if out.len() % 2 != 0 {
        out.push(0);
    }
    write_u16(&mut out, 0x00FF); // OpEndPic
    Ok(out)
}

// ---------------------------------------------------------------------------
// ClipRgn opcode builder helper.
// ---------------------------------------------------------------------------

/// Build the bytes for a PICT v2 `ClipRgn` opcode (`0x0001`) carrying
/// a **rectangular** clipping region with the supplied `(top, left,
/// bottom, right)` bounds (picture-frame coordinates).
///
/// A rectangular region has `rgnSize == 10` (no inversion data), so
/// the total opcode payload is 12 bytes (opcode word + rgnSize + bbox).
///
/// Prepend the returned bytes to a v2 opcode stream (after the headerOp
/// stanza, before any drawing opcodes) to set the clipping rectangle.
/// [`encode_pict_v2_with_clip`] does this automatically.
pub fn build_clip_rgn_rect(top: i16, left: i16, bottom: i16, right: i16) -> Vec<u8> {
    let mut buf = Vec::with_capacity(12);
    write_u16(&mut buf, 0x0001); // ClipRgn opcode
    write_u16(&mut buf, 10); // rgnSize = 10 (rectangular, no inversion data)
    write_i16(&mut buf, top);
    write_i16(&mut buf, left);
    write_i16(&mut buf, bottom);
    write_i16(&mut buf, right);
    buf
}

/// Encode an RGBA8 raster as a PICT v2 stream, prepending a `ClipRgn`
/// opcode before the pixel data.
///
/// `clip` is `[top, left, bottom, right]` in picture-frame coordinates.
/// Coordinates outside `[0, width] × [0, height]` are clamped by
/// QuickDraw at draw time; the encoder emits them verbatim.
pub fn encode_pict_v2_with_clip(
    width: u32,
    height: u32,
    data: &[u8],
    pack: PackType,
    clip: [i16; 4],
) -> Result<Vec<u8>> {
    // Encode the base stream (includes everything up to and including
    // the headerOp stanza, then the DirectBitsRect pixel block).
    // We must insert the ClipRgn AFTER the v2 header stanza and BEFORE
    // the DirectBitsRect opcode.
    //
    // Strategy: build the v2 stream normally, then inject the ClipRgn
    // bytes at the known injection point (right after the 512+2+8+2+2+26
    // = 552 byte prefix).
    let base = encode_pict_v2(width, height, data, pack)?;

    // The ClipRgn belongs immediately after the headerOp 24-byte
    // payload, i.e. after offset:
    //   512 (stub) + 2 (picSize) + 8 (picFrame) + 2 (0x0011) + 2 (0x02FF)
    //   + 2 (0x0C00) + 24 (headerOp payload) = 552.
    const INJECT_OFFSET: usize = 552;
    if base.len() < INJECT_OFFSET {
        return Err(PictError::invalid(
            "encode: base stream shorter than expected — cannot inject ClipRgn",
        ));
    }

    let clip_bytes = build_clip_rgn_rect(clip[0], clip[1], clip[2], clip[3]);
    let mut out = Vec::with_capacity(base.len() + clip_bytes.len());
    out.extend_from_slice(&base[..INJECT_OFFSET]);
    out.extend_from_slice(&clip_bytes);
    out.extend_from_slice(&base[INJECT_OFFSET..]);
    Ok(out)
}

// ---------------------------------------------------------------------------
// Measurement helper (used by tests).
// ---------------------------------------------------------------------------

/// Compute the byte length of the pixel-data section only (everything
/// after the fixed PixMap header, before `OpEndPic`), for a given pack
/// type. Used in tests to assert byte-savings ratios.
///
/// Returns `(raw_bytes, packed_bytes)`.
pub fn pixel_data_sizes(width: u32, height: u32, data: &[u8], pack: PackType) -> (usize, usize) {
    let raw_size = width as usize * height as usize * 4;
    let packed_size = match pack {
        PackType::Raw => raw_size,
        PackType::Packed24 => width as usize * height as usize * 3,
        PackType::Rle16 => {
            let w = width as usize;
            let h = height as usize;
            let mut total = 0usize;
            for y in 0..h {
                let row = &data[y * w * 4..(y + 1) * w * 4];
                let mut row_u16: Vec<u16> = Vec::with_capacity(w);
                for px in row.chunks_exact(4) {
                    let r5 = (px[0] >> 3) as u16 & 0x1F;
                    let g5 = (px[1] >> 3) as u16 & 0x1F;
                    let b5 = (px[2] >> 3) as u16 & 0x1F;
                    row_u16.push(0x8000 | (r5 << 10) | (g5 << 5) | b5);
                }
                let enc = packbits::encode_u16(&row_u16);
                let prefix_len = if w * 2 > 250 { 2 } else { 1 };
                total += prefix_len + enc.len();
            }
            total
        }
        PackType::ComponentPackBits => {
            let w = width as usize;
            let h = height as usize;
            let mut total = 0usize;
            for y in 0..h {
                let row = &data[y * w * 4..(y + 1) * w * 4];
                let mut r = vec![0u8; w];
                let mut g = vec![0u8; w];
                let mut b = vec![0u8; w];
                for (x, px) in row.chunks_exact(4).enumerate() {
                    r[x] = px[0];
                    g[x] = px[1];
                    b[x] = px[2];
                }
                let rc = packbits::encode(&r);
                let gc = packbits::encode(&g);
                let bc = packbits::encode(&b);
                let plane_total = rc.len() + gc.len() + bc.len();
                // byteCount prefix overhead
                let prefix_len = if w * 4 > 250 { 2 } else { 1 };
                total += prefix_len + plane_total;
            }
            total
        }
    };
    (raw_size, packed_size)
}

// ---------------------------------------------------------------------------
// Internal helpers.
// ---------------------------------------------------------------------------

fn validate_dims(width: u32, height: u32, data: &[u8]) -> Result<()> {
    if width == 0 || height == 0 {
        return Err(PictError::invalid(
            "encode: width and height must be non-zero",
        ));
    }
    let expected = width as usize * height as usize * 4;
    if data.len() != expected {
        return Err(PictError::invalid(format!(
            "encode: data.len() = {} but width × height × 4 = {expected}",
            data.len()
        )));
    }
    Ok(())
}

#[inline]
fn write_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_be_bytes());
}

#[inline]
fn write_i16(out: &mut Vec<u8>, v: i16) {
    out.extend_from_slice(&v.to_be_bytes());
}

#[inline]
fn write_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_be_bytes());
}

// ---------------------------------------------------------------------------
// Unit tests.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decoder::parse_pict;

    // ---- basic rejection tests ----

    #[test]
    fn rejects_size_mismatch() {
        let err = encode_pict(2, 2, &[0u8; 8]).unwrap_err();
        assert!(matches!(err, PictError::InvalidData(_)));
    }

    #[test]
    fn rejects_zero_dim() {
        assert!(matches!(
            encode_pict(0, 1, &[]).unwrap_err(),
            PictError::InvalidData(_)
        ));
    }

    // ---- round-trip: packType 1 (raw) ----

    #[test]
    fn roundtrip_3x2_rgba_packtype1() {
        let width = 3u32;
        let height = 2u32;
        let rgba: Vec<u8> = (0..(width * height * 4)).map(|i| (i * 37) as u8).collect();
        let encoded = encode_pict(width, height, &rgba).expect("encode failed");
        let img = parse_pict(&encoded).expect("decode failed");
        assert_eq!(img.width, width);
        assert_eq!(img.height, height);
        for y in 0..height as usize {
            for x in 0..width as usize {
                let src = (y * width as usize + x) * 4;
                let dst = src;
                assert_eq!(img.data[dst], rgba[src], "R ({x},{y})");
                assert_eq!(img.data[dst + 1], rgba[src + 1], "G ({x},{y})");
                assert_eq!(img.data[dst + 2], rgba[src + 2], "B ({x},{y})");
                assert_eq!(img.data[dst + 3], 0xFF, "A ({x},{y})");
            }
        }
    }

    // ---- round-trip: packType 2 (24-bpp) ----

    #[test]
    fn roundtrip_4x4_packtype2() {
        let width = 4u32;
        let height = 4u32;
        let rgba: Vec<u8> = (0..(width * height * 4)).map(|i| i as u8).collect();
        let enc = encode_pict_v2(width, height, &rgba, PackType::Packed24).unwrap();
        let img = parse_pict(&enc).unwrap();
        assert_eq!(img.width, width);
        assert_eq!(img.height, height);
        for y in 0..height as usize {
            for x in 0..width as usize {
                let s = (y * width as usize + x) * 4;
                assert_eq!(img.data[s], rgba[s], "R ({x},{y})");
                assert_eq!(img.data[s + 1], rgba[s + 1], "G ({x},{y})");
                assert_eq!(img.data[s + 2], rgba[s + 2], "B ({x},{y})");
                assert_eq!(img.data[s + 3], 0xFF, "A ({x},{y})");
            }
        }
    }

    // ---- round-trip: packType 4 (component-separated PackBits) ----

    #[test]
    fn roundtrip_8x8_packtype4_runs() {
        // Solid-colour image: all channels run-compress perfectly.
        let width = 8u32;
        let height = 8u32;
        let mut rgba = vec![0u8; (width * height * 4) as usize];
        for y in 0..height as usize {
            for x in 0..width as usize {
                let off = (y * width as usize + x) * 4;
                rgba[off] = 0xAA;
                rgba[off + 1] = 0xBB;
                rgba[off + 2] = 0xCC;
                rgba[off + 3] = 0xFF;
            }
        }
        let enc = encode_pict_v2(width, height, &rgba, PackType::ComponentPackBits).unwrap();
        let img = parse_pict(&enc).unwrap();
        assert_eq!(img.width, width);
        assert_eq!(img.height, height);
        for y in 0..height as usize {
            for x in 0..width as usize {
                let off = (y * width as usize + x) * 4;
                assert_eq!(img.data[off], 0xAA, "R ({x},{y})");
                assert_eq!(img.data[off + 1], 0xBB, "G ({x},{y})");
                assert_eq!(img.data[off + 2], 0xCC, "B ({x},{y})");
                assert_eq!(img.data[off + 3], 0xFF, "A ({x},{y})");
            }
        }
    }

    #[test]
    fn roundtrip_8x8_packtype4_gradient() {
        // Gradient image: moderate compression.
        let width = 8u32;
        let height = 8u32;
        let mut rgba = vec![0u8; (width * height * 4) as usize];
        for y in 0..height as usize {
            for x in 0..width as usize {
                let off = (y * width as usize + x) * 4;
                rgba[off] = (x * 32) as u8;
                rgba[off + 1] = (y * 32) as u8;
                rgba[off + 2] = ((x + y) * 16) as u8;
                rgba[off + 3] = 0xFF;
            }
        }
        let enc = encode_pict_v2(width, height, &rgba, PackType::ComponentPackBits).unwrap();
        let img = parse_pict(&enc).unwrap();
        assert_eq!(img.width, width);
        assert_eq!(img.height, height);
        for y in 0..height as usize {
            for x in 0..width as usize {
                let off = (y * width as usize + x) * 4;
                assert_eq!(img.data[off], rgba[off], "R ({x},{y})");
                assert_eq!(img.data[off + 1], rgba[off + 1], "G ({x},{y})");
                assert_eq!(img.data[off + 2], rgba[off + 2], "B ({x},{y})");
                assert_eq!(img.data[off + 3], 0xFF, "A ({x},{y})");
            }
        }
    }

    // ---- byte-savings measurements ----

    #[test]
    fn packtype4_saves_bytes_vs_raw_for_solid() {
        // A solid-colour image should compress very well with packType 4.
        let width = 64u32;
        let height = 64u32;
        let rgba = vec![0xABu8; (width * height * 4) as usize];
        let (raw, packed) = pixel_data_sizes(width, height, &rgba, PackType::ComponentPackBits);
        // Each row: 3 planes × (1 flag + 1 byte) = 6 bytes packed vs
        // 64×4 = 256 raw. Expect at least 90 % saving.
        assert!(
            packed < raw / 10,
            "expected packed={packed} < raw/10={}, got no compression",
            raw / 10
        );
    }

    #[test]
    fn packtype2_saves_25pct_vs_raw() {
        let width = 64u32;
        let height = 64u32;
        let rgba = vec![0u8; (width * height * 4) as usize];
        let (raw, packed) = pixel_data_sizes(width, height, &rgba, PackType::Packed24);
        assert_eq!(
            packed * 4,
            raw * 3,
            "packType2 should be exactly 3/4 of raw"
        );
    }

    // ---- v1 encoder round-trip ----

    #[test]
    fn roundtrip_v1_4x4() {
        let width = 4u32;
        let height = 4u32;
        let rgba: Vec<u8> = (0..(width * height * 4)).map(|i| (i * 17) as u8).collect();
        let enc = encode_pict_v1(width, height, &rgba).unwrap();
        let img = parse_pict(&enc).unwrap();
        assert_eq!(img.width, width);
        assert_eq!(img.height, height);
        for y in 0..height as usize {
            for x in 0..width as usize {
                let off = (y * width as usize + x) * 4;
                assert_eq!(img.data[off], rgba[off], "R ({x},{y})");
                assert_eq!(img.data[off + 1], rgba[off + 1], "G ({x},{y})");
                assert_eq!(img.data[off + 2], rgba[off + 2], "B ({x},{y})");
                assert_eq!(img.data[off + 3], 0xFF, "A ({x},{y})");
            }
        }
    }

    #[test]
    fn v1_no_512_stub() {
        // v1 output must NOT begin with a 512-byte zero block.
        let rgba = vec![0u8; 4 * 4 * 4];
        let enc = encode_pict_v1(4, 4, &rgba).unwrap();
        // The first 2 bytes are picSize (0x0000), bytes 2..10 are
        // picFrame, byte 10..12 are 0x11 0x01 (v1 sentinel).
        assert_eq!(enc[10], 0x11);
        assert_eq!(enc[11], 0x01);
        // Total size must be much less than 512.
        assert!(
            enc.len() < 512,
            "v1 output should not include 512-byte stub"
        );
    }

    // ---- ClipRgn injection ----

    #[test]
    fn clip_rgn_decode_roundtrip() {
        // Encode with a clip region, verify the decode still produces
        // the correct image (the decoder currently ignores the clip mask
        // but must not return an error on the ClipRgn opcode).
        let width = 4u32;
        let height = 4u32;
        let rgba: Vec<u8> = (0..(width * height * 4)).map(|i| i as u8).collect();
        let enc =
            encode_pict_v2_with_clip(width, height, &rgba, PackType::Raw, [0, 0, 4, 4]).unwrap();
        let img = parse_pict(&enc).unwrap();
        assert_eq!(img.width, width);
        assert_eq!(img.height, height);
    }

    #[test]
    fn clip_rgn_bytes_correct() {
        // The ClipRgn opcode bytes (0x0001, rgnSize=10, bbox) must be
        // at the correct position inside the stream.
        let rgba = vec![0u8; 2 * 2 * 4];
        let enc = encode_pict_v2_with_clip(2, 2, &rgba, PackType::Raw, [0, 0, 2, 2]).unwrap();
        // ClipRgn starts at offset 552 (after stub+header+headerOp).
        let pos = 552usize;
        // opcode 0x0001.
        assert_eq!(enc[pos], 0x00);
        assert_eq!(enc[pos + 1], 0x01);
        // rgnSize = 10.
        assert_eq!(enc[pos + 2], 0x00);
        assert_eq!(enc[pos + 3], 0x0A);
    }

    // ---- round 5: build_direct_bits_rect_op layout ----

    #[test]
    fn build_direct_bits_rect_op_opcode_byte() {
        // Just verify the opening opcode bytes: 0x009A.
        let rgba = vec![0u8; 2 * 2 * 4];
        let bytes = build_direct_bits_rect_op(0, 0, 2, 2, &rgba, PackType::Raw).unwrap();
        assert_eq!(&bytes[0..2], &[0x00, 0x9A]);
    }

    #[test]
    fn build_direct_bits_rect_op_rejects_degenerate() {
        let rgba = vec![0u8; 0];
        assert!(matches!(
            build_direct_bits_rect_op(0, 0, 0, 0, &rgba, PackType::Raw).unwrap_err(),
            PictError::InvalidData(_)
        ));
    }

    #[test]
    fn build_direct_bits_rect_op_rejects_size_mismatch() {
        let rgba = vec![0u8; 5];
        assert!(matches!(
            build_direct_bits_rect_op(0, 0, 2, 2, &rgba, PackType::Raw).unwrap_err(),
            PictError::InvalidData(_)
        ));
    }

    // ---- round 5: 1-bpp BitMap encoders ----

    #[test]
    fn bits_rect_emits_bits_rect_opcode() {
        // The opcode at the start of the picture record body should be
        // 0x0090 (BitsRect), not 0x0098 / 0x009A.
        let rgba = vec![0xFFu8; 8 * 8 * 4];
        let enc = encode_pict_bits_rect(8, 8, &rgba).unwrap();
        // After stub(512) + picSize(2) + picFrame(8) + sentinel(2+2) +
        // headerOp(2) + 24-byte payload = 552, the next two bytes are
        // the first opcode word.
        let pos = 552usize;
        assert_eq!(enc[pos], 0x00, "high byte of BitsRect opcode");
        assert_eq!(enc[pos + 1], 0x90, "low byte of BitsRect opcode");
    }

    #[test]
    fn pack_bits_rect_emits_pack_bits_rect_opcode() {
        let rgba = vec![0xFFu8; 64 * 8 * 4]; // wide enough for RLE path
        let enc = encode_pict_pack_bits_rect(64, 8, &rgba).unwrap();
        let pos = 552usize;
        assert_eq!(enc[pos], 0x00, "high byte of PackBitsRect opcode");
        assert_eq!(enc[pos + 1], 0x98, "low byte of PackBitsRect opcode");
    }

    #[test]
    fn bits_rect_rejects_size_mismatch() {
        let err = encode_pict_bits_rect(2, 2, &[0u8; 7]).unwrap_err();
        assert!(matches!(err, PictError::InvalidData(_)));
    }

    // ---- round 5: v1 with PackType selector ----

    #[test]
    fn v1_with_packtype_emits_correct_pack_type_word() {
        // v1 PixMap header sits at: picSize(2) + picFrame(8) +
        // sentinel(2) + opcode(1) + baseAddr(4) + rowBytes(2) +
        // bounds(8) + pmVersion(2) = offset 29; packType is the next 2.
        let rgba = vec![0u8; 4 * 4 * 4];
        for &(pack, expected_word) in &[
            (PackType::Raw, 1u16),
            (PackType::Packed24, 2u16),
            (PackType::Rle16, 3u16),
            (PackType::ComponentPackBits, 4u16),
        ] {
            let enc = encode_pict_v1_with(4, 4, &rgba, pack).unwrap();
            let pack_word = u16::from_be_bytes([enc[29], enc[30]]);
            assert_eq!(pack_word, expected_word, "{pack:?}");
        }
    }
}
