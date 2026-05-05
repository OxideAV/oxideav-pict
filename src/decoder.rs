//! PICT decoder.
//!
//! Clean-room implementation of Inside Macintosh: Imaging With QuickDraw
//! (Apple, 1994). The relevant chapters are:
//!
//! * §A-3 ("Picture opcodes") — the opcode table for v1 (8-bit) and
//!   v2 (16-bit, word-aligned) streams.
//! * §A-5 ("The PackBits procedure") — RLE algorithm.
//! * §4 ("Color QuickDraw and PixMaps") — `PixMap` record layout.
//!
//! Round 1 walks the v2 opcode stream, skipping every opcode whose
//! semantic effect is purely drawing-state (pen / colour / font /
//! pattern), drawing-shape (lines / rects / polys / arcs / regions /
//! text), or comment. When the walk hits a `PackBitsRect` (`0x0098`)
//! or `DirectBitsRect` (`0x009A`) it decodes the PixMap into an RGBA
//! buffer and returns it. Subsequent raster opcodes are still parsed
//! (so the byte stream stays in sync) but only the *first* extracted
//! raster surfaces — multi-image PICT support is round-2.
//!
//! v1 PICTs are detected (sentinel `0x1101` at offset +10 from
//! body-start) but their raster opcodes return `Unsupported` for now;
//! the workspace's modern targets are all v2.

use crate::error::{PictError, Result};
use crate::image::{PictImage, PictPixelFormat};
use crate::opcodes::*;
use crate::packbits;
use crate::reader::Reader;

/// Decode a complete PICT byte stream and return the first raster
/// found via `PackBitsRect` / `DirectBitsRect`.
///
/// Accepts both forms produced by real-world generators:
///
/// * Raw PICT body — the 10-byte v1/v2 picture record header is at
///   offset 0.
/// * 512-byte launch-stub prefix + PICT body — Apple's pre-OS-X file-
///   manager habit. Detected by checking that offset 512 looks like a
///   plausible picture record (picSize then picFrame then the version
///   sentinel at +10) and the 0..512 prefix doesn't.
///
/// Returns `PictError::NoRaster` if the opcode stream terminates
/// (`OpEndPic`) or runs out of bytes without producing a raster.
pub fn parse_pict(bytes: &[u8]) -> Result<PictImage> {
    // PICT files commonly start with a 512-byte launch-stub prefix.
    // The body's picture-record format is identical with or without it,
    // so we just need to find the right starting offset.
    let body_offset = detect_body_offset(bytes)?;
    let body = &bytes[body_offset..];

    // Picture record: 2 bytes picSize, 8 bytes picFrame, then the
    // opcode stream. picSize is mostly ignored for modern files (real
    // size routinely exceeds 64 KB so the field is just `0x0000`).
    let mut r = Reader::new(body);
    let _pic_size = r.read_u16()?;
    let pic_frame = r.read_rect()?;

    // Version detection. v2 emits the 2-byte version opcode `0x0011`
    // followed by the 2-byte `0x02FF` v2 sentinel and a 2-byte
    // `0x0C00` headerOp opcode whose 24-byte operand we then skip.
    // v1 emits the 1-byte version opcode `0x11` followed by `0x01`.
    let version = detect_version(&mut r)?;

    match version {
        PictVersion::V1 => {
            // v1 raster opcodes are not in round 1. The stream is
            // recognised so callers can distinguish "this is genuinely
            // a v1 PICT — please ask for round 2" from "this is
            // garbage".
            Err(PictError::unsupported(
                "PICT v1 (8-bit opcodes) raster extraction deferred to round 2",
            ))
        }
        PictVersion::V2 => parse_v2_opcodes(&mut r, pic_frame),
    }
}

/// Picture version, as detected after the 10-byte picture-record
/// header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PictVersion {
    V1,
    V2,
}

/// Probe `bytes` for the most plausible PICT body start offset.
///
/// Real-world PICT files come in two shapes: with the 512-byte
/// pre-OS-X launch-stub prefix, or without. We try `0` first and only
/// fall back to `512` if `0` doesn't validate.
fn detect_body_offset(bytes: &[u8]) -> Result<usize> {
    if looks_like_picture_record(bytes) {
        return Ok(0);
    }
    if bytes.len() >= 512 + 12 && looks_like_picture_record(&bytes[512..]) {
        return Ok(512);
    }
    Err(PictError::invalid(
        "no PICT picture record at offset 0 or 512",
    ))
}

/// Cheap structural check: at offset 10 in a picture record, a v2
/// stream starts `0x00 0x11 0x02 0xFF`, a v1 stream starts `0x11 0x01`.
/// `bytes` must include offset 0..=11; we ignore the picSize field
/// (offsets 0..2) since it routinely lies in modern files.
fn looks_like_picture_record(bytes: &[u8]) -> bool {
    if bytes.len() < 12 {
        return false;
    }
    // v2 sentinel: `0x00 0x11 0x02 0xFF`
    if &bytes[10..14.min(bytes.len())] == [0x00, 0x11, 0x02, 0xFF].as_slice() {
        return true;
    }
    // v1 sentinel: `0x11 0x01`. Tolerate the high byte being 0 because
    // the version opcode is 1 byte in v1 — different generators pad
    // differently.
    if bytes[10] == 0x11 && bytes[11] == 0x01 {
        return true;
    }
    false
}

/// Read past the version-opcode + (for v2) the headerOp stanza, and
/// return which version we're on. On entry `r` is positioned right
/// after the 10-byte picture-record header (picSize + picFrame).
fn detect_version(r: &mut Reader<'_>) -> Result<PictVersion> {
    let v_op = r.read_u16()?;
    if v_op == 0x0011 {
        // Either v1 (next byte is 0x01 — but the v2 path ate the
        // upper-byte `0x00`, so the next two bytes determine version).
        let next = r.read_u16()?;
        if next == 0x02FF {
            // v2 picture. The next 2-byte opcode should be the
            // headerOp (0x0C00), followed by 24 bytes of header data:
            //
            //   2 bytes  headerVersion (-1 / -2 / 0xFFFE / 0xFFFF)
            //   2 bytes  reserved
            //   16 bytes hRes + vRes + srcRect (or fixed-point bounds)
            //   4 bytes  reserved
            //
            // Inside Macintosh §A-3 doesn't break the sub-fields out
            // for the round-1 reader; we just consume 26 bytes total
            // (2 opcode + 24 payload).
            let header_op = r.read_u16()?;
            if header_op != OP_HEADER_OP {
                return Err(PictError::invalid(format!(
                    "expected headerOp 0x0C00 after v2 sentinel, got 0x{header_op:04X}"
                )));
            }
            r.skip(24)?;
            return Ok(PictVersion::V2);
        }
        // v1 picture: the version opcode is one byte (`0x11`) followed
        // by a one-byte version (`0x01`). We read 0x0011 as a 16-bit
        // word, so the high byte was the opcode and the low byte was
        // already the version. But the low byte was `0x11`, not the
        // version — so we have to back up. Easier: detect v1 by the
        // raw byte pattern at offset 10 from the body start, which we
        // already did in `looks_like_picture_record`. Re-reading is
        // simpler than tracking pre-read state: rewind 2 bytes and
        // check.
        if (next >> 8) == 0x01 {
            // v1: opcode 0x11 (1 byte) + version 0x01 (1 byte) =
            // 2 bytes total. We just consumed both.
            return Ok(PictVersion::V1);
        }
        return Err(PictError::invalid(format!(
            "unrecognised version stanza after 0x0011: 0x{next:04X}"
        )));
    }
    // Some PICTs (rare) use 1-byte version opcode even at the v1/v2
    // discriminator. Reject — round 1 only sees the canonical forms.
    Err(PictError::invalid(format!(
        "expected version opcode 0x0011, got 0x{v_op:04X}"
    )))
}

/// Walk a v2 opcode stream and return the first PackBitsRect /
/// DirectBitsRect raster.
fn parse_v2_opcodes(r: &mut Reader<'_>, pic_frame: (i16, i16, i16, i16)) -> Result<PictImage> {
    while !r.at_eof() {
        // v2 opcodes are word-aligned. If the previous opcode left the
        // cursor at an odd offset, pad to the next word.
        r.align_word()?;
        if r.at_eof() {
            break;
        }

        let opcode = r.read_u16()?;
        // Fixed-size opcodes: skip a known number of bytes.
        if let Some(n) = fixed_operand_size(opcode) {
            r.skip(n)?;
            continue;
        }
        match opcode {
            OP_NOP => continue,
            OP_OP_END_PIC => break,
            OP_CLIP_RGN | OP_FRAME_RGN | OP_PAINT_RGN | OP_ERASE_RGN | OP_INVERT_RGN
            | OP_FILL_RGN => {
                // Region: 2-byte rgnSize (BE u16, includes the size
                // word itself), 8-byte rgnBBox, then optional inversion
                // data totalling rgnSize bytes.
                let rgn_size = r.read_u16()? as usize;
                if rgn_size < 2 {
                    return Err(PictError::invalid(format!(
                        "region size {rgn_size} smaller than the 2-byte size word"
                    )));
                }
                r.skip(rgn_size - 2)?;
            }
            OP_BK_PIX_PAT | OP_PN_PIX_PAT | OP_FILL_PIX_PAT => {
                // PixPat: variable; round 1 doesn't honour patterns,
                // and the FDIS doesn't include enough info to skip
                // safely without a partial PixMap parse. Reject.
                return Err(PictError::unsupported(
                    "PixPat opcodes (BkPixPat / PnPixPat / FillPixPat) need a PixMap-aware skip",
                ));
            }
            OP_FRAME_POLY | OP_PAINT_POLY | OP_ERASE_POLY | OP_INVERT_POLY | OP_FILL_POLY => {
                // Polygon: 2-byte polySize, 8-byte bounding rect, then
                // (polySize - 10)/4 pairs of (h, v) i16 vertices.
                let poly_size = r.read_u16()? as usize;
                if poly_size < 2 {
                    return Err(PictError::invalid(format!(
                        "polygon size {poly_size} smaller than the 2-byte size word"
                    )));
                }
                r.skip(poly_size - 2)?;
            }
            OP_LONG_TEXT => {
                // 4 bytes (txLoc point: v, h) + 1 byte count + count
                // bytes of MacRoman text.
                r.skip(4)?;
                let n = r.read_u8()? as usize;
                r.skip(n)?;
            }
            OP_DH_TEXT | OP_DV_TEXT => {
                // 1 byte dh-or-dv, 1 byte count, count bytes of text.
                r.skip(1)?;
                let n = r.read_u8()? as usize;
                r.skip(n)?;
            }
            OP_DHDV_TEXT => {
                // 1 byte dh + 1 byte dv + 1 byte count + count text bytes.
                r.skip(2)?;
                let n = r.read_u8()? as usize;
                r.skip(n)?;
            }
            OP_FONT_NAME => {
                // 2 bytes data-size (incl. the size word? per Inside
                // Macintosh: yes, the dataLength includes itself). We
                // therefore read it and skip dataLength bytes total
                // minus the 2 already consumed.
                let n = r.read_u16()? as usize;
                if n < 2 {
                    return Err(PictError::invalid(format!(
                        "fontName dataLength {n} smaller than the size word"
                    )));
                }
                r.skip(n - 2)?;
            }
            OP_LINE_JUSTIFY => {
                // 2 bytes dataSize (always 8 per spec), 8 bytes payload.
                let n = r.read_u16()? as usize;
                r.skip(n)?;
            }
            OP_GLYPH_STATE => {
                // 2 bytes dataSize, dataSize bytes payload.
                let n = r.read_u16()? as usize;
                r.skip(n)?;
            }
            OP_SHORT_COMMENT => {
                // 2 bytes kind, no payload.
                r.skip(2)?;
            }
            OP_LONG_COMMENT => {
                // 2 bytes kind, 2 bytes size, size bytes payload.
                r.skip(2)?;
                let n = r.read_u16()? as usize;
                r.skip(n)?;
            }
            OP_PACK_BITS_RECT => {
                let img = decode_pack_bits_rect(r)?;
                return Ok(img);
            }
            OP_DIRECT_BITS_RECT => {
                let img = decode_direct_bits_rect(r)?;
                return Ok(img);
            }
            OP_BITS_RECT
            | OP_BITS_RGN
            | OP_PACK_BITS_RGN
            | OP_DIRECT_BITS_RGN
            | OP_COMPRESSED_QUICKTIME
            | OP_UNCOMPRESSED_QUICKTIME => {
                return Err(PictError::unsupported(format!(
                    "raster opcode 0x{opcode:04X} not implemented in round 1"
                )));
            }
            _ => {
                return Err(PictError::unsupported(format!(
                    "unknown / unsupported v2 opcode 0x{opcode:04X} at offset {} \
                     (frame={pic_frame:?})",
                    r.pos - 2
                )));
            }
        }
    }
    Err(PictError::NoRaster)
}

/// Static operand size for opcodes whose payload is a fixed number of
/// bytes per Inside Macintosh §A-3. Returning `None` means "this
/// opcode has variable-size or zero-size operands; handle it in the
/// `match` arm".
fn fixed_operand_size(opcode: u16) -> Option<usize> {
    Some(match opcode {
        // No-op: 0 bytes.
        OP_BK_PAT => 8,          // 8-byte pattern
        OP_TX_FONT => 2,         // i16 font ID
        OP_TX_FACE => 1,         // u8 style; padded to word elsewhere
        OP_TX_MODE => 2,         // i16 transfer mode
        OP_SP_EXTRA => 4,        // Fixed (4 bytes)
        OP_PN_SIZE => 4,         // Point (h, v) i16 each
        OP_PN_MODE => 2,         // i16
        OP_PN_PAT => 8,          // 8-byte pattern
        OP_FILL_PAT => 8,        // 8-byte pattern
        OP_OV_SIZE => 4,         // Point
        OP_ORIGIN => 4,          // Point
        OP_TX_SIZE => 2,         // i16
        OP_FG_COLOR => 4,        // 32-bit Pascal Color value
        OP_BG_COLOR => 4,        // 32-bit Pascal Color value
        OP_TX_RATIO => 8,        // 2 Points
        OP_PN_LOC_HFRAC => 2,    // Fixed-point fraction
        OP_CH_EXTRA => 2,        // i16
        OP_RGB_FG_COL => 6,      // RGBColor (3× u16)
        OP_RGB_BK_COL => 6,      // RGBColor
        OP_HILITE_MODE => 0,     // No data — selects state
        OP_HILITE_COLOR => 6,    // RGBColor
        OP_DEF_HILITE => 0,      // No data
        OP_OP_COLOR => 6,        // RGBColor
        OP_LINE => 8,            // Two Points
        OP_LINE_FROM => 4,       // One Point
        OP_SHORT_LINE => 6,      // Point + 2-byte dh,dv
        OP_SHORT_LINE_FROM => 2, // 2-byte dh,dv
        OP_FRAME_RECT => 8,      // Rect
        OP_PAINT_RECT => 8,
        OP_ERASE_RECT => 8,
        OP_INVERT_RECT => 8,
        OP_FILL_RECT => 8,
        OP_FRAME_SAME_RECT => 0,
        OP_PAINT_SAME_RECT => 0,
        OP_ERASE_SAME_RECT => 0,
        OP_INVERT_SAME_RECT => 0,
        OP_FILL_SAME_RECT => 0,
        OP_FRAME_RRECT => 8,
        OP_PAINT_RRECT => 8,
        OP_ERASE_RRECT => 8,
        OP_INVERT_RRECT => 8,
        OP_FILL_RRECT => 8,
        OP_FRAME_SAME_RRECT => 0,
        OP_PAINT_SAME_RRECT => 0,
        OP_ERASE_SAME_RRECT => 0,
        OP_INVERT_SAME_RRECT => 0,
        OP_FILL_SAME_RRECT => 0,
        OP_FRAME_OVAL => 8,
        OP_PAINT_OVAL => 8,
        OP_ERASE_OVAL => 8,
        OP_INVERT_OVAL => 8,
        OP_FILL_OVAL => 8,
        OP_FRAME_SAME_OVAL => 0,
        OP_PAINT_SAME_OVAL => 0,
        OP_ERASE_SAME_OVAL => 0,
        OP_INVERT_SAME_OVAL => 0,
        OP_FILL_SAME_OVAL => 0,
        OP_FRAME_ARC => 12, // Rect + 2× i16 angles
        OP_PAINT_ARC => 12,
        OP_ERASE_ARC => 12,
        OP_INVERT_ARC => 12,
        OP_FILL_ARC => 12,
        OP_FRAME_SAME_ARC => 4, // 2× i16 angles only (rect repeated)
        OP_PAINT_SAME_ARC => 4,
        OP_ERASE_SAME_ARC => 4,
        OP_INVERT_SAME_ARC => 4,
        OP_FILL_SAME_ARC => 4,
        _ => return None,
    })
}

/// Decode a `PackBitsRect` (`0x0098`) into a [`PictImage`].
///
/// PackBitsRect carries a *BitMap* (the legacy 1-bit type), not a
/// PixMap. The layout is:
///
/// * 2 bytes rowBytes (top bit is *not* set — that distinguishes
///   BitMap from PixMap)
/// * 8 bytes bounds (i16 each)
/// * 8 bytes srcRect
/// * 8 bytes dstRect
/// * 2 bytes mode (transfer mode)
/// * Per-row: 1 or 2 bytes byteCount, then byteCount bytes of
///   PackBits-compressed data.
///
/// The byteCount is 1 byte if rowBytes < 8, 2 bytes otherwise.
/// PackBits is suppressed entirely (each row is raw, byteCount =
/// rowBytes) when rowBytes < 8.
fn decode_pack_bits_rect(r: &mut Reader<'_>) -> Result<PictImage> {
    let row_bytes_raw = r.read_u16()?;
    if row_bytes_raw & 0x8000 != 0 {
        return Err(PictError::invalid(
            "PackBitsRect rowBytes top bit is set (looks like a PixMap, not a BitMap)",
        ));
    }
    let row_bytes = row_bytes_raw as usize;
    let bounds = r.read_rect()?;
    let _src_rect = r.read_rect()?;
    let _dst_rect = r.read_rect()?;
    let _mode = r.read_u16()?;

    let width = (bounds.3 - bounds.1).max(0) as u32;
    let height = (bounds.2 - bounds.0).max(0) as u32;

    // Row data. PackBits is suppressed when rowBytes < 8: each row is
    // emitted raw without a byteCount header.
    let mut bitmap = vec![0u8; row_bytes * height as usize];
    if row_bytes < 8 {
        for y in 0..height as usize {
            let row = r.read_bytes(row_bytes)?;
            bitmap[y * row_bytes..(y + 1) * row_bytes].copy_from_slice(row);
        }
    } else {
        for y in 0..height as usize {
            let _byte_count = if row_bytes > 250 {
                r.read_u16()? as usize
            } else {
                r.read_u8()? as usize
            };
            let dst = &mut bitmap[y * row_bytes..(y + 1) * row_bytes];
            packbits::decode_into(r, dst)?;
        }
    }

    // Expand 1-bit -> RGBA. Bit 7 of byte 0 is the leftmost pixel
    // (Mac convention); 1 = black ink, 0 = white paper.
    let mut rgba = vec![0u8; (width * height * 4) as usize];
    for y in 0..height as usize {
        for x in 0..width as usize {
            let bit_index = x;
            let byte = bitmap[y * row_bytes + (bit_index >> 3)];
            let bit = (byte >> (7 - (bit_index & 7))) & 1;
            let v = if bit == 1 { 0x00 } else { 0xFF };
            let off = (y * width as usize + x) * 4;
            rgba[off] = v;
            rgba[off + 1] = v;
            rgba[off + 2] = v;
            rgba[off + 3] = 0xFF;
        }
    }

    Ok(PictImage {
        width,
        height,
        pixel_format: PictPixelFormat::Rgba,
        data: rgba,
        pts: None,
    })
}

/// Decode a `DirectBitsRect` (`0x009A`) into a [`PictImage`].
///
/// DirectBitsRect carries a full PixMap with direct-pixel data
/// (cmpCount = 3, cmpSize = 5 / 8). The on-disk layout is:
///
/// * 4 bytes baseAddr (always 0x000000FF placeholder per Apple TN
///   PT-39, but real-world generators emit 0; tolerate both)
/// * 2 bytes rowBytes (top bit set — flags this as a PixMap)
/// * 8 bytes bounds
/// * 2 bytes pmVersion
/// * 2 bytes packType
/// * 4 bytes packSize
/// * 4 bytes hRes
/// * 4 bytes vRes
/// * 2 bytes pixelType (`16` = RGBDirect)
/// * 2 bytes pixelSize (16, 24, or 32)
/// * 2 bytes cmpCount (3 for RGB direct, 4 for RGBA)
/// * 2 bytes cmpSize (5 for 16-bit pixels, 8 for 24/32-bit)
/// * 4 bytes planeBytes
/// * 4 bytes pmTable handle (ignored)
/// * 4 bytes pmReserved
/// * (no colour table — direct-pixel)
/// * srcRect (8 bytes), dstRect (8 bytes), mode (2 bytes)
/// * Per-row pixel data (PackBits-compressed if pixelSize > 8 and
///   packType is 0; for direct-pixel real-world files the per-row
///   layout depends on packType — round 1 supports packType 1
///   "uncompressed" only).
fn decode_direct_bits_rect(r: &mut Reader<'_>) -> Result<PictImage> {
    let _base_addr = r.read_u32()?;
    let row_bytes_raw = r.read_u16()?;
    if row_bytes_raw & 0x8000 == 0 {
        return Err(PictError::invalid(
            "DirectBitsRect rowBytes top bit is clear (looks like a BitMap, not a PixMap)",
        ));
    }
    let row_bytes = (row_bytes_raw & 0x3FFF) as usize;
    let bounds = r.read_rect()?;
    let _pm_version = r.read_u16()?;
    let pack_type = r.read_u16()?;
    let _pack_size = r.read_u32()?;
    let _h_res = r.read_u32()?;
    let _v_res = r.read_u32()?;
    let _pixel_type = r.read_u16()?;
    let pixel_size = r.read_u16()?;
    let cmp_count = r.read_u16()?;
    let cmp_size = r.read_u16()?;
    let _plane_bytes = r.read_u32()?;
    let _pm_table = r.read_u32()?;
    let _pm_reserved = r.read_u32()?;
    let _src_rect = r.read_rect()?;
    let _dst_rect = r.read_rect()?;
    let _mode = r.read_u16()?;

    let width = (bounds.3 - bounds.1).max(0) as u32;
    let height = (bounds.2 - bounds.0).max(0) as u32;

    if pack_type != 1 {
        return Err(PictError::unsupported(format!(
            "DirectBitsRect packType {pack_type} (only packType=1 uncompressed in round 1)"
        )));
    }

    // Uncompressed path. Row layout per pixelSize:
    //   16 bpp: A1R5G5B5 packed BE u16 per pixel (top bit = ignored).
    //   24 bpp: separated planes? No — packType=1 is interleaved BGR.
    //           cmpCount=3 cmpSize=8.
    //   32 bpp: interleaved 0xFF R G B ("XRGB"), with cmpCount=3,
    //           or alpha R G B with cmpCount=4. 4 bytes per pixel.
    let mut rgba = vec![0u8; (width * height * 4) as usize];
    match pixel_size {
        16 => {
            if !(cmp_count == 3 && cmp_size == 5) {
                return Err(PictError::unsupported(format!(
                    "DirectBitsRect 16bpp expects cmpCount=3 cmpSize=5, got {cmp_count}/{cmp_size}"
                )));
            }
            for y in 0..height as usize {
                let row = r.read_bytes(row_bytes)?;
                for x in 0..width as usize {
                    let p = u16::from_be_bytes([row[x * 2], row[x * 2 + 1]]);
                    // A1R5G5B5: bits 14..10 R, 9..5 G, 4..0 B. Expand
                    // 5-bit channels to 8-bit by replicating top bits.
                    let r5 = ((p >> 10) & 0x1F) as u8;
                    let g5 = ((p >> 5) & 0x1F) as u8;
                    let b5 = (p & 0x1F) as u8;
                    let off = (y * width as usize + x) * 4;
                    rgba[off] = (r5 << 3) | (r5 >> 2);
                    rgba[off + 1] = (g5 << 3) | (g5 >> 2);
                    rgba[off + 2] = (b5 << 3) | (b5 >> 2);
                    rgba[off + 3] = 0xFF;
                }
            }
        }
        32 => {
            if !(cmp_size == 8 && (cmp_count == 3 || cmp_count == 4)) {
                return Err(PictError::unsupported(format!(
                    "DirectBitsRect 32bpp expects cmpSize=8 cmpCount=3|4, got {cmp_count}/{cmp_size}"
                )));
            }
            // Disk layout for packType=1, pixelSize=32: 4 bytes per
            // pixel, interleaved 0xFF R G B (cmpCount=3) or A R G B
            // (cmpCount=4). Inside Macintosh §A-3 doesn't explicitly
            // pin packType=1 to interleaved; the round-1 reader
            // standardises on it. PackType 2 (interleaved 3 byte BGR)
            // and packType 3/4 (component-separated 16-bit RLE) are
            // round-2.
            for y in 0..height as usize {
                let row = r.read_bytes(row_bytes)?;
                for x in 0..width as usize {
                    let off_in = x * 4;
                    let off_out = (y * width as usize + x) * 4;
                    let (a, rr, gg, bb) = if cmp_count == 4 {
                        (
                            row[off_in],
                            row[off_in + 1],
                            row[off_in + 2],
                            row[off_in + 3],
                        )
                    } else {
                        (0xFF, row[off_in + 1], row[off_in + 2], row[off_in + 3])
                    };
                    rgba[off_out] = rr;
                    rgba[off_out + 1] = gg;
                    rgba[off_out + 2] = bb;
                    rgba[off_out + 3] = a;
                }
            }
        }
        _ => {
            return Err(PictError::unsupported(format!(
                "DirectBitsRect pixelSize {pixel_size} not in {{16, 32}}"
            )));
        }
    }

    Ok(PictImage {
        width,
        height,
        pixel_format: PictPixelFormat::Rgba,
        data: rgba,
        pts: None,
    })
}

// ---------------------------------------------------------------------------
// Registry-feature trait surface.
// ---------------------------------------------------------------------------

#[cfg(feature = "registry")]
use oxideav_core::Decoder;
#[cfg(feature = "registry")]
use oxideav_core::{CodecId, CodecParameters, Frame, Packet, VideoFrame, VideoPlane};

/// Factory registered with the codec registry. Consumes one packet
/// per whole PICT file and produces one frame.
#[cfg(feature = "registry")]
pub fn make_decoder(_params: &CodecParameters) -> oxideav_core::Result<Box<dyn Decoder>> {
    Ok(Box::new(PictDecoder {
        codec_id: CodecId::new(crate::CODEC_ID_STR),
        pending: None,
        eof: false,
    }))
}

#[cfg(feature = "registry")]
struct PictDecoder {
    codec_id: CodecId,
    pending: Option<VideoFrame>,
    eof: bool,
}

#[cfg(feature = "registry")]
impl Decoder for PictDecoder {
    fn codec_id(&self) -> &CodecId {
        &self.codec_id
    }
    fn send_packet(&mut self, packet: &Packet) -> oxideav_core::Result<()> {
        let image = parse_pict(&packet.data)?;
        self.pending = Some(image_to_video_frame(image));
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

#[cfg(feature = "registry")]
fn image_to_video_frame(image: PictImage) -> VideoFrame {
    let stride = image.stride();
    VideoFrame {
        pts: image.pts,
        planes: vec![VideoPlane {
            stride,
            data: image.data,
        }],
    }
}
