//! Pure-Rust PICT (Apple QuickDraw picture) reader.
//!
//! Clean-room implementation of the public **Inside Macintosh: Imaging
//! With QuickDraw** (Apple, 1994).
//!
//! ## Status
//!
//! PICT is opcode-based: the file is a stream of QuickDraw drawing
//! commands. Round 2 walks the v2 opcode stream, steps a small
//! drawing-state machine ([`state::PictState`]) and folds every
//! drawing command (line / rect / round-rect / oval / arc / poly /
//! region / raster) onto an in-crate software-rasteriser canvas
//! ([`raster::Canvas`]). DirectBitsRect packTypes 0/1 (uncompressed),
//! 2 (3-byte packed RGB), 3 (16-bit u16-PackBits) and 4 (component-
//! separated PackBits) all decode. PackBitsRgn / DirectBitsRgn region
//! variants are honoured for the embedded raster (clip-mask use is a
//! future round).
//!
//! v1 PICTs (8-bit opcodes) parse the same drawing-state machine and
//! a smaller raster opcode set (`BitsRect 0x90`, `BitsRgn 0x91`,
//! `PackBitsRect 0x98`, `PackBitsRgn 0x99`).
//!
//! `CompressedQuickTime` (`0x8200`) and `UncompressedQuickTime`
//! (`0x8201`) opcodes are *parsed* (length-prefixed payload skipped
//! cleanly) so they don't wedge a surrounding-PICT decode, but the
//! embedded image (typically JPEG) is not yet decoded.
//!
//! Text glyph opcodes (`LongText` / `DH/DV/DHDVText`) are walked but
//! not rasterised — that needs a TrueType engine. The spec-determined
//! **text-drawing pen location** they carry (the baseline origin from
//! `LongText`'s `txLoc`, advanced by the compact `DH/DV/DHDV` deltas)
//! is tracked into `PictTextState::text_pen` / `text_op_count` (round
//! 295) so callers see where text would land even without glyphs.
//!
//! ## Standalone vs registry-integrated
//!
//! The crate's default `registry` Cargo feature pulls in `oxideav-core`
//! and exposes the framework `Decoder` trait surface plus a
//! [`registry::register`] entry point. Disable the feature
//! (`default-features = false`) for an `oxideav-core`-free build that
//! still exposes the standalone [`parse_pict`] API plus crate-local
//! [`PictImage`] / [`PictPixelFormat`] / [`PictError`] types.

pub mod decoder;
pub mod encoder;
pub mod error;
pub mod header;
pub mod image;
pub mod opcodes;
pub mod ops;
pub mod packbits;
pub mod probe;
pub mod raster;
pub mod reader;
pub mod region;
#[cfg(feature = "registry")]
pub mod registry;
pub mod state;

/// Codec id for PICT image frames.
pub const CODEC_ID_STR: &str = "pict";

pub use decoder::parse_pict;
pub use encoder::{
    build_clip_rgn_rect, build_direct_bits_rect_op, build_direct_bits_rect_op_with_mode,
    build_pix_pat_dither_op, build_pix_pat_op, encode_pict, encode_pict_bits_rect,
    encode_pict_bits_rgn, encode_pict_indexed_bits_rect, encode_pict_indexed_bits_rgn,
    encode_pict_indexed_pack_bits_rect, encode_pict_indexed_pack_bits_rgn,
    encode_pict_pack_bits_rect, encode_pict_pack_bits_rgn, encode_pict_v1, encode_pict_v1_with,
    encode_pict_v2, encode_pict_v2_with_clip, pixel_data_sizes, IndexedPixelSize, PackType,
    PixPatSlot,
};
pub use error::{PictError, Result};
pub use header::{Fixed, PictHeader};
pub use image::{PictComment, PictImage, PictPixelFormat};
pub use ops::{
    build_arc_op, build_bk_pat, build_ch_extra, build_def_hilite, build_fill_pat, build_font_name,
    build_glyph_state, build_hilite_color, build_hilite_mode, build_line, build_line_from,
    build_line_justify, build_long_comment, build_long_comment_v1, build_op_color, build_oval_op,
    build_oval_size, build_pn_loc_h_frac, build_pn_mode, build_pn_pat, build_pn_size,
    build_poly_op, build_rect_op, build_rgb_bk_col, build_rgb_fg_col, build_rgn_inverted_op,
    build_rgn_rect_op, build_round_rect_op, build_short_comment, build_short_comment_v1,
    build_sp_extra, build_tx_face, build_tx_font, build_tx_mode, build_tx_ratio, build_tx_size,
    PictBuilder, Verb,
};
pub use probe::{probe_pict, PictProbe, ProbeRect, ProbeTermination, ProbeVersion};
pub use raster::{blend_arith, blend_source, ArithMode, PatternMode, SourceMode, HILITE_MODE};
pub use state::{
    Pattern, PictFontName, PictGlyphState, PictLineJustify, PictPattern, PictTextFace,
    PictTextState, PixPattern, Rgba, TextRatio,
};

#[cfg(feature = "registry")]
pub use registry::{__oxideav_entry, register, register_codecs, register_containers};
