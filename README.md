# oxideav-pict

Pure-Rust PICT (Apple QuickDraw picture) reader + writer for the
[`oxideav`](https://github.com/OxideAV/oxideav) framework.

Clean-room implementation of the public **Inside Macintosh: Imaging
With QuickDraw** (Apple, 1994). No Apple QuickDraw source, no
`image` crate's PICT submodule (if any), no Bitmap.framework, no GIMP
PICT plugin, no libavif PICT path, no Wine PICT-conversion code, no
NetPBM `picttoppm` source consulted.

## Decode

PICT is opcode-based: the file is a stream of QuickDraw drawing
commands. Round 2 walks the v2 (16-bit, word-aligned) opcode stream,
steps a small drawing-state machine and folds every command —
lines, rectangles, round-rects, ovals, arcs, polygons, regions and
embedded rasters — onto an in-crate software-rasteriser canvas. The
canvas is sized to `picFrame`, pre-filled with the QuickDraw "paper"
colour (white) and returned as the decoded `PictImage`.

| Opcode   | Name                | Round-2 behaviour       |
| -------- | ------------------- | ----------------------- |
| `0x0000` | NOP                 | skip                    |
| `0x0001` | ClipRgn             | parse region (bbox + inversion data) |
| `0x0002` | **BkPat**           | 8-byte background pattern → `state.back_pat` |
| `0x0009` | **PnPat**           | 8-byte pen pattern → `state.pen_pat`        |
| `0x000A` | **FillPat**         | 8-byte fill pattern → `state.fill_pat`      |
| `0x0003`-`0x0008`, `0x000B`-`0x0010`, `0x0015`, `0x0016`, `0x001A`-`0x001F` | pen / colour / text state | rasteriser tracks fg/bg colour, pen size, oval-corner size, origin; round 230 also captures `TxFont` / `TxFace` / `TxMode` / `SpExtra` / `PnMode` / `TxSize` / `TxRatio` / `PnLocHFrac` / `ChExtra` / `HiliteMode` / `HiliteColor` / `DefHilite` / `OpColor` into [`PictTextState`] |
| `0x0020`-`0x0023` | Line / LineFrom / ShortLine[From] | **draw via Bresenham** |
| `0x0028`-`0x002B` | Long/DH/DV/DHDV Text  | length-prefixed skip (no font rasteriser) |
| `0x002C`-`0x002E` | FontName / LineJustify / GlyphState | size-prefixed skip |
| `0x0030`-`0x006C` | Frame / Paint / Erase / Invert / Fill of Rect / RoundRect / Oval / Arc | **rasterise via in-crate kernel** |
| `0x0070`-`0x0074` | Frame / Paint / Erase / Invert / Fill Poly | **rasterise via even-odd scanline** |
| `0x0080`-`0x0084` | Frame / Paint / Erase / Invert / Fill Rgn | **rasterise (rect bbox + per-row inversion mask)** |
| `0x0090` | **BitsRect**        | **decode -> RGBA** (1-bpp BitMap, raw rows OR indexed PixMap, raw rows when `rowBytes` high bit is set; round 186) |
| `0x0091` | **BitsRgn**         | **decode -> RGBA** (BitsRect + clip region; indexed PixMap also honoured; round 186) |
| `0x0098` | **PackBitsRect**    | **decode -> RGBA** (1-bpp BitMap, PackBits-RLE rows OR indexed 1/2/4/8-bit PixMap, PackBits rows; round 186) |
| `0x0099` | **PackBitsRgn**     | **decode -> RGBA** (PackBitsRect + clip region; indexed PixMap also honoured; round 186) |
| `0x009A` | **DirectBitsRect**  | **decode -> RGBA** (16-bit A1R5G5B5 / 32-bit XRGB|ARGB; packType 0/1 raw, 2 packed 24bpp, 3 u16-PackBits, 4 component-separated PackBits) |
| `0x009B` | **DirectBitsRgn**   | **decode -> RGBA** (DirectBitsRect + clip region) |
| `0x00A0` | **ShortComment**    | parse → `PictComment::short(kind)` (round 224)        |
| `0x00A1` | **LongComment**     | parse → `PictComment::long(kind, data)` (round 224)   |
| `0x8200` | CompressedQuickTime | length-prefixed skip (embedded JPEG/RLE/Animation decode is a future round) |
| `0x8201` | UncompressedQuickTime | length-prefixed skip   |
| `0x00FF` | OpEndPic            | terminate               |
| `0x0024`-`0x0027`, `0x002F`, `0x0035`-`0x0037`, `0x003D`-`0x003F`, `0x0045`-`0x0047`, `0x004D`-`0x004F`, `0x0055`-`0x0057`, `0x005D`-`0x005F`, `0x0065`-`0x0067`, `0x006D`-`0x006F`, `0x0075`-`0x007F`, `0x0085`-`0x008F`, `0x0092`-`0x0097`, `0x009C`-`0x009F`, `0x00A2`-`0x00FE`, `0x0100`-`0x7FFF`, `0x8000`-`0x80FF`, `0x8100`-`0x81FF`, `0x8202`-`0xFFFF` | §A-3 **reserved-for-Apple-use** | published-size skip (fixed, u16-prefixed, u32-prefixed, polySize, rgnSize, or `2 × nn` per §A-3 page A-5 Note) — round 199 |

§A-3 lists `0x0017`-`0x0019` as "Not determined" — those three remain
a hard error rather than risk silently mis-skipping. All other
reserved-for-Apple-use opcodes are walked past per the published
payload size so a PICT carrying a private-extension opcode no longer
aborts the rest of the picture. Probe callers can inspect
`PictProbe::reserved_op_count` to count how many were stepped past.

The PICT version stanza (`0x0011 0x02FF` for v2, `0x1101` for v1) is
recognised. The 24-byte `headerOp` (`0x0C00`) payload that follows
the v2 sentinel is parsed into a structured [`PictHeader`] (round
217) per Inside Macintosh §A-3 "Version and Header Opcodes" + §A-22
Listings A-5 / A-6:

* `PictHeader::ExtendedV2 { hres, vres, optimal_source_rect }` —
  the `OpenCPicture` `version=-2` shape, carrying explicit 16.16
  fixed-point hRes / vRes and an optimal source rectangle (matches
  Listing A-5).
* `PictHeader::V2 { fixed_bounds }` — the `OpenPicture`-in-CGrafPort
  `version=-1` shape, carrying a fixed-point bounding rectangle
  (matches Listing A-6).

Both shapes are surfaced on `PictImage::header` and `PictProbe::header`
as `Option<PictHeader>`. v1 PICTs have no `HeaderOp` per §A-25, so
their decoded image reports `header: None`. The optional
512-byte launch-stub prefix (Apple's pre-OS-X file-manager habit) is
auto-detected by sniffing for a plausible picture record at offset
512.

The encoder side (every v2 emit path — `encode_pict` /
`encode_pict_v2` / `encode_pict_v2_with_clip` /
`encode_pict_bits_rect` / `encode_pict_pack_bits_rect` / the `*_rgn`
counterparts / `encode_pict_indexed_*` / `PictBuilder::new`) emits a
canonical Listing-A-5 extended-v2 header (`version=-2`,
`hRes=vRes=72.0` dpi via `Fixed::SEVENTY_TWO_DPI` = `$00480000`,
`optimal_source_rect = picFrame`, reserved fields zero) instead of
the pre-r217 zero-pad. Pre-r217 PICTs are still accepted on the
decode side — the parser tolerates a leading-word that isn't
`0xFFFE` / `0xFFFF` by falling back to a raw 24-byte skip and
reporting `header: None`.

PackBits (`n` byte: `0..=127` = literal `n+1` bytes; `129..=255` =
repeat next byte `257-n` times; `128` = NOP) is implemented per
Inside Macintosh §A-5; see [`packbits`](src/packbits.rs). The
DirectBitsRect packType-3 variant uses the same RLE algorithm at u16
unit size; packType 4 is byte-PackBits per channel plane.

PICT v1 (8-bit opcodes) parses the same drawing-state machine plus a
smaller raster opcode set (`BitsRect 0x90`, `BitsRgn 0x91`,
`PackBitsRect 0x98`, `PackBitsRgn 0x99`). Round 8 wires up the v1
pattern opcodes too (`BkPat 0x02`, `PnPat 0x09`, `FillPat 0x0A`) — the
same 8-byte monochrome pattern payload, just inside the 8-bit opcode
wrapper.

Round 205 closes the remaining gaps in the v1 dispatcher per Inside
Macintosh §A-3 Table A-3: the **text / font / pen state opcodes**
(`TxFont 0x03`, `TxFace 0x04`, `TxMode 0x05`, `SpExtra 0x06`,
`PnMode 0x08`, `TxSize 0x0D`, `TxRatio 0x10`), the **text-glyph
opcodes** (`LongText 0x28`, `DHText 0x29`, `DVText 0x2A`,
`DHDVText 0x2B` — walked past, no glyph rasterisation), and the
full **Same-shape opcode family** (`frameSameRect..fillSameRect
0x38..0x3C`, `frameSameRRect..fillSameRRect 0x48..0x4C`,
`frameSameOval..fillSameOval 0x58..0x5C`, `frameSameArc..fillSameArc
0x68..0x6C`). The four shape *Same* arms reuse the existing v2
`last_rect` / `last_rrect` / `last_oval` / `last_arc_rect` state
slots, so a v1 picture using the §A-3 payload-elision optimisation
now decodes identically to a v2 picture making the same calls.
`frameSamePoly..fillSamePoly 0x78..0x7C` and `frameSameRgn..
fillSameRgn 0x88..0x8C` are marked "(Not yet implemented)" in §A-3
and accepted as zero-byte no-ops so a private-extension PICT
carrying one doesn't poison the surrounding decode.

## Patterns (round 8)

The three monochrome pattern slots in Inside Macintosh: Imaging With
QuickDraw §A-3 are honoured by the rasteriser:

* **`PnPat` (0x0009 / v1 0x09)** — pen pattern. Consumed by `frame` and
  `paint` verbs of rect / round-rect / oval / poly / region. On-bits
  select the current foreground colour; off-bits select the current
  background colour.
* **`BkPat` (0x0002 / v1 0x02)** — background pattern. Consumed by
  `erase` verbs. On-bits select **background**, off-bits select
  **foreground** (the inverted convention from §A-3 — erase is the
  "paint background" verb so the pattern interpretation flips).
* **`FillPat` (0x000A / v1 0x0A)** — fill pattern. Consumed by `fill`
  verbs (low-nibble `4`).
* `invert` verbs ignore patterns.

Each pattern is 8 bytes representing an 8 row × 8 column on/off
bitmap, tiled across the canvas every 8 pixels on both axes (the
QuickDraw `Pattern` record layout from §A-3 — most-significant bit of
byte 0 is the top-left pixel, least-significant bit of byte 7 the
bottom-right). The default state matches the Mac defaults
(`qd.black = [0xFF; 8]` for pen / fill, `qd.white = [0x00; 8]` for
background), so PICTs that never emit a pattern opcode behave
identically to the round-7 solid-colour pipeline (the all-ones and
all-zeros patterns take a `fill_rect` / `fill_oval` / `fill_polygon`
solid-colour fast path).

```rust
use oxideav_pict::ops::{PictBuilder, Verb};
use oxideav_pict::{parse_pict, Pattern};

const HSTRIPE: [u8; 8] = [0xFF, 0x00, 0xFF, 0x00, 0xFF, 0x00, 0xFF, 0x00];

let mut b = PictBuilder::new(0, 0, 8, 8);
b.fg_color(0xFF, 0x00, 0x00); // red on-bits
b.pen_pattern(HSTRIPE);
b.rect(Verb::Paint, 0, 0, 8, 8);
let img = parse_pict(&b.finish())?;
assert_eq!(img.width, 8);
// Row 0 (on-bits) is red; row 1 (off-bits) is paper white.
assert_eq!(&img.data[0..4], &[0xFF, 0x00, 0x00, 0xFF]);
assert_eq!(&img.data[8 * 4..8 * 4 + 4], &[0xFF, 0xFF, 0xFF, 0xFF]);

// Pattern::BLACK / Pattern::WHITE are the QuickDraw `qd.black` /
// `qd.white` defaults; either collapses to a solid-colour fill.
assert!(Pattern::BLACK.is_solid_fg());
assert!(Pattern::WHITE.is_solid_bg());
# Ok::<(), Box<dyn std::error::Error>>(())
```

## PixPat (round 91 — multi-colour 8×8 pixel pattern)

The three colour pattern slots from Inside Macintosh: Imaging With
QuickDraw §A-3 Listing A-1 are honoured by the rasteriser:

* **`PnPixPat` (`0x0013`)** — colour pen pattern. Consumed by `frame`
  and `paint` verbs of rect / round-rect / oval / poly / region.
* **`BkPixPat` (`0x0012`)** — colour background pattern. Consumed by
  `erase` verbs. (Unlike monochrome `BkPat`, the colour-pixmap variant
  emits the tile's RGB directly — no fg / bg substitution.)
* **`FillPixPat` (`0x0014`)** — colour fill pattern. Consumed by `fill`
  verbs (low-nibble `4`).

A subsequent mono `PnPat / BkPat / FillPat` clears the corresponding
colour slot (classic "most-recent-pattern-wins" QuickDraw semantics).
PixPat is a v2-only feature — v1 PICTs have no PixPat opcodes per
§A-3 Table A-3.

The on-disk record (`patType=1` colour-pixmap sub-type) carries:

1. `PatType: word` — type=1 here; type=2 (ditherPat) falls back to the
   monochrome `Pat1Data` for round 91.
2. `Pat1Data: Pattern` (8 bytes) — monochrome fallback.
3. `PixMap` (sans baseAddr — matches the Listing A-2 BitsRect /
   PackBitsRect convention) — pixelType, pixelSize, cmpCount, cmpSize
   describe the indexed palette format.
4. `ColorTable` — `ctSeed` (4) + `ctFlags` (2) + `ctSize` (2) +
   `(ctSize + 1)` × `ColorSpec` (8 each = value:2 + RGB:6).
5. `PixData` — per-row PackBits / raw indexed-pixel bytes per §A-3.

```rust
use oxideav_pict::ops::{PictBuilder, Verb};
use oxideav_pict::parse_pict;

// Horizontal red / green stripe tile.
let mut tile = [[0u8; 4]; 64];
for y in 0..8 {
    for x in 0..8 {
        tile[y * 8 + x] = if y % 2 == 0 {
            [0xFF, 0, 0, 0xFF]
        } else {
            [0, 0xFF, 0, 0xFF]
        };
    }
}

let mut b = PictBuilder::new(0, 0, 16, 16);
b.pen_pix_pattern([0xFF; 8], &tile)?;
b.rect(Verb::Paint, 0, 0, 16, 16);
let img = parse_pict(&b.finish())?;
// Row 0 (even row of the tile) → red.
assert_eq!(&img.data[0..4], &[0xFF, 0, 0, 0xFF]);
// Row 1 (odd row) → green.
assert_eq!(&img.data[16 * 4..16 * 4 + 4], &[0, 0xFF, 0, 0xFF]);
# Ok::<(), Box<dyn std::error::Error>>(())
```

Tile sizes other than 8×8 fall back to `Pat1Data`. Inside Macintosh
§A-3 nominally permits arbitrary `bounds` rectangles in the PixMap
record, but every real-world PICT we've audited carries an 8×8 tile
(matching the `PixPat` record's 8-byte `Pat1Data` field).

## Dithered PixPat (round 95 — `patType=2`)

The `ditherPat` sub-type (Inside Macintosh §A-3 Listing A-1 with
`patType = 2`) carries only the target `RGBColor` plus the 8-byte
`Pat1Data` monochrome fallback — the actual 8×8 tile is computed by
Color QuickDraw's `MakeRGBPat` (§4-90) at draw time against the active
`GDevice` palette. Quoting §4: *"For an RGB pixel pattern, the
RGBColor record that you specify to the MakeRGBPat procedure defines
the image; there is no image data."*

Our rasteriser draws to a true-colour RGBA canvas (no indexed
`GDevice` in the loop), so the spec contract — *"approximates the
color you specify in the myColor parameter"* — reduces to **emitting
the target RGB at every cell**. This satisfies both the §4 colour-
approximation requirement (zero approximation error on a 24-bit
canvas) and the §A-3 luminance guarantee (*"QuickDraw draws pixel
patterns created with the MakeRGBPat procedure as bit patterns having
approximately the same luminance as the pixel patterns"*) by
construction.

The decoded tile is surfaced on `state::PictState`'s `pen_pix_pat` /
`back_pix_pat` / `fill_pix_pat` slots identically to the `patType=1`
colour-pixmap variant — so every verb routing already wired up for
colour-pixmap (paint / fill / erase across rect / oval / round-rect /
poly / region) carries the dither sub-type for free. A
`PictPattern::DitheredPixmap` enum variant on `state.rs` preserves the
target RGB + `Pat1Data` round-trip when external inspectors need to
distinguish dither from colour-pixmap (e.g. for re-emission against a
different GDevice).

```rust
use oxideav_pict::ops::{PictBuilder, Verb};
use oxideav_pict::parse_pict;

let mut b = PictBuilder::new(0, 0, 8, 8);
// Wrong fg confirms the dither tile overrides the state-machine fg.
b.fg_color(0x00, 0xFF, 0x00); // green — should NOT appear
b.pen_dither_pix_pattern([0xFF; 8], [0xC0, 0x00, 0xC0]); // purple
b.rect(Verb::Paint, 0, 0, 8, 8);
let img = parse_pict(&b.finish())?;
assert_eq!(&img.data[0..4], &[0xC0, 0x00, 0xC0, 0xFF]);
# Ok::<(), Box<dyn std::error::Error>>(())
```

Encoder side: `build_pix_pat_dither_op(slot, fallback, [r, g, b])`
emits the 18-byte payload (opcode word + `patType=2` + 8-byte
`Pat1Data` + 6-byte `RGBColor`). The PICT v2 opcode word is
`0x0012` / `0x0013` / `0x0014` per the `PixPatSlot` enum.
`PictBuilder::{pen,bg,fill}_dither_pix_pattern` are the chainable
convenience wrappers. The 16-bit-per-channel on-disk `RGBColor`
replicates the 8-bit input across both bytes (`high8 = low8 =
channel`) so the decoder's `Rgba::from_rgb16` high-byte selection
round-trips bit-exact.

```rust
use oxideav_pict::{parse_pict, PictPixelFormat};

let pict_bytes: Vec<u8> = std::fs::read("photo.pct")?;
let img = parse_pict(&pict_bytes)?;
assert_eq!(img.pixel_format, PictPixelFormat::Rgba);
assert_eq!(img.data.len(), img.width as usize * img.height as usize * 4);
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Encode

Round 5 widens the encoder to v1-with-PackType, 1-bpp BitMap
emit, and a builder-with-raster path:

| Function | Format | Notes |
| -------- | ------ | ----- |
| `encode_pict` | v2 packType 1 (raw 4 bpp) | Compat alias; identical to round 2 |
| `encode_pict_v2(…, PackType::Raw)` | v2 packType 1 | Largest, broadest compat |
| `encode_pict_v2(…, PackType::Packed24)` | v2 packType 2 | 25 % smaller — no pad byte |
| `encode_pict_v2(…, PackType::Rle16)` | v2 packType 3 | A1R5G5B5 + u16-PackBits per row; typically 30–60 % smaller than raw, with 5-bit-per-channel quantisation |
| `encode_pict_v2(…, PackType::ComponentPackBits)` | v2 packType 4 | Component-separated PackBits per row; typically 20–40 % smaller than raw for photographic content; may be *larger* than raw for random noise |
| `encode_pict_v1` | v1, packType 1 | Compat alias for round-3 behaviour |
| `encode_pict_v1_with(…, PackType)` | v1, packType 1 / 2 / 3 / 4 | round 5 — v1 emit gains the same PackType selector as v2; no 512-byte stub, no headerOp |
| `encode_pict_bits_rect` | v2 + `BitsRect` (`0x0090`) | round 5 — 1-bpp BitMap, raw rows; RGBA reduced via 50 %-luminance threshold |
| `encode_pict_pack_bits_rect` | v2 + `PackBitsRect` (`0x0098`) | round 5 — 1-bpp BitMap, PackBits-RLE rows when `rowBytes >= 8` (raw fall-through for narrower bitmaps) |
| `encode_pict_bits_rgn` | v2 + `BitsRgn` (`0x0091`) | round 6 — 1-bpp BitMap with rectangular clip-region attached after the rect/mode header |
| `encode_pict_pack_bits_rgn` | v2 + `PackBitsRgn` (`0x0099`) | round 6 — masked PackBits-RLE 1-bpp variant; rectangular clip region injected |
| `encode_pict_v2_with_clip` | v2 + `ClipRgn` opcode | Injects rectangular `ClipRgn` before pixel data; honoured by the decoder as a draw-time mask (round 6) |
| `ops::PictBuilder` | v2 drawing-command synth | assembles drawing PICT streams from line / rect / round-rect / oval / arc / polygon / region opcodes (`build_*_op` low-level helpers also exposed) |
| `PictBuilder::raster` | drawing + raster combined | round 5 — appends a `DirectBitsRect` raster onto a builder so callers can mix drawing primitives + raster in the same v2 stream |
| `PictBuilder::pen_pattern` / `bg_pattern` / `fill_pattern` | pattern-set opcodes | round 8 — emits `PnPat` / `BkPat` / `FillPat`; honoured by the decoder's stipple path |
| `build_pn_pat` / `build_bk_pat` / `build_fill_pat` | pattern opcode bytes | round 8 — public helpers for the raw `0x0009` / `0x0002` / `0x000A` opcode bytes |
| `PictBuilder::pen_pix_pattern` / `bg_pix_pattern` / `fill_pix_pattern` | colour pattern-set opcodes | round 91 — emits `PnPixPat 0x0013` / `BkPixPat 0x0012` / `FillPixPat 0x0014` with a fully-resolved 8×8 RGBA tile; honoured by the decoder's colour-pattern path |
| `build_pix_pat_op` | colour pattern opcode bytes | round 91 — public helper for the raw `0x0012` / `0x0013` / `0x0014` opcode bytes (PixPat record with `patType=1` colour pixmap, indexed 8 bpp PixData + ColorTable, PackBits row encoding) |
| `PictBuilder::pen_dither_pix_pattern` / `bg_dither_pix_pattern` / `fill_dither_pix_pattern` | dither pattern-set opcodes | round 95 — emits `PnPixPat 0x0013` / `BkPixPat 0x0012` / `FillPixPat 0x0014` with a `patType=2` ditherPat record (target `RGBColor` + `Pat1Data` only) |
| `build_pix_pat_dither_op` | dither pattern opcode bytes | round 95 — public helper for the 18-byte raw payload (opcode word + patType=2 + Pat1Data + RGBColor) |
| `build_direct_bits_rect_op` | DirectBitsRect opcode bytes | round 5 — public helper for the raw `0x009A` opcode bytes (no stub / header / OpEndPic) |
| `encode_pict_indexed_bits_rect` / `encode_pict_indexed_pack_bits_rect` | v2 + indexed PixMap BitsRect (`0x0090`) / PackBitsRect (`0x0098`) | round 211 — emits a `1/2/4/8`-bpp indexed PixMap with embedded ColorTable (no `baseAddr` per §A-3 footnote `§`); PackBits-RLE rows when `rowBytes >= 8`, raw otherwise |
| `encode_pict_indexed_bits_rgn` / `encode_pict_indexed_pack_bits_rgn` | v2 + indexed PixMap BitsRgn (`0x0091`) / PackBitsRgn (`0x0099`) | round 211 — indexed-PixMap variant of the BitsRgn / PackBitsRgn family with a rectangular clip region attached after the rect/mode header |

```rust
use oxideav_pict::{encode_pict, encode_pict_v2, encode_pict_v1,
                   encode_pict_v1_with, encode_pict_v2_with_clip,
                   encode_pict_bits_rect, encode_pict_pack_bits_rect,
                   parse_pict, PackType};
use oxideav_pict::ops::{PictBuilder, Verb};

// Round-2 compat: raw 32-bpp packType 1.
let rgba = vec![0u8; 4 * 4 * 4];
let pict = encode_pict(4, 4, &rgba)?;
let img = parse_pict(&pict)?;
assert_eq!(img.width, 4);

// packType 3: 16-bpp PackBits at u16 unit size.
let pict3 = encode_pict_v2(4, 4, &rgba, PackType::Rle16)?;
let _ = parse_pict(&pict3)?;

// packType 4: component-separated PackBits (often smaller).
let pict4 = encode_pict_v2(4, 4, &rgba, PackType::ComponentPackBits)?;
let img4 = parse_pict(&pict4)?;
assert_eq!(img4.width, 4);

// v1 format: 8-bit opcodes, no 512-byte stub. Round-3 default
// (raw 32-bpp).
let pict_v1 = encode_pict_v1(4, 4, &rgba)?;
assert!(pict_v1.len() < 512); // no stub

// Round 5: v1 with PackType selector — same compression options as
// v2, just inside the 8-bit-opcode v1 wrapper.
let pict_v1c = encode_pict_v1_with(4, 4, &rgba, PackType::ComponentPackBits)?;
let _ = parse_pict(&pict_v1c)?;

// Round 5: 1-bpp BitMap encoders. Pixels are reduced via a
// 50 %-luminance threshold (Y < 128 → black/bit=1).
let pict_bm = encode_pict_bits_rect(8, 8, &vec![0xFFu8; 8 * 8 * 4])?;
let pict_pbm = encode_pict_pack_bits_rect(64, 16, &vec![0xFFu8; 64 * 16 * 4])?;

// With ClipRgn: clip = [top, left, bottom, right].
let pict_clip = encode_pict_v2_with_clip(4, 4, &rgba, PackType::Raw, [1, 1, 3, 3])?;
let _ = parse_pict(&pict_clip)?; // decoder parses ClipRgn opcode cleanly

// Drawing + raster combined: synth a green page, paste a 4×4 yellow
// raster, then frame it in red.
let mut b = PictBuilder::new(0, 0, 16, 16);
b.fg_color(0x00, 0xFF, 0x00);
b.rect(Verb::Paint, 0, 0, 16, 16);
let raster_rgba = vec![0xFFu8; 4 * 4 * 4]; // bytes interpreted RGBA
b.raster(4, 4, 8, 8, &raster_rgba, PackType::Raw)?;
b.fg_color(0xFF, 0x00, 0x00);
b.rect(Verb::Frame, 4, 4, 8, 8);
let combined_pict = b.finish();
let _img_combined = parse_pict(&combined_pict)?;

// Drawing-only PICT (round-4 path).
let mut b = PictBuilder::new(0, 0, 16, 16);
b.fg_color(0xFF, 0x00, 0x00);
b.rect(Verb::Paint, 4, 4, 12, 12);
b.fg_color(0, 0, 0);
b.rect(Verb::Frame, 2, 2, 14, 14);
let drawing_pict = b.finish();
let img_dr = parse_pict(&drawing_pict)?;
assert_eq!(img_dr.width, 16);
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Probe (read-only introspection)

Round 7 adds [`probe_pict`] — a read-only walker that returns a
`PictProbe` summary without rasterising any pixels. Useful for
thumbnail UIs, content scanners (spotting embedded QuickTime payloads
before paying the JPEG-decode cost), and test harnesses asserting an
encoder emitted the expected opcode mix.

```rust
use oxideav_pict::{encode_pict, probe_pict, ProbeTermination, ProbeVersion};

let rgba = vec![0x80u8; 8 * 8 * 4];
let pict = encode_pict(8, 8, &rgba)?;
let p = probe_pict(&pict)?;
assert_eq!(p.version, ProbeVersion::V2);
assert_eq!(p.width, 8);
assert_eq!(p.height, 8);
assert_eq!(p.raster_count, 1);
assert_eq!(p.drawing_count, 0);
assert!(p.end_pic_seen);
assert_eq!(p.termination, ProbeTermination::EndPic);
assert!(p.has_visible_content());
assert!(!p.has_quicktime());
# Ok::<(), Box<dyn std::error::Error>>(())
```

The probe shares its opcode walker with the decoder: every opcode the
rasteriser observes is counted here. Unsupported opcodes terminate the
walk *without* losing the statistics gathered up to that point — the
caller still sees how many primitives appeared before the failure.

## Standalone vs registry-integrated

The crate's default `registry` Cargo feature pulls in `oxideav-core`
and exposes the framework `Decoder` trait surface plus a
`registry::register` entry point. Disable the feature
(`default-features = false`) for an `oxideav-core`-free build that
still exposes the standalone `parse_pict` / `encode_pict` API plus
crate-local `PictImage` / `PictPixelFormat` / `PictError` types.

```toml
[dependencies]
# Framework integration (default).
oxideav-pict = "0.0"

# Image-library use — no oxideav-core dep.
oxideav-pict = { version = "0.0", default-features = false }
```

## Indexed PixMaps (round 186 — `BitsRect` / `PackBitsRect` / region variants)

Inside Macintosh §A-3 footnote `§` ("The first word following the
opcode is rowBytes. If the high bit of rowBytes is set, then it is a
pixel map containing multiple bits per pixel; if it is not set, it is
a bitmap containing 1 bit per pixel") routes four opcodes through two
on-disk record families:

* `rowBytes` high bit **clear** → 1-bpp BitMap (round 1).
* `rowBytes` high bit **set** → indexed 1/2/4/8-bit PixMap with an
  embedded `ColorTable` (round 186).

The indexed variant carries a full 46-byte PixMap header — same
layout the `PixPat` round-91 decoder already reads, minus the
`baseAddr` placeholder which is exclusive to `DirectBitsRect`
(`0x009A`) / `DirectBitsRgn` (`0x009B`) per §A-3 footnote `§`. The
ColorTable then provides `(ctSize + 1)` `RGBColor` entries; each
PixData index is folded into RGBA via `Rgba::from_rgb16` (high byte).
Out-of-range indices map to `Rgba::BLACK` (§4 *"empty entries in the
ctTable array are drawn as black"*). PixData layout follows §A-3
"PixData": raw rows when `rowBytes < 8` (narrow-row carve-out) or
when the opcode is the unpacked `BitsRect` / `BitsRgn` family;
otherwise per-row `byteCount`-prefixed PackBits at the `rowBytes`-byte
width.

The probe surfaces `PictProbe::indexed_raster_count` — bumped once
per indexed-PixMap `BitsRect` / `BitsRgn` / `PackBitsRect` /
`PackBitsRgn` so callers can spot indexed rasters without paying the
decode cost. `DirectBitsRect 0x009A` / `DirectBitsRgn 0x009B` are
always direct (never indexed) and remain in `raster_count` only.

Round 211 closes the encoder side: `encode_pict_indexed_bits_rect` /
`encode_pict_indexed_pack_bits_rect` (and the `*_rgn` region-clipped
counterparts) emit a v2 PICT stream containing one indexed PixMap at
the chosen `IndexedPixelSize` (1 / 2 / 4 / 8 bpp). The encoder packs
indices MSB-first to match the decoder's `read_indexed_pixel`, omits
`baseAddr` (the BitsRect family drops it; only DirectBits* carries
it), and replicates 8-bit ColorTable RGB across both bytes of the
16-bit-per-channel on-disk `RGBColor` so `Rgba::from_rgb16`
round-trips bit-exact.

```rust
use oxideav_pict::{encode_pict_indexed_pack_bits_rect, parse_pict, IndexedPixelSize};

let palette = vec![[0xFF, 0, 0, 0xFF], [0, 0xFF, 0, 0xFF]];
let indices: Vec<u8> = (0..64).map(|i| ((i / 8) & 1) as u8).collect();
let pict = encode_pict_indexed_pack_bits_rect(
    8, 8, &indices, &palette, IndexedPixelSize::EightBpp,
)?;
let img = parse_pict(&pict)?;
assert_eq!(img.width, 8);
// Row 0 (index 0) → red; row 1 (index 1) → green; …
assert_eq!(&img.data[0..4], &[0xFF, 0, 0, 0xFF]);
assert_eq!(&img.data[8 * 4..8 * 4 + 4], &[0, 0xFF, 0, 0xFF]);
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Picture Comments (round 224 — `ShortComment` / `LongComment`)

Inside Macintosh: Imaging With QuickDraw §A-3 Table A-2 (v2) and
Table A-3 (v1) define two metadata-only opcodes that carry
application-defined annotations alongside the drawing-state stream:

* **`ShortComment`** (`$00A0` v2 / `$A0` v1) — 2-byte `Kind (Integer)`
  word, no further data.
* **`LongComment`** (`$00A1` v2 / `$A1` v1) — 2-byte `Kind` +
  2-byte `size` byte count + `size` raw data bytes.

Round 224 surfaces both records as structured `PictComment` entries
on `PictImage::comments` and `PictProbe::comments`. The decoder
captures `kind` and (for `LongComment`) the on-disk data slice in
stream order; rasterisation is untouched (Picture Comments are
passive metadata). `PictComment::is_long` carries the
`ShortComment`-vs-`LongComment` distinction so consumers can
re-emit the original opcode shape on the encoder side.

```rust
use oxideav_pict::ops::{PictBuilder, Verb};
use oxideav_pict::{parse_pict, probe_pict, PictComment};

let mut b = PictBuilder::new(0, 0, 4, 4);
b.short_comment(0x00C8);
b.long_comment(150, b"PostScriptHandle:8,0,72")?;
b.fg_color(0, 0, 0);
b.rect(Verb::Paint, 0, 0, 4, 4);
let bytes = b.finish();

// Decoder surface.
let img = parse_pict(&bytes)?;
assert_eq!(img.comments.len(), 2);
assert_eq!(img.comments[0], PictComment::short(0x00C8));
assert_eq!(img.comments[1].kind, 150);
assert_eq!(img.comments[1].data, b"PostScriptHandle:8,0,72");
assert!(img.comments[1].is_long);

// Probe surface (read-only walk, same records).
let p = probe_pict(&bytes)?;
assert_eq!(p.comment_count, 2);
assert_eq!(p.comments, img.comments);
# Ok::<(), Box<dyn std::error::Error>>(())
```

Encoder side: `build_short_comment(kind)` / `build_long_comment(kind,
data)` emit raw v2 opcode bytes; `build_short_comment_v1` /
`build_long_comment_v1` emit the v1 (1-byte-opcode) variant.
`PictBuilder::short_comment` / `long_comment` are the chainable
convenience wrappers — `long_comment` returns
`Err(PictError::InvalidData)` when the data slice overflows the on-
disk u16 `size` field (the §A-3 record caps the payload at 65535
bytes; longer annotations must split across multiple opcodes). The
builder's word-alignment pass handles odd-length data blocks
automatically (next opcode picks up a pad byte if needed).

The drawing-state machine itself ignores the comment payload — the
records exist purely as a passive annotation channel for PostScript
fragments, application-specific drawing hints, page breaks, and font
/ line-style overrides. PICT consumers that need to interpret a
specific `Kind` value can inspect `PictImage::comments` and dispatch
on the integer themselves; the decoder doesn't impose a parse on the
data slice.

## Structured text / pen-mode / highlight state (round 230)

Inside Macintosh: Imaging With QuickDraw §A-3 Table A-2 (v2) and
Table A-3 (v1) define a block of state-mutating opcodes that don't
paint pixels — `TxFont $0003`, `TxFace $0004`, `TxMode $0005`,
`SpExtra $0006`, `PnMode $0008`, `TxSize $000D`, `TxRatio $0010`,
`PnLocHFrac $0015`, `ChExtra $0016`, `HiliteMode $001C`,
`HiliteColor $001D`, `DefHilite $001E`, `OpColor $001F` — but
nevertheless carry parameters that downstream consumers (and round-
trip encoders) need to recover. Round 230 promotes each of these
opcodes from "skip the payload" to a structured capture into
[`PictTextState`], surfaced on `PictImage::text_state` and
`PictProbe::text_state`.

```rust
use oxideav_pict::ops::{PictBuilder, Verb};
use oxideav_pict::{parse_pict, PictTextState};

let mut b = PictBuilder::new(0, 0, 4, 4);
b.tx_font(0x4242)
    .tx_face(0x05)
    .tx_size(24)
    .pn_mode(10)
    .op_color(0x10, 0x20, 0x30)
    .hilite_color(0xFF, 0x00, 0x00);
b.fg_color(0, 0, 0).rect(Verb::Paint, 0, 0, 4, 4);
let bytes = b.finish();

let img = parse_pict(&bytes)?;
assert_eq!(img.text_state.tx_font, 0x4242);
assert_eq!(img.text_state.tx_size, 24);
assert_eq!(img.text_state.pn_mode, 10);
let oc = img.text_state.op_color.expect("op_color set");
assert_eq!((oc.r, oc.g, oc.b), (0x10, 0x20, 0x30));
# Ok::<(), Box<dyn std::error::Error>>(())
```

`HiliteMode` is a flag — emitting it sets `text_state.hilite_mode_flag`
to `true`. `DefHilite` resets `hilite_color` to `None` and sets
`hilite_default = true`; a subsequent `HiliteColor` overrides the
default flag back to `false`. Missing state opcodes leave their slot
at the §A-3 fresh-GrafPort default ([`PictTextState::fresh_graf_port`]:
`tx_size = 12`, `pn_mode = 8` patCopy, `pn_loc_h_frac = 0x4000` ≈ 0.5,
every other field zero).

The probe walker mirrors the decoder byte-for-byte:
`PictProbe::text_state` carries the same final-state snapshot, and the
new `PictProbe::text_state_op_count` field counts the number of
state-opcode occurrences observed during the walk. This lets a probe
caller distinguish "producer used the default shape and the slot
happened to have the default value" from "producer set the slot to
the default value explicitly." The rasterisation path is unchanged —
these opcodes do not paint pixels — but consumers no longer need to
re-walk the byte stream to recover the producer's declared text shape
or arithmetic-transfer-mode op-colour.

`OpColor` supplies the colour parameter for the §A-3 arithmetic
transfer modes (`blend`, `addPin`, `addOver`, `subPin`, `addMax`,
`subOver`, `addMin`); round 230 captures the declared colour but does
not yet honour the arithmetic transfer modes on the canvas — that is a
follow-up round on top of `state.text_state.pn_mode` /
`state.text_state.tx_mode` dispatch.

## What's not yet in

* **Non-8×8 PixPat tiles.** Inside Macintosh §A-3 nominally permits
  arbitrary `bounds` in the PixMap; round 91 honours 8×8 only and
  falls back to the monochrome `Pat1Data` for other tile sizes.
* **Text glyphs.** `LongText` / `DH/DV/DHDVText` are walked past but
  not rasterised — a TrueType engine is a separate round.
* **Arithmetic transfer modes on the canvas.** Round 230 captures
  `TxMode` / `PnMode` / `OpColor` into `text_state`, but the
  arithmetic transfer modes (`blend`, `addPin`, `addOver`, `subPin`,
  `addMax`, `subOver`, `addMin`) are not yet honoured on the
  rasteriser — every draw still uses the `srcCopy` default.
* **CompressedQuickTime decode.** The opcode is parsed (length-prefixed
  payload skipped cleanly so the surrounding decode keeps going), but
  the embedded image (typically JPEG) is not decoded — that needs
  `oxideav-mjpeg`'s `decode_jpeg` exposed publicly.
* **Multi-image PICTs.** Each subsequent raster blits onto the same
  canvas — no separate per-image surfaces.

## License

[MIT](LICENSE) — Copyright (c) 2026 Karpelès Lab Inc.
