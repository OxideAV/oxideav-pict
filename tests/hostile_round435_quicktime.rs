//! Round 435 — hostile-input robustness for the QuickTime payload
//! parsers.
//!
//! Every length field inside a `$8200` / `$8201` payload (`MatteSize`,
//! `MaskSize`, `idSize`, `dataSize`, the wrapper `Size` itself) is
//! attacker-controlled. The typed parsers must return `Err` — never
//! panic, never allocate beyond the `Size`-bounded payload actually
//! present — and the enclosing opcode walker must degrade to the
//! verbatim capture (`image = None`) rather than fail the picture.
//! This suite drives:
//!
//! * `parse_compressed_quicktime` / `parse_uncompressed_quicktime`
//!   over every truncation prefix of conforming payloads (matte +
//!   mask + extension variants included),
//! * seeded byte mutations of those payloads (deterministic xorshift
//!   PRNG, no dev-dependencies),
//! * systematic 32-bit length-field maxing (`0xFFFFFFFF` / `0x80000000`
//!   / off-by-one) at every long offset in the fixed parts and the
//!   `idSize` / `dataSize` slots,
//! * `parse_pict` + `probe_pict` over full PICTs whose QuickTime
//!   payloads carry each of the above corruptions — the picture must
//!   still parse (the `Size` field is authoritative per Inside
//!   Macintosh: QuickTime page 3-26).

use oxideav_pict::ops::PictBuilder;
use oxideav_pict::{
    build_direct_bits_rect_op, parse_compressed_quicktime, parse_pict,
    parse_uncompressed_quicktime, probe_pict, Fixed, ImageDescription, PackType,
    QuickTimeCompressed, QuickTimeMatte, QuickTimeUncompressed, Verb,
};

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

fn desc(codec: [u8; 4], data_size: u32, extension: Vec<u8>) -> ImageDescription {
    let mut name_raw = [0u8; 32];
    name_raw[0] = 3;
    name_raw[1..4].copy_from_slice(b"Fuz");
    ImageDescription {
        id_size: 86 + extension.len() as u32,
        codec,
        version: 1,
        revision_level: 1,
        vendor: *b"appl",
        temporal_quality: 0,
        spatial_quality: 0x0200,
        width: 32,
        height: 24,
        h_res: Fixed::SEVENTY_TWO_DPI,
        v_res: Fixed::SEVENTY_TWO_DPI,
        data_size,
        frame_count: 1,
        name_raw,
        depth: 24,
        clut_id: -1,
        extension,
    }
}

/// Conforming payload corpus: minimal $8200, matte + mask + extension
/// $8200, zero-dataSize $8200, minimal $8201, matte $8201.
fn corpus() -> Vec<(bool, Vec<u8>)> {
    let mut seeds = Vec::new();

    let plain = QuickTimeCompressed::still(desc(*b"jpeg", 0, Vec::new()), vec![0x5A; 40]);
    seeds.push((true, plain.to_payload_bytes().unwrap()));

    let mut rich = QuickTimeCompressed::still(desc(*b"rpza", 0, vec![0xAB; 12]), vec![0xC3; 25]);
    rich.matte = Some(QuickTimeMatte {
        description: desc(*b"raw ", 6, Vec::new()),
        data: vec![0x0F; 6],
    });
    let mut mask = Vec::new();
    mask.extend_from_slice(&10u16.to_be_bytes());
    for v in [0i16, 0, 8, 8] {
        mask.extend_from_slice(&v.to_be_bytes());
    }
    rich.mask_region = Some(mask);
    seeds.push((true, rich.to_payload_bytes().unwrap()));

    let mut unknown_size = QuickTimeCompressed::still(desc(*b"jpeg", 0, Vec::new()), vec![1; 9]);
    unknown_size.image_description.data_size = 0;
    seeds.push((true, unknown_size.to_payload_bytes().unwrap()));

    let red: Vec<u8> = [255u8, 0, 0, 255].repeat(16);
    let sub = build_direct_bits_rect_op(2, 2, 6, 6, &red, PackType::Raw).unwrap();
    let uncompressed = QuickTimeUncompressed::wrapping(&sub).unwrap();
    seeds.push((false, uncompressed.to_payload_bytes().unwrap()));

    let mut uncompressed_matte = QuickTimeUncompressed::wrapping(&sub).unwrap();
    uncompressed_matte.matte = Some(QuickTimeMatte {
        description: desc(*b"raw ", 4, Vec::new()),
        data: vec![0x77; 4],
    });
    seeds.push((false, uncompressed_matte.to_payload_bytes().unwrap()));

    seeds
}

/// Feed one corrupted payload through the standalone parser AND
/// through a full PICT via parse_pict + probe_pict. Nothing may
/// panic; the full-picture paths must succeed (degrading to the
/// verbatim capture) because `Size` still bounds the walk.
fn drive(compressed: bool, payload: &[u8]) {
    if compressed {
        let _ = parse_compressed_quicktime(payload);
    } else {
        let _ = parse_uncompressed_quicktime(payload);
    }
    let mut b = PictBuilder::new(0, 0, 16, 16);
    // A visible op alongside the QuickTime payload, so the walk also
    // proves it resumes cleanly past the (possibly corrupt) opcode.
    b.rect(Verb::Paint, 0, 0, 2, 2);
    if compressed {
        b.compressed_quicktime(payload).unwrap();
    } else {
        b.uncompressed_quicktime(payload).unwrap();
    }
    let bytes = b.finish();
    let img = parse_pict(&bytes).expect("Size-bounded QuickTime payload must never fail the walk");
    assert_eq!(img.quicktime.len(), 1);
    assert_eq!(img.quicktime[0].data, payload);
    let p = probe_pict(&bytes).expect("probe must survive any QuickTime interior");
    assert_eq!(p.quicktime.len(), 1);
    assert_eq!(p.quicktime[0].payload_len, payload.len());
}

#[test]
fn every_truncation_prefix_is_handled() {
    for (compressed, payload) in corpus() {
        for cut in 0..payload.len() {
            drive(compressed, &payload[..cut]);
        }
    }
}

#[test]
fn seeded_mutations_never_panic() {
    let mut rng = Rng(0x8200_8201_C0FF_EE00);
    for (compressed, payload) in corpus() {
        for _ in 0..400 {
            let mut m = payload.clone();
            for _ in 0..=rng.below(4) {
                let i = rng.below(m.len());
                m[i] ^= (rng.next() & 0xFF) as u8;
            }
            drive(compressed, &m);
        }
    }
}

#[test]
fn maxed_length_longs_are_rejected_not_allocated() {
    // Stamp hostile values over every 32-bit-aligned long in the
    // fixed parts plus the idSize/dataSize slots of each embedded
    // ImageDescription. The parsers must Err (or succeed harmlessly
    // when the slot wasn't a length) without over-allocating: every
    // read is bounded by the payload actually present.
    let hostile: [u32; 4] = [0xFFFF_FFFF, 0x8000_0000, 0x7FFF_FFFF, 0x0000_1000];
    for (compressed, payload) in corpus() {
        for off in (0..payload.len().saturating_sub(4)).step_by(2) {
            for v in hostile {
                let mut m = payload.clone();
                m[off..off + 4].copy_from_slice(&v.to_be_bytes());
                drive(compressed, &m);
            }
        }
    }
}

#[test]
fn wrapper_size_truncation_inside_stream_errors_cleanly() {
    // A $8200 whose Size long claims more bytes than the stream has:
    // the walker must surface InvalidData (truncated read), not
    // panic. Build a full PICT then cut it mid-payload.
    let payload = corpus().remove(0).1;
    let mut b = PictBuilder::new(0, 0, 16, 16);
    b.compressed_quicktime(&payload).unwrap();
    let bytes = b.finish();
    // Find a cut point inside the QuickTime payload: the last
    // payload.len() bytes precede the two-byte OpEndPic (+ possible
    // pad), so cutting 10 bytes before the end lands inside it.
    for cut in [bytes.len() - 10, bytes.len() - payload.len() / 2] {
        assert!(parse_pict(&bytes[..cut]).is_err());
        // probe reports a termination instead of panicking.
        let _ = probe_pict(&bytes[..cut]);
    }
}
