# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- round 361: **`TxRatio` glyph scaling + `lineJustify` intercharacter
  spacing reach the text rasteriser.** Both opcodes were already captured
  into the drawing state but discarded at draw time. `font::draw_text` /
  `measure_text` now take a `TextScale` (the new public struct bundling
  `txSize` with the `TxRatio` `$0010` horizontal `numer.h/denom.h` and
  vertical `numer.v/denom.v` factors — Imaging With QuickDraw book page
  12-13) plus an `inter_char` advance, so a wide / condensed run stretches
  or squeezes the glyph cells along each axis with the baseline anchored
  on the pen, and the `lineJustify` (`$002D`) intercharacter spacing
  (§A-3 footnote `†`) is added to every glyph's advance (distinct from the
  nonspace-only `chExtra` and space-only `spExtra`). Ratio scaling rounds
  in `i64` to nearest with a 1-px floor and clamps zero denominators.
  Four new `parse_pict`-level tests in `synth_text_layout_round361`.

- round 352: **text rasterisation.** The QuickDraw text-drawing opcodes
  `LongText` (`$0028` / v1 `$28`), `DHText` (`$0029` / `$29`), `DVText`
  (`$002A` / `$2A`) and `DHDVText` (`$002B` / `$2B`) now draw glyphs onto
  the canvas instead of being walked past. A new public `font` module
  carries an original crate-authored 5×7 ASCII bitmap face plus
  `draw_text` / `measure_text`; the decoder places the string's baseline
  at the text pen (Imaging With QuickDraw book page 2-13), scales by
  `txSize` (book page 2-34: `point × resolution / 72`), inks in `fgColor`,
  honours the `srcOr` / `srcXor` / `srcBic` text source modes (book page
  2-34) and advances the running pen by each glyph plus `chExtra` /
  `spExtra`. PICT embeds no font data and Imaging With QuickDraw defers
  the system-font bitmaps + `txFace` style synthesis to Inside Macintosh:
  Text (not in the crate's reference set), so text is legible and
  spec-positioned but not pixel-identical to a particular Mac font.
  round 295's pen-tracking tests were rewritten to expect the
  glyph-advanced pen position (`declared + measure_text(text)`); new
  `synth_text_raster_round352` asserts ink placement, `txSize` scaling,
  `fgColor` inking, multi-opcode lines and empty-string no-op.

### Other

- round 333: `FramePoly` (`$0070` / v1 `$70`) honours the current pen
  size (`PnSize`) and pen pattern / pattern mode (`PnPat` / `PnMode`).
  Inside Macintosh: Imaging With QuickDraw, "QuickDraw Drawing
  Reference" (book page 3-81): the outline is drawn *"using the current
  graphics port's pen pattern, pattern mode, and size"* and *"the
  graphics pen hangs below and to the right of each point on the
  boundary"*. Previously the polygon frame verb drew a fixed 1-pixel
  Bresenham outline at the foreground colour, ignoring `PnSize` /
  `PnPat` — every other frame verb already honoured the pen. New public
  raster primitives `frame_polygon_thick` /
  `frame_polygon_pattern_thick_mode` / `frame_polygon_pix_pattern_thick`.
- round 328: `DirectBits` `packType = 0` resolves to the §A-3 page A-16
  documented default packing — type 3 (16-bit PackBits) for a 16-bit
  PixMap, type 4 (component PackBits) for a 32-bit PixMap — when
  `rowBytes ≥ 8`; `rowBytes < 8` keeps data unpacked. Previously
  `packType 0` was mis-decoded as raw pixel rows.
- round 322: honour `srcRect` on every raster blit (`BitsRect` /
  `BitsRgn` / `PackBitsRect` / `PackBitsRgn` / `DirectBitsRect` /
  `DirectBitsRgn`) — the §A-3 Listing A-2 / A-3 `srcRect` sub-rectangle
  of the source PixMap `bounds` is cropped before the scaling
  `CopyBits` blit onto `dstRect`. `srcRect == bounds` stays an identity
  no-op.

## [0.0.4](https://github.com/OxideAV/oxideav-pict/compare/v0.0.3...v0.0.4) - 2026-06-15

### Other

- r308 — structured TxMode resolution via PictTextState::tx_source_mode
- arbitrary power-of-2 PixPat tiles (round 302)
- round 295: track QuickDraw text-drawing pen through LongText / DH/DV/DHDVText
- round 290: hilite = 50 highlighting transfer mode honoured
- round 282: CopyBits transfer modes honoured on the raster blit
- honour Color QuickDraw arithmetic transfer modes on patterned fills
- round 266: typed PictTextFace newtype for the TxFace Style byte
- round 252: Invert* verbs honoured on round-rect / oval / arc / poly
- drop release-plz.toml — use release-plz defaults across the workspace
- round 247: PnMode Boolean transfer modes honoured on the rasteriser
- scrub pre-existing decorative attribution (parent r236)
- round 236: structured fontName / lineJustify / glyphState capture
- round 230: structured text / pen-mode / highlight state opcodes
- round 224: structured Picture Comments ($00A0 ShortComment / $00A1 LongComment)
- round 217: structured v2 HeaderOp (0x0C00) 24-byte payload parser + spec-correct encoder header
- round 211: indexed-PixMap encoder — BitsRect / PackBitsRect / *Rgn variants
- round 205: v1 dispatcher state-machine + text + Same-shape opcode coverage
- round 199: §A-3 reserved-for-Apple-use v2 opcode skip table

### Added

- Round 302: **Arbitrary power-of-2 PixPat tiles.** Inside Macintosh §3
  ("QuickDraw Drawing Reference", book page 3-40): *"A pixel pattern …
  can use additional colors and can be of any width and height that's a
  power of 2."* Round 91 honoured only the universal 8×8 colour-pixmap
  tile; round 302 wires up the full power-of-2 `bounds`. [`PixPattern`]
  now carries `width` / `height` + a row-major `Vec<Rgba>`, and
  [`PixPattern::sample`] wraps modulo the actual tile dimensions, so a
  4×2 / 2×4 / 16×16 (etc.) colour pattern tiles correctly across the
  canvas through every existing `fill_*_pix_pattern` rasteriser (paint /
  fill / erase of rect / round-rect / oval / poly / region). The
  `decode_pix_pat` reader builds the tile from the PixMap `bounds`
  (`rowBytes < 8` flat rows, `rowBytes ≥ 8` per-row byteCount +
  PackBits, per §A-3 "PixData"). A degenerate or non-power-of-2 tile
  still falls back to the `Pat1Data` monochrome interpretation. New
  encoder helpers `build_pix_pat_op_sized` and
  `PictBuilder::pen_pix_pattern_sized` emit arbitrary power-of-2
  colour-pixmap `PixPat` opcodes; `build_pix_pat_op` / `pen_pix_pattern`
  remain the 8×8 special case. `PixPattern::new(width, height, pixels,
  fallback)` constructs a tile with the cell-count invariant enforced.
  Six new round-trip tests (`tests/synth_v2_round302.rs`) cover 4×2,
  2×4, 16×16, 1×1, the probe walk, and encoder validation.

  *API change:* [`PixPattern`]'s `pixels` field changes from a fixed
  `[Rgba; 64]` to a `Vec<Rgba>`, and the struct gains `width: u16` /
  `height: u16` fields. Construct via `PixPattern::new(width, height,
  pixels, fallback)` rather than the struct literal.
- Round 295: **QuickDraw text-drawing pen-location tracking.** The
  text-glyph opcodes `LongText` (`$0028`), `DHText` (`$0029`),
  `DVText` (`$002A`) and `DHDVText` (`$002B`) — previously walked past
  without effect — now drive the running text pen recorded on
  [`PictTextState::text_pen`] (an `Option<(h, v)>` in picture-frame
  coordinates) plus a [`PictTextState::text_op_count`] counter. Inside
  Macintosh: Imaging With QuickDraw, "About Basic QuickDraw" (book page
  2-13): *"Text is drawn with the base line positioned at the pen
  location."* `LongText` carries an absolute `txLoc` Point that sets
  the pen; the compact `DH/DV/DHDV` variants carry positive `(0..255)`
  deltas (Appendix A, Table A-2 / v1 Table A-3) that advance the pen
  relative to the position the previous text opcode left — the reason
  the compact forms exist (successive `DrawText` calls on one line
  record only the increment). A delta with no prior `LongText`
  advances from the graphics origin `(0, 0)`. The slot is `None` until
  the first text opcode and is surfaced on `PictImage::text_state` for
  both the v1 and v2 dispatchers. Glyph bytes are still walked past
  without rendering (no font rasteriser) and the per-character pen
  advance the glyph widths would add is not modelled — only the
  explicit stream-encoded text origin / inter-call movement, which is
  fully spec-determined. 10 new synth tests (`synth_v2_round295`).

- Round 290: **`hilite = 50` highlighting transfer mode honoured.**
  Inside Macintosh: Imaging With QuickDraw §4 ("Color QuickDraw"),
  "Highlighting" (book pages 4-41..4-43), defines the `hilite = 50`
  transfer-mode constant (*"add to source or pattern mode for
  highlighting"*). Rounds 247 / 273 / 282 folded it to `patCopy` /
  `srcCopy`. Round 290 resolves it on both the `PnMode` pattern-fill
  path and the `CopyBits` raster-blit `mode` word into new
  [`PatternMode::Hilite`] / [`SourceMode::Hilite`] variants. Per §4
  (*"replaces the background color with the highlight color … only bits
  that are on in the pattern or source image can be highlighted"*),
  every on-bit cell exchanges the destination's background colour for
  the highlight colour and vice versa (every other colour unchanged) —
  a reversible exchange matching the §4-40 Table 4-2 1-bit revert
  (`hilite → srcXor`). The highlight colour is the `HiliteColor` opcode
  (`$001D`); when absent the §4-40 revert folds the mode to `patXor` /
  `srcXor`. `from_pn_mode_with` / `from_mode_word` gain a
  `hilite_color` parameter; the public `HILITE_MODE` constant (= 50) is
  exported. +12 tests (`tests/synth_v2_round290.rs`).
- Round 282: **`CopyBits` transfer modes honoured on the raster blit.**
  Every PICT raster opcode record (`BitsRect 0x0090` / `BitsRgn 0x0091`
  / `PackBitsRect 0x0098` / `PackBitsRgn 0x0099` / `DirectBitsRect
  0x009A` / `DirectBitsRgn 0x009B`) carries a `mode` (transfer mode)
  word between `dstRect` and the pixel data per Inside Macintosh:
  Imaging With QuickDraw §A-3 Listings A-2 / A-3. Rounds 1..273 parsed
  and discarded it — every blit rendered `srcCopy` against a black-fg /
  white-bg port. Round 282 resolves the word into the new public
  [`SourceMode`] enum and honours it per pixel through the new
  [`blend_source`] combiner + `Canvas::blit_mode`:
  - the eight §3 Boolean source modes (`srcCopy = 0` … `notSrcBic = 7`,
    book pages 3-113..3-114) with the §4 Table 4-1 (page 4-33) colour
    semantics — per channel, the source's closeness to black applies
    that portion of the foreground colour (background for the BIC ops),
    white applies the mode's "leave" colour, *"any other color"*
    applies weighted portions per the §4-33 `CopyBits` description;
    XOR is a whole-pixel decision (only exactly-black / exactly-white
    source pixels invert the destination, channel-wise NOT);
  - the eight §4 arithmetic transfer modes (`blend = 32` … `adMin =
    39`) — legal in the `CopyBits` mode parameter per the §4-40 Note —
    reusing the round-273 `blend_arith` combiner with the decoded
    raster pixel as the source, the declared `OpColor` (per-§4-39/4-40
    defaults when absent) and the background colour as the
    transparent-mode key;
  - the additive `ditherCopy = 64` bit (§3-114), recognised and
    stripped (dithering approximates colours on indexed destinations;
    the canvas here is true-colour RGBA). Unknown codes (including the
    pattern band `8..=15`) fall back to `srcCopy`, mirroring the
    round-247 total-function posture; the additive `hilite = 50` and
    `grayishTextOr = 49` remain unresolved pending glyph rasterisation.

  `srcCopy` under the fresh-GrafPort black-fg / white-bg state is the
  §4-34 identity (*"always reproduces the source image, regardless of
  the pixel depth"*) and short-circuits to the raw-copy fast path, so
  pre-r282 streams decode bit-for-bit unchanged; a non-default port
  now colorizes `srcCopy` blits per §4-33 (the Listing 4-5 coloration
  effect) including 1-bpp BitMap sources (black bits → foreground).
  Encoder side: `build_direct_bits_rect_op_with_mode` /
  `PictBuilder::raster_with_mode` emit a `DirectBitsRect` with an
  explicit mode word (`build_direct_bits_rect_op` keeps emitting
  `mode = 0`, byte-for-byte unchanged). 25 new tests in
  `tests/synth_v2_round282.rs`: encoder byte layout (mode word at
  record offset 68), `SourceMode::from_mode_word` band mapping +
  dither-strip + `OpColor` defaults + fallback, hand-pinned
  `blend_source` weighted formulas, full decode round-trips for each
  Boolean mode + `addOver` / `addPin` / `transparent` on the blit, the
  identity fast path, and 1-bpp `BitsRect` port-colorization
  (`srcCopy` + `srcOr`). One pre-existing test
  (`synth_v2::opcode_stream_with_state_then_raster`) spliced a
  non-black `RGBFgCol` before its raster and asserted a raw copy — its
  expectation now follows the §4-33 colorization it always requested.

- Round 273: **Color QuickDraw arithmetic transfer modes honoured on
  patterned shape fills.** Inside Macintosh: Imaging With QuickDraw §4
  ("Color QuickDraw") pages 4-38..4-40 define eight arithmetic transfer
  modes (`blend = 32`, `addPin = 33`, `addOver = 34`, `subPin = 35`,
  `transparent = 36`, `addMax = 37`, `subOver = 38`, `adMin = 39`).
  Round 230 captured the `PnMode` code + `OpColor` into `PictTextState`;
  round 247 honoured the Boolean pattern modes (`8..=15`). Round 273
  wires the arithmetic modes into every patterned `frame` / `paint` /
  `erase` / `fill` of rect / round-rect / oval / poly / region via the
  shared per-cell dispatch path established in round 247. New public
  [`ArithMode`] enum (the eight §4 modes) + `ArithMode::from_code`, a
  pure [`blend_arith`] combiner implementing the §4 per-channel
  formulas at 8-bit ("truncated RGB" direct-pixel) precision, and a new
  [`PatternMode::Arith`] variant carrying the active `OpColor` pin /
  blend weight + the transparent-mode background key. The new
  `PatternMode::from_pn_mode_with(code, op_color, bg_key)` constructor
  resolves the arithmetic modes when colour context is available;
  absent `OpColor` defaults per §4-39/4-40 (max-pin → white, min-pin →
  black, blend → 50% gray). The bare const `PatternMode::from_pn_mode`
  still folds the arithmetic codes to `patCopy` for callers without
  colour context, so producers that never emit `PnMode` render
  bit-for-bit identically to pre-r273. 15 new synth tests covering each
  mode + the pin / weight defaults + the cross-shape dispatch +
  `blend_arith` directly. New `ArithMode` / `blend_arith` / `Rgba` lib
  re-exports.

- Round 266: **Typed `PictTextFace` newtype for the `TxFace` style
  byte.** Inside Macintosh: Imaging With QuickDraw §A-3 Table A-2 row
  `$0004` / Table A-3 row `$04` describes the `TxFace` payload as
  *"Text's font style (0..255)"* — a classic-Mac `Style` bitfield. Round
  266 promotes the storage on `PictTextState::tx_face` from raw `u8` to
  a typed [`PictTextFace`] newtype with named-bit predicates
  ([`PictTextFace::bold`] / `italic` / `underline` / `outline` /
  `shadow` / `condense` / `extend`), bit-mask constants
  ([`PictTextFace::BOLD`] etc), an [`PictTextFace::is_plain`] predicate,
  and a [`PictTextFace::PLAIN`] default. `From<u8>` + `Into<u8>` round-
  trip the on-disk byte verbatim, and a `PartialEq<u8>` impl lets
  pre-r266 call sites that compared `tx_face` to a raw byte keep
  working unchanged. The encoder side (`build_tx_face` /
  `PictBuilder::tx_face`) keeps taking a raw `u8` so existing producers
  do not need to migrate. Reserved bit 7 (`0x80`) is preserved verbatim
  on the raw byte but masked off by `is_plain` per the §A-3 caption
  (which only names bits 0..=6). Eight new state-mod tests + new
  `PictTextFace` lib export.

- Round 252: **`Invert*` verbs honoured on round-rect / oval / arc /
  polygon shapes.** Inside Macintosh: Imaging With QuickDraw §3
  ("QuickDraw Drawing Reference") and §A-3 Table A-2 specify five
  invert verbs — `InvertRect $0033`, `InvertRRect $0043`, `InvertOval
  $0053`, `InvertArc $0063`, `InvertPoly $0073` — each *"inverts the
  destination pixel"* across the shape's interior. The round-2
  dispatcher routed only `InvertRect` through a true pixel invert
  (`invert_rect`); the other four collapsed onto the *frame* helper as
  a documented placeholder. Round 252 wires the missing four through
  new spec-correct raster helpers (`invert_oval`, `invert_round_rect`,
  `invert_arc`, `invert_polygon`) that share the same per-row coverage
  shape as their `fill_*` siblings so the §3 self-inverse contract
  holds (invert twice on the same geometry restores the canvas pixel-
  for-pixel). The Same-shape companion opcodes (`$004B`, `$005B`,
  `$006B`) and the v1 byte-opcode variants (`$43`, `$53`, `$63`) pick
  up the new behaviour through the existing shared `apply_*_verb`
  dispatchers. New `Canvas::invert_span` helper carries the
  channel-wise-NOT primitive that the four shape helpers iterate over.
- Round 247: **`PnMode` Boolean pattern transfer modes honoured on the
  rasteriser.** Inside Macintosh: Imaging With QuickDraw §3
  ("QuickDraw Drawing Reference") `PenMode` procedure (book page 3-44)
  defines eight Boolean pattern modes (`patCopy = 8` … `notPatBic = 15`)
  consumed by every pattern-fill verb. Round 230 captured the
  `PnMode $0008` payload into `PictTextState::pn_mode` but the
  rasteriser still wrote every cell as if the mode were `patCopy`.
  Round 247 routes `state.text_state.pn_mode` through a new
  `PatternMode` enum into every patterned `frame` / `paint` / `erase` /
  `fill` of `rect` / `round-rect` / `oval` / `poly` / `region` verb,
  so the §3-44 per-cell Boolean op is honoured at draw time. The
  default `pn_mode = 8` (`patCopy`) collapses to the round-8 solid-fg
  / solid-bg fast paths bit-for-bit; non-default modes go through the
  per-cell read-modify-write path with `Canvas::pixel_at` for the
  `patXor` / `notPatXor` destination-invert cases. Codes outside
  `8..=15` (including the §3-44 source modes `0..=7` and the §4-38
  arithmetic transfer modes `32..=49`) fall back to `patCopy`. New
  exports: `PatternMode`, `PatternMode::from_pn_mode`,
  `PatternMode::is_pat_copy`, plus mode-aware raster primitives
  (`fill_rect_pattern_mode`, `fill_oval_pattern_mode`,
  `fill_round_rect_pattern_mode`, `fill_polygon_pattern_mode`,
  `frame_rect_pattern_thick_mode`, `plot_region_cell_mode`) alongside
  the original `*_pattern` shapes. Sixteen new `synth_v2_round247`
  tests cover every mode + verb (rect / oval / poly / region) + the
  `fresh_graf_port` default.

- Round 236: **Structured `fontName` / `lineJustify` / `glyphState`
  opcode capture** — Inside Macintosh: Imaging With QuickDraw §A-3
  Table A-2 lists three v2-only state-mutating opcodes whose payloads
  carry Script-Manager and font-engine round-trip parameters but had
  previously been walked past with no further structure:
  - `fontName` `$002C` — footnote `*`: `dataLength (Integer)` (inclusive
    of itself), `oldFontID (Integer)`, `nameLength (0..255)`, `name
    (nameLength bytes)`. Declares the font identity that subsequent
    text-glyph opcodes belong to.
  - `lineJustify` `$002D` — footnote `†`: `dataLength = 8` (exclusive
    of itself), two `Fixed` 16.16 values — intercharacter spacing and
    total extra spread across the style run. Matches the appendix's
    worked example `2D 00 08 00 01 00 00 00 0A 00 00`.
  - `glyphState` `$002E` — `dataLength`-prefixed block carrying four
    1-byte Booleans (`outline preferred`, `preserve glyph`, `fractional
    widths`, `scaling disabled`); the encoder writes `dataLength = 6`
    and two trailing zero pad bytes so the §A-3 8-byte
    "Additional data size" column is honoured.

  Round 236 captures each payload into the new `PictTextState`
  fields `font_name: Option<PictFontName>`, `line_justify:
  Option<PictLineJustify>`, `glyph_state: Option<PictGlyphState>`,
  surfaced on `PictImage::text_state` / `PictProbe::text_state`.
  `PictProbe::text_state_op_count` bumps once per occurrence,
  mirroring round 230's accounting. The rasterisation path is
  unchanged — these are passive state opcodes — but consumers can now
  recover the producer's declared font name + Script-Manager line-
  layout state + glyph-renderer preferences without re-walking the
  byte stream. `PictTextState` lost its `Copy` impl (the heap-
  allocated `name: Vec<u8>` on `PictFontName` is the reason) but kept
  `Clone + Default + PartialEq + Eq`.
- Round 236: Encoder helpers `build_font_name`, `build_line_justify`,
  `build_glyph_state` emit the §A-3 on-disk record bytes (opcode word
  + length-prefixed payload). Chainable `PictBuilder::font_name` /
  `line_justify` / `glyph_state` wrappers route through the byte
  builders; `font_name` returns `Err(PictError::InvalidData)` when
  the name length overflows the on-disk u8 nameLength field (cap = 255
  bytes). The decoder + probe also reject `fontName` records whose
  `dataLength` falls below the 5-byte minimum (length + oldFontID +
  nameLen) and `lineJustify` records whose `dataLength` is below the
  8-byte Fixed-pair payload.
- Round 236: 16 synthesis tests in `tests/synth_v2_round236.rs`:
  fresh-GrafPort defaults (the three new slots start at `None`);
  byte-layout assertions for every encoder helper including the
  spec's worked `lineJustify` example; `PictBuilder` round-trip
  through `parse_pict` for each of the three opcodes (font name +
  line-justify Fixed values + glyph-state Booleans); "last opcode
  wins" semantics when the producer emits multiple of the same
  opcode; probe parity vs decoder; `text_state_op_count` accuracy
  (3 for a stream with all three opcodes; 0 when none emitted);
  invalid-stream rejection (`fontName` with `dataLength < 5`). Every
  byte sequence is traceable back to §A-3 Table A-2 footnotes `*` and
  `†` and row `$002E`.

- Round 230: **Structured text / pen-mode / highlight state opcodes** —
  Inside Macintosh: Imaging With QuickDraw §A-3 Table A-2 (v2) and
  Table A-3 (v1) list a block of state-mutating opcodes whose payloads
  were previously stepped past with no further accounting:
  - `TxFont` `$0003` v2 / `$03` v1 — 2-byte `Integer` font number.
  - `TxFace` `$0004` v2 / `$04` v1 — 1-byte `Style` flag-byte.
  - `TxMode` `$0005` v2 / `$05` v1 — 2-byte `Integer` source-mode code.
  - `SpExtra` `$0006` v2 / `$06` v1 — 4-byte `Fixed` extra-space value.
  - `PnMode` `$0008` v2 / `$08` v1 — 2-byte `Integer` pen-mode code.
  - `TxSize` `$000D` v2 / `$0D` v1 — 2-byte `Integer` text size in points.
  - `TxRatio` `$0010` v2 / `$10` v1 — 8-byte numerator + denominator
    Point pair (each `Point` is `(v: i16, h: i16)` on disk).
  - `PnLocHFrac` `$0015` (v2 only) — 2-byte fractional pen position.
  - `ChExtra` `$0016` (v2 only) — 2-byte per-character extra-width.
  - `HiliteMode` `$001C` (v2 only) — 0-byte "use highlight mode" flag.
  - `HiliteColor` `$001D` (v2 only) — 6-byte `RGBColor`.
  - `DefHilite` `$001E` (v2 only) — 0-byte reset to system-default
    highlight colour.
  - `OpColor` `$001F` (v2 only) — 6-byte `RGBColor` consumed by the
    arithmetic transfer modes (`blend`, `addPin`, `addOver`, `subPin`,
    `addMax`, `subOver`, `addMin`).

  Round 230 captures every payload into the new `PictTextState`
  struct, surfaced on `PictImage::text_state` and
  `PictProbe::text_state`. A new `PictProbe::text_state_op_count`
  field counts the number of state-opcode occurrences. The
  rasterisation path is untouched — these opcodes don't paint
  pixels — but consumers (and round-trip encoders) can now recover
  the producer's declared text shape and arithmetic-transfer-mode
  op-colour without re-walking the byte stream. `DefHilite` resets
  `hilite_color` to `None` and sets `hilite_default = true`;
  `HiliteColor` after a `DefHilite` overrides the default flag back
  to `false`. Missing state opcodes leave the slot at its fresh-
  GrafPort default (`PictTextState::fresh_graf_port`): `tx_size = 12`,
  `pn_mode = 8` (patCopy), `pn_loc_h_frac = 0x4000` (0.5), every
  other field zero.
- Round 230: Encoder helpers `build_tx_font`, `build_tx_face`,
  `build_tx_mode`, `build_sp_extra`, `build_pn_mode`, `build_tx_size`,
  `build_tx_ratio`, `build_pn_loc_h_frac`, `build_ch_extra`,
  `build_hilite_mode`, `build_hilite_color`, `build_def_hilite`,
  `build_op_color` emit the §A-3 on-disk record bytes (opcode word +
  payload). Chainable `PictBuilder::tx_font` / `tx_face` / `tx_mode` /
  `sp_extra` / `pn_mode` / `tx_size` / `tx_ratio` / `pn_loc_h_frac` /
  `ch_extra` / `hilite_mode` / `hilite_color` / `def_hilite` /
  `op_color` wrappers route through the byte builders. The
  `HiliteColor` / `OpColor` 8-bit RGB inputs are replicated to the
  16-bit-per-channel on-disk form (`high8 = low8 = channel`) so the
  decoder's `Rgba::from_rgb16` high-byte selection round-trips
  bit-exact.
- Round 230: 29 synthesis tests in `tests/synth_v2_round230.rs`:
  byte-layout assertions for every encoder helper; `PictTextState`
  default-slot validation; the multi-opcode round-trip through
  `parse_pict` confirming each slot lands; `HiliteMode` / `HiliteColor`
  / `DefHilite` ordering semantics (set-after-default override and
  default-after-set reset); decoder ↔ probe text-state equality;
  `PictProbe::text_state_op_count` accuracy across single-opcode and
  13-opcode streams; v1 dispatcher coverage for `0x03`..`0x06`,
  `0x08`, `0x0D`, `0x10`; 16-bit RGB high-byte round-trip via
  `HiliteColor` + `OpColor`. Every byte sequence is traceable back to
  §A-3 Table A-2 / A-3.

- Round 224: **Structured Picture Comments** — `ShortComment` (`$00A0`
  v2 / `$A0` v1) and `LongComment` (`$00A1` v2 / `$A1` v1) opcodes
  are now captured as `PictComment` records on `PictImage::comments`
  and `PictProbe::comments` instead of being silently skipped past
  by the opcode walker. Inside Macintosh: Imaging With QuickDraw
  §A-3 Table A-2 (v2) and Table A-3 (v1) define the on-disk records:
  `ShortComment` carries a 2-byte `Kind (Integer)` word; `LongComment`
  adds a 2-byte `size` byte count and that many raw data bytes.
  `PictComment` owns `kind: u16`, `data: Vec<u8>` (empty for
  `ShortComment`), and an `is_long: bool` flag so consumers can
  re-emit the original opcode shape. The drawing-state machine
  remains untouched — Picture Comments are a passive annotation
  channel for PostScript fragments, application drawing hints,
  page-break markers, and font / line-style overrides; the decoder
  doesn't impose a parse on the data slice. The probe walker stays
  byte-identical to the decoder's comment path so
  `PictProbe::comments` and `PictImage::comments` carry the same
  records.
- Round 224: Encoder helpers `build_short_comment(kind)` and
  `build_long_comment(kind, data)` emit the raw v2 opcode bytes
  (`build_long_comment` returns `PictError::InvalidData` when the
  data slice overflows the on-disk u16 `size` field at 65535 bytes —
  longer annotations must split across multiple opcodes per §A-3).
  `build_short_comment_v1` and `build_long_comment_v1` emit the v1
  (1-byte-opcode) variants. `PictBuilder::short_comment` and
  `PictBuilder::long_comment` are the chainable convenience wrappers
  for v2 stream synthesis; the builder's existing word-alignment
  pass pads odd-length `LongComment` data automatically before the
  next opcode.
- Round 224: 18 synthesis tests in `tests/synth_v2_round224.rs`
  covering the `PictComment` constructors, the four encoder
  byte-layout helpers (including u16-size-field overflow rejection
  and max-u16-size acceptance), single-comment and multi-comment
  v2 round-trips through `parse_pict`, stream-order preservation
  across mixed `ShortComment` + `LongComment` runs, odd-size
  word-alignment, empty-`LongComment` round-trip, probe
  surface-equality with the decoder, hand-rolled v1 PICT comment
  decoding (`ShortComment 0xA0` + `LongComment 0xA1`), the v1 probe
  surface, and the `InvalidData` error path on a truncated
  `LongComment` payload.

- Round 217: **v2 `HeaderOp` (`0x0C00`) 24-byte payload** is now parsed
  into a structured `PictHeader` instead of being discarded with a raw
  24-byte skip (Inside Macintosh: Imaging With QuickDraw §A-3 "Version
  and Header Opcodes" + §A-22 Listings A-5 / A-6). Two variants per
  the §A-3 contract:
  - `PictHeader::ExtendedV2 { hres, vres, optimal_source_rect }` — the
    `OpenCPicture` `version=-2` shape carrying explicit 16.16
    fixed-point resolution and an optimal source rectangle.
  - `PictHeader::V2 { fixed_bounds }` — the `OpenPicture`-in-CGrafPort
    `version=-1` shape carrying a fixed-point bounding rectangle
    (top, left, bottom, right).

  Surfaced via the new `PictImage::header: Option<PictHeader>` field
  (None for v1 PICTs and non-canonical pads). The read-only probe path
  picks it up too — `PictProbe::header` reports the parsed variant
  without rasterising. The encoder side now emits a canonical
  Listing-A-5 extended-v2 header (`version=-2`, `hRes=vRes=72.0` dpi,
  `optimal_source_rect = picFrame`, reserved fields zero) from every
  v2 emit path (`encode_pict`, `encode_pict_v2`, `encode_pict_v1`'s v2
  counterparts, the BitsRect / PackBitsRect / region encoders, the
  indexed-PixMap encoders, `encode_pict_v2_with_clip`, and
  `PictBuilder::new`). A new `Fixed` newtype wraps the QuickDraw 16.16
  fixed-point type with `to_f32` / `integer_part` / `as_u32` helpers
  and a `SEVENTY_TWO_DPI` constant. Pre-r217 PICTs whose 24-byte
  payload was zeroed out keep decoding — the parser tolerates a
  non-canonical leading word by falling back to the raw 24-byte skip
  and reporting `header: None`.

- Round 211: **Indexed-PixMap encoder** — closes the round-186
  decoder's encoder side. Four new public functions emit a v2 PICT
  stream containing a single indexed-PixMap raster opcode (Inside
  Macintosh §A-3 footnote `§`: "If the high bit of rowBytes is set,
  then it is a pixel map containing multiple bits per pixel"):
  - `encode_pict_indexed_bits_rect` → `BitsRect 0x0090` (raw PixData
    rows).
  - `encode_pict_indexed_pack_bits_rect` → `PackBitsRect 0x0098`
    (per-row PackBits when `rowBytes >= 8`; raw rows otherwise per
    §A-3 narrow-row carve-out).
  - `encode_pict_indexed_bits_rgn` → `BitsRgn 0x0091` (BitsRect plus
    a rectangular clip region inserted between the rect/mode header
    and the PixData rows).
  - `encode_pict_indexed_pack_bits_rgn` → `PackBitsRgn 0x0099`
    (PackBitsRect with the same rectangular clip).

  A new public `IndexedPixelSize` enum selects between 1 / 2 / 4 / 8
  bpp; the per-row packer is MSB-first per QuickDraw convention,
  matching the round-186 decoder's `read_indexed_pixel` switch. The
  on-disk PixMap header omits the `baseAddr` placeholder (`BitsRect`-
  family opcodes drop it — only `DirectBitsRect 0x009A` /
  `DirectBitsRgn 0x009B` carry it per §A-3). Embedded ColorTable
  layout reuses the round-91 `build_pix_pat_op` convention:
  sequential `value` field, 16-bit-per-channel RGBColor entries with
  the 8-bit input replicated to both bytes (`high8 = low8 =
  channel`). Validation: dimensions / palette-size / pixel-size
  combinations are checked up front, and `rowBytes` is capped at the
  14-bit PICT v2 limit (`0x3FFE`).
- Round 211: 15 synthesis tests in `tests/synth_v2_round211.rs`:
  - 8-bpp `BitsRect` (4×4 / 8×8 / 16×4) and `PackBitsRect`
    round-trips across the three rowBytes regimes (carve-out raw,
    1-byte byteCount, 2-byte byteCount).
  - One round-trip per indexed pixel size: 1-bpp (8×4 vertical
    stripe), 2-bpp (4×4 quadrants), 4-bpp (8×2, 8 distinct palette
    entries within the 16-entry cap), 8-bpp (full 256-entry palette).
  - `BitsRgn` (full-frame clip) and `PackBitsRgn` (4×4 inset clip,
    confirming outside-clip pixels remain canvas-default white).
  - Probe coverage: confirms `PictProbe::indexed_raster_count` is
    bumped (with `raster_count` as the rolling super-count).
  - Validation: zero dims, size mismatch, empty palette, per-pixel-
    size palette overflow (3 entries with 1-bpp, 5 with 2-bpp, 17
    with 4-bpp).
- Round 205: **v1 dispatcher state-machine + text + Same-shape opcode
  coverage per §A-3 Table A-3.** Prior rounds wired the v1 (8-bit-
  opcode) walker for the small shape verbs (`frameRect`..`fillPoly`/
  `fillRgn`), the four pattern opcodes, and every raster opcode
  (`BitsRect`/`BitsRgn`/`PackBitsRect`/`PackBitsRgn`/`DirectBitsRect`/
  `DirectBitsRgn`), but several Table A-3 entries still triggered the
  fatal `unknown / unsupported v1 opcode` error:
  - **State / text setup opcodes** (walked past per the fixed payload
    sizes in Table A-3): `TxFont 0x03` (2), `TxFace 0x04` (1),
    `TxMode 0x05` (2), `SpExtra 0x06` (4), `PnMode 0x08` (2),
    `TxSize 0x0D` (2), `TxRatio 0x10` (8).
  - **Text-glyph opcodes** (walked past — no font rasteriser yet,
    same as v2): `LongText 0x28` (5 + text), `DHText 0x29` (2 + text),
    `DVText 0x2A` (2 + text), `DHDVText 0x2B` (3 + text).
  - **Same-shape opcodes** (use the v2 `last_rect` / `last_rrect` /
    `last_oval` / `last_arc_rect` state slots): `frameSameRect..
    fillSameRect 0x38..0x3C`, `frameSameRRect..fillSameRRect 0x48..
    0x4C`, `frameSameOval..fillSameOval 0x58..0x5C`, `frameSameArc..
    fillSameArc 0x68..0x6C` (the arc family carries a 4-byte
    `start + arc` payload; the other three families are zero-byte).
    Routing reuses the verb-nibble convention via `opcode - 8` so
    `apply_rect_verb` / `apply_rrect_verb` / `apply_oval_verb` /
    `apply_arc_verb` need no v1-specific helpers.
  - **"Not yet implemented" same-shape ranges** (§A-3 marks these
    explicitly NYI with a 0-byte payload): `frameSamePoly..
    fillSamePoly 0x78..0x7C`, `frameSameRgn..fillSameRgn 0x88..0x8C`.
    Accepted as silent no-ops so a private-extension PICT carrying
    one doesn't poison the decode.
  When a `*Same*` opcode runs with no matching `last_*` slot
  established (§A-3 leaves the behaviour implementation-defined), the
  arm silently does nothing — matching QuickDraw's "no previous shape
  to repeat" no-op semantics. The probe walker (`probe::probe_v1_opcode`)
  is widened in lock-step: the new text opcodes increment
  `PictProbe::drawing_count`; the new same-shape opcodes (including
  the NYI poly / rgn ranges) increment `PictProbe::same_shape_count`.
  A `0x35` byte (genuinely undefined in Table A-3) still surfaces the
  `Unsupported` error — the dispatcher was widened, not replaced with
  a catch-all fallback.
- Round 205: 25 synthesis tests in `tests/synth_v1_round205.rs` that
  hand-build minimal v1 PICTs (no launch stub, no v2 sentinel; just
  the 10-byte picture record + `0x11 0x01` version stanza + opcode +
  `paintRect` + `0xFF`) and assert: every state-opcode payload is
  walked past with its §A-3 byte count; every text-opcode payload is
  walked past with its variable count + text bytes; every same-shape
  opcode family (rect / rrect / oval / arc) decodes cleanly with the
  matching last-* slot established; the probe's `same_shape_count`
  increments by exactly the number of *Same* opcodes (4 for one each
  from the four implemented families, 2 for the NYI poly + rgn
  pair); an orphan `paintSameRect` (no prior `paintRect`) is a
  silent no-op; and a genuinely undefined `0x35` still rejects.
  Every byte sequence is traceable back to §A-3 Table A-3 (book
  pages A-18..A-21). No external implementation consulted.

- Round 199: **§A-3 reserved-for-Apple-use v2 opcode skip table** —
  the decoder + probe now walk past every reserved entry in Inside
  Macintosh: Imaging With QuickDraw §A-3 (Table A-2) using the
  published payload size, instead of surfacing them as fatal
  `unknown / unsupported v2 opcode 0xNNNN` errors. The new
  `opcodes::reserved_v2_payload_size(opcode) -> Option<ReservedV2Skip>`
  helper enumerates the published shape for every range:
  - **Fixed** — `0x0035`-`0x0037`, `0x0045`-`0x0047`, `0x0055`-
    `0x0057` (8 bytes), `0x0065`-`0x0067` (12), `0x006D`-`0x006F` (4),
    `0x003D`-`0x003F`, `0x004D`-`0x004F`, `0x005D`-`0x005F`,
    `0x007D`-`0x007F`, `0x0088`-`0x008F` (0), `0x0078`-`0x007C` "Not
    yet implemented" same-poly slots (0), `0x00B0`-`0x00CF` (0),
    `0x8000`-`0x80FF` (0), `0x0100`-`0x7FFF` `2 × nn` (per the §A-3
    page A-5 Note — boundary rows `$0200`→4, `$0BFF`→22, `$0C01`→24,
    `$7FFF`→254 all explicitly tabulated and confirmed by the synth
    tests).
  - **U16-prefixed** (16-bit data-length word + that many bytes) —
    `0x0024`-`0x0027`, `0x002F`, `0x0092`-`0x0097`, `0x009C`-`0x009F`,
    `0x00A2`-`0x00AF`.
  - **U32-prefixed** (32-bit data-length word + that many bytes) —
    `0x00D0`-`0x00FE`, `0x8100`-`0x81FF`, `0x8202`-`0xFFFF`.
  - **Polygon-sized** (16-bit polySize-includes-itself word) —
    `0x0075`-`0x0077`.
  - **Region-sized** (same shape) — `0x0085`-`0x0087`.
  The three `0x0017`-`0x0019` "Not determined" opcodes intentionally
  remain a hard `Unsupported` error — §A-3 leaves their payload size
  unspecified, so any picture that emits one is malformed and silent
  mis-skip is the worse failure mode.
- Round 199: `PictProbe::reserved_op_count: u32` — bumps once per
  reserved-for-Apple-use opcode the walker steps past. Lets a probe
  caller spot PICTs carrying private / Apple-internal extension
  records without paying the full decode cost. Not bumped for the
  "Not determined" range (which still terminates the probe as
  `ProbeTermination::Unsupported`).
- Round 199: 27 synthesis tests in `tests/synth_v2_round199.rs` that
  hand-build minimal v2 PICTs carrying one reserved opcode per range
  and assert the decoder finishes cleanly + the probe's
  `reserved_op_count` matches. Boundary-row coverage (`0x0BFF`,
  `0x0C01`, `0x7FFF`, `0xFFFF`), explicit "Not determined" rejection
  (`0x0018`, `0x0017`), and truncated-payload `InvalidData` shape
  (`0x00D1` declares 100 bytes, supplies 4) are all included. No
  dependence on external PICT fixtures — every byte sequence is
  traceable back to §A-3 Table A-2 + the page A-5 Note.

## [0.0.3](https://github.com/OxideAV/oxideav-pict/compare/v0.0.2...v0.0.3) - 2026-05-29

### Other

- round 186: indexed PixMap variant of BitsRect / PackBitsRect families
- round 95: dithered PixPat sub-type (patType=2)
- round 91: PixPat (multi-colour 8×8 pixel pattern) opcodes
- round 8: monochrome pattern opcodes (PnPat / BkPat / FillPat)
- round 7: read-only probe_pict() introspection API

### Added

- Round 186: **Indexed PixMap variant of `BitsRect 0x0090` / `BitsRgn
  0x0091` / `PackBitsRect 0x0098` / `PackBitsRgn 0x0099`** — decoded
  per Inside Macintosh: Imaging With QuickDraw §A-3 footnote `§`
  (rowBytes high-bit dispatch) + Listing A-2 / A-3 (record layout).
  The rowBytes word's high bit (clear for round-1 1-bpp BitMap, set
  for indexed PixMap) selects between the two on-disk record families;
  the indexed variant carries a full 46-byte PixMap header (no
  `baseAddr` placeholder — that's exclusive to `DirectBitsRect`
  `0x009A` / `DirectBitsRgn` `0x009B` per §A-3 footnote `§`) plus an
  embedded `ColorTable` (`ctSeed(4) + ctFlags(2) + ctSize(2)` followed
  by `(ctSize+1) × { value(2) + r(2) + g(2) + b(2) }`), then the
  standard `srcRect / dstRect / mode` trailer, then PixData. Pixel
  sizes 1 / 2 / 4 / 8 are honoured per §4 ("Color QuickDraw and
  PixMaps"); palette entries are read as 16-bit-per-channel `RGBColor`
  and folded into RGBA via the high byte (`Rgba::from_rgb16`).
  Out-of-range PixData indices map to `Rgba::BLACK` per §4 *"empty
  entries … are drawn as black"*. PixData is raw rows when
  `rowBytes < 8` (§A-3 "PixData" narrow-row carve-out) or when the
  opcode is the unpacked `BitsRect` / `BitsRgn` family; otherwise
  per-row `byteCount`-prefixed PackBits at the rowBytes-byte width.
  The `BitsRgn 0x0091` / `PackBitsRgn 0x0099` indexed variants emit a
  `Region` after the `mode` word that the rasteriser honours as a
  transient blit mask (same path as the round-1 BitMap region
  variant).
- Round 186: `probe::PictProbe::indexed_raster_count` field —
  bumps once per indexed-PixMap `BitsRect` / `BitsRgn` / `PackBitsRect`
  / `PackBitsRgn` so a thumbnail UI can distinguish indexed-vs-direct
  rasters before paying the decode cost. `DirectBitsRect 0x009A` /
  `DirectBitsRgn 0x009B` are never counted here (they're always
  direct, not indexed); they remain in `raster_count` only. The
  probe's v1 + v2 walkers both update the field — `0x90 / 0x91 / 0x98
  / 0x99` opcodes route through a shared per-variant skipper that
  parses the PixMap header + ColorTable accurately.
- Round 186: 6 synthesis tests (`tests/synth_v2_round186.rs`) that
  hand-build indexed-PixMap PICTs byte-by-byte against the §A-3
  listings — `PackBitsRect 4-bpp narrow raw rows`, `PackBitsRect 8-bpp
  PackBits-encoded rows`, `BitsRect 8-bpp unpacked rows`,
  `PackBitsRgn 8-bpp clip-full-frame`, `BitsRect oob-palette-index →
  black`, `PackBitsRect 1-bpp two-entry palette`. No dependence on
  the (round-1) BitMap encoders — the helpers go straight from
  little-Endian RGB samples to the §A-3 wire bytes via local
  `put_u16` / `put_i16` / `put_u32` writers.

- Round 95: **Dithered PixPat sub-type (`patType=2`)** — decoded per
  Inside Macintosh: Imaging With QuickDraw §A-3 Listing A-1 (on-disk
  layout) + §4 ("Color QuickDraw → Pixel Patterns") + §4-90
  (`MakeRGBPat` algorithmic contract). The on-disk record carries only
  a 6-byte target `RGBColor` plus the 8-byte `Pat1Data` monochrome
  fallback; Color QuickDraw's `MakeRGBPat` expands the 8×8 tile at draw
  time against the active `GDevice` palette. Our true-colour RGBA
  canvas satisfies the §4 *"approximates the color"* contract with zero
  approximation error by emitting the target RGB at every cell (§A-3
  luminance guarantee preserved by construction; §4-90's *"this
  implementation opted for a fast pattern selection rather than the
  best possible pattern selection"* confirms the bit-pattern is
  implementation-defined).
- Round 95: `state::PixPattern::from_dither_rgb(rgb, fallback)`
  constructor — builds an 8×8 tile populated uniformly with the target
  colour, preserving the `Pat1Data` fallback verbatim for 1-bpp
  consumers.
- Round 95: `state::PictPattern::DitheredPixmap { rgb, fallback,
  pixels }` enum variant — round-trips the target RGB + `Pat1Data` so
  external inspectors can distinguish `patType=2` from `patType=1`
  (e.g. for re-emission against a different GDevice). `mono()` returns
  `fallback`.
- Round 95: `encoder::build_pix_pat_dither_op(slot, fallback, [r, g,
  b])` — emits the 18-byte on-disk payload (opcode word + `patType=2`
  + 8-byte `Pat1Data` + 6-byte `RGBColor`). The 8-bit input is
  replicated to 16-bit (`high8 = low8`) so the decoder's
  `Rgba::from_rgb16` high-byte selection round-trips bit-exact.
- Round 95: `PictBuilder::pen_dither_pix_pattern` /
  `bg_dither_pix_pattern` / `fill_dither_pix_pattern` chainable
  builder methods — convenience wrappers around
  `build_pix_pat_dither_op` for each of the three slot variants.
- Round 95: `tests/synth_v2_round95.rs` — 10 round-trip tests
  covering: paint verb routing through the pen-dither slot; fill /
  erase verb routing through the matching slots; mono `PnPat`
  clearing the colour slot (most-recent-wins semantics); opcode-word
  emission for each slot via `build_pix_pat_dither_op`; probe
  `pix_pattern_set_count` accounting for `patType=2` opcodes; 16-bit
  RGB high-byte round-trip; solid-black target edge case; mixed
  colour-pixmap + dither slot precedence.
- Round 91: **PixPat (multi-colour 8×8 pixel pattern) opcodes** —
  `BkPixPat 0x0012`, `PnPixPat 0x0013`, `FillPixPat 0x0014` — are now
  decoded per Inside Macintosh: Imaging With QuickDraw §A-3 Listing
  A-1. The `patType=1` colour-pixmap sub-type's `PixMap` (sans
  baseAddr) + `ColorTable` + indexed-pixel `PixData` is parsed,
  resolved into an 8×8 RGBA grid, and folded onto the rasteriser via
  the new `paint_region_pix_pattern` / `fill_rect_pix_pattern` /
  `fill_oval_pix_pattern` / `fill_round_rect_pix_pattern` /
  `fill_polygon_pix_pattern` / `frame_rect_pix_pattern_thick`
  primitives. Frame / paint verbs consult the pen-pix-pat slot, erase
  consults the back-pix-pat slot, fill consults the fill-pix-pat slot
  (PixPat is colour-explicit — fg / bg state is not consulted, unlike
  monochrome `Pattern`). A subsequent mono `PnPat / BkPat / FillPat`
  clears the corresponding colour slot per classic "most-recent wins"
  QuickDraw semantics.
- Round 91: new `state::PixPattern` public type — 8-byte `Pat1Data`
  fallback + `[Rgba; 64]` colour grid + `sample(x, y)` helper
  (wraps modulo 8 on both axes, matching `Pattern::sample`).
- Round 91: new `state::PictPattern` public enum — `Mono(Pattern)`
  vs `ColourPixmap(Box<PixPattern>)`; `mono()` helper returns the
  monochrome representation regardless of variant.
- Round 91: `state::PictState` gains `pen_pix_pat`, `back_pix_pat`,
  `fill_pix_pat: Option<PixPattern>` fields tracking the active
  colour-pattern slot. All three default to `None` so PICTs that never
  emit a PixPat opcode behave identically to round 8.
- Round 91: `encoder::PixPatSlot` enum (`Background` / `Pen` / `Fill`)
  + `encoder::build_pix_pat_op` — emits the bytes for a single PixPat
  opcode (`0x0012` / `0x0013` / `0x0014`) with `patType=1` colour
  pixmap. Dedupes the input RGBA tile into a ColorTable (≤ 256
  entries; theoretical max for an 8×8 tile is 64), emits an 8-bpp
  indexed PixMap header, and PackBits-encodes each row of the indexed
  PixData. Default PackType = no packing per Inside Macintosh §A-3.
- Round 91: `PictBuilder::pen_pix_pattern` / `bg_pix_pattern` /
  `fill_pix_pattern` chainable builder methods on the v2 builder.
- Round 91: `PictProbe::pix_pattern_set_count` field — number of
  PixPat opcodes observed during the walk. The probe walks the same
  byte layout as the decoder (delegating to a `skip_pix_pat` helper)
  so it stays in sync with the decode path.
- Round 91: `tests/synth_v2_round91.rs` — 12 round-trip tests covering
  pen-pix-pat paint of rect / oval / round-rect / polygon / region with
  RGBA tiles; fill / erase verb routing through the matching colour
  slots; palette-dedup correctness on a 4-colour tile; pen-pix-pat →
  pen-pat fallback ordering; probe `pix_pattern_set_count` accounting;
  `build_pix_pat_op` opcode-word emission for each slot; uniform
  solid-colour pix-pat ignoring active fg.
- Round 91: existing `tests/probe.rs::probe_unsupported_opcode_preserves_prior_counts`
  smoke test repointed from `0x0012 BkPixPat` (now supported) to
  `0x0017` (reserved, undefined size per §A-3 Table A-2) so it still
  exercises the "unsupported opcode preserves prior counts" path.

- Round 8: monochrome pattern opcodes (`PnPat 0x0009 / v1 0x09`,
  `BkPat 0x0002 / v1 0x02`, `FillPat 0x000A / v1 0x0A`) are now decoded
  and folded into the rasteriser. Each is an 8-byte 8×8 on/off bitmap
  per Inside Macintosh: Imaging With QuickDraw §A-3; on-bits select
  the current foreground colour and off-bits select the background.
  Frame / paint verbs consult `PnPat`, fill verbs consult `FillPat`,
  erase verbs consult `BkPat` with the inverted on=bg / off=fg
  convention. Invert verbs ignore patterns.
- Round 8: new `state::Pattern` public type — 8-byte monochrome bitmap
  with `Pattern::BLACK` (`[0xFF; 8]`, `qd.black` — solid fg) and
  `Pattern::WHITE` (`[0x00; 8]`, `qd.white` — solid bg) constants plus
  `is_solid_fg` / `is_solid_bg` / `sample(x, y)` helpers. The default
  state matches Mac defaults (PnPat = FillPat = BLACK, BkPat = WHITE)
  so PICTs that never emit a pattern opcode behave identically to the
  round-7 solid-colour pipeline.
- Round 8: `state::PictState` gains `pen_pat`, `back_pat`, `fill_pat`
  fields tracking the current pattern slot for each verb family.
- Round 8: `raster::fill_rect_pattern`, `fill_oval_pattern`,
  `fill_round_rect_pattern`, `fill_polygon_pattern`,
  `frame_rect_pattern_thick` patterned-fill primitives. All-ones and
  all-zeros patterns short-circuit to the existing solid-colour
  primitives so default-pattern PICTs are byte-identical to round 7.
- Round 8: `ops::build_pn_pat` / `build_bk_pat` / `build_fill_pat`
  opcode-bytes helpers plus `PictBuilder::pen_pattern` /
  `bg_pattern` / `fill_pattern` chainable methods on the v2 builder.
- Round 8: `PictProbe::pattern_set_count` field — number of pattern
  opcodes (`PnPat` + `BkPat` + `FillPat`) observed during the walk.
  Recognised on both v1 and v2 streams.
- Round 8: `tests/synth_v2_round8.rs` — 17 round-trip tests covering
  pen-pattern paint of rect / oval / region with horizontal /
  vertical / 50%-grey stipples, fill-pattern routing (fill verb
  consults FillPat not PnPat), background-pattern erase using the
  inverted on=bg convention, frame outline stippling, hand-assembled
  v1 streams for `0x09` (PnPat) and `0x02` (BkPat), probe
  `pattern_set_count` accounting on v1 and v2 streams, pen-pattern
  persistence across multiple draws, and solid-pattern collapse
  byte-equality vs no-pattern.

- Round 7: `probe_pict` — read-only opcode-stream walker returning a
  `PictProbe` (version, picFrame, width/height, has_launch_stub,
  per-category opcode counts, termination cause, terminated_at). No
  pixel data, no canvas allocation; useful for thumbnail UIs, content
  scanners spotting embedded QuickTime payloads, and encoder-side
  test harnesses that want to assert the emitted opcode mix without
  reaching into the decoded raster.
- Round 7: `PictProbe`, `ProbeVersion`, `ProbeTermination`, `ProbeRect`
  public types re-exported from the crate root. Termination cause is
  one of `EndPic` (clean `0x00FF`/`0xFF` terminator), `Eof` (input ran
  out before the terminator), `Unsupported(String)` (the same message
  the decoder would surface), or `Invalid(String)` (truncated payload,
  malformed region/polygon header). Partial statistics are preserved
  across `Unsupported`/`Invalid` terminations.
- Round 7: `PictProbe::has_visible_content` (`true` when any raster /
  drawing / same-shape opcode appears) and `PictProbe::has_quicktime`
  (`true` when the stream embeds a `CompressedQuickTime` or
  `UncompressedQuickTime` opcode) helpers.
- Round 7: `tests/probe.rs` — 18 round-trip tests covering v1 and v2
  framing detection, launch-stub vs raw-body detection, per-category
  opcode counting (drawing primitives, same-shape, lines, polygons,
  comments, ClipRgn, embedded QuickTime), all four DirectBitsRect
  packTypes (1/2/3/4), 1-bpp PackBitsRect with `rowBytes ≥ 8`, EOF
  termination without `OpEndPic`, framing-error rejection, and
  unsupported-opcode partial-statistic preservation.

## [0.0.2](https://github.com/OxideAV/oxideav-pict/compare/v0.0.1...v0.0.2) - 2026-05-08

### Other

- encoder round 6: ClipRgn honoured + pen-size aware drawing + BitsRgn/PackBitsRgn emit
- encoder round 5: v1+PackType, 1-bpp BitMap emit, builder+raster
- oxideav-pict round 4: encoder push to ~60% — packType 3 + drawing-command builder
- drop stale REGISTRARS / with_all_features intra-doc links
- drop dead `linkme` dep
- re-export __oxideav_entry from registry sub-module
- fix needless_range_loop clippy lint in synth_v2_round3
- oxideav-pict round 3: encoder push to ~60% — packType 2/4 + v1 emit + ClipRgn + decoder v1 DirectBits fix
- oxideav-pict round 2: drawing-command rasteriser + DirectBits packType 2/3/4 + Region paths + v1 raster + writer
- auto-register via oxideav_core::register! macro (linkme distributed slice)
- unify entry point on register(&mut RuntimeContext) ([#502](https://github.com/OxideAV/oxideav-pict/pull/502))
- register .pict / .pic / .pct extensions

### Added

- Round 6: drawing-clipping by region. The decoder now honours
  `ClipRgn` (v2 `0x0001`, v1 `0x01`) and the per-blit region embedded
  in `BitsRgn` (`0x0091`) / `PackBitsRgn` (`0x0099`) /
  `DirectBitsRgn` (`0x009B`). Implementation: `Canvas` carries an
  optional `width × height` boolean mask; every plot primitive
  (`Canvas::put` / `Canvas::span` / `Canvas::blit`) consults it.
  `ClipRgn` is materialised once into a canvas-local mask and survives
  across opcodes; per-blit `Rgn` opcodes intersect their region with
  the active clip for that blit only and restore afterwards.
- Round 6: pen-size aware drawing. `Line` / `LineFrom` / `ShortLine` /
  `ShortLineFrom` / `Frame(Rect|Oval)` now stamp a `pen_h × pen_v`
  brush at every plot per `PnSize`. New rasteriser primitives
  `line_thick`, `frame_rect_thick`, `frame_oval_thick`. 1×1 pens
  collapse to the original 1-pixel primitives so default behaviour
  is unchanged.
- Round 6: `encode_pict_bits_rgn` (`0x0091`) /
  `encode_pict_pack_bits_rgn` (`0x0099`) — 1-bpp BitMap encoders
  with an attached rectangular clip region. Mirror round 5's
  `encode_pict_bits_rect` / `encode_pict_pack_bits_rect` byte layout
  with a 10-byte rectangular region inserted after the rect/mode
  header.
- Round 6: `tests/synth_v2_round6.rs` — 13 round-trip tests covering
  `ClipRgn` masks (raster + drawing primitives + line), pen-size aware
  lines + frame-rects (3×3, 2×2, 1×1 fallback), and `BitsRgn` /
  `PackBitsRgn` encoders (full-clip + narrow-clip + RLE-branch
  round-trip + size-mismatch rejection).

- Round 5: `encode_pict_v1_with` — v1 PICT emit with a `PackType`
  selector (raw / packed-24 / 16-bpp PackBits / component-separated
  PackBits), bringing v1 emit to parity with v2. The legacy
  `encode_pict_v1` is preserved as a `PackType::Raw` adapter so the
  round-3 API stays callable; a 32×32 solid image shrinks ~89 % under
  v1 packType 3 vs v1 packType 1.
- Round 5: `encode_pict_bits_rect` / `encode_pict_pack_bits_rect` —
  1-bpp BitMap encoders emitting `BitsRect` (`0x0090`) and
  `PackBitsRect` (`0x0098`) opcodes for monochrome content. Pixels
  are reduced from RGBA via a 50 %-luminance threshold (Y =
  0.299 R + 0.587 G + 0.114 B). PackBitsRect path takes the per-row
  RLE branch when `rowBytes >= 8` (Inside Macintosh §A-3) and
  fall-through to raw rows otherwise.
- Round 5: `build_direct_bits_rect_op` — public helper that builds
  the bytes for a single PICT v2 `DirectBitsRect` (`0x009A`) opcode
  + payload (no stub, no headerOp, no `OpEndPic`). Used by
  `PictBuilder::raster` and exposed for callers needing the raw
  opcode chunk.
- Round 5: `PictBuilder::raster` — appends a raster opcode to a
  drawing-only builder, allowing mixed drawing + raster in the same
  v2 stream. Drawing primitives emitted before paint underneath the
  raster; primitives emitted after overlay it.
- Round 5: `tests/synth_v2_round5.rs` — 18 round-trip tests covering
  v1 packType 2/3/4 emit, 1-bpp BitsRect / PackBitsRect (all-white,
  all-black, checkerboard, luminance threshold, narrow-row
  fall-through, wide-row PackBits compression), and builder + raster
  composition (raster alone, drawing-then-raster overlay,
  raster-then-drawing overlay, packType 3 raster).

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
