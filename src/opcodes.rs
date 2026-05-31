//! PICT v2 opcode constants + per-opcode static byte sizes.
//!
//! Inside Macintosh: Imaging With QuickDraw §A-3 ("Picture opcodes")
//! tabulates every opcode with its operand size (or "variable" + a
//! formula). Round 1 transcribes the *fixed-size* opcodes — for each
//! we know exactly how many bytes to skip, no parsing needed. The
//! variable-sized opcodes (raster, region, polygon, text, comment,
//! reserved ranges) get their own match arm in `decoder::parse_v2`.

/// Picture comment opcode — produced by `PicComment` traps.
pub const OP_NOP: u16 = 0x0000;
pub const OP_CLIP_RGN: u16 = 0x0001;
pub const OP_BK_PAT: u16 = 0x0002;
pub const OP_TX_FONT: u16 = 0x0003;
pub const OP_TX_FACE: u16 = 0x0004;
pub const OP_TX_MODE: u16 = 0x0005;
pub const OP_SP_EXTRA: u16 = 0x0006;
pub const OP_PN_SIZE: u16 = 0x0007;
pub const OP_PN_MODE: u16 = 0x0008;
pub const OP_PN_PAT: u16 = 0x0009;
pub const OP_FILL_PAT: u16 = 0x000A;
pub const OP_OV_SIZE: u16 = 0x000B;
pub const OP_ORIGIN: u16 = 0x000C;
pub const OP_TX_SIZE: u16 = 0x000D;
pub const OP_FG_COLOR: u16 = 0x000E;
pub const OP_BG_COLOR: u16 = 0x000F;
pub const OP_TX_RATIO: u16 = 0x0010;
pub const OP_VERSION: u16 = 0x0011;
pub const OP_BK_PIX_PAT: u16 = 0x0012;
pub const OP_PN_PIX_PAT: u16 = 0x0013;
pub const OP_FILL_PIX_PAT: u16 = 0x0014;
pub const OP_PN_LOC_HFRAC: u16 = 0x0015;
pub const OP_CH_EXTRA: u16 = 0x0016;
pub const OP_RGB_FG_COL: u16 = 0x001A;
pub const OP_RGB_BK_COL: u16 = 0x001B;
pub const OP_HILITE_MODE: u16 = 0x001C;
pub const OP_HILITE_COLOR: u16 = 0x001D;
pub const OP_DEF_HILITE: u16 = 0x001E;
pub const OP_OP_COLOR: u16 = 0x001F;
pub const OP_LINE: u16 = 0x0020;
pub const OP_LINE_FROM: u16 = 0x0021;
pub const OP_SHORT_LINE: u16 = 0x0022;
pub const OP_SHORT_LINE_FROM: u16 = 0x0023;
pub const OP_LONG_TEXT: u16 = 0x0028;
pub const OP_DH_TEXT: u16 = 0x0029;
pub const OP_DV_TEXT: u16 = 0x002A;
pub const OP_DHDV_TEXT: u16 = 0x002B;
pub const OP_FONT_NAME: u16 = 0x002C;
pub const OP_LINE_JUSTIFY: u16 = 0x002D;
pub const OP_GLYPH_STATE: u16 = 0x002E;

// Frame / paint / erase / invert / fill of rect (per shape verb).
// First nibble of low byte = verb (frame=0, paint=1, erase=2, invert=3,
// fill=4); second nibble = state (0=primitive operands, 8=same-as-last
// no operands).
pub const OP_FRAME_RECT: u16 = 0x0030;
pub const OP_PAINT_RECT: u16 = 0x0031;
pub const OP_ERASE_RECT: u16 = 0x0032;
pub const OP_INVERT_RECT: u16 = 0x0033;
pub const OP_FILL_RECT: u16 = 0x0034;
pub const OP_FRAME_SAME_RECT: u16 = 0x0038;
pub const OP_PAINT_SAME_RECT: u16 = 0x0039;
pub const OP_ERASE_SAME_RECT: u16 = 0x003A;
pub const OP_INVERT_SAME_RECT: u16 = 0x003B;
pub const OP_FILL_SAME_RECT: u16 = 0x003C;

pub const OP_FRAME_RRECT: u16 = 0x0040;
pub const OP_PAINT_RRECT: u16 = 0x0041;
pub const OP_ERASE_RRECT: u16 = 0x0042;
pub const OP_INVERT_RRECT: u16 = 0x0043;
pub const OP_FILL_RRECT: u16 = 0x0044;
pub const OP_FRAME_SAME_RRECT: u16 = 0x0048;
pub const OP_PAINT_SAME_RRECT: u16 = 0x0049;
pub const OP_ERASE_SAME_RRECT: u16 = 0x004A;
pub const OP_INVERT_SAME_RRECT: u16 = 0x004B;
pub const OP_FILL_SAME_RRECT: u16 = 0x004C;

pub const OP_FRAME_OVAL: u16 = 0x0050;
pub const OP_PAINT_OVAL: u16 = 0x0051;
pub const OP_ERASE_OVAL: u16 = 0x0052;
pub const OP_INVERT_OVAL: u16 = 0x0053;
pub const OP_FILL_OVAL: u16 = 0x0054;
pub const OP_FRAME_SAME_OVAL: u16 = 0x0058;
pub const OP_PAINT_SAME_OVAL: u16 = 0x0059;
pub const OP_ERASE_SAME_OVAL: u16 = 0x005A;
pub const OP_INVERT_SAME_OVAL: u16 = 0x005B;
pub const OP_FILL_SAME_OVAL: u16 = 0x005C;

pub const OP_FRAME_ARC: u16 = 0x0060;
pub const OP_PAINT_ARC: u16 = 0x0061;
pub const OP_ERASE_ARC: u16 = 0x0062;
pub const OP_INVERT_ARC: u16 = 0x0063;
pub const OP_FILL_ARC: u16 = 0x0064;
pub const OP_FRAME_SAME_ARC: u16 = 0x0068;
pub const OP_PAINT_SAME_ARC: u16 = 0x0069;
pub const OP_ERASE_SAME_ARC: u16 = 0x006A;
pub const OP_INVERT_SAME_ARC: u16 = 0x006B;
pub const OP_FILL_SAME_ARC: u16 = 0x006C;

pub const OP_FRAME_POLY: u16 = 0x0070;
pub const OP_PAINT_POLY: u16 = 0x0071;
pub const OP_ERASE_POLY: u16 = 0x0072;
pub const OP_INVERT_POLY: u16 = 0x0073;
pub const OP_FILL_POLY: u16 = 0x0074;

pub const OP_FRAME_RGN: u16 = 0x0080;
pub const OP_PAINT_RGN: u16 = 0x0081;
pub const OP_ERASE_RGN: u16 = 0x0082;
pub const OP_INVERT_RGN: u16 = 0x0083;
pub const OP_FILL_RGN: u16 = 0x0084;

pub const OP_BITS_RECT: u16 = 0x0090;
pub const OP_BITS_RGN: u16 = 0x0091;
pub const OP_PACK_BITS_RECT: u16 = 0x0098;
pub const OP_PACK_BITS_RGN: u16 = 0x0099;
pub const OP_DIRECT_BITS_RECT: u16 = 0x009A;
pub const OP_DIRECT_BITS_RGN: u16 = 0x009B;

pub const OP_SHORT_COMMENT: u16 = 0x00A0;
pub const OP_LONG_COMMENT: u16 = 0x00A1;

pub const OP_OP_END_PIC: u16 = 0x00FF;

// Reserved 2-byte operand opcodes (per Inside Macintosh §A-3) are
// dispatched through `reserved_v2_payload_size` below.

pub const OP_HEADER_OP: u16 = 0x0C00;
pub const OP_COMPRESSED_QUICKTIME: u16 = 0x8200;
pub const OP_UNCOMPRESSED_QUICKTIME: u16 = 0x8201;

/// How many bytes of payload a §A-3 *reserved* v2 opcode consumes.
///
/// Inside Macintosh: Imaging With QuickDraw §A-3 (Table A-2) ranges
/// every `0x0000..=0xFFFF` opcode value into one of four families:
/// **defined**, **reserved with a fixed-size payload**, **reserved
/// with a length-prefixed payload** (a 16-bit or 32-bit data-length
/// word followed by that many opaque payload bytes), or
/// **not-determined** (`0x0017..=0x0019` only).
///
/// This helper covers the *reserved* families — the defined opcodes
/// are handled by dedicated `match` arms / [`fixed_operand_size`] in
/// the decoder and probe. A `Some(ReservedV2Skip)` return tells the
/// caller exactly how to walk past the payload so an unknown opcode
/// doesn't terminate the rest of the picture.
///
/// Returns `None` for:
/// * opcodes that are *defined* in §A-3 (the caller's dedicated arm
///   should have matched first),
/// * the three not-determined opcodes (`0x0017..=0x0019`), which §A-3
///   leaves with no size at all — treating them as a hard error is
///   the safe option (any picture that emits one is malformed).
pub fn reserved_v2_payload_size(opcode: u16) -> Option<ReservedV2Skip> {
    use ReservedV2Skip::*;
    Some(match opcode {
        // §A-3 "Not determined" — no payload size published.
        0x0017..=0x0019 => return None,

        // 0x0024..=0x0027 (between ShortLineFrom and LongText) — "Data
        // length (Integer), data".
        0x0024..=0x0027 => U16Prefixed,
        // 0x002F — slot between glyphState (0x002E) and frameRect
        // (0x0030); same length-prefixed family.
        0x002F => U16Prefixed,

        // 0x0035..=0x0037 — three 8-byte reserved slots between fillRect
        // and frameSameRect (mirror of the rect verbs).
        0x0035..=0x0037 => Fixed(8),
        // 0x003D..=0x003F — three 0-byte reserved slots between
        // fillSameRect and frameRRect (mirror of the *SameRect verbs).
        0x003D..=0x003F => Fixed(0),
        // 0x0045..=0x0047 — three 8-byte reserved slots between
        // fillRRect and frameSameRRect.
        0x0045..=0x0047 => Fixed(8),
        // 0x004D..=0x004F — three 0-byte reserved slots between
        // fillSameRRect and frameOval.
        0x004D..=0x004F => Fixed(0),
        // 0x0055..=0x0057 — three 8-byte reserved slots between fillOval
        // and frameSameOval.
        0x0055..=0x0057 => Fixed(8),
        // 0x005D..=0x005F — three 0-byte reserved slots between
        // fillSameOval and frameArc.
        0x005D..=0x005F => Fixed(0),
        // 0x0065..=0x0067 — three 12-byte reserved slots between fillArc
        // and frameSameArc (the 12 = rect + start + arc mirror).
        0x0065..=0x0067 => Fixed(12),
        // 0x006D..=0x006F — three 4-byte reserved slots between
        // fillSameArc and framePoly.
        0x006D..=0x006F => Fixed(4),
        // 0x0075..=0x0077 — three Polygon-shaped reserved slots between
        // fillPoly and frameSamePoly: a 16-bit polySize word that
        // *includes* itself (so payload-after-opcode = `polySize`,
        // payload-after-size-word = `polySize - 2`).
        0x0075..=0x0077 => PolygonSized,
        // 0x0078..=0x007C — frameSamePoly..fillSamePoly: "Not yet
        // implemented" in §A-3; 0-byte payload.
        0x0078..=0x007C => Fixed(0),
        // 0x007D..=0x007F — three 0-byte reserved slots between
        // fillSamePoly and frameRgn.
        0x007D..=0x007F => Fixed(0),
        // 0x0085..=0x0087 — three Region-shaped reserved slots between
        // fillRgn and frameSameRgn: same polySize-style 16-bit word.
        0x0085..=0x0087 => RegionSized,
        // 0x0088..=0x008C — frameSameRgn..fillSameRgn: "Not yet
        // implemented" in §A-3; 0-byte payload.
        0x0088..=0x008C => Fixed(0),
        // 0x008D..=0x008F — three 0-byte reserved slots between
        // fillSameRgn and BitsRect.
        0x008D..=0x008F => Fixed(0),
        // 0x0092..=0x0097 — six length-prefixed reserved slots between
        // BitsRgn and PackBitsRect.
        0x0092..=0x0097 => U16Prefixed,
        // 0x009C..=0x009F — four length-prefixed reserved slots between
        // DirectBitsRgn and ShortComment.
        0x009C..=0x009F => U16Prefixed,
        // 0x00A2..=0x00AF — fourteen length-prefixed reserved slots
        // between LongComment and the 0x00B0 zero-payload range.
        0x00A2..=0x00AF => U16Prefixed,
        // 0x00B0..=0x00CF — thirty-two 0-byte reserved slots.
        0x00B0..=0x00CF => Fixed(0),
        // 0x00D0..=0x00FE — forty-six u32-length-prefixed reserved slots
        // between the zero-payload range and OpEndPic (0x00FF).
        0x00D0..=0x00FE => U32Prefixed,

        // 0x0100..=0x7FFF — the long upper reserved band. §A-3's
        // page A-5 Note: "opcode `$nnXX` carries `2 × nn` bytes of
        // data." The boundary rows in Table A-2 confirm the rule
        // (`$0200`→4, `$0BFF`→22, `$0C00`→24 — but 0x0C00 is HeaderOp,
        // matched separately, `$7F00`/`$7FFF`→254). The high byte
        // ranges 0x01..=0x7F so `2 × nn ∈ [2, 254]`.
        0x0100..=0x01FF => Fixed(2),
        // 0x0200 itself is reserved with 4-byte payload (matches the
        // `2 × nn` rule for nn = 0x02). 0x02FF is the Version word —
        // it's consumed by the version-stanza detector, not the
        // opcode walker, so it should never reach this helper.
        0x0200..=0x02FE => Fixed(4),
        0x0300..=0x03FF => Fixed(6),
        0x0400..=0x04FF => Fixed(8),
        0x0500..=0x05FF => Fixed(10),
        0x0600..=0x06FF => Fixed(12),
        0x0700..=0x07FF => Fixed(14),
        0x0800..=0x08FF => Fixed(16),
        0x0900..=0x09FF => Fixed(18),
        0x0A00..=0x0AFF => Fixed(20),
        0x0B00..=0x0BFF => Fixed(22),
        // 0x0C00 is HeaderOp (24 bytes), already matched as a defined
        // opcode by the v2 version stanza. The rest of the 0x0Cxx band
        // is reserved with the same 24-byte payload.
        0x0C01..=0x0CFF => Fixed(24),
        0x0D00..=0x0DFF => Fixed(26),
        0x0E00..=0x0EFF => Fixed(28),
        0x0F00..=0x0FFF => Fixed(30),
        // 0x1000..=0x7FFF — the rest of the `2 × nn` band.
        0x1000..=0x7FFF => Fixed(2 * ((opcode >> 8) & 0xFF) as usize),

        // 0x8000..=0x80FF — 0-byte reserved.
        0x8000..=0x80FF => Fixed(0),
        // 0x8100..=0x81FF — u32-length-prefixed reserved.
        0x8100..=0x81FF => U32Prefixed,
        // 0x8200 / 0x8201 are defined (CompressedQuickTime /
        // UncompressedQuickTime). Everything else 0x8202..=0xFFFF is
        // u32-length-prefixed reserved per the §A-3 closing rows.
        0x8202..=0xFFFF => U32Prefixed,

        // Defined opcodes — caller's dedicated arm should handle.
        _ => return None,
    })
}

/// Skip strategy for a §A-3 reserved v2 opcode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReservedV2Skip {
    /// Payload is exactly `n` bytes of opaque data immediately after
    /// the opcode word.
    Fixed(usize),
    /// Payload is a 16-bit big-endian data-length word followed by
    /// that many opaque bytes (so total = `2 + dataLen`).
    U16Prefixed,
    /// Payload is a 32-bit big-endian data-length word followed by
    /// that many opaque bytes (so total = `4 + dataLen`).
    U32Prefixed,
    /// Payload is a 16-bit `polySize` word that *includes itself*,
    /// followed by `polySize - 2` opaque bytes (so total = `polySize`).
    /// Matches the §A-3 reserved poly slots 0x0075..=0x0077.
    PolygonSized,
    /// Payload is a 16-bit region-size word that *includes itself*,
    /// followed by `rgnSize - 2` opaque bytes (so total = `rgnSize`).
    /// Matches the §A-3 reserved rgn slots 0x0085..=0x0087.
    RegionSized,
}
