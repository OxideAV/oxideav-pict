//! Round 407 — `txFace` style synthesis is rasterised.
//!
//! The re-staged Inside Macintosh Volume I (1985) specifies the classic
//! QuickDraw style treatments (QuickDraw chapter pages I-151/I-152) and
//! the Font Manager's screen characterization-table amounts (page
//! I-226 Figure 4): bold smears one pixel right (+1 advance), italic
//! shears above-baseline rows right / below-baseline rows left,
//! underline draws offset-1/thickness-1 below the baseline gapping one
//! pixel around descender ink, outline hollows the character behind a
//! 1-pixel ring (+1 advance), shadow thickens that ring below and to
//! the right (+2 advance), and condense / extend tighten / widen every
//! character's advance by one pixel.
//!
//! These tests drive full PICT v2 streams (`TxFace $0004` + `LongText
//! $0028`) through `parse_pict` and inspect the rendered RGBA canvas.

use oxideav_pict::font::{measure_text, TextScale};
use oxideav_pict::ops::PictBuilder;
use oxideav_pict::{build_tx_face, build_tx_size, parse_pict, PictImage, PictTextFace};

/// A `LongText` opcode body: `$0028`, `txLoc (v, h)`, `count`, `text`.
fn long_text(v: i16, h: i16, text: &[u8]) -> Vec<u8> {
    let mut b = vec![0x00, 0x28];
    b.extend_from_slice(&v.to_be_bytes());
    b.extend_from_slice(&h.to_be_bytes());
    b.push(text.len() as u8);
    b.extend_from_slice(text);
    b
}

/// Render `text` at native design scale with the given style byte; the
/// baseline pen sits at (h=8, v=16) on a 64-wide × 32-tall canvas
/// (`PictBuilder::new` takes `(top, left, bottom, right)`).
fn render(text: &[u8], face: u8) -> PictImage {
    let mut b = PictBuilder::new(0, 0, 32, 64);
    b.push(&build_tx_size(8));
    b.push(&build_tx_face(face));
    b.push(&long_text(16, 8, text));
    parse_pict(&b.finish()).unwrap()
}

/// The set of near-black ink pixels of `img`.
fn ink_set(img: &PictImage) -> std::collections::BTreeSet<(i32, i32)> {
    let mut s = std::collections::BTreeSet::new();
    for y in 0..img.height {
        for x in 0..img.width {
            let off = ((y * img.width + x) * 4) as usize;
            if img.data[off] < 40 && img.data[off + 1] < 40 && img.data[off + 2] < 40 {
                s.insert((x as i32, y as i32));
            }
        }
    }
    s
}

// ---------------------------------------------------------------------------
// Bold: the plain image plus a one-pixel-right smear, advance +1.
// ---------------------------------------------------------------------------

#[test]
fn bold_smears_and_widens_the_advance() {
    let plain = ink_set(&render(b"|", 0));
    let bold = ink_set(&render(b"|", PictTextFace::BOLD));
    let smear: std::collections::BTreeSet<_> = plain.iter().map(|&(x, y)| (x + 1, y)).collect();
    let expect: std::collections::BTreeSet<_> = plain.union(&smear).copied().collect();
    assert_eq!(bold, expect, "bold = plain ∪ (plain shifted +1 in x)");

    // The pen advance grows by the characterization table's extra (+1).
    let img = render(b"AB", PictTextFace::BOLD);
    let adv = measure_text(
        b"AB",
        TextScale::isotropic(8),
        0,
        0,
        0,
        PictTextFace::from(PictTextFace::BOLD),
    );
    assert_eq!(img.text_state.text_pen, Some((8 + adv, 16)));
    let plain_adv = measure_text(b"AB", TextScale::isotropic(8), 0, 0, 0, PictTextFace::PLAIN);
    assert_eq!(adv, plain_adv + 2);
}

// ---------------------------------------------------------------------------
// Italic: above-baseline rows shift right, the baseline row stays put.
// ---------------------------------------------------------------------------

#[test]
fn italic_shears_the_glyph() {
    let plain = ink_set(&render(b"|", 0));
    let italic = ink_set(&render(b"|", PictTextFace::ITALIC));
    assert_eq!(plain.len(), italic.len(), "shear moves ink, adds none");
    // Baseline row (v = 16) unmoved.
    let bp: Vec<_> = plain.iter().filter(|&&(_, y)| y == 16).collect();
    let bi: Vec<_> = italic.iter().filter(|&&(_, y)| y == 16).collect();
    assert_eq!(bp, bi);
    // Top authored row (v = 10) shifted right by (6·8)>>4 = 3.
    let tp: Vec<_> = plain.iter().filter(|&&(_, y)| y == 10).copied().collect();
    let ti: Vec<_> = italic.iter().filter(|&&(_, y)| y == 10).copied().collect();
    assert_eq!(ti, tp.iter().map(|&(x, y)| (x + 3, y)).collect::<Vec<_>>());
}

// ---------------------------------------------------------------------------
// Underline: one row below the baseline across the advance; the glyph
// above it is untouched.
// ---------------------------------------------------------------------------

#[test]
fn underline_row_spans_the_advance() {
    let plain = ink_set(&render(b"AB", 0));
    let under = ink_set(&render(b"AB", PictTextFace::UNDERLINE));
    assert!(plain.iter().all(|&(_, y)| y != 17), "plain has no y=17 ink");
    let row: Vec<_> = under
        .iter()
        .filter(|&&(_, y)| y == 17)
        .map(|&(x, _)| x)
        .collect();
    // Two chars × advance 6 starting at pen h=8: a continuous 8..=19.
    assert_eq!(row, (8..=19).collect::<Vec<_>>());
    // Everything except the underline row matches the plain rendering.
    let rest: std::collections::BTreeSet<_> =
        under.iter().copied().filter(|&(_, y)| y != 17).collect();
    assert_eq!(rest, plain);
}

// ---------------------------------------------------------------------------
// Underline also runs beneath spaces (it belongs to the whole run).
// ---------------------------------------------------------------------------

#[test]
fn underline_covers_spaces() {
    let under = ink_set(&render(b" ", PictTextFace::UNDERLINE));
    let row: Vec<_> = under
        .iter()
        .filter(|&&(_, y)| y == 17)
        .map(|&(x, _)| x)
        .collect();
    assert_eq!(row, (8..=13).collect::<Vec<_>>());
}

// ---------------------------------------------------------------------------
// Outline: hollow — the plain strokes are unpainted, a ring surrounds
// them.
// ---------------------------------------------------------------------------

#[test]
fn outline_hollows_the_character() {
    let plain = ink_set(&render(b"O", 0));
    let outline = ink_set(&render(b"O", PictTextFace::OUTLINE));
    assert!(!outline.is_empty());
    assert!(
        plain.is_disjoint(&outline),
        "outlined characters are hollow"
    );
    // Every ring pixel borders a stroke pixel (8-adjacency).
    for &(x, y) in &outline {
        assert!(
            plain
                .iter()
                .any(|&(px, py)| (px - x).abs() <= 1 && (py - y).abs() <= 1),
            "ring pixel ({x},{y}) not adjacent to the stroke"
        );
    }
}

// ---------------------------------------------------------------------------
// Shadow: a superset of the outline ring, thickened below/right, still
// hollow, advance +2.
// ---------------------------------------------------------------------------

#[test]
fn shadow_thickens_the_ring() {
    let plain = ink_set(&render(b"O", 0));
    let outline = ink_set(&render(b"O", PictTextFace::OUTLINE));
    let shadow = ink_set(&render(b"O", PictTextFace::SHADOW));
    assert!(outline.is_subset(&shadow));
    assert!(shadow.len() > outline.len());
    assert!(plain.is_disjoint(&shadow), "shadow stays hollow");

    let img = render(b"O", PictTextFace::SHADOW);
    let plain_adv = measure_text(b"O", TextScale::isotropic(8), 0, 0, 0, PictTextFace::PLAIN);
    assert_eq!(img.text_state.text_pen, Some((8 + plain_adv + 2, 16)));
}

// ---------------------------------------------------------------------------
// Condense / extend: advance −1 / +1 per character.
// ---------------------------------------------------------------------------

#[test]
fn condense_and_extend_move_the_pen() {
    let plain = render(b"MN", 0).text_state.text_pen.unwrap().0;
    let cond = render(b"MN", PictTextFace::CONDENSE)
        .text_state
        .text_pen
        .unwrap()
        .0;
    let ext = render(b"MN", PictTextFace::EXTEND)
        .text_state
        .text_pen
        .unwrap()
        .0;
    assert_eq!(cond, plain - 2);
    assert_eq!(ext, plain + 2);
}

// ---------------------------------------------------------------------------
// Style state round-trips: the byte is honoured mid-stream (plain text
// before the TxFace opcode stays plain).
// ---------------------------------------------------------------------------

#[test]
fn tx_face_applies_from_the_opcode_onward() {
    let mut b = PictBuilder::new(0, 0, 32, 96);
    b.push(&build_tx_size(8));
    b.push(&long_text(16, 4, b"A")); // plain
    b.push(&build_tx_face(PictTextFace::UNDERLINE));
    b.push(&long_text(16, 40, b"A")); // underlined
    let img = parse_pict(&b.finish()).unwrap();
    let ink = ink_set(&img);
    // No underline under the first glyph …
    assert!(ink.iter().all(|&(x, y)| !(y == 17 && x < 20)));
    // … and an underline under the second.
    assert!(ink.iter().any(|&(x, y)| y == 17 && (40..46).contains(&x)));
    assert_eq!(
        img.text_state.tx_face,
        PictTextFace::from(PictTextFace::UNDERLINE)
    );
}
