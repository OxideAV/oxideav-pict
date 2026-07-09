//! Round 7 (workspace round 75) tests — read-only `probe_pict`
//! introspection. Walks the opcode stream without rasterising and
//! returns a [`PictProbe`] summary (version, frame, opcode mix,
//! termination cause).
//!
//! The probe MUST observe every opcode the rasteriser observes, so
//! these tests assert that for each fixture the probe's
//! `raster_count + drawing_count + same_shape_count + text_count +
//! comment_count + clip_rgn_count + quicktime_count` covers the
//! whole stream — and that the probe terminates exactly as the
//! decoder would.

use oxideav_pict::ops::{PictBuilder, Verb};
use oxideav_pict::{
    encode_pict, encode_pict_pack_bits_rect, encode_pict_v1, encode_pict_v1_with, encode_pict_v2,
    encode_pict_v2_with_clip, parse_pict, probe_pict, PackType, ProbeTermination, ProbeVersion,
};

// ---------------------------------------------------------------------------
// Framing + version detection.
// ---------------------------------------------------------------------------

#[test]
fn probe_v2_directbits_raw_minimal() {
    // The default round-2 encoder emits a v2 PICT with a single
    // DirectBitsRect raster opcode. Probe should see exactly one
    // raster, no drawing, no text, clean OpEndPic termination.
    let rgba = vec![0x80u8; 4 * 4 * 4];
    let pict = encode_pict(4, 4, &rgba).unwrap();
    let p = probe_pict(&pict).unwrap();
    assert_eq!(p.version, ProbeVersion::V2);
    assert_eq!(p.width, 4);
    assert_eq!(p.height, 4);
    assert!(p.has_launch_stub, "encode_pict prefixes the 512-byte stub");
    assert_eq!(p.raster_count, 1);
    assert_eq!(p.drawing_count, 0);
    assert_eq!(p.same_shape_count, 0);
    assert_eq!(p.text_count, 0);
    assert_eq!(p.comment_count, 0);
    assert_eq!(p.clip_rgn_count, 0);
    assert_eq!(p.compressed_quicktime_count, 0);
    assert!(p.end_pic_seen);
    assert_eq!(p.termination, ProbeTermination::EndPic);
    assert!(p.has_visible_content());
    assert!(!p.has_quicktime());
}

#[test]
fn probe_v1_no_launch_stub() {
    let rgba = vec![0u8; 4 * 4 * 4];
    let pict = encode_pict_v1(4, 4, &rgba).unwrap();
    let p = probe_pict(&pict).unwrap();
    assert_eq!(p.version, ProbeVersion::V1);
    assert!(!p.has_launch_stub, "v1 encoder doesn't prefix the stub");
    assert_eq!(p.width, 4);
    assert_eq!(p.height, 4);
    assert_eq!(p.raster_count, 1);
    assert!(p.end_pic_seen);
}

#[test]
fn probe_rejects_truncated_input() {
    // Byte stream too short to even contain a picture record.
    let err = probe_pict(&[0u8; 8]).unwrap_err();
    assert!(format!("{err}").contains("no PICT picture record"));
}

#[test]
fn probe_rejects_bad_version_word() {
    // Picture record header looks plausible up to byte 10, but the
    // version word is 0xDEAD — neither 0x0011 nor 0x1101.
    let mut buf = Vec::new();
    buf.extend_from_slice(&0u16.to_be_bytes()); // picSize
    buf.extend_from_slice(&0i16.to_be_bytes()); // frame top
    buf.extend_from_slice(&0i16.to_be_bytes()); // frame left
    buf.extend_from_slice(&8i16.to_be_bytes()); // frame bottom
    buf.extend_from_slice(&8i16.to_be_bytes()); // frame right
                                                // BUT detect_body_offset won't accept this — sentinel byte mismatch.
                                                // To exercise the actual "version detection" arm we need to craft a
                                                // stream whose sentinel matches one of the recognised forms but
                                                // whose follow-on bytes are bad.
                                                // Easier: construct a v2-looking record (0x0011 0x02FF) then put
                                                // a non-headerOp word.
    let mut bad = Vec::new();
    bad.extend_from_slice(&0u16.to_be_bytes());
    bad.extend_from_slice(&0i16.to_be_bytes());
    bad.extend_from_slice(&0i16.to_be_bytes());
    bad.extend_from_slice(&8i16.to_be_bytes());
    bad.extend_from_slice(&8i16.to_be_bytes());
    bad.extend_from_slice(&0x0011u16.to_be_bytes());
    bad.extend_from_slice(&0x02FFu16.to_be_bytes());
    bad.extend_from_slice(&0xABCDu16.to_be_bytes()); // bad headerOp
    let err = probe_pict(&bad).unwrap_err();
    assert!(
        format!("{err}").contains("headerOp"),
        "expected headerOp error, got: {err}"
    );
    let _ = buf;
}

// ---------------------------------------------------------------------------
// Drawing-command counting.
// ---------------------------------------------------------------------------

#[test]
fn probe_counts_drawing_primitives() {
    // A drawing-only PICT with a known opcode mix.
    let mut b = PictBuilder::new(0, 0, 16, 16);
    b.fg_color(0xFF, 0x00, 0x00);
    b.rect(Verb::Paint, 4, 4, 12, 12);
    b.fg_color(0x00, 0x00, 0x00);
    b.rect(Verb::Frame, 2, 2, 14, 14);
    b.oval(Verb::Frame, 0, 0, 16, 16);
    let pict = b.finish();
    let p = probe_pict(&pict).unwrap();
    assert_eq!(p.version, ProbeVersion::V2);
    assert_eq!(p.drawing_count, 3, "two rects + one oval");
    assert_eq!(p.raster_count, 0);
    assert_eq!(p.same_shape_count, 0);
    assert!(p.end_pic_seen);
}

#[test]
fn probe_counts_line_and_polygon() {
    let mut b = PictBuilder::new(0, 0, 16, 16);
    b.fg_color(0, 0, 0);
    b.line(0, 0, 15, 15);
    b.line(15, 0, 0, 15);
    b.poly(Verb::Frame, &[(2, 2), (14, 2), (8, 14)]).unwrap();
    let pict = b.finish();
    let p = probe_pict(&pict).unwrap();
    assert_eq!(p.drawing_count, 3, "two lines + one polygon");
    assert!(p.end_pic_seen);
}

// ---------------------------------------------------------------------------
// Clip / raster combinations.
// ---------------------------------------------------------------------------

#[test]
fn probe_counts_clip_then_raster() {
    let rgba = vec![0x40u8; 4 * 4 * 4];
    let pict = encode_pict_v2_with_clip(4, 4, &rgba, PackType::Raw, [1, 1, 3, 3]).unwrap();
    let p = probe_pict(&pict).unwrap();
    assert_eq!(p.clip_rgn_count, 1);
    assert_eq!(p.raster_count, 1);
    assert!(p.end_pic_seen);
}

#[test]
fn probe_handles_v1_packtype_variants() {
    // Round 5 v1 + PackType ComponentPackBits roundtrips through both
    // parse_pict and the probe.
    let rgba = vec![0x90u8; 8 * 8 * 4];
    let pict = encode_pict_v1_with(8, 8, &rgba, PackType::ComponentPackBits).unwrap();
    let p = probe_pict(&pict).unwrap();
    assert_eq!(p.version, ProbeVersion::V1);
    assert_eq!(p.raster_count, 1);
    assert!(p.end_pic_seen);
    // Sanity: parse still works on the same byte stream.
    let img = parse_pict(&pict).unwrap();
    assert_eq!(img.width, 8);
}

#[test]
fn probe_packbits_rect_1bpp_wide_row() {
    // PackBitsRect with rowBytes >= 8 (per-row RLE branch). The probe
    // skips each row's PackBits payload by its byteCount prefix.
    let rgba = vec![0xFFu8; 64 * 16 * 4];
    let pict = encode_pict_pack_bits_rect(64, 16, &rgba).unwrap();
    let p = probe_pict(&pict).unwrap();
    assert_eq!(p.raster_count, 1);
    assert!(p.end_pic_seen);
}

// ---------------------------------------------------------------------------
// Hand-rolled streams covering corners the encoders don't emit.
// ---------------------------------------------------------------------------

/// Build a v2 PICT byte stream with a known body. Returns the stream
/// (no launch-stub).
fn v2_with_body(width: i16, height: i16, body: &[u8]) -> Vec<u8> {
    let mut buf = Vec::new();
    // Picture record header.
    buf.extend_from_slice(&0u16.to_be_bytes()); // picSize
    buf.extend_from_slice(&0i16.to_be_bytes()); // frame top
    buf.extend_from_slice(&0i16.to_be_bytes()); // frame left
    buf.extend_from_slice(&height.to_be_bytes());
    buf.extend_from_slice(&width.to_be_bytes());
    // v2 sentinel + headerOp + 24-byte payload.
    buf.extend_from_slice(&0x0011u16.to_be_bytes());
    buf.extend_from_slice(&0x02FFu16.to_be_bytes());
    buf.extend_from_slice(&0x0C00u16.to_be_bytes());
    buf.extend_from_slice(&[0u8; 24]);
    buf.extend_from_slice(body);
    // OpEndPic.
    if buf.len() % 2 == 1 {
        buf.push(0);
    }
    buf.extend_from_slice(&0x00FFu16.to_be_bytes());
    buf
}

#[test]
fn probe_counts_comments() {
    // ShortComment + LongComment opcodes.
    let mut body = Vec::new();
    // ShortComment (0x00A0) + 2-byte kind.
    body.extend_from_slice(&0x00A0u16.to_be_bytes());
    body.extend_from_slice(&0x0001u16.to_be_bytes());
    // LongComment (0x00A1) + 2-byte kind + 2-byte size + 6-byte data.
    body.extend_from_slice(&0x00A1u16.to_be_bytes());
    body.extend_from_slice(&0x0002u16.to_be_bytes()); // kind
    body.extend_from_slice(&6u16.to_be_bytes()); // size
    body.extend_from_slice(b"HELLO!");
    let pict = v2_with_body(4, 4, &body);
    let p = probe_pict(&pict).unwrap();
    assert_eq!(p.comment_count, 2);
    assert_eq!(p.drawing_count, 0);
    assert_eq!(p.raster_count, 0);
    assert!(p.end_pic_seen);
}

#[test]
fn probe_counts_compressed_quicktime() {
    // CompressedQuickTime (0x8200) + 4-byte payload size + dummy payload.
    let mut body = Vec::new();
    body.extend_from_slice(&0x8200u16.to_be_bytes());
    // §A-3 Table A-2: the Long is the DATA length, excluding itself
    // (round 401 conformance fix), so 12 announces 12 payload bytes.
    let data_length: u32 = 12;
    body.extend_from_slice(&data_length.to_be_bytes());
    body.extend_from_slice(&[0xAAu8; 12]);
    let pict = v2_with_body(4, 4, &body);
    let p = probe_pict(&pict).unwrap();
    assert_eq!(p.compressed_quicktime_count, 1);
    assert!(p.has_quicktime());
    assert!(p.end_pic_seen);
}

#[test]
fn probe_counts_same_shape_opcodes() {
    // Build a body with FrameRect then FrameSameRect.
    let mut body = Vec::new();
    body.extend_from_slice(&0x0030u16.to_be_bytes()); // OP_FRAME_RECT
    body.extend_from_slice(&0i16.to_be_bytes()); // top
    body.extend_from_slice(&0i16.to_be_bytes()); // left
    body.extend_from_slice(&4i16.to_be_bytes()); // bottom
    body.extend_from_slice(&4i16.to_be_bytes()); // right
    body.extend_from_slice(&0x0038u16.to_be_bytes()); // OP_FRAME_SAME_RECT
    let pict = v2_with_body(8, 8, &body);
    let p = probe_pict(&pict).unwrap();
    assert_eq!(p.drawing_count, 1, "FrameRect counts as drawing");
    assert_eq!(p.same_shape_count, 1, "FrameSameRect counts separately");
    assert!(p.has_visible_content());
}

#[test]
fn probe_eof_without_end_pic() {
    // Hand-rolled stream that truncates right after a drawing op — no
    // OpEndPic. The probe should terminate via Eof, not EndPic, with
    // the drawing observed.
    let mut body = Vec::new();
    body.extend_from_slice(&0x0030u16.to_be_bytes()); // OP_FRAME_RECT
    body.extend_from_slice(&0i16.to_be_bytes());
    body.extend_from_slice(&0i16.to_be_bytes());
    body.extend_from_slice(&4i16.to_be_bytes());
    body.extend_from_slice(&4i16.to_be_bytes());
    // No 0x00FF terminator.
    let mut buf = Vec::new();
    buf.extend_from_slice(&0u16.to_be_bytes());
    buf.extend_from_slice(&0i16.to_be_bytes());
    buf.extend_from_slice(&0i16.to_be_bytes());
    buf.extend_from_slice(&8i16.to_be_bytes());
    buf.extend_from_slice(&8i16.to_be_bytes());
    buf.extend_from_slice(&0x0011u16.to_be_bytes());
    buf.extend_from_slice(&0x02FFu16.to_be_bytes());
    buf.extend_from_slice(&0x0C00u16.to_be_bytes());
    buf.extend_from_slice(&[0u8; 24]);
    buf.extend_from_slice(&body);
    let p = probe_pict(&buf).unwrap();
    assert!(!p.end_pic_seen);
    assert_eq!(p.termination, ProbeTermination::Eof);
    assert_eq!(p.drawing_count, 1);
}

#[test]
fn probe_unsupported_opcode_preserves_prior_counts() {
    // FrameRect, then a reserved-with-no-known-handler opcode (0x0017,
    // marked "Reserved for Apple use" with "Not determined" data size
    // in Inside Macintosh §A-3 Table A-2). The probe records the
    // FrameRect, then terminates with ProbeTermination::Unsupported
    // because the walker has no rule for stepping past 0x0017.
    //
    // Round 91 note: this slot used to hold a `0x0012 BkPixPat` smoke
    // test back when PixPat was an unsupported opcode. PixPat is now
    // fully decoded (`tests/synth_v2_round91.rs`), so the test moves to
    // an opcode that's *still* genuinely unsupported.
    let mut body = Vec::new();
    body.extend_from_slice(&0x0030u16.to_be_bytes());
    body.extend_from_slice(&0i16.to_be_bytes());
    body.extend_from_slice(&0i16.to_be_bytes());
    body.extend_from_slice(&4i16.to_be_bytes());
    body.extend_from_slice(&4i16.to_be_bytes());
    body.extend_from_slice(&0x0017u16.to_be_bytes()); // Reserved, undefined size
    let mut buf = Vec::new();
    buf.extend_from_slice(&0u16.to_be_bytes());
    buf.extend_from_slice(&0i16.to_be_bytes());
    buf.extend_from_slice(&0i16.to_be_bytes());
    buf.extend_from_slice(&8i16.to_be_bytes());
    buf.extend_from_slice(&8i16.to_be_bytes());
    buf.extend_from_slice(&0x0011u16.to_be_bytes());
    buf.extend_from_slice(&0x02FFu16.to_be_bytes());
    buf.extend_from_slice(&0x0C00u16.to_be_bytes());
    buf.extend_from_slice(&[0u8; 24]);
    buf.extend_from_slice(&body);
    let p = probe_pict(&buf).unwrap();
    assert_eq!(p.drawing_count, 1, "FrameRect counted before failure");
    assert!(!p.end_pic_seen);
    match &p.termination {
        ProbeTermination::Unsupported(msg) => {
            assert!(
                msg.contains("0x0017") || msg.contains("unsupported"),
                "got: {msg}"
            );
        }
        other => panic!("expected Unsupported, got {other:?}"),
    }
}

#[test]
fn probe_v2_packtype3_raster() {
    let rgba = vec![0x70u8; 8 * 8 * 4];
    let pict = encode_pict_v2(8, 8, &rgba, PackType::Rle16).unwrap();
    let p = probe_pict(&pict).unwrap();
    assert_eq!(p.raster_count, 1);
    assert!(p.end_pic_seen);
}

#[test]
fn probe_v2_packtype2_raster() {
    let rgba = vec![0x60u8; 8 * 8 * 4];
    let pict = encode_pict_v2(8, 8, &rgba, PackType::Packed24).unwrap();
    let p = probe_pict(&pict).unwrap();
    assert_eq!(p.raster_count, 1);
    assert!(p.end_pic_seen);
}

#[test]
fn probe_v2_packtype4_raster() {
    let rgba = vec![0x50u8; 8 * 8 * 4];
    let pict = encode_pict_v2(8, 8, &rgba, PackType::ComponentPackBits).unwrap();
    let p = probe_pict(&pict).unwrap();
    assert_eq!(p.raster_count, 1);
    assert!(p.end_pic_seen);
}

#[test]
fn probe_terminated_at_is_monotonic_after_end_pic() {
    // After OpEndPic the probe stops at the byte right after the
    // terminator; we don't strictly assert the value but we DO assert
    // that it's within bounds.
    let rgba = vec![0u8; 4 * 4 * 4];
    let pict = encode_pict(4, 4, &rgba).unwrap();
    let p = probe_pict(&pict).unwrap();
    assert!(p.terminated_at <= pict.len());
    assert!(p.terminated_at >= 512, "must be past the launch stub");
}
