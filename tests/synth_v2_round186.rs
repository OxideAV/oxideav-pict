//! Round 186 tests — indexed PixMap variant of `BitsRect 0x0090`,
//! `BitsRgn 0x0091`, `PackBitsRect 0x0098` and `PackBitsRgn 0x0099`.
//!
//! Inside Macintosh: Imaging With QuickDraw §A-3 footnote `§` documents
//! the rowBytes-high-bit dispatch that toggles each of those four
//! opcodes from its 1-bpp BitMap (round 1) layout to a §A-3 Listing
//! A-2 / A-3 indexed PixMap (this round) layout. The on-disk record
//! family is:
//!
//! ```text
//! rowBytes(2)  -- high bit set marks PixMap
//! PixMap (44 bytes after rowBytes): bounds(8) + pmVersion(2) +
//!   packType(2) + packSize(4) + hRes(4) + vRes(4) +
//!   pixelType(2) + pixelSize(2) + cmpCount(2) + cmpSize(2) +
//!   planeBytes(4) + pmTable(4) + pmReserved(4)
//! ColorTable: ctSeed(4) + ctFlags(2) + ctSize(2) +
//!             (ctSize+1) × { value(2) + r(2) + g(2) + b(2) }
//! srcRect(8) + dstRect(8) + mode(2)
//! [maskRgn — only `0x0091` / `0x0099`]
//! PixData (per §A-3 "PixData"):
//!   rowBytes < 8 OR Bits family: raw unpacked rows
//!   else (PackBits family): per-row PackBits at rowBytes-byte width
//! ```
//!
//! The synth helpers in this file hand-build that record byte-by-byte
//! against the clean-room spec PDFs in `docs/image/quickdraw/` —
//! they do not depend on the (round-1) BitMap encoder paths.

use oxideav_pict::{parse_pict, probe_pict, ProbeTermination};

// ---------------------------------------------------------------------------
// Spec-driven byte builders.
// ---------------------------------------------------------------------------

fn put_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_be_bytes());
}
fn put_i16(out: &mut Vec<u8>, v: i16) {
    out.extend_from_slice(&v.to_be_bytes());
}
fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_be_bytes());
}

/// 46-byte PixMap header (rowBytes word INCLUDED, top bit set).
fn put_indexed_pixmap_header(
    out: &mut Vec<u8>,
    row_bytes: u16,
    width: i16,
    height: i16,
    pixel_size: u16,
) {
    // rowBytes — high bit set per §A-3 footnote `§`.
    put_u16(out, row_bytes | 0x8000);
    // bounds: top, left, bottom, right.
    put_i16(out, 0);
    put_i16(out, 0);
    put_i16(out, height);
    put_i16(out, width);
    // pmVersion.
    put_u16(out, 0);
    // packType — §A-3 packing-types table: 0 = default for indexed (raw or
    // per-row PackBits depending on rowBytes; here we let the opcode
    // family decide as both decoder & spec do).
    put_u16(out, 0);
    // packSize — §A-3: "must be 0 for future compatibility".
    put_u32(out, 0);
    // hRes / vRes — 72 dpi as Fixed (72 << 16).
    put_u32(out, 72 << 16);
    put_u32(out, 72 << 16);
    // pixelType — 0 for indexed/chunky per §4.
    put_u16(out, 0);
    // pixelSize.
    put_u16(out, pixel_size);
    // cmpCount + cmpSize — 1 channel of `pixel_size` bits for indexed.
    put_u16(out, 1);
    put_u16(out, pixel_size);
    // planeBytes / pmTable / pmReserved — all zero in PICT-embedded
    // PixMaps per §A-3 Listing A-2.
    put_u32(out, 0);
    put_u32(out, 0);
    put_u32(out, 0);
}

/// ColorTable: ctSeed + ctFlags + ctSize + (ctSize+1) × ColorSpec.
fn put_color_table(out: &mut Vec<u8>, palette: &[[u8; 3]]) {
    put_u32(out, 0xDEADBEEF); // ctSeed
    put_i16(out, 0); // ctFlags
                     // ctSize = number of entries - 1.
    put_i16(out, (palette.len() as i16) - 1);
    for (i, [r, g, b]) in palette.iter().enumerate() {
        put_u16(out, i as u16); // value (entry's index)
                                // 16-bit-per-channel RGBColor — replicate the 8-bit input
                                // across both bytes so `Rgba::from_rgb16` (high byte) is
                                // bit-exact on round-trip.
        put_u16(out, u16::from_be_bytes([*r, *r]));
        put_u16(out, u16::from_be_bytes([*g, *g]));
        put_u16(out, u16::from_be_bytes([*b, *b]));
    }
}

/// Picture wrapper (no 512-byte launch stub for compactness): picSize +
/// picFrame + v2 sentinel + 24-byte HeaderOp.
fn put_pict_v2_prefix(out: &mut Vec<u8>, width: i16, height: i16) {
    put_u16(out, 0); // picSize (unused)
    put_i16(out, 0); // top
    put_i16(out, 0); // left
    put_i16(out, height);
    put_i16(out, width);
    put_u16(out, 0x0011); // VersionOp
    put_u16(out, 0x02FF); // v2 sentinel
    put_u16(out, 0x0C00); // HeaderOp
    out.extend_from_slice(&[0u8; 24]);
}

/// Standard 4-entry palette: black / red / green / blue. Indices 0..=3.
fn rgb_palette_4() -> [[u8; 3]; 4] {
    [
        [0x00, 0x00, 0x00], // 0 — black
        [0xFF, 0x00, 0x00], // 1 — red
        [0x00, 0xFF, 0x00], // 2 — green
        [0x00, 0x00, 0xFF], // 3 — blue
    ]
}

/// Pack a row of 4-bpp indices: two pixels per byte, high nibble first
/// (MSB-first per QuickDraw convention).
fn pack_4bpp(indices: &[u8], row_bytes: usize) -> Vec<u8> {
    let mut out = vec![0u8; row_bytes];
    for (x, idx) in indices.iter().enumerate() {
        let byte_off = x >> 1;
        if (x & 1) == 0 {
            out[byte_off] |= (idx & 0x0F) << 4;
        } else {
            out[byte_off] |= idx & 0x0F;
        }
    }
    out
}

/// Pack 8-bpp indices — one byte per pixel.
fn pack_8bpp(indices: &[u8], width: usize, row_bytes: usize) -> Vec<u8> {
    let mut out = vec![0u8; row_bytes];
    out[..width].copy_from_slice(&indices[..width]);
    out
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

/// `PackBitsRect 0x0098` indexed PixMap, 4-bpp, 4-entry palette, width 4
/// (rowBytes = 2 so the "rowBytes < 8" raw-row carve-out applies per
/// §A-3 "PixData").
#[test]
fn indexed_pixmap_pack_bits_rect_4bpp_raw_narrow() {
    let width: i16 = 4;
    let height: i16 = 2;
    let row_bytes: u16 = 2; // 4 pixels × 4 bpp = 16 bits = 2 bytes
    let palette = rgb_palette_4();

    let mut bytes: Vec<u8> = Vec::new();
    put_pict_v2_prefix(&mut bytes, width, height);

    // PackBitsRect opcode.
    put_u16(&mut bytes, 0x0098);
    put_indexed_pixmap_header(
        &mut bytes, row_bytes, width, height, /* pixel_size = */ 4,
    );
    put_color_table(&mut bytes, &palette);
    // srcRect.
    put_i16(&mut bytes, 0);
    put_i16(&mut bytes, 0);
    put_i16(&mut bytes, height);
    put_i16(&mut bytes, width);
    // dstRect — same as picFrame.
    put_i16(&mut bytes, 0);
    put_i16(&mut bytes, 0);
    put_i16(&mut bytes, height);
    put_i16(&mut bytes, width);
    // mode.
    put_u16(&mut bytes, 0);

    // PixData — rowBytes=2 < 8 → raw rows per §A-3 "PixData".
    // Row 0: [red, green, blue, black] = [1, 2, 3, 0].
    bytes.extend_from_slice(&pack_4bpp(&[1, 2, 3, 0], row_bytes as usize));
    // Row 1: [black, blue, green, red] = [0, 3, 2, 1].
    bytes.extend_from_slice(&pack_4bpp(&[0, 3, 2, 1], row_bytes as usize));

    // Word-align before OpEndPic.
    if bytes.len() % 2 != 0 {
        bytes.push(0);
    }
    put_u16(&mut bytes, 0x00FF);

    let img = parse_pict(&bytes).expect("decode indexed PackBitsRect");
    assert_eq!(img.width, width as u32);
    assert_eq!(img.height, height as u32);

    // Row 0 pixel 0 → red.
    let p = |x: u32, y: u32| {
        let off = ((y * img.width + x) * 4) as usize;
        [
            img.data[off],
            img.data[off + 1],
            img.data[off + 2],
            img.data[off + 3],
        ]
    };
    assert_eq!(p(0, 0), [0xFF, 0x00, 0x00, 0xFF]); // red
    assert_eq!(p(1, 0), [0x00, 0xFF, 0x00, 0xFF]); // green
    assert_eq!(p(2, 0), [0x00, 0x00, 0xFF, 0xFF]); // blue
    assert_eq!(p(3, 0), [0x00, 0x00, 0x00, 0xFF]); // black
    assert_eq!(p(0, 1), [0x00, 0x00, 0x00, 0xFF]); // black
    assert_eq!(p(1, 1), [0x00, 0x00, 0xFF, 0xFF]); // blue
    assert_eq!(p(2, 1), [0x00, 0xFF, 0x00, 0xFF]); // green
    assert_eq!(p(3, 1), [0xFF, 0x00, 0x00, 0xFF]); // red

    // Probe should see one raster, flagged indexed.
    let p = probe_pict(&bytes).expect("probe");
    assert_eq!(p.raster_count, 1);
    assert_eq!(p.indexed_raster_count, 1);
    assert_eq!(p.termination, ProbeTermination::EndPic);
}

/// `PackBitsRect 0x0098` indexed PixMap, 8-bpp, 64-pixel-wide row so the
/// per-row PackBits byteCount-prefixed path is exercised (§A-3 "PixData"
/// — *"if rowBytes ≥ 8 … each scanline consists of byteCount + data"*).
#[test]
fn indexed_pixmap_pack_bits_rect_8bpp_packed_rows() {
    let width: i16 = 64;
    let height: i16 = 4;
    let row_bytes: u16 = 64; // 1 byte per pixel
    let palette = rgb_palette_4();

    let mut bytes: Vec<u8> = Vec::new();
    put_pict_v2_prefix(&mut bytes, width, height);
    put_u16(&mut bytes, 0x0098);
    put_indexed_pixmap_header(
        &mut bytes, row_bytes, width, height, /* pixel_size = */ 8,
    );
    put_color_table(&mut bytes, &palette);
    // srcRect.
    put_i16(&mut bytes, 0);
    put_i16(&mut bytes, 0);
    put_i16(&mut bytes, height);
    put_i16(&mut bytes, width);
    // dstRect.
    put_i16(&mut bytes, 0);
    put_i16(&mut bytes, 0);
    put_i16(&mut bytes, height);
    put_i16(&mut bytes, width);
    // mode.
    put_u16(&mut bytes, 0);

    // PackBits-encoded rows. Each row is 64 pixels of a single colour
    // index — the PackBits literal/repeat encoding compresses 64 copies
    // of one byte down to a 2-byte run (count + value).
    for y in 0..height as u8 {
        let idx = y % 4;
        let raw = vec![idx; width as usize];
        let enc = oxideav_pict::packbits::encode(&raw);
        // rowBytes=64 <= 250 → 1-byte length prefix.
        bytes.push(enc.len() as u8);
        bytes.extend_from_slice(&enc);
    }

    if bytes.len() % 2 != 0 {
        bytes.push(0);
    }
    put_u16(&mut bytes, 0x00FF);

    let img = parse_pict(&bytes).expect("decode 8-bpp indexed PackBitsRect");
    assert_eq!(img.width, width as u32);
    assert_eq!(img.height, height as u32);
    let p = |x: u32, y: u32| {
        let off = ((y * img.width + x) * 4) as usize;
        [
            img.data[off],
            img.data[off + 1],
            img.data[off + 2],
            img.data[off + 3],
        ]
    };
    // Every pixel on row y is `palette[y % 4]`.
    let expect = |idx: usize| {
        let [r, g, b] = palette[idx];
        [r, g, b, 0xFF]
    };
    for x in 0..width as u32 {
        assert_eq!(p(x, 0), expect(0));
        assert_eq!(p(x, 1), expect(1));
        assert_eq!(p(x, 2), expect(2));
        assert_eq!(p(x, 3), expect(3));
    }

    let probe = probe_pict(&bytes).expect("probe");
    assert_eq!(probe.raster_count, 1);
    assert_eq!(probe.indexed_raster_count, 1);
}

/// `BitsRect 0x0090` indexed PixMap (the unpacked Bits family — every
/// row is raw regardless of rowBytes per §A-3 footnote `§`).
#[test]
fn indexed_pixmap_bits_rect_8bpp_unpacked_rows() {
    let width: i16 = 8;
    let height: i16 = 2;
    let row_bytes: u16 = 8;
    let palette = rgb_palette_4();

    let mut bytes: Vec<u8> = Vec::new();
    put_pict_v2_prefix(&mut bytes, width, height);
    put_u16(&mut bytes, 0x0090);
    put_indexed_pixmap_header(
        &mut bytes, row_bytes, width, height, /* pixel_size = */ 8,
    );
    put_color_table(&mut bytes, &palette);
    // srcRect / dstRect / mode.
    for _ in 0..2 {
        put_i16(&mut bytes, 0);
        put_i16(&mut bytes, 0);
        put_i16(&mut bytes, height);
        put_i16(&mut bytes, width);
    }
    put_u16(&mut bytes, 0);

    // Raw 8-bpp rows.
    bytes.extend_from_slice(&pack_8bpp(
        &[0, 1, 2, 3, 0, 1, 2, 3],
        width as usize,
        row_bytes as usize,
    ));
    bytes.extend_from_slice(&pack_8bpp(
        &[3, 2, 1, 0, 3, 2, 1, 0],
        width as usize,
        row_bytes as usize,
    ));

    if bytes.len() % 2 != 0 {
        bytes.push(0);
    }
    put_u16(&mut bytes, 0x00FF);

    let img = parse_pict(&bytes).expect("decode indexed BitsRect");
    assert_eq!(img.width, 8);
    assert_eq!(img.height, 2);

    let p = |x: u32, y: u32| {
        let off = ((y * img.width + x) * 4) as usize;
        [
            img.data[off],
            img.data[off + 1],
            img.data[off + 2],
            img.data[off + 3],
        ]
    };
    assert_eq!(p(0, 0), [0x00, 0x00, 0x00, 0xFF]); // black
    assert_eq!(p(3, 0), [0x00, 0x00, 0xFF, 0xFF]); // blue
    assert_eq!(p(0, 1), [0x00, 0x00, 0xFF, 0xFF]); // blue
    assert_eq!(p(7, 1), [0x00, 0x00, 0x00, 0xFF]); // black

    let probe = probe_pict(&bytes).expect("probe");
    assert_eq!(probe.raster_count, 1);
    assert_eq!(probe.indexed_raster_count, 1);
}

/// `PackBitsRgn 0x0099` indexed PixMap — same as the PackBitsRect test
/// above but with a `Region` clip just before the PixData. The region
/// is the trivial 10-byte rectangular form covering the full bounds, so
/// the decoded RGBA should match the unclipped case.
#[test]
fn indexed_pixmap_pack_bits_rgn_8bpp_clip_full_frame() {
    let width: i16 = 64;
    let height: i16 = 2;
    let row_bytes: u16 = 64;
    let palette = rgb_palette_4();

    let mut bytes: Vec<u8> = Vec::new();
    put_pict_v2_prefix(&mut bytes, width, height);
    put_u16(&mut bytes, 0x0099);
    put_indexed_pixmap_header(
        &mut bytes, row_bytes, width, height, /* pixel_size = */ 8,
    );
    put_color_table(&mut bytes, &palette);
    // srcRect / dstRect / mode.
    for _ in 0..2 {
        put_i16(&mut bytes, 0);
        put_i16(&mut bytes, 0);
        put_i16(&mut bytes, height);
        put_i16(&mut bytes, width);
    }
    put_u16(&mut bytes, 0);

    // Region: rgnSize=10 + bbox covering the full frame.
    put_u16(&mut bytes, 10);
    put_i16(&mut bytes, 0);
    put_i16(&mut bytes, 0);
    put_i16(&mut bytes, height);
    put_i16(&mut bytes, width);

    // PackBits rows of solid palette[1] (red) and palette[2] (green).
    for y in 0..height as u8 {
        let idx = if y == 0 { 1 } else { 2 };
        let raw = vec![idx; width as usize];
        let enc = oxideav_pict::packbits::encode(&raw);
        bytes.push(enc.len() as u8);
        bytes.extend_from_slice(&enc);
    }

    if bytes.len() % 2 != 0 {
        bytes.push(0);
    }
    put_u16(&mut bytes, 0x00FF);

    let img = parse_pict(&bytes).expect("decode indexed PackBitsRgn");
    assert_eq!(img.width, width as u32);
    assert_eq!(img.height, height as u32);

    let p = |x: u32, y: u32| {
        let off = ((y * img.width + x) * 4) as usize;
        [
            img.data[off],
            img.data[off + 1],
            img.data[off + 2],
            img.data[off + 3],
        ]
    };
    // Row 0 → red, row 1 → green, full width.
    for x in 0..width as u32 {
        assert_eq!(p(x, 0), [0xFF, 0x00, 0x00, 0xFF]);
        assert_eq!(p(x, 1), [0x00, 0xFF, 0x00, 0xFF]);
    }

    let probe = probe_pict(&bytes).expect("probe");
    assert_eq!(probe.raster_count, 1);
    assert_eq!(probe.indexed_raster_count, 1);
}

/// Out-of-range palette indices fall back to `Rgba::BLACK` per §4
/// ("Color QuickDraw and PixMaps") — empty entries are drawn as black.
#[test]
fn indexed_pixmap_out_of_range_index_maps_to_black() {
    let width: i16 = 4;
    let height: i16 = 1;
    let row_bytes: u16 = 4;
    // Two-entry palette (red, green); indices 2 and 3 are out of range
    // and must map to black.
    let palette: Vec<[u8; 3]> = vec![[0xFF, 0x00, 0x00], [0x00, 0xFF, 0x00]];

    let mut bytes: Vec<u8> = Vec::new();
    put_pict_v2_prefix(&mut bytes, width, height);
    put_u16(&mut bytes, 0x0090); // BitsRect (raw rows)
    put_indexed_pixmap_header(
        &mut bytes, row_bytes, width, height, /* pixel_size = */ 8,
    );
    put_color_table(&mut bytes, &palette);
    for _ in 0..2 {
        put_i16(&mut bytes, 0);
        put_i16(&mut bytes, 0);
        put_i16(&mut bytes, height);
        put_i16(&mut bytes, width);
    }
    put_u16(&mut bytes, 0);

    // Row: [palette[0]=red, palette[1]=green, idx 2=oob, idx 3=oob].
    bytes.extend_from_slice(&[0, 1, 2, 3]);

    if bytes.len() % 2 != 0 {
        bytes.push(0);
    }
    put_u16(&mut bytes, 0x00FF);

    let img = parse_pict(&bytes).expect("decode oob indexed BitsRect");
    let p = |x: u32| {
        let off = (x * 4) as usize;
        [
            img.data[off],
            img.data[off + 1],
            img.data[off + 2],
            img.data[off + 3],
        ]
    };
    assert_eq!(p(0), [0xFF, 0x00, 0x00, 0xFF]); // palette[0]
    assert_eq!(p(1), [0x00, 0xFF, 0x00, 0xFF]); // palette[1]
    assert_eq!(p(2), [0x00, 0x00, 0x00, 0xFF]); // oob → black
    assert_eq!(p(3), [0x00, 0x00, 0x00, 0xFF]); // oob → black
}

/// pixelSize=1 indexed PixMap with a two-entry palette is the indexed
/// equivalent of the round-1 1-bpp BitMap path — every bit selects
/// `palette[bit]`. Exercises the `read_indexed_pixel` MSB-first
/// bit-ordering at the pixelSize=1 leg.
#[test]
fn indexed_pixmap_pack_bits_rect_1bpp() {
    let width: i16 = 8;
    let height: i16 = 1;
    let row_bytes: u16 = 1; // 8 bits = 1 byte
    let palette: Vec<[u8; 3]> = vec![[0xFF, 0xFF, 0xFF], [0x00, 0x00, 0x00]];

    let mut bytes: Vec<u8> = Vec::new();
    put_pict_v2_prefix(&mut bytes, width, height);
    put_u16(&mut bytes, 0x0098);
    put_indexed_pixmap_header(
        &mut bytes, row_bytes, width, height, /* pixel_size = */ 1,
    );
    put_color_table(&mut bytes, &palette);
    for _ in 0..2 {
        put_i16(&mut bytes, 0);
        put_i16(&mut bytes, 0);
        put_i16(&mut bytes, height);
        put_i16(&mut bytes, width);
    }
    put_u16(&mut bytes, 0);

    // rowBytes=1 < 8 → raw row per §A-3.
    // 0b10101010 = pixels [black, white, black, white, black, white, black, white].
    bytes.push(0b1010_1010);

    if bytes.len() % 2 != 0 {
        bytes.push(0);
    }
    put_u16(&mut bytes, 0x00FF);

    let img = parse_pict(&bytes).expect("decode 1-bpp indexed PackBitsRect");
    let p = |x: u32| {
        let off = (x * 4) as usize;
        [
            img.data[off],
            img.data[off + 1],
            img.data[off + 2],
            img.data[off + 3],
        ]
    };
    assert_eq!(p(0), [0x00, 0x00, 0x00, 0xFF]); // bit 1 → palette[1] = black
    assert_eq!(p(1), [0xFF, 0xFF, 0xFF, 0xFF]); // bit 0 → palette[0] = white
    assert_eq!(p(2), [0x00, 0x00, 0x00, 0xFF]);
    assert_eq!(p(7), [0xFF, 0xFF, 0xFF, 0xFF]);

    let probe = probe_pict(&bytes).expect("probe");
    assert_eq!(probe.indexed_raster_count, 1);
}
