//! Pure-Rust PICT (Apple QuickDraw picture) reader.
//!
//! Clean-room implementation of the public **Inside Macintosh: Imaging
//! With QuickDraw** (Apple, 1994). No Apple QuickDraw source, no
//! `image` crate's PICT submodule (if any), no Bitmap.framework, no
//! GIMP PICT plugin, no libavif PICT path, no Wine PICT-conversion
//! code, no NetPBM `picttoppm` source consulted.
//!
//! ## Status
//!
//! PICT is opcode-based: the file is a stream of QuickDraw drawing
//! commands. Round 1 *recognises* the entire v2 opcode roster — pen /
//! colour / font / pattern state, line / rect / oval / round-rect /
//! arc / poly / region drawing verbs, text glyph emit, comments — and
//! correctly walks past every opcode by its published operand size,
//! so the decoder can reach the *raster* opcodes (`PackBitsRect` =
//! `0x0098`, `DirectBitsRect` = `0x009A`) regardless of how much
//! drawing-state padding the encoder chose to emit.
//!
//! The drawing commands themselves aren't *rasterised* in round 1 —
//! the decoder returns the first raster bitmap it finds. PICTs that
//! contain only drawing commands (no `PackBitsRect` / `DirectBitsRect`)
//! produce [`PictError::NoRaster`].
//!
//! v1 PICTs (8-bit opcodes) are detected (sentinel `0x1101`) but
//! their raster opcodes return [`PictError::Unsupported`]; the
//! workspace's modern targets are all v2.
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
pub mod error;
pub mod image;
pub mod opcodes;
pub mod packbits;
pub mod reader;
#[cfg(feature = "registry")]
pub mod registry;

/// Codec id for PICT image frames.
pub const CODEC_ID_STR: &str = "pict";

pub use decoder::parse_pict;
pub use error::{PictError, Result};
pub use image::{PictImage, PictPixelFormat};

#[cfg(feature = "registry")]
pub use registry::{register, register_codecs, register_containers};
