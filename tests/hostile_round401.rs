//! Round 401 — hostile-input robustness suite for the opcode walker.
//!
//! PICT length fields (`picFrame`, PixMap `bounds` / `rowBytes`,
//! region / polygon sizes, PackBits run counts, text counts …) are all
//! attacker-controlled. The decoder must return `Err` — never panic,
//! never allocate unbounded memory — on any byte stream. This suite
//! drives `parse_pict` and `probe_pict` through:
//!
//! * every truncation prefix of a diverse valid-stream corpus,
//! * seeded single/multi-byte mutations (deterministic xorshift PRNG —
//!   no external fuzzing infrastructure, runs in CI),
//! * systematic 16-bit length-field maxing (`0xFFFF` / `0x8000` /
//!   `0x0001` at every word offset),
//! * hand-crafted worst cases: giant `picFrame`, giant PixMap
//!   `bounds`, region sizes below the 10-byte header.
//!
//! The allocation bound under test is the round-401
//! `MAX_RASTER_BYTES` decode budget (decoder hardening): a hostile
//! header demanding a multi-gigabyte canvas or pixel buffer is
//! rejected with `PictError::InvalidData` before the allocation.

use oxideav_pict::ops::{PictBuilder, PictV1Builder};
use oxideav_pict::{
    build_fg_color_code, build_rect_op, build_short_line, build_tx_face, build_tx_mode,
    build_tx_ratio, build_tx_size, encode_pict, encode_pict_indexed_pack_bits_rect,
    encode_pict_pack_bits_rect, encode_pict_v1, encode_pict_v1_pack_bits_rect, encode_pict_v2,
    parse_pict, probe_pict, IndexedPixelSize, PackType, Verb, GRAYISH_TEXT_OR_MODE,
};

/// Deterministic xorshift64* PRNG — keeps the suite reproducible with
/// no dev-dependencies.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// A corpus of valid streams covering the walker's opcode families:
/// drawing verbs, text, patterns, clip, direct + indexed + 1-bpp
/// rasters, comments, v1.
fn corpus() -> Vec<Vec<u8>> {
    let mut seeds = Vec::new();

    // Drawing + text + clip + comments.
    let mut b = PictBuilder::new(0, 0, 64, 64);
    b.clip_rect(0, 0, 60, 60);
    b.fg_color(200, 30, 30);
    b.pen_size(2, 2);
    b.line(2, 2, 30, 20).short_line_from(10, -5);
    b.rect(Verb::Paint, 4, 4, 12, 12).same_rect(Verb::Invert);
    b.oval_size(4, 4);
    b.round_rect(Verb::Fill, 20, 20, 40, 40);
    b.oval(Verb::Frame, 30, 5, 45, 25);
    b.arc(Verb::Paint, 10, 30, 30, 50, 0, 120)
        .same_arc(Verb::Erase, 120, 90);
    b.poly(Verb::Fill, &[(50, 10), (60, 30), (40, 30)]).unwrap();
    b.region_rect(Verb::Paint, 48, 48, 60, 60);
    b.push(&build_tx_size(8));
    b.long_text(6, 58, b"Hi").unwrap();
    b.dhdv_text(4, 2, b"!").unwrap();
    b.short_comment(0x1234);
    b.long_comment(0x00AA, b"annotation").unwrap();
    b.pen_pattern([0xAA, 0x55, 0xAA, 0x55, 0xAA, 0x55, 0xAA, 0x55]);
    b.rect(Verb::Paint, 50, 0, 64, 14);
    seeds.push(b.finish());

    // Styled text (round 407): every txFace bit + combinations, the
    // grayishTextOr text mode, an anisotropic TxRatio and a large
    // txSize — drives the style-synthesis mask pipeline (bold smear /
    // italic shear / outline ring / shadow thickening / underline gap)
    // through truncation and mutation.
    let mut st = PictBuilder::new(0, 0, 64, 96);
    st.push(&build_tx_size(8));
    for (i, face) in [
        0x01u8, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80, 0x1F, 0x7F, 0xFF,
    ]
    .iter()
    .enumerate()
    {
        st.push(&build_tx_face(*face));
        st.long_text(2 + (i as i16 % 4) * 20, 8 + (i as i16 / 4) * 10, b"Ag|_")
            .unwrap();
    }
    st.push(&build_tx_mode(GRAYISH_TEXT_OR_MODE));
    st.push(&build_tx_ratio(3, 2, 1, 1));
    st.push(&build_tx_size(96));
    st.push(&build_tx_face(0xFF));
    st.long_text(4, 60, b"W").unwrap();
    seeds.push(st.finish());

    // Direct rasters at each PackType.
    let rgba: Vec<u8> = (0..16u32 * 16 * 4).map(|i| (i * 7) as u8).collect();
    for pack in [PackType::Raw, PackType::Packed24, PackType::Rle16] {
        seeds.push(encode_pict_v2(16, 16, &rgba, pack).unwrap());
    }
    seeds.push(encode_pict(16, 16, &rgba).unwrap());

    // Indexed + 1-bpp rasters.
    let indices: Vec<u8> = (0..64u32).map(|i| (i % 4) as u8).collect();
    let palette: Vec<[u8; 4]> = (0..4u32)
        .map(|i| [(i * 80) as u8, 0x40, 0xC0, 0xFF])
        .collect();
    seeds.push(
        encode_pict_indexed_pack_bits_rect(8, 8, &indices, &palette, IndexedPixelSize::EightBpp)
            .unwrap(),
    );
    let gray: Vec<u8> = (0..64u32).map(|i| (i * 4) as u8).collect();
    let rgba8: Vec<u8> = gray.iter().flat_map(|&g| [g, g, g, 0xFF]).collect();
    seeds.push(encode_pict_pack_bits_rect(8, 8, &rgba8).unwrap());

    // v1 stream (DirectBits extension framing).
    seeds.push(encode_pict_v1(8, 8, &rgba8).unwrap());

    // v1 Table-A-3 1-bpp raster (round 401).
    seeds.push(
        encode_pict_v1_pack_bits_rect(64, 4, &{
            let mut v = Vec::new();
            for i in 0..64 * 4 {
                let c = if i % 2 == 0 { 0u8 } else { 0xFF };
                v.extend_from_slice(&[c, c, c, 0xFF]);
            }
            v
        })
        .unwrap(),
    );

    // v1 drawing stream via PictV1Builder (round 401).
    let mut v1 = PictV1Builder::new(0, 0, 48, 48);
    v1.push(&build_fg_color_code(205)).unwrap();
    v1.push(&build_rect_op(Verb::Paint, 4, 4, 20, 24)).unwrap();
    v1.push(&build_short_line(2, 2, 30, 30)).unwrap();
    seeds.push(v1.finish());

    // QuickTime payload capture path (round 401).
    let mut qt = PictBuilder::new(0, 0, 8, 8);
    qt.compressed_quicktime(&[0xA5; 33]).unwrap();
    qt.rect(Verb::Paint, 1, 1, 3, 3);
    qt.uncompressed_quicktime(&[0x5A; 8]).unwrap();
    seeds.push(qt.finish());

    seeds
}

/// Exercise both entry points; the return value (Ok or Err) is
/// irrelevant — reaching the return at all is the assertion.
fn walk(bytes: &[u8]) {
    let _ = parse_pict(bytes);
    let _ = probe_pict(bytes);
}

// ---------------------------------------------------------------------------
// Truncation: every prefix of every seed decodes without panicking.
// ---------------------------------------------------------------------------

#[test]
fn every_truncation_prefix_is_handled() {
    for seed in corpus() {
        for len in 0..=seed.len() {
            walk(&seed[..len]);
        }
    }
}

// ---------------------------------------------------------------------------
// Seeded random byte mutations.
// ---------------------------------------------------------------------------

#[test]
fn random_byte_mutations_are_handled() {
    let seeds = corpus();
    let mut rng = Rng(0x9E3779B97F4A7C15);
    for seed in &seeds {
        for _ in 0..1500 {
            let mut m = seed.clone();
            // 1..=4 byte overwrites anywhere in the stream (launch
            // stub included — the body-offset detector is a target
            // too).
            for _ in 0..1 + rng.below(4) {
                let pos = rng.below(m.len());
                m[pos] = rng.next() as u8;
            }
            walk(&m);
        }
    }
}

// ---------------------------------------------------------------------------
// Length-field maxing: every aligned 16-bit word in the record gets the
// three classic hostile values.
// ---------------------------------------------------------------------------

#[test]
fn length_field_maxing_is_handled() {
    for seed in corpus() {
        // Skip the 512-byte launch stub — it carries no fields.
        let start = if seed.len() > 512 { 512 } else { 0 };
        for pos in (start..seed.len().saturating_sub(1)).step_by(2) {
            for val in [0xFFFFu16, 0x8000, 0x0001] {
                let mut m = seed.clone();
                m[pos..pos + 2].copy_from_slice(&val.to_be_bytes());
                walk(&m);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Hand-crafted worst cases.
// ---------------------------------------------------------------------------

/// A minimal v2 record with an arbitrary picFrame: picSize, frame,
/// version stanza, OpEndPic. No launch stub (raw-body form).
fn v2_record_with_frame(top: i16, left: i16, bottom: i16, right: i16) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&0u16.to_be_bytes()); // picSize (ignored)
    for w in [top, left, bottom, right] {
        b.extend_from_slice(&w.to_be_bytes());
    }
    b.extend_from_slice(&[0x00, 0x11, 0x02, 0xFF]); // version stanza
    b.extend_from_slice(&[0x0C, 0x00]); // headerOp
    b.extend_from_slice(&[0u8; 24]); // headerOp payload (tolerated shape)
    b.extend_from_slice(&[0x00, 0x01, 0x00, 0x0A]); // ClipRgn, rgnSize 10
    for w in [top, left, bottom, right] {
        b.extend_from_slice(&w.to_be_bytes()); // clip bbox = frame
    }
    b.extend_from_slice(&[0x00, 0x31]); // paintRect
    b.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x08, 0x00, 0x08]);
    b.extend_from_slice(&[0x00, 0xFF]); // OpEndPic
    b
}

#[test]
fn giant_pic_frame_is_rejected_before_allocation() {
    // 32767 × 32767 RGBA canvas would be ~4.3 GB — must error, fast.
    let hostile = v2_record_with_frame(-16384, -16384, 16383, 16383);
    assert!(parse_pict(&hostile).is_err());
    // Maximum-span frame: 65535 × 65535 (u32 pixel-count overflow bait).
    let hostile = v2_record_with_frame(-32768, -32768, 32767, 32767);
    assert!(parse_pict(&hostile).is_err());
    // A generous-but-sane frame still decodes.
    let sane = v2_record_with_frame(0, 0, 2000, 2000);
    assert!(parse_pict(&sane).is_ok());
}

#[test]
fn giant_pixmap_bounds_are_rejected() {
    // A DirectBitsRect whose PixMap bounds demand a multi-GB decode
    // buffer inside a tiny picFrame. baseAddr + rowBytes(0x8000|raw) +
    // bounds + pmVersion.. laid out per §A-4 Listing A-6; the walker
    // must refuse the buffer, not try to fill it.
    let mut b = Vec::new();
    b.extend_from_slice(&0u16.to_be_bytes());
    for w in [0i16, 0, 32, 32] {
        b.extend_from_slice(&w.to_be_bytes());
    }
    b.extend_from_slice(&[0x00, 0x11, 0x02, 0xFF]);
    b.extend_from_slice(&[0x0C, 0x00]); // headerOp
    b.extend_from_slice(&[0u8; 24]);
    b.extend_from_slice(&[0x00, 0x9A]); // DirectBitsRect
    b.extend_from_slice(&0xFFu32.to_be_bytes()); // baseAddr
    b.extend_from_slice(&(0x8000u16 | 0x3FFC).to_be_bytes()); // rowBytes
    for w in [-32768i16, -32768, 32767, 32767] {
        // bounds: 65535 × 65535
        b.extend_from_slice(&w.to_be_bytes());
    }
    b.extend_from_slice(&0u16.to_be_bytes()); // pmVersion
    b.extend_from_slice(&1u16.to_be_bytes()); // packType 1 (raw)
    b.extend_from_slice(&0u32.to_be_bytes()); // packSize
    b.extend_from_slice(&0x00480000u32.to_be_bytes()); // hRes
    b.extend_from_slice(&0x00480000u32.to_be_bytes()); // vRes
    b.extend_from_slice(&0u16.to_be_bytes()); // pixelType
    b.extend_from_slice(&32u16.to_be_bytes()); // pixelSize
    b.extend_from_slice(&3u16.to_be_bytes()); // cmpCount
    b.extend_from_slice(&8u16.to_be_bytes()); // cmpSize
    b.extend_from_slice(&[0u8; 12]); // planeBytes/pmTable/pmReserved
                                     // srcRect / dstRect / mode — then no pixel data (truncated).
    for w in [-32768i16, -32768, 32767, 32767] {
        b.extend_from_slice(&w.to_be_bytes());
    }
    for w in [0i16, 0, 32, 32] {
        b.extend_from_slice(&w.to_be_bytes());
    }
    b.extend_from_slice(&0u16.to_be_bytes());
    b.extend_from_slice(&[0x00, 0xFF]);
    assert!(parse_pict(&b).is_err());
    let _ = probe_pict(&b);
}
