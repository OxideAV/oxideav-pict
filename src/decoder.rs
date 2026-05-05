//! PICT decoder.
//!
//! Clean-room implementation of Inside Macintosh: Imaging With QuickDraw
//! (Apple, 1994). The relevant chapters are:
//!
//! * §A-3 ("Picture opcodes") — the opcode table for v1 (8-bit) and
//!   v2 (16-bit, word-aligned) streams.
//! * §A-5 ("The PackBits procedure") — RLE algorithm.
//! * §4 ("Color QuickDraw and PixMaps") — `PixMap` record layout.
//! * §2 ("QuickDraw Drawing") — `Region` and drawing-verb semantics.
//!
//! Round 2 walks the v2 opcode stream, stepping a small drawing-state
//! machine ([`crate::state::PictState`]) and folding every drawing
//! command (line / rect / round-rect / oval / arc / poly / region /
//! raster) onto an in-crate software-rasteriser canvas
//! ([`crate::raster::Canvas`]). When the walk terminates (`OpEndPic`,
//! end-of-stream, or an unsupported opcode) the canvas — sized to
//! `picFrame` and pre-filled with the background colour (paper) — is
//! returned as the [`PictImage`]. PICTs that contain no drawing
//! commands at all return [`PictError::NoRaster`].
//!
//! v1 PICTs (8-bit opcodes) have basic raster + line/rect/region
//! support — same drawing-state model, smaller opcode roster.
//!
//! ## DirectBitsRect packType
//!
//! * `0` (none) and `1` (uncompressed): per-row pixel data laid out
//!   verbatim, `rowBytes` bytes long.
//! * `2`: same uncompressed data but with the per-pixel pad byte
//!   removed (32-bit pixels become 24 bytes per row × 3 byte
//!   stride).
//! * `3`: per-row 16-bit-pixel PackBits compression, where the unit
//!   replicated by run packets is a `u16` rather than a `u8`.
//! * `4`: per-row component-separated PackBits (R plane, G plane, B
//!   plane, optionally A plane) compression at 8-bit byte units.
//!
//! Byte-count prefix per row (1 byte if `rowBytes < 250`, else
//! 2 bytes) is shared between packTypes 3 and 4.

use crate::error::{PictError, Result};
use crate::image::{PictImage, PictPixelFormat};
use crate::opcodes::*;
use crate::packbits;
use crate::raster::{
    fill_arc, fill_oval, fill_polygon, fill_rect, fill_round_rect, frame_arc, frame_oval,
    frame_polygon, frame_rect, frame_round_rect, line as draw_line, Canvas,
};
use crate::reader::Reader;
use crate::region::{parse_region, Region};
use crate::state::{PictState, RectI32, Rgba};

/// Decode a complete PICT byte stream into a single rasterised
/// [`PictImage`].
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
/// Returns [`PictError::NoRaster`] if the opcode stream terminates
/// (`OpEndPic`) or runs out of bytes without producing any drawing
/// or raster output.
pub fn parse_pict(bytes: &[u8]) -> Result<PictImage> {
    let body_offset = detect_body_offset(bytes)?;
    let body = &bytes[body_offset..];

    let mut r = Reader::new(body);
    let _pic_size = r.read_u16()?;
    let pic_frame = r.read_rect()?;
    let pic_frame = RectI32::from_be(pic_frame.0, pic_frame.1, pic_frame.2, pic_frame.3);

    let version = detect_version(&mut r)?;

    // Initial canvas sized to the picture frame, pre-filled white
    // (QuickDraw "paper"). The drawing-state machine adjusts the
    // origin so we always plot in canvas-local 0..width, 0..height
    // coordinates.
    let width = (pic_frame.right - pic_frame.left).max(0) as u32;
    let height = (pic_frame.bottom - pic_frame.top).max(0) as u32;
    if width == 0 || height == 0 {
        return Err(PictError::invalid(format!(
            "picFrame degenerate: {:?}",
            (
                pic_frame.top,
                pic_frame.left,
                pic_frame.bottom,
                pic_frame.right
            )
        )));
    }
    let canvas = Canvas::new(width, height, Rgba::WHITE);
    let state = PictState {
        // Origin shifts so picFrame.top/left maps to canvas (0, 0).
        origin: (pic_frame.left, pic_frame.top),
        ..PictState::default()
    };

    match version {
        PictVersion::V1 => parse_v1_opcodes(&mut r, pic_frame, canvas, state),
        PictVersion::V2 => parse_v2_opcodes(&mut r, pic_frame, canvas, state),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PictVersion {
    V1,
    V2,
}

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

fn looks_like_picture_record(bytes: &[u8]) -> bool {
    if bytes.len() < 12 {
        return false;
    }
    if &bytes[10..14.min(bytes.len())] == [0x00, 0x11, 0x02, 0xFF].as_slice() {
        return true;
    }
    if bytes[10] == 0x11 && bytes[11] == 0x01 {
        return true;
    }
    false
}

fn detect_version(r: &mut Reader<'_>) -> Result<PictVersion> {
    // The version stanza is the first thing after the 10-byte
    // picture-record header. v2 emits the 2-byte opcode 0x0011
    // followed by the 2-byte 0x02FF v2 sentinel and the headerOp
    // stanza. v1 emits the 1-byte opcode 0x11 followed by the
    // 1-byte version 0x01 — so reading 2 bytes gives 0x1101.
    let v_word = r.read_u16()?;
    if v_word == 0x0011 {
        let next = r.read_u16()?;
        if next == 0x02FF {
            let header_op = r.read_u16()?;
            if header_op != OP_HEADER_OP {
                return Err(PictError::invalid(format!(
                    "expected headerOp 0x0C00 after v2 sentinel, got 0x{header_op:04X}"
                )));
            }
            r.skip(24)?;
            return Ok(PictVersion::V2);
        }
        // Some pre-v2 generators pad the version opcode out to 2 bytes
        // (`0x0011`) followed by a 1-byte version `0x01` then v1
        // opcodes. The `next` word's high byte is then 0x01 — we
        // already consumed it as part of the second read, so the
        // first v1 opcode is in the low byte.
        if (next >> 8) == 0x01 {
            // We have to back up one byte: the low byte of `next` is
            // the first v1 opcode.
            r.pos -= 1;
            return Ok(PictVersion::V1);
        }
        return Err(PictError::invalid(format!(
            "unrecognised version stanza after 0x0011: 0x{next:04X}"
        )));
    }
    if v_word == 0x1101 {
        // Canonical v1 form: 1-byte opcode 0x11 then 1-byte version
        // 0x01. Both bytes consumed; the next byte is the first v1
        // opcode.
        return Ok(PictVersion::V1);
    }
    Err(PictError::invalid(format!(
        "expected version opcode 0x0011 or 0x1101, got 0x{v_word:04X}"
    )))
}

/// Translate a picture-frame coordinate to canvas-local coordinates.
fn to_canvas(state: &PictState, x: i32, y: i32) -> (i32, i32) {
    (x - state.origin.0, y - state.origin.1)
}

/// Translate a picture-frame rectangle to canvas-local coords.
fn rect_to_canvas(state: &PictState, rect: RectI32) -> (i32, i32, i32, i32) {
    let (x0, y0) = to_canvas(state, rect.left, rect.top);
    let (x1, y1) = to_canvas(state, rect.right, rect.bottom);
    (y0, x0, y1, x1) // (top, left, bottom, right) for raster fns
}

/// Walk a v2 opcode stream, fold every command into the canvas, and
/// return the result.
fn parse_v2_opcodes(
    r: &mut Reader<'_>,
    pic_frame: RectI32,
    mut canvas: Canvas,
    mut state: PictState,
) -> Result<PictImage> {
    while !r.at_eof() {
        r.align_word()?;
        if r.at_eof() {
            break;
        }
        let opcode = r.read_u16()?;
        if !dispatch_v2_opcode(r, opcode, &pic_frame, &mut canvas, &mut state)? {
            break; // OpEndPic
        }
    }
    finalise_canvas(canvas, &state)
}

/// One v2 opcode dispatch. Returns `Ok(false)` only on `OpEndPic` —
/// the caller breaks out of the walk.
fn dispatch_v2_opcode(
    r: &mut Reader<'_>,
    opcode: u16,
    pic_frame: &RectI32,
    canvas: &mut Canvas,
    state: &mut PictState,
) -> Result<bool> {
    match opcode {
        OP_NOP => Ok(true),
        OP_OP_END_PIC => Ok(false),
        OP_CLIP_RGN => {
            // Clip region: parse + discard for now (round 2 doesn't
            // honour clipping on subsequent draws — we record the
            // bbox so future rounds can without rejigging the parser).
            let _rgn = parse_region(r)?;
            Ok(true)
        }
        OP_FRAME_RGN => {
            let rgn = parse_region(r)?;
            paint_region_outline(canvas, state, &rgn);
            Ok(true)
        }
        OP_PAINT_RGN | OP_FILL_RGN => {
            let rgn = parse_region(r)?;
            paint_region_filled(canvas, state, &rgn, state.fg);
            Ok(true)
        }
        OP_ERASE_RGN => {
            let rgn = parse_region(r)?;
            paint_region_filled(canvas, state, &rgn, state.bg);
            Ok(true)
        }
        OP_INVERT_RGN => {
            let rgn = parse_region(r)?;
            invert_region(canvas, state, &rgn);
            Ok(true)
        }
        // Drawing-state opcodes that change `state` (we keep a running
        // copy so subsequent geometry uses the right colour / pen).
        OP_RGB_FG_COL => {
            let (rr, gg, bb) = read_rgb16(r)?;
            state.fg = Rgba::from_rgb16(rr, gg, bb);
            Ok(true)
        }
        OP_RGB_BK_COL => {
            let (rr, gg, bb) = read_rgb16(r)?;
            state.bg = Rgba::from_rgb16(rr, gg, bb);
            Ok(true)
        }
        OP_FG_COLOR => {
            let code = r.read_u32()?;
            state.fg = Rgba::from_pascal_colour(code);
            Ok(true)
        }
        OP_BG_COLOR => {
            let code = r.read_u32()?;
            state.bg = Rgba::from_pascal_colour(code);
            Ok(true)
        }
        OP_PN_SIZE => {
            let v = r.read_i16()?;
            let h = r.read_i16()?;
            state.pen_size = (h as i32, v as i32);
            Ok(true)
        }
        OP_OV_SIZE => {
            let v = r.read_i16()?;
            let h = r.read_i16()?;
            state.oval_size = (h as i32, v as i32);
            Ok(true)
        }
        OP_ORIGIN => {
            // Per Inside Macintosh, Origin shifts the coordinate
            // system: subsequent draws at the same picture-frame
            // coords land at a different spot. We track the running
            // origin shift; the (dh, dv) in the opcode is added.
            let dh = r.read_i16()?;
            let dv = r.read_i16()?;
            state.origin.0 -= dh as i32;
            state.origin.1 -= dv as i32;
            Ok(true)
        }
        // Pen-position-affecting ops.
        OP_LINE => {
            let pt0_v = r.read_i16()? as i32;
            let pt0_h = r.read_i16()? as i32;
            let pt1_v = r.read_i16()? as i32;
            let pt1_h = r.read_i16()? as i32;
            let (x0, y0) = to_canvas(state, pt0_h, pt0_v);
            let (x1, y1) = to_canvas(state, pt1_h, pt1_v);
            draw_line(canvas, x0, y0, x1, y1, state.fg);
            state.pen = (pt1_h, pt1_v);
            Ok(true)
        }
        OP_LINE_FROM => {
            let pt_v = r.read_i16()? as i32;
            let pt_h = r.read_i16()? as i32;
            let (x0, y0) = to_canvas(state, state.pen.0, state.pen.1);
            let (x1, y1) = to_canvas(state, pt_h, pt_v);
            draw_line(canvas, x0, y0, x1, y1, state.fg);
            state.pen = (pt_h, pt_v);
            Ok(true)
        }
        OP_SHORT_LINE => {
            let pt_v = r.read_i16()? as i32;
            let pt_h = r.read_i16()? as i32;
            let dh = r.read_u8()? as i8 as i32;
            let dv = r.read_u8()? as i8 as i32;
            let nx = pt_h + dh;
            let ny = pt_v + dv;
            let (x0, y0) = to_canvas(state, pt_h, pt_v);
            let (x1, y1) = to_canvas(state, nx, ny);
            draw_line(canvas, x0, y0, x1, y1, state.fg);
            state.pen = (nx, ny);
            Ok(true)
        }
        OP_SHORT_LINE_FROM => {
            let dh = r.read_u8()? as i8 as i32;
            let dv = r.read_u8()? as i8 as i32;
            let nx = state.pen.0 + dh;
            let ny = state.pen.1 + dv;
            let (x0, y0) = to_canvas(state, state.pen.0, state.pen.1);
            let (x1, y1) = to_canvas(state, nx, ny);
            draw_line(canvas, x0, y0, x1, y1, state.fg);
            state.pen = (nx, ny);
            Ok(true)
        }
        // Rect verbs.
        OP_FRAME_RECT | OP_PAINT_RECT | OP_ERASE_RECT | OP_INVERT_RECT | OP_FILL_RECT => {
            let rect = read_rect_op(r)?;
            state.last_rect = Some(rect);
            apply_rect_verb(canvas, state, opcode, rect);
            Ok(true)
        }
        OP_FRAME_SAME_RECT | OP_PAINT_SAME_RECT | OP_ERASE_SAME_RECT | OP_INVERT_SAME_RECT
        | OP_FILL_SAME_RECT => {
            if let Some(rect) = state.last_rect {
                apply_rect_verb(canvas, state, opcode - 8, rect);
            }
            Ok(true)
        }
        OP_FRAME_RRECT | OP_PAINT_RRECT | OP_ERASE_RRECT | OP_INVERT_RRECT | OP_FILL_RRECT => {
            let rect = read_rect_op(r)?;
            state.last_rrect = Some(rect);
            apply_rrect_verb(canvas, state, opcode, rect);
            Ok(true)
        }
        OP_FRAME_SAME_RRECT | OP_PAINT_SAME_RRECT | OP_ERASE_SAME_RRECT | OP_INVERT_SAME_RRECT
        | OP_FILL_SAME_RRECT => {
            if let Some(rect) = state.last_rrect {
                apply_rrect_verb(canvas, state, opcode - 8, rect);
            }
            Ok(true)
        }
        OP_FRAME_OVAL | OP_PAINT_OVAL | OP_ERASE_OVAL | OP_INVERT_OVAL | OP_FILL_OVAL => {
            let rect = read_rect_op(r)?;
            state.last_oval = Some(rect);
            apply_oval_verb(canvas, state, opcode, rect);
            Ok(true)
        }
        OP_FRAME_SAME_OVAL | OP_PAINT_SAME_OVAL | OP_ERASE_SAME_OVAL | OP_INVERT_SAME_OVAL
        | OP_FILL_SAME_OVAL => {
            if let Some(rect) = state.last_oval {
                apply_oval_verb(canvas, state, opcode - 8, rect);
            }
            Ok(true)
        }
        OP_FRAME_ARC | OP_PAINT_ARC | OP_ERASE_ARC | OP_INVERT_ARC | OP_FILL_ARC => {
            let rect = read_rect_op(r)?;
            let start = r.read_i16()? as i32;
            let arc = r.read_i16()? as i32;
            state.last_arc_rect = Some(rect);
            apply_arc_verb(canvas, state, opcode, rect, start, arc);
            Ok(true)
        }
        OP_FRAME_SAME_ARC | OP_PAINT_SAME_ARC | OP_ERASE_SAME_ARC | OP_INVERT_SAME_ARC
        | OP_FILL_SAME_ARC => {
            let start = r.read_i16()? as i32;
            let arc = r.read_i16()? as i32;
            if let Some(rect) = state.last_arc_rect {
                apply_arc_verb(canvas, state, opcode - 8, rect, start, arc);
            }
            Ok(true)
        }
        // Polygon verbs: 2-byte size, 8-byte bounding rect, then
        // (size - 10)/4 (h, v) pairs.
        OP_FRAME_POLY | OP_PAINT_POLY | OP_ERASE_POLY | OP_INVERT_POLY | OP_FILL_POLY => {
            let poly_size = r.read_u16()? as usize;
            if poly_size < 10 {
                return Err(PictError::invalid(format!(
                    "polygon size {poly_size} smaller than the 10-byte header"
                )));
            }
            let _bbox = r.read_rect()?;
            let n_verts = (poly_size - 10) / 4;
            let mut verts = Vec::with_capacity(n_verts);
            for _ in 0..n_verts {
                let v = r.read_i16()? as i32;
                let h = r.read_i16()? as i32;
                let (cx, cy) = to_canvas(state, h, v);
                verts.push((cx, cy));
            }
            apply_poly_verb(canvas, state, opcode, &verts);
            Ok(true)
        }
        OP_LONG_TEXT => {
            // Text glyphs: round 2 doesn't have a font rasteriser
            // (would require a TrueType engine), so we skip silently.
            // This keeps the opcode walker in sync without wedging
            // the decoder when a PICT just has a label on it.
            r.skip(4)?;
            let n = r.read_u8()? as usize;
            r.skip(n)?;
            Ok(true)
        }
        OP_DH_TEXT | OP_DV_TEXT => {
            r.skip(1)?;
            let n = r.read_u8()? as usize;
            r.skip(n)?;
            Ok(true)
        }
        OP_DHDV_TEXT => {
            r.skip(2)?;
            let n = r.read_u8()? as usize;
            r.skip(n)?;
            Ok(true)
        }
        OP_FONT_NAME => {
            let n = r.read_u16()? as usize;
            if n < 2 {
                return Err(PictError::invalid(format!(
                    "fontName dataLength {n} smaller than the size word"
                )));
            }
            r.skip(n - 2)?;
            Ok(true)
        }
        OP_LINE_JUSTIFY | OP_GLYPH_STATE => {
            let n = r.read_u16()? as usize;
            r.skip(n)?;
            Ok(true)
        }
        OP_BK_PIX_PAT | OP_PN_PIX_PAT | OP_FILL_PIX_PAT => {
            // PixPat is variable-size and needs a partial PixMap
            // parse to skip safely. We don't implement patterns so
            // this stays Unsupported for now.
            Err(PictError::unsupported(
                "PixPat opcodes (BkPixPat / PnPixPat / FillPixPat) need a PixMap-aware skip",
            ))
        }
        OP_SHORT_COMMENT => {
            r.skip(2)?;
            Ok(true)
        }
        OP_LONG_COMMENT => {
            r.skip(2)?;
            let n = r.read_u16()? as usize;
            r.skip(n)?;
            Ok(true)
        }
        OP_PACK_BITS_RECT => {
            let (img, dst) = decode_pack_bits_rect(r)?;
            blit_subimage(canvas, state, &img, &dst);
            Ok(true)
        }
        OP_DIRECT_BITS_RECT => {
            let (img, dst) = decode_direct_bits_rect(r)?;
            blit_subimage(canvas, state, &img, &dst);
            Ok(true)
        }
        OP_PACK_BITS_RGN => {
            let (img, dst, _rgn) = decode_pack_bits_rgn(r)?;
            blit_subimage(canvas, state, &img, &dst);
            Ok(true)
        }
        OP_DIRECT_BITS_RGN => {
            let (img, dst, _rgn) = decode_direct_bits_rgn(r)?;
            blit_subimage(canvas, state, &img, &dst);
            Ok(true)
        }
        OP_BITS_RECT | OP_BITS_RGN => {
            // v2 BitsRect / BitsRgn — uncompressed BitMap (no
            // PackBits per row). Layout is identical to PackBitsRect
            // but with raw row data for each scan line.
            let (img, dst) = decode_bits_rect_v2(r, opcode == OP_BITS_RGN)?;
            blit_subimage(canvas, state, &img, &dst);
            Ok(true)
        }
        OP_COMPRESSED_QUICKTIME => {
            // Embedded QuickTime image (typically JPEG). The opcode
            // payload starts with a 4-byte total-payload size, then
            // the QTM matte / mask records, then the actual ImageDesc
            // + image data. Decoding the embedded JPEG would need
            // mjpeg's `decode_jpeg` exposed publicly; for round 2 we
            // parse the size + skip the payload so the surrounding
            // PICT decode keeps going.
            let payload_size = r.read_u32()? as usize;
            // The 4 bytes we just read are NOT included in
            // payload_size (they're the size word itself), so skip
            // payload_size more bytes.
            r.skip(payload_size.saturating_sub(4))?;
            Ok(true)
        }
        OP_UNCOMPRESSED_QUICKTIME => {
            let payload_size = r.read_u32()? as usize;
            r.skip(payload_size.saturating_sub(4))?;
            Ok(true)
        }
        _ => {
            // Fixed-size opcodes table covers everything from §A-3
            // we don't have a dedicated arm for.
            if let Some(n) = fixed_operand_size(opcode) {
                r.skip(n)?;
                Ok(true)
            } else {
                Err(PictError::unsupported(format!(
                    "unknown / unsupported v2 opcode 0x{opcode:04X} at offset {} \
                     (frame={pic_frame:?})",
                    r.pos - 2
                )))
            }
        }
    }
}

/// Static operand size for opcodes whose payload is a fixed number of
/// bytes per Inside Macintosh §A-3. Returning `None` means the opcode
/// has variable-size operands — those have their own match arm in
/// [`dispatch_v2_opcode`].
fn fixed_operand_size(opcode: u16) -> Option<usize> {
    Some(match opcode {
        OP_BK_PAT => 8,
        OP_TX_FONT => 2,
        OP_TX_FACE => 1,
        OP_TX_MODE => 2,
        OP_SP_EXTRA => 4,
        OP_PN_MODE => 2,
        OP_PN_PAT => 8,
        OP_FILL_PAT => 8,
        OP_TX_SIZE => 2,
        OP_TX_RATIO => 8,
        OP_PN_LOC_HFRAC => 2,
        OP_CH_EXTRA => 2,
        OP_HILITE_MODE => 0,
        OP_HILITE_COLOR => 6,
        OP_DEF_HILITE => 0,
        OP_OP_COLOR => 6,
        _ => return None,
    })
}

fn read_rect_op(r: &mut Reader<'_>) -> Result<RectI32> {
    let (top, left, bottom, right) = r.read_rect()?;
    Ok(RectI32::from_be(top, left, bottom, right))
}

fn read_rgb16(r: &mut Reader<'_>) -> Result<(u16, u16, u16)> {
    let r_ = r.read_u16()?;
    let g_ = r.read_u16()?;
    let b_ = r.read_u16()?;
    Ok((r_, g_, b_))
}

/// Apply a `frame|paint|erase|invert|fill Rect` opcode (`opcode` ∈
/// `0x30..=0x34`) to the canvas.
fn apply_rect_verb(canvas: &mut Canvas, state: &PictState, opcode: u16, rect: RectI32) {
    let (top, left, bottom, right) = rect_to_canvas(state, rect);
    match opcode & 0x000F {
        0 => frame_rect(canvas, top, left, bottom, right, state.fg),
        1 => fill_rect(canvas, top, left, bottom, right, state.fg),
        2 => fill_rect(canvas, top, left, bottom, right, state.bg),
        3 => invert_rect(canvas, top, left, bottom, right),
        4 => fill_rect(canvas, top, left, bottom, right, state.fg),
        _ => {}
    }
}

fn apply_rrect_verb(canvas: &mut Canvas, state: &PictState, opcode: u16, rect: RectI32) {
    let (top, left, bottom, right) = rect_to_canvas(state, rect);
    let (ow, oh) = state.oval_size;
    match opcode & 0x000F {
        0 => frame_round_rect(canvas, top, left, bottom, right, ow, oh, state.fg),
        1 => fill_round_rect(canvas, top, left, bottom, right, ow, oh, state.fg),
        2 => fill_round_rect(canvas, top, left, bottom, right, ow, oh, state.bg),
        3 => {
            // Approximate invert as "frame for outline + nothing for
            // interior" — true invert needs per-pixel toggle and we
            // don't have an alpha pipeline. Round 2: just frame.
            frame_round_rect(canvas, top, left, bottom, right, ow, oh, state.fg)
        }
        4 => fill_round_rect(canvas, top, left, bottom, right, ow, oh, state.fg),
        _ => {}
    }
}

fn apply_oval_verb(canvas: &mut Canvas, state: &PictState, opcode: u16, rect: RectI32) {
    let (top, left, bottom, right) = rect_to_canvas(state, rect);
    match opcode & 0x000F {
        0 => frame_oval(canvas, top, left, bottom, right, state.fg),
        1 => fill_oval(canvas, top, left, bottom, right, state.fg),
        2 => fill_oval(canvas, top, left, bottom, right, state.bg),
        3 => frame_oval(canvas, top, left, bottom, right, state.fg),
        4 => fill_oval(canvas, top, left, bottom, right, state.fg),
        _ => {}
    }
}

fn apply_arc_verb(
    canvas: &mut Canvas,
    state: &PictState,
    opcode: u16,
    rect: RectI32,
    start: i32,
    arc: i32,
) {
    let (top, left, bottom, right) = rect_to_canvas(state, rect);
    match opcode & 0x000F {
        0 => frame_arc(canvas, top, left, bottom, right, start, arc, state.fg),
        1 => fill_arc(canvas, top, left, bottom, right, start, arc, state.fg),
        2 => fill_arc(canvas, top, left, bottom, right, start, arc, state.bg),
        3 => frame_arc(canvas, top, left, bottom, right, start, arc, state.fg),
        4 => fill_arc(canvas, top, left, bottom, right, start, arc, state.fg),
        _ => {}
    }
}

fn apply_poly_verb(canvas: &mut Canvas, state: &PictState, opcode: u16, verts: &[(i32, i32)]) {
    match opcode & 0x000F {
        0 => frame_polygon(canvas, verts, state.fg),
        1 => fill_polygon(canvas, verts, state.fg),
        2 => fill_polygon(canvas, verts, state.bg),
        3 => frame_polygon(canvas, verts, state.fg),
        4 => fill_polygon(canvas, verts, state.fg),
        _ => {}
    }
}

/// Trivial XOR invert of an RGBA rectangle. Each channel is bit-
/// inverted; alpha is preserved.
fn invert_rect(canvas: &mut Canvas, top: i32, left: i32, bottom: i32, right: i32) {
    if right <= left || bottom <= top {
        return;
    }
    for y in top..bottom {
        if y < 0 || (y as u32) >= canvas.height {
            continue;
        }
        for x in left..right {
            if x < 0 || (x as u32) >= canvas.width {
                continue;
            }
            let off = ((y as u32 * canvas.width + x as u32) * 4) as usize;
            canvas.data[off] = !canvas.data[off];
            canvas.data[off + 1] = !canvas.data[off + 1];
            canvas.data[off + 2] = !canvas.data[off + 2];
        }
    }
    canvas.dirty = true;
}

/// Paint a region's interior with `colour`.
fn paint_region_filled(canvas: &mut Canvas, state: &PictState, rgn: &Region, colour: Rgba) {
    let bb_w = rgn.width().max(0);
    let bb_h = rgn.height().max(0);
    if bb_w == 0 || bb_h == 0 {
        return;
    }
    if rgn.mask.is_none() {
        // Pure rectangular region — equivalent to fill_rect on the
        // bbox.
        let (top, left, bottom, right) = rect_to_canvas(state, rgn.bbox);
        fill_rect(canvas, top, left, bottom, right, colour);
        return;
    }
    for y in rgn.bbox.top..rgn.bbox.bottom {
        for x in rgn.bbox.left..rgn.bbox.right {
            if rgn.contains(x, y) {
                let (cx, cy) = to_canvas(state, x, y);
                canvas.put(cx, cy, colour);
            }
        }
    }
}

/// Paint a region's outline (edges between inside / outside) with
/// the foreground colour.
fn paint_region_outline(canvas: &mut Canvas, state: &PictState, rgn: &Region) {
    let bb_w = rgn.width().max(0);
    let bb_h = rgn.height().max(0);
    if bb_w == 0 || bb_h == 0 {
        return;
    }
    if rgn.mask.is_none() {
        let (top, left, bottom, right) = rect_to_canvas(state, rgn.bbox);
        frame_rect(canvas, top, left, bottom, right, state.fg);
        return;
    }
    // Edge: pixel inside region with at least one outside neighbour.
    for y in rgn.bbox.top..rgn.bbox.bottom {
        for x in rgn.bbox.left..rgn.bbox.right {
            if !rgn.contains(x, y) {
                continue;
            }
            let n_outside = !rgn.contains(x, y - 1)
                || !rgn.contains(x, y + 1)
                || !rgn.contains(x - 1, y)
                || !rgn.contains(x + 1, y);
            if n_outside {
                let (cx, cy) = to_canvas(state, x, y);
                canvas.put(cx, cy, state.fg);
            }
        }
    }
}

fn invert_region(canvas: &mut Canvas, state: &PictState, rgn: &Region) {
    for y in rgn.bbox.top..rgn.bbox.bottom {
        for x in rgn.bbox.left..rgn.bbox.right {
            if !rgn.contains(x, y) {
                continue;
            }
            let (cx, cy) = to_canvas(state, x, y);
            if cx < 0 || cy < 0 || (cx as u32) >= canvas.width || (cy as u32) >= canvas.height {
                continue;
            }
            let off = ((cy as u32 * canvas.width + cx as u32) * 4) as usize;
            canvas.data[off] = !canvas.data[off];
            canvas.data[off + 1] = !canvas.data[off + 1];
            canvas.data[off + 2] = !canvas.data[off + 2];
            canvas.dirty = true;
        }
    }
}

/// One RGBA sub-image with its destination rectangle. Returned by
/// the various `decode_*_rect` raster opcode handlers so the canvas
/// blit happens in one place.
struct RasterSub {
    width: u32,
    height: u32,
    data: Vec<u8>,
}

fn blit_subimage(canvas: &mut Canvas, state: &PictState, img: &RasterSub, dst: &RectI32) {
    let (top, left, bottom, right) = rect_to_canvas(state, *dst);
    canvas.blit(&img.data, img.width, img.height, top, left, bottom, right);
}

/// Final canvas → PictImage. Returns NoRaster if nothing was drawn.
fn finalise_canvas(canvas: Canvas, _state: &PictState) -> Result<PictImage> {
    if !canvas.dirty {
        return Err(PictError::NoRaster);
    }
    Ok(PictImage {
        width: canvas.width,
        height: canvas.height,
        pixel_format: PictPixelFormat::Rgba,
        data: canvas.data,
        pts: None,
    })
}

// ---------------------------------------------------------------------------
// Raster opcode decoders. Each returns `(RasterSub, dstRect)` so the
// canvas blit happens in one place.
// ---------------------------------------------------------------------------

/// `PackBitsRect` (`0x0098`).
///
/// Carries a *BitMap* (1-bit). Layout per Inside Macintosh §A-3:
///   2 bytes rowBytes (top bit clear)
///   8 bytes bounds
///   8 bytes srcRect
///   8 bytes dstRect
///   2 bytes mode
///   per-row data (raw if rowBytes < 8, else byteCount + PackBits).
fn decode_pack_bits_rect(r: &mut Reader<'_>) -> Result<(RasterSub, RectI32)> {
    let row_bytes_raw = r.read_u16()?;
    if row_bytes_raw & 0x8000 != 0 {
        return Err(PictError::invalid(
            "PackBitsRect rowBytes top bit is set (looks like a PixMap, not a BitMap)",
        ));
    }
    let row_bytes = row_bytes_raw as usize;
    let bounds = r.read_rect()?;
    let _src_rect = r.read_rect()?;
    let dst_rect = r.read_rect()?;
    let _mode = r.read_u16()?;

    let width = (bounds.3 - bounds.1).max(0) as u32;
    let height = (bounds.2 - bounds.0).max(0) as u32;

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

    let rgba = expand_1bpp_to_rgba(&bitmap, width, height, row_bytes);
    Ok((
        RasterSub {
            width,
            height,
            data: rgba,
        },
        RectI32::from_be(dst_rect.0, dst_rect.1, dst_rect.2, dst_rect.3),
    ))
}

fn expand_1bpp_to_rgba(bitmap: &[u8], width: u32, height: u32, row_bytes: usize) -> Vec<u8> {
    let mut rgba = vec![0u8; (width * height * 4) as usize];
    for y in 0..height as usize {
        for x in 0..width as usize {
            let byte = bitmap[y * row_bytes + (x >> 3)];
            let bit = (byte >> (7 - (x & 7))) & 1;
            let v = if bit == 1 { 0x00 } else { 0xFF };
            let off = (y * width as usize + x) * 4;
            rgba[off] = v;
            rgba[off + 1] = v;
            rgba[off + 2] = v;
            rgba[off + 3] = 0xFF;
        }
    }
    rgba
}

/// v2 `BitsRect` (`0x0090`) — same layout as PackBitsRect but every
/// row is raw (no PackBits, no per-row byteCount). `BitsRgn` (0x91)
/// adds a clipping region after the rects (consumed + ignored here).
fn decode_bits_rect_v2(r: &mut Reader<'_>, with_region: bool) -> Result<(RasterSub, RectI32)> {
    let row_bytes_raw = r.read_u16()?;
    if row_bytes_raw & 0x8000 != 0 {
        return Err(PictError::invalid(
            "BitsRect rowBytes top bit is set (PixMap)",
        ));
    }
    let row_bytes = row_bytes_raw as usize;
    let bounds = r.read_rect()?;
    let _src_rect = r.read_rect()?;
    let dst_rect = r.read_rect()?;
    let _mode = r.read_u16()?;
    if with_region {
        let _ = parse_region(r)?;
    }

    let width = (bounds.3 - bounds.1).max(0) as u32;
    let height = (bounds.2 - bounds.0).max(0) as u32;

    let mut bitmap = vec![0u8; row_bytes * height as usize];
    for y in 0..height as usize {
        let row = r.read_bytes(row_bytes)?;
        bitmap[y * row_bytes..(y + 1) * row_bytes].copy_from_slice(row);
    }
    let rgba = expand_1bpp_to_rgba(&bitmap, width, height, row_bytes);
    Ok((
        RasterSub {
            width,
            height,
            data: rgba,
        },
        RectI32::from_be(dst_rect.0, dst_rect.1, dst_rect.2, dst_rect.3),
    ))
}

/// `PackBitsRgn` (`0x0099`) — same as PackBitsRect plus a Region
/// clipping path inserted just before the per-row pixel data.
fn decode_pack_bits_rgn(r: &mut Reader<'_>) -> Result<(RasterSub, RectI32, Region)> {
    let row_bytes_raw = r.read_u16()?;
    if row_bytes_raw & 0x8000 != 0 {
        return Err(PictError::invalid(
            "PackBitsRgn rowBytes top bit is set (looks like a PixMap)",
        ));
    }
    let row_bytes = row_bytes_raw as usize;
    let bounds = r.read_rect()?;
    let _src_rect = r.read_rect()?;
    let dst_rect = r.read_rect()?;
    let _mode = r.read_u16()?;
    let rgn = parse_region(r)?;

    let width = (bounds.3 - bounds.1).max(0) as u32;
    let height = (bounds.2 - bounds.0).max(0) as u32;
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

    let rgba = expand_1bpp_to_rgba(&bitmap, width, height, row_bytes);
    Ok((
        RasterSub {
            width,
            height,
            data: rgba,
        },
        RectI32::from_be(dst_rect.0, dst_rect.1, dst_rect.2, dst_rect.3),
        rgn,
    ))
}

/// `DirectBitsRgn` (`0x009B`) — same as DirectBitsRect plus a Region
/// clipping path inserted just before the per-row pixel data.
fn decode_direct_bits_rgn(r: &mut Reader<'_>) -> Result<(RasterSub, RectI32, Region)> {
    let header = read_pixmap_header(r)?;
    let _src_rect = r.read_rect()?;
    let dst_rect = r.read_rect()?;
    let _mode = r.read_u16()?;
    let rgn = parse_region(r)?;
    let (rgba, dst) = decode_direct_bits_pixels(r, &header, dst_rect)?;
    Ok((
        RasterSub {
            width: header.width,
            height: header.height,
            data: rgba,
        },
        dst,
        rgn,
    ))
}

/// PixMap header sub-fields read from a DirectBitsRect / DirectBitsRgn
/// opcode body.
struct PixMapHeader {
    row_bytes: usize,
    width: u32,
    height: u32,
    pack_type: u16,
    pixel_size: u16,
    cmp_count: u16,
    cmp_size: u16,
}

/// Read the DirectBits[Rect|Rgn] PixMap header: baseAddr, rowBytes,
/// bounds, pmVersion, packType, packSize, hRes, vRes, pixelType,
/// pixelSize, cmpCount, cmpSize, planeBytes, pmTable, pmReserved.
fn read_pixmap_header(r: &mut Reader<'_>) -> Result<PixMapHeader> {
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

    Ok(PixMapHeader {
        row_bytes,
        width: (bounds.3 - bounds.1).max(0) as u32,
        height: (bounds.2 - bounds.0).max(0) as u32,
        pack_type,
        pixel_size,
        cmp_count,
        cmp_size,
    })
}

/// `DirectBitsRect` (`0x009A`).
fn decode_direct_bits_rect(r: &mut Reader<'_>) -> Result<(RasterSub, RectI32)> {
    let header = read_pixmap_header(r)?;
    let _src_rect = r.read_rect()?;
    let dst_rect = r.read_rect()?;
    let _mode = r.read_u16()?;
    let (rgba, dst) = decode_direct_bits_pixels(r, &header, dst_rect)?;
    Ok((
        RasterSub {
            width: header.width,
            height: header.height,
            data: rgba,
        },
        dst,
    ))
}

/// Decode the per-row pixel data of a DirectBits[Rect|Rgn] opcode
/// into a `width × height` RGBA buffer. Honours packType 1 (raw),
/// 2 (24-byte interleaved BGR for 32-bit), 3 (16-bit PackBits) and
/// 4 (component-separated PackBits).
fn decode_direct_bits_pixels(
    r: &mut Reader<'_>,
    h: &PixMapHeader,
    dst_rect: (i16, i16, i16, i16),
) -> Result<(Vec<u8>, RectI32)> {
    let dst = RectI32::from_be(dst_rect.0, dst_rect.1, dst_rect.2, dst_rect.3);
    let mut rgba = vec![0u8; (h.width * h.height * 4) as usize];
    match (h.pack_type, h.pixel_size) {
        (0, 16) | (1, 16) => decode_dbr_16bpp_raw(r, h, &mut rgba)?,
        (0, 32) | (1, 32) => decode_dbr_32bpp_raw(r, h, &mut rgba)?,
        (2, 32) => decode_dbr_32bpp_packtype2(r, h, &mut rgba)?,
        (3, 16) => decode_dbr_16bpp_packbits(r, h, &mut rgba)?,
        (4, 32) => decode_dbr_32bpp_planar_packbits(r, h, &mut rgba)?,
        _ => {
            return Err(PictError::unsupported(format!(
                "DirectBitsRect packType={} pixelSize={} cmpCount={} cmpSize={}",
                h.pack_type, h.pixel_size, h.cmp_count, h.cmp_size
            )));
        }
    }
    Ok((rgba, dst))
}

/// packType 1, pixelSize 16: A1R5G5B5 packed BE u16 per pixel.
fn decode_dbr_16bpp_raw(r: &mut Reader<'_>, h: &PixMapHeader, rgba: &mut [u8]) -> Result<()> {
    if !(h.cmp_count == 3 && h.cmp_size == 5) {
        return Err(PictError::unsupported(format!(
            "DirectBitsRect 16bpp expects cmpCount=3 cmpSize=5, got {}/{}",
            h.cmp_count, h.cmp_size
        )));
    }
    for y in 0..h.height as usize {
        let row = r.read_bytes(h.row_bytes)?;
        write_16bpp_row(row, h.width as usize, &mut rgba[y * h.width as usize * 4..]);
    }
    Ok(())
}

fn write_16bpp_row(row: &[u8], width: usize, rgba_row: &mut [u8]) {
    for x in 0..width {
        let p = u16::from_be_bytes([row[x * 2], row[x * 2 + 1]]);
        let r5 = ((p >> 10) & 0x1F) as u8;
        let g5 = ((p >> 5) & 0x1F) as u8;
        let b5 = (p & 0x1F) as u8;
        let off = x * 4;
        rgba_row[off] = (r5 << 3) | (r5 >> 2);
        rgba_row[off + 1] = (g5 << 3) | (g5 >> 2);
        rgba_row[off + 2] = (b5 << 3) | (b5 >> 2);
        rgba_row[off + 3] = 0xFF;
    }
}

/// packType 1, pixelSize 32: 4 bytes per pixel. cmpCount=3 means
/// 0xFF R G B; cmpCount=4 means A R G B.
fn decode_dbr_32bpp_raw(r: &mut Reader<'_>, h: &PixMapHeader, rgba: &mut [u8]) -> Result<()> {
    if !(h.cmp_size == 8 && (h.cmp_count == 3 || h.cmp_count == 4)) {
        return Err(PictError::unsupported(format!(
            "DirectBitsRect 32bpp expects cmpSize=8 cmpCount=3|4, got {}/{}",
            h.cmp_count, h.cmp_size
        )));
    }
    for y in 0..h.height as usize {
        let row = r.read_bytes(h.row_bytes)?;
        write_32bpp_row(
            row,
            h.width as usize,
            h.cmp_count,
            &mut rgba[y * h.width as usize * 4..],
        );
    }
    Ok(())
}

fn write_32bpp_row(row: &[u8], width: usize, cmp_count: u16, rgba_row: &mut [u8]) {
    for x in 0..width {
        let off_in = x * 4;
        let off_out = x * 4;
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
        rgba_row[off_out] = rr;
        rgba_row[off_out + 1] = gg;
        rgba_row[off_out + 2] = bb;
        rgba_row[off_out + 3] = a;
    }
}

/// packType 2, pixelSize 32: 3 bytes per pixel (R G B), no pad byte
/// — the only difference vs packType 1 is that the alpha / fill byte
/// is omitted. cmpCount=3, cmpSize=8.
fn decode_dbr_32bpp_packtype2(r: &mut Reader<'_>, h: &PixMapHeader, rgba: &mut [u8]) -> Result<()> {
    if !(h.cmp_size == 8 && h.cmp_count == 3) {
        return Err(PictError::unsupported(format!(
            "DirectBitsRect packType=2 expects cmpCount=3 cmpSize=8, got {}/{}",
            h.cmp_count, h.cmp_size
        )));
    }
    let stride = (h.width as usize) * 3;
    for y in 0..h.height as usize {
        let row = r.read_bytes(stride)?;
        for x in 0..h.width as usize {
            let off_in = x * 3;
            let off_out = (y * h.width as usize + x) * 4;
            rgba[off_out] = row[off_in];
            rgba[off_out + 1] = row[off_in + 1];
            rgba[off_out + 2] = row[off_in + 2];
            rgba[off_out + 3] = 0xFF;
        }
    }
    Ok(())
}

/// packType 3, pixelSize 16: PackBits-compressed per row, where the
/// byte unit replicated by the encoder is a `u16` (so a "run of 5"
/// produces 5 u16 pixels, 10 bytes). The per-row byteCount prefix is
/// 1 byte if `rowBytes < 250` and 2 bytes otherwise.
fn decode_dbr_16bpp_packbits(r: &mut Reader<'_>, h: &PixMapHeader, rgba: &mut [u8]) -> Result<()> {
    if !(h.cmp_count == 3 && h.cmp_size == 5) {
        return Err(PictError::unsupported(format!(
            "DirectBitsRect packType=3 16bpp expects cmpCount=3 cmpSize=5, got {}/{}",
            h.cmp_count, h.cmp_size
        )));
    }
    let row_pixels = h.width as usize;
    let mut row_buf = vec![0u8; row_pixels * 2];
    for y in 0..h.height as usize {
        let _byte_count = if h.row_bytes > 250 {
            r.read_u16()? as usize
        } else {
            r.read_u8()? as usize
        };
        decode_packbits_u16_into(r, &mut row_buf)?;
        write_16bpp_row(&row_buf, row_pixels, &mut rgba[y * row_pixels * 4..]);
    }
    Ok(())
}

/// PackBits where the replicated unit is a `u16` (big-endian). Used
/// by DirectBitsRect packType 3.
fn decode_packbits_u16_into(src: &mut Reader<'_>, out: &mut [u8]) -> Result<()> {
    let expected_pixels = out.len() / 2;
    let mut written = 0usize;
    while written < expected_pixels {
        let flag = src.read_u8()? as i8;
        if flag >= 0 {
            // Raw packet: copy next (flag + 1) u16 pixels verbatim.
            let n = flag as usize + 1;
            if written + n > expected_pixels {
                return Err(PictError::invalid(format!(
                    "PackBits16 raw packet overruns row: {written} + {n} > {expected_pixels}"
                )));
            }
            let bytes = src.read_bytes(n * 2)?;
            out[written * 2..(written + n) * 2].copy_from_slice(bytes);
            written += n;
        } else if flag == -128 {
            continue;
        } else {
            // Run packet: replicate next u16 (1 - flag) times.
            let n = (1 - flag as i32) as usize;
            if written + n > expected_pixels {
                return Err(PictError::invalid(format!(
                    "PackBits16 run packet overruns row: {written} + {n} > {expected_pixels}"
                )));
            }
            let lo = src.read_u8()?;
            let hi = src.read_u8()?;
            for px in 0..n {
                out[(written + px) * 2] = lo;
                out[(written + px) * 2 + 1] = hi;
            }
            written += n;
        }
    }
    Ok(())
}

/// packType 4, pixelSize 32: per-row component-separated PackBits.
/// The decoder expects N planes per row (N = cmpCount = 3 or 4),
/// each plane PackBits-compressed at u8 unit size, stored
/// concatenated. After decoding all planes we interleave them into
/// RGBA.
fn decode_dbr_32bpp_planar_packbits(
    r: &mut Reader<'_>,
    h: &PixMapHeader,
    rgba: &mut [u8],
) -> Result<()> {
    if !(h.cmp_size == 8 && (h.cmp_count == 3 || h.cmp_count == 4)) {
        return Err(PictError::unsupported(format!(
            "DirectBitsRect packType=4 32bpp expects cmpCount=3|4 cmpSize=8, got {}/{}",
            h.cmp_count, h.cmp_size
        )));
    }
    let n_planes = h.cmp_count as usize;
    let row_pixels = h.width as usize;
    let plane_bytes = row_pixels;
    let mut plane_buf = vec![0u8; plane_bytes * n_planes];
    for y in 0..h.height as usize {
        let _byte_count = if h.row_bytes > 250 {
            r.read_u16()? as usize
        } else {
            r.read_u8()? as usize
        };
        // The per-row byteCount covers ALL planes together; PackBits
        // packets reset between planes since we know each plane is
        // exactly `row_pixels` bytes long.
        for p in 0..n_planes {
            packbits::decode_into(r, &mut plane_buf[p * plane_bytes..(p + 1) * plane_bytes])?;
        }
        // Interleave: plane order is R, G, B (cmpCount=3) or A, R,
        // G, B (cmpCount=4).
        for x in 0..row_pixels {
            let off_out = (y * row_pixels + x) * 4;
            if n_planes == 4 {
                let a = plane_buf[x];
                let rr = plane_buf[plane_bytes + x];
                let gg = plane_buf[2 * plane_bytes + x];
                let bb = plane_buf[3 * plane_bytes + x];
                rgba[off_out] = rr;
                rgba[off_out + 1] = gg;
                rgba[off_out + 2] = bb;
                rgba[off_out + 3] = a;
            } else {
                let rr = plane_buf[x];
                let gg = plane_buf[plane_bytes + x];
                let bb = plane_buf[2 * plane_bytes + x];
                rgba[off_out] = rr;
                rgba[off_out + 1] = gg;
                rgba[off_out + 2] = bb;
                rgba[off_out + 3] = 0xFF;
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// PICT v1 opcode walker.
// ---------------------------------------------------------------------------

/// PICT v1 opcodes (per Inside Macintosh §A-3) — 8 bits each, NO
/// word alignment between opcodes. The opcode set is a strict subset
/// of v2's: same numbering for the shape verbs, but raster opcodes
/// only go up to 0x99 and the opcode operand sizes are unchanged.
fn parse_v1_opcodes(
    r: &mut Reader<'_>,
    pic_frame: RectI32,
    mut canvas: Canvas,
    mut state: PictState,
) -> Result<PictImage> {
    while !r.at_eof() {
        let opcode = r.read_u8()? as u16;
        if !dispatch_v1_opcode(r, opcode, &pic_frame, &mut canvas, &mut state)? {
            break;
        }
    }
    finalise_canvas(canvas, &state)
}

fn dispatch_v1_opcode(
    r: &mut Reader<'_>,
    opcode: u16,
    pic_frame: &RectI32,
    canvas: &mut Canvas,
    state: &mut PictState,
) -> Result<bool> {
    // v1 opcodes share v2's numbering for the small shape / state ops
    // (one byte instead of two, no word alignment). Raster ops differ.
    match opcode {
        0x00 => Ok(true),
        0xFF => Ok(false), // OpEndPic in v1 is one byte 0xFF
        0x01 => {
            let _ = parse_region(r)?;
            Ok(true)
        }
        0x02 => {
            r.skip(8)?; // BkPat
            Ok(true)
        }
        0x07 => {
            let v = r.read_i16()?;
            let h = r.read_i16()?;
            state.pen_size = (h as i32, v as i32);
            Ok(true)
        }
        0x0B => {
            let v = r.read_i16()?;
            let h = r.read_i16()?;
            state.oval_size = (h as i32, v as i32);
            Ok(true)
        }
        0x0C => {
            let dh = r.read_i16()?;
            let dv = r.read_i16()?;
            state.origin.0 -= dh as i32;
            state.origin.1 -= dv as i32;
            Ok(true)
        }
        0x0E => {
            let code = r.read_u32()?;
            state.fg = Rgba::from_pascal_colour(code);
            Ok(true)
        }
        0x0F => {
            let code = r.read_u32()?;
            state.bg = Rgba::from_pascal_colour(code);
            Ok(true)
        }
        0x20 => {
            let pt0_v = r.read_i16()? as i32;
            let pt0_h = r.read_i16()? as i32;
            let pt1_v = r.read_i16()? as i32;
            let pt1_h = r.read_i16()? as i32;
            let (x0, y0) = to_canvas(state, pt0_h, pt0_v);
            let (x1, y1) = to_canvas(state, pt1_h, pt1_v);
            draw_line(canvas, x0, y0, x1, y1, state.fg);
            state.pen = (pt1_h, pt1_v);
            Ok(true)
        }
        0x21 => {
            let pt_v = r.read_i16()? as i32;
            let pt_h = r.read_i16()? as i32;
            let (x0, y0) = to_canvas(state, state.pen.0, state.pen.1);
            let (x1, y1) = to_canvas(state, pt_h, pt_v);
            draw_line(canvas, x0, y0, x1, y1, state.fg);
            state.pen = (pt_h, pt_v);
            Ok(true)
        }
        0x22 => {
            let pt_v = r.read_i16()? as i32;
            let pt_h = r.read_i16()? as i32;
            let dh = r.read_u8()? as i8 as i32;
            let dv = r.read_u8()? as i8 as i32;
            let nx = pt_h + dh;
            let ny = pt_v + dv;
            let (x0, y0) = to_canvas(state, pt_h, pt_v);
            let (x1, y1) = to_canvas(state, nx, ny);
            draw_line(canvas, x0, y0, x1, y1, state.fg);
            state.pen = (nx, ny);
            Ok(true)
        }
        0x23 => {
            let dh = r.read_u8()? as i8 as i32;
            let dv = r.read_u8()? as i8 as i32;
            let nx = state.pen.0 + dh;
            let ny = state.pen.1 + dv;
            let (x0, y0) = to_canvas(state, state.pen.0, state.pen.1);
            let (x1, y1) = to_canvas(state, nx, ny);
            draw_line(canvas, x0, y0, x1, y1, state.fg);
            state.pen = (nx, ny);
            Ok(true)
        }
        0x30..=0x34 | 0x40..=0x44 | 0x50..=0x54 => {
            let rect = read_rect_op(r)?;
            match opcode {
                0x30..=0x34 => {
                    state.last_rect = Some(rect);
                    apply_rect_verb(canvas, state, opcode, rect);
                }
                0x40..=0x44 => {
                    state.last_rrect = Some(rect);
                    apply_rrect_verb(canvas, state, opcode, rect);
                }
                _ => {
                    state.last_oval = Some(rect);
                    apply_oval_verb(canvas, state, opcode, rect);
                }
            }
            Ok(true)
        }
        0x60..=0x64 => {
            let rect = read_rect_op(r)?;
            let start = r.read_i16()? as i32;
            let arc = r.read_i16()? as i32;
            state.last_arc_rect = Some(rect);
            apply_arc_verb(canvas, state, opcode, rect, start, arc);
            Ok(true)
        }
        0x70..=0x74 => {
            let poly_size = r.read_u16()? as usize;
            if poly_size < 10 {
                return Err(PictError::invalid(format!(
                    "v1 polygon size {poly_size} smaller than 10-byte header"
                )));
            }
            let _bbox = r.read_rect()?;
            let n_verts = (poly_size - 10) / 4;
            let mut verts = Vec::with_capacity(n_verts);
            for _ in 0..n_verts {
                let v = r.read_i16()? as i32;
                let h = r.read_i16()? as i32;
                let (cx, cy) = to_canvas(state, h, v);
                verts.push((cx, cy));
            }
            apply_poly_verb(canvas, state, opcode, &verts);
            Ok(true)
        }
        0x80..=0x84 => {
            let rgn = parse_region(r)?;
            match opcode & 0x0F {
                0 => paint_region_outline(canvas, state, &rgn),
                1 | 4 => paint_region_filled(canvas, state, &rgn, state.fg),
                2 => paint_region_filled(canvas, state, &rgn, state.bg),
                3 => invert_region(canvas, state, &rgn),
                _ => {}
            }
            Ok(true)
        }
        0x90 => {
            let (img, dst) = decode_bits_rect_v2(r, false)?;
            blit_subimage(canvas, state, &img, &dst);
            Ok(true)
        }
        0x91 => {
            let (img, dst) = decode_bits_rect_v2(r, true)?;
            blit_subimage(canvas, state, &img, &dst);
            Ok(true)
        }
        0x98 => {
            let (img, dst) = decode_pack_bits_rect(r)?;
            blit_subimage(canvas, state, &img, &dst);
            Ok(true)
        }
        0x99 => {
            let (img, dst, _rgn) = decode_pack_bits_rgn(r)?;
            blit_subimage(canvas, state, &img, &dst);
            Ok(true)
        }
        0xA0 => {
            r.skip(2)?;
            Ok(true)
        }
        0xA1 => {
            r.skip(2)?;
            let n = r.read_u16()? as usize;
            r.skip(n)?;
            Ok(true)
        }
        _ => Err(PictError::unsupported(format!(
            "unknown / unsupported v1 opcode 0x{opcode:02X} at offset {} (frame={pic_frame:?})",
            r.pos - 1
        ))),
    }
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
