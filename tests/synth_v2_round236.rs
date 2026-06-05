//! Round 236 — structured `fontName` / `lineJustify` / `glyphState`
//! opcode capture.
//!
//! Inside Macintosh: Imaging With QuickDraw §A-3 Table A-2 lists three
//! v2-only state-mutating opcodes whose payloads carry Script-Manager
//! and font-engine round-trip parameters but had previously been walked
//! past with no further structure:
//!
//! * `fontName` `$002C` — footnote `*`: `dataLength (Integer)`,
//!   `oldFontID (Integer)`, `nameLength (0..255)`, `name (nameLength
//!   bytes)`. The `dataLength` value includes itself, so the column's
//!   "5 + nameLen" total is `2 (length) + 2 (oldFontID) + 1 (nameLen) +
//!   N (name)`.
//! * `lineJustify` `$002D` — footnote `†`: `dataLength = 8`, two
//!   `Fixed` values (intercharacter spacing + total extra). The
//!   `dataLength` excludes itself, matching the appendix's worked
//!   example `2D 00 08 00 01 00 00 00 0A 00 00`.
//! * `glyphState` `$002E` — `dataLength`-prefixed block carrying four
//!   1-byte Booleans (`outline preferred`, `preserve glyph`,
//!   `fractional widths`, `scaling disabled`). The §A-3 "Additional
//!   data size" column is 8 = 2 (length) + 4 (Booleans) + 2 (pad), so
//!   the encoder writes `dataLength = 6` and two trailing zero pad
//!   bytes.
//!
//! Round 236 promotes the three walk-past paths to structured capture
//! into [`PictTextState`]'s new `font_name` / `line_justify` /
//! `glyph_state` slots, surfaced on [`PictImage::text_state`] and
//! [`PictProbe::text_state`]. The probe's `text_state_op_count` field
//! is bumped once per occurrence, mirroring round 230's accounting.

use oxideav_pict::ops::{PictBuilder, Verb};
use oxideav_pict::{
    build_font_name, build_glyph_state, build_line_justify, parse_pict, probe_pict, Fixed,
    PictError, PictFontName, PictGlyphState, PictLineJustify, PictTextState,
};

// ---------------------------------------------------------------------------
// Default state — the new slots start at `None`.
// ---------------------------------------------------------------------------

#[test]
fn fresh_graf_port_round236_slots_are_none() {
    let ts = PictTextState::fresh_graf_port();
    assert!(ts.font_name.is_none());
    assert!(ts.line_justify.is_none());
    assert!(ts.glyph_state.is_none());
}

// ---------------------------------------------------------------------------
// Encoder byte-layout assertions.
// ---------------------------------------------------------------------------

#[test]
fn build_font_name_layout() {
    // oldFontID = 0x0102 = "Chicago" (legacy FOND ID), name = "Geneva".
    let bytes = build_font_name(0x0102, b"Geneva").expect("encode");
    // Opcode (00 2C) + dataLength (00 0B = 11 = 5 + 6) +
    // oldFontID (01 02) + nameLen (06) + 6 bytes "Geneva".
    let expected: &[u8] = &[
        0x00, 0x2C, 0x00, 0x0B, 0x01, 0x02, 0x06, b'G', b'e', b'n', b'e', b'v', b'a',
    ];
    assert_eq!(bytes, expected);
}

#[test]
fn build_font_name_empty_name_is_valid() {
    let bytes = build_font_name(0x0001, b"").expect("encode");
    // dataLength = 5 = 2 (length) + 2 (oldFontID) + 1 (nameLen=0).
    assert_eq!(bytes, [0x00, 0x2C, 0x00, 0x05, 0x00, 0x01, 0x00]);
}

#[test]
fn build_font_name_rejects_oversize_name() {
    let oversize = vec![b'X'; 256];
    let err = build_font_name(0, &oversize).expect_err("must reject");
    assert!(matches!(err, PictError::InvalidData { .. }));
}

#[test]
fn build_line_justify_matches_spec_worked_example() {
    // Appendix A-3 footnote `†` worked example for v1:
    //   `2D 00 08 00 01 00 00 00 0A 00 00`
    // - intercharacter spacing = 0x00010000 (= 1.0 in 16.16 Fixed)
    // - total extra            = 0x000A0000 (= 10.0)
    // The v2 builder writes the same byte sequence after a 2-byte opcode
    // word (00 2D instead of v1's 2D), so we strip the v1 1-byte opcode
    // and compare against the v2 form here.
    let bytes = build_line_justify(0x0001_0000, 0x000A_0000);
    let expected: &[u8] = &[
        0x00, 0x2D, 0x00, 0x08, 0x00, 0x01, 0x00, 0x00, 0x00, 0x0A, 0x00, 0x00,
    ];
    assert_eq!(bytes, expected);
}

#[test]
fn build_glyph_state_layout_four_flags_plus_pad() {
    // outline_preferred = true, preserve_glyph = false,
    // fractional_widths = true, scaling_disabled = false.
    let bytes = build_glyph_state(true, false, true, false);
    let expected: &[u8] = &[
        0x00, 0x2E, // opcode
        0x00, 0x06, // dataLength = 6 (4 bools + 2 pad)
        0x01, 0x00, 0x01, 0x00, // outline / preserve / fractional / scaling
        0x00, 0x00, // pad
    ];
    assert_eq!(bytes, expected);
}

#[test]
fn build_glyph_state_all_zero_flags() {
    let bytes = build_glyph_state(false, false, false, false);
    assert_eq!(
        bytes,
        [0x00, 0x2E, 0x00, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]
    );
}

// ---------------------------------------------------------------------------
// Round-trip via the PictBuilder + parse_pict path.
// ---------------------------------------------------------------------------

fn paint_canvas(b: &mut PictBuilder) {
    b.fg_color(0, 0, 0).rect(Verb::Paint, 0, 0, 4, 4);
}

#[test]
fn pict_image_carries_font_name_after_font_name_opcode() {
    let mut b = PictBuilder::new(0, 0, 4, 4);
    b.font_name(0x0102, b"Geneva").expect("emit fontName");
    paint_canvas(&mut b);
    let bytes = b.finish();

    let img = parse_pict(&bytes).expect("decode");
    let font_name = img.text_state.font_name.as_ref().expect("font_name set");
    assert_eq!(font_name.old_font_id, 0x0102);
    assert_eq!(font_name.name, b"Geneva");
    // Rasterisation is unaffected by these passive state opcodes — the
    // canvas still has the paint-rect from `paint_canvas`.
    assert_eq!(img.width, 4);
    assert_eq!(img.height, 4);
    assert_eq!(&img.data[0..4], &[0x00, 0x00, 0x00, 0xFF]);
}

#[test]
fn pict_image_carries_line_justify_after_line_justify_opcode() {
    let mut b = PictBuilder::new(0, 0, 4, 4);
    // 1.5 pixel intercharacter + 7.25 pixel total extra.
    b.line_justify(0x0001_8000, 0x0007_4000);
    paint_canvas(&mut b);
    let bytes = b.finish();

    let img = parse_pict(&bytes).expect("decode");
    let lj = img.text_state.line_justify.expect("line_justify set");
    assert_eq!(
        lj,
        PictLineJustify {
            inter_char_spacing: Fixed(0x0001_8000),
            total_extra: Fixed(0x0007_4000),
        }
    );
    assert!((lj.inter_char_spacing.to_f32() - 1.5).abs() < 1e-6);
    assert!((lj.total_extra.to_f32() - 7.25).abs() < 1e-6);
}

#[test]
fn pict_image_carries_glyph_state_after_glyph_state_opcode() {
    let mut b = PictBuilder::new(0, 0, 4, 4);
    b.glyph_state(true, false, true, false);
    paint_canvas(&mut b);
    let bytes = b.finish();

    let img = parse_pict(&bytes).expect("decode");
    let gs = img.text_state.glyph_state.expect("glyph_state set");
    assert_eq!(
        gs,
        PictGlyphState {
            outline_preferred: true,
            preserve_glyph: false,
            fractional_widths: true,
            scaling_disabled: false,
        }
    );
}

#[test]
fn last_opcode_wins_when_emitted_multiple_times() {
    let mut b = PictBuilder::new(0, 0, 4, 4);
    b.font_name(1, b"first").expect("emit first");
    b.font_name(99, b"replacement").expect("emit second");
    b.glyph_state(true, true, true, true);
    b.glyph_state(false, false, false, false);
    paint_canvas(&mut b);
    let bytes = b.finish();

    let img = parse_pict(&bytes).expect("decode");
    let fn_ = img.text_state.font_name.as_ref().expect("font_name");
    assert_eq!(fn_.old_font_id, 99);
    assert_eq!(fn_.name, b"replacement");
    let gs = img.text_state.glyph_state.expect("glyph_state");
    assert!(!gs.outline_preferred);
    assert!(!gs.preserve_glyph);
    assert!(!gs.fractional_widths);
    assert!(!gs.scaling_disabled);
}

#[test]
fn pict_text_font_name_struct_constructor() {
    let f = PictFontName::new(7, b"Helvetica".to_vec());
    assert_eq!(f.old_font_id, 7);
    assert_eq!(f.name, b"Helvetica");
}

// ---------------------------------------------------------------------------
// Probe parity — the read-only walker captures the same final state.
// ---------------------------------------------------------------------------

#[test]
fn probe_text_state_mirrors_decoder() {
    let mut b = PictBuilder::new(0, 0, 4, 4);
    b.font_name(0x0202, b"Monaco").expect("emit fontName");
    b.line_justify(0x0002_0000, 0x0004_0000);
    b.glyph_state(false, true, false, true);
    paint_canvas(&mut b);
    let bytes = b.finish();

    let img = parse_pict(&bytes).expect("decode");
    let p = probe_pict(&bytes).expect("probe");
    assert_eq!(p.text_state, img.text_state);
}

#[test]
fn probe_text_state_op_count_includes_round236_opcodes() {
    let mut b = PictBuilder::new(0, 0, 4, 4);
    b.font_name(1, b"X").expect("emit fontName");
    b.line_justify(0, 0);
    b.glyph_state(true, false, true, false);
    paint_canvas(&mut b);
    let bytes = b.finish();

    let p = probe_pict(&bytes).expect("probe");
    // Three round-236 opcodes — count starts at zero in fresh_graf_port.
    assert_eq!(p.text_state_op_count, 3);
}

#[test]
fn probe_text_state_op_count_zero_when_no_state_opcode_emitted() {
    let mut b = PictBuilder::new(0, 0, 4, 4);
    paint_canvas(&mut b);
    let bytes = b.finish();

    let p = probe_pict(&bytes).expect("probe");
    assert_eq!(p.text_state_op_count, 0);
    assert!(p.text_state.font_name.is_none());
    assert!(p.text_state.line_justify.is_none());
    assert!(p.text_state.glyph_state.is_none());
}

// ---------------------------------------------------------------------------
// Invalid-stream rejection.
// ---------------------------------------------------------------------------

/// Manually-assembled v2 stream carrying a `fontName` opcode whose
/// `dataLength` word lies below the 5-byte minimum (length + oldFontID +
/// nameLen = 2 + 2 + 1). The decoder must reject this rather than silently
/// consume garbage bytes.
#[test]
fn font_name_too_short_data_length_rejected() {
    // Build a complete v2 stream by hand: 512-byte stub + picture
    // record header + version stanza + headerOp (24 bytes) +
    // fontName-with-broken-length + OpEndPic.
    let mut bytes = vec![0u8; 512];
    // picSize placeholder + picFrame 0..4 0..4.
    bytes.extend_from_slice(&[0, 0]);
    bytes.extend_from_slice(&[0, 0, 0, 0, 0, 4, 0, 4]);
    // v2 sentinel + headerOp.
    bytes.extend_from_slice(&[0x00, 0x11, 0x02, 0xFF, 0x0C, 0x00]);
    bytes.extend_from_slice(&[0u8; 24]);
    // fontName opcode with dataLength = 3 (illegal, must be ≥ 5).
    bytes.extend_from_slice(&[0x00, 0x2C, 0x00, 0x03, 0x00, 0x00, 0x00]);
    // OpEndPic.
    bytes.extend_from_slice(&[0x00, 0xFF]);

    let err = parse_pict(&bytes).expect_err("must reject");
    assert!(matches!(err, PictError::InvalidData { .. }));
}
