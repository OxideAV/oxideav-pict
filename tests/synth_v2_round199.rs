//! Round 199 tests — §A-3 reserved-opcode skip table.
//!
//! Inside Macintosh: Imaging With QuickDraw §A-3 (Table A-2) gives a
//! published payload size for every "Reserved for Apple use" entry in
//! the v2 opcode space. Prior to this round the decoder + probe
//! treated those reserved opcodes as fatal "unknown / unsupported v2
//! opcode" errors, even though §A-3 spells out exactly how many
//! payload bytes to walk past. That bit the rasteriser on any PICT
//! that embedded a private Apple extension (e.g. an unused
//! 0x00B0..=0x00CF zero-payload reserved opcode, or a third-party
//! emitter that abused the `0x0100..=0x7FFF` band as `2 × nn`-byte
//! comment metadata).
//!
//! The synth helpers in this file emit a minimal v2 picture-record
//! framing (`picSize / picFrame / 0x0011 0x02FF / 0x0C00 + 24-byte
//! payload`) then sprinkle the reserved opcode under test ahead of the
//! `OpEndPic 0x00FF`. The picture frame is 1×1 so the decoder
//! materialises a single-pixel canvas — the assertion is that the
//! decode finishes without surfacing `PictError::Unsupported` AND that
//! the probe agrees on the reserved-opcode count.
//!
//! All payload sizes asserted here trace back to §A-3 Table A-2 (book
//! page A-13) and the page A-5 Note ("opcode `$nnXX` carries
//! `2 × nn` bytes of data"). No external implementation consulted.

use oxideav_pict::{parse_pict, probe_pict, ProbeTermination};

// ---------------------------------------------------------------------------
// Bytestream builders.
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

/// Minimal v2 picture-record framing. Returns the byte vector with
/// the header already emitted; caller appends opcodes then `OpEndPic`.
fn v2_header() -> Vec<u8> {
    let mut out = Vec::new();
    // picSize (ignored by the decoder).
    put_u16(&mut out, 0);
    // picFrame: 1 × 1, top-left at origin.
    put_i16(&mut out, 0);
    put_i16(&mut out, 0);
    put_i16(&mut out, 1);
    put_i16(&mut out, 1);
    // v2 sentinel + headerOp 0x0C00 + 24-byte payload.
    put_u16(&mut out, 0x0011);
    put_u16(&mut out, 0x02FF);
    put_u16(&mut out, 0x0C00);
    out.extend_from_slice(&[0u8; 24]);
    out
}

/// Append a one-pixel `paintRect` then `OpEndPic 0x00FF` so the
/// decoder finishes with a non-empty canvas (`PictError::NoRaster` is
/// raised when no opcode actually paints). The rect covers the 1×1
/// picture frame so the resulting image is a single foreground pixel
/// regardless of which reserved-opcode was sprinkled ahead of it.
fn close_pict(out: &mut Vec<u8>) {
    // paintRect (0x0031) — rect = (top, left, bottom, right) = (0,0,1,1).
    put_u16(out, 0x0031);
    put_i16(out, 0);
    put_i16(out, 0);
    put_i16(out, 1);
    put_i16(out, 1);
    // OpEndPic.
    put_u16(out, 0x00FF);
}

/// Build a v2 PICT carrying one reserved opcode (`opcode`) with a
/// fixed payload of `payload` bytes (no length prefix).
fn build_v2_reserved_fixed(opcode: u16, payload: &[u8]) -> Vec<u8> {
    let mut out = v2_header();
    put_u16(&mut out, opcode);
    out.extend_from_slice(payload);
    if out.len() % 2 != 0 {
        out.push(0);
    }
    close_pict(&mut out);
    out
}

/// Build a v2 PICT carrying one reserved opcode (`opcode`) with a
/// u16 length-prefixed payload (so total = `2 + payload.len()`).
fn build_v2_reserved_u16_prefixed(opcode: u16, payload: &[u8]) -> Vec<u8> {
    let mut out = v2_header();
    put_u16(&mut out, opcode);
    put_u16(&mut out, payload.len() as u16);
    out.extend_from_slice(payload);
    if out.len() % 2 != 0 {
        out.push(0);
    }
    close_pict(&mut out);
    out
}

/// Build a v2 PICT carrying one reserved opcode (`opcode`) with a
/// u32 length-prefixed payload (so total = `4 + payload.len()`).
fn build_v2_reserved_u32_prefixed(opcode: u16, payload: &[u8]) -> Vec<u8> {
    let mut out = v2_header();
    put_u16(&mut out, opcode);
    put_u32(&mut out, payload.len() as u32);
    out.extend_from_slice(payload);
    if out.len() % 2 != 0 {
        out.push(0);
    }
    close_pict(&mut out);
    out
}

/// Build a v2 PICT carrying one reserved poly-shaped opcode
/// (`opcode`) with a 16-bit poly-size word that *includes itself*
/// (so total bytes after opcode = `poly_size`).
fn build_v2_reserved_poly_sized(opcode: u16, payload: &[u8]) -> Vec<u8> {
    let mut out = v2_header();
    put_u16(&mut out, opcode);
    let poly_size = 2 + payload.len() as u16;
    put_u16(&mut out, poly_size);
    out.extend_from_slice(payload);
    if out.len() % 2 != 0 {
        out.push(0);
    }
    close_pict(&mut out);
    out
}

/// Like `build_v2_reserved_poly_sized` but for the §A-3
/// "Region size" reserved range (`0x0085..=0x0087`).
fn build_v2_reserved_rgn_sized(opcode: u16, payload: &[u8]) -> Vec<u8> {
    build_v2_reserved_poly_sized(opcode, payload)
}

// ---------------------------------------------------------------------------
// Decoder assertions.
// ---------------------------------------------------------------------------

#[test]
fn decode_skips_reserved_0024_u16_prefixed() {
    // 0x0024..=0x0027: "Data length (Integer), data".
    let bytes = build_v2_reserved_u16_prefixed(0x0024, b"hello-apple");
    let img = parse_pict(&bytes).expect("reserved 0x0024 should be skipped");
    assert_eq!(img.width, 1);
    assert_eq!(img.height, 1);
}

#[test]
fn decode_skips_reserved_0027_u16_prefixed_empty() {
    let bytes = build_v2_reserved_u16_prefixed(0x0027, b"");
    parse_pict(&bytes).expect("reserved 0x0027 empty payload should be skipped");
}

#[test]
fn decode_skips_reserved_002f_u16_prefixed() {
    let bytes = build_v2_reserved_u16_prefixed(0x002F, &[0xAA; 8]);
    parse_pict(&bytes).expect("reserved 0x002F should be skipped");
}

#[test]
fn decode_skips_reserved_0035_fixed_8() {
    // 0x0035..=0x0037 — three 8-byte reserved slots between fillRect
    // (0x0034) and frameSameRect (0x0038).
    let bytes = build_v2_reserved_fixed(0x0036, &[0x00; 8]);
    parse_pict(&bytes).expect("reserved 0x0036 fixed-8 should be skipped");
}

#[test]
fn decode_skips_reserved_003d_zero_payload() {
    // 0x003D..=0x003F — three 0-byte reserved slots.
    let bytes = build_v2_reserved_fixed(0x003E, &[]);
    parse_pict(&bytes).expect("reserved 0x003E zero-payload should be skipped");
}

#[test]
fn decode_skips_reserved_0066_fixed_12() {
    // 0x0065..=0x0067 — three 12-byte reserved slots (rect+arc shaped).
    let bytes = build_v2_reserved_fixed(0x0066, &[0x42; 12]);
    parse_pict(&bytes).expect("reserved 0x0066 fixed-12 should be skipped");
}

#[test]
fn decode_skips_reserved_0076_poly_sized() {
    // 0x0075..=0x0077 — poly-shaped: 2-byte polySize-including-itself
    // word + (polySize - 2) bytes of opaque polygon payload.
    let bytes = build_v2_reserved_poly_sized(0x0076, &[0u8; 18]); // 18 = 8 bbox + 4 verts
    parse_pict(&bytes).expect("reserved 0x0076 poly-sized should be skipped");
}

#[test]
fn decode_skips_reserved_0086_rgn_sized() {
    // 0x0085..=0x0087 — region-shaped: same size-includes-itself rule.
    let bytes = build_v2_reserved_rgn_sized(0x0086, &[0u8; 8]); // 8 = bbox only
    parse_pict(&bytes).expect("reserved 0x0086 rgn-sized should be skipped");
}

#[test]
fn decode_skips_reserved_0093_u16_prefixed() {
    // 0x0092..=0x0097 — between BitsRgn (0x0091) and PackBitsRect (0x0098).
    let bytes = build_v2_reserved_u16_prefixed(0x0093, &[0x55; 64]);
    parse_pict(&bytes).expect("reserved 0x0093 should be skipped");
}

#[test]
fn decode_skips_reserved_009d_u16_prefixed() {
    let bytes = build_v2_reserved_u16_prefixed(0x009D, b"private");
    parse_pict(&bytes).expect("reserved 0x009D should be skipped");
}

#[test]
fn decode_skips_reserved_00a5_u16_prefixed() {
    // 0x00A2..=0x00AF — between LongComment (0x00A1) and the 0x00B0
    // zero-payload range.
    let bytes = build_v2_reserved_u16_prefixed(0x00A5, &[0x77; 128]);
    parse_pict(&bytes).expect("reserved 0x00A5 should be skipped");
}

#[test]
fn decode_skips_reserved_00b8_zero_payload() {
    // 0x00B0..=0x00CF — thirty-two 0-byte reserved slots.
    let bytes = build_v2_reserved_fixed(0x00B8, &[]);
    parse_pict(&bytes).expect("reserved 0x00B8 zero-payload should be skipped");
}

#[test]
fn decode_skips_reserved_00d3_u32_prefixed() {
    // 0x00D0..=0x00FE — forty-six u32-prefixed reserved slots between
    // the zero-payload range and OpEndPic (0x00FF). 0x00D3 is a typical
    // pick.
    let bytes = build_v2_reserved_u32_prefixed(0x00D3, &[0x33; 16]);
    parse_pict(&bytes).expect("reserved 0x00D3 u32-prefixed should be skipped");
}

#[test]
fn decode_skips_reserved_0100_2byte_fixed() {
    // 0x0100..=0x01FF — the long upper band starts with 2-byte payload
    // entries.
    let bytes = build_v2_reserved_fixed(0x0123, &[0x12, 0x34]);
    parse_pict(&bytes).expect("reserved 0x0123 fixed-2 should be skipped");
}

#[test]
fn decode_skips_reserved_0200_4byte_fixed() {
    // 0x0200 — first u16 of the next band, 4-byte payload.
    let bytes = build_v2_reserved_fixed(0x0200, &[0x12, 0x34, 0x56, 0x78]);
    parse_pict(&bytes).expect("reserved 0x0200 fixed-4 should be skipped");
}

#[test]
fn decode_skips_reserved_0bff_22byte_fixed() {
    // §A-3 explicitly tabulates 0x0BFF at 22-byte payload.
    let bytes = build_v2_reserved_fixed(0x0BFF, &[0xAA; 22]);
    parse_pict(&bytes).expect("reserved 0x0BFF fixed-22 should be skipped");
}

#[test]
fn decode_skips_reserved_0c01_24byte_fixed() {
    // 0x0C00 is the HeaderOp itself (consumed by the version stanza);
    // 0x0C01 sits in the same 0x0Cxx band with 24-byte payload.
    let bytes = build_v2_reserved_fixed(0x0C01, &[0xCC; 24]);
    parse_pict(&bytes).expect("reserved 0x0C01 fixed-24 should be skipped");
}

#[test]
fn decode_skips_reserved_7fff_254byte_fixed() {
    // Boundary row: 0x7F00..=0x7FFF carry 254 bytes (§A-3 explicit).
    let bytes = build_v2_reserved_fixed(0x7FFF, &[0xEE; 254]);
    parse_pict(&bytes).expect("reserved 0x7FFF fixed-254 should be skipped");
}

#[test]
fn decode_skips_reserved_8042_zero_payload() {
    // 0x8000..=0x80FF — 0-byte reserved.
    let bytes = build_v2_reserved_fixed(0x8042, &[]);
    parse_pict(&bytes).expect("reserved 0x8042 zero-payload should be skipped");
}

#[test]
fn decode_skips_reserved_8155_u32_prefixed() {
    // 0x8100..=0x81FF — u32 length-prefixed.
    let bytes = build_v2_reserved_u32_prefixed(0x8155, b"some-private-payload");
    parse_pict(&bytes).expect("reserved 0x8155 u32-prefixed should be skipped");
}

#[test]
fn decode_skips_reserved_ffff_u32_prefixed() {
    // The 0xFFFF row in §A-3 closes the table: u32 length-prefixed.
    let bytes = build_v2_reserved_u32_prefixed(0xFFFF, &[0x99; 32]);
    parse_pict(&bytes).expect("reserved 0xFFFF u32-prefixed should be skipped");
}

// ---------------------------------------------------------------------------
// Multiple reserved opcodes in the same PICT — and that subsequent
// rendering opcodes still flow through.
// ---------------------------------------------------------------------------

#[test]
fn decode_skips_multiple_reserved_opcodes_followed_by_endpic() {
    let mut out = v2_header();
    // Sprinkle four different reserved opcodes from different §A-3
    // sub-ranges so we exercise the dispatcher (fixed, u16-prefixed,
    // u32-prefixed, fixed `2 × nn`).
    put_u16(&mut out, 0x00B5); // fixed 0
    put_u16(&mut out, 0x0025); // u16-prefixed
    put_u16(&mut out, 0);
    put_u16(&mut out, 0x00D7); // u32-prefixed
    put_u32(&mut out, 4);
    out.extend_from_slice(&[0xAB, 0xCD, 0xEF, 0x12]);
    put_u16(&mut out, 0x0200); // fixed 4
    out.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
    close_pict(&mut out);
    parse_pict(&out).expect("multiple reserved opcodes should all be skipped");
}

// ---------------------------------------------------------------------------
// Probe assertions: `reserved_op_count` and clean `EndPic` termination.
// ---------------------------------------------------------------------------

#[test]
fn probe_counts_reserved_opcodes() {
    let mut out = v2_header();
    // Five reserved opcodes from §A-3 ranges with different shapes.
    put_u16(&mut out, 0x00B0); // fixed 0
    put_u16(&mut out, 0x00C7); // fixed 0
    put_u16(&mut out, 0x0026); // u16-prefixed empty
    put_u16(&mut out, 0);
    put_u16(&mut out, 0x009E); // u16-prefixed empty
    put_u16(&mut out, 0);
    put_u16(&mut out, 0x0BFF); // fixed 22
    out.extend_from_slice(&[0; 22]);
    close_pict(&mut out);

    let probe = probe_pict(&out).expect("probe should succeed");
    assert_eq!(probe.reserved_op_count, 5);
    assert_eq!(probe.termination, ProbeTermination::EndPic);
    // `close_pict` emits a `paintRect 0x0031` so drawing_count is 1
    // (this confirms the walker continues PAST the reserved opcodes,
    // not just past the first one).
    assert_eq!(probe.drawing_count, 1);
    assert_eq!(probe.raster_count, 0);
    assert!(probe.end_pic_seen);
}

#[test]
fn probe_termination_unchanged_when_no_reserved_opcodes() {
    let mut out = v2_header();
    close_pict(&mut out); // paintRect + OpEndPic
    let probe = probe_pict(&out).expect("probe should succeed");
    assert_eq!(probe.reserved_op_count, 0);
    assert_eq!(probe.termination, ProbeTermination::EndPic);
    assert_eq!(probe.drawing_count, 1);
}

// ---------------------------------------------------------------------------
// "Not determined" range still fails — those three opcodes (0x0017..=
// 0x0019) carry no published payload size, so the decoder + probe
// continue to refuse them.
// ---------------------------------------------------------------------------

#[test]
fn decode_rejects_not_determined_opcode_0018() {
    // 0x0017..=0x0019 are §A-3 "Not determined" — the spec leaves
    // their payload size unspecified, so we must surface this as a
    // hard `Unsupported` error rather than silently mis-skip.
    let mut out = v2_header();
    put_u16(&mut out, 0x0018);
    close_pict(&mut out);
    let err = parse_pict(&out).expect_err("0x0018 must remain unsupported");
    let msg = format!("{err}");
    assert!(
        msg.contains("0x0018"),
        "error should mention the offending opcode, got: {msg}"
    );
}

#[test]
fn probe_terminates_on_not_determined_opcode_0017() {
    let mut out = v2_header();
    put_u16(&mut out, 0x0017);
    close_pict(&mut out);
    let probe = probe_pict(&out).expect("probe should still succeed (framing OK)");
    match probe.termination {
        ProbeTermination::Unsupported(msg) => assert!(
            msg.contains("0x0017"),
            "termination msg should mention 0x0017, got: {msg}"
        ),
        other => panic!("expected Unsupported termination, got {other:?}"),
    }
    // reserved_op_count should NOT count the not-determined opcode —
    // we never made it past dispatch.
    assert_eq!(probe.reserved_op_count, 0);
}

// ---------------------------------------------------------------------------
// Truncation inside a reserved payload should surface as InvalidData,
// not silently terminate. (Round-1 behaviour would have been
// "Unsupported"; round-199 routes through `Reader::skip` which is
// `InvalidData("truncated …")`.)
// ---------------------------------------------------------------------------

#[test]
fn decode_truncated_reserved_payload_is_invalid_data() {
    // 0x00D0..=0x00FE — u32-prefixed reserved. Build a stream that
    // declares 100 bytes of payload but only supplies 4.
    let mut out = v2_header();
    put_u16(&mut out, 0x00D1);
    put_u32(&mut out, 100);
    out.extend_from_slice(&[0; 4]);
    // No OpEndPic; we want the reserved-payload skip to fail.
    let err = parse_pict(&out).expect_err("truncated reserved payload must error");
    let msg = format!("{err}");
    assert!(
        msg.contains("truncated"),
        "should surface truncation, got: {msg}"
    );
}
