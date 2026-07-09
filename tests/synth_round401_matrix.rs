//! Round 401 — v1 ↔ v2 emission matrix.
//!
//! [`PictV1Builder`] assembles a version 1 stream (§A-3 Table A-3:
//! 1-byte opcodes, no word alignment, 10-byte record header, `$11 $01`
//! stanza, `$FF` terminator) from the same `build_*` chunks as the v2
//! [`PictBuilder`]. Table A-3 is numbering-compatible with the v2
//! table, so a drawing sequence emitted through both builders must
//! decode to a **pixel-identical** canvas — that equivalence, across
//! every v1-legal opcode family the crate can emit, is the pin here.

use oxideav_pict::ops::{PictBuilder, PictV1Builder};
use oxideav_pict::{
    build_arc_op, build_bk_color_code, build_dh_text, build_dhdv_text, build_dv_text,
    build_fg_color_code, build_fill_pat, build_line, build_line_from, build_long_comment_v1,
    build_long_text, build_origin, build_oval_op, build_oval_size, build_pn_pat, build_pn_size,
    build_poly_op, build_rect_op, build_rgb_fg_col, build_rgn_rect_op, build_round_rect_op,
    build_same_arc_op, build_same_rect_op, build_short_comment_v1, build_short_line,
    build_short_line_from, build_tx_size, parse_pict, probe_pict, ProbeVersion, Verb,
};

/// The shared drawing sequence: one chunk per v1-legal opcode family
/// the crate can emit. Chunks are in the v2 (`0x00`-high-byte) form.
fn shared_sequence() -> Vec<Vec<u8>> {
    vec![
        build_fg_color_code(205),         // classic red
        build_bk_color_code(30),          // classic white
        build_pn_size(2, 2),              // pen 2×2
        build_line(2, 2, 20, 10),         // Line
        build_line_from(26, 6),           // LineFrom
        build_short_line(30, 30, 12, -4), // ShortLine
        build_short_line_from(-6, 8),     // ShortLineFrom
        build_pn_pat([0xAA, 0x55, 0xAA, 0x55, 0xAA, 0x55, 0xAA, 0x55]),
        build_rect_op(Verb::Paint, 40, 4, 52, 16), // Rect
        build_same_rect_op(Verb::Invert),          // SameRect
        build_oval_size(4, 4),
        build_round_rect_op(Verb::Frame, 40, 20, 56, 40), // RRect
        build_oval_op(Verb::Paint, 4, 40, 18, 58),        // Oval
        build_arc_op(Verb::Paint, 20, 40, 40, 60, 0, 90), // Arc
        build_same_arc_op(Verb::Paint, 90, 120),          // SameArc
        build_poly_op(Verb::Fill, &[(6, 56), (18, 60), (10, 62)]).unwrap(),
        build_fill_pat([0xFF; 8]),
        build_rgn_rect_op(Verb::Fill, 56, 44, 62, 60), // Rgn
        build_origin(-2, -3),                          // shift down/right
        build_rect_op(Verb::Paint, 54, 2, 58, 6),      // drawn at +3/+2
        build_origin(2, 3),                            // restore
        build_tx_size(8),
        build_long_text(4, 34, b"Hi").unwrap(), // LongText
        build_dh_text(2, b"!").unwrap(),        // DHText
        build_dv_text(9, b"v").unwrap(),        // DVText
        build_dhdv_text(1, 1, b".").unwrap(),   // DHDVText
        build_short_comment_v1(0x0042)[..].to_vec(), // placeholder, replaced below
    ]
}

/// Comments differ in build helper (v1 helpers emit the 1-byte form
/// directly), so the matrix pushes them separately.
#[test]
fn v1_and_v2_streams_decode_pixel_identically() {
    let seq = {
        let mut s = shared_sequence();
        s.pop(); // drop the placeholder comment chunk
        s
    };

    let mut v2 = PictBuilder::new(0, 0, 64, 64);
    for chunk in &seq {
        v2.push(chunk);
    }
    v2.short_comment(0x0042);
    v2.long_comment(0x0043, b"matrix").unwrap();
    let v2_bytes = v2.finish();

    let mut v1 = PictV1Builder::new(0, 0, 64, 64);
    for chunk in &seq {
        v1.push(chunk).unwrap();
    }
    // The v1 comment helpers already emit the 1-byte-opcode form; they
    // bypass `push`'s v2→v1 conversion by design, so splice them raw.
    let mut v1_bytes = v1.finish();
    let end = v1_bytes.len() - 1; // before the $FF terminator
    let mut tail = build_short_comment_v1(0x0042);
    tail.extend_from_slice(&build_long_comment_v1(0x0043, b"matrix").unwrap());
    v1_bytes.splice(end..end, tail);

    let i2 = parse_pict(&v2_bytes).unwrap();
    let i1 = parse_pict(&v1_bytes).unwrap();
    assert_eq!(i1.width, i2.width);
    assert_eq!(
        i1.data, i2.data,
        "v1 and v2 emissions of the same sequence must rasterise identically"
    );
    // Sanity: the sequence really inked the canvas.
    assert!(i1.data.chunks_exact(4).any(|px| px[0] != 0xFF));

    // Probe agrees on the version split and sees the comments in both.
    let p2 = probe_pict(&v2_bytes).unwrap();
    let p1 = probe_pict(&v1_bytes).unwrap();
    assert_eq!(p2.version, ProbeVersion::V2);
    assert_eq!(p1.version, ProbeVersion::V1);
    assert_eq!(p1.comment_count, 2);
    assert_eq!(p2.comment_count, 2);
    assert_eq!(p1.text_count, 4);
    assert_eq!(p2.text_count, 4);
    assert_eq!(p1.drawing_count, p2.drawing_count);
}

// ---------------------------------------------------------------------------
// The v1 builder refuses Color-QuickDraw-only opcodes at build time.
// ---------------------------------------------------------------------------

#[test]
fn v1_builder_rejects_color_quickdraw_opcodes() {
    let mut b = PictV1Builder::new(0, 0, 32, 32);
    // RGBFgCol ($001A) postdates version 1.
    assert!(b.push(&build_rgb_fg_col(255, 0, 0)).is_err());
    // Chunks shorter than an opcode word are malformed.
    assert!(b.push(&[0x00]).is_err());
    // A high opcode byte means a v2-only opcode value.
    assert!(b.push(&[0x0C, 0x00, 0, 0]).is_err());
    // The classic colour codes are the v1 way.
    assert!(b.push(&build_fg_color_code(205)).is_ok());
}

// ---------------------------------------------------------------------------
// picSize: patched with the real record length when it fits.
// ---------------------------------------------------------------------------

#[test]
fn v1_pic_size_word_records_the_stream_length() {
    let mut b = PictV1Builder::new(0, 0, 16, 16);
    b.push(&build_rect_op(Verb::Paint, 2, 2, 10, 10)).unwrap();
    let bytes = b.finish();
    let pic_size = u16::from_be_bytes([bytes[0], bytes[1]]) as usize;
    assert_eq!(pic_size, bytes.len());
    // And the stream decodes.
    let img = parse_pict(&bytes).unwrap();
    assert_eq!(img.width, 16);
}

// ---------------------------------------------------------------------------
// Classic colour codes ink identically through both walkers.
// ---------------------------------------------------------------------------

#[test]
fn classic_colour_codes_ink_both_versions() {
    let paint = build_rect_op(Verb::Paint, 4, 4, 12, 12);
    for (code, rgb) in [
        (205u32, [255u8, 0, 0]),
        (409, [0, 0, 255]),
        (341, [0, 255, 0]),
    ] {
        let mut v2 = PictBuilder::new(0, 0, 16, 16);
        v2.push(&build_fg_color_code(code));
        v2.push(&paint);
        let mut v1 = PictV1Builder::new(0, 0, 16, 16);
        v1.push(&build_fg_color_code(code)).unwrap();
        v1.push(&paint).unwrap();
        for bytes in [v2.finish(), v1.finish()] {
            let img = parse_pict(&bytes).unwrap();
            let off = (5 * 16 + 5) * 4;
            assert_eq!(&img.data[off..off + 3], &rgb, "code {code}");
        }
    }
}
