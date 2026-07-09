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
use crate::header::{Fixed, PictHeader};
use crate::image::PictComment;
use crate::opcodes::*;
use crate::reader::Reader;
use crate::state::{
    PictFontName, PictGlyphState, PictLineJustify, PictTextState, RectI32, Rgba, TextRatio,
};

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
    /// How many of [`Self::raster_count`] use the indexed PixMap variant
    /// (rowBytes high-bit set) of `BitsRect 0x0090` / `BitsRgn 0x0091` /
    /// `PackBitsRect 0x0098` / `PackBitsRgn 0x0099`. Lets a probe caller
    /// know the raster pipeline will produce indexed RGBA against an
    /// embedded `ColorTable` rather than mono-from-BitMap or
    /// direct-from-PixMap. `DirectBitsRect 0x009A` /
    /// `DirectBitsRgn 0x009B` (always direct, always PixMap) are counted
    /// in [`Self::raster_count`] only.
    pub indexed_raster_count: u32,
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
    /// Each opcode counted here also appears as one entry in
    /// [`Self::comments`], so `comments.len() as u32 == comment_count`
    /// when the walker reaches `OpEndPic` cleanly. The two surfaces
    /// stay in sync even when the walker terminates early — every
    /// comment seen *before* the failure is recorded.
    pub comment_count: u32,
    /// Picture Comments captured during the walk, in stream order.
    /// Inside Macintosh: Imaging With QuickDraw §A-3 Table A-2
    /// (`$00A0` `ShortComment` / `$00A1` `LongComment`) and Table A-3
    /// (`$A0` / `$A1`) share the [`PictComment`] record layout via
    /// the [`PictComment::is_long`] flag. Content scanners use this
    /// surface to fish out PostScript fragments, application drawing
    /// hints, page-break markers, and other annotation metadata
    /// without paying the rasterisation cost.
    pub comments: Vec<PictComment>,
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
    /// How many §A-3 *reserved* v2 opcodes the walker stepped past
    /// without dispatching — i.e. opcodes in the ranges Inside
    /// Macintosh: Imaging With QuickDraw §A-3 (Table A-2) lists as
    /// "Reserved for Apple use" with a known payload size
    /// (`0x0024..=0x0027`, `0x002F`, `0x0035..=0x0037`, `0x003D..=
    /// 0x003F`, …, `0x0078..=0x008F`, `0x0092..=0x0097`, `0x009C..=
    /// 0x009F`, `0x00A2..=0x00FE`, `0x0100..=0x7FFF`, `0x8000..=
    /// 0x80FF`, `0x8100..=0x81FF`, `0x8202..=0xFFFF`). The three
    /// "Not determined" opcodes (`0x0017..=0x0019`) are *not* counted
    /// here — they still terminate the probe as
    /// [`ProbeTermination::Unsupported`] because their size is
    /// undocumented. Useful for spotting PICTs that carry private /
    /// Apple-internal extension records.
    pub reserved_op_count: u32,
    /// `true` if the opcode walker observed an `OpEndPic` (`0x00FF` /
    /// `0xFF`) and terminated cleanly.
    pub end_pic_seen: bool,
    /// Reason the walker terminated.
    pub termination: ProbeTermination,
    /// Byte offset (into the *whole* input buffer including the
    /// launch-stub prefix if any) at which the walker stopped.
    pub terminated_at: usize,
    /// Decoded form of the 24-byte v2 `HeaderOp` (`0x0C00`) payload
    /// (Inside Macintosh §A-3 / §A-22 Listing A-5 + A-6). `None` for
    /// v1 streams (no `HeaderOp` per §A-25) or v2 streams whose
    /// header version word doesn't match `0xFFFE` or `0xFFFF` (the
    /// probe tolerates a non-canonical 24-byte pad to keep statistics
    /// available for the surrounding opcode walk).
    pub header: Option<PictHeader>,
    /// Final tracked text / pen-mode / highlight state observed by the
    /// probe walker. Mirrors [`crate::PictImage::text_state`] — round 230
    /// captures the §A-3 Table A-2 / A-3 state opcodes (`TxFont`,
    /// `TxFace`, `TxMode`, `SpExtra`, `PnMode`, `TxSize`, `TxRatio`,
    /// `PnLocHFrac`, `ChExtra`, `HiliteMode`, `HiliteColor`,
    /// `DefHilite`, `OpColor`) into a structured snapshot so probe
    /// consumers can spot the producer's declared text shape and
    /// arithmetic-transfer-mode op-colour without paying the
    /// rasterisation cost. Defaults to
    /// [`PictTextState::fresh_graf_port`] when the picture emits no
    /// state opcode in the corresponding slot. The decoder + probe
    /// walkers share the same byte parse so the two surfaces stay in
    /// sync.
    pub text_state: PictTextState,
    /// How many §A-3 text / pen-mode / highlight state opcodes the
    /// walker observed (the same set that updates [`Self::text_state`]).
    /// Lets a probe caller distinguish "producer used the default
    /// shape and the slot has the default value" from "producer set
    /// the slot to the default value explicitly." Counted once per
    /// occurrence regardless of which opcode was emitted.
    pub text_state_op_count: u32,
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
    let (version, header) = detect_version_probe(&mut r)?;

    let width = (frame.right - frame.left).max(0) as u32;
    let height = (frame.bottom - frame.top).max(0) as u32;

    let mut p = PictProbe {
        version,
        frame: frame.into(),
        width,
        height,
        has_launch_stub,
        raster_count: 0,
        indexed_raster_count: 0,
        drawing_count: 0,
        same_shape_count: 0,
        text_count: 0,
        comment_count: 0,
        comments: Vec::new(),
        clip_rgn_count: 0,
        pattern_set_count: 0,
        pix_pattern_set_count: 0,
        compressed_quicktime_count: 0,
        uncompressed_quicktime_count: 0,
        reserved_op_count: 0,
        end_pic_seen: false,
        termination: ProbeTermination::Eof,
        terminated_at: body_offset + r.pos,
        header,
        text_state: PictTextState::fresh_graf_port(),
        text_state_op_count: 0,
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

fn detect_version_probe(r: &mut Reader<'_>) -> Result<(ProbeVersion, Option<PictHeader>)> {
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
            // Same tolerant parse as the decoder — fall back to a
            // raw 24-byte skip when the leading version word isn't
            // FFFE / FFFF so non-canonical headers still produce a
            // valid probe.
            let header = match PictHeader::parse(r) {
                Ok(h) => Some(h),
                Err(_) => {
                    r.pos -= 2;
                    r.skip(24)?;
                    None
                }
            };
            return Ok((ProbeVersion::V2, header));
        }
        if (next >> 8) == 0x01 {
            r.pos -= 1;
            return Ok((ProbeVersion::V1, None));
        }
        return Err(PictError::invalid(format!(
            "unrecognised version stanza after 0x0011: 0x{next:04X}"
        )));
    }
    if v_word == 0x1101 {
        return Ok((ProbeVersion::V1, None));
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
            // Round 236 mirrors the decoder's structured `fontName`
            // capture so probe consumers see the same final-state
            // snapshot without paying the rasterisation cost. See the
            // decoder's `OP_FONT_NAME` arm for the §A-3 layout notes.
            let n = r.read_u16()? as usize;
            if n < 5 {
                return Err(PictError::invalid(format!(
                    "fontName dataLength {n} smaller than the 5-byte minimum"
                )));
            }
            let old_font_id = r.read_i16()?;
            let name_len = r.read_u8()? as usize;
            let remaining = n.saturating_sub(5);
            if name_len > remaining {
                return Err(PictError::invalid(format!(
                    "fontName nameLength {name_len} exceeds remaining {remaining} bytes",
                )));
            }
            let name = r.read_bytes(name_len)?.to_vec();
            r.skip(remaining - name_len)?;
            p.text_state.font_name = Some(PictFontName::new(old_font_id, name));
            p.text_state_op_count += 1;
            Ok(OpStep::Continue)
        }
        OP_LINE_JUSTIFY => {
            let n = r.read_u16()? as usize;
            if n < 8 {
                return Err(PictError::invalid(format!(
                    "lineJustify dataLength {n} smaller than the 8-byte payload",
                )));
            }
            let inter = Fixed(r.read_u32()? as i32);
            let extra = Fixed(r.read_u32()? as i32);
            r.skip(n - 8)?;
            p.text_state.line_justify = Some(PictLineJustify {
                inter_char_spacing: inter,
                total_extra: extra,
            });
            p.text_state_op_count += 1;
            Ok(OpStep::Continue)
        }
        OP_GLYPH_STATE => {
            let n = r.read_u16()? as usize;
            if n < 4 {
                return Err(PictError::invalid(format!(
                    "glyphState dataLength {n} smaller than the 4-byte payload",
                )));
            }
            let outline_preferred = r.read_u8()? != 0;
            let preserve_glyph = r.read_u8()? != 0;
            let fractional_widths = r.read_u8()? != 0;
            let scaling_disabled = r.read_u8()? != 0;
            r.skip(n - 4)?;
            p.text_state.glyph_state = Some(PictGlyphState {
                outline_preferred,
                preserve_glyph,
                fractional_widths,
                scaling_disabled,
            });
            p.text_state_op_count += 1;
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
        // §A-3 Table A-2 text / pen / transfer-mode / highlight state
        // opcodes — round 230 promotes from generic skip-table to
        // dedicated arms so `p.text_state` and `p.text_state_op_count`
        // can be updated in lock-step with the decoder.
        OP_TX_FONT => {
            p.text_state.tx_font = r.read_i16()?;
            p.text_state_op_count += 1;
            Ok(OpStep::Continue)
        }
        OP_TX_FACE => {
            p.text_state.tx_face = crate::state::PictTextFace::from(r.read_u8()?);
            p.text_state_op_count += 1;
            Ok(OpStep::Continue)
        }
        OP_TX_MODE => {
            p.text_state.tx_mode = r.read_i16()?;
            p.text_state_op_count += 1;
            Ok(OpStep::Continue)
        }
        OP_SP_EXTRA => {
            p.text_state.sp_extra = Fixed(r.read_u32()? as i32);
            p.text_state_op_count += 1;
            Ok(OpStep::Continue)
        }
        OP_PN_MODE => {
            p.text_state.pn_mode = r.read_i16()?;
            p.text_state_op_count += 1;
            Ok(OpStep::Continue)
        }
        OP_TX_SIZE => {
            p.text_state.tx_size = r.read_i16()?;
            p.text_state_op_count += 1;
            Ok(OpStep::Continue)
        }
        OP_TX_RATIO => {
            let numer_v = r.read_i16()?;
            let numer_h = r.read_i16()?;
            let denom_v = r.read_i16()?;
            let denom_h = r.read_i16()?;
            p.text_state.tx_ratio = TextRatio {
                numer_v,
                numer_h,
                denom_v,
                denom_h,
            };
            p.text_state_op_count += 1;
            Ok(OpStep::Continue)
        }
        OP_PN_LOC_HFRAC => {
            p.text_state.pn_loc_h_frac = r.read_i16()?;
            p.text_state_op_count += 1;
            Ok(OpStep::Continue)
        }
        OP_CH_EXTRA => {
            p.text_state.ch_extra = r.read_i16()?;
            p.text_state_op_count += 1;
            Ok(OpStep::Continue)
        }
        OP_HILITE_MODE => {
            p.text_state.hilite_mode_flag = true;
            p.text_state_op_count += 1;
            Ok(OpStep::Continue)
        }
        OP_HILITE_COLOR => {
            let rr = r.read_u16()?;
            let gg = r.read_u16()?;
            let bb = r.read_u16()?;
            p.text_state.hilite_color = Some(Rgba::from_rgb16(rr, gg, bb));
            p.text_state.hilite_default = false;
            p.text_state_op_count += 1;
            Ok(OpStep::Continue)
        }
        OP_DEF_HILITE => {
            p.text_state.hilite_default = true;
            p.text_state.hilite_color = None;
            p.text_state_op_count += 1;
            Ok(OpStep::Continue)
        }
        OP_OP_COLOR => {
            let rr = r.read_u16()?;
            let gg = r.read_u16()?;
            let bb = r.read_u16()?;
            p.text_state.op_color = Some(Rgba::from_rgb16(rr, gg, bb));
            p.text_state_op_count += 1;
            Ok(OpStep::Continue)
        }
        OP_SHORT_COMMENT => {
            let kind = r.read_u16()?;
            p.comments.push(PictComment::short(kind));
            p.comment_count += 1;
            Ok(OpStep::Continue)
        }
        OP_LONG_COMMENT => {
            let kind = r.read_u16()?;
            let n = r.read_u16()? as usize;
            let data = r.read_bytes(n)?.to_vec();
            p.comments.push(PictComment::long(kind, data));
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
            let indexed = skip_raster_opcode_v2(r, opcode)?;
            p.raster_count += 1;
            if indexed {
                p.indexed_raster_count += 1;
            }
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
            } else if let Some(skip) = reserved_v2_payload_size(opcode) {
                probe_skip_reserved_v2(r, skip)?;
                p.reserved_op_count += 1;
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

/// Mirror of `decoder::skip_reserved_v2_payload`. Kept local so the
/// probe doesn't depend on the decoder's private surface.
fn probe_skip_reserved_v2(r: &mut Reader<'_>, skip: ReservedV2Skip) -> Result<()> {
    match skip {
        ReservedV2Skip::Fixed(n) => r.skip(n),
        ReservedV2Skip::U16Prefixed => {
            let n = r.read_u16()? as usize;
            r.skip(n)
        }
        ReservedV2Skip::U32Prefixed => {
            let n = r.read_u32()? as usize;
            r.skip(n)
        }
        ReservedV2Skip::PolygonSized => {
            let n = r.read_u16()? as usize;
            if n < 2 {
                return Err(PictError::invalid(format!(
                    "reserved poly size {n} smaller than the 2-byte size word"
                )));
            }
            r.skip(n - 2)
        }
        ReservedV2Skip::RegionSized => {
            let n = r.read_u16()? as usize;
            if n < 2 {
                return Err(PictError::invalid(format!(
                    "reserved rgn size {n} smaller than the 2-byte size word"
                )));
            }
            r.skip(n - 2)
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
        // §A-3 Table A-3 text / pen / font state opcodes — round 230
        // promotes from skip-only to structured capture into
        // `p.text_state` (mirrors the v2 walker arms above). The pen-
        // size / oval-size / origin / fg-color / bg-color opcodes
        // continue to consume their byte payload here (no `PictState`
        // surface on the probe — those are decoder-only state slots).
        0x03 => {
            p.text_state.tx_font = r.read_i16()?;
            p.text_state_op_count += 1;
            Ok(OpStep::Continue)
        }
        0x04 => {
            p.text_state.tx_face = crate::state::PictTextFace::from(r.read_u8()?);
            p.text_state_op_count += 1;
            Ok(OpStep::Continue)
        }
        0x05 => {
            p.text_state.tx_mode = r.read_i16()?;
            p.text_state_op_count += 1;
            Ok(OpStep::Continue)
        }
        0x06 => {
            p.text_state.sp_extra = Fixed(r.read_u32()? as i32);
            p.text_state_op_count += 1;
            Ok(OpStep::Continue)
        }
        0x07 | 0x0B => {
            r.skip(4)?;
            Ok(OpStep::Continue)
        }
        0x08 => {
            p.text_state.pn_mode = r.read_i16()?;
            p.text_state_op_count += 1;
            Ok(OpStep::Continue)
        }
        0x0C => {
            r.skip(4)?;
            Ok(OpStep::Continue)
        }
        0x0D => {
            p.text_state.tx_size = r.read_i16()?;
            p.text_state_op_count += 1;
            Ok(OpStep::Continue)
        }
        0x0E | 0x0F => {
            r.skip(4)?;
            Ok(OpStep::Continue)
        }
        0x10 => {
            // TxRatio: numerator (Point=4) + denominator (Point=4) = 8.
            let numer_v = r.read_i16()?;
            let numer_h = r.read_i16()?;
            let denom_v = r.read_i16()?;
            let denom_h = r.read_i16()?;
            p.text_state.tx_ratio = TextRatio {
                numer_v,
                numer_h,
                denom_v,
                denom_h,
            };
            p.text_state_op_count += 1;
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
        // §A-3 Table A-3 text opcodes — walked past identically to the
        // decoder. Counted as drawing for probe purposes since a "label"
        // qualifies as visible content per QuickDraw.
        0x28 => {
            r.skip(4)?;
            let n = r.read_u8()? as usize;
            r.skip(n)?;
            p.drawing_count += 1;
            Ok(OpStep::Continue)
        }
        0x29 | 0x2A => {
            r.skip(1)?;
            let n = r.read_u8()? as usize;
            r.skip(n)?;
            p.drawing_count += 1;
            Ok(OpStep::Continue)
        }
        0x2B => {
            r.skip(2)?;
            let n = r.read_u8()? as usize;
            r.skip(n)?;
            p.drawing_count += 1;
            Ok(OpStep::Continue)
        }
        0x30..=0x34 | 0x40..=0x44 | 0x50..=0x54 => {
            r.skip(8)?;
            p.drawing_count += 1;
            Ok(OpStep::Continue)
        }
        // §A-3 Table A-3 *Same*-shape opcodes: zero-byte payload, count
        // as a same-shape repeat. Mirrors the v2 same-shape counting
        // convention (`same_shape_count` is the canonical signal that a
        // v1 / v2 stream is making heavy use of the state-machine
        // payload-elision optimisation).
        0x38..=0x3C | 0x48..=0x4C | 0x58..=0x5C => {
            p.same_shape_count += 1;
            Ok(OpStep::Continue)
        }
        0x60..=0x64 => {
            r.skip(8 + 4)?;
            p.drawing_count += 1;
            Ok(OpStep::Continue)
        }
        0x68..=0x6C => {
            // frameSameArc..fillSameArc: 4-byte payload = start + arc.
            r.skip(4)?;
            p.same_shape_count += 1;
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
        // §A-3 Table A-3 lists 0x78..0x7C (frameSamePoly..fillSamePoly)
        // and 0x88..0x8C (frameSameRgn..fillSameRgn) as "(Not yet
        // implemented)" with a 0-byte payload. They never actually
        // appeared in QuickDraw output; we accept them as a no-op so a
        // private-extension PICT carrying one doesn't poison the
        // statistics gathered up to that point.
        0x78..=0x7C | 0x88..=0x8C => {
            p.same_shape_count += 1;
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
            let indexed = skip_raster_opcode_v1(r, opcode)?;
            p.raster_count += 1;
            if indexed {
                p.indexed_raster_count += 1;
            }
            Ok(OpStep::Continue)
        }
        0xA0 => {
            let kind = r.read_u16()?;
            p.comments.push(PictComment::short(kind));
            p.comment_count += 1;
            Ok(OpStep::Continue)
        }
        0xA1 => {
            let kind = r.read_u16()?;
            let n = r.read_u16()? as usize;
            let data = r.read_bytes(n)?.to_vec();
            p.comments.push(PictComment::long(kind, data));
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
///
/// Round 230 promotes every §A-3 state opcode previously listed here to
/// its own arm in `probe_v2_opcode` so the count / structured-value
/// surfaces can be updated. Kept returning `Option<usize>` for future
/// per-opcode fixed-skip arms that don't update [`PictProbe`].
fn fixed_operand_size(_opcode: u16) -> Option<usize> {
    None
}

/// Skip a v2 raster opcode payload (BitsRect / BitsRgn / PackBitsRect
/// / PackBitsRgn / DirectBitsRect / DirectBitsRgn) without decoding
/// pixels. Returns `Err` only on truncation.
/// Skip the body of a v2 raster opcode (`0x0090..=0x009B`). Returns
/// `true` when the opcode used the indexed PixMap variant (rowBytes
/// high-bit set) of `BitsRect` / `BitsRgn` / `PackBitsRect` /
/// `PackBitsRgn`. `DirectBitsRect 0x009A` and `DirectBitsRgn 0x009B`
/// always carry a PixMap (and an explicit baseAddr placeholder per §A-3
/// footnote `§`) but they are *direct*, not indexed — they return `false`.
fn skip_raster_opcode_v2(r: &mut Reader<'_>, opcode: u16) -> Result<bool> {
    let is_direct_pixmap = matches!(opcode, OP_DIRECT_BITS_RECT | OP_DIRECT_BITS_RGN);
    let with_rgn = matches!(opcode, OP_BITS_RGN | OP_PACK_BITS_RGN | OP_DIRECT_BITS_RGN);
    let mut indexed_pixmap = false;
    let row_bytes;
    let height;
    let pack_type;
    let pixel_size_for_indexed;
    if is_direct_pixmap {
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
        height = (bounds.2 as i32 - bounds.0 as i32).max(0) as usize;
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
        pixel_size_for_indexed = 0;
    } else {
        // BitMap-or-indexed-PixMap header. The first word is `rowBytes`;
        // the high bit selects between BitMap (legacy 1-bpp, §A-3
        // footnote `§`) and indexed PixMap (rowBytes-high-bit-set, §A-3
        // Listing A-2).
        let rb_raw = r.read_u16()?;
        if rb_raw & 0x8000 != 0 {
            // Indexed PixMap path. The PixMap (sans baseAddr — Bits /
            // PackBits opcodes don't carry the §A-3 footnote-§ baseAddr
            // placeholder) is 46 bytes (rowBytes already consumed → 44
            // more) followed by an embedded ColorTable.
            indexed_pixmap = true;
            row_bytes = (rb_raw & 0x3FFF) as usize;
            let bounds = r.read_rect()?;
            height = (bounds.2 as i32 - bounds.0 as i32).max(0) as usize;
            let _pm_version = r.read_u16()?;
            pack_type = r.read_u16()?;
            let _pack_size = r.read_u32()?;
            let _h_res = r.read_u32()?;
            let _v_res = r.read_u32()?;
            let _pixel_type = r.read_u16()?;
            let pixel_size = r.read_u16()?;
            let _cmp_count = r.read_u16()?;
            let _cmp_size = r.read_u16()?;
            let _plane_bytes = r.read_u32()?;
            let _pm_table = r.read_u32()?;
            let _pm_reserved = r.read_u32()?;
            // ColorTable: ctSeed(4) + ctFlags(2) + ctSize(2) + entries.
            let _ct_seed = r.read_u32()?;
            let _ct_flags = r.read_i16()?;
            let ct_size = r.read_i16()?;
            if !(0..=255).contains(&ct_size) {
                return Err(PictError::invalid(format!(
                    "raster-opcode indexed ColorTable ctSize out of range: {ct_size}"
                )));
            }
            let n_entries = (ct_size as usize) + 1;
            r.skip(n_entries * 8)?;
            pixel_size_for_indexed = pixel_size;
        } else {
            // Legacy 1-bpp BitMap path.
            row_bytes = rb_raw as usize;
            let bounds = r.read_rect()?;
            height = (bounds.2 as i32 - bounds.0 as i32).max(0) as usize;
            pack_type = 0;
            pixel_size_for_indexed = 0;
        }
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

    // Pixel-data payload.
    match opcode {
        OP_BITS_RECT | OP_BITS_RGN if !indexed_pixmap => {
            // 1-bpp BitMap: raw rows.
            r.skip(row_bytes * height)?;
        }
        OP_BITS_RECT | OP_BITS_RGN => {
            // Indexed PixMap on a `BitsRect` / `BitsRgn` opcode →
            // unpacked rows per §A-3 footnote `§` ("the difference
            // between version 2 and version 1 formats is that the
            // pixel map replaces the bitmap, a color table has been
            // added, and pixData replaces bitData") — `Bits…` is the
            // unpacked family, `PackBits…` is the packed family.
            let _ = pixel_size_for_indexed; // pixel_size resolution
                                            // happens in the decoder.
            r.skip(row_bytes * height)?;
        }
        OP_PACK_BITS_RECT | OP_PACK_BITS_RGN => {
            // Per-row PackBits when row_bytes >= 8, raw rows otherwise.
            // Applies to both BitMap and indexed-PixMap sub-variants of
            // `PackBitsRect` / `PackBitsRgn` per §A-3 "PixData".
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
    Ok(indexed_pixmap)
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
            let height = (bounds.2 as i32 - bounds.0 as i32).max(0) as usize;
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

fn skip_raster_opcode_v1(r: &mut Reader<'_>, opcode: u16) -> Result<bool> {
    // The v1 raster opcodes 0x90/0x91/0x98/0x99/0x9A/0x9B have the
    // exact same byte layout as their v2 counterparts — only the
    // opcode width (1 byte vs 2 bytes) differs. Returns the
    // indexed-PixMap flag the v2 helper detects.
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
