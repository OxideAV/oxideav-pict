//! PICT byte-stream probe — read-only introspection without rasterising.
//!
//! Consumers that only need *metadata* about a PICT byte stream (which
//! version, what picture frame, whether there's a 512-byte launch-stub
//! prefix, what mix of drawing / raster / text / comment / QuickTime
//! opcodes the file actually contains) can call [`probe_pict`] and skip
//! the cost of materialising a [`crate::PictImage`] canvas. The probe
//! shares its opcode walker with the decoder so the set of recognised
//! opcodes stays in sync — anything the decoder rasterises is counted
//! here, and anything the decoder treats as fatal makes the probe
//! return [`ProbeTermination::Unsupported`] *without* losing the
//! statistics gathered up to that point.
//!
//! No new spec material is consulted: the probe re-uses the same
//! Inside Macintosh: Imaging With QuickDraw §A-3 opcode interpretation
//! the decoder already implements. The walker is *byte-identical* to
//! [`crate::decoder`]'s opcode loop where it overlaps; the only
//! difference is that side-effect calls (canvas blit, region install,
//! state-machine mutations) are replaced by a counter bump.
//!
//! ## Use cases
//!
//! * Thumbnail UIs deciding whether a `.pct` is worth rasterising at
//!   all (degenerate frame? no raster?).
//! * Content scanners spotting embedded QuickTime payloads
//!   (`0x8200` / `0x8201`) before paying the JPEG-decode cost.
//! * Test harnesses asserting an encoder emitted the expected opcode
//!   mix without reaching into the canvas pixels.

use crate::error::{PictError, Result};
use crate::opcodes::*;
use crate::reader::Reader;
use crate::state::RectI32;

/// Read-only metadata extracted from a PICT byte stream.
///
/// Produced by [`probe_pict`]; never carries pixel data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PictProbe {
    /// PICT framing version detected by the version stanza.
    pub version: ProbeVersion,
    /// `picFrame` rectangle (top, left, bottom, right) in picture
    /// coordinates. Width and height come from `right − left` and
    /// `bottom − top`; either can be zero on a degenerate frame.
    pub frame: ProbeRect,
    /// Width derived from `picFrame` (`right − left`, clamped at 0).
    pub width: u32,
    /// Height derived from `picFrame` (`bottom − top`, clamped at 0).
    pub height: u32,
    /// `true` when the byte stream begins with the 512-byte Apple
    /// pre-OS-X launch-stub prefix and the actual picture record
    /// starts at byte 512.
    pub has_launch_stub: bool,
    /// How many raster-producing opcodes (`BitsRect`, `PackBitsRect`,
    /// `DirectBitsRect`, region variants for each) appear in the stream.
    pub raster_count: u32,
    /// How many drawing-primitive opcodes (line / rect / round-rect /
    /// oval / arc / poly / region verbs) appear in the stream. The
    /// *same-as-last* variants (`OP_FRAME_SAME_RECT` etc) are counted
    /// separately under [`Self::same_shape_count`].
    pub drawing_count: u32,
    /// How many "same as last" shape opcodes appear. These don't carry
    /// new operands but still produce a draw.
    pub same_shape_count: u32,
    /// How many text-glyph opcodes (`LongText`, `DH/DV/DHDVText`)
    /// appear. The decoder walks past them without rasterising glyphs;
    /// the count is useful for spotting label-bearing PICTs.
    pub text_count: u32,
    /// How many comment opcodes (`ShortComment` + `LongComment`).
    pub comment_count: u32,
    /// How many `ClipRgn` opcodes (each one *replaces* the active clip).
    pub clip_rgn_count: u32,
    /// How many pattern-set opcodes (`PnPat`, `BkPat`, `FillPat`) appear.
    /// Inside Macintosh §A-3 monochrome 8×8 patterns; counted once per
    /// occurrence regardless of which slot they target.
    pub pattern_set_count: u32,
    /// How many multi-colour pattern-set opcodes (`PnPixPat 0x0013`,
    /// `BkPixPat 0x0012`, `FillPixPat 0x0014`) appear. Inside Macintosh
    /// §A-3 Listing A-1; counted once per occurrence regardless of
    /// slot or sub-type (`patType=1` colour-pixmap and `patType=2`
    /// dither both count).
    pub pix_pattern_set_count: u32,
    /// How many `CompressedQuickTime` (`0x8200`) opcodes appear. Each
    /// carries an embedded QuickTime image (typically JPEG) the
    /// decoder currently skips.
    pub compressed_quicktime_count: u32,
    /// How many `UncompressedQuickTime` (`0x8201`) opcodes appear.
    pub uncompressed_quicktime_count: u32,
    /// `true` if the opcode walker observed an `OpEndPic` (`0x00FF` /
    /// `0xFF`) and terminated cleanly.
    pub end_pic_seen: bool,
    /// Reason the walker terminated.
    pub termination: ProbeTermination,
    /// Byte offset (into the *whole* input buffer including the
    /// launch-stub prefix if any) at which the walker stopped.
    pub terminated_at: usize,
}

impl PictProbe {
    /// `true` if the probe saw any drawing or raster opcode.
    pub fn has_visible_content(&self) -> bool {
        self.raster_count > 0 || self.drawing_count > 0 || self.same_shape_count > 0
    }

    /// `true` if the probe saw any embedded QuickTime payload.
    pub fn has_quicktime(&self) -> bool {
        self.compressed_quicktime_count > 0 || self.uncompressed_quicktime_count > 0
    }
}

/// PICT framing version reported by [`PictProbe`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeVersion {
    /// v1: 8-bit opcodes, sentinel `0x1101`.
    V1,
    /// v2: 16-bit word-aligned opcodes, sentinel `0x0011 0x02FF`.
    V2,
}

/// Why the probe stopped walking opcodes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeTermination {
    /// Reached an `OpEndPic` opcode — the canonical clean termination.
    EndPic,
    /// Hit end-of-input without an `OpEndPic`. Real-world generators
    /// sometimes truncate the stream right after the last raster.
    Eof,
    /// Hit an opcode the underlying parser would have rejected. The
    /// preserved statistics describe everything seen *before* this
    /// opcode. The string payload is the same message the decoder
    /// would have surfaced via `PictError::Unsupported`.
    Unsupported(String),
    /// Hit a malformed stream (truncated opcode payload, polygon size
    /// smaller than the header, etc). Same string as the decoder's
    /// `PictError::InvalidData`.
    Invalid(String),
}

/// `picFrame` rectangle in the same (top, left, bottom, right) order
/// used by the QuickDraw `Rect` on-disk layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbeRect {
    pub top: i32,
    pub left: i32,
    pub bottom: i32,
    pub right: i32,
}

impl From<RectI32> for ProbeRect {
    fn from(r: RectI32) -> Self {
        Self {
            top: r.top,
            left: r.left,
            bottom: r.bottom,
            right: r.right,
        }
    }
}

/// Walk a PICT byte stream and return a [`PictProbe`] summary.
///
/// Returns an `Err` only when the framing itself is broken (no picture
/// record at offset 0 or 512, invalid version stanza). Opcode-walk
/// failures — unsupported opcodes, truncated bodies — are recorded in
/// [`PictProbe::termination`] so the caller still sees the partial
/// statistics gathered before the failure.
pub fn probe_pict(bytes: &[u8]) -> Result<PictProbe> {
    let body_offset = detect_body_offset(bytes)?;
    let has_launch_stub = body_offset > 0;
    let body = &bytes[body_offset..];

    let mut r = Reader::new(body);
    let _pic_size = r.read_u16()?;
    let frame_tuple = r.read_rect()?;
    let frame = RectI32::from_be(frame_tuple.0, frame_tuple.1, frame_tuple.2, frame_tuple.3);
    let version = detect_version_probe(&mut r)?;

    let width = (frame.right - frame.left).max(0) as u32;
    let height = (frame.bottom - frame.top).max(0) as u32;

    let mut p = PictProbe {
        version,
        frame: frame.into(),
        width,
        height,
        has_launch_stub,
        raster_count: 0,
        drawing_count: 0,
        same_shape_count: 0,
        text_count: 0,
        comment_count: 0,
        clip_rgn_count: 0,
        pattern_set_count: 0,
        pix_pattern_set_count: 0,
        compressed_quicktime_count: 0,
        uncompressed_quicktime_count: 0,
        end_pic_seen: false,
        termination: ProbeTermination::Eof,
        terminated_at: body_offset + r.pos,
    };

    let result = match version {
        ProbeVersion::V1 => probe_v1(&mut r, &mut p),
        ProbeVersion::V2 => probe_v2(&mut r, &mut p),
    };
    p.terminated_at = body_offset + r.pos;
    p.termination = match result {
        Ok(true) => {
            p.end_pic_seen = true;
            ProbeTermination::EndPic
        }
        Ok(false) => ProbeTermination::Eof,
        Err(PictError::Unsupported(msg)) => ProbeTermination::Unsupported(msg),
        Err(PictError::InvalidData(msg)) => ProbeTermination::Invalid(msg),
        Err(PictError::NoRaster) => ProbeTermination::Eof,
    };
    Ok(p)
}

/// Byte-offset detection shared with the decoder. Inlined here so we
/// don't have to make the decoder's private helper `pub(crate)` just
/// for the probe.
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

fn detect_version_probe(r: &mut Reader<'_>) -> Result<ProbeVersion> {
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
            return Ok(ProbeVersion::V2);
        }
        if (next >> 8) == 0x01 {
            r.pos -= 1;
            return Ok(ProbeVersion::V1);
        }
        return Err(PictError::invalid(format!(
            "unrecognised version stanza after 0x0011: 0x{next:04X}"
        )));
    }
    if v_word == 0x1101 {
        return Ok(ProbeVersion::V1);
    }
    Err(PictError::invalid(format!(
        "expected version opcode 0x0011 or 0x1101, got 0x{v_word:04X}"
    )))
}

/// `Ok(true)` on `OpEndPic`, `Ok(false)` on clean EOF; `Err` on a
/// malformed or unsupported opcode (the caller folds these into
/// [`ProbeTermination`]).
fn probe_v2(r: &mut Reader<'_>, p: &mut PictProbe) -> Result<bool> {
    loop {
        r.align_word()?;
        if r.at_eof() {
            return Ok(false);
        }
        let opcode = r.read_u16()?;
        match probe_v2_opcode(r, opcode, p)? {
            OpStep::Continue => {}
            OpStep::EndPic => return Ok(true),
        }
    }
}

fn probe_v1(r: &mut Reader<'_>, p: &mut PictProbe) -> Result<bool> {
    loop {
        if r.at_eof() {
            return Ok(false);
        }
        let opcode = r.read_u8()? as u16;
        match probe_v1_opcode(r, opcode, p)? {
            OpStep::Continue => {}
            OpStep::EndPic => return Ok(true),
        }
    }
}

enum OpStep {
    Continue,
    EndPic,
}

fn probe_v2_opcode(r: &mut Reader<'_>, opcode: u16, p: &mut PictProbe) -> Result<OpStep> {
    match opcode {
        OP_NOP => Ok(OpStep::Continue),
        OP_OP_END_PIC => Ok(OpStep::EndPic),
        OP_CLIP_RGN | OP_FRAME_RGN | OP_PAINT_RGN | OP_ERASE_RGN | OP_INVERT_RGN | OP_FILL_RGN => {
            // Region size word + payload — same parse as the decoder.
            let rgn_size = r.read_u16()? as usize;
            if rgn_size < 10 {
                return Err(PictError::invalid(format!(
                    "region size {rgn_size} smaller than the 10-byte header"
                )));
            }
            r.skip(rgn_size - 2)?;
            if opcode == OP_CLIP_RGN {
                p.clip_rgn_count += 1;
            } else {
                p.drawing_count += 1;
            }
            Ok(OpStep::Continue)
        }
        OP_RGB_FG_COL | OP_RGB_BK_COL => {
            r.skip(6)?;
            Ok(OpStep::Continue)
        }
        OP_FG_COLOR | OP_BG_COLOR => {
            r.skip(4)?;
            Ok(OpStep::Continue)
        }
        OP_PN_SIZE | OP_OV_SIZE | OP_ORIGIN => {
            r.skip(4)?;
            Ok(OpStep::Continue)
        }
        OP_LINE => {
            r.skip(8)?;
            p.drawing_count += 1;
            Ok(OpStep::Continue)
        }
        OP_LINE_FROM => {
            r.skip(4)?;
            p.drawing_count += 1;
            Ok(OpStep::Continue)
        }
        OP_SHORT_LINE => {
            r.skip(6)?;
            p.drawing_count += 1;
            Ok(OpStep::Continue)
        }
        OP_SHORT_LINE_FROM => {
            r.skip(2)?;
            p.drawing_count += 1;
            Ok(OpStep::Continue)
        }
        OP_FRAME_RECT | OP_PAINT_RECT | OP_ERASE_RECT | OP_INVERT_RECT | OP_FILL_RECT
        | OP_FRAME_RRECT | OP_PAINT_RRECT | OP_ERASE_RRECT | OP_INVERT_RRECT | OP_FILL_RRECT
        | OP_FRAME_OVAL | OP_PAINT_OVAL | OP_ERASE_OVAL | OP_INVERT_OVAL | OP_FILL_OVAL => {
            r.skip(8)?;
            p.drawing_count += 1;
            Ok(OpStep::Continue)
        }
        OP_FRAME_SAME_RECT | OP_PAINT_SAME_RECT | OP_ERASE_SAME_RECT | OP_INVERT_SAME_RECT
        | OP_FILL_SAME_RECT | OP_FRAME_SAME_RRECT | OP_PAINT_SAME_RRECT | OP_ERASE_SAME_RRECT
        | OP_INVERT_SAME_RRECT | OP_FILL_SAME_RRECT | OP_FRAME_SAME_OVAL | OP_PAINT_SAME_OVAL
        | OP_ERASE_SAME_OVAL | OP_INVERT_SAME_OVAL | OP_FILL_SAME_OVAL => {
            p.same_shape_count += 1;
            Ok(OpStep::Continue)
        }
        OP_FRAME_ARC | OP_PAINT_ARC | OP_ERASE_ARC | OP_INVERT_ARC | OP_FILL_ARC => {
            r.skip(8 + 4)?; // rect + start/arc words
            p.drawing_count += 1;
            Ok(OpStep::Continue)
        }
        OP_FRAME_SAME_ARC | OP_PAINT_SAME_ARC | OP_ERASE_SAME_ARC | OP_INVERT_SAME_ARC
        | OP_FILL_SAME_ARC => {
            r.skip(4)?;
            p.same_shape_count += 1;
            Ok(OpStep::Continue)
        }
        OP_FRAME_POLY | OP_PAINT_POLY | OP_ERASE_POLY | OP_INVERT_POLY | OP_FILL_POLY => {
            let poly_size = r.read_u16()? as usize;
            if poly_size < 10 {
                return Err(PictError::invalid(format!(
                    "polygon size {poly_size} smaller than the 10-byte header"
                )));
            }
            r.skip(poly_size - 2)?;
            p.drawing_count += 1;
            Ok(OpStep::Continue)
        }
        OP_LONG_TEXT => {
            r.skip(4)?;
            let n = r.read_u8()? as usize;
            r.skip(n)?;
            p.text_count += 1;
            Ok(OpStep::Continue)
        }
        OP_DH_TEXT | OP_DV_TEXT => {
            r.skip(1)?;
            let n = r.read_u8()? as usize;
            r.skip(n)?;
            p.text_count += 1;
            Ok(OpStep::Continue)
        }
        OP_DHDV_TEXT => {
            r.skip(2)?;
            let n = r.read_u8()? as usize;
            r.skip(n)?;
            p.text_count += 1;
            Ok(OpStep::Continue)
        }
        OP_FONT_NAME => {
            let n = r.read_u16()? as usize;
            if n < 2 {
                return Err(PictError::invalid(format!(
                    "fontName dataLength {n} smaller than the size word"
                )));
            }
            r.skip(n - 2)?;
            Ok(OpStep::Continue)
        }
        OP_LINE_JUSTIFY | OP_GLYPH_STATE => {
            let n = r.read_u16()? as usize;
            r.skip(n)?;
            Ok(OpStep::Continue)
        }
        OP_BK_PAT | OP_PN_PAT | OP_FILL_PAT => {
            // Monochrome 8×8 pattern payload (round 8 — workspace round
            // 81). The decoder mirror reads the same 8 bytes into the
            // drawing-state's `back_pat` / `pen_pat` / `fill_pat` slot;
            // the probe just records the occurrence so callers can spot
            // patterned PICTs without rasterising.
            r.skip(8)?;
            p.pattern_set_count += 1;
            Ok(OpStep::Continue)
        }
        OP_BK_PIX_PAT | OP_PN_PIX_PAT | OP_FILL_PIX_PAT => {
            // PixPat — variable-size colour pattern (Inside Macintosh
            // §A-3 Listing A-1). Walk the same record layout the
            // decoder consumes to keep the probe in sync; the read
            // bytes are discarded.
            skip_pix_pat(r)?;
            p.pix_pattern_set_count += 1;
            Ok(OpStep::Continue)
        }
        OP_SHORT_COMMENT => {
            r.skip(2)?;
            p.comment_count += 1;
            Ok(OpStep::Continue)
        }
        OP_LONG_COMMENT => {
            r.skip(2)?;
            let n = r.read_u16()? as usize;
            r.skip(n)?;
            p.comment_count += 1;
            Ok(OpStep::Continue)
        }
        OP_BITS_RECT | OP_BITS_RGN | OP_PACK_BITS_RECT | OP_PACK_BITS_RGN | OP_DIRECT_BITS_RECT
        | OP_DIRECT_BITS_RGN => {
            // Raster opcodes have variable-size payloads. We delegate
            // to the decoder's existing parser by re-using the byte
            // sequences it reads: rather than duplicate the whole
            // PixMap-header / packType / per-row decode logic, we just
            // count the opcode and let the decoder reach end-of-stream
            // when the caller actually rasterises. The probe doesn't
            // need pixel bytes — so we have to either skip the payload
            // or stop. We stop counting raster bytes and forward the
            // request to the existing decode path via a thin walker.
            skip_raster_opcode_v2(r, opcode)?;
            p.raster_count += 1;
            Ok(OpStep::Continue)
        }
        OP_COMPRESSED_QUICKTIME => {
            let payload_size = r.read_u32()? as usize;
            r.skip(payload_size.saturating_sub(4))?;
            p.compressed_quicktime_count += 1;
            Ok(OpStep::Continue)
        }
        OP_UNCOMPRESSED_QUICKTIME => {
            let payload_size = r.read_u32()? as usize;
            r.skip(payload_size.saturating_sub(4))?;
            p.uncompressed_quicktime_count += 1;
            Ok(OpStep::Continue)
        }
        _ => {
            if let Some(n) = fixed_operand_size(opcode) {
                r.skip(n)?;
                Ok(OpStep::Continue)
            } else {
                Err(PictError::unsupported(format!(
                    "unknown / unsupported v2 opcode 0x{opcode:04X} at offset {}",
                    r.pos - 2
                )))
            }
        }
    }
}

fn probe_v1_opcode(r: &mut Reader<'_>, opcode: u16, p: &mut PictProbe) -> Result<OpStep> {
    match opcode {
        0x00 => Ok(OpStep::Continue),
        0xFF => Ok(OpStep::EndPic),
        0x01 => {
            // ClipRgn
            let rgn_size = r.read_u16()? as usize;
            if rgn_size < 10 {
                return Err(PictError::invalid(format!(
                    "v1 region size {rgn_size} smaller than 10-byte header"
                )));
            }
            r.skip(rgn_size - 2)?;
            p.clip_rgn_count += 1;
            Ok(OpStep::Continue)
        }
        0x02 | 0x09 | 0x0A => {
            // BkPat / PnPat / FillPat (v1 opcodes 0x02 / 0x09 / 0x0A) —
            // 8-byte monochrome 8×8 pattern payload.
            r.skip(8)?;
            p.pattern_set_count += 1;
            Ok(OpStep::Continue)
        }
        0x07 | 0x0B => {
            r.skip(4)?;
            Ok(OpStep::Continue)
        }
        0x0C => {
            r.skip(4)?;
            Ok(OpStep::Continue)
        }
        0x0E | 0x0F => {
            r.skip(4)?;
            Ok(OpStep::Continue)
        }
        0x20 => {
            r.skip(8)?;
            p.drawing_count += 1;
            Ok(OpStep::Continue)
        }
        0x21 => {
            r.skip(4)?;
            p.drawing_count += 1;
            Ok(OpStep::Continue)
        }
        0x22 => {
            r.skip(6)?;
            p.drawing_count += 1;
            Ok(OpStep::Continue)
        }
        0x23 => {
            r.skip(2)?;
            p.drawing_count += 1;
            Ok(OpStep::Continue)
        }
        0x30..=0x34 | 0x40..=0x44 | 0x50..=0x54 => {
            r.skip(8)?;
            p.drawing_count += 1;
            Ok(OpStep::Continue)
        }
        0x60..=0x64 => {
            r.skip(8 + 4)?;
            p.drawing_count += 1;
            Ok(OpStep::Continue)
        }
        0x70..=0x74 => {
            let poly_size = r.read_u16()? as usize;
            if poly_size < 10 {
                return Err(PictError::invalid(format!(
                    "v1 polygon size {poly_size} smaller than 10-byte header"
                )));
            }
            r.skip(poly_size - 2)?;
            p.drawing_count += 1;
            Ok(OpStep::Continue)
        }
        0x80..=0x84 => {
            let rgn_size = r.read_u16()? as usize;
            if rgn_size < 10 {
                return Err(PictError::invalid(format!(
                    "v1 region size {rgn_size} smaller than 10-byte header"
                )));
            }
            r.skip(rgn_size - 2)?;
            p.drawing_count += 1;
            Ok(OpStep::Continue)
        }
        0x90 | 0x91 | 0x98 | 0x99 | 0x9A | 0x9B => {
            skip_raster_opcode_v1(r, opcode)?;
            p.raster_count += 1;
            Ok(OpStep::Continue)
        }
        0xA0 => {
            r.skip(2)?;
            p.comment_count += 1;
            Ok(OpStep::Continue)
        }
        0xA1 => {
            r.skip(2)?;
            let n = r.read_u16()? as usize;
            r.skip(n)?;
            p.comment_count += 1;
            Ok(OpStep::Continue)
        }
        _ => Err(PictError::unsupported(format!(
            "unknown / unsupported v1 opcode 0x{opcode:02X} at offset {}",
            r.pos - 1
        ))),
    }
}

/// Static operand size for opcodes whose payload is a fixed number of
/// bytes per Inside Macintosh §A-3. Mirrors the same table used by
/// the decoder.
fn fixed_operand_size(opcode: u16) -> Option<usize> {
    // OP_BK_PAT / OP_PN_PAT / OP_FILL_PAT have dedicated arms in
    // `probe_v2_opcode` that bump `pattern_set_count`.
    Some(match opcode {
        OP_TX_FONT | OP_TX_MODE | OP_TX_SIZE | OP_PN_MODE | OP_PN_LOC_HFRAC | OP_CH_EXTRA => 2,
        OP_TX_FACE => 1,
        OP_SP_EXTRA => 4,
        OP_TX_RATIO => 8,
        OP_HILITE_MODE | OP_DEF_HILITE => 0,
        OP_HILITE_COLOR | OP_OP_COLOR => 6,
        _ => return None,
    })
}

/// Skip a v2 raster opcode payload (BitsRect / BitsRgn / PackBitsRect
/// / PackBitsRgn / DirectBitsRect / DirectBitsRgn) without decoding
/// pixels. Returns `Err` only on truncation.
fn skip_raster_opcode_v2(r: &mut Reader<'_>, opcode: u16) -> Result<()> {
    let is_pixmap = matches!(opcode, OP_DIRECT_BITS_RECT | OP_DIRECT_BITS_RGN);
    let with_rgn = matches!(opcode, OP_BITS_RGN | OP_PACK_BITS_RGN | OP_DIRECT_BITS_RGN);
    let row_bytes;
    let height;
    let pack_type;
    if is_pixmap {
        // DirectBits PixMap header: baseAddr (4) + rowBytes (2) +
        // bounds (8) + pmVersion (2) + packType (2) + packSize (4) +
        // hRes (4) + vRes (4) + pixelType (2) + pixelSize (2) +
        // cmpCount (2) + cmpSize (2) + planeBytes (4) + pmTable (4) +
        // pmReserved (4) = 50 bytes.
        let _base = r.read_u32()?;
        let rb_raw = r.read_u16()?;
        if rb_raw & 0x8000 == 0 {
            return Err(PictError::invalid(
                "DirectBitsRect rowBytes top bit is clear (looks like a BitMap, not a PixMap)",
            ));
        }
        row_bytes = (rb_raw & 0x3FFF) as usize;
        let bounds = r.read_rect()?;
        height = (bounds.2 - bounds.0).max(0) as usize;
        let _pm_version = r.read_u16()?;
        pack_type = r.read_u16()?;
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
        let _ = (pixel_size, cmp_count, cmp_size);
    } else {
        // BitMap header: rowBytes (2) + bounds (8) = 10 bytes total
        let rb_raw = r.read_u16()?;
        if rb_raw & 0x8000 != 0 {
            return Err(PictError::invalid(
                "BitsRect / PackBitsRect rowBytes top bit is set (looks like a PixMap)",
            ));
        }
        row_bytes = rb_raw as usize;
        let bounds = r.read_rect()?;
        height = (bounds.2 - bounds.0).max(0) as usize;
        pack_type = 0; // BitMap rows are always raw
    }

    // Shared trailer: srcRect (8) + dstRect (8) + mode (2).
    r.skip(8 + 8 + 2)?;

    // Optional region after the mode word.
    if with_rgn {
        let rgn_size = r.read_u16()? as usize;
        if rgn_size < 10 {
            return Err(PictError::invalid(format!(
                "raster-opcode region size {rgn_size} smaller than 10-byte header"
            )));
        }
        r.skip(rgn_size - 2)?;
    }

    // Pixel-data payload. For BitsRect / PackBitsRect (BitMap path)
    // we cover both raw and per-row PackBits cases.
    match opcode {
        OP_BITS_RECT | OP_BITS_RGN => {
            // Raw rows: row_bytes per row, height rows.
            r.skip(row_bytes * height)?;
        }
        OP_PACK_BITS_RECT | OP_PACK_BITS_RGN => {
            // Per-row PackBits when row_bytes >= 8, raw rows otherwise.
            if row_bytes < 8 {
                r.skip(row_bytes * height)?;
            } else {
                for _ in 0..height {
                    let byte_count = if row_bytes > 250 {
                        r.read_u16()? as usize
                    } else {
                        r.read_u8()? as usize
                    };
                    r.skip(byte_count)?;
                }
            }
        }
        OP_DIRECT_BITS_RECT | OP_DIRECT_BITS_RGN => {
            // DirectBits packType-specific row layout. packType 1
            // (raw) and packType 2 (packed 24-bpp, no pad byte) are
            // raw rows of a known size; packType 3 and 4 carry a
            // per-row byteCount prefix.
            match pack_type {
                0 | 1 => r.skip(row_bytes * height)?,
                2 => {
                    // 3 bytes per pixel; rowBytes (with the high bit
                    // stripped above) is the in-memory 4-bpp row width
                    // → divide by 4 to get pixel count, multiply by 3.
                    let pixels = row_bytes / 4;
                    let on_disk = pixels * 3;
                    r.skip(on_disk * height)?;
                }
                3 | 4 => {
                    for _ in 0..height {
                        let byte_count = if row_bytes > 250 {
                            r.read_u16()? as usize
                        } else {
                            r.read_u8()? as usize
                        };
                        r.skip(byte_count)?;
                    }
                }
                _ => {
                    return Err(PictError::unsupported(format!(
                        "DirectBitsRect packType {pack_type} (probe doesn't know the row layout)"
                    )));
                }
            }
        }
        _ => unreachable!(),
    }
    Ok(())
}

/// Skip a PixPat opcode payload (`BkPixPat 0x0012` / `PnPixPat 0x0013` /
/// `FillPixPat 0x0014`) without resolving the colour grid. Mirrors the
/// decoder's `decode_pix_pat` byte walk per Inside Macintosh §A-3
/// Listing A-1.
fn skip_pix_pat(r: &mut Reader<'_>) -> Result<()> {
    let pat_type = r.read_u16()?;
    // Pat1Data (8-byte fallback).
    r.skip(8)?;
    match pat_type {
        1 => {
            // colour-pixmap sub-type.
            let rb_raw = r.read_u16()?;
            let row_bytes = (rb_raw & 0x3FFF) as usize;
            let bounds = r.read_rect()?;
            let height = (bounds.2 - bounds.0).max(0) as usize;
            // pmVersion (2) + packType (2) + packSize (4) + hRes (4) +
            // vRes (4) + pixelType (2) + pixelSize (2) + cmpCount (2) +
            // cmpSize (2) + planeBytes (4) + pmTable (4) +
            // pmReserved (4) = 36 bytes.
            r.skip(36)?;
            // ColorTable: ctSeed (4) + ctFlags (2) + ctSize (2) + entries.
            let _ct_seed = r.read_u32()?;
            let _ct_flags = r.read_i16()?;
            let ct_size = r.read_i16()?;
            let n_entries = ((ct_size as i32) + 1).max(0) as usize;
            r.skip(n_entries * 8)?; // ColorSpec = value(2) + RGB(6)
                                    // PixData.
            if row_bytes < 8 {
                r.skip(row_bytes * height)?;
            } else {
                for _ in 0..height {
                    let bc = if row_bytes > 250 {
                        r.read_u16()? as usize
                    } else {
                        r.read_u8()? as usize
                    };
                    r.skip(bc)?;
                }
            }
            Ok(())
        }
        2 => {
            // ditherPat: 6-byte RGBColor.
            r.skip(6)
        }
        other => Err(PictError::unsupported(format!(
            "PixPat patType={other} (only 1=colourPixmap, 2=ditherPat per IM §A-3 Listing A-1)"
        ))),
    }
}

fn skip_raster_opcode_v1(r: &mut Reader<'_>, opcode: u16) -> Result<()> {
    // The v1 raster opcodes 0x90/0x91/0x98/0x99/0x9A/0x9B have the
    // exact same byte layout as their v2 counterparts — only the
    // opcode width (1 byte vs 2 bytes) differs.
    let v2_equivalent = match opcode {
        0x90 => OP_BITS_RECT,
        0x91 => OP_BITS_RGN,
        0x98 => OP_PACK_BITS_RECT,
        0x99 => OP_PACK_BITS_RGN,
        0x9A => OP_DIRECT_BITS_RECT,
        0x9B => OP_DIRECT_BITS_RGN,
        _ => unreachable!("non-raster v1 opcode reached skip_raster_opcode_v1"),
    };
    skip_raster_opcode_v2(r, v2_equivalent)
}
