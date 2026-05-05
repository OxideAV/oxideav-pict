# oxideav-pict

Pure-Rust PICT (Apple QuickDraw picture) reader for the
[`oxideav`](https://github.com/OxideAV/oxideav) framework.

Clean-room implementation of the public **Inside Macintosh: Imaging
With QuickDraw** (Apple, 1994). No Apple QuickDraw source, no
`image` crate's PICT submodule (if any), no Bitmap.framework, no GIMP
PICT plugin, no libavif PICT path, no Wine PICT-conversion code, no
NetPBM `picttoppm` source consulted.

## Decode

PICT is opcode-based: the file is a stream of QuickDraw drawing
commands. Round 1 walks the v2 (16-bit, word-aligned) opcode stream,
correctly skips every drawing-state / drawing-shape / comment opcode
by its published operand size, and extracts the first raster bitmap
it finds.

| Opcode   | Name                | Round-1 behaviour       |
| -------- | ------------------- | ----------------------- |
| `0x0000` | NOP                 | skip                    |
| `0x0001` | ClipRgn             | size-prefixed skip      |
| `0x0002`-`0x0010`, `0x0015`, `0x0016`, `0x001A`-`0x001F` | pen / colour / pattern / text state | fixed-size skip |
| `0x0020`-`0x0023` | Line / LineFrom / ShortLine[From] | fixed-size skip |
| `0x0028`-`0x002B` | Long/DH/DV/DHDV Text  | length-prefixed skip   |
| `0x002C`-`0x002E` | FontName / LineJustify / GlyphState | size-prefixed skip |
| `0x0030`-`0x006C` | Frame / Paint / Erase / Invert / Fill of Rect / RoundRect / Oval / Arc | fixed-size skip |
| `0x0070`-`0x0074` | poly verbs          | size-prefixed skip      |
| `0x0080`-`0x0084` | region verbs        | size-prefixed skip      |
| `0x0098` | **PackBitsRect**    | **decode -> RGBA** (1-bpp BitMap, PackBits-RLE rows) |
| `0x009A` | **DirectBitsRect**  | **decode -> RGBA** (16-bit A1R5G5B5 / 32-bit XRGB / ARGB, packType=1) |
| `0x00A0` | ShortComment        | fixed-size skip         |
| `0x00A1` | LongComment         | length-prefixed skip    |
| `0x00FF` | OpEndPic            | terminate               |

The PICT version stanza (`0x0011 0x02FF` for v2, `0x1101` for v1) is
recognised. The 24-byte `headerOp` (`0x0C00`) payload that follows
the v2 sentinel is consumed but otherwise ignored. The optional
512-byte launch-stub prefix (Apple's pre-OS-X file-manager habit) is
auto-detected by sniffing for a plausible picture record at offset
512.

PackBits (`n` byte: `0..=127` = literal `n+1` bytes; `129..=255` =
repeat next byte `257-n` times; `128` = NOP) is implemented per
Inside Macintosh §A-5; see [`packbits`](src/packbits.rs).

```rust
use oxideav_pict::{parse_pict, PictPixelFormat};

let pict_bytes: Vec<u8> = std::fs::read("photo.pct")?;
let img = parse_pict(&pict_bytes)?;
assert_eq!(img.pixel_format, PictPixelFormat::Rgba);
assert_eq!(img.data.len(), img.width as usize * img.height as usize * 4);
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Standalone vs registry-integrated

The crate's default `registry` Cargo feature pulls in `oxideav-core`
and exposes the framework `Decoder` trait surface plus a
`registry::register` entry point. Disable the feature
(`default-features = false`) for an `oxideav-core`-free build that
still exposes the standalone `parse_pict` API plus crate-local
`PictImage` / `PictPixelFormat` / `PictError` types.

```toml
[dependencies]
# Framework integration (default).
oxideav-pict = "0.0"

# Image-library use — no oxideav-core dep.
oxideav-pict = { version = "0.0", default-features = false }
```

## What's not in round 1

* **Drawing-command extraction.** Lines, polygons, regions, text glyphs
  are recognised + skipped, not rasterised. PICTs that contain only
  drawing commands (no `PackBitsRect` / `DirectBitsRect`) decode as
  `PictError::NoRaster` — round 2 will add a software rasteriser.
* **DirectBitsRect packType 2 / 3 / 4.** Only the uncompressed
  packType=1 form decodes; component-separated and packed 16-bit RLE
  planes are deferred.
* **Region-clipped raster.** `PackBitsRgn` (`0x0099`) and
  `DirectBitsRgn` (`0x009B`) return `Unsupported`.
* **CompressedQuickTime** (`0x8200`). Embedded JPEG / Animation / RLE
  QuickTime ImageDescription decode is round-2.
* **PICT v1 raster.** v1 (8-bit opcodes) is *detected* (sentinel
  `0x1101`), but its raster opcodes return `Unsupported`.
* **Multi-image PICTs.** Only the first extractable raster is
  surfaced; PICTs with several embedded images need a different API
  shape.
* **PICT writing.** Many opcodes to emit + old-Mac-only consumer
  base — round 2 at the earliest.

## License

[MIT](LICENSE) — Copyright (c) 2026 Karpelès Lab Inc.
