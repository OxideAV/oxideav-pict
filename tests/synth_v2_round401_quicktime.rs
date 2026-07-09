//! Round 401 — QuickTime opcode payload capture + emission.
//!
//! `CompressedQuickTime $8200` / `UncompressedQuickTime $8201` carry a
//! `Data length (Long)` followed by `data length` opaque bytes (Inside
//! Macintosh: Imaging With QuickDraw §A-3 Table A-2, total additional
//! data `4 + data length` — the length word **excludes itself**;
//! round 401 fixes the previous self-inclusive reading, which
//! under-walked conforming streams by 4 bytes). The decoder now
//! captures the payload verbatim into [`PictImage::quicktime`] instead
//! of discarding it, and `build_compressed_quicktime` /
//! `build_uncompressed_quicktime` close the emission gap.

use oxideav_pict::ops::PictBuilder;
use oxideav_pict::{
    build_compressed_quicktime, build_uncompressed_quicktime, parse_pict, probe_pict,
    ProbeTermination, Verb,
};

// ---------------------------------------------------------------------------
// Wire layout: length word excludes itself.
// ---------------------------------------------------------------------------

#[test]
fn quicktime_builders_pin_the_wire_layout() {
    assert_eq!(
        build_compressed_quicktime(&[0xDE, 0xAD]).unwrap(),
        vec![0x82, 0x00, 0x00, 0x00, 0x00, 0x02, 0xDE, 0xAD],
    );
    assert_eq!(
        build_uncompressed_quicktime(&[]).unwrap(),
        vec![0x82, 0x01, 0x00, 0x00, 0x00, 0x00],
    );
}

// ---------------------------------------------------------------------------
// Round-trip: emitted payloads come back verbatim, in stream order,
// and the walker resumes cleanly after each (odd-length payload
// exercises the v2 word-alignment pad).
// ---------------------------------------------------------------------------

#[test]
fn quicktime_payloads_round_trip_verbatim() {
    let jpeg_ish: Vec<u8> = (0..301u32).map(|i| (i * 13) as u8).collect(); // odd length
    let raw: Vec<u8> = vec![0x42; 64];

    let mut b = PictBuilder::new(0, 0, 16, 16);
    b.compressed_quicktime(&jpeg_ish).unwrap();
    b.rect(Verb::Paint, 2, 2, 6, 6); // walker must resume after QT
    b.uncompressed_quicktime(&raw).unwrap();
    let bytes = b.finish();

    let img = parse_pict(&bytes).unwrap();
    assert_eq!(img.quicktime.len(), 2);
    assert!(img.quicktime[0].compressed);
    assert_eq!(img.quicktime[0].data, jpeg_ish);
    assert!(!img.quicktime[1].compressed);
    assert_eq!(img.quicktime[1].data, raw);
    // The paint between the two QT opcodes really landed.
    let off = (3 * 16 + 3) * 4;
    assert_eq!(&img.data[off..off + 4], &[0, 0, 0, 255]);

    let p = probe_pict(&bytes).unwrap();
    assert_eq!(p.compressed_quicktime_count, 1);
    assert_eq!(p.uncompressed_quicktime_count, 1);
    assert!(p.has_quicktime());
    assert!(p.end_pic_seen);
}

// ---------------------------------------------------------------------------
// A truncated payload (hostile length) errors instead of wedging.
// ---------------------------------------------------------------------------

#[test]
fn truncated_quicktime_payload_errors() {
    let mut b = PictBuilder::new(0, 0, 16, 16);
    b.rect(Verb::Paint, 2, 2, 6, 6);
    let mut bytes = b.finish();
    // Splice a QT opcode announcing 1000 bytes but carrying none,
    // right before the OpEndPic word.
    let end = bytes.len() - 2;
    bytes.splice(end..end, [0x82, 0x00, 0x00, 0x00, 0x03, 0xE8]);
    assert!(parse_pict(&bytes).is_err());
    // Probe's contract is to survive malformed streams and report the
    // reason instead of erroring.
    let p = probe_pict(&bytes).unwrap();
    assert!(matches!(p.termination, ProbeTermination::Invalid(_)));
    assert!(!p.end_pic_seen);
}
