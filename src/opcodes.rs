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

// Reserved 2-byte operand opcodes (per Inside Macintosh §A-3 the
// 0x00B0..=0x00CF range is reserved with a 0-byte payload, 0x00D0..=
// 0x00FE is reserved with 4-byte payload, etc). The decoder skips
// reserved opcodes by their published operand size; round-1 just
// treats any unknown opcode in the reserved-skip ranges as fatal so
// real corruption isn't silently consumed.

pub const OP_HEADER_OP: u16 = 0x0C00;
pub const OP_COMPRESSED_QUICKTIME: u16 = 0x8200;
pub const OP_UNCOMPRESSED_QUICKTIME: u16 = 0x8201;
