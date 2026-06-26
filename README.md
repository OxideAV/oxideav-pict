# oxideav-pict

Pure-Rust PICT (Apple QuickDraw picture) reader + writer for the
[`oxideav`](https://github.com/OxideAV/oxideav) framework. Clean-room
implementation of the public **Inside Macintosh: Imaging With QuickDraw**
(Apple, 1994); no external implementation source consulted.

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
| `BitsRect` / `BitsRgn` / `PackBitsRect` / `PackBitsRgn` | 1-bpp BitMap or indexed 1/2/4/8-bit PixMap → RGBA |
| `DirectBitsRect` / `DirectBitsRgn` | 16-bit A1R5G5B5 / 32-bit XRGB\|ARGB → RGBA; `packType` 1 (raw), 2 (drop-pad), 3 (16-bit RLE), 4 (component RLE), and 0 → §A-3 page A-16 default packing (3 for 16-bit / 4 for 32-bit when `rowBytes ≥ 8`, else raw) |
| `ShortComment` / `LongComment` | captured as structured [`PictComment`] |
| Text-glyph opcodes (`LongText` / `DH/DV/DHDVText`) | **rasterised** — glyph bytes drawn through a built-in clean-room ASCII bitmap face at the baseline pen, scaled by `txSize` **and the `TxRatio` (`$0010`) horizontal / vertical `numer/denom` factors** (book page 12-13), inked in `fgColor`, advancing the pen by each glyph + `chExtra` / `spExtra` + the `lineJustify` (`$002D`) intercharacter spacing (§A-3 footnote `†`); honours the `srcOr` / `srcXor` / `srcBic` text source modes |
| CompressedQuickTime / UncompressedQuickTime | length-prefixed skip (embedded image not decoded) |
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
| `encode_pict_v1` / `encode_pict_v1_with(…, PackType)` | v1, same PackType selector, no stub / headerOp |
| `encode_pict_bits_rect` / `encode_pict_pack_bits_rect` | v2 1-bpp BitMap (raw / PackBits-RLE rows) |
| `encode_pict_bits_rgn` / `encode_pict_pack_bits_rgn` | masked 1-bpp variants with rectangular clip region |
| `encode_pict_indexed_*` (`bits_rect` / `pack_bits_rect` + `*_rgn`) | indexed 1/2/4/8-bpp PixMap with embedded ColorTable |
| `encode_pict_v2_with_clip` | v2 with a `ClipRgn` opcode before pixel data |
| `ops::PictBuilder` | drawing-command synth (lines / shapes / regions / patterns / comments / raster), chainable |

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

## Probe (read-only introspection)

[`probe_pict`] returns a `PictProbe` summary without rasterising —
useful for thumbnail UIs, content scanners (spotting embedded QuickTime
before paying decode cost), and encoder tests asserting an opcode mix.
It shares its opcode walker with the decoder and surfaces counts
(`raster_count`, `indexed_raster_count`, `drawing_count`,
`comment_count`, `reserved_op_count`, `text_state_op_count`), the parsed
`header` / `text_state` / `comments`, and a `termination` reason.

```rust
use oxideav_pict::{encode_pict, probe_pict, ProbeVersion};

let pict = encode_pict(8, 8, &vec![0x80u8; 8 * 8 * 4])?;
let p = probe_pict(&pict)?;
assert_eq!(p.version, ProbeVersion::V2);
assert_eq!(p.raster_count, 1);
# Ok::<(), Box<dyn std::error::Error>>(())
```

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

* **System-font fidelity.** Text opcodes **are** rasterised (`LongText` /
  `DH/DV/DHDVText` draw glyphs at the baseline pen, scaled by `txSize`,
  inked in `fgColor`, honouring the `srcOr` / `srcXor` / `srcBic` text
  source modes). The glyph artwork is the crate's own built-in clean-room
  ASCII bitmap face — PICT embeds no font data, and Imaging With QuickDraw
  defers the actual system-font bitmaps + `txFace` bold/italic/outline
  style synthesis to a separate book ("the chapter 'Font Manager' in
  Inside Macintosh: Text") that is not in this crate's reference set. So
  text is legible and positioned per spec, but not pixel-identical to a
  particular Mac font. Text *geometry* that **is** fully spec-determined —
  `txSize` cell scaling, the `TxRatio` (`$0010`) horizontal / vertical
  scaling factors, and the `lineJustify` (`$002D`) intercharacter spacing
  — is applied to the built-in face. The `txFace` style bits (bold /
  italic / underline pixel synthesis) and the `grayishTextOr = 49`
  shading mode remain tracked-but-not-synthesised: their per-pixel
  geometry lives in the absent Font Manager / Color-QuickDraw chapters.
* **CompressedQuickTime decode.** The opcode is parsed (payload skipped
  cleanly), but the embedded image (typically JPEG) is not decoded.
* **Multi-image PICTs.** Each raster blits onto the same canvas — no
  separate per-image surfaces.

## License

[MIT](LICENSE) — Copyright (c) 2026 Karpelès Lab Inc.
