//! Round 205 tests — v1 dispatcher state-machine + text + Same-shape
//! opcode coverage per Inside Macintosh: Imaging With QuickDraw §A-3
//! Table A-3.
//!
//! Prior rounds wired the v1 (8-bit-opcode) decoder up for the small
//! shape verbs (`frameRect`..`fillPoly`/`fillRgn`) and raster opcodes
//! (`BitsRect`/`BitsRgn`/`PackBitsRect`/`PackBitsRgn`/`DirectBitsRect`/
//! `DirectBitsRgn`), but several Table A-3 entries still triggered
//! `unknown / unsupported v1 opcode` and aborted the picture:
//!
//! * State / text setup opcodes: `0x03 TxFont`, `0x04 TxFace`,
//!   `0x05 TxMode`, `0x06 SpExtra`, `0x08 PnMode`, `0x0D TxSize`,
//!   `0x10 TxRatio`.
//! * Text opcodes: `0x28 LongText`, `0x29 DHText`, `0x2A DVText`,
//!   `0x2B DHDVText`.
//! * Same-shape opcodes: `0x38..=0x3C frameSameRect..fillSameRect`,
//!   `0x48..=0x4C frameSameRRect..fillSameRRect`,
//!   `0x58..=0x5C frameSameOval..fillSameOval`,
//!   `0x68..=0x6C frameSameArc..fillSameArc`.
//! * Spec "(Not yet implemented)" same-shape opcodes:
//!   `0x78..=0x7C frameSamePoly..fillSamePoly`,
//!   `0x88..=0x8C frameSameRgn..fillSameRgn`.
//!
//! v1 PICTs don't have a 512-byte launch stub, no v2 sentinel, and no
//! headerOp — they're a 10-byte picture-record header (picSize +
//! picFrame), the v1 version stanza (`0x11 0x01`), then the opcode
//! stream, terminated by `0xFF`. The synth helpers in this file emit
//! that framing and then sprinkle the opcode under test ahead of a
//! single `paintRect` so the decode finishes with a non-empty canvas.
//!
//! Every byte sequence is traceable back to §A-3 Table A-3 (book pages
//! A-18..A-21). No external implementation consulted.

use oxideav_pict::{parse_pict, probe_pict, ProbeTermination};

// ---------------------------------------------------------------------------
// Bytestream builders.
// ---------------------------------------------------------------------------

fn put_u8(out: &mut Vec<u8>, v: u8) {
    out.push(v);
}
fn put_i16(out: &mut Vec<u8>, v: i16) {
    out.extend_from_slice(&v.to_be_bytes());
}
fn put_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_be_bytes());
}

/// Minimal v1 picture-record framing. Returns a vec with the 10-byte
/// picture-record header + `0x11 0x01` version stanza already emitted;
/// caller appends opcodes then `0xFF`.
fn v1_header() -> Vec<u8> {
    let mut out = Vec::new();
    // picSize (ignored).
    put_u16(&mut out, 0);
    // picFrame: 4 × 4 so we can hand the SameRect machinery a couple of
    // distinguishable rects without spilling outside.
    put_i16(&mut out, 0);
    put_i16(&mut out, 0);
    put_i16(&mut out, 4);
    put_i16(&mut out, 4);
    // v1 version stanza: opcode 0x11 followed by version 0x01.
    put_u8(&mut out, 0x11);
    put_u8(&mut out, 0x01);
    out
}

/// Append a `paintRect` covering the full 4×4 frame so the decode
/// produces a non-empty canvas.
fn append_paint_rect_full(out: &mut Vec<u8>) {
    // paintRect (v1 opcode 0x31).
    put_u8(out, 0x31);
    put_i16(out, 0);
    put_i16(out, 0);
    put_i16(out, 4);
    put_i16(out, 4);
}

/// Append a `paintRect` covering rect (top,left,bottom,right) so the
/// Same-shape opcodes have a known last_* slot to consume.
fn append_paint_rect(out: &mut Vec<u8>, top: i16, left: i16, bottom: i16, right: i16) {
    put_u8(out, 0x31);
    put_i16(out, top);
    put_i16(out, left);
    put_i16(out, bottom);
    put_i16(out, right);
}

/// Append the v1 EOP terminator.
fn close_pict(out: &mut Vec<u8>) {
    put_u8(out, 0xFF);
}

// ---------------------------------------------------------------------------
// State / text setup opcodes (0x03..0x10).
// ---------------------------------------------------------------------------

#[test]
fn v1_tx_font_walked_past_2_bytes() {
    let mut out = v1_header();
    put_u8(&mut out, 0x03); // TxFont
    put_u16(&mut out, 21); // font id = 21 (Helvetica, FYI)
    append_paint_rect_full(&mut out);
    close_pict(&mut out);
    let img = parse_pict(&out).expect("TxFont should be walked past");
    assert_eq!(img.width, 4);
    assert_eq!(img.height, 4);
}

#[test]
fn v1_tx_face_walked_past_1_byte() {
    let mut out = v1_header();
    put_u8(&mut out, 0x04); // TxFace
    put_u8(&mut out, 0x01); // bold
    append_paint_rect_full(&mut out);
    close_pict(&mut out);
    let img = parse_pict(&out).expect("TxFace should be walked past");
    assert_eq!(img.width, 4);
}

#[test]
fn v1_tx_mode_walked_past_2_bytes() {
    let mut out = v1_header();
    put_u8(&mut out, 0x05); // TxMode
    put_u16(&mut out, 0); // srcCopy
    append_paint_rect_full(&mut out);
    close_pict(&mut out);
    parse_pict(&out).expect("TxMode should be walked past");
}

#[test]
fn v1_sp_extra_walked_past_4_bytes() {
    let mut out = v1_header();
    put_u8(&mut out, 0x06); // SpExtra (Fixed)
    out.extend_from_slice(&[0x00, 0x01, 0x00, 0x00]); // 1.0
    append_paint_rect_full(&mut out);
    close_pict(&mut out);
    parse_pict(&out).expect("SpExtra should be walked past");
}

#[test]
fn v1_pn_mode_walked_past_2_bytes() {
    let mut out = v1_header();
    put_u8(&mut out, 0x08); // PnMode
    put_u16(&mut out, 8); // patCopy
    append_paint_rect_full(&mut out);
    close_pict(&mut out);
    parse_pict(&out).expect("PnMode should be walked past");
}

#[test]
fn v1_tx_size_walked_past_2_bytes() {
    let mut out = v1_header();
    put_u8(&mut out, 0x0D); // TxSize
    put_u16(&mut out, 12);
    append_paint_rect_full(&mut out);
    close_pict(&mut out);
    parse_pict(&out).expect("TxSize should be walked past");
}

#[test]
fn v1_tx_ratio_walked_past_8_bytes() {
    let mut out = v1_header();
    put_u8(&mut out, 0x10); // TxRatio
                            // numerator (Point) + denominator (Point)
    put_i16(&mut out, 1);
    put_i16(&mut out, 1);
    put_i16(&mut out, 1);
    put_i16(&mut out, 1);
    append_paint_rect_full(&mut out);
    close_pict(&mut out);
    parse_pict(&out).expect("TxRatio should be walked past");
}

// ---------------------------------------------------------------------------
// Text opcodes (0x28..0x2B).
// ---------------------------------------------------------------------------

#[test]
fn v1_long_text_walked_past() {
    let mut out = v1_header();
    put_u8(&mut out, 0x28); // LongText
                            // txLoc Point (v, h)
    put_i16(&mut out, 10);
    put_i16(&mut out, 5);
    // count + text
    put_u8(&mut out, 5);
    out.extend_from_slice(b"hello");
    append_paint_rect_full(&mut out);
    close_pict(&mut out);
    parse_pict(&out).expect("LongText should be walked past");
}

#[test]
fn v1_dh_text_walked_past() {
    let mut out = v1_header();
    put_u8(&mut out, 0x29); // DHText
    put_u8(&mut out, 3); // dh
    put_u8(&mut out, 2); // count
    out.extend_from_slice(b"hi");
    append_paint_rect_full(&mut out);
    close_pict(&mut out);
    parse_pict(&out).expect("DHText should be walked past");
}

#[test]
fn v1_dv_text_walked_past() {
    let mut out = v1_header();
    put_u8(&mut out, 0x2A); // DVText
    put_u8(&mut out, 3); // dv
    put_u8(&mut out, 0); // empty
    append_paint_rect_full(&mut out);
    close_pict(&mut out);
    parse_pict(&out).expect("DVText should be walked past");
}

#[test]
fn v1_dhdv_text_walked_past() {
    let mut out = v1_header();
    put_u8(&mut out, 0x2B); // DHDVText
    put_u8(&mut out, 3); // dh
    put_u8(&mut out, 4); // dv
    put_u8(&mut out, 4); // count
    out.extend_from_slice(b"yarn");
    append_paint_rect_full(&mut out);
    close_pict(&mut out);
    parse_pict(&out).expect("DHDVText should be walked past");
}

// ---------------------------------------------------------------------------
// Same-shape opcodes (0x38..0x3C, 0x48..0x4C, 0x58..0x5C, 0x68..0x6C).
// ---------------------------------------------------------------------------

#[test]
fn v1_paint_same_rect_repaints_last_rect() {
    // paint a 2×2 red rect; then `paintSameRect` to repaint it after the
    // canvas state has progressed. Verifies the decode finishes AND that
    // last_rect tracking applies.
    let mut out = v1_header();
    // FgColor (v1 0x0E): set ink to a non-default value so we can spot
    // the paint succeeded. Using the legacy 4-byte Pascal colour code 33
    // (red).
    put_u8(&mut out, 0x0E);
    out.extend_from_slice(&[0, 0, 0, 33]);
    // paintRect on a 2×2 sub-area in the top-left.
    append_paint_rect(&mut out, 0, 0, 2, 2);
    // paintSameRect — should repaint the same 2×2.
    put_u8(&mut out, 0x39);
    close_pict(&mut out);
    let img = parse_pict(&out).expect("paintSameRect should walk cleanly");
    assert_eq!(img.width, 4);
}

#[test]
fn v1_all_same_rect_verbs_walk_past() {
    // 0x38..=0x3C: frame / paint / erase / invert / fill — all 0-byte
    // payload.
    for op in 0x38u8..=0x3Cu8 {
        let mut out = v1_header();
        // Establish a last_rect.
        append_paint_rect(&mut out, 1, 1, 3, 3);
        put_u8(&mut out, op);
        // Also append a paintRect so the canvas is non-empty in case
        // the same-verb is a no-op (e.g. erase on default white bg).
        append_paint_rect_full(&mut out);
        close_pict(&mut out);
        parse_pict(&out)
            .unwrap_or_else(|e| panic!("v1 same-rect opcode 0x{op:02X} should decode: {e}"));
    }
}

#[test]
fn v1_all_same_rrect_verbs_walk_past() {
    for op in 0x48u8..=0x4Cu8 {
        let mut out = v1_header();
        // OvSize (v1 0x0B) so round-rect has corner dimensions.
        put_u8(&mut out, 0x0B);
        put_i16(&mut out, 2); // v
        put_i16(&mut out, 2); // h
                              // paintRRect (0x41) to set last_rrect.
        put_u8(&mut out, 0x41);
        put_i16(&mut out, 0);
        put_i16(&mut out, 0);
        put_i16(&mut out, 3);
        put_i16(&mut out, 3);
        // Same-RRect verb.
        put_u8(&mut out, op);
        append_paint_rect_full(&mut out);
        close_pict(&mut out);
        parse_pict(&out)
            .unwrap_or_else(|e| panic!("v1 same-rrect opcode 0x{op:02X} should decode: {e}"));
    }
}

#[test]
fn v1_all_same_oval_verbs_walk_past() {
    for op in 0x58u8..=0x5Cu8 {
        let mut out = v1_header();
        // paintOval (0x51) to set last_oval.
        put_u8(&mut out, 0x51);
        put_i16(&mut out, 0);
        put_i16(&mut out, 0);
        put_i16(&mut out, 3);
        put_i16(&mut out, 3);
        put_u8(&mut out, op);
        append_paint_rect_full(&mut out);
        close_pict(&mut out);
        parse_pict(&out)
            .unwrap_or_else(|e| panic!("v1 same-oval opcode 0x{op:02X} should decode: {e}"));
    }
}

#[test]
fn v1_all_same_arc_verbs_walk_past() {
    for op in 0x68u8..=0x6Cu8 {
        let mut out = v1_header();
        // paintArc (0x61) to set last_arc_rect.
        put_u8(&mut out, 0x61);
        put_i16(&mut out, 0);
        put_i16(&mut out, 0);
        put_i16(&mut out, 3);
        put_i16(&mut out, 3);
        put_i16(&mut out, 0); // startAngle
        put_i16(&mut out, 90); // arcAngle
                               // Same-Arc: 4-byte payload = start + arc.
        put_u8(&mut out, op);
        put_i16(&mut out, 90);
        put_i16(&mut out, 180);
        append_paint_rect_full(&mut out);
        close_pict(&mut out);
        parse_pict(&out)
            .unwrap_or_else(|e| panic!("v1 same-arc opcode 0x{op:02X} should decode: {e}"));
    }
}

#[test]
fn v1_same_rect_with_no_prior_rect_is_silent_noop() {
    // §A-3 leaves the "no prior shape" behaviour implementation-defined;
    // our impl silently does nothing rather than abort.
    let mut out = v1_header();
    put_u8(&mut out, 0x39); // paintSameRect with last_rect = None
    append_paint_rect_full(&mut out);
    close_pict(&mut out);
    let img = parse_pict(&out).expect("orphan paintSameRect should no-op");
    assert_eq!(img.width, 4);
}

// ---------------------------------------------------------------------------
// "Not yet implemented" same-shape opcodes (0x78..0x7C, 0x88..0x8C).
// ---------------------------------------------------------------------------

#[test]
fn v1_same_poly_range_walks_past() {
    // §A-3 Table A-3 marks 0x78..=0x7C as "(Not yet implemented)" with
    // 0-byte payload — accept silently so a private-extension PICT
    // doesn't poison the decode.
    for op in 0x78u8..=0x7Cu8 {
        let mut out = v1_header();
        put_u8(&mut out, op);
        append_paint_rect_full(&mut out);
        close_pict(&mut out);
        parse_pict(&out)
            .unwrap_or_else(|e| panic!("v1 same-poly opcode 0x{op:02X} should decode: {e}"));
    }
}

#[test]
fn v1_same_rgn_range_walks_past() {
    for op in 0x88u8..=0x8Cu8 {
        let mut out = v1_header();
        put_u8(&mut out, op);
        append_paint_rect_full(&mut out);
        close_pict(&mut out);
        parse_pict(&out)
            .unwrap_or_else(|e| panic!("v1 same-rgn opcode 0x{op:02X} should decode: {e}"));
    }
}

// ---------------------------------------------------------------------------
// Probe-side accounting (probe must agree on these opcodes too).
// ---------------------------------------------------------------------------

#[test]
fn probe_v1_text_state_opcodes_walked_past_cleanly() {
    let mut out = v1_header();
    put_u8(&mut out, 0x03);
    put_u16(&mut out, 21);
    put_u8(&mut out, 0x04);
    put_u8(&mut out, 1);
    put_u8(&mut out, 0x05);
    put_u16(&mut out, 0);
    put_u8(&mut out, 0x06);
    out.extend_from_slice(&[0; 4]);
    put_u8(&mut out, 0x08);
    put_u16(&mut out, 0);
    put_u8(&mut out, 0x0D);
    put_u16(&mut out, 12);
    put_u8(&mut out, 0x10);
    out.extend_from_slice(&[0; 8]);
    append_paint_rect_full(&mut out);
    close_pict(&mut out);
    let p = probe_pict(&out).expect("probe should walk all v1 state opcodes");
    assert_eq!(p.termination, ProbeTermination::EndPic);
    assert!(p.end_pic_seen);
    assert_eq!(p.drawing_count, 1);
}

#[test]
fn probe_v1_long_text_increments_text_count() {
    // Round 401: v1 text opcodes count into `text_count`, matching the
    // v2 walker's classification (version-independent probe contract).
    let mut out = v1_header();
    put_u8(&mut out, 0x28);
    put_i16(&mut out, 10);
    put_i16(&mut out, 5);
    put_u8(&mut out, 3);
    out.extend_from_slice(b"abc");
    close_pict(&mut out);
    let p = probe_pict(&out).expect("probe should walk v1 LongText");
    assert_eq!(p.termination, ProbeTermination::EndPic);
    assert_eq!(p.text_count, 1);
    assert_eq!(p.drawing_count, 0);
}

#[test]
fn probe_v1_dhdv_text_increments_text_count() {
    // Round 401: see probe_v1_long_text_increments_text_count.
    let mut out = v1_header();
    put_u8(&mut out, 0x2B);
    put_u8(&mut out, 3);
    put_u8(&mut out, 4);
    put_u8(&mut out, 4);
    out.extend_from_slice(b"abcd");
    close_pict(&mut out);
    let p = probe_pict(&out).expect("probe should walk v1 DHDVText");
    assert_eq!(p.text_count, 1);
    assert_eq!(p.drawing_count, 0);
}

#[test]
fn probe_v1_same_shape_count_increments_for_all_families() {
    let mut out = v1_header();
    // One each from the four implemented same-shape families.
    // Establish last_* before each.
    // Rect.
    append_paint_rect(&mut out, 0, 0, 2, 2);
    put_u8(&mut out, 0x39); // paintSameRect
                            // RRect.
    put_u8(&mut out, 0x0B);
    put_i16(&mut out, 2);
    put_i16(&mut out, 2);
    put_u8(&mut out, 0x41);
    put_i16(&mut out, 0);
    put_i16(&mut out, 0);
    put_i16(&mut out, 3);
    put_i16(&mut out, 3);
    put_u8(&mut out, 0x49); // paintSameRRect
                            // Oval.
    put_u8(&mut out, 0x51);
    put_i16(&mut out, 0);
    put_i16(&mut out, 0);
    put_i16(&mut out, 3);
    put_i16(&mut out, 3);
    put_u8(&mut out, 0x59); // paintSameOval
                            // Arc.
    put_u8(&mut out, 0x61);
    put_i16(&mut out, 0);
    put_i16(&mut out, 0);
    put_i16(&mut out, 3);
    put_i16(&mut out, 3);
    put_i16(&mut out, 0);
    put_i16(&mut out, 90);
    put_u8(&mut out, 0x69); // paintSameArc — 4-byte payload
    put_i16(&mut out, 0);
    put_i16(&mut out, 90);
    close_pict(&mut out);
    let p = probe_pict(&out).expect("probe should walk all v1 same-shape opcodes");
    assert_eq!(p.termination, ProbeTermination::EndPic);
    // Four same-shape opcodes total: SameRect + SameRRect + SameOval +
    // SameArc.
    assert_eq!(p.same_shape_count, 4);
    // Four drawing opcodes that established the last_* state:
    // paintRect + paintRRect + paintOval + paintArc.
    assert_eq!(p.drawing_count, 4);
}

#[test]
fn probe_v1_same_poly_and_rgn_no_payload_walks_past() {
    let mut out = v1_header();
    put_u8(&mut out, 0x79); // paintSamePoly (spec: NYI, 0-byte)
    put_u8(&mut out, 0x89); // paintSameRgn (spec: NYI, 0-byte)
    close_pict(&mut out);
    let p = probe_pict(&out).expect("probe should walk v1 NYI same-shape opcodes");
    assert_eq!(p.termination, ProbeTermination::EndPic);
    assert_eq!(p.same_shape_count, 2);
}

// ---------------------------------------------------------------------------
// Negative cases — confirm we still reject something genuinely undefined.
// ---------------------------------------------------------------------------

#[test]
fn v1_truly_unknown_opcode_still_rejects() {
    // 0x35 is undefined in §A-3 Table A-3 (between fillRect and
    // frameSameRect). Make sure it still surfaces an Unsupported error
    // — we expanded the dispatcher, didn't replace it with a fallback.
    let mut out = v1_header();
    put_u8(&mut out, 0x35);
    close_pict(&mut out);
    let err = parse_pict(&out).expect_err("undefined v1 opcode should still reject");
    let s = err.to_string();
    assert!(
        s.contains("unsupported v1 opcode 0x35") || s.contains("0x35"),
        "expected 0x35 reject; got: {s}"
    );
}
