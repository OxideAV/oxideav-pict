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
//! variants apply their `maskRgn` as a per-blit clip.
//!
//! v1 PICTs (8-bit opcodes) parse the same drawing-state machine and
//! a smaller raster opcode set (`BitsRect 0x90`, `BitsRgn 0x91`,
//! `PackBitsRect 0x98`, `PackBitsRgn 0x99`). The emit side mirrors
//! both framings: [`ops::PictBuilder`] (v2) and [`ops::PictV1Builder`]
//! (v1) assemble streams from the same `build_*` opcode chunks.
//!
//! `CompressedQuickTime` (`0x8200`) and `UncompressedQuickTime`
//! (`0x8201`) payloads are captured verbatim into
//! [`PictImage::quicktime`] (round 401) **and** parsed into typed
//! [`quicktime::QuickTimePayload`] interiors (round 435) per Inside
//! Macintosh: QuickTime (1993) Chapter 3: `$8200` surfaces its
//! wrapper (display matrix, matte, mask region, mode, srcRect,
//! accuracy), the embedded [`quicktime::ImageDescription`] and the
//! compressed image bytes — a CODEC-tag boundary routed through the
//! framework resolver (`registry::resolve_quicktime_codec`, never
//! decoded in-crate) — while `$8201`'s embedded `$98`–`$9B`
//! pixel-data subopcode is rasterised through the normal raster
//! dispatch.
//!
//! Decode-side allocations sized from attacker-controlled fields are
//! bounded by [`MAX_RASTER_BYTES`] (round 401 hostile-input
//! hardening); the `hostile_round401` suite fuzzes the walker in CI.
//!
//! Text glyph opcodes (`LongText` / `DH/DV/DHDVText`) are **rasterised**
//! (round 352): the glyph bytes are drawn through the crate's built-in
//! clean-room ASCII bitmap face ([`font`]) at the text-drawing baseline
//! pen, scaled by `txSize` and the `TxRatio` (`$0010`) horizontal /
//! vertical `numer/denom` factors (round 361), inked in `fgColor`,
//! honouring the `srcOr` / `srcXor` / `srcBic` text source modes plus
//! the `grayishTextOr = 49` dimmed-text blend (round 407, Inside
//! Macintosh Volume VI page 17-17), synthesising the `txFace` styles —
//! bold / italic / underline / outline / shadow / condense / extend —
//! per Volume I pages I-151/I-152 with the I-226 characterization-table
//! amounts (round 407), and advancing the pen by each styled glyph plus
//! `chExtra` / `spExtra` plus the `lineJustify` (`$002D`) intercharacter
//! spacing (round 361). PICT carries no font data, so text is legible,
//! spec-positioned and spec-styled, but not pixel-identical to a
//! particular Mac font (the glyph artwork is the crate's own face).
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
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub mod font;
pub mod header;
pub mod image;
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub mod opcodes;
pub mod ops;
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub mod packbits;
pub mod probe;
pub mod quicktime;
pub mod raster;
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub mod reader;
// internal — exposed for tests/fuzz; not part of the stable API
#[doc(hidden)]
pub mod region;
#[cfg(feature = "registry")]
pub mod registry;
pub mod state;

/// Codec id for PICT image frames.
pub const CODEC_ID_STR: &str = "pict";

pub use decoder::{parse_pict, MAX_RASTER_BYTES};
pub use encoder::{
    build_clip_rgn_rect, build_direct_bits_rect_op, build_direct_bits_rect_op_with_mode,
    build_pix_pat_dither_op, build_pix_pat_op, build_pix_pat_op_sized, encode_pict,
    encode_pict_bits_rect, encode_pict_bits_rgn, encode_pict_indexed_bits_rect,
    encode_pict_indexed_bits_rgn, encode_pict_indexed_pack_bits_rect,
    encode_pict_indexed_pack_bits_rgn, encode_pict_pack_bits_rect, encode_pict_pack_bits_rgn,
    encode_pict_v1, encode_pict_v1_bits_rect, encode_pict_v1_pack_bits_rect, encode_pict_v1_with,
    encode_pict_v2, encode_pict_v2_with_clip, pixel_data_sizes, IndexedPixelSize, PackType,
    PixPatSlot,
};
pub use error::{PictError, Result};
pub use header::{Fixed, PictHeader};
pub use image::{PictComment, PictImage, PictPixelFormat, PictQuickTime};
pub use ops::{
    build_arc_op, build_bk_color_code, build_bk_pat, build_ch_extra, build_clip_rgn,
    build_compressed_quicktime, build_compressed_quicktime_image, build_def_hilite, build_dh_text,
    build_dhdv_text, build_dv_text, build_fg_color_code, build_fill_pat, build_font_name,
    build_glyph_state, build_hilite_color, build_hilite_mode, build_line, build_line_from,
    build_line_justify, build_long_comment, build_long_comment_v1, build_long_text, build_op_color,
    build_origin, build_oval_op, build_oval_size, build_pn_loc_h_frac, build_pn_mode, build_pn_pat,
    build_pn_size, build_poly_op, build_rect_op, build_rgb_bk_col, build_rgb_fg_col,
    build_rgn_inverted_op, build_rgn_rect_op, build_round_rect_op, build_same_arc_op,
    build_same_oval_op, build_same_rect_op, build_same_round_rect_op, build_short_comment,
    build_short_comment_v1, build_short_line, build_short_line_from, build_sp_extra, build_tx_face,
    build_tx_font, build_tx_mode, build_tx_ratio, build_tx_size, build_uncompressed_quicktime,
    build_uncompressed_quicktime_image, PictBuilder, PictV1Builder, Verb,
};
pub use probe::{probe_pict, PictProbe, ProbeQuickTime, ProbeRect, ProbeTermination, ProbeVersion};
pub use quicktime::{
    parse_compressed_quicktime, parse_uncompressed_quicktime, ImageDescription,
    QuickTimeCompressed, QuickTimeMatrix, QuickTimeMatte, QuickTimePayload, QuickTimeUncompressed,
};
pub use raster::{
    blend_arith, blend_source, ArithMode, PatternMode, SourceMode, GRAYISH_TEXT_OR_MODE,
    HILITE_MODE,
};
pub use state::{
    Pattern, PictFontName, PictGlyphState, PictLineJustify, PictPattern, PictTextFace,
    PictTextState, PixPattern, Rgba, TextRatio,
};

#[cfg(feature = "registry")]
pub use registry::{
    __oxideav_entry, quicktime_codec_parameters, register, register_codecs, register_containers,
    resolve_quicktime_codec,
};
