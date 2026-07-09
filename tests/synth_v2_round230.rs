//! Round 230 — structured text / pen-mode / highlight state opcodes.
//!
//! Inside Macintosh: Imaging With QuickDraw §A-3 Table A-2 (v2) and
//! Table A-3 (v1) list a block of state-mutating opcodes whose payloads
//! were previously walked past with no further accounting:
//!
//! * `TxFont` `$0003` v2 / `$03` v1 — 2-byte `Integer` font number.
//! * `TxFace` `$0004` v2 / `$04` v1 — 1-byte `Style` flags.
//! * `TxMode` `$0005` v2 / `$05` v1 — 2-byte `Integer` source mode.
//! * `SpExtra` `$0006` v2 / `$06` v1 — 4-byte `Fixed` extra space.
//! * `PnMode` `$0008` v2 / `$08` v1 — 2-byte `Integer` pen mode.
//! * `TxSize` `$000D` v2 / `$0D` v1 — 2-byte `Integer` size in points.
//! * `TxRatio` `$0010` v2 / `$10` v1 — 8-byte numerator+denominator
//!   Point pair.
//! * `PnLocHFrac` `$0015` (v2 only) — 2-byte fractional pen position.
//! * `ChExtra` `$0016` (v2 only) — 2-byte per-character extra width.
//! * `HiliteMode` `$001C` (v2 only) — 0-byte flag.
//! * `HiliteColor` `$001D` (v2 only) — 6-byte `RGBColor`.
//! * `DefHilite` `$001E` (v2 only) — 0-byte reset.
//! * `OpColor` `$001F` (v2 only) — 6-byte `RGBColor` for arithmetic
//!   transfer modes.
//!
//! Round 230 captures the payloads into [`PictTextState`] (surfaced on
//! [`PictImage::text_state`] / [`PictProbe::text_state`]) and counts
//! the occurrences via [`PictProbe::text_state_op_count`]. Encoder
//! helpers ([`build_tx_font`] … [`build_op_color`]) and chainable
//! [`PictBuilder`] methods ([`tx_font`] … [`op_color`]) round-trip
//! through `parse_pict` / `probe_pict` bit-for-bit.

use oxideav_pict::ops::{PictBuilder, Verb};
use oxideav_pict::{
    build_ch_extra, build_def_hilite, build_hilite_color, build_hilite_mode, build_op_color,
    build_pn_loc_h_frac, build_pn_mode, build_sp_extra, build_tx_face, build_tx_font,
    build_tx_mode, build_tx_ratio, build_tx_size, parse_pict, probe_pict, PictTextState, TextRatio,
};

// ---------------------------------------------------------------------------
// Default state.
// ---------------------------------------------------------------------------

#[test]
fn fresh_graf_port_defaults_match_quickdraw() {
    let ts = PictTextState::fresh_graf_port();
    assert_eq!(ts.tx_font, 0);
    assert_eq!(ts.tx_face, 0);
    assert_eq!(ts.tx_mode, 0); // srcCopy
    assert_eq!(ts.sp_extra.0, 0);
    assert_eq!(ts.pn_mode, 8); // patCopy
    assert_eq!(ts.tx_size, 12);
    assert_eq!(ts.tx_ratio, TextRatio::default());
    // 0.5 = bit pattern 0x8000 as the low word of a Fixed (round 401;
    // previously mis-defaulted to 0x4000 = 0.25).
    assert_eq!(ts.pn_loc_h_frac as u16, 0x8000);
    assert_eq!(ts.ch_extra, 0);
    assert!(ts.hilite_color.is_none());
    assert!(ts.op_color.is_none());
    assert!(!ts.hilite_default);
    assert!(!ts.hilite_mode_flag);
}

#[test]
fn text_ratio_default_is_unit() {
    let r = TextRatio::default();
    assert_eq!(r.numer_v, 1);
    assert_eq!(r.numer_h, 1);
    assert_eq!(r.denom_v, 1);
    assert_eq!(r.denom_h, 1);
}

// ---------------------------------------------------------------------------
// Encoder byte-layout assertions.
// ---------------------------------------------------------------------------

#[test]
fn build_tx_font_emits_opcode_plus_integer() {
    assert_eq!(build_tx_font(0x1234), [0x00, 0x03, 0x12, 0x34]);
}

#[test]
fn build_tx_face_emits_opcode_plus_byte() {
    assert_eq!(build_tx_face(0x55), [0x00, 0x04, 0x55]);
}

#[test]
fn build_tx_mode_emits_opcode_plus_integer() {
    assert_eq!(build_tx_mode(0x0007), [0x00, 0x05, 0x00, 0x07]);
}

#[test]
fn build_sp_extra_emits_opcode_plus_fixed() {
    // 1.5 = 0x0001_8000 in Fixed (16.16).
    assert_eq!(
        build_sp_extra(0x0001_8000),
        [0x00, 0x06, 0x00, 0x01, 0x80, 0x00]
    );
}

#[test]
fn build_pn_mode_emits_opcode_plus_integer() {
    assert_eq!(build_pn_mode(0x000A), [0x00, 0x08, 0x00, 0x0A]);
}

#[test]
fn build_tx_size_emits_opcode_plus_integer() {
    assert_eq!(build_tx_size(0x0018), [0x00, 0x0D, 0x00, 0x18]);
}

#[test]
fn build_tx_ratio_emits_opcode_plus_point_pair() {
    // numer=(2v, 3h), denom=(4v, 5h)
    assert_eq!(
        build_tx_ratio(2, 3, 4, 5),
        [0x00, 0x10, 0x00, 0x02, 0x00, 0x03, 0x00, 0x04, 0x00, 0x05]
    );
}

#[test]
fn build_pn_loc_h_frac_emits_opcode_plus_word() {
    assert_eq!(build_pn_loc_h_frac(0x4000), [0x00, 0x15, 0x40, 0x00]);
}

#[test]
fn build_ch_extra_emits_opcode_plus_integer() {
    assert_eq!(build_ch_extra(0x0042), [0x00, 0x16, 0x00, 0x42]);
}

#[test]
fn build_hilite_mode_emits_bare_opcode() {
    assert_eq!(build_hilite_mode(), [0x00, 0x1C]);
}

#[test]
fn build_def_hilite_emits_bare_opcode() {
    assert_eq!(build_def_hilite(), [0x00, 0x1E]);
}

#[test]
fn build_hilite_color_replicates_8bit_to_16bit() {
    // 0xAB -> 0xABAB on each channel per the encoder convention.
    assert_eq!(
        build_hilite_color(0xAB, 0xCD, 0xEF),
        [0x00, 0x1D, 0xAB, 0xAB, 0xCD, 0xCD, 0xEF, 0xEF]
    );
}

#[test]
fn build_op_color_replicates_8bit_to_16bit() {
    assert_eq!(
        build_op_color(0x10, 0x20, 0x30),
        [0x00, 0x1F, 0x10, 0x10, 0x20, 0x20, 0x30, 0x30]
    );
}

// ---------------------------------------------------------------------------
// Decoder + probe round-trip surface.
// ---------------------------------------------------------------------------

fn paint_canvas(b: &mut PictBuilder) {
    b.fg_color(0, 0, 0).rect(Verb::Paint, 0, 0, 4, 4);
}

#[test]
fn pict_image_carries_text_state_after_state_opcodes() {
    let mut b = PictBuilder::new(0, 0, 4, 4);
    b.tx_font(0x0102)
        .tx_face(0x05)
        .tx_mode(7)
        .sp_extra(0x0001_8000)
        .pn_mode(10)
        .tx_size(24)
        .tx_ratio(2, 3, 4, 5)
        .pn_loc_h_frac(0x6000)
        .ch_extra(0x0042)
        .op_color(0x10, 0x20, 0x30);
    paint_canvas(&mut b);
    let bytes = b.finish();

    let img = parse_pict(&bytes).expect("decode");
    let ts = img.text_state;
    assert_eq!(ts.tx_font, 0x0102);
    assert_eq!(ts.tx_face, 0x05);
    assert_eq!(ts.tx_mode, 7);
    assert_eq!(ts.sp_extra.0, 0x0001_8000);
    assert_eq!(ts.pn_mode, 10);
    assert_eq!(ts.tx_size, 24);
    assert_eq!(
        ts.tx_ratio,
        TextRatio {
            numer_v: 2,
            numer_h: 3,
            denom_v: 4,
            denom_h: 5,
        }
    );
    assert_eq!(ts.pn_loc_h_frac, 0x6000);
    assert_eq!(ts.ch_extra, 0x0042);
    let op = ts.op_color.expect("op_color set");
    assert_eq!((op.r, op.g, op.b), (0x10, 0x20, 0x30));
}

#[test]
fn hilite_mode_then_hilite_color_then_def_hilite_sequence() {
    let mut b = PictBuilder::new(0, 0, 4, 4);
    b.hilite_mode().hilite_color(0xAA, 0xBB, 0xCC).def_hilite();
    paint_canvas(&mut b);
    let bytes = b.finish();

    let img = parse_pict(&bytes).expect("decode");
    // DefHilite resets hilite_color to None and sets the default flag;
    // HiliteMode flag is independent and stays set.
    assert!(img.text_state.hilite_mode_flag);
    assert!(img.text_state.hilite_default);
    assert!(img.text_state.hilite_color.is_none());
}

#[test]
fn hilite_color_without_def_hilite_persists() {
    let mut b = PictBuilder::new(0, 0, 4, 4);
    b.hilite_color(0xFF, 0x00, 0x00);
    paint_canvas(&mut b);
    let bytes = b.finish();

    let img = parse_pict(&bytes).expect("decode");
    let hc = img.text_state.hilite_color.expect("hilite_color set");
    assert_eq!((hc.r, hc.g, hc.b), (0xFF, 0x00, 0x00));
    assert!(!img.text_state.hilite_default);
}

#[test]
fn def_hilite_after_hilite_color_clears_to_default() {
    let mut b = PictBuilder::new(0, 0, 4, 4);
    b.hilite_color(0xFF, 0x00, 0x00).def_hilite();
    paint_canvas(&mut b);
    let bytes = b.finish();

    let img = parse_pict(&bytes).expect("decode");
    assert!(img.text_state.hilite_default);
    assert!(img.text_state.hilite_color.is_none());
}

#[test]
fn hilite_color_after_def_hilite_overrides_default() {
    let mut b = PictBuilder::new(0, 0, 4, 4);
    b.def_hilite().hilite_color(0x00, 0xFF, 0x00);
    paint_canvas(&mut b);
    let bytes = b.finish();

    let img = parse_pict(&bytes).expect("decode");
    let hc = img.text_state.hilite_color.expect("hilite_color set");
    assert_eq!((hc.r, hc.g, hc.b), (0x00, 0xFF, 0x00));
    assert!(!img.text_state.hilite_default);
}

#[test]
fn missing_state_opcodes_keep_fresh_graf_port_defaults() {
    let mut b = PictBuilder::new(0, 0, 4, 4);
    paint_canvas(&mut b);
    let bytes = b.finish();

    let img = parse_pict(&bytes).expect("decode");
    assert_eq!(img.text_state, PictTextState::fresh_graf_port());
}

// ---------------------------------------------------------------------------
// Probe surface mirrors the decoder surface.
// ---------------------------------------------------------------------------

#[test]
fn probe_captures_text_state_without_rasterising() {
    let mut b = PictBuilder::new(0, 0, 4, 4);
    b.tx_font(0x0099).tx_size(18).op_color(0x11, 0x22, 0x33);
    paint_canvas(&mut b);
    let bytes = b.finish();

    let p = probe_pict(&bytes).expect("probe");
    assert_eq!(p.text_state.tx_font, 0x0099);
    assert_eq!(p.text_state.tx_size, 18);
    let oc = p.text_state.op_color.expect("op_color");
    assert_eq!((oc.r, oc.g, oc.b), (0x11, 0x22, 0x33));
    // tx_font + tx_size + op_color = 3 state opcodes counted.
    assert_eq!(p.text_state_op_count, 3);
}

#[test]
fn probe_text_state_default_when_no_state_opcodes() {
    let mut b = PictBuilder::new(0, 0, 4, 4);
    paint_canvas(&mut b);
    let bytes = b.finish();

    let p = probe_pict(&bytes).expect("probe");
    assert_eq!(p.text_state, PictTextState::fresh_graf_port());
    assert_eq!(p.text_state_op_count, 0);
}

#[test]
fn probe_counts_every_state_opcode_emission() {
    let mut b = PictBuilder::new(0, 0, 4, 4);
    // 13 distinct state opcodes — count must increment for each.
    b.tx_font(0)
        .tx_face(0)
        .tx_mode(0)
        .sp_extra(0)
        .pn_mode(0)
        .tx_size(0)
        .tx_ratio(1, 1, 1, 1)
        .pn_loc_h_frac(0)
        .ch_extra(0)
        .hilite_mode()
        .hilite_color(0, 0, 0)
        .def_hilite()
        .op_color(0, 0, 0);
    paint_canvas(&mut b);
    let bytes = b.finish();

    let p = probe_pict(&bytes).expect("probe");
    assert_eq!(p.text_state_op_count, 13);
}

#[test]
fn probe_and_decoder_text_state_agree() {
    let mut b = PictBuilder::new(0, 0, 4, 4);
    // pn_mode = 8 (patCopy, §3-44 default) keeps the §247 transfer-mode
    // path on the round-8 solid-fg fast path so paint_canvas still
    // emits visible pixels here; the assertion is on text_state
    // round-trip parity between probe + decoder, not on the canvas
    // contents (transfer-mode-aware rasterisation is covered by the
    // round-247 suite).
    b.tx_font(0x4242).pn_mode(8).hilite_color(0x7F, 0x80, 0x81);
    paint_canvas(&mut b);
    let bytes = b.finish();

    let img = parse_pict(&bytes).expect("decode");
    let p = probe_pict(&bytes).expect("probe");
    assert_eq!(img.text_state, p.text_state);
}

// ---------------------------------------------------------------------------
// v1 dispatcher.
// ---------------------------------------------------------------------------

/// Build a minimal v1 PICT: 10-byte picture record + `0x11 0x01`
/// version stanza + body opcodes + `0xFF` terminator.
fn build_v1(body: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    // picSize placeholder.
    out.extend_from_slice(&[0u8; 2]);
    // picFrame top/left/bottom/right = 0,0,4,4.
    out.extend_from_slice(&[0, 0, 0, 0, 0, 4, 0, 4]);
    // v1 version stanza.
    out.extend_from_slice(&[0x11, 0x01]);
    // Body.
    out.extend_from_slice(body);
    // paintRect 0..4 to guarantee a raster.
    out.extend_from_slice(&[0x31, 0, 0, 0, 0, 0, 4, 0, 4]);
    // Terminator.
    out.push(0xFF);
    out
}

#[test]
fn v1_decoder_captures_tx_font_tx_face_tx_mode_tx_size() {
    let bytes = build_v1(&[
        0x03, 0x12, 0x34, // TxFont 0x1234
        0x04, 0x07, // TxFace 0x07
        0x05, 0x00, 0x01, // TxMode 1 (srcOr)
        0x0D, 0x00, 0x18, // TxSize 24
    ]);
    let img = parse_pict(&bytes).expect("decode v1");
    assert_eq!(img.text_state.tx_font, 0x1234);
    assert_eq!(img.text_state.tx_face, 0x07);
    assert_eq!(img.text_state.tx_mode, 1);
    assert_eq!(img.text_state.tx_size, 24);
}

#[test]
fn v1_decoder_captures_sp_extra_pn_mode_tx_ratio() {
    let bytes = build_v1(&[
        0x06, 0x00, 0x01, 0x80, 0x00, // SpExtra 1.5 Fixed
        0x08, 0x00, 0x0A, // PnMode 10
        0x10, 0x00, 0x02, 0x00, 0x03, 0x00, 0x04, 0x00, 0x05, // TxRatio (2,3)/(4,5)
    ]);
    let img = parse_pict(&bytes).expect("decode v1");
    assert_eq!(img.text_state.sp_extra.0, 0x0001_8000);
    assert_eq!(img.text_state.pn_mode, 10);
    assert_eq!(
        img.text_state.tx_ratio,
        TextRatio {
            numer_v: 2,
            numer_h: 3,
            denom_v: 4,
            denom_h: 5,
        }
    );
}

#[test]
fn v1_probe_captures_text_state_and_counts() {
    let bytes = build_v1(&[
        0x03, 0x00, 0x01, // TxFont 1
        0x0D, 0x00, 0x0C, // TxSize 12 (== default — exercises counter regardless)
    ]);
    let p = probe_pict(&bytes).expect("probe v1");
    assert_eq!(p.text_state.tx_font, 1);
    assert_eq!(p.text_state.tx_size, 12);
    assert_eq!(p.text_state_op_count, 2);
}

// ---------------------------------------------------------------------------
// 16-bit-RGB high-byte round-trip (matches HiliteColor / OpColor wire form).
// ---------------------------------------------------------------------------

#[test]
fn hilite_color_and_op_color_round_trip_high_byte() {
    let mut b = PictBuilder::new(0, 0, 4, 4);
    b.hilite_color(0xC0, 0x40, 0x80).op_color(0x33, 0x66, 0x99);
    paint_canvas(&mut b);
    let bytes = b.finish();

    let img = parse_pict(&bytes).expect("decode");
    let hc = img.text_state.hilite_color.expect("hilite_color");
    assert_eq!((hc.r, hc.g, hc.b), (0xC0, 0x40, 0x80));
    let oc = img.text_state.op_color.expect("op_color");
    assert_eq!((oc.r, oc.g, oc.b), (0x33, 0x66, 0x99));
}
