//! Round 322 — `srcRect` cropping on every raster blit.
//!
//! Inside Macintosh: Imaging With QuickDraw §A-3 Listings A-2 / A-3 lay
//! out every raster opcode (`BitsRect`/`BitsRgn`/`PackBitsRect`/
//! `PackBitsRgn`/`DirectBitsRect`/`DirectBitsRgn`) as
//! `PixMap`/`bounds`, then `srcRect`, then `dstRect`. `CopyBits` — the
//! routine these opcodes replay — copies the *`srcRect` sub-rectangle*
//! of the source pixel map and scales it onto `dstRect`. Earlier rounds
//! decoded the full `bounds`-sized pixel map and scaled all of it onto
//! `dstRect`, ignoring `srcRect`. This round honours `srcRect` so a
//! record whose `srcRect ⊊ bounds` draws only the selected sub-image.
//!
//! All streams here are hand-built to the §A-3 byte layout (the public
//! `PictBuilder` always emits `srcRect == bounds == dstRect`, so a
//! sub-rectangle `srcRect` can only be exercised at the byte level).

use oxideav_pict::parse_pict;

/// Build a v2 PICT with a single `DirectBitsRect` (`0x009A`) carrying a
/// `bw × bh` 32-bit source PixMap (`0xFF R G B` per pixel). `src` and
/// `dst` are `(top, left, bottom, right)` rectangles for the opcode's
/// `srcRect` / `dstRect` fields. `picFrame` is sized to `dst` so the
/// canvas exactly contains the destination.
fn build_directbits_with_src(
    bw: u16,
    bh: u16,
    rgb: &[u8], // bw*bh*3 — R,G,B per pixel
    src: (i16, i16, i16, i16),
    dst: (i16, i16, i16, i16),
) -> Vec<u8> {
    assert_eq!(rgb.len(), (bw as usize) * (bh as usize) * 3);
    let row_bytes = (bw as usize) * 4;
    let mut buf = Vec::new();

    // Picture record header — picFrame == dst so the canvas is dst-sized.
    buf.extend_from_slice(&0u16.to_be_bytes()); // picSize (ignored)
    buf.extend_from_slice(&dst.0.to_be_bytes());
    buf.extend_from_slice(&dst.1.to_be_bytes());
    buf.extend_from_slice(&dst.2.to_be_bytes());
    buf.extend_from_slice(&dst.3.to_be_bytes());

    // v2 version stanza + headerOp.
    buf.extend_from_slice(&0x0011u16.to_be_bytes());
    buf.extend_from_slice(&0x02FFu16.to_be_bytes());
    buf.extend_from_slice(&0x0C00u16.to_be_bytes());
    buf.extend_from_slice(&[0u8; 24]);

    // DirectBitsRect.
    buf.extend_from_slice(&0x009Au16.to_be_bytes());
    buf.extend_from_slice(&0xFFu32.to_be_bytes()); // baseAddr placeholder
    let rb = (row_bytes as u16) | 0x8000; // PixMap flag
    buf.extend_from_slice(&rb.to_be_bytes());
    // bounds = 0,0,bh,bw.
    buf.extend_from_slice(&0i16.to_be_bytes());
    buf.extend_from_slice(&0i16.to_be_bytes());
    buf.extend_from_slice(&(bh as i16).to_be_bytes());
    buf.extend_from_slice(&(bw as i16).to_be_bytes());
    buf.extend_from_slice(&0u16.to_be_bytes()); // pmVersion
    buf.extend_from_slice(&1u16.to_be_bytes()); // packType 1
    buf.extend_from_slice(&0u32.to_be_bytes()); // packSize
    buf.extend_from_slice(&0x00480000u32.to_be_bytes()); // hRes 72
    buf.extend_from_slice(&0x00480000u32.to_be_bytes()); // vRes 72
    buf.extend_from_slice(&16u16.to_be_bytes()); // pixelType RGBDirect
    buf.extend_from_slice(&32u16.to_be_bytes()); // pixelSize
    buf.extend_from_slice(&3u16.to_be_bytes()); // cmpCount
    buf.extend_from_slice(&8u16.to_be_bytes()); // cmpSize
    buf.extend_from_slice(&0u32.to_be_bytes()); // planeBytes
    buf.extend_from_slice(&0u32.to_be_bytes()); // pmTable
    buf.extend_from_slice(&0u32.to_be_bytes()); // pmReserved

    // srcRect — the sub-rectangle of the source PixMap to copy.
    buf.extend_from_slice(&src.0.to_be_bytes());
    buf.extend_from_slice(&src.1.to_be_bytes());
    buf.extend_from_slice(&src.2.to_be_bytes());
    buf.extend_from_slice(&src.3.to_be_bytes());

    // dstRect.
    buf.extend_from_slice(&dst.0.to_be_bytes());
    buf.extend_from_slice(&dst.1.to_be_bytes());
    buf.extend_from_slice(&dst.2.to_be_bytes());
    buf.extend_from_slice(&dst.3.to_be_bytes());
    buf.extend_from_slice(&0u16.to_be_bytes()); // mode = srcCopy

    // Pixel data — packType 1 stores 0xFF R G B per pixel, row_bytes/row.
    for y in 0..bh as usize {
        for x in 0..bw as usize {
            let off = (y * bw as usize + x) * 3;
            buf.push(0xFF);
            buf.push(rgb[off]);
            buf.push(rgb[off + 1]);
            buf.push(rgb[off + 2]);
        }
    }
    if buf.len() % 2 != 0 {
        buf.push(0);
    }
    buf.extend_from_slice(&0x00FFu16.to_be_bytes()); // OpEndPic
    buf
}

/// A 4×4 source PixMap, each pixel a distinct colour keyed off its
/// (x, y). `srcRect = (1,1,3,3)` selects the inner 2×2 block; the
/// destination is a 2×2 canvas. Only the inner block must appear, at
/// 1:1 scale (2×2 → 2×2). Earlier rounds scaled the whole 4×4 bounds
/// down to 2×2 and so painted the wrong (corner-sampled) pixels.
#[test]
fn direct_bits_src_rect_crops_inner_block() {
    let color =
        |x: usize, y: usize| -> [u8; 3] { [(x as u8) * 40 + 10, (y as u8) * 40 + 20, 0x80] };
    let mut rgb = vec![0u8; 4 * 4 * 3];
    for y in 0..4 {
        for x in 0..4 {
            let c = color(x, y);
            let off = (y * 4 + x) * 3;
            rgb[off..off + 3].copy_from_slice(&c);
        }
    }

    let bytes = build_directbits_with_src(4, 4, &rgb, (1, 1, 3, 3), (0, 0, 2, 2));
    let img = parse_pict(&bytes).unwrap();
    assert_eq!(img.width, 2);
    assert_eq!(img.height, 2);

    let px = |x: usize, y: usize| {
        let off = (y * img.width as usize + x) * 4;
        [img.data[off], img.data[off + 1], img.data[off + 2]]
    };

    // The destination 2×2 maps 1:1 onto the source srcRect (1,1)..(3,3).
    assert_eq!(px(0, 0), color(1, 1), "dst (0,0) == src (1,1)");
    assert_eq!(px(1, 0), color(2, 1), "dst (1,0) == src (2,1)");
    assert_eq!(px(0, 1), color(1, 2), "dst (0,1) == src (1,2)");
    assert_eq!(px(1, 1), color(2, 2), "dst (1,1) == src (2,2)");
}

/// A `srcRect` that equals `bounds` is the common QuickDraw-emitter
/// case and must round-trip every source pixel unchanged (no crop, no
/// scaling). Regression guard for the "srcRect ⊇ bounds → identity"
/// fast path.
#[test]
fn direct_bits_full_src_rect_is_identity() {
    let color = |x: usize, y: usize| -> [u8; 3] { [(x as u8) * 60, (y as u8) * 60, 0x33] };
    let mut rgb = vec![0u8; 3 * 3 * 3];
    for y in 0..3 {
        for x in 0..3 {
            let c = color(x, y);
            let off = (y * 3 + x) * 3;
            rgb[off..off + 3].copy_from_slice(&c);
        }
    }

    let bytes = build_directbits_with_src(3, 3, &rgb, (0, 0, 3, 3), (0, 0, 3, 3));
    let img = parse_pict(&bytes).unwrap();
    assert_eq!(img.width, 3);
    assert_eq!(img.height, 3);

    for y in 0..3 {
        for x in 0..3 {
            let off = (y * img.width as usize + x) * 4;
            let got = [img.data[off], img.data[off + 1], img.data[off + 2]];
            assert_eq!(got, color(x, y), "({x},{y}) identity");
        }
    }
}

/// `srcRect` selecting a single source column, drawn into a wider
/// destination, scales that one column across the destination — the
/// CopyBits stretch path on the *cropped* source. Confirms the crop
/// feeds the scaler (not the full bounds): every destination column is
/// the same source column's colour.
#[test]
fn direct_bits_src_rect_single_column_stretched() {
    // 4×2 source; colour depends only on x so a column is uniform.
    let col = |x: usize| -> [u8; 3] { [(x as u8) * 50 + 5, 0x11, 0x22] };
    let mut rgb = vec![0u8; 4 * 2 * 3];
    for y in 0..2 {
        for x in 0..4 {
            let c = col(x);
            let off = (y * 4 + x) * 3;
            rgb[off..off + 3].copy_from_slice(&c);
        }
    }

    // srcRect selects column x=2 only (left=2, right=3), full height.
    // dstRect is 3 wide × 2 tall — the one column stretches across.
    let bytes = build_directbits_with_src(4, 2, &rgb, (0, 2, 2, 3), (0, 0, 2, 3));
    let img = parse_pict(&bytes).unwrap();
    assert_eq!(img.width, 3);
    assert_eq!(img.height, 2);

    for y in 0..2 {
        for x in 0..3 {
            let off = (y * img.width as usize + x) * 4;
            let got = [img.data[off], img.data[off + 1], img.data[off + 2]];
            assert_eq!(got, col(2), "dst ({x},{y}) is the stretched src column 2");
        }
    }
}
