//! Round 224 — Picture Comments (`ShortComment` `$00A0` / `LongComment`
//! `$00A1` for v2; `$A0` / `$A1` for v1) are captured as structured
//! [`PictComment`] records on [`PictImage::comments`] and
//! [`PictProbe::comments`] instead of being silently skipped.
//!
//! Inside Macintosh: Imaging With QuickDraw §A-3 Table A-2 (v2) and
//! Table A-3 (v1) describe the on-disk records:
//!
//! * `ShortComment` — opcode + 2-byte `Kind (Integer)`.
//! * `LongComment` — opcode + 2-byte `Kind` + 2-byte `size` + `size`
//!   raw data bytes.
//!
//! The decoder folds them into a `Vec<PictComment>` carrying the
//! [`PictComment::kind`] / [`PictComment::data`] / [`PictComment::is_long`]
//! triplet in stream order; the encoder helpers
//! ([`build_short_comment`] / [`build_long_comment`] +
//! [`PictBuilder::short_comment`] / [`PictBuilder::long_comment`]) round-trip
//! every emit through the decoder bit-for-bit.

use oxideav_pict::ops::{PictBuilder, Verb};
use oxideav_pict::{
    build_long_comment, build_long_comment_v1, build_short_comment, build_short_comment_v1,
    parse_pict, probe_pict, PictComment, PictError,
};

// ---------------------------------------------------------------------------
// PictComment record helpers.
// ---------------------------------------------------------------------------

#[test]
fn pict_comment_short_constructor_sets_flag_and_empty_data() {
    let c = PictComment::short(0x00C8);
    assert_eq!(c.kind, 0x00C8);
    assert!(c.data.is_empty());
    assert!(!c.is_long);
}

#[test]
fn pict_comment_long_constructor_sets_flag_and_data() {
    let c = PictComment::long(150, b"PostScriptBegin".to_vec());
    assert_eq!(c.kind, 150);
    assert_eq!(c.data, b"PostScriptBegin");
    assert!(c.is_long);
}

// ---------------------------------------------------------------------------
// build_short_comment / build_long_comment byte-layout assertions.
// ---------------------------------------------------------------------------

#[test]
fn build_short_comment_emits_opcode_plus_kind_word() {
    let bytes = build_short_comment(0x1234);
    assert_eq!(bytes, [0x00, 0xA0, 0x12, 0x34]);
}

#[test]
fn build_long_comment_emits_opcode_kind_size_data() {
    let bytes = build_long_comment(0x00C8, &[0xDE, 0xAD, 0xBE, 0xEF]).expect("build_long_comment");
    assert_eq!(
        bytes,
        [0x00, 0xA1, 0x00, 0xC8, 0x00, 0x04, 0xDE, 0xAD, 0xBE, 0xEF]
    );
}

#[test]
fn build_long_comment_rejects_oversized_payload() {
    // 65536 bytes overflows the u16 `size` field per §A-3.
    let data = vec![0u8; 65536];
    let err = build_long_comment(0x100, &data).expect_err("must reject u16 overflow");
    match err {
        PictError::InvalidData(msg) => {
            assert!(
                msg.contains("65535") || msg.contains("65536"),
                "expected size-field overflow message, got: {msg}"
            );
        }
        other => panic!("expected InvalidData, got {other:?}"),
    }
}

#[test]
fn build_long_comment_accepts_max_u16_size() {
    let data = vec![0xABu8; 65535];
    let bytes = build_long_comment(0x0099, &data).expect("max u16 size accepted");
    assert_eq!(bytes.len(), 2 + 2 + 2 + 65535);
    assert_eq!(&bytes[..6], &[0x00, 0xA1, 0x00, 0x99, 0xFF, 0xFF]);
    assert_eq!(bytes[6], 0xAB);
    assert_eq!(*bytes.last().unwrap(), 0xAB);
}

#[test]
fn build_short_comment_v1_emits_one_byte_opcode() {
    let bytes = build_short_comment_v1(0x00C8);
    assert_eq!(bytes, [0xA0, 0x00, 0xC8]);
}

#[test]
fn build_long_comment_v1_emits_one_byte_opcode() {
    let bytes = build_long_comment_v1(0x00C8, &[0x11, 0x22]).expect("build_long_comment_v1");
    assert_eq!(bytes, [0xA1, 0x00, 0xC8, 0x00, 0x02, 0x11, 0x22]);
}

// ---------------------------------------------------------------------------
// PictBuilder round-trip — emit comments, decode, assert kind / data.
// ---------------------------------------------------------------------------

#[test]
fn builder_short_comment_round_trips_through_decoder() {
    // PICT must produce a raster for the decoder to return Ok; a
    // 4x4 filled rect suffices to take the canvas off the NoRaster
    // path.
    let mut b = PictBuilder::new(0, 0, 4, 4);
    b.short_comment(0x1234);
    b.fg_color(0, 0, 0);
    b.rect(Verb::Paint, 0, 0, 4, 4);
    let bytes = b.finish();
    let img = parse_pict(&bytes).expect("decode");
    assert_eq!(img.comments.len(), 1);
    assert_eq!(img.comments[0].kind, 0x1234);
    assert!(!img.comments[0].is_long);
    assert!(img.comments[0].data.is_empty());
}

#[test]
fn builder_long_comment_round_trips_through_decoder() {
    let payload = b"PostScriptHandle:8,0,72".to_vec();
    let mut b = PictBuilder::new(0, 0, 4, 4);
    b.long_comment(150, &payload).expect("emit long comment");
    b.fg_color(0, 0, 0);
    b.rect(Verb::Paint, 0, 0, 4, 4);
    let bytes = b.finish();
    let img = parse_pict(&bytes).expect("decode");
    assert_eq!(img.comments.len(), 1);
    let c = &img.comments[0];
    assert_eq!(c.kind, 150);
    assert!(c.is_long);
    assert_eq!(c.data, payload);
}

#[test]
fn builder_multiple_comments_preserve_stream_order() {
    let mut b = PictBuilder::new(0, 0, 4, 4);
    b.short_comment(100);
    b.long_comment(150, b"first").unwrap();
    b.short_comment(101);
    b.long_comment(151, b"second-longer").unwrap();
    b.short_comment(102);
    b.fg_color(0, 0, 0);
    b.rect(Verb::Paint, 0, 0, 4, 4);
    let bytes = b.finish();
    let img = parse_pict(&bytes).expect("decode");

    assert_eq!(img.comments.len(), 5);
    assert_eq!(img.comments[0], PictComment::short(100));
    assert_eq!(img.comments[1], PictComment::long(150, b"first".to_vec()));
    assert_eq!(img.comments[2], PictComment::short(101));
    assert_eq!(
        img.comments[3],
        PictComment::long(151, b"second-longer".to_vec())
    );
    assert_eq!(img.comments[4], PictComment::short(102));
}

#[test]
fn long_comment_with_odd_size_decodes_under_word_alignment() {
    // LongComment carrying 1 data byte ends at an odd offset within
    // the picture record. The builder must pad to a 2-byte boundary
    // before the next opcode so the rectangle that follows isn't
    // mis-aligned.
    let mut b = PictBuilder::new(0, 0, 4, 4);
    b.long_comment(0x00C8, &[0x42]).unwrap();
    b.fg_color(0, 0, 0);
    b.rect(Verb::Paint, 0, 0, 4, 4);
    let bytes = b.finish();
    let img = parse_pict(&bytes).expect("decode after odd LongComment");
    assert_eq!(img.width, 4);
    assert_eq!(img.comments.len(), 1);
    assert_eq!(img.comments[0].kind, 0x00C8);
    assert_eq!(img.comments[0].data, [0x42]);
    assert!(img.comments[0].is_long);
}

#[test]
fn empty_long_comment_round_trips_as_zero_data() {
    let mut b = PictBuilder::new(0, 0, 4, 4);
    b.long_comment(151, &[]).unwrap();
    b.fg_color(0, 0, 0);
    b.rect(Verb::Paint, 0, 0, 4, 4);
    let bytes = b.finish();
    let img = parse_pict(&bytes).expect("decode");
    assert_eq!(img.comments.len(), 1);
    assert_eq!(img.comments[0].kind, 151);
    assert!(img.comments[0].is_long);
    assert!(img.comments[0].data.is_empty());
}

// ---------------------------------------------------------------------------
// Probe surface — counter stays in sync with `comments.len()`.
// ---------------------------------------------------------------------------

#[test]
fn probe_captures_short_and_long_comment_payloads() {
    let mut b = PictBuilder::new(0, 0, 4, 4);
    b.short_comment(0xABCD);
    b.long_comment(0x1234, b"meta").unwrap();
    b.fg_color(0, 0, 0);
    b.rect(Verb::Paint, 0, 0, 4, 4);
    let bytes = b.finish();
    let p = probe_pict(&bytes).expect("probe");

    assert_eq!(p.comment_count, 2);
    assert_eq!(p.comments.len(), 2);
    assert_eq!(p.comments[0].kind, 0xABCD);
    assert!(!p.comments[0].is_long);
    assert_eq!(p.comments[1].kind, 0x1234);
    assert_eq!(p.comments[1].data, b"meta");
    assert!(p.comments[1].is_long);
}

#[test]
fn probe_comment_data_matches_decoder_surface() {
    let mut b = PictBuilder::new(0, 0, 4, 4);
    b.long_comment(150, b"PostScript fragment").unwrap();
    b.fg_color(0, 0, 0);
    b.rect(Verb::Paint, 0, 0, 4, 4);
    let bytes = b.finish();
    let img = parse_pict(&bytes).expect("decode");
    let p = probe_pict(&bytes).expect("probe");
    assert_eq!(img.comments, p.comments);
}

// ---------------------------------------------------------------------------
// v1 dispatcher — same record shape via 1-byte opcodes.
// ---------------------------------------------------------------------------

#[test]
fn v1_short_comment_decodes_into_image_comments() {
    // Hand-roll a minimal v1 PICT: picSize(2) + picFrame(8) + 0x1101
    // version stanza + opcodes + 0xFF OpEndPic. The picture frame is
    // 4x4 and we paint it solid to take the NoRaster branch out of
    // the way.
    let mut body: Vec<u8> = Vec::new();
    body.extend_from_slice(&0u16.to_be_bytes()); // picSize placeholder
    body.extend_from_slice(&0i16.to_be_bytes()); // top
    body.extend_from_slice(&0i16.to_be_bytes()); // left
    body.extend_from_slice(&4i16.to_be_bytes()); // bottom
    body.extend_from_slice(&4i16.to_be_bytes()); // right
    body.extend_from_slice(&[0x11, 0x01]); // versionOp + version
                                           // ShortComment 0xA0 + kind 0x1234.
    body.extend_from_slice(&build_short_comment_v1(0x1234));
    // LongComment 0xA1 + kind 0x00C8 + size 5 + "hello".
    body.extend_from_slice(&build_long_comment_v1(0x00C8, b"hello").unwrap());
    // RGBFgCol — paint primitive needs a colour; v1 default fg is
    // black so we can skip and rely on the default.
    // paintRect (v1 opcode 0x31) over the full frame to take the
    // canvas off the NoRaster path.
    body.push(0x31);
    body.extend_from_slice(&0i16.to_be_bytes()); // top
    body.extend_from_slice(&0i16.to_be_bytes()); // left
    body.extend_from_slice(&4i16.to_be_bytes()); // bottom
    body.extend_from_slice(&4i16.to_be_bytes()); // right
    body.push(0xFF); // OpEndPic

    let img = parse_pict(&body).expect("v1 decode");
    assert_eq!(img.width, 4);
    assert_eq!(img.height, 4);
    assert_eq!(img.comments.len(), 2);
    assert_eq!(img.comments[0], PictComment::short(0x1234));
    assert_eq!(
        img.comments[1],
        PictComment::long(0x00C8, b"hello".to_vec())
    );
}

#[test]
fn v1_probe_captures_comments_same_as_decoder() {
    let mut body: Vec<u8> = Vec::new();
    body.extend_from_slice(&0u16.to_be_bytes());
    body.extend_from_slice(&0i16.to_be_bytes());
    body.extend_from_slice(&0i16.to_be_bytes());
    body.extend_from_slice(&4i16.to_be_bytes());
    body.extend_from_slice(&4i16.to_be_bytes());
    body.extend_from_slice(&[0x11, 0x01]);
    body.extend_from_slice(&build_short_comment_v1(0xAA00));
    body.extend_from_slice(&build_long_comment_v1(0xAA01, b"v1-payload").unwrap());
    // paintRect over the whole frame.
    body.push(0x31);
    body.extend_from_slice(&0i16.to_be_bytes());
    body.extend_from_slice(&0i16.to_be_bytes());
    body.extend_from_slice(&4i16.to_be_bytes());
    body.extend_from_slice(&4i16.to_be_bytes());
    body.push(0xFF);

    let p = probe_pict(&body).expect("v1 probe");
    assert_eq!(p.comment_count, 2);
    assert_eq!(p.comments.len(), 2);
    assert_eq!(p.comments[0].kind, 0xAA00);
    assert!(!p.comments[0].is_long);
    assert_eq!(p.comments[1].kind, 0xAA01);
    assert_eq!(p.comments[1].data, b"v1-payload");
    assert!(p.comments[1].is_long);
}

// ---------------------------------------------------------------------------
// Decoder is tolerant of a LongComment with a `size` that consumes the
// remaining stream (truncation surface).
// ---------------------------------------------------------------------------

#[test]
fn long_comment_with_truncated_payload_errors_cleanly() {
    // LongComment claims 8 data bytes but only provides 4. The
    // decoder must surface an InvalidData error rather than panic.
    let mut b = PictBuilder::new(0, 0, 4, 4);
    b.fg_color(0, 0, 0);
    b.rect(Verb::Paint, 0, 0, 4, 4);
    let mut bytes = b.finish();

    // Splice a truncated LongComment in just before OpEndPic.
    let end = bytes.len() - 2; // OpEndPic is the trailing 2 bytes (word-aligned)
    let mut truncated = Vec::new();
    truncated.extend_from_slice(&[0x00, 0xA1]); // opcode
    truncated.extend_from_slice(&[0x00, 0xC8]); // kind
    truncated.extend_from_slice(&[0x00, 0x08]); // size=8
    truncated.extend_from_slice(&[0x01, 0x02, 0x03, 0x04]); // only 4 bytes of data
                                                            // Splice in before OpEndPic.
    bytes.splice(end..end, truncated.iter().copied());

    let err = parse_pict(&bytes).expect_err("truncated LongComment must error");
    match err {
        PictError::InvalidData(_) => {}
        other => panic!("expected InvalidData, got {other:?}"),
    }
}
