# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

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
