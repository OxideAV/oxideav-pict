# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

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

### Deferred to round 2

- Drawing-command extraction (lines / polygons / regions / text glyphs
  raster-rendered to a synthesised image canvas). Round 1 only
  surfaces *raster* bitmaps embedded via PackBitsRect / DirectBitsRect.
- PackType 2 / 3 / 4 of DirectBitsRect (component-separated and packed
  16-bit RLE planes).
- `PackBitsRgn` (`0x0099`) and `DirectBitsRgn` (`0x009B`) region-clipped
  raster paths.
- `CompressedQuickTime` (`0x8200`) opcode — embedded JPEG / Animation /
  RLE QuickTime ImageDescription decode.
- v1 raster opcodes (`BitsRect` `0x90`, `BitsRgn` `0x91`, `PackBitsRect`
  `0x98` 8-bit form, `PackBitsRgn` `0x99` 8-bit form). v1 *header* is
  parsed; v1 raster opcodes return `Unsupported` in round 1.
- Multi-image PICT files (current API surfaces only the *first*
  extractable raster).
- PICT writing — many opcodes to emit, old-Mac-only consumer base.
