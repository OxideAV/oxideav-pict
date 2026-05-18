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
| `0x0002`-`0x0010`, `0x0015`, `0x0016`, `0x001A`-`0x001F` | pen / colour / pattern / text state | rasteriser tracks fg/bg colour, pen size, oval-corner size, origin |
| `0x0020`-`0x0023` | Line / LineFrom / ShortLine[From] | **draw via Bresenham** |
| `0x0028`-`0x002B` | Long/DH/DV/DHDV Text  | length-prefixed skip (no font rasteriser) |
| `0x002C`-`0x002E` | FontName / LineJustify / GlyphState | size-prefixed skip |
| `0x0030`-`0x006C` | Frame / Paint / Erase / Invert / Fill of Rect / RoundRect / Oval / Arc | **rasterise via in-crate kernel** |
| `0x0070`-`0x0074` | Frame / Paint / Erase / Invert / Fill Poly | **rasterise via even-odd scanline** |
| `0x0080`-`0x0084` | Frame / Paint / Erase / Invert / Fill Rgn | **rasterise (rect bbox + per-row inversion mask)** |
| `0x0090` | **BitsRect**        | **decode -> RGBA** (1-bpp BitMap, raw rows) |
| `0x0091` | **BitsRgn**         | **decode -> RGBA** (BitsRect + clip region) |
| `0x0098` | **PackBitsRect**    | **decode -> RGBA** (1-bpp BitMap, PackBits-RLE rows) |
| `0x0099` | **PackBitsRgn**     | **decode -> RGBA** (PackBitsRect + clip region) |
| `0x009A` | **DirectBitsRect**  | **decode -> RGBA** (16-bit A1R5G5B5 / 32-bit XRGB|ARGB; packType 0/1 raw, 2 packed 24bpp, 3 u16-PackBits, 4 component-separated PackBits) |
| `0x009B` | **DirectBitsRgn**   | **decode -> RGBA** (DirectBitsRect + clip region) |
| `0x00A0` | ShortComment        | fixed-size skip         |
| `0x00A1` | LongComment         | length-prefixed skip    |
| `0x8200` | CompressedQuickTime | length-prefixed skip (embedded JPEG/RLE/Animation decode is a future round) |
| `0x8201` | UncompressedQuickTime | length-prefixed skip   |
| `0x00FF` | OpEndPic            | terminate               |

The PICT version stanza (`0x0011 0x02FF` for v2, `0x1101` for v1) is
recognised. The 24-byte `headerOp` (`0x0C00`) payload that follows
the v2 sentinel is consumed but otherwise ignored. The optional
512-byte launch-stub prefix (Apple's pre-OS-X file-manager habit) is
auto-detected by sniffing for a plausible picture record at offset
512.

PackBits (`n` byte: `0..=127` = literal `n+1` bytes; `129..=255` =
repeat next byte `257-n` times; `128` = NOP) is implemented per
Inside Macintosh §A-5; see [`packbits`](src/packbits.rs). The
DirectBitsRect packType-3 variant uses the same RLE algorithm at u16
unit size; packType 4 is byte-PackBits per channel plane.

PICT v1 (8-bit opcodes) parses the same drawing-state machine plus a
smaller raster opcode set (`BitsRect 0x90`, `BitsRgn 0x91`,
`PackBitsRect 0x98`, `PackBitsRgn 0x99`).

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
| `build_direct_bits_rect_op` | DirectBitsRect opcode bytes | round 5 — public helper for the raw `0x009A` opcode bytes (no stub / header / OpEndPic) |

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

## What's not yet in

* **Pattern fills (`PnPat`, `BkPixPat`, `PnPixPat`, `FillPixPat`).**
  Solid-colour ink only — patterns return `Unsupported`.
* **Text glyphs.** `LongText` / `DH/DV/DHDVText` are walked past but
  not rasterised — a TrueType engine is a separate round.
* **CompressedQuickTime decode.** The opcode is parsed (length-prefixed
  payload skipped cleanly so the surrounding decode keeps going), but
  the embedded image (typically JPEG) is not decoded — that needs
  `oxideav-mjpeg`'s `decode_jpeg` exposed publicly.
* **Multi-image PICTs.** Each subsequent raster blits onto the same
  canvas — no separate per-image surfaces.
* **8-bit-indexed PixMaps.** `DirectBitsRect` (`0x009A`) only; the
  indexed-colour `PackBitsRect`-as-PixMap path with a colour table
  is a future round.

## License

[MIT](LICENSE) — Copyright (c) 2026 Karpelès Lab Inc.
