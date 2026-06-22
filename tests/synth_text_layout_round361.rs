//! Round 361 — `TxRatio` ($0010) glyph scaling + `lineJustify` ($002D)
//! intercharacter spacing now reach the text rasteriser.
//!
//! Both opcodes were captured into the drawing state through earlier
//! rounds but discarded at draw time: glyphs scaled isotropically by
//! `txSize` and the pen advanced by `chExtra`/`spExtra` only. Round 361
//! applies them per Imaging With QuickDraw:
//!
//! * `TxRatio` (book page 12-13): `numer.h/denom.h` is the horizontal
//!   glyph-cell scale, `numer.v/denom.v` the vertical.
//! * `lineJustify` (§A-3 footnote `†`): the intercharacter spacing is
//!   added to *every* character's advance.
//!
//! These tests drive the full `parse_pict` path and inspect the rendered
//! RGBA canvas + final text pen.

use oxideav_pict::ops::PictBuilder;
use oxideav_pict::{build_line_justify, build_tx_ratio, build_tx_size, parse_pict, PictImage};

/// A `LongText` opcode body: `$0028`, `txLoc (v, h)`, `count`, `text`.
fn long_text(v: i16, h: i16, text: &[u8]) -> Vec<u8> {
    let mut b = vec![0x00, 0x28];
    b.extend_from_slice(&v.to_be_bytes());
    b.extend_from_slice(&h.to_be_bytes());
    b.push(text.len() as u8);
    b.extend_from_slice(text);
    b
}

/// True if pixel `(x, y)` in `img` is (near-)black ink.
fn is_ink(img: &PictImage, x: u32, y: u32) -> bool {
    if x >= img.width || y >= img.height {
        return false;
    }
    let off = ((y * img.width + x) * 4) as usize;
    let (r, g, b) = (img.data[off], img.data[off + 1], img.data[off + 2]);
    r < 40 && g < 40 && b < 40
}

/// The rightmost canvas column carrying ink (None when no ink at all).
fn max_inked_x(img: &PictImage) -> Option<u32> {
    let mut found = None;
    for y in 0..img.height {
        for x in 0..img.width {
            if is_ink(img, x, y) {
                found = Some(found.map_or(x, |m: u32| m.max(x)));
            }
        }
    }
    found
}

/// The topmost canvas row carrying ink (None when no ink at all).
fn min_inked_y(img: &PictImage) -> Option<u32> {
    for y in 0..img.height {
        for x in 0..img.width {
            if is_ink(img, x, y) {
                return Some(y);
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// TxRatio horizontal stretch widens the inked glyph run + the text pen.
// ---------------------------------------------------------------------------

#[test]
fn tx_ratio_horizontal_stretch_widens_run() {
    // Baseline: native-scale (txSize == design em 8), default 1/1 ratio.
    let mut base = PictBuilder::new(0, 0, 160, 40);
    base.push(&build_tx_size(8));
    base.push(&long_text(20, 4, b"HII"));
    let base_img = parse_pict(&base.finish()).unwrap();
    let base_pen = base_img.text_state.text_pen.unwrap();
    let base_right = max_inked_x(&base_img).expect("baseline drew ink");

    // 2/1 horizontal TxRatio: numer.v=1 numer.h=2 denom.v=1 denom.h=1.
    let mut wide = PictBuilder::new(0, 0, 160, 40);
    wide.push(&build_tx_size(8));
    wide.push(&build_tx_ratio(1, 2, 1, 1));
    wide.push(&long_text(20, 4, b"HII"));
    let wide_img = parse_pict(&wide.finish()).unwrap();
    let wide_pen = wide_img.text_state.text_pen.unwrap();
    let wide_right = max_inked_x(&wide_img).expect("wide drew ink");

    // The pen advance from the same left pen (h=4) roughly doubles.
    let base_adv = base_pen.0 - 4;
    let wide_adv = wide_pen.0 - 4;
    assert_eq!(wide_adv, 2 * base_adv, "2/1 ratio should double advance");

    // The inked run extends further to the right than the baseline.
    assert!(
        wide_right > base_right,
        "wide run right={wide_right} should exceed base right={base_right}"
    );
}

// ---------------------------------------------------------------------------
// TxRatio vertical stretch raises glyph tops without changing the advance.
// ---------------------------------------------------------------------------

#[test]
fn tx_ratio_vertical_stretch_raises_tops_not_advance() {
    let mut base = PictBuilder::new(0, 0, 160, 60);
    base.push(&build_tx_size(8));
    base.push(&long_text(40, 4, b"HII"));
    let base_img = parse_pict(&base.finish()).unwrap();
    let base_pen = base_img.text_state.text_pen.unwrap();
    let base_top = min_inked_y(&base_img).expect("baseline drew ink");

    // 2/1 vertical TxRatio: numer.v=2 numer.h=1 denom.v=1 denom.h=1.
    let mut tall = PictBuilder::new(0, 0, 160, 60);
    tall.push(&build_tx_size(8));
    tall.push(&build_tx_ratio(2, 1, 1, 1));
    tall.push(&long_text(40, 4, b"HII"));
    let tall_img = parse_pict(&tall.finish()).unwrap();
    let tall_pen = tall_img.text_state.text_pen.unwrap();
    let tall_top = min_inked_y(&tall_img).expect("tall drew ink");

    // Vertical scaling does not change the horizontal advance.
    assert_eq!(
        tall_pen.0, base_pen.0,
        "vertical ratio must not move the horizontal pen"
    );
    // The baseline stays at v=40; a taller cell reaches higher (smaller y).
    assert!(
        tall_top < base_top,
        "tall top={tall_top} should sit above base top={base_top}"
    );
}

// ---------------------------------------------------------------------------
// lineJustify intercharacter spacing pads every glyph's advance.
// ---------------------------------------------------------------------------

#[test]
fn line_justify_inter_char_pads_every_advance() {
    let text = b"HII";
    let mut base = PictBuilder::new(0, 0, 160, 40);
    base.push(&build_tx_size(8));
    base.push(&long_text(20, 4, text));
    let base_img = parse_pict(&base.finish()).unwrap();
    let base_pen = base_img.text_state.text_pen.unwrap();

    // intercharacter spacing = 3.0 (Fixed 16.16), total extra = 0.
    let mut just = PictBuilder::new(0, 0, 160, 40);
    just.push(&build_tx_size(8));
    just.push(&build_line_justify(3 << 16, 0));
    just.push(&long_text(20, 4, text));
    let just_img = parse_pict(&just.finish()).unwrap();
    let just_pen = just_img.text_state.text_pen.unwrap();

    // Every one of the 3 characters gains 3 px of advance.
    assert_eq!(
        just_pen.0,
        base_pen.0 + (text.len() as i32) * 3,
        "interChar 3 should add 3px per char to the advance"
    );
}

// ---------------------------------------------------------------------------
// A 1/1 TxRatio with no lineJustify is a no-op (regression guard).
// ---------------------------------------------------------------------------

#[test]
fn identity_ratio_no_justify_matches_plain_draw() {
    let mut plain = PictBuilder::new(0, 0, 120, 40);
    plain.push(&build_tx_size(8));
    plain.push(&long_text(20, 4, b"AB"));
    let plain_img = parse_pict(&plain.finish()).unwrap();

    let mut ident = PictBuilder::new(0, 0, 120, 40);
    ident.push(&build_tx_size(8));
    ident.push(&build_tx_ratio(1, 1, 1, 1));
    ident.push(&long_text(20, 4, b"AB"));
    let ident_img = parse_pict(&ident.finish()).unwrap();

    assert_eq!(
        plain_img.text_state.text_pen, ident_img.text_state.text_pen,
        "1/1 ratio must not change the pen"
    );
    assert_eq!(
        plain_img.data, ident_img.data,
        "1/1 ratio must render identically to no ratio"
    );
}
