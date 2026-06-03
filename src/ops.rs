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
//! geometry. We don't emit those — every drawing op carries explicit
//! coordinates.

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

    /// Word-align then append `OpEndPic` (`0x00FF`) and return the
    /// final byte stream.
    pub fn finish(mut self) -> Vec<u8> {
        self.align_word();
        write_u16(&mut self.bytes, OP_OP_END_PIC);
        self.bytes
    }
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
