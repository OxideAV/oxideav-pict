# oxideav-pict

[![CI](https://github.com/OxideAV/oxideav-pict/actions/workflows/ci.yml/badge.svg)](https://github.com/OxideAV/oxideav-pict/actions/workflows/ci.yml) [![crates.io](https://img.shields.io/crates/v/oxideav-pict.svg)](https://crates.io/crates/oxideav-pict) [![docs.rs](https://docs.rs/oxideav-pict/badge.svg)](https://docs.rs/oxideav-pict) [![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Pure-Rust PICT (Apple QuickDraw picture) reader + writer for the
[`oxideav`](https://github.com/OxideAV/oxideav) framework. Clean-room
implementation of the public **Inside Macintosh** books — *Imaging With
QuickDraw* (1994) plus Volumes I (1985), V (1986) and VI (1991) for the
classic QuickDraw / Color QuickDraw behaviour the opcode tables defer
to; no external implementation source consulted.

## Decode

PICT is opcode-based: the file is a stream of QuickDraw drawing
commands. The decoder walks both the v2 (16-bit, word-aligned) and v1
(8-bit) opcode streams, steps a drawing-state machine, and folds every
command — lines, rectangles, round-rects, ovals, arcs, polygons,
regions, embedded rasters, **and text** — onto an in-crate
software-rasteriser RGBA canvas sized to `picFrame`. The result is
returned as a `PictImage`.

| Opcode group | Behaviour |
| ------------ | --------- |
| NOP, ClipRgn, pen / colour / text-state opcodes | parsed; rasteriser tracks state; full text state captured into [`PictTextState`] |
| Line / LineFrom / ShortLine | drawn via Bresenham, honouring the pen size + pen pattern / pixel-pattern + pattern mode (book page 3-81) |
| Frame / Paint / Erase / Invert / Fill of Rect / RoundRect / Oval / Arc | rasterised via in-crate kernels; every `Frame*` verb honours the pen size + pen pattern / pixel-pattern + pattern mode (book page 3-13 "Framing Shapes") |
| Frame / Paint / Erase / Invert / Fill Poly | rasterised via even-odd scanline; `Frame` honours the pen size + pen pattern / mode (pen hangs below+right, §3 page 3-81) |
| Frame / Paint / Erase / Invert / Fill Rgn | rasterised (bbox + per-row inversion mask); `Frame` honours the pen size + pen pattern / mode (book page 3-13, pen hangs below+right) |
| `BkPat` / `PnPat` / `FillPat` | 8-byte monochrome patterns |
| `BkPixPat` / `PnPixPat` / `FillPixPat` | colour pixel patterns (`patType=1` colour-pixmap, `patType=2` ditherPat) |
| `BitsRect` / `BitsRgn` / `PackBitsRect` / `PackBitsRgn` | 1-bpp BitMap or indexed 1/2/4/8-bit PixMap → RGBA; the embedded `ColorTable` is resolved by each `ColorSpec`'s `value` field (book page 4-55) for a plain pixel-map table, and **sequentially** (entry *n* ↔ pixel value *n*) for a table flagged device (`ctFlags` bit 15) or palette-index (bit 14) — see "Colour tables" below |
| `DirectBitsRect` / `DirectBitsRgn` | 16-bit A1R5G5B5 / 32-bit XRGB\|ARGB → RGBA; `packType` 1 (raw), 2 (drop-pad), 3 (16-bit RLE), 4 (component RLE — the row's planes decoded as one PackBits stream, so emitter runs spanning planes are fine), and 0 → §A-3 page A-16 default packing (3 for 16-bit / 4 for 32-bit when `rowBytes ≥ 8`, else raw) |
| `ShortComment` / `LongComment` | captured as structured [`PictComment`] |
| Text-glyph opcodes (`LongText` / `DH/DV/DHDVText`) | **rasterised** — glyph bytes drawn through a built-in clean-room ASCII bitmap face at the baseline pen, scaled by `txSize` **and the `TxRatio` (`$0010`) horizontal / vertical `numer/denom` factors** (book page 12-13), inked in `fgColor`, advancing the pen by each glyph + `chExtra` / `spExtra` + the `lineJustify` (`$002D`) intercharacter spacing (§A-3 footnote `†`); honours the `srcOr` / `srcXor` / `srcBic` text source modes plus `grayishTextOr = 49` (Inside Macintosh Vol VI page 17-17), and **synthesises the full `txFace` style set** — bold / italic / underline / outline / shadow / condense / extend — per Vol I pages I-151/I-152 with the page I-226 characterization-table amounts |
| CompressedQuickTime / UncompressedQuickTime | payload captured verbatim into `PictImage::quicktime`, parsed into a typed [`QuickTimePayload`] per Inside Macintosh: QuickTime (1993) Tables 3-1 / 3-2, **and rendered**: `$8200`'s image data goes to a [`QuickTimeImageDecoder`] (`'raw '` built in, `'jpeg'` through `oxideav-mjpeg`, anything else through a caller decoder / `oxideav-core` registry) and both opcodes composite through one `StdPix` path — 3×3 matrix, `srcRect`, mask region, `CopyDeepMask` matte, transfer mode; outcome on `PictQuickTime::render` |
| Reserved-for-Apple opcodes | walked past per published payload size |
| OpEndPic | terminate |

Every raster blit (`BitsRect` / `BitsRgn` / `PackBitsRect` /
`PackBitsRgn` / `DirectBitsRect` / `DirectBitsRgn`) honours the
record's `srcRect`: per §A-3 Listings A-2 / A-3 the decoded pixel
buffer covers the full PixMap `bounds`, of which `srcRect` selects the
sub-rectangle actually copied and scaled onto `dstRect`. When
`srcRect == bounds` (the common emitter case) this is the identity
no-op; a `srcRect ⊊ bounds` crops the source before the scaling blit.

The version stanza (`0x0011 0x02FF` for v2, `0x1101` for v1) is
recognised, and the 24-byte `headerOp` is parsed into a structured
[`PictHeader`] (`ExtendedV2` `OpenCPicture` and `V2` `OpenPicture`
shapes). The optional 512-byte launch-stub prefix is auto-detected.
PackBits (§A-5) is implemented at both byte and u16 unit sizes plus
per-channel for packType 4.

### Colour tables and the Palette Manager rule

An indexed PixMap's `ColorTable` is normally keyed by value: *Imaging
With QuickDraw* (book page 4-55) makes each `ColorSpec.value` the pixel
value its RGB belongs to, and the decoder builds the palette from that
field. Two `ctFlags` bits change the rule. Bit 15 marks a device
colour table (book pages 4-104 / 4-120: "high bit: 0 = PixMap; 1 =
device"); bit 14 marks a table whose `value`s are Palette Manager entry
numbers. Apple's *develop* Issue 1 (January 1990), "All About the
Palette Manager", section *Drawing With Palette Colors* (page 29),
states what both mean for indexing — verbatim:

> "a pixMap or pixPat color table may be specified to point to palette
> entries. To do this, set bit 14 in the ctFlags field of the color
> table (ctFlags is called transindex in older equate files). Then set
> the desired palette entry numbers in the value field of each
> colorSpec. The color table is then assumed to be sequential, as
> device tables are (colorSpec 0 refers to pixel value 0 in the pixMap
> or pixPat; color value 1 refers to pixel value 1, and so on)."

So with either bit set the `value` field is **not** a pixel index —
device-private data for a device table, a palette entry number for a
palette-index table — and array position supplies the pixel value. The
decoder applies the sequential rule to both (round 461 re-verified the
bit-15 change of 2026-09 against the article and extended it to bit 14;
`tests/synth_v2_round372_colortable.rs` pins each). For a palette-index
table a PICT reader has no live Palette Manager to resolve the entry
numbers against, so the entry's own RGB is used — the colours the
article's desktop-pattern example starts from before `AnimatePalette`
would replace them.

### Patterns

The three monochrome pattern slots (`PnPat` / `BkPat` / `FillPat`) and
the three colour slots (`PnPixPat` / `BkPixPat` / `FillPixPat`) are
honoured by the rasteriser. Colour PixPat tiles support any power-of-2
`bounds` (§3); a non-power-of-2 or zero dimension falls back to the
monochrome `Pat1Data`. The `ditherPat` sub-type resolves its target RGB
at every cell (exact on a true-colour canvas).

```rust
use oxideav_pict::{parse_pict, PictPixelFormat};

let img = parse_pict(&std::fs::read("photo.pct")?)?;
assert_eq!(img.pixel_format, PictPixelFormat::Rgba);
assert_eq!(img.data.len(), img.width as usize * img.height as usize * 4);
# Ok::<(), Box<dyn std::error::Error>>(())
```

### Transfer modes

- **Boolean pattern modes** (`patCopy = 8` … `notPatBic = 15`, §3) are
  honoured per cell on every patterned fill / frame / paint / erase verb.
- **Arithmetic transfer modes** (`blend = 32` … `adMin = 39`, §4) are
  honoured on pattern fills and on the `CopyBits` raster blit, resolved
  against the declared `OpColor`.
- **Boolean source modes** (`srcCopy = 0` … `notSrcBic = 7`, §3/§4) are
  honoured on the `CopyBits` raster blit via each record's `mode` word,
  with the §4 Table 4-1 foreground/background colour semantics.
- **Highlighting** (`hilite = 50`) is honoured on both pattern fills and
  the raster blit, using the `HiliteColor` opcode (reverting to `srcXor`
  when none was emitted).
- **Dimmed text** (`grayishTextOr = 49`, text-only) inks glyphs in the
  per-channel fg/bg average per Inside Macintosh Vol VI page 17-17
  (Vol VI notes the mode is not normally stored in pictures; this is
  defensive-decode fidelity).
- `ditherCopy = 64` is recognised and stripped (no-op on RGBA).

Invert verbs (`InvertRect` / `InvertRRect` / `InvertOval` / `InvertArc`
/ `InvertPoly`) apply a channel-wise NOT over the shape interior and are
their own inverse. Structured text / pen-mode / highlight state opcodes
(`TxFont`, `TxFace` as a typed style bitfield, `TxMode`, `OpColor`,
`fontName`, `lineJustify`, `glyphState`, …) are captured into
[`PictTextState`] for round-trip tooling; `TxMode` resolves to a typed
[`SourceMode`] via `tx_source_mode`.

## Encode

| Function | Format |
| -------- | ------ |
| `encode_pict` / `encode_pict_v2(…, PackType)` | v2, packType 1 (raw) / 2 (packed24) / 3 (Rle16) / 4 (ComponentPackBits) |
| `encode_pict_v1` / `encode_pict_v1_with(…, PackType)` | v1 framing around a v2-style `DirectBitsRect $9A` (an extension — §A-3 Table A-3 defines no `$9A`; readable by this crate, not by strict Table-A-3 readers), no stub / headerOp |
| `encode_pict_v1_bits_rect` / `encode_pict_v1_pack_bits_rect` | strict Table-A-3 v1 raster: 1-bpp BitMap via `$90` (footnote `‡`: `rowBytes < 8` only) / `$98` PackBits rows |
| `encode_pict_bits_rect` / `encode_pict_pack_bits_rect` | v2 1-bpp BitMap (raw / PackBits-RLE rows) |
| `encode_pict_bits_rgn` / `encode_pict_pack_bits_rgn` | masked 1-bpp variants with rectangular clip region |
| `encode_pict_indexed_*` (`bits_rect` / `pack_bits_rect` + `*_rgn`) | indexed 1/2/4/8-bpp PixMap with embedded ColorTable |
| `encode_pict_v2_with_clip` | v2 with a `ClipRgn` opcode before pixel data |
| `ops::PictBuilder` | drawing-command synth (lines incl. `ShortLine*` compact forms / shapes incl. the same-shape replay verbs / regions / clip / `Origin` / patterns / comments / raster / text — `LongText` + `DH/DV/DHDVText`), chainable |
| `ops::PictV1Builder` | the same opcode chunks assembled as a **v1** stream (1-byte opcodes, `$11 $01` stanza, `picSize` recorded); rejects Color-QuickDraw-only opcodes at build time |

Every v2 emit path writes a canonical extended-v2 header
(`version=-2`, `hRes=vRes=72.0` dpi, `optimal_source_rect = picFrame`);
pre-header PICTs are still accepted on decode.

```rust
use oxideav_pict::{encode_pict_v2, parse_pict, PackType};

let rgba = vec![0u8; 4 * 4 * 4];
let pict = encode_pict_v2(4, 4, &rgba, PackType::Rle16)?;
let img = parse_pict(&pict)?;
assert_eq!(img.width, 4);
# Ok::<(), Box<dyn std::error::Error>>(())
```

### Embedded QuickTime images

Imaging With QuickDraw §A-3 declares the `$8200` / `$8201` payloads
"private to QuickTime"; the layout is published in **Inside Macintosh:
QuickTime** (1993), Chapter 3 "Image Compression Manager" (Tables
3-1 / 3-2, pages 3-25 – 3-27; `ImageDescription` pages 3-49 – 3-51),
which the `quicktime` module parses, and the `$8200` opcode is the
`StdPix` call (pages 3-137 – 3-139) serialised field for field — which
is what the decoder replays.

* **Codec boundary.** The image data's compressor is named by the
  `ImageDescription` `cType` FourCC (page 3-50), so `oxideav-pict`
  never implements a compressor: `parse_pict` hands the description +
  bytes to a [`QuickTimeImageDecoder`] and composites the RGBA it
  returns. The default chain ([`DefaultQuickTimeDecoder`]) covers
  `'raw '` (Table 3-3's compressor that "does not compress": the data
  is the pixel map at the description's `depth` — 32 / 24 / 16-bit)
  and, with the `registry` feature, `'jpeg'` ("Photo - JPEG") through
  the sibling `oxideav-mjpeg` decoder — the compressor every
  QuickTime-emitted `$8200` found in the wild carries.
  `registry::RegistryQuickTimeDecoder` resolves the FourCC through a
  caller's `oxideav_core::CodecRegistry` first (`'cvid'` → Cinepak,
  …) and folds the decoded frame to RGBA; `parse_pict_with` takes any
  decoder of your own. A compressor nobody decodes leaves the canvas
  alone, keeps the wrapper typed, and is reported as
  `QuickTimeRender::Unsupported { codec, .. }` — the page 3-26 posture
  of honouring `Size` and carrying on.
* **Matrix** (`pict-quicktime-matrix.md`): row-vector convention of
  Figure 2-19 (book page 2-26) — `x' = a·x + c·y + tx`,
  `y' = b·x + d·y + ty`, serialised `a b u / c d v / tx ty w` with the
  third column `Fract` 2.30 (page 2-28), so the emitter identity ends
  in `0x40000000`. The matrix "specifies the mapping of the source
  rectangle to the destination" (page 3-138): the effective
  destination is `TransformRect(matrix, srcRect)` (pages 2-348 –
  2-352), exact for scale / translate; rotation, skew and the
  (inferred) perspective divide use the corner bounding box with each
  destination pixel centre inverse-mapped to its nearest source
  pixel. The books leave the fixed-point → integer rounding open
  (matrix note §9); this crate rounds `TransformRect` half-up and
  samples pixel centres, which is exact for integer scale factors.
* **`srcRect`** crops in source space (`(0,0)–(desc.width,
  desc.height)`, page 3-78); the **mask region** clips in destination
  space (page 3-138) and is never transformed; the **matte** lives in
  source space with `matteRect` cropping it (pages 3-81 / 3-139) and
  blends per Imaging With QuickDraw `CopyDeepMask` (book page 3-120,
  which the QuickTime book routes through `StdPix`): "A black mask
  pixel value means … take the source pixel; a white value means …
  take the destination pixel. Intermediate values specify a weighted
  average … `(1 – mask) × source + (mask) × destination`", per colour
  component. A matte that cannot be decoded is skipped and reported
  (`Rendered { matte_skipped: Some(..) }`) rather than blocking the
  image. **`Mode`** resolves like every other raster opcode.
* **`$8201` (UncompressedQuickTime)** wraps one ordinary `$98`–`$9B`
  pixel-data subopcode inside its `Size` window; it is decoded through
  the normal raster path and then composited through the *same* code
  as `$8200`, so the wrapper's matrix and matte apply to it too.
* **Default warning placeholder.** `StdPix` appends "default picture
  opcodes (for displaying a warning when QuickTime is not installed)"
  after the `$8200` (page 3-139) — the "QuickTime™ and a *name*
  decompressor are needed to see this picture" `LongText` lines every
  emitter-written file carries. Once the image has drawn, the decoder
  skips that run (pen / text-state and text-drawing opcodes only,
  ending at the emitter's `NOP`) and reports the count on
  `Rendered { placeholder_skipped }`; an unsupported compressor still
  shows the warning. The books do not state the suppression mechanism
  — the lookahead is this crate's reading.
* **Validation.** Hand-built `'raw '` fixtures pin identity / crop /
  scale / rotation / mask / matte / mode pixel-exact
  (`tests/synth_v2_round461_quicktime.rs`). The genuine QuickTime
  emitter sample `JDSnowyBlog.pct` (500×281 "Photo - JPEG", SHA-256 and
  structure in `docs/image/quickdraw/pict-8200-real-world-fixtures.md`;
  not vendored) renders within 45.6 dB PSNR of an ImageMagick
  black-box render, 18 of 140 500 pixels beyond 3 %, peak 24/255 —
  JPEG-decoder-level agreement — with the three warning lines
  suppressed.
* **Degradation** (page 3-26): a payload interior that doesn't match
  the published layout keeps the verbatim `PictQuickTime::data`
  capture with `image = None` / `render = NotAttempted` and never
  fails the picture.
* **Emit**: `QuickTimeCompressed::still` /
  `QuickTimeUncompressed::wrapping` +
  `PictBuilder::compressed_quicktime_image` /
  `uncompressed_quicktime_image` compose conforming opcodes (e.g. for
  embedding a JPEG in a PICT); `QuickTimeMatrix::rect_matrix` builds
  the `RectMatrix` placement; emit → parse round-trips compare equal
  at the typed level.

## Probe (read-only introspection)

[`probe_pict`] returns a `PictProbe` summary without rasterising —
useful for thumbnail UIs, content scanners (spotting embedded QuickTime
before paying decode cost), and encoder tests asserting an opcode mix.
It shares its opcode walker with the decoder and surfaces counts
(`raster_count`, `indexed_raster_count`, `drawing_count`,
`comment_count`, `reserved_op_count`, `text_state_op_count`), the parsed
`header` / `text_state` / `comments`, per-opcode QuickTime wrapper
summaries (`quicktime: Vec<ProbeQuickTime>` — compressor FourCC,
dimensions, depth, matte / mask presence, `$8201` subopcode — without
retaining payload bytes), and a `termination` reason.

```rust
use oxideav_pict::{encode_pict, probe_pict, ProbeVersion};

let pict = encode_pict(8, 8, &vec![0x80u8; 8 * 8 * 4])?;
let p = probe_pict(&pict)?;
assert_eq!(p.version, ProbeVersion::V2);
assert_eq!(p.raster_count, 1);
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Hostile-input hardening

PICT length fields (`picFrame`, PixMap `bounds` / `rowBytes`, region /
polygon sizes …) are attacker-controlled. Every decode-side buffer
sized from them is checked against the [`MAX_RASTER_BYTES`] budget
(256 MiB) with overflow-safe arithmetic before allocation, and raw
pixel rows must physically fit their declared bounds width. The
`hostile_round401` test suite drives `parse_pict` / `probe_pict`
through every truncation prefix of an opcode-family corpus, seeded
byte mutations, systematic length-field maxing, and hand-crafted
giant-header records — the decoder returns `Err`, it never panics.

Emitter tolerance (round 461): ImageMagick's PICT writer emits a
one-byte per-scanline PackBits count even when `rowBytes > 250`
(§A-3 calls for a word); the decoder and probe take the byte reading
only when the word reading cannot describe the row, so conforming
streams are never reinterpreted. `tests/emitter_round461_imagemagick.rs`
pins a tool-generated fixture byte-identical to ImageMagick's own
render of it.

`fuzz/` carries three `cargo fuzz` targets (round 461): `parse_pict`
(whole-file decode through the default QuickTime decoder chain),
`probe_pict` (the decode-free walker + the typed `$8200` / `$8201`
payload parsers) and `quicktime_8200` (the fuzzer's bytes become the
interior of one `$8200` — and one `$8201` — opcode inside a valid
picture, so the matrix / mask / matte / mode compositor and the
`'raw '` + `'jpeg'` decoders are hit far more often than a whole-file
target reaches them). Run with
`cargo +nightly fuzz run quicktime_8200 -- -max_len=65536`.

## Standalone vs registry-integrated

The default `registry` Cargo feature pulls in `oxideav-core` and exposes
the framework `Decoder` trait surface plus a `registry::register` entry
point. Disable it for an `oxideav-core`-free build that still exposes
`parse_pict` / `encode_pict` plus crate-local `PictImage` /
`PictPixelFormat` / `PictError` types.

```toml
[dependencies]
oxideav-pict = "0.0"                                        # framework
oxideav-pict = { version = "0.0", default-features = false } # standalone
```

## What's not yet in

* **System-font fidelity.** Text opcodes **are** rasterised, positioned,
  scaled, styled (`txFace` synthesis per Inside Macintosh Vol I — round
  407) and moded (`srcOr` / `srcXor` / `srcBic` / `grayishTextOr`) per
  spec — but the glyph artwork itself is the crate's own built-in
  clean-room ASCII bitmap face. PICT embeds no font data and the actual
  Mac system-font bitmaps live in resource files, not in any staged
  spec, so text is legible and spec-faithful in geometry and styling
  without being pixel-identical to a particular Mac font. (Within the
  spec-set rules, two knobs are book-acknowledged implementation
  choices: the italic shear rate — Vol I page I-227 leaves the
  characterization-table factor's exact use to experimentation, this
  crate fixes `8` as a `>>4` slope, one pixel per two rows — and the
  bold smear count for "an appropriate number of times", which the
  screen table sets to 1.)
* **QuickTime compressors without a decoder.** `$8200` renders for
  `'raw '` (16 / 24 / 32-bit) and `'jpeg'`; `'rle '`, `'rpza'`,
  `'smc '` and friends have no workspace decoder yet and are reported
  `Unsupported` (a caller `CodecRegistry` that carries one is used
  automatically). Indexed / grayscale `'raw '` depths (1–8, 34 / 36 /
  40) need the colour table named by `clutID` — a system `'clut'` or
  a custom table in the description's extension bytes, whose on-disk
  layout is not in the staged books — and are `Unsupported` too.
* **Placeholder suppression is structural.** The default warning
  run after a rendered `$8200` is recognised by shape (state + text
  opcodes ending at a `NOP`), not by a documented marker; a picture
  that draws a caption and a `NOP` right after a `$8200` would lose
  the caption. `Rendered { placeholder_skipped }` says when it fired.
* **Multi-image PICTs.** Each raster blits onto the same canvas — no
  separate per-image surfaces.

## License

[MIT](LICENSE) — Copyright (c) 2026 Karpelès Lab Inc.
