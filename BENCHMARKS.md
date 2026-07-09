# oxideav-pict benchmarks

`cargo bench` (criterion; `benches/pict_bench.rs`). Reference numbers
from an Apple-silicon macOS host (aarch64, release profile), round 401.

## Raster path — 256 × 256 RGBA DirectBitsRect

| PackType | encode | decode |
| -------- | ------ | ------ |
| 1 (raw) | ~96 µs | ~99 µs |
| 2 (packed 24-bit) | ~74 µs | ~97 µs |
| 3 (16-bit RLE) | ~100 µs | ~145 µs |
| 4 (component PackBits) | ~126 µs | ~112 µs |

≈ 0.45–0.9 Gpixel/s either direction; the 16-bit RLE decode pays for
the per-row u16-unit PackBits expansion plus the A1R5G5B5 → RGBA8
widening.

## Vector path

| Stream | time |
| ------ | ---- |
| 400-opcode drawing stream (shapes / patterns / text / regions onto a 320 × 240 canvas), full rasterise | ~158 µs |

## Probe (no rasterisation)

| Stream | time |
| ------ | ---- |
| the 400-opcode drawing stream | ~1.08 µs |
| a 256 × 256 Rle16 raster stream | ~1.01 µs |

`probe_pict` stays ~150× cheaper than `parse_pict` on the drawing
stream (and ~140× on the raster stream) — the "introspect before you
pay decode cost" contract the probe API documents.
