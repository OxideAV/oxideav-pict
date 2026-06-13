//! Round 282 — `CopyBits` transfer modes honoured on the raster blit.
//!
//! Every PICT raster opcode record (`BitsRect 0x0090` / `BitsRgn
//! 0x0091` / `PackBitsRect 0x0098` / `PackBitsRgn 0x0099` /
//! `DirectBitsRect 0x009A` / `DirectBitsRgn 0x009B`) carries a `mode`
//! (transfer mode) word between `dstRect` and the pixel data per
//! Inside Macintosh: Imaging With QuickDraw §A-3 Listings A-2 / A-3.
//! Rounds 1..273 parsed and discarded it (every blit rendered
//! `srcCopy` against a black-fg / white-bg port). Round 282 honours
//! it:
//!
//! * the eight Boolean source modes (`srcCopy = 0` … `notSrcBic = 7`,
//!   §3 pages 3-113..3-114) with the §4 Table 4-1 (page 4-33) colour
//!   semantics — black source pixels apply the foreground colour
//!   (background for the BIC ops), white source pixels apply the
//!   mode's "leave" colour, and any other colour applies weighted
//!   portions per the §4-33 `CopyBits` description;
//! * the eight §4 arithmetic transfer modes (`blend = 32` …
//!   `adMin = 39`) — legal in the same mode word per the §4-40 Note —
//!   reusing the round-273 `blend_arith` combiner with the decoded
//!   raster pixel as the source colour;
//! * the additive `ditherCopy = 64` bit (§3-114), recognised and
//!   stripped (dithering approximates colours on indexed
//!   destinations; the canvas here is true-colour RGBA).
//!
//! `srcCopy` under the fresh-GrafPort black-fg / white-bg state is the
//! §4-34 identity (*"always reproduces the source image"*) so every
//! pre-round-282 stream decodes bit-for-bit unchanged.

use oxideav_pict::ops::{PictBuilder, Verb};
use oxideav_pict::{
    blend_source, build_direct_bits_rect_op, build_direct_bits_rect_op_with_mode, parse_pict,
    ArithMode, PackType, PictImage, Rgba, SourceMode,
};

/// Read pixel `(x, y)` as `(r, g, b)`.
fn pix(img: &PictImage, x: u32, y: u32) -> (u8, u8, u8) {
    let off = ((y * img.width + x) * 4) as usize;
    (img.data[off], img.data[off + 1], img.data[off + 2])
}

/// Flatten a list of RGB triples into the RGBA8 buffer
/// `PictBuilder::raster_with_mode` consumes (alpha = 0xFF).
fn rgba_buf(px: &[(u8, u8, u8)]) -> Vec<u8> {
    px.iter().flat_map(|&(r, g, b)| [r, g, b, 0xFF]).collect()
}

/// Build a 2×2 PICT: wash the canvas a solid `dst` colour, set the
/// blit-time foreground / background to `fg` / `bg`, then blit the
/// four `src` pixels through a `DirectBitsRect` whose record carries
/// transfer-mode word `mode`.
fn build_mode_blit(
    mode: u16,
    dst: (u8, u8, u8),
    fg: (u8, u8, u8),
    bg: (u8, u8, u8),
    src: &[(u8, u8, u8); 4],
) -> Vec<u8> {
    let mut b = PictBuilder::new(0, 0, 2, 2);
    // Destination wash: patCopy + solid-fg pen pattern collapses to a
    // solid fill at `dst` (round-8 fast path).
    b.fg_color(dst.0, dst.1, dst.2)
        .bg_color(dst.0, dst.1, dst.2)
        .pn_mode(8)
        .pen_pattern([0xFF; 8])
        .rect(Verb::Paint, 0, 0, 2, 2);
    // Blit-time port colours.
    b.fg_color(fg.0, fg.1, fg.2).bg_color(bg.0, bg.1, bg.2);
    b.raster_with_mode(0, 0, 2, 2, &rgba_buf(src), PackType::Raw, mode)
        .expect("raster_with_mode");
    b.finish()
}

// ---------------------------------------------------------------------------
// Encoder byte layout.
// ---------------------------------------------------------------------------

/// The mode word sits at a fixed offset in the `DirectBitsRect` opcode
/// bytes: opcode(2) + baseAddr(4) + rowBytes(2) + bounds(8) +
/// pmVersion(2) + packType(2) + packSize(4) + hRes(4) + vRes(4) +
/// pixelType(2) + pixelSize(2) + cmpCount(2) + cmpSize(2) +
/// planeBytes(4) + pmTable(4) + pmReserved(4) + srcRect(8) +
/// dstRect(8) = 68.
#[test]
fn mode_word_byte_offset_in_direct_bits_record() {
    let data = vec![0u8; 2 * 2 * 4];
    let bytes =
        build_direct_bits_rect_op_with_mode(0, 0, 2, 2, &data, PackType::Raw, 0x0022).unwrap();
    assert_eq!(&bytes[68..70], &[0x00, 0x22]);
}

/// `build_direct_bits_rect_op` is the `mode = 0` (srcCopy) shape of
/// the `_with_mode` builder, byte-for-byte.
#[test]
fn plain_builder_is_mode_zero() {
    let data = vec![0x55u8; 2 * 2 * 4];
    let plain = build_direct_bits_rect_op(0, 0, 2, 2, &data, PackType::Raw).unwrap();
    let with0 = build_direct_bits_rect_op_with_mode(0, 0, 2, 2, &data, PackType::Raw, 0).unwrap();
    assert_eq!(plain, with0);
}

// ---------------------------------------------------------------------------
// SourceMode resolution.
// ---------------------------------------------------------------------------

#[test]
fn from_mode_word_boolean_band() {
    let w = Rgba::WHITE;
    assert_eq!(
        SourceMode::from_mode_word(0, None, w, None),
        SourceMode::SrcCopy
    );
    assert_eq!(
        SourceMode::from_mode_word(1, None, w, None),
        SourceMode::SrcOr
    );
    assert_eq!(
        SourceMode::from_mode_word(2, None, w, None),
        SourceMode::SrcXor
    );
    assert_eq!(
        SourceMode::from_mode_word(3, None, w, None),
        SourceMode::SrcBic
    );
    assert_eq!(
        SourceMode::from_mode_word(4, None, w, None),
        SourceMode::NotSrcCopy
    );
    assert_eq!(
        SourceMode::from_mode_word(5, None, w, None),
        SourceMode::NotSrcOr
    );
    assert_eq!(
        SourceMode::from_mode_word(6, None, w, None),
        SourceMode::NotSrcXor
    );
    assert_eq!(
        SourceMode::from_mode_word(7, None, w, None),
        SourceMode::NotSrcBic
    );
}

/// §3-114: `ditherCopy = 64` is additive on any source mode. The bit
/// is stripped before resolution.
#[test]
fn from_mode_word_strips_dither_copy() {
    let w = Rgba::WHITE;
    assert_eq!(
        SourceMode::from_mode_word(64, None, w, None),
        SourceMode::SrcCopy
    );
    assert_eq!(
        SourceMode::from_mode_word(64 + 3, None, w, None),
        SourceMode::SrcBic
    );
    assert_eq!(
        SourceMode::from_mode_word(64 + 34, None, w, None),
        SourceMode::Arith {
            mode: ArithMode::AddOver,
            op_color: Rgba::WHITE,
            bg_key: Rgba::WHITE,
        }
    );
}

/// §4-40 absent-`OpColor` defaults: max-pin → white, min-pin → black,
/// blend → 50 % gray.
#[test]
fn from_mode_word_arith_op_color_defaults() {
    let key = Rgba::new(1, 2, 3, 255);
    match SourceMode::from_mode_word(33, None, key, None) {
        SourceMode::Arith {
            mode,
            op_color,
            bg_key,
        } => {
            assert_eq!(mode, ArithMode::AddPin);
            assert_eq!(op_color, Rgba::WHITE);
            assert_eq!(bg_key, key);
        }
        other => panic!("expected Arith, got {other:?}"),
    }
    match SourceMode::from_mode_word(35, None, key, None) {
        SourceMode::Arith { op_color, .. } => assert_eq!(op_color, Rgba::BLACK),
        other => panic!("expected Arith, got {other:?}"),
    }
    match SourceMode::from_mode_word(32, None, key, None) {
        SourceMode::Arith { op_color, .. } => assert_eq!(op_color, Rgba::new(128, 128, 128, 255)),
        other => panic!("expected Arith, got {other:?}"),
    }
}

/// Codes outside the §3-113 / §4-38 bands (e.g. the pattern modes
/// `8..=15`, which are not defined for `CopyBits`) fall back to
/// `srcCopy` — total-function posture, same as the pattern path.
#[test]
fn from_mode_word_unknown_codes_fall_back_to_src_copy() {
    let w = Rgba::WHITE;
    assert_eq!(
        SourceMode::from_mode_word(8, None, w, None),
        SourceMode::SrcCopy
    );
    // `hilite = 50` is handled separately (see the round-290 hilite
    // tests); every other out-of-band code falls back to `srcCopy`.
    assert_eq!(
        SourceMode::from_mode_word(0x0100, None, w, None),
        SourceMode::SrcCopy
    );
}

#[test]
fn identity_copy_predicate() {
    assert!(SourceMode::SrcCopy.is_identity_copy(Rgba::BLACK, Rgba::WHITE));
    assert!(!SourceMode::SrcCopy.is_identity_copy(Rgba::new(255, 0, 0, 255), Rgba::WHITE));
    assert!(!SourceMode::SrcCopy.is_identity_copy(Rgba::BLACK, Rgba::BLACK));
    assert!(!SourceMode::SrcOr.is_identity_copy(Rgba::BLACK, Rgba::WHITE));
}

// ---------------------------------------------------------------------------
// blend_source — §4 Table 4-1 weighted formulas, hand-computed pins.
// ---------------------------------------------------------------------------

#[test]
fn blend_source_src_copy_weighted_portions() {
    let fg = Rgba::new(0, 0, 255, 255); // blue
    let bg = Rgba::new(255, 0, 0, 255); // red
    let dst = Rgba::new(9, 9, 9, 255); // ignored by COPY
                                       // Black source → foreground; white source → background (Table 4-1).
    assert_eq!(
        blend_source(SourceMode::SrcCopy, Rgba::BLACK, dst, fg, bg),
        fg
    );
    assert_eq!(
        blend_source(SourceMode::SrcCopy, Rgba::WHITE, dst, fg, bg),
        bg
    );
    // Mid-gray 128: w = 127 of fg + 128 of bg per channel —
    // r = (127·0 + 128·255 + 127)/255 = 128, g = 0,
    // b = (127·255 + 128·0 + 127)/255 = 127.
    let mid = Rgba::new(128, 128, 128, 255);
    assert_eq!(
        blend_source(SourceMode::SrcCopy, mid, dst, fg, bg),
        Rgba::new(128, 0, 127, 255)
    );
}

/// §4-34: *"the notSrcCopy mode reverses the foreground and background
/// colors."*
#[test]
fn blend_source_not_src_copy_reverses_roles() {
    let fg = Rgba::new(0, 0, 255, 255);
    let bg = Rgba::new(255, 0, 0, 255);
    let dst = Rgba::new(9, 9, 9, 255);
    assert_eq!(
        blend_source(SourceMode::NotSrcCopy, Rgba::BLACK, dst, fg, bg),
        bg
    );
    assert_eq!(
        blend_source(SourceMode::NotSrcCopy, Rgba::WHITE, dst, fg, bg),
        fg
    );
}

/// §4-34: *"Drawing into a white background with a black foreground
/// always reproduces the source image."*
#[test]
fn blend_source_src_copy_black_fg_white_bg_is_identity() {
    for &c in &[
        (0u8, 0u8, 0u8),
        (255, 255, 255),
        (1, 128, 254),
        (40, 0, 200),
    ] {
        let src = Rgba::new(c.0, c.1, c.2, 255);
        assert_eq!(
            blend_source(
                SourceMode::SrcCopy,
                src,
                Rgba::new(7, 7, 7, 255),
                Rgba::BLACK,
                Rgba::WHITE
            ),
            src
        );
    }
}

// ---------------------------------------------------------------------------
// Full decode round-trips per mode.
// ---------------------------------------------------------------------------

/// Mode 0 with no port-colour opcodes — the §4-34 identity fast path.
/// Pixels reproduce the source bit-for-bit, exactly as pre-r282.
#[test]
fn mode_zero_default_port_is_identity() {
    let src = [(10, 20, 30), (40, 50, 60), (70, 80, 90), (100, 110, 120)];
    let mut b = PictBuilder::new(0, 0, 2, 2);
    b.raster_with_mode(0, 0, 2, 2, &rgba_buf(&src), PackType::Raw, 0)
        .unwrap();
    let img = parse_pict(&b.finish()).unwrap();
    for (i, &(r, g, bl)) in src.iter().enumerate() {
        assert_eq!(pix(&img, (i % 2) as u32, (i / 2) as u32), (r, g, bl));
    }
}

/// `srcCopy + ditherCopy` (mode 64) decodes identically to plain
/// `srcCopy` — the dither bit is recognised and stripped.
#[test]
fn dither_copy_blit_matches_src_copy() {
    let src = [(10, 20, 30), (40, 50, 60), (70, 80, 90), (100, 110, 120)];
    let mut b = PictBuilder::new(0, 0, 2, 2);
    b.raster_with_mode(0, 0, 2, 2, &rgba_buf(&src), PackType::Raw, 64)
        .unwrap();
    let img = parse_pict(&b.finish()).unwrap();
    assert_eq!(pix(&img, 0, 0), src[0]);
    assert_eq!(pix(&img, 1, 1), src[3]);
}

/// The Listing 4-5 coloration shape: a grayscale source copied through
/// `srcCopy` with a blue foreground and a red background lands as
/// shades of blue and red.
#[test]
fn src_copy_blit_colorizes_against_port_colours() {
    let src = [(0, 0, 0), (255, 255, 255), (128, 128, 128), (0, 0, 0)];
    let pict = build_mode_blit(0, (9, 9, 9), (0, 0, 255), (255, 0, 0), &src);
    let img = parse_pict(&pict).unwrap();
    assert_eq!(pix(&img, 0, 0), (0, 0, 255)); // black → fg (blue)
    assert_eq!(pix(&img, 1, 0), (255, 0, 0)); // white → bg (red)
    assert_eq!(pix(&img, 0, 1), (128, 0, 127)); // mid-gray → weighted mix
}

#[test]
fn not_src_copy_blit_reverses_fg_bg() {
    let src = [(0, 0, 0), (255, 255, 255), (0, 0, 0), (255, 255, 255)];
    let pict = build_mode_blit(4, (9, 9, 9), (0, 0, 255), (255, 0, 0), &src);
    let img = parse_pict(&pict).unwrap();
    assert_eq!(pix(&img, 0, 0), (255, 0, 0)); // black → bg
    assert_eq!(pix(&img, 1, 0), (0, 0, 255)); // white → fg
}

/// Table 4-1 `srcOr`: black source applies the foreground; white
/// leaves the destination alone.
#[test]
fn src_or_blit_applies_fg_where_black() {
    let src = [(0, 0, 0), (255, 255, 255), (255, 255, 255), (0, 0, 0)];
    let pict = build_mode_blit(1, (0, 255, 0), (255, 0, 0), (0, 0, 255), &src);
    let img = parse_pict(&pict).unwrap();
    assert_eq!(pix(&img, 0, 0), (255, 0, 0)); // black → fg (red)
    assert_eq!(pix(&img, 1, 0), (0, 255, 0)); // white → dst (green wash)
}

/// Table 4-1 `notSrcOr`: white source applies the foreground; black
/// leaves the destination alone.
#[test]
fn not_src_or_blit_applies_fg_where_white() {
    let src = [(0, 0, 0), (255, 255, 255), (255, 255, 255), (0, 0, 0)];
    let pict = build_mode_blit(5, (0, 255, 0), (255, 0, 0), (0, 0, 255), &src);
    let img = parse_pict(&pict).unwrap();
    assert_eq!(pix(&img, 0, 0), (0, 255, 0)); // black → dst
    assert_eq!(pix(&img, 1, 0), (255, 0, 0)); // white → fg
}

/// Table 4-1 `srcXor`: only an exactly-black source pixel inverts the
/// destination; white and coloured source pixels leave it alone.
#[test]
fn src_xor_blit_inverts_only_under_black_source() {
    let src = [(0, 0, 0), (255, 255, 255), (200, 10, 10), (0, 0, 0)];
    let pict = build_mode_blit(2, (0x80, 0x80, 0x80), (255, 0, 0), (0, 0, 255), &src);
    let img = parse_pict(&pict).unwrap();
    assert_eq!(pix(&img, 0, 0), (0x7F, 0x7F, 0x7F)); // black → invert dst
    assert_eq!(pix(&img, 1, 0), (0x80, 0x80, 0x80)); // white → unchanged
    assert_eq!(pix(&img, 0, 1), (0x80, 0x80, 0x80)); // coloured → unchanged
}

#[test]
fn not_src_xor_blit_inverts_only_under_white_source() {
    let src = [(0, 0, 0), (255, 255, 255), (200, 10, 10), (255, 255, 255)];
    let pict = build_mode_blit(6, (0x80, 0x80, 0x80), (255, 0, 0), (0, 0, 255), &src);
    let img = parse_pict(&pict).unwrap();
    assert_eq!(pix(&img, 0, 0), (0x80, 0x80, 0x80)); // black → unchanged
    assert_eq!(pix(&img, 1, 0), (0x7F, 0x7F, 0x7F)); // white → invert dst
    assert_eq!(pix(&img, 0, 1), (0x80, 0x80, 0x80)); // coloured → unchanged
}

/// Table 4-1 `srcBic`: black source applies the *background* colour;
/// white leaves the destination alone. (§4-34: with a white background
/// the black portions of the source are erased.)
#[test]
fn src_bic_blit_applies_bg_where_black() {
    let src = [(0, 0, 0), (255, 255, 255), (255, 255, 255), (0, 0, 0)];
    let pict = build_mode_blit(3, (0, 255, 0), (255, 0, 0), (0, 0, 255), &src);
    let img = parse_pict(&pict).unwrap();
    assert_eq!(pix(&img, 0, 0), (0, 0, 255)); // black → bg (blue)
    assert_eq!(pix(&img, 1, 0), (0, 255, 0)); // white → dst
}

#[test]
fn not_src_bic_blit_applies_bg_where_white() {
    let src = [(0, 0, 0), (255, 255, 255), (255, 255, 255), (0, 0, 0)];
    let pict = build_mode_blit(7, (0, 255, 0), (255, 0, 0), (0, 0, 255), &src);
    let img = parse_pict(&pict).unwrap();
    assert_eq!(pix(&img, 0, 0), (0, 255, 0)); // black → dst
    assert_eq!(pix(&img, 1, 0), (0, 0, 255)); // white → bg
}

// ---------------------------------------------------------------------------
// Arithmetic transfer modes on the blit (§4-40 Note: legal in the
// CopyBits mode parameter).
// ---------------------------------------------------------------------------

/// `addOver = 34` — wrapping per-channel add of source and
/// destination.
#[test]
fn add_over_blit_sums_with_destination() {
    let src = [(50, 50, 50); 4];
    let pict = build_mode_blit(34, (100, 100, 100), (0, 0, 0), (255, 255, 255), &src);
    let img = parse_pict(&pict).unwrap();
    assert_eq!(pix(&img, 0, 0), (150, 150, 150));
    assert_eq!(pix(&img, 1, 1), (150, 150, 150));
}

/// `addPin = 33` — the sum pins to the declared `OpColor` maximum.
#[test]
fn add_pin_blit_honours_op_color_maximum() {
    let src = [(50, 50, 50); 4];
    let mut b = PictBuilder::new(0, 0, 2, 2);
    b.fg_color(100, 100, 100)
        .bg_color(100, 100, 100)
        .pn_mode(8)
        .pen_pattern([0xFF; 8])
        .rect(Verb::Paint, 0, 0, 2, 2);
    b.op_color(120, 120, 120);
    b.raster_with_mode(0, 0, 2, 2, &rgba_buf(&src), PackType::Raw, 33)
        .unwrap();
    let img = parse_pict(&b.finish()).unwrap();
    // min(100 + 50, 120) = 120 per channel.
    assert_eq!(pix(&img, 0, 0), (120, 120, 120));
}

/// `transparent = 36` — source pixels equal to the background colour
/// are holes; everything else copies through.
#[test]
fn transparent_blit_keys_on_background_colour() {
    let src = [
        (255, 255, 255), // == bg (white) → hole
        (0, 200, 0),     // copies
        (255, 255, 255), // hole
        (0, 0, 200),     // copies
    ];
    let pict = build_mode_blit(36, (255, 0, 0), (0, 0, 0), (255, 255, 255), &src);
    let img = parse_pict(&pict).unwrap();
    assert_eq!(pix(&img, 0, 0), (255, 0, 0)); // hole keeps the red wash
    assert_eq!(pix(&img, 1, 0), (0, 200, 0));
    assert_eq!(pix(&img, 0, 1), (255, 0, 0));
    assert_eq!(pix(&img, 1, 1), (0, 0, 200));
}

// ---------------------------------------------------------------------------
// 1-bpp BitMap source — Table 4-1 black/white rows against port
// colours.
// ---------------------------------------------------------------------------

/// Hand-build a v2 PICT: RGBFgCol + RGBBkCol, then a 1-bpp `BitsRect`
/// (`0x0090`, rowBytes = 1 < 8 ⇒ raw rows per §A-3 footnote `¶`) whose
/// record carries `mode`. 8×2: row 0 = 0xF0 (4 black, 4 white),
/// row 1 = 0x0F.
fn build_v2_1bpp_bits_rect_with_port_colours(
    fg: (u8, u8, u8),
    bg: (u8, u8, u8),
    mode: u16,
) -> Vec<u8> {
    let (width, height) = (8i16, 2i16);
    let mut buf = Vec::new();
    // Picture record header.
    buf.extend_from_slice(&0u16.to_be_bytes());
    buf.extend_from_slice(&0i16.to_be_bytes());
    buf.extend_from_slice(&0i16.to_be_bytes());
    buf.extend_from_slice(&height.to_be_bytes());
    buf.extend_from_slice(&width.to_be_bytes());
    // v2 stanza + headerOp.
    buf.extend_from_slice(&0x0011u16.to_be_bytes());
    buf.extend_from_slice(&0x02FFu16.to_be_bytes());
    buf.extend_from_slice(&0x0C00u16.to_be_bytes());
    buf.extend_from_slice(&[0u8; 24]);
    // RGBFgCol + RGBBkCol (16-bit per channel, high byte = value).
    buf.extend_from_slice(&0x001Au16.to_be_bytes());
    for c in [fg.0, fg.1, fg.2] {
        buf.extend_from_slice(&u16::from_be_bytes([c, c]).to_be_bytes());
    }
    buf.extend_from_slice(&0x001Bu16.to_be_bytes());
    for c in [bg.0, bg.1, bg.2] {
        buf.extend_from_slice(&u16::from_be_bytes([c, c]).to_be_bytes());
    }
    // BitsRect 0x0090, rowBytes = 1 (high bit clear ⇒ BitMap).
    buf.extend_from_slice(&0x0090u16.to_be_bytes());
    buf.extend_from_slice(&1u16.to_be_bytes());
    // bounds / srcRect / dstRect = 0,0,2,8.
    for _ in 0..3 {
        buf.extend_from_slice(&0i16.to_be_bytes());
        buf.extend_from_slice(&0i16.to_be_bytes());
        buf.extend_from_slice(&height.to_be_bytes());
        buf.extend_from_slice(&width.to_be_bytes());
    }
    buf.extend_from_slice(&mode.to_be_bytes());
    // Raw rows: bit = 1 → black source pixel.
    buf.extend_from_slice(&[0xF0, 0x0F]);
    // OpEndPic (already word-aligned).
    buf.extend_from_slice(&0x00FFu16.to_be_bytes());
    buf
}

/// A 1-bit source through `srcCopy` against non-default port colours:
/// black bits take the foreground, white bits take the background
/// (Table 4-1 first row — same rule "regardless of the pixel depth").
#[test]
fn one_bpp_bits_rect_src_copy_takes_port_colours() {
    let pict = build_v2_1bpp_bits_rect_with_port_colours((0, 200, 0), (255, 255, 0), 0);
    let img = parse_pict(&pict).unwrap();
    // Row 0 = 0xF0: x 0..4 black bits → fg, x 4..8 white bits → bg.
    assert_eq!(pix(&img, 0, 0), (0, 200, 0));
    assert_eq!(pix(&img, 3, 0), (0, 200, 0));
    assert_eq!(pix(&img, 4, 0), (255, 255, 0));
    assert_eq!(pix(&img, 7, 0), (255, 255, 0));
    // Row 1 = 0x0F: reversed.
    assert_eq!(pix(&img, 0, 1), (255, 255, 0));
    assert_eq!(pix(&img, 7, 1), (0, 200, 0));
}

/// The same 1-bit source through `srcOr`: black bits apply the
/// foreground, white bits leave the (paper-white) canvas alone.
#[test]
fn one_bpp_bits_rect_src_or_leaves_white_bits_alone() {
    let pict = build_v2_1bpp_bits_rect_with_port_colours((200, 0, 0), (0, 0, 255), 1);
    let img = parse_pict(&pict).unwrap();
    assert_eq!(pix(&img, 0, 0), (200, 0, 0)); // black bit → fg
    assert_eq!(pix(&img, 7, 0), (255, 255, 255)); // white bit → paper
}
