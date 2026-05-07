# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Round 4: `PackType::Rle16` — packType 3 emit. Each row is packed as
  A1R5G5B5 (alpha bit always set) then PackBits-RLE'd at u16 unit
  size. Self-roundtrips through `parse_pict` and is accepted by
  ImageMagick's PICT delegate. Adds 5-bit-per-channel quantisation
  (decoder reproduces `0xFF` losslessly via 5→8 bit replication).
- Round 4: `packbits::encode_u16` — public PackBits encoder at u16
  unit size. Mirrors `encode` (existing 8-bit) but each replicated
  packet carries a u16 BE pixel.
- Round 4: `ops` module — v2 opcode-builder API. Public surface:
  `Verb` enum (Frame / Paint / Erase / Invert / Fill); low-level
  byte builders `build_line` / `build_line_from` / `build_rect_op` /
  `build_round_rect_op` / `build_oval_op` / `build_arc_op` /
  `build_poly_op` / `build_rgn_rect_op` / `build_rgn_inverted_op` /
  `build_rgb_fg_col` / `build_rgb_bk_col` / `build_pn_size` /
  `build_oval_size`; high-level `PictBuilder` that owns the launch
  stub + picture-record header + headerOp stanza + per-opcode word
  alignment + OpEndPic. Drawing-only PICT streams now self-roundtrip
  through `parse_pict` and pass ImageMagick cross-decode.
- Round 4: `tests/synth_v2_round4.rs` — 12 tests covering packType=3
  round-trip + size compression + every drawing-command opcode family
  via the builder + region inversion-encoded emit + ImageMagick
  cross-decoder validation (`magick` invoked with a tempfile to
  preserve the launch-stub seek).

- Round 3: `encode_pict_v2` with `PackType` selector: `Raw` (packType 1,
  round-2 behaviour), `Packed24` (packType 2, 3 bytes/pixel, 25 % smaller),
  `ComponentPackBits` (packType 4, component-separated PackBits per row,
  typically 20–40 % smaller for photographic content). Self-roundtrips
  through `parse_pict` for all three modes.
- Round 3: `encode_pict_v1` — PICT v1 (8-bit opcode) writer. Emits a
  10-byte picture-record header, v1 sentinel (`0x11 0x01`), and a v1
  `DirectBitsRect` opcode (`0x9A`) with packType=1 raw pixels. No
  512-byte launch-stub prefix. Decoder extended to handle `0x9A`/`0x9B`
  (`DirectBitsRect`/`DirectBitsRgn`) in v1 opcode streams.
- Round 3: `encode_pict_v2_with_clip` — injects a `ClipRgn` (`0x0001`)
  opcode with a rectangular region into a v2 stream immediately after
  the headerOp stanza.
- Round 3: `build_clip_rgn_rect` — builds the raw `ClipRgn` opcode bytes
  for a rectangular region (rgnSize=10, no inversion data).
- Round 3: `pixel_data_sizes` — measurement helper returning
  `(raw_bytes, packed_bytes)` for a given `PackType` without allocating
  the full stream; used in tests to assert byte-savings ratios.
- Round 3: `packbits::encode` promoted from `#[cfg(test)]` to a public
  function so the encoder can use it at runtime.

### Fixed

- v1 decoder: added `DirectBitsRect` (opcode `0x9A`) and
  `DirectBitsRgn` (opcode `0x9B`) to the v1 dispatch table. Previously
  v1 streams with colour direct-bitmap opcodes returned
  `Unsupported("unknown / unsupported v1 opcode 0x9A …")`.

- Round 2: drawing-command rasteriser. Lines, rectangles, round-
  rectangles, ovals, arcs, polygons and regions are folded onto an
  in-crate software-rasteriser canvas (Bresenham line, mid-point
  ellipse, even-odd active-edge-list polygon scanline) sized to
  `picFrame` and pre-filled with the QuickDraw "paper" colour. PICTs
  containing only drawing commands now decode to an actual raster
  instead of returning `NoRaster`. Drawing-state machine
  ([`PictState`]) tracks pen position / size, foreground /
  background colour, oval-corner size, and the last-rect /
  -roundrect / -oval / -arc operands consumed by the *SameRect*
  family.
- Round 2: `DirectBitsRect` packType 2 (3-byte interleaved RGB),
  packType 3 (16-bit u16-PackBits) and packType 4 (component-
  separated PackBits, 3 or 4 channels) decoding.
- Round 2: `BitsRect` (`0x0090`) and `BitsRgn` (`0x0091`)
  uncompressed 1-bpp BitMap decoding.
- Round 2: `PackBitsRgn` (`0x0099`) and `DirectBitsRgn` (`0x009B`)
  region-clipped raster paths — region payload parsed; embedded
  raster decoded and composited (clip-mask use deferred).
- Round 2: PICT v1 (8-bit opcode) raster + drawing decode —
  `BitsRect`, `BitsRgn`, `PackBitsRect`, `PackBitsRgn` all decode;
  same drawing-state machine as v2.
- Round 2: minimal PICT v2 writer (`encode_pict`) emitting one
  `DirectBitsRect` with packType=1 32-bit pixels per the picFrame
  bounds, plus the 512-byte launch-stub prefix.
- Round 2: `Region` parser (`region.rs`) covering both rectangular
  regions (rgnSize == 10) and inversion-encoded regions per the
  Apple §2 algorithm — running x-flip parity per scanline.
- Round 2: `CompressedQuickTime` (`0x8200`) and
  `UncompressedQuickTime` (`0x8201`) opcode parsing — length-prefixed
  payload skipped cleanly so the surrounding PICT decode no longer
  wedges.
- Round 1: clean-room PICT reader per the public **Inside Macintosh:
  Imaging With QuickDraw** (Apple, 1994). PICT v1 (8-bit opcodes) +
  v2 (16-bit, word-aligned opcodes) framing.
- 512-byte launch-stub header detection (skip if `picSize` at offset 512
  plausibly frames the body).
- 8-byte `picFrame` parsing (top, left, bottom, right -> image bounds).
- v2 sentinel detection (`0x0011 0x02FF`) and v1 sentinel detection
  (`0x1101`).
- v2 opcode coverage (recognise + skip): NOP (`0x0000`), Clip
  (`0x0001`), pen state opcodes (PnSize / PnMode / PnPat / OvSize /
  Origin / TxFont / TxFace / TxMode / SpExtra / FgColor / BgColor /
  TxRatio / StdText / TxSize / FillPat / BkPat / RGBFgCol / RGBBkCol /
  HiliteMode / HiliteColor / DefHilite / OpColor / LineJustify), the
  ShortLine / ShortLineFrom / Line / LineFrom family, frame/paint/
  erase/invert/fill rect/oval/round-rect/arc/poly/region opcodes,
  text glyph opcodes (LongText / DHText / DVText / DHDVText), and
  `LongComment` (`0x00A1`).
- v2 raster opcodes that produce a `PictImage`: `PackBitsRect`
  (`0x0098`, packed BitMap, 1 bpp + b/w expansion to RGBA), and
  `DirectBitsRect` (`0x009A`, uncompressed 16/24/32-bit RGB
  direct-pixel, packType=1 only in round 1; expanded to RGBA).
- PackBits RLE decode (`n` byte: 0..=127 = literal n+1 bytes; 129..=255
  = repeat next byte (257-n) times; 128 = NOP) per Inside Macintosh.
- `OpEndPic` (`0x00FF`) terminator handling.
- Default-on `registry` Cargo feature gating the `oxideav-core`
  `Decoder` trait implementation; standalone (no-`registry`) build
  exposes only the framework-free `parse_pict` API surface.
- `register_containers` now registers the canonical PICT file
  extensions (`.pict`, `.pic`, `.pct`) against the `"pict"` codec id
  in the framework `ContainerRegistry`.

### Deferred to round 3

- Drawing-clipping by region. `ClipRgn` is parsed but the resulting
  mask isn't yet honoured by subsequent drawing primitives; same for
  the clip mask in `PackBitsRgn` / `DirectBitsRgn` (the embedded
  raster decodes + blits but doesn't honour the supplied region).
- Pattern fills (`PnPat`, `BkPixPat`, `PnPixPat`, `FillPixPat`).
  Solid-colour ink only — patterns return `Unsupported`.
- Text-glyph rasterisation (`LongText` / `DH/DV/DHDVText`). Currently
  walked-past without rendering — needs a TrueType engine.
- CompressedQuickTime decode. The opcode is parsed (skipped) but the
  embedded image (typically JPEG) is not decoded — needs
  `oxideav-mjpeg`'s `decode_jpeg` exposed publicly.
- Pen-size aware line / frame draws. Pen size is tracked in the
  state machine but the rasteriser draws single-pixel pen only.
- Multi-image PICTs as separate surfaces. Currently each subsequent
  raster blits onto the same canvas.
