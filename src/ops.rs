//! Round-4 v2 opcode emit helpers (drawing commands + region path +
//! drawing-state).
//!
//! Every helper here builds the **bytes** for one v2 opcode + payload:
//! the caller is expected to assemble those bytes into a v2 stream
//! between the headerOp stanza and `OpEndPic` (use [`PictBuilder`]).
//! The builder also knows about word-alignment between opcodes, which
//! Inside Macintosh §A-3 requires for v2 streams: each opcode is
//! 2-byte aligned relative to the start of the picture record.
//!
//! Opcode codings come from Inside Macintosh: Imaging With QuickDraw
//! §A-3 ("Picture opcodes"). Each verb has a low-byte nibble:
//!
//! | nibble | semantic |
//! |--------|----------|
//! | 0      | frame    |
//! | 1      | paint    |
//! | 2      | erase    |
//! | 3      | invert   |
//! | 4      | fill     |
//!
//! and a high-byte nibble selecting the shape:
//!
//! | nibble | shape       |
//! |--------|-------------|
//! | 0x20   | line family |
//! | 0x30   | rectangle   |
//! | 0x40   | round-rect  |
//! | 0x50   | oval        |
//! | 0x60   | arc         |
//! | 0x70   | polygon     |
//! | 0x80   | region      |
//!
//! Same-as-last (`+0x08`) variants reuse the previously emitted shape
//! geometry — see [`build_same_rect_op`] / [`build_same_round_rect_op`]
//! / [`build_same_oval_op`] / [`build_same_arc_op`] (round 401). They
//! save 8 bytes per repeat by carrying no rectangle.

use crate::encoder::build_clip_rgn_rect;
use crate::error::{PictError, Result};
use crate::header::PictHeader;
use crate::opcodes::*;
use crate::state::RectI32;

// ---------------------------------------------------------------------------
// Drawing-verb selector.
// ---------------------------------------------------------------------------

/// Verb applied to a shape opcode.
///
/// Maps directly to the low-byte nibble of the opcode word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    /// 1-pixel outline at the current foreground colour.
    Frame,
    /// Solid fill at the current foreground colour.
    Paint,
    /// Solid fill at the current background colour.
    Erase,
    /// XOR-invert every pixel covered.
    Invert,
    /// Solid fill at the current foreground colour. The round-8
    /// decoder honours the current `FillPat` pattern slot for this
    /// verb — see `state::Pattern` and `PictBuilder::fill_pattern`.
    Fill,
}

impl Verb {
    /// Low-byte nibble (`0..=4`).
    pub fn nibble(self) -> u16 {
        match self {
            Verb::Frame => 0,
            Verb::Paint => 1,
            Verb::Erase => 2,
            Verb::Invert => 3,
            Verb::Fill => 4,
        }
    }
}

// ---------------------------------------------------------------------------
// Geometry-opcode emit.
// ---------------------------------------------------------------------------

/// Build a v2 `Line` opcode (`0x0020`): a line from (h0, v0) to (h1, v1)
/// in picture-frame coordinates. Updates the QuickDraw pen position to
/// (h1, v1) when interpreted by the decoder.
pub fn build_line(h0: i16, v0: i16, h1: i16, v1: i16) -> Vec<u8> {
    let mut buf = Vec::with_capacity(10);
    write_u16(&mut buf, OP_LINE);
    // QuickDraw stores points as (v, h) — vertical first.
    write_i16(&mut buf, v0);
    write_i16(&mut buf, h0);
    write_i16(&mut buf, v1);
    write_i16(&mut buf, h1);
    buf
}

/// Build a v2 `LineFrom` opcode (`0x0021`): line from current pen to
/// (h1, v1).
pub fn build_line_from(h1: i16, v1: i16) -> Vec<u8> {
    let mut buf = Vec::with_capacity(6);
    write_u16(&mut buf, OP_LINE_FROM);
    write_i16(&mut buf, v1);
    write_i16(&mut buf, h1);
    buf
}

/// Build a v2 `ShortLine` opcode (`$0022`) per Inside Macintosh:
/// Imaging With QuickDraw §A-3 Table A-2: `pnLoc` (Point, stored
/// `(v, h)`), then `dh` and `dv` as SignedBytes (−128..127). Moves the
/// pen to `(h, v)` and draws to `(h + dh, v + dv)`, leaving the pen at
/// the line's end — 4 bytes shorter than the equivalent `Line`.
pub fn build_short_line(h: i16, v: i16, dh: i8, dv: i8) -> Vec<u8> {
    let mut buf = Vec::with_capacity(8);
    write_u16(&mut buf, OP_SHORT_LINE);
    write_i16(&mut buf, v);
    write_i16(&mut buf, h);
    buf.push(dh as u8);
    buf.push(dv as u8);
    buf
}

/// Build a v2 `ShortLineFrom` opcode (`$0023`) per §A-3 Table A-2:
/// `dh` and `dv` as SignedBytes (−128..127). Draws from the current
/// pen to `pen + (dh, dv)`, leaving the pen at the line's end — the
/// most compact polyline continuation (2 payload bytes).
pub fn build_short_line_from(dh: i8, dv: i8) -> Vec<u8> {
    let mut buf = Vec::with_capacity(4);
    write_u16(&mut buf, OP_SHORT_LINE_FROM);
    buf.push(dh as u8);
    buf.push(dv as u8);
    buf
}

/// Build an `Origin` opcode (`$000C`) per §A-3 Table A-2: `dh`, `dv`
/// (Integer each) — the delta applied to the picture's coordinate
/// origin.
///
/// Per the `SetOrigin` discussion in Inside Macintosh: Imaging With
/// QuickDraw §2 "Basic QuickDraw" (book pages 2-23 f.), *increasing*
/// the origin coordinates makes subsequently drawn shapes land
/// *up / left* on the canvas: the port's upper-left corner takes the
/// new origin coordinates, so a shape at unchanged coordinates sits
/// closer to (or past) that corner.
pub fn build_origin(dh: i16, dv: i16) -> Vec<u8> {
    let mut buf = Vec::with_capacity(6);
    write_u16(&mut buf, OP_ORIGIN);
    write_i16(&mut buf, dh);
    write_i16(&mut buf, dv);
    buf
}

/// Build a rect-family opcode for the chosen [`Verb`] over the rect
/// `(top, left, bottom, right)` in picture-frame coords. Opcodes are
/// `0x0030..=0x0034`.
pub fn build_rect_op(verb: Verb, top: i16, left: i16, bottom: i16, right: i16) -> Vec<u8> {
    let mut buf = Vec::with_capacity(10);
    write_u16(&mut buf, 0x0030 | verb.nibble());
    write_i16(&mut buf, top);
    write_i16(&mut buf, left);
    write_i16(&mut buf, bottom);
    write_i16(&mut buf, right);
    buf
}

/// Build a round-rect opcode (`0x0040..=0x0044`) over the rect. The
/// caller must have already emitted an `OvSize` opcode (see
/// [`build_oval_size`]) to set the corner radius.
pub fn build_round_rect_op(verb: Verb, top: i16, left: i16, bottom: i16, right: i16) -> Vec<u8> {
    let mut buf = Vec::with_capacity(10);
    write_u16(&mut buf, 0x0040 | verb.nibble());
    write_i16(&mut buf, top);
    write_i16(&mut buf, left);
    write_i16(&mut buf, bottom);
    write_i16(&mut buf, right);
    buf
}

/// Build an oval opcode (`0x0050..=0x0054`) inscribed in `(top, left,
/// bottom, right)`.
pub fn build_oval_op(verb: Verb, top: i16, left: i16, bottom: i16, right: i16) -> Vec<u8> {
    let mut buf = Vec::with_capacity(10);
    write_u16(&mut buf, 0x0050 | verb.nibble());
    write_i16(&mut buf, top);
    write_i16(&mut buf, left);
    write_i16(&mut buf, bottom);
    write_i16(&mut buf, right);
    buf
}

/// Build an arc opcode (`0x0060..=0x0064`). `start_angle` is the
/// starting angle in degrees clockwise from 12 o'clock; `arc_angle` is
/// the sweep (positive = clockwise) per Inside Macintosh §3 ("Drawing
/// arcs and wedges").
pub fn build_arc_op(
    verb: Verb,
    top: i16,
    left: i16,
    bottom: i16,
    right: i16,
    start_angle: i16,
    arc_angle: i16,
) -> Vec<u8> {
    let mut buf = Vec::with_capacity(14);
    write_u16(&mut buf, 0x0060 | verb.nibble());
    write_i16(&mut buf, top);
    write_i16(&mut buf, left);
    write_i16(&mut buf, bottom);
    write_i16(&mut buf, right);
    write_i16(&mut buf, start_angle);
    write_i16(&mut buf, arc_angle);
    buf
}

/// Build a same-rect opcode (`0x0038..=0x003C`) per Inside Macintosh:
/// Imaging With QuickDraw §A-3 Table A-2: apply the chosen [`Verb`] to
/// the rectangle carried by the **previous** rect-family opcode. No
/// payload — the repeat saves the 8 rectangle bytes. A same-rect
/// opcode with no prior rect-family opcode in the stream is a no-op on
/// decode.
pub fn build_same_rect_op(verb: Verb) -> Vec<u8> {
    let mut buf = Vec::with_capacity(2);
    write_u16(&mut buf, 0x0038 | verb.nibble());
    buf
}

/// Build a same-round-rect opcode (`0x0048..=0x004C`) per §A-3 Table
/// A-2: apply the chosen [`Verb`] to the rectangle carried by the
/// previous round-rect-family opcode. No payload; the corner radius is
/// still the current `OvSize` state.
pub fn build_same_round_rect_op(verb: Verb) -> Vec<u8> {
    let mut buf = Vec::with_capacity(2);
    write_u16(&mut buf, 0x0048 | verb.nibble());
    buf
}

/// Build a same-oval opcode (`0x0058..=0x005C`) per §A-3 Table A-2:
/// apply the chosen [`Verb`] to the rectangle carried by the previous
/// oval-family opcode. No payload.
pub fn build_same_oval_op(verb: Verb) -> Vec<u8> {
    let mut buf = Vec::with_capacity(2);
    write_u16(&mut buf, 0x0058 | verb.nibble());
    buf
}

/// Build a same-arc opcode (`0x0068..=0x006C`) per §A-3 Table A-2:
/// apply the chosen [`Verb`] over the enclosing rectangle carried by
/// the previous arc-family opcode. Unlike the other same-shape ops it
/// carries a 4-byte payload: fresh `startAngle` / `arcAngle` words, so
/// a fan of wedges can share one rectangle.
pub fn build_same_arc_op(verb: Verb, start_angle: i16, arc_angle: i16) -> Vec<u8> {
    let mut buf = Vec::with_capacity(6);
    write_u16(&mut buf, 0x0068 | verb.nibble());
    write_i16(&mut buf, start_angle);
    write_i16(&mut buf, arc_angle);
    buf
}

/// Build a polygon opcode (`0x0070..=0x0074`).
///
/// `vertices` is a slice of (h, v) picture-frame points; we emit them
/// in the on-disk (v, h) order. The polygon record also carries a
/// 2-byte `polySize` (total bytes, including this size word) and an
/// 8-byte bounding rectangle (auto-derived from the vertex extents)
/// per Inside Macintosh §3 ("Polygon"). Returns `InvalidData` if there
/// are fewer than 2 vertices.
pub fn build_poly_op(verb: Verb, vertices: &[(i16, i16)]) -> Result<Vec<u8>> {
    if vertices.len() < 2 {
        return Err(PictError::invalid(format!(
            "build_poly_op needs ≥ 2 vertices, got {}",
            vertices.len()
        )));
    }
    let n = vertices.len();
    // polySize = 2 (size) + 8 (bbox) + n*4 (verts) = 10 + 4n.
    let poly_size = 10 + 4 * n;
    if poly_size > u16::MAX as usize {
        return Err(PictError::invalid(format!(
            "polygon too large: {n} vertices ⇒ {poly_size} bytes > 65 535"
        )));
    }

    // Derive bbox from vertex extents.
    let (mut min_h, mut min_v) = vertices[0];
    let (mut max_h, mut max_v) = vertices[0];
    for &(h, v) in &vertices[1..] {
        min_h = min_h.min(h);
        min_v = min_v.min(v);
        max_h = max_h.max(h);
        max_v = max_v.max(v);
    }

    let mut buf = Vec::with_capacity(2 + poly_size);
    write_u16(&mut buf, 0x0070 | verb.nibble());
    write_u16(&mut buf, poly_size as u16);
    // bbox (top, left, bottom, right) = (min_v, min_h, max_v + 1, max_h + 1).
    // QuickDraw rect bottom/right are exclusive; we pad by 1 so the
    // bbox encloses the polygon line ink.
    write_i16(&mut buf, min_v);
    write_i16(&mut buf, min_h);
    write_i16(&mut buf, max_v.saturating_add(1));
    write_i16(&mut buf, max_h.saturating_add(1));
    for &(h, v) in vertices {
        write_i16(&mut buf, v);
        write_i16(&mut buf, h);
    }
    Ok(buf)
}

// ---------------------------------------------------------------------------
// Region path emit.
// ---------------------------------------------------------------------------

/// Build a v2 region opcode (`0x0080..=0x0084`) carrying a rectangular
/// region.
///
/// A rectangular region is the simplest form: `rgnSize == 10`, no
/// inversion data. Total opcode payload is 12 bytes (opcode word +
/// rgnSize + 8-byte bbox). See Inside Macintosh §2 ("QuickDraw
/// Drawing").
pub fn build_rgn_rect_op(verb: Verb, top: i16, left: i16, bottom: i16, right: i16) -> Vec<u8> {
    let mut buf = Vec::with_capacity(14);
    write_u16(&mut buf, 0x0080 | verb.nibble());
    write_u16(&mut buf, 10);
    write_i16(&mut buf, top);
    write_i16(&mut buf, left);
    write_i16(&mut buf, bottom);
    write_i16(&mut buf, right);
    buf
}

/// Build a v2 region opcode (`0x0080..=0x0084`) carrying an
/// inversion-encoded region.
///
/// `bbox_top..bbox_bottom × bbox_left..bbox_right` is the region's
/// bounding rectangle.
///
/// `scanlines` is a slice of (`y`, `x_pairs`) records. Each record:
///
/// * `y` — picture-frame scanline at which the running column-flip
///   parity changes. Records must be in **strictly ascending** y
///   order, all in `[bbox_top, bbox_bottom)`.
/// * `x_pairs` — the picture-frame x columns at which to toggle
///   membership for this row and all subsequent rows until the next
///   record overrides. Pairs must be **strictly ascending** within a
///   record; the decoder interprets each pair `(x0, x1)` as columns
///   `[x0, x1)` newly inside.
///
/// The encoder packages the inversion data per Inside Macintosh §2
/// ("Region"):
///
/// ```text
/// for each scanline record:
///   i16 y
///   i16 x_pairs[..]
///   i16 0x7FFF                 // line terminator
/// i16 0x7FFF                   // region terminator
/// ```
///
/// The first 10 bytes are header (rgnSize + bbox); `rgnSize` includes
/// the size word itself.
///
/// Returns `InvalidData` if any constraint is violated or the total
/// region size exceeds `u16::MAX`.
pub fn build_rgn_inverted_op(
    verb: Verb,
    bbox_top: i16,
    bbox_left: i16,
    bbox_bottom: i16,
    bbox_right: i16,
    scanlines: &[(i16, &[i16])],
) -> Result<Vec<u8>> {
    if bbox_bottom <= bbox_top || bbox_right <= bbox_left {
        return Err(PictError::invalid(format!(
            "build_rgn_inverted_op: degenerate bbox ({bbox_top},{bbox_left})→({bbox_bottom},{bbox_right})"
        )));
    }

    // Validate ordering.
    let mut prev_y: Option<i16> = None;
    for (y, xs) in scanlines {
        if *y < bbox_top || *y >= bbox_bottom {
            return Err(PictError::invalid(format!(
                "build_rgn_inverted_op: y={y} out of bbox [{bbox_top}, {bbox_bottom})"
            )));
        }
        if let Some(p) = prev_y {
            if *y <= p {
                return Err(PictError::invalid(format!(
                    "build_rgn_inverted_op: scanlines not strictly ascending: {p} ≥ {y}"
                )));
            }
        }
        prev_y = Some(*y);
        let mut prev_x: Option<i16> = None;
        for x in *xs {
            if *x < bbox_left || *x > bbox_right {
                return Err(PictError::invalid(format!(
                    "build_rgn_inverted_op: x={x} out of bbox [{bbox_left}, {bbox_right}]"
                )));
            }
            if let Some(p) = prev_x {
                if *x <= p {
                    return Err(PictError::invalid(format!(
                        "build_rgn_inverted_op: x's not strictly ascending: {p} ≥ {x}"
                    )));
                }
            }
            prev_x = Some(*x);
        }
    }

    // Compute payload size.
    // Inversion data: per scanline 2 (y) + 2*n_x + 2 (line terminator).
    // Then 2 bytes for the region terminator (0x7FFF as the y value of
    // a "phantom" scanline).
    let mut inv_bytes = 0usize;
    for (_, xs) in scanlines {
        inv_bytes += 2 + 2 * xs.len() + 2;
    }
    inv_bytes += 2; // region terminator

    let rgn_size = 10 + inv_bytes;
    if rgn_size > u16::MAX as usize {
        return Err(PictError::invalid(format!(
            "region payload {rgn_size} exceeds 65 535-byte rgnSize limit"
        )));
    }

    let mut buf = Vec::with_capacity(2 + rgn_size);
    write_u16(&mut buf, 0x0080 | verb.nibble());
    write_u16(&mut buf, rgn_size as u16);
    write_i16(&mut buf, bbox_top);
    write_i16(&mut buf, bbox_left);
    write_i16(&mut buf, bbox_bottom);
    write_i16(&mut buf, bbox_right);
    for (y, xs) in scanlines {
        write_i16(&mut buf, *y);
        for x in *xs {
            write_i16(&mut buf, *x);
        }
        // 0x7FFF — line terminator (Inside Macintosh: the magic
        // sentinel value chosen because it's the maximum positive i16,
        // unlikely to collide with a real coord).
        write_i16(&mut buf, 0x7FFF);
    }
    write_i16(&mut buf, 0x7FFF);
    Ok(buf)
}

// ---------------------------------------------------------------------------
// Drawing-state opcode emit.
// ---------------------------------------------------------------------------

/// Build an `RGBFgCol` (`0x001A`) opcode setting the foreground colour.
/// Each component is an 8-bit value, expanded to 16 bits on the wire by
/// duplicating the byte (so 0xAB becomes 0xABAB) per QuickDraw's
/// `RGBColor` convention (high byte = colour, low byte = resolution
/// padding).
pub fn build_rgb_fg_col(r: u8, g: u8, b: u8) -> Vec<u8> {
    let mut buf = Vec::with_capacity(8);
    write_u16(&mut buf, OP_RGB_FG_COL);
    write_u16(&mut buf, expand_8_to_16(r));
    write_u16(&mut buf, expand_8_to_16(g));
    write_u16(&mut buf, expand_8_to_16(b));
    buf
}

/// Build an `RGBBkCol` (`0x001B`) opcode setting the background colour.
pub fn build_rgb_bk_col(r: u8, g: u8, b: u8) -> Vec<u8> {
    let mut buf = Vec::with_capacity(8);
    write_u16(&mut buf, OP_RGB_BK_COL);
    write_u16(&mut buf, expand_8_to_16(r));
    write_u16(&mut buf, expand_8_to_16(g));
    write_u16(&mut buf, expand_8_to_16(b));
    buf
}

/// Build a `PnSize` (`0x0007`) opcode — pen size.
pub fn build_pn_size(h: i16, v: i16) -> Vec<u8> {
    let mut buf = Vec::with_capacity(6);
    write_u16(&mut buf, OP_PN_SIZE);
    write_i16(&mut buf, v);
    write_i16(&mut buf, h);
    buf
}

/// Build an `OvSize` (`0x000B`) opcode — round-rect corner-oval size.
pub fn build_oval_size(h: i16, v: i16) -> Vec<u8> {
    let mut buf = Vec::with_capacity(6);
    write_u16(&mut buf, OP_OV_SIZE);
    write_i16(&mut buf, v);
    write_i16(&mut buf, h);
    buf
}

/// Build a `PnPat` (`0x0009`) opcode carrying an 8-byte monochrome
/// pen pattern. Honoured by the decoder's frame / paint verbs (Inside
/// Macintosh §A-3).
pub fn build_pn_pat(pattern: [u8; 8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(10);
    write_u16(&mut buf, OP_PN_PAT);
    buf.extend_from_slice(&pattern);
    buf
}

/// Build a `BkPat` (`0x0002`) opcode carrying an 8-byte monochrome
/// background pattern. Honoured by erase verbs.
pub fn build_bk_pat(pattern: [u8; 8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(10);
    write_u16(&mut buf, OP_BK_PAT);
    buf.extend_from_slice(&pattern);
    buf
}

/// Build a `FillPat` (`0x000A`) opcode carrying an 8-byte monochrome
/// fill pattern. Honoured by fill verbs (low nibble `4`).
pub fn build_fill_pat(pattern: [u8; 8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(10);
    write_u16(&mut buf, OP_FILL_PAT);
    buf.extend_from_slice(&pattern);
    buf
}

/// Build a `TxFont` (`$0003`) opcode carrying a 2-byte `Integer` font
/// number per Inside Macintosh: Imaging With QuickDraw §A-3 Table A-2.
pub fn build_tx_font(font: i16) -> Vec<u8> {
    let mut buf = Vec::with_capacity(4);
    write_u16(&mut buf, OP_TX_FONT);
    write_i16(&mut buf, font);
    buf
}

/// Build a `TxFace` (`$0004`) opcode carrying a 1-byte font-style byte
/// (the classic Mac `Style` bitfield) per §A-3 Table A-2.
///
/// §A-3 Table A-2 lists the operand as 1 byte (`0..255`). The opcode is
/// 2 bytes — caller is responsible for the word-alignment pad expected
/// before the next v2 opcode; `PictBuilder` handles it automatically.
pub fn build_tx_face(face: u8) -> Vec<u8> {
    let mut buf = Vec::with_capacity(3);
    write_u16(&mut buf, OP_TX_FACE);
    buf.push(face);
    buf
}

/// Build a `TxMode` (`$0005`) opcode carrying a 2-byte `Integer`
/// source-mode value (`srcCopy = 0`, `srcOr = 1`, …) per §A-3 Table A-2.
pub fn build_tx_mode(mode: i16) -> Vec<u8> {
    let mut buf = Vec::with_capacity(4);
    write_u16(&mut buf, OP_TX_MODE);
    write_i16(&mut buf, mode);
    buf
}

/// Build a `SpExtra` (`$0006`) opcode carrying a 4-byte `Fixed`
/// extra-space value per §A-3 Table A-2.
pub fn build_sp_extra(extra: i32) -> Vec<u8> {
    let mut buf = Vec::with_capacity(6);
    write_u16(&mut buf, OP_SP_EXTRA);
    buf.extend_from_slice(&extra.to_be_bytes());
    buf
}

/// Build a `PnMode` (`$0008`) opcode carrying a 2-byte `Integer`
/// pen-mode value (same numeric catalog as `TxMode`) per §A-3
/// Table A-2.
pub fn build_pn_mode(mode: i16) -> Vec<u8> {
    let mut buf = Vec::with_capacity(4);
    write_u16(&mut buf, OP_PN_MODE);
    write_i16(&mut buf, mode);
    buf
}

/// Build a `TxSize` (`$000D`) opcode carrying a 2-byte `Integer` text
/// size in points per §A-3 Table A-2.
pub fn build_tx_size(size: i16) -> Vec<u8> {
    let mut buf = Vec::with_capacity(4);
    write_u16(&mut buf, OP_TX_SIZE);
    write_i16(&mut buf, size);
    buf
}

/// Build a `TxRatio` (`$0010`) opcode carrying an 8-byte payload —
/// numerator (Point) + denominator (Point) per §A-3 Table A-2. Each
/// `Point` is `(v: i16, h: i16)` on disk.
pub fn build_tx_ratio(numer_v: i16, numer_h: i16, denom_v: i16, denom_h: i16) -> Vec<u8> {
    let mut buf = Vec::with_capacity(10);
    write_u16(&mut buf, OP_TX_RATIO);
    write_i16(&mut buf, numer_v);
    write_i16(&mut buf, numer_h);
    write_i16(&mut buf, denom_v);
    write_i16(&mut buf, denom_h);
    buf
}

/// Build a `PnLocHFrac` (`$0015`) opcode carrying a 2-byte `Integer`
/// (low word of `Fixed`) per §A-3 Table A-2.
pub fn build_pn_loc_h_frac(frac: i16) -> Vec<u8> {
    let mut buf = Vec::with_capacity(4);
    write_u16(&mut buf, OP_PN_LOC_HFRAC);
    write_i16(&mut buf, frac);
    buf
}

/// Build a `ChExtra` (`$0016`) opcode carrying a 2-byte `Integer`
/// per-character extra-width adjustment per §A-3 Table A-2.
pub fn build_ch_extra(extra: i16) -> Vec<u8> {
    let mut buf = Vec::with_capacity(4);
    write_u16(&mut buf, OP_CH_EXTRA);
    write_i16(&mut buf, extra);
    buf
}

/// Build a `HiliteMode` (`$001C`) opcode — a 0-byte-payload "flag"
/// per §A-3 Table A-2 indicating the next drawing operation should use
/// the highlight mode.
pub fn build_hilite_mode() -> Vec<u8> {
    let mut buf = Vec::with_capacity(2);
    write_u16(&mut buf, OP_HILITE_MODE);
    buf
}

/// Build a `HiliteColor` (`$001D`) opcode carrying a 6-byte `RGBColor`
/// per §A-3 Table A-2. The 8-bit input is replicated across both bytes
/// of each 16-bit-per-channel on-disk component so the decoder's
/// `Rgba::from_rgb16` high-byte selection round-trips bit-exact.
pub fn build_hilite_color(r: u8, g: u8, b: u8) -> Vec<u8> {
    let mut buf = Vec::with_capacity(8);
    write_u16(&mut buf, OP_HILITE_COLOR);
    write_u16(&mut buf, expand_8_to_16(r));
    write_u16(&mut buf, expand_8_to_16(g));
    write_u16(&mut buf, expand_8_to_16(b));
    buf
}

/// Build a `DefHilite` (`$001E`) opcode — a 0-byte-payload reset that
/// switches subsequent draws back to the system-default highlight
/// colour per §A-3 Table A-2.
pub fn build_def_hilite() -> Vec<u8> {
    let mut buf = Vec::with_capacity(2);
    write_u16(&mut buf, OP_DEF_HILITE);
    buf
}

/// Build an `OpColor` (`$001F`) opcode carrying a 6-byte `RGBColor`
/// per §A-3 Table A-2. The opcode supplies the colour parameter for
/// arithmetic transfer modes (`blend`, `addPin`, etc.) — round 230
/// captures it as state; the arithmetic transfer modes themselves are
/// a future round.
pub fn build_op_color(r: u8, g: u8, b: u8) -> Vec<u8> {
    let mut buf = Vec::with_capacity(8);
    write_u16(&mut buf, OP_OP_COLOR);
    write_u16(&mut buf, expand_8_to_16(r));
    write_u16(&mut buf, expand_8_to_16(g));
    write_u16(&mut buf, expand_8_to_16(b));
    buf
}

/// Build an `FgColor` (`$000E`) opcode carrying a classic-QuickDraw
/// colour code (Long) per Inside Macintosh: Imaging With QuickDraw
/// §A-3 Table A-2 / Table A-3. This is the pre-Color-QuickDraw
/// eight-colour planar model (the v1 way to select ink); v2 streams
/// normally use `RGBFgCol` (`$001A`, [`build_rgb_fg_col`]) instead,
/// but the decoder honours `$000E` in both versions.
pub fn build_fg_color_code(code: u32) -> Vec<u8> {
    let mut buf = Vec::with_capacity(6);
    write_u16(&mut buf, OP_FG_COLOR);
    buf.extend_from_slice(&code.to_be_bytes());
    buf
}

/// Build a `BkColor` (`$000F`) opcode carrying a classic-QuickDraw
/// colour code (Long) — the background counterpart of
/// [`build_fg_color_code`].
pub fn build_bk_color_code(code: u32) -> Vec<u8> {
    let mut buf = Vec::with_capacity(6);
    write_u16(&mut buf, OP_BG_COLOR);
    buf.extend_from_slice(&code.to_be_bytes());
    buf
}

/// Build a `fontName` (`$002C`) opcode carrying the producer's
/// `oldFontID` + font-name bytes per Inside Macintosh: Imaging With
/// QuickDraw §A-3 Table A-2 footnote `*`.
///
/// On-disk layout (after the 2-byte opcode):
///
/// ```text
/// dataLength (Integer)  = 5 + name.len()  // includes itself
/// oldFontID  (Integer)  = the producer's TxFont pairing
/// nameLength (Byte)     = name.len()
/// name       (Bytes)    = the raw font-name bytes
/// ```
///
/// `dataLength` is written inclusive of itself, matching the decoder
/// arm (round 236) and the §A-3 "Additional data size" column of
/// `5 + nameLength`. The 1-byte `nameLength` field caps the font name
/// at 255 bytes; an oversize input returns
/// [`PictError::InvalidData`].
pub fn build_font_name(old_font_id: i16, name: &[u8]) -> Result<Vec<u8>> {
    let name_len: u8 = name.len().try_into().map_err(|_| {
        PictError::invalid(format!(
            "fontName name length {} exceeds the u8 (255-byte) name-length field",
            name.len()
        ))
    })?;
    let data_length: u16 = (5usize + name.len()).try_into().map_err(|_| {
        PictError::invalid(format!(
            "fontName total record length {} exceeds the u16 dataLength field",
            5usize + name.len()
        ))
    })?;
    let mut buf = Vec::with_capacity(2 + 5 + name.len());
    write_u16(&mut buf, OP_FONT_NAME);
    write_u16(&mut buf, data_length);
    write_i16(&mut buf, old_font_id);
    buf.push(name_len);
    buf.extend_from_slice(name);
    Ok(buf)
}

/// Build a `lineJustify` (`$002D`) opcode carrying the Script-Manager
/// line-layout state per Inside Macintosh: Imaging With QuickDraw §A-3
/// Table A-2 footnote `†`.
///
/// On-disk layout (after the 2-byte opcode):
///
/// ```text
/// dataLength             (Integer) = 8   // bytes after itself
/// intercharacter spacing (Fixed)         // 16.16 i32
/// total extra space      (Fixed)         // 16.16 i32
/// ```
///
/// `dataLength` is fixed at 8 (footnote `†`: *"the field's data length,
/// which should always be 8 bytes"*) and **excludes itself** — the
/// total additional-data column in Table A-2 is 10 = 2 (length) + 8
/// (two Fixed values).
pub fn build_line_justify(inter_char_spacing: i32, total_extra: i32) -> Vec<u8> {
    let mut buf = Vec::with_capacity(2 + 10);
    write_u16(&mut buf, OP_LINE_JUSTIFY);
    write_u16(&mut buf, 8);
    buf.extend_from_slice(&inter_char_spacing.to_be_bytes());
    buf.extend_from_slice(&total_extra.to_be_bytes());
    buf
}

/// Build a `glyphState` (`$002E`) opcode carrying the four
/// preserved-glyph Booleans per Inside Macintosh: Imaging With
/// QuickDraw §A-3 Table A-2 row `$002E`.
///
/// On-disk layout (after the 2-byte opcode):
///
/// ```text
/// dataLength         (Integer) = 6
/// outline_preferred  (Byte)   // 0 = false, non-zero = true
/// preserve_glyph     (Byte)
/// fractional_widths  (Byte)
/// scaling_disabled   (Byte)
/// pad                (2 bytes of 0)
/// ```
///
/// `dataLength` is set to 6 — the four 1-byte Booleans plus two pad
/// bytes — so that the §A-3 Table A-2 "Additional data size" column of
/// `8` (2 length + 6 payload) is honoured. The pad keeps the next v2
/// opcode word-aligned without relying on the builder's
/// align-on-push pass for record-internal padding.
pub fn build_glyph_state(
    outline_preferred: bool,
    preserve_glyph: bool,
    fractional_widths: bool,
    scaling_disabled: bool,
) -> Vec<u8> {
    let mut buf = Vec::with_capacity(2 + 8);
    write_u16(&mut buf, OP_GLYPH_STATE);
    write_u16(&mut buf, 6);
    buf.push(outline_preferred as u8);
    buf.push(preserve_glyph as u8);
    buf.push(fractional_widths as u8);
    buf.push(scaling_disabled as u8);
    buf.push(0);
    buf.push(0);
    buf
}

/// Build a `LongText` (`$0028`) opcode per Inside Macintosh: Imaging
/// With QuickDraw §A-3 Table A-2: `txLoc` (Point), `count` (0..255),
/// then `count` glyph bytes.
///
/// `txLoc` establishes the absolute text-drawing baseline pen. The
/// on-disk Point order is `(v, h)` — vertical first — while this
/// function takes the crate-conventional `(h, v)` argument order,
/// matching the decoder's `text_pen` tuple. Returns
/// [`PictError::InvalidData`] when `text.len()` overflows the 1-byte
/// `count` field (§A-3 caps a single text opcode at 255 glyph bytes;
/// longer runs must split across multiple opcodes, e.g. continuing
/// with [`build_dh_text`]).
pub fn build_long_text(h: i16, v: i16, text: &[u8]) -> Result<Vec<u8>> {
    let count = text_count(text, "LongText")?;
    let mut buf = Vec::with_capacity(2 + 5 + text.len());
    write_u16(&mut buf, OP_LONG_TEXT);
    write_i16(&mut buf, v);
    write_i16(&mut buf, h);
    buf.push(count);
    buf.extend_from_slice(text);
    Ok(buf)
}

/// Build a `DHText` (`$0029`) opcode per §A-3 Table A-2: `dh`
/// (0..255), `count` (0..255), then `count` glyph bytes.
///
/// Advances the running text pen rightward by `dh` relative to the
/// position the previous text opcode left (with no prior `LongText`
/// the pen advances from the graphics origin). Returns
/// [`PictError::InvalidData`] on a text run longer than 255 bytes.
pub fn build_dh_text(dh: u8, text: &[u8]) -> Result<Vec<u8>> {
    let count = text_count(text, "DHText")?;
    let mut buf = Vec::with_capacity(2 + 2 + text.len());
    write_u16(&mut buf, OP_DH_TEXT);
    buf.push(dh);
    buf.push(count);
    buf.extend_from_slice(text);
    Ok(buf)
}

/// Build a `DVText` (`$002A`) opcode per §A-3 Table A-2: `dv`
/// (0..255), `count` (0..255), then `count` glyph bytes.
///
/// Advances the running text pen downward by `dv` relative to the
/// position the previous text opcode left. Returns
/// [`PictError::InvalidData`] on a text run longer than 255 bytes.
pub fn build_dv_text(dv: u8, text: &[u8]) -> Result<Vec<u8>> {
    let count = text_count(text, "DVText")?;
    let mut buf = Vec::with_capacity(2 + 2 + text.len());
    write_u16(&mut buf, OP_DV_TEXT);
    buf.push(dv);
    buf.push(count);
    buf.extend_from_slice(text);
    Ok(buf)
}

/// Build a `DHDVText` (`$002B`) opcode per §A-3 Table A-2: `dh`
/// (0..255), `dv` (0..255), `count` (0..255), then `count` glyph
/// bytes.
///
/// Advances the running text pen by both deltas relative to the
/// position the previous text opcode left. Returns
/// [`PictError::InvalidData`] on a text run longer than 255 bytes.
pub fn build_dhdv_text(dh: u8, dv: u8, text: &[u8]) -> Result<Vec<u8>> {
    let count = text_count(text, "DHDVText")?;
    let mut buf = Vec::with_capacity(2 + 3 + text.len());
    write_u16(&mut buf, OP_DHDV_TEXT);
    buf.push(dh);
    buf.push(dv);
    buf.push(count);
    buf.extend_from_slice(text);
    Ok(buf)
}

/// Shared 1-byte `count` field validation for the four §A-3 text
/// opcodes.
fn text_count(text: &[u8], op_name: &str) -> Result<u8> {
    text.len().try_into().map_err(|_| {
        PictError::invalid(format!(
            "{op_name} text run of {} bytes exceeds the u8 (255-byte) count field",
            text.len()
        ))
    })
}

/// Build a v2 `ShortComment` (`$00A0`) opcode carrying a 2-byte `Kind`
/// integer per Inside Macintosh: Imaging With QuickDraw §A-3 Table A-2.
///
/// `ShortComment` records carry metadata only — they don't influence
/// the drawing-state machine — so the encoder simply writes the opcode
/// + kind word with no further payload.
pub fn build_short_comment(kind: u16) -> Vec<u8> {
    let mut buf = Vec::with_capacity(4);
    write_u16(&mut buf, OP_SHORT_COMMENT);
    write_u16(&mut buf, kind);
    buf
}

/// Build a v2 `LongComment` (`$00A1`) opcode carrying a 2-byte `Kind`
/// integer, a 2-byte `size` byte count, and `size` raw data bytes per
/// Inside Macintosh: Imaging With QuickDraw §A-3 Table A-2.
///
/// Returns [`PictError::InvalidData`] when `data.len()` overflows the
/// `u16` `size` field (the §A-3 record layout caps the payload at
/// 65535 bytes; longer annotations must split across multiple opcodes).
/// The §A-3 word-alignment rule for v2 opcodes is the
/// [`PictBuilder`]'s job — odd-size data blocks get a pad byte before
/// the *next* opcode, not inside this record.
pub fn build_long_comment(kind: u16, data: &[u8]) -> Result<Vec<u8>> {
    let size: u16 = data.len().try_into().map_err(|_| {
        PictError::invalid(format!(
            "LongComment data size {} exceeds the u16 (65535-byte) size field",
            data.len()
        ))
    })?;
    let mut buf = Vec::with_capacity(6 + data.len());
    write_u16(&mut buf, OP_LONG_COMMENT);
    write_u16(&mut buf, kind);
    write_u16(&mut buf, size);
    buf.extend_from_slice(data);
    Ok(buf)
}

/// Build a v1 `ShortComment` (`$A0`) opcode — a 1-byte opcode followed
/// by a 2-byte `Kind` integer per Inside Macintosh: Imaging With
/// QuickDraw §A-3 Table A-3.
///
/// v1 streams use 8-bit opcodes and have no word-alignment requirement,
/// so the on-disk record is one byte shorter than its v2 counterpart.
pub fn build_short_comment_v1(kind: u16) -> Vec<u8> {
    let mut buf = Vec::with_capacity(3);
    buf.push(0xA0u8);
    write_u16(&mut buf, kind);
    buf
}

/// Build a v1 `LongComment` (`$A1`) opcode — a 1-byte opcode followed
/// by a 2-byte `Kind` integer, a 2-byte `size` field, and `size` raw
/// data bytes per Inside Macintosh: Imaging With QuickDraw §A-3
/// Table A-3.
///
/// Same `u16` `size` cap as [`build_long_comment`] — overflow returns
/// [`PictError::InvalidData`].
pub fn build_long_comment_v1(kind: u16, data: &[u8]) -> Result<Vec<u8>> {
    let size: u16 = data.len().try_into().map_err(|_| {
        PictError::invalid(format!(
            "LongComment data size {} exceeds the u16 (65535-byte) size field",
            data.len()
        ))
    })?;
    let mut buf = Vec::with_capacity(5 + data.len());
    buf.push(0xA1u8);
    write_u16(&mut buf, kind);
    write_u16(&mut buf, size);
    buf.extend_from_slice(data);
    Ok(buf)
}

/// Build a `CompressedQuickTime` (`$8200`) opcode wrapping an opaque
/// embedded-image payload per Inside Macintosh: Imaging With QuickDraw
/// §A-3 Table A-2: `Data length (Long)` followed by `data length`
/// bytes *private to QuickTime* (total additional data =
/// `4 + data length`; the length word excludes itself).
///
/// The bytes are emitted verbatim — their internal structure belongs
/// to Inside Macintosh: QuickTime, so composing a valid embedded image
/// stream is the caller's job. Returns [`PictError::InvalidData`] when
/// `data.len()` overflows the 32-bit length field.
pub fn build_compressed_quicktime(data: &[u8]) -> Result<Vec<u8>> {
    build_quicktime_op(OP_COMPRESSED_QUICKTIME, data)
}

/// Build an `UncompressedQuickTime` (`$8201`) opcode — the
/// uncompressed counterpart of [`build_compressed_quicktime`], same
/// `Data length (Long)` + opaque-bytes layout.
pub fn build_uncompressed_quicktime(data: &[u8]) -> Result<Vec<u8>> {
    build_quicktime_op(OP_UNCOMPRESSED_QUICKTIME, data)
}

fn build_quicktime_op(opcode: u16, data: &[u8]) -> Result<Vec<u8>> {
    let data_length: u32 = data.len().try_into().map_err(|_| {
        PictError::invalid(format!(
            "QuickTime payload of {} bytes exceeds the u32 data-length field",
            data.len()
        ))
    })?;
    let mut buf = Vec::with_capacity(2 + 4 + data.len());
    write_u16(&mut buf, opcode);
    buf.extend_from_slice(&data_length.to_be_bytes());
    buf.extend_from_slice(data);
    Ok(buf)
}

// ---------------------------------------------------------------------------
// PictBuilder: assemble a complete v2 stream.
// ---------------------------------------------------------------------------

/// Builder that assembles a complete PICT v2 byte stream from
/// individual opcode chunks.
///
/// Lifecycle:
///
/// 1. [`PictBuilder::new`] sets up the 512-byte launch stub, picture
///    record header, and v2 sentinel + headerOp stanza.
/// 2. The caller pushes opcode chunks via [`push`](Self::push), or one
///    of the typed convenience methods ([`line`](Self::line) /
///    [`rect`](Self::rect) / [`region`](Self::region) / etc).
/// 3. [`finish`](Self::finish) word-aligns, appends `OpEndPic`, and
///    returns the byte vector.
///
/// Word-alignment (Inside Macintosh §A-3): every v2 opcode word starts
/// at a 16-bit-aligned offset within the picture record. The builder
/// inserts a zero pad byte before each `push` if the running
/// picture-record-relative offset is odd, mirroring the parser's
/// `align_word` behaviour.
pub struct PictBuilder {
    bytes: Vec<u8>,
    // Picture record starts at offset 512 (after the launch stub).
    // We track the running offset *within the record* so that
    // `align_word` matches the parser's view.
    record_start: usize,
}

impl PictBuilder {
    /// Start a new v2 stream with the given `picFrame`.
    ///
    /// Emits a canonical Listing-A-5 extended-v2 `HeaderOp` payload:
    /// `version=-2`, `hRes=vRes=72.0` dpi, `optimal_source_rect = picFrame`,
    /// reserved fields zero.
    pub fn new(top: i16, left: i16, bottom: i16, right: i16) -> Self {
        let mut bytes = Vec::with_capacity(1024);
        // 512-byte launch stub.
        bytes.extend_from_slice(&[0u8; 512]);
        let record_start = bytes.len();
        // picSize (placeholder = 0) + picFrame.
        write_u16(&mut bytes, 0);
        write_i16(&mut bytes, top);
        write_i16(&mut bytes, left);
        write_i16(&mut bytes, bottom);
        write_i16(&mut bytes, right);
        // v2 version sentinel + headerOp stanza.
        write_u16(&mut bytes, 0x0011);
        write_u16(&mut bytes, 0x02FF);
        write_u16(&mut bytes, OP_HEADER_OP);
        let header = PictHeader::extended_v2_72dpi(RectI32::from_be(top, left, bottom, right));
        bytes.extend_from_slice(&header.to_wire());
        Self {
            bytes,
            record_start,
        }
    }

    /// Picture-record-relative offset (used for v2 word alignment).
    fn record_offset(&self) -> usize {
        self.bytes.len() - self.record_start
    }

    /// Pad to a 2-byte boundary within the picture record.
    fn align_word(&mut self) {
        if self.record_offset() % 2 != 0 {
            self.bytes.push(0);
        }
    }

    /// Append raw opcode bytes. The builder pads to a 2-byte boundary
    /// before pushing, so callers don't have to worry about word
    /// alignment.
    pub fn push(&mut self, opcode_bytes: &[u8]) {
        self.align_word();
        self.bytes.extend_from_slice(opcode_bytes);
    }

    /// Push a `Line` opcode.
    pub fn line(&mut self, h0: i16, v0: i16, h1: i16, v1: i16) -> &mut Self {
        let bytes = build_line(h0, v0, h1, v1);
        self.push(&bytes);
        self
    }

    /// Push a `LineFrom` opcode (`$0021`): draw from the current pen
    /// to `(h1, v1)`, leaving the pen there.
    pub fn line_from(&mut self, h1: i16, v1: i16) -> &mut Self {
        let bytes = build_line_from(h1, v1);
        self.push(&bytes);
        self
    }

    /// Push a `ShortLine` opcode (`$0022`): move the pen to `(h, v)`
    /// and draw to `(h + dh, v + dv)` with SignedByte deltas.
    pub fn short_line(&mut self, h: i16, v: i16, dh: i8, dv: i8) -> &mut Self {
        let bytes = build_short_line(h, v, dh, dv);
        self.push(&bytes);
        self
    }

    /// Push a `ShortLineFrom` opcode (`$0023`): draw from the current
    /// pen to `pen + (dh, dv)` with SignedByte deltas — the most
    /// compact polyline continuation.
    pub fn short_line_from(&mut self, dh: i8, dv: i8) -> &mut Self {
        let bytes = build_short_line_from(dh, dv);
        self.push(&bytes);
        self
    }

    /// Push an `Origin` opcode (`$000C`): shift the picture's
    /// coordinate origin by `(dh, dv)`. Per the `SetOrigin` semantics
    /// (Inside Macintosh: Imaging With QuickDraw §2, book pages
    /// 2-23 f.), positive deltas move subsequently drawn shapes
    /// up / left on the canvas.
    pub fn origin(&mut self, dh: i16, dv: i16) -> &mut Self {
        let bytes = build_origin(dh, dv);
        self.push(&bytes);
        self
    }

    /// Push a `ClipRgn` opcode (`$0001`) carrying a rectangular
    /// clipping region. Subsequent drawing is masked to the rectangle
    /// until the next `ClipRgn` opcode.
    pub fn clip_rect(&mut self, top: i16, left: i16, bottom: i16, right: i16) -> &mut Self {
        let bytes = build_clip_rgn_rect(top, left, bottom, right);
        self.push(&bytes);
        self
    }

    /// Push a rectangle opcode with `verb`.
    pub fn rect(&mut self, verb: Verb, top: i16, left: i16, bottom: i16, right: i16) -> &mut Self {
        let bytes = build_rect_op(verb, top, left, bottom, right);
        self.push(&bytes);
        self
    }

    /// Push an oval opcode.
    pub fn oval(&mut self, verb: Verb, top: i16, left: i16, bottom: i16, right: i16) -> &mut Self {
        let bytes = build_oval_op(verb, top, left, bottom, right);
        self.push(&bytes);
        self
    }

    /// Push a round-rect opcode.
    pub fn round_rect(
        &mut self,
        verb: Verb,
        top: i16,
        left: i16,
        bottom: i16,
        right: i16,
    ) -> &mut Self {
        let bytes = build_round_rect_op(verb, top, left, bottom, right);
        self.push(&bytes);
        self
    }

    /// Push an arc opcode.
    #[allow(clippy::too_many_arguments)]
    pub fn arc(
        &mut self,
        verb: Verb,
        top: i16,
        left: i16,
        bottom: i16,
        right: i16,
        start_angle: i16,
        arc_angle: i16,
    ) -> &mut Self {
        let bytes = build_arc_op(verb, top, left, bottom, right, start_angle, arc_angle);
        self.push(&bytes);
        self
    }

    /// Push a same-rect opcode (`0x0038..=0x003C`): apply `verb` to
    /// the rectangle of the previous rect-family opcode (no payload).
    pub fn same_rect(&mut self, verb: Verb) -> &mut Self {
        let bytes = build_same_rect_op(verb);
        self.push(&bytes);
        self
    }

    /// Push a same-round-rect opcode (`0x0048..=0x004C`): apply `verb`
    /// to the rectangle of the previous round-rect-family opcode.
    pub fn same_round_rect(&mut self, verb: Verb) -> &mut Self {
        let bytes = build_same_round_rect_op(verb);
        self.push(&bytes);
        self
    }

    /// Push a same-oval opcode (`0x0058..=0x005C`): apply `verb` to
    /// the rectangle of the previous oval-family opcode.
    pub fn same_oval(&mut self, verb: Verb) -> &mut Self {
        let bytes = build_same_oval_op(verb);
        self.push(&bytes);
        self
    }

    /// Push a same-arc opcode (`0x0068..=0x006C`): apply `verb` over
    /// the enclosing rectangle of the previous arc-family opcode, with
    /// fresh `startAngle` / `arcAngle` words.
    pub fn same_arc(&mut self, verb: Verb, start_angle: i16, arc_angle: i16) -> &mut Self {
        let bytes = build_same_arc_op(verb, start_angle, arc_angle);
        self.push(&bytes);
        self
    }

    /// Push a polygon opcode. Returns `InvalidData` if `vertices.len() < 2`.
    pub fn poly(&mut self, verb: Verb, vertices: &[(i16, i16)]) -> Result<&mut Self> {
        let bytes = build_poly_op(verb, vertices)?;
        self.push(&bytes);
        Ok(self)
    }

    /// Push a rectangular region opcode.
    pub fn region_rect(
        &mut self,
        verb: Verb,
        top: i16,
        left: i16,
        bottom: i16,
        right: i16,
    ) -> &mut Self {
        let bytes = build_rgn_rect_op(verb, top, left, bottom, right);
        self.push(&bytes);
        self
    }

    /// Push an inversion-encoded region opcode.
    pub fn region(
        &mut self,
        verb: Verb,
        bbox_top: i16,
        bbox_left: i16,
        bbox_bottom: i16,
        bbox_right: i16,
        scanlines: &[(i16, &[i16])],
    ) -> Result<&mut Self> {
        let bytes = build_rgn_inverted_op(
            verb,
            bbox_top,
            bbox_left,
            bbox_bottom,
            bbox_right,
            scanlines,
        )?;
        self.push(&bytes);
        Ok(self)
    }

    /// Push an `RGBFgCol` opcode (foreground colour).
    pub fn fg_color(&mut self, r: u8, g: u8, b: u8) -> &mut Self {
        let bytes = build_rgb_fg_col(r, g, b);
        self.push(&bytes);
        self
    }

    /// Push an `RGBBkCol` opcode (background colour).
    /// Push an `FgColor` opcode (`$000E`) carrying a classic-QuickDraw
    /// colour code (the pre-Color-QuickDraw planar model).
    pub fn fg_color_code(&mut self, code: u32) -> &mut Self {
        let bytes = build_fg_color_code(code);
        self.push(&bytes);
        self
    }

    /// Push a `BkColor` opcode (`$000F`) carrying a classic-QuickDraw
    /// colour code.
    pub fn bg_color_code(&mut self, code: u32) -> &mut Self {
        let bytes = build_bk_color_code(code);
        self.push(&bytes);
        self
    }

    pub fn bg_color(&mut self, r: u8, g: u8, b: u8) -> &mut Self {
        let bytes = build_rgb_bk_col(r, g, b);
        self.push(&bytes);
        self
    }

    /// Push a `PnSize` opcode.
    pub fn pen_size(&mut self, h: i16, v: i16) -> &mut Self {
        let bytes = build_pn_size(h, v);
        self.push(&bytes);
        self
    }

    /// Push an `OvSize` opcode (round-rect corner radius).
    pub fn oval_size(&mut self, h: i16, v: i16) -> &mut Self {
        let bytes = build_oval_size(h, v);
        self.push(&bytes);
        self
    }

    /// Push a `PnPat` opcode (pen pattern, 8 bytes).
    pub fn pen_pattern(&mut self, pattern: [u8; 8]) -> &mut Self {
        let bytes = build_pn_pat(pattern);
        self.push(&bytes);
        self
    }

    /// Push a `BkPat` opcode (background pattern, 8 bytes).
    pub fn bg_pattern(&mut self, pattern: [u8; 8]) -> &mut Self {
        let bytes = build_bk_pat(pattern);
        self.push(&bytes);
        self
    }

    /// Push a `FillPat` opcode (fill pattern, 8 bytes).
    pub fn fill_pattern(&mut self, pattern: [u8; 8]) -> &mut Self {
        let bytes = build_fill_pat(pattern);
        self.push(&bytes);
        self
    }

    /// Push a `TxFont` opcode setting the text font number. Round 230.
    pub fn tx_font(&mut self, font: i16) -> &mut Self {
        let bytes = build_tx_font(font);
        self.push(&bytes);
        self
    }

    /// Push a `TxFace` opcode setting the text face / style flags.
    /// Round 230.
    pub fn tx_face(&mut self, face: u8) -> &mut Self {
        let bytes = build_tx_face(face);
        self.push(&bytes);
        self
    }

    /// Push a `TxMode` opcode setting the source-mode transfer code
    /// (`srcCopy = 0`, …). Round 230.
    pub fn tx_mode(&mut self, mode: i16) -> &mut Self {
        let bytes = build_tx_mode(mode);
        self.push(&bytes);
        self
    }

    /// Push a `SpExtra` opcode setting the extra-space `Fixed` value
    /// (raw on-disk i32 16.16). Round 230.
    pub fn sp_extra(&mut self, extra: i32) -> &mut Self {
        let bytes = build_sp_extra(extra);
        self.push(&bytes);
        self
    }

    /// Push a `PnMode` opcode setting the pen transfer-mode code.
    /// Round 230.
    pub fn pn_mode(&mut self, mode: i16) -> &mut Self {
        let bytes = build_pn_mode(mode);
        self.push(&bytes);
        self
    }

    /// Push a `TxSize` opcode setting the text size in points.
    /// Round 230.
    pub fn tx_size(&mut self, size: i16) -> &mut Self {
        let bytes = build_tx_size(size);
        self.push(&bytes);
        self
    }

    /// Push a `TxRatio` opcode setting the text scaling ratio.
    /// `numer` and `denom` are each a `Point` on disk `(v, h)` per
    /// §A-3 Table A-1. Round 230.
    pub fn tx_ratio(
        &mut self,
        numer_v: i16,
        numer_h: i16,
        denom_v: i16,
        denom_h: i16,
    ) -> &mut Self {
        let bytes = build_tx_ratio(numer_v, numer_h, denom_v, denom_h);
        self.push(&bytes);
        self
    }

    /// Push a `PnLocHFrac` opcode setting the fractional pen position
    /// (low word of a `Fixed`). Round 230.
    pub fn pn_loc_h_frac(&mut self, frac: i16) -> &mut Self {
        let bytes = build_pn_loc_h_frac(frac);
        self.push(&bytes);
        self
    }

    /// Push a `ChExtra` opcode setting the per-character extra-width
    /// adjustment. Round 230.
    pub fn ch_extra(&mut self, extra: i16) -> &mut Self {
        let bytes = build_ch_extra(extra);
        self.push(&bytes);
        self
    }

    /// Push a `HiliteMode` opcode — the 0-byte "flag" that signals
    /// the next drawing operation should use the highlight mode.
    /// Round 230.
    pub fn hilite_mode(&mut self) -> &mut Self {
        let bytes = build_hilite_mode();
        self.push(&bytes);
        self
    }

    /// Push a `HiliteColor` opcode carrying an `RGBColor` triple.
    /// Round 230.
    pub fn hilite_color(&mut self, r: u8, g: u8, b: u8) -> &mut Self {
        let bytes = build_hilite_color(r, g, b);
        self.push(&bytes);
        self
    }

    /// Push a `DefHilite` opcode — switches subsequent draws back to
    /// the system-default highlight colour. Round 230.
    pub fn def_hilite(&mut self) -> &mut Self {
        let bytes = build_def_hilite();
        self.push(&bytes);
        self
    }

    /// Push an `OpColor` opcode carrying an `RGBColor` triple — the
    /// colour parameter for arithmetic transfer modes (`blend`,
    /// `addPin`, …). Round 230.
    pub fn op_color(&mut self, r: u8, g: u8, b: u8) -> &mut Self {
        let bytes = build_op_color(r, g, b);
        self.push(&bytes);
        self
    }

    /// Push a `fontName` opcode (`$002C`) recording the producer's
    /// `oldFontID` + font-name bytes per Inside Macintosh: Imaging With
    /// QuickDraw §A-3 Table A-2 footnote `*`. Returns
    /// [`PictError::InvalidData`] when `name.len()` exceeds the on-disk
    /// `u8` (255-byte) name-length field. Round 236.
    pub fn font_name(&mut self, old_font_id: i16, name: &[u8]) -> Result<&mut Self> {
        let bytes = build_font_name(old_font_id, name)?;
        self.push(&bytes);
        Ok(self)
    }

    /// Push a `lineJustify` opcode (`$002D`) recording the Script-
    /// Manager line-layout state per Inside Macintosh: Imaging With
    /// QuickDraw §A-3 Table A-2 footnote `†`. Each parameter is a raw
    /// `Fixed` (16.16 i32). Round 236.
    pub fn line_justify(&mut self, inter_char_spacing: i32, total_extra: i32) -> &mut Self {
        let bytes = build_line_justify(inter_char_spacing, total_extra);
        self.push(&bytes);
        self
    }

    /// Push a `glyphState` opcode (`$002E`) recording the four
    /// preserved-glyph Booleans per Inside Macintosh: Imaging With
    /// QuickDraw §A-3 Table A-2 row `$002E`. Round 236.
    pub fn glyph_state(
        &mut self,
        outline_preferred: bool,
        preserve_glyph: bool,
        fractional_widths: bool,
        scaling_disabled: bool,
    ) -> &mut Self {
        let bytes = build_glyph_state(
            outline_preferred,
            preserve_glyph,
            fractional_widths,
            scaling_disabled,
        );
        self.push(&bytes);
        self
    }

    /// Push a `LongText` opcode (`$0028`): draw `text` with the
    /// baseline pen established at the absolute point `(h, v)` in
    /// picture-frame coordinates. Errors when `text` exceeds the
    /// 1-byte `count` field (255 bytes).
    pub fn long_text(&mut self, h: i16, v: i16, text: &[u8]) -> Result<&mut Self> {
        let bytes = build_long_text(h, v, text)?;
        self.push(&bytes);
        Ok(self)
    }

    /// Push a `DHText` opcode (`$0029`): advance the running text pen
    /// rightward by `dh`, then draw `text`. Errors when `text` exceeds
    /// the 1-byte `count` field (255 bytes).
    pub fn dh_text(&mut self, dh: u8, text: &[u8]) -> Result<&mut Self> {
        let bytes = build_dh_text(dh, text)?;
        self.push(&bytes);
        Ok(self)
    }

    /// Push a `DVText` opcode (`$002A`): advance the running text pen
    /// downward by `dv`, then draw `text`. Errors when `text` exceeds
    /// the 1-byte `count` field (255 bytes).
    pub fn dv_text(&mut self, dv: u8, text: &[u8]) -> Result<&mut Self> {
        let bytes = build_dv_text(dv, text)?;
        self.push(&bytes);
        Ok(self)
    }

    /// Push a `DHDVText` opcode (`$002B`): advance the running text
    /// pen by `(dh, dv)`, then draw `text`. Errors when `text` exceeds
    /// the 1-byte `count` field (255 bytes).
    pub fn dhdv_text(&mut self, dh: u8, dv: u8, text: &[u8]) -> Result<&mut Self> {
        let bytes = build_dhdv_text(dh, dv, text)?;
        self.push(&bytes);
        Ok(self)
    }

    /// Push a `CompressedQuickTime` opcode (`$8200`) wrapping an
    /// opaque embedded-image payload (bytes private to QuickTime per
    /// §A-3). Errors when `data` exceeds the 32-bit length field.
    pub fn compressed_quicktime(&mut self, data: &[u8]) -> Result<&mut Self> {
        let bytes = build_compressed_quicktime(data)?;
        self.push(&bytes);
        Ok(self)
    }

    /// Push an `UncompressedQuickTime` opcode (`$8201`) wrapping an
    /// opaque embedded-image payload. Errors when `data` exceeds the
    /// 32-bit length field.
    pub fn uncompressed_quicktime(&mut self, data: &[u8]) -> Result<&mut Self> {
        let bytes = build_uncompressed_quicktime(data)?;
        self.push(&bytes);
        Ok(self)
    }

    /// Push a `ShortComment` opcode (`$00A0`) carrying the
    /// application-defined `kind` word per Inside Macintosh: Imaging
    /// With QuickDraw §A-3 Table A-2. Comments are passive metadata —
    /// they don't influence rasterisation.
    pub fn short_comment(&mut self, kind: u16) -> &mut Self {
        let bytes = build_short_comment(kind);
        self.push(&bytes);
        self
    }

    /// Push a `LongComment` opcode (`$00A1`) carrying `kind` and the
    /// raw `data` bytes per Inside Macintosh: Imaging With QuickDraw
    /// §A-3 Table A-2. Returns [`PictError::InvalidData`] when
    /// `data.len()` overflows the on-disk u16 `size` field.
    ///
    /// The §A-3 word-alignment between subsequent opcodes is handled by
    /// the builder, so odd-length data blocks don't need padding from
    /// the caller — the next `push` adds a zero pad byte
    /// automatically.
    pub fn long_comment(&mut self, kind: u16, data: &[u8]) -> Result<&mut Self> {
        let bytes = build_long_comment(kind, data)?;
        self.push(&bytes);
        Ok(self)
    }

    /// Push a `PnPixPat` opcode (colour pen pattern). The 8×8 RGBA tile
    /// is row-major; `fallback` is the monochrome `Pat1Data` that
    /// classic QuickDraw consults when the colour pattern can't be
    /// rendered (e.g. on a b/w device). Inside Macintosh §A-3 Listing
    /// A-1.
    pub fn pen_pix_pattern(
        &mut self,
        fallback: [u8; 8],
        pixels: &[[u8; 4]; 64],
    ) -> Result<&mut Self> {
        let bytes =
            crate::encoder::build_pix_pat_op(crate::encoder::PixPatSlot::Pen, fallback, pixels)?;
        self.push(&bytes);
        Ok(self)
    }

    /// Push a `PnPixPat` opcode (colour pen pattern) carrying an
    /// arbitrary power-of-2 `width`×`height` tile.
    ///
    /// Inside Macintosh §3 (book page 3-40): *"A pixel pattern … can be
    /// of any width and height that's a power of 2."* `pixels` is
    /// row-major and must hold exactly `width * height` RGBA cells.
    /// Returns `InvalidData` when the dimensions aren't powers of two or
    /// the cell count doesn't match (round 302).
    pub fn pen_pix_pattern_sized(
        &mut self,
        fallback: [u8; 8],
        width: u16,
        height: u16,
        pixels: &[[u8; 4]],
    ) -> Result<&mut Self> {
        let bytes = crate::encoder::build_pix_pat_op_sized(
            crate::encoder::PixPatSlot::Pen,
            fallback,
            width,
            height,
            pixels,
        )?;
        self.push(&bytes);
        Ok(self)
    }

    /// Push a `BkPixPat` opcode (colour background pattern). See
    /// [`pen_pix_pattern`](Self::pen_pix_pattern).
    pub fn bg_pix_pattern(
        &mut self,
        fallback: [u8; 8],
        pixels: &[[u8; 4]; 64],
    ) -> Result<&mut Self> {
        let bytes = crate::encoder::build_pix_pat_op(
            crate::encoder::PixPatSlot::Background,
            fallback,
            pixels,
        )?;
        self.push(&bytes);
        Ok(self)
    }

    /// Push a `FillPixPat` opcode (colour fill pattern). See
    /// [`pen_pix_pattern`](Self::pen_pix_pattern).
    pub fn fill_pix_pattern(
        &mut self,
        fallback: [u8; 8],
        pixels: &[[u8; 4]; 64],
    ) -> Result<&mut Self> {
        let bytes =
            crate::encoder::build_pix_pat_op(crate::encoder::PixPatSlot::Fill, fallback, pixels)?;
        self.push(&bytes);
        Ok(self)
    }

    /// Push a `PnPixPat` opcode (dither pen pattern, `patType=2`).
    /// Inside Macintosh: Imaging With QuickDraw §A-3 Listing A-1.
    ///
    /// `rgb` is the target colour the dither tile should approximate;
    /// `fallback` is the monochrome `Pat1Data` for 1-bpp consumers.
    /// Round 95 — see [`PixPattern::from_dither_rgb`] for the decode
    /// behaviour on a true-colour canvas.
    ///
    /// [`PixPattern::from_dither_rgb`]: crate::state::PixPattern::from_dither_rgb
    pub fn pen_dither_pix_pattern(&mut self, fallback: [u8; 8], rgb: [u8; 3]) -> &mut Self {
        let bytes =
            crate::encoder::build_pix_pat_dither_op(crate::encoder::PixPatSlot::Pen, fallback, rgb);
        self.push(&bytes);
        self
    }

    /// Push a `BkPixPat` opcode (dither background pattern,
    /// `patType=2`). See [`pen_dither_pix_pattern`](Self::pen_dither_pix_pattern).
    pub fn bg_dither_pix_pattern(&mut self, fallback: [u8; 8], rgb: [u8; 3]) -> &mut Self {
        let bytes = crate::encoder::build_pix_pat_dither_op(
            crate::encoder::PixPatSlot::Background,
            fallback,
            rgb,
        );
        self.push(&bytes);
        self
    }

    /// Push a `FillPixPat` opcode (dither fill pattern, `patType=2`).
    /// See [`pen_dither_pix_pattern`](Self::pen_dither_pix_pattern).
    pub fn fill_dither_pix_pattern(&mut self, fallback: [u8; 8], rgb: [u8; 3]) -> &mut Self {
        let bytes = crate::encoder::build_pix_pat_dither_op(
            crate::encoder::PixPatSlot::Fill,
            fallback,
            rgb,
        );
        self.push(&bytes);
        self
    }

    /// Push a `DirectBitsRect` raster opcode at picture-frame
    /// destination `(top, left, bottom, right)`. The raster's width
    /// and height are derived from the rect; `data` must be RGBA8
    /// row-major sized `width × height × 4` bytes.
    ///
    /// Mixes a raster blit into an otherwise drawing-only PICT stream:
    /// every drawing primitive emitted *before* the raster paints
    /// underneath it; every drawing primitive emitted *after* paints
    /// over it.
    pub fn raster(
        &mut self,
        top: i16,
        left: i16,
        bottom: i16,
        right: i16,
        data: &[u8],
        pack: crate::encoder::PackType,
    ) -> Result<&mut Self> {
        let bytes =
            crate::encoder::build_direct_bits_rect_op(top, left, bottom, right, data, pack)?;
        self.push(&bytes);
        Ok(self)
    }

    /// [`PictBuilder::raster`] with an explicit transfer-mode word in
    /// the `DirectBitsRect` record's `mode` field (§A-3 Listing A-2):
    /// `0..=7` are the §3-113 Boolean source modes (`srcCopy` …
    /// `notSrcBic`), `32..=39` the §4 arithmetic transfer modes
    /// (`blend` … `adMin`), `+ 64` adds `ditherCopy`. The decoder
    /// honours the word at blit time against the active foreground /
    /// background / `OpColor` state.
    #[allow(clippy::too_many_arguments)]
    pub fn raster_with_mode(
        &mut self,
        top: i16,
        left: i16,
        bottom: i16,
        right: i16,
        data: &[u8],
        pack: crate::encoder::PackType,
        mode: u16,
    ) -> Result<&mut Self> {
        let bytes = crate::encoder::build_direct_bits_rect_op_with_mode(
            top, left, bottom, right, data, pack, mode,
        )?;
        self.push(&bytes);
        Ok(self)
    }

    /// Word-align then append `OpEndPic` (`0x00FF`) and return the
    /// final byte stream.
    pub fn finish(mut self) -> Vec<u8> {
        self.align_word();
        write_u16(&mut self.bytes, OP_OP_END_PIC);
        self.bytes
    }
}

// ---------------------------------------------------------------------------
// PictV1Builder: assemble a complete v1 stream.
// ---------------------------------------------------------------------------

/// Builder that assembles a complete **PICT v1** byte stream from the
/// same opcode chunks as [`PictBuilder`] (round 401).
///
/// Version 1 pictures (Inside Macintosh: Imaging With QuickDraw §A-3
/// Table A-3) use **1-byte opcodes** with **no word alignment**, a
/// plain 10-byte picture-record header (no launch stub, no v2
/// `headerOp` stanza), the `$11 $01` version stanza, and a 1-byte
/// `$FF` `OpEndPic`. Table A-3 is numbering-compatible with the v2
/// table for every opcode it defines — the payload layouts are
/// byte-identical and the v2 opcode word is simply `0x00` followed by
/// the v1 opcode byte. [`PictV1Builder::push`] exploits that: it
/// accepts any chunk produced by the `build_*` helpers whose opcode
/// exists in Table A-3 and re-emits it with the high `0x00` byte
/// stripped.
///
/// Colour-QuickDraw-only opcodes (`RGBFgCol $001A`, pix patterns,
/// `DirectBits*` …) are **not** legal in a v1 stream; `push` rejects
/// any chunk whose opcode byte is not defined by Table A-3. For v1
/// colour selection use the classic colour codes
/// ([`build_fg_color_code`] / [`build_bk_color_code`]).
///
/// `finish` records the picture size in the `picSize` word when it
/// fits (Table A-3: *"Version 1 pictures are limited to 32 KB"*;
/// oversize streams keep the conventional `0` placeholder).
pub struct PictV1Builder {
    bytes: Vec<u8>,
}

impl PictV1Builder {
    /// Start a new v1 stream with the given `picFrame`.
    pub fn new(top: i16, left: i16, bottom: i16, right: i16) -> Self {
        let mut bytes = Vec::with_capacity(64);
        // picSize (patched by `finish`) + picFrame.
        write_u16(&mut bytes, 0);
        write_i16(&mut bytes, top);
        write_i16(&mut bytes, left);
        write_i16(&mut bytes, bottom);
        write_i16(&mut bytes, right);
        // v1 version stanza: opcode $11, version $01.
        bytes.push(0x11);
        bytes.push(0x01);
        Self { bytes }
    }

    /// Append a `build_*` opcode chunk, converting it from the v2
    /// two-byte-opcode form to the v1 one-byte form.
    ///
    /// Returns [`PictError::InvalidData`] when the chunk is shorter
    /// than an opcode word, when its high opcode byte is non-zero, or
    /// when the opcode is not defined for version 1 pictures by §A-3
    /// Table A-3 (the version-1 walker would misparse everything after
    /// an undefined opcode, so refusing at build time is the safe
    /// contract).
    pub fn push(&mut self, v2_opcode_bytes: &[u8]) -> Result<&mut Self> {
        if v2_opcode_bytes.len() < 2 {
            return Err(PictError::invalid(
                "v1 push needs at least the 2-byte opcode word",
            ));
        }
        let opcode = u16::from_be_bytes([v2_opcode_bytes[0], v2_opcode_bytes[1]]);
        if !v1_defines_opcode(opcode) {
            return Err(PictError::invalid(format!(
                "opcode 0x{opcode:04X} is not defined for version 1 pictures (§A-3 Table A-3)"
            )));
        }
        self.bytes.push(v2_opcode_bytes[1]);
        self.bytes.extend_from_slice(&v2_opcode_bytes[2..]);
        Ok(self)
    }

    /// Terminate with the 1-byte `$FF` `OpEndPic` and return the
    /// stream, patching `picSize` when the record fits its 16-bit
    /// field.
    pub fn finish(mut self) -> Vec<u8> {
        self.bytes.push(0xFF);
        if let Ok(size) = u16::try_from(self.bytes.len()) {
            self.bytes[0..2].copy_from_slice(&size.to_be_bytes());
        }
        self.bytes
    }
}

/// Whether §A-3 Table A-3 defines `opcode` for version 1 pictures.
///
/// Everything in Table A-3 except the raster opcodes (`$90`/`$91`/
/// `$98`/`$99` — those carry BitMap payloads the `build_*` helpers in
/// this module don't produce; use `encode_pict_v1*` for v1 rasters)
/// and the `$11` version stanza (emitted by [`PictV1Builder::new`]).
fn v1_defines_opcode(opcode: u16) -> bool {
    matches!(
        opcode,
        0x0000..=0x0010            // NOP..TxRatio (state + patterns)
        | 0x0020..=0x0023          // Line family
        | 0x0028..=0x002B          // Text family
        | 0x0030..=0x0034 | 0x0038..=0x003C // Rect verbs + same-rect
        | 0x0040..=0x0044 | 0x0048..=0x004C // RRect verbs + same-rrect
        | 0x0050..=0x0054 | 0x0058..=0x005C // Oval verbs + same-oval
        | 0x0060..=0x0064 | 0x0068..=0x006C // Arc verbs + same-arc
        | 0x0070..=0x0074          // Poly verbs
        | 0x0080..=0x0084          // Rgn verbs
        | 0x00A0..=0x00A1 // Comments
    )
}

// ---------------------------------------------------------------------------
// Internal helpers.
// ---------------------------------------------------------------------------

#[inline]
fn write_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_be_bytes());
}

#[inline]
fn write_i16(out: &mut Vec<u8>, v: i16) {
    out.extend_from_slice(&v.to_be_bytes());
}

#[inline]
fn expand_8_to_16(v: u8) -> u16 {
    // QuickDraw's RGBColor stores 16 bits per channel; the high byte
    // holds the actual colour and the low byte is treated as
    // resolution padding by the decoder. Duplicating the byte mirrors
    // the convention real-world Mac apps use.
    ((v as u16) << 8) | (v as u16)
}

// ---------------------------------------------------------------------------
// Unit tests.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decoder::parse_pict;

    #[test]
    fn build_line_layout() {
        let b = build_line(10, 20, 30, 40);
        // Opcode 0x0020.
        assert_eq!(&b[0..2], &[0x00, 0x20]);
        // (v0, h0, v1, h1) = (20, 10, 40, 30).
        assert_eq!(i16::from_be_bytes([b[2], b[3]]), 20);
        assert_eq!(i16::from_be_bytes([b[4], b[5]]), 10);
        assert_eq!(i16::from_be_bytes([b[6], b[7]]), 40);
        assert_eq!(i16::from_be_bytes([b[8], b[9]]), 30);
    }

    #[test]
    fn build_rect_op_layout() {
        let b = build_rect_op(Verb::Frame, 1, 2, 3, 4);
        // opcode 0x0030 + verb=0 (Frame) → 0x0030.
        assert_eq!(&b[0..2], &[0x00, 0x30]);
        // top=1, left=2, bottom=3, right=4.
        assert_eq!(i16::from_be_bytes([b[2], b[3]]), 1);
        assert_eq!(i16::from_be_bytes([b[4], b[5]]), 2);
        assert_eq!(i16::from_be_bytes([b[6], b[7]]), 3);
        assert_eq!(i16::from_be_bytes([b[8], b[9]]), 4);
    }

    #[test]
    fn build_oval_paint_opcode_byte() {
        let b = build_oval_op(Verb::Paint, 0, 0, 10, 10);
        // 0x0050 | 1 = 0x0051 (paint oval).
        assert_eq!(&b[0..2], &[0x00, 0x51]);
    }

    #[test]
    fn build_arc_carries_angles() {
        let b = build_arc_op(Verb::Frame, 0, 0, 10, 10, 90, 45);
        assert_eq!(&b[0..2], &[0x00, 0x60]);
        assert_eq!(i16::from_be_bytes([b[10], b[11]]), 90);
        assert_eq!(i16::from_be_bytes([b[12], b[13]]), 45);
    }

    #[test]
    fn build_poly_layout() {
        let verts = [(1i16, 2i16), (3, 4), (5, 6)];
        let b = build_poly_op(Verb::Frame, &verts).unwrap();
        // opcode 0x0070.
        assert_eq!(&b[0..2], &[0x00, 0x70]);
        // polySize = 10 + 4*3 = 22.
        assert_eq!(u16::from_be_bytes([b[2], b[3]]), 22);
        // bbox: top=2, left=1, bottom=7 (=6+1), right=6 (=5+1).
        assert_eq!(i16::from_be_bytes([b[4], b[5]]), 2);
        assert_eq!(i16::from_be_bytes([b[6], b[7]]), 1);
        assert_eq!(i16::from_be_bytes([b[8], b[9]]), 7);
        assert_eq!(i16::from_be_bytes([b[10], b[11]]), 6);
        // First vertex on disk is (v=2, h=1).
        assert_eq!(i16::from_be_bytes([b[12], b[13]]), 2);
        assert_eq!(i16::from_be_bytes([b[14], b[15]]), 1);
    }

    #[test]
    fn build_poly_rejects_too_few_verts() {
        assert!(build_poly_op(Verb::Frame, &[]).is_err());
        assert!(build_poly_op(Verb::Frame, &[(0, 0)]).is_err());
    }

    #[test]
    fn build_rgn_rect_layout() {
        let b = build_rgn_rect_op(Verb::Paint, 0, 0, 10, 10);
        // 0x0080 | 1 = 0x0081.
        assert_eq!(&b[0..2], &[0x00, 0x81]);
        // rgnSize = 10.
        assert_eq!(u16::from_be_bytes([b[2], b[3]]), 10);
        // Total payload = opcode word (2) + rgnSize word (2) + bbox (8)
        // = 12 bytes.
        assert_eq!(b.len(), 12);
    }

    #[test]
    fn build_rgn_inverted_layout() {
        // Region: 4×4 bbox; one inversion record at y=1 with x_pairs
        // [1, 3] (toggle membership at columns 1 and 3 starting at
        // row 1).
        let scanlines = [(1i16, [1i16, 3i16].as_slice())];
        let b = build_rgn_inverted_op(Verb::Paint, 0, 0, 4, 4, &scanlines).unwrap();
        // 0x0080 | 1 = 0x0081.
        assert_eq!(&b[0..2], &[0x00, 0x81]);
        // rgnSize: 10 (header) + 2 (y) + 4 (x_pairs) + 2 (line term) +
        // 2 (region term) = 20.
        assert_eq!(u16::from_be_bytes([b[2], b[3]]), 20);
        // bbox.
        assert_eq!(i16::from_be_bytes([b[4], b[5]]), 0); // top
        assert_eq!(i16::from_be_bytes([b[6], b[7]]), 0); // left
        assert_eq!(i16::from_be_bytes([b[8], b[9]]), 4); // bottom
        assert_eq!(i16::from_be_bytes([b[10], b[11]]), 4); // right
                                                           // First inv-record y = 1.
        assert_eq!(i16::from_be_bytes([b[12], b[13]]), 1);
        // x_pairs[0] = 1.
        assert_eq!(i16::from_be_bytes([b[14], b[15]]), 1);
        // x_pairs[1] = 3.
        assert_eq!(i16::from_be_bytes([b[16], b[17]]), 3);
        // 0x7FFF line terminator.
        assert_eq!(i16::from_be_bytes([b[18], b[19]]), 0x7FFF);
        // 0x7FFF region terminator.
        assert_eq!(i16::from_be_bytes([b[20], b[21]]), 0x7FFF);
        assert_eq!(b.len(), 22);
    }

    #[test]
    fn build_rgn_rejects_descending_y() {
        let scanlines = [
            (3i16, [0i16].as_slice()),
            (1i16, [0i16].as_slice()), // out of order!
        ];
        assert!(build_rgn_inverted_op(Verb::Paint, 0, 0, 4, 4, &scanlines).is_err());
    }

    #[test]
    fn rgb_fg_col_expansion() {
        let b = build_rgb_fg_col(0xAB, 0xCD, 0xEF);
        // opcode 0x001A.
        assert_eq!(&b[0..2], &[0x00, 0x1A]);
        // R = 0xABAB, G = 0xCDCD, B = 0xEFEF.
        assert_eq!(u16::from_be_bytes([b[2], b[3]]), 0xABAB);
        assert_eq!(u16::from_be_bytes([b[4], b[5]]), 0xCDCD);
        assert_eq!(u16::from_be_bytes([b[6], b[7]]), 0xEFEF);
    }

    // ---- end-to-end PictBuilder tests ----

    #[test]
    fn builder_just_a_filled_rect_decodes() {
        let mut b = PictBuilder::new(0, 0, 16, 16);
        b.fg_color(0xFF, 0x00, 0x00);
        b.rect(Verb::Paint, 4, 4, 12, 12);
        let bytes = b.finish();
        let img = parse_pict(&bytes).expect("decode failed");
        assert_eq!(img.width, 16);
        assert_eq!(img.height, 16);
        // Centre pixel should be red (paint at fg).
        let off = (8 * 16 + 8) * 4;
        assert_eq!(img.data[off], 0xFF, "R");
        assert_eq!(img.data[off + 1], 0x00, "G");
        assert_eq!(img.data[off + 2], 0x00, "B");
        // Outside the rect should be background paper (white).
        let off = (16 + 1) * 4;
        assert_eq!(img.data[off], 0xFF, "paper R");
        assert_eq!(img.data[off + 1], 0xFF, "paper G");
        assert_eq!(img.data[off + 2], 0xFF, "paper B");
    }

    #[test]
    fn builder_oval_fill_decodes() {
        let mut b = PictBuilder::new(0, 0, 32, 32);
        b.fg_color(0, 0xFF, 0);
        b.oval(Verb::Fill, 0, 0, 32, 32);
        let bytes = b.finish();
        let img = parse_pict(&bytes).expect("decode failed");
        // Centre pixel inside the oval — should be green.
        let cx = 16usize;
        let cy = 16usize;
        let off = (cy * 32 + cx) * 4;
        assert_eq!(img.data[off + 1], 0xFF, "G at centre");
        // Corner outside the oval — should still be paper.
        let off = 0; // (0,0)
        assert_eq!(img.data[off + 1], 0xFF, "G corner = paper");
    }

    #[test]
    fn builder_round_rect_decodes() {
        let mut b = PictBuilder::new(0, 0, 32, 32);
        b.oval_size(8, 8);
        b.fg_color(0, 0, 0xFF);
        b.round_rect(Verb::Paint, 4, 4, 28, 28);
        let bytes = b.finish();
        let img = parse_pict(&bytes).expect("decode failed");
        // Mid-edge of round-rect — should be blue.
        let off = (16 * 32 + 16) * 4;
        assert_eq!(img.data[off + 2], 0xFF, "B at centre");
    }

    #[test]
    fn builder_polygon_decodes() {
        // Triangle filling the lower-right corner.
        let mut b = PictBuilder::new(0, 0, 16, 16);
        b.fg_color(0xFF, 0xFF, 0x00);
        b.poly(Verb::Fill, &[(2, 2), (14, 2), (8, 14)]).unwrap();
        let bytes = b.finish();
        let img = parse_pict(&bytes).expect("decode failed");
        // The triangle's centroid should be yellow.
        let off = (6 * 16 + 8) * 4;
        assert_eq!(img.data[off], 0xFF, "R");
        assert_eq!(img.data[off + 1], 0xFF, "G");
        assert_eq!(img.data[off + 2], 0x00, "B");
    }

    #[test]
    fn builder_arc_decodes() {
        // Quarter-arc — round-trip parse should succeed; the rasteriser
        // produces SOME ink along the arc, so the canvas is dirty.
        let mut b = PictBuilder::new(0, 0, 32, 32);
        b.fg_color(0xFF, 0x00, 0xFF);
        b.arc(Verb::Frame, 0, 0, 32, 32, 0, 90);
        let bytes = b.finish();
        let img = parse_pict(&bytes).expect("decode failed");
        // Pick some pixel ON the arc rim (top-right quadrant); we
        // can't predict the exact rasterisation — just check that at
        // least one pixel along the right edge isn't paper.
        let mut rim_inked = false;
        for y in 0..16 {
            let off = (y * 32 + 30) * 4;
            if img.data[off] != 0xFF || img.data[off + 1] != 0xFF || img.data[off + 2] != 0xFF {
                rim_inked = true;
                break;
            }
        }
        assert!(rim_inked, "arc should ink some pixel along the rim");
    }

    #[test]
    fn builder_region_rect_decodes() {
        let mut b = PictBuilder::new(0, 0, 16, 16);
        b.fg_color(0x00, 0xFF, 0xFF);
        b.region_rect(Verb::Paint, 2, 2, 14, 14);
        let bytes = b.finish();
        let img = parse_pict(&bytes).expect("decode failed");
        let off = (8 * 16 + 8) * 4;
        // Cyan (00 FF FF) inside the region.
        assert_eq!(img.data[off], 0x00, "R");
        assert_eq!(img.data[off + 1], 0xFF, "G");
        assert_eq!(img.data[off + 2], 0xFF, "B");
    }

    #[test]
    fn builder_region_inverted_decodes() {
        let mut b = PictBuilder::new(0, 0, 8, 8);
        b.fg_color(0xFF, 0xAA, 0x00);
        // 4×4 inversion region inside an 8×8 bbox: starting at y=2,
        // toggle membership at columns 2 and 6 (so cols [2,6) are
        // inside for rows 2..8).
        let scanlines = [(2i16, [2i16, 6i16].as_slice())];
        b.region(Verb::Paint, 0, 0, 8, 8, &scanlines).unwrap();
        let bytes = b.finish();
        let img = parse_pict(&bytes).expect("decode failed");
        // Inside the region (row 4, col 3) → orange.
        let off = (4 * 8 + 3) * 4;
        assert_eq!(img.data[off], 0xFF, "R inside");
        assert_eq!(img.data[off + 1], 0xAA, "G inside");
        assert_eq!(img.data[off + 2], 0x00, "B inside");
        // Outside the region (row 0, col 0) → paper.
        let off = 0;
        assert_eq!(img.data[off], 0xFF, "paper R");
        assert_eq!(img.data[off + 1], 0xFF, "paper G");
        assert_eq!(img.data[off + 2], 0xFF, "paper B");
    }

    #[test]
    fn builder_line_decodes() {
        let mut b = PictBuilder::new(0, 0, 16, 16);
        b.fg_color(0x00, 0x00, 0x00);
        b.line(0, 0, 15, 15);
        let bytes = b.finish();
        let img = parse_pict(&bytes).expect("decode failed");
        // Diagonal pixel (5, 5) should be black.
        let off = (5 * 16 + 5) * 4;
        assert_eq!(img.data[off], 0x00, "R on diagonal");
        assert_eq!(img.data[off + 1], 0x00, "G on diagonal");
        assert_eq!(img.data[off + 2], 0x00, "B on diagonal");
    }

    #[test]
    fn builder_word_alignment() {
        // Push a `LongComment` opcode (0x00A1) with a 1-byte data
        // payload — total opcode size = 2 (opcode) + 2 (kind) + 2
        // (size) + 1 (data) = 7 bytes (odd). The next push must
        // word-align to a 2-byte boundary so the parser doesn't
        // mis-read the rectangle opcode.
        let mut b = PictBuilder::new(0, 0, 4, 4);
        b.fg_color(0, 0, 0);
        // LongComment: opcode 0x00A1 + kind=0 + size=1 + 1 data byte.
        let long_comment = [0x00, 0xA1, 0x00, 0x00, 0x00, 0x01, 0x42];
        b.push(&long_comment);
        b.rect(Verb::Frame, 0, 0, 4, 4); // must be word-aligned
        let bytes = b.finish();
        let img = parse_pict(&bytes).expect("decode after alignment failed");
        assert_eq!(img.width, 4);
        assert_eq!(img.height, 4);
    }
}
