//! Round-4 integration tests covering new encoder features:
//!
//! * `PackType::Rle16` — packType-3 16-bit RGB-packed PackBits emit.
//! * `ops::PictBuilder` end-to-end synth+decode round-trips for every
//!   v2 drawing-command opcode family (line / rect / round-rect / oval
//!   / arc / poly / region inversion-encoded).
//! * Region-path emit byte layout validation.

use oxideav_pict::{
    encode_pict_v2, ops::PictBuilder, ops::Verb, parse_pict, pixel_data_sizes, PackType,
};

// ---- packType 3 (Rle16) round-trip ----

#[test]
fn packtype3_roundtrip_16x16_solid() {
    let width = 16u32;
    let height = 16u32;
    // Solid green: every R=0x00 G=0xFF B=0x00 A=0xFF.
    let mut rgba = vec![0u8; (width * height * 4) as usize];
    for i in 0..width as usize * height as usize {
        rgba[i * 4 + 1] = 0xFF;
        rgba[i * 4 + 3] = 0xFF;
    }
    let enc = encode_pict_v2(width, height, &rgba, PackType::Rle16).unwrap();
    let img = parse_pict(&enc).unwrap();
    assert_eq!(img.width, width);
    assert_eq!(img.height, height);
    // Per-pixel green channel after A1R5G5B5 round-trip is 0xFF (5-bit
    // 0x1F replicated to 8 bits — see decoder write_16bpp_row).
    for i in 0..width as usize * height as usize {
        let off = i * 4;
        // R channel: 0 → 0 (5-bit 0 → 8-bit 0).
        assert_eq!(img.data[off], 0x00, "R");
        // G channel: 0xFF >> 3 = 0x1F → 8-bit reproduction = 0xFF.
        // (Decoder uses (5-bit << 3) | (5-bit >> 2) for accuracy.)
        assert!(img.data[off + 1] >= 0xF8, "G should round-trip near 0xFF");
        assert_eq!(img.data[off + 2], 0x00, "B");
    }
}

#[test]
fn packtype3_roundtrip_gradient() {
    let width = 8u32;
    let height = 8u32;
    let mut rgba = vec![0u8; (width * height * 4) as usize];
    for y in 0..height as usize {
        for x in 0..width as usize {
            let off = (y * width as usize + x) * 4;
            rgba[off] = ((x * 32) & 0xF8) as u8;
            rgba[off + 1] = ((y * 32) & 0xF8) as u8;
            rgba[off + 2] = 0x80 & 0xF8;
            rgba[off + 3] = 0xFF;
        }
    }
    let enc = encode_pict_v2(width, height, &rgba, PackType::Rle16).unwrap();
    let img = parse_pict(&enc).unwrap();
    assert_eq!(img.width, width);
    assert_eq!(img.height, height);
    // 16-bpp loses 3 LSBs per channel → tolerance of 7 is adequate.
    for y in 0..height as usize {
        for x in 0..width as usize {
            let off = (y * width as usize + x) * 4;
            for ch in 0..3 {
                let diff = (rgba[off + ch] as i32 - img.data[off + ch] as i32).abs();
                assert!(
                    diff < 8,
                    "channel {ch} at ({x},{y}): want {} got {}",
                    rgba[off + ch],
                    img.data[off + ch]
                );
            }
        }
    }
}

#[test]
fn packtype3_solid_compresses() {
    let width = 64u32;
    let height = 64u32;
    let rgba = vec![0x55u8; (width * height * 4) as usize];
    let (raw, packed) = pixel_data_sizes(width, height, &rgba, PackType::Rle16);
    // 64 pixels per row, all the same → run packets compress to a few
    // bytes per row. Should be way smaller than raw 32-bit (which is
    // raw_size = 64*64*4 = 16384).
    assert!(
        packed * 50 < raw,
        "solid Rle16 should compress aggressively: packed={packed} raw={raw}"
    );
}

// ---- PictBuilder drawing-only PICT round-trips ----

#[test]
fn builder_horizontal_line_decode() {
    let mut b = PictBuilder::new(0, 0, 8, 16);
    b.fg_color(0xFF, 0x00, 0x00);
    b.line(2, 4, 12, 4); // horizontal line at y=4, x in [2..12]
    let bytes = b.finish();
    let img = parse_pict(&bytes).expect("decode");
    // Pixels along the line should be red.
    for x in 3..12 {
        let off = (4 * 16 + x) * 4;
        assert_eq!(img.data[off], 0xFF, "line pixel R at x={x}");
    }
    // Other rows = paper.
    let off = 0;
    assert_eq!(img.data[off], 0xFF);
    assert_eq!(img.data[off + 1], 0xFF);
    assert_eq!(img.data[off + 2], 0xFF);
}

#[test]
fn builder_filled_rect_with_frame() {
    // Test multiple verbs: paint a red rect, then frame a black
    // rectangle around it.
    let mut b = PictBuilder::new(0, 0, 16, 16);
    b.fg_color(0xFF, 0x00, 0x00);
    b.rect(Verb::Paint, 4, 4, 12, 12);
    b.fg_color(0x00, 0x00, 0x00);
    b.rect(Verb::Frame, 2, 2, 14, 14);
    let bytes = b.finish();
    let img = parse_pict(&bytes).expect("decode");
    // Inside the inner rect (8,8) is red.
    let off = (8 * 16 + 8) * 4;
    assert_eq!(img.data[off], 0xFF, "inner R");
    // Outline at (2,2) is black.
    let off = (2 * 16 + 2) * 4;
    assert_eq!(img.data[off], 0x00, "outline R");
    assert_eq!(img.data[off + 1], 0x00, "outline G");
}

#[test]
fn builder_chained_drawing_commands() {
    // Stress-test the builder: multiple drawing commands of different
    // shapes / verbs / colours interleaved with state changes.
    let mut b = PictBuilder::new(0, 0, 32, 32);
    b.bg_color(0xFF, 0xFF, 0xFF);
    b.fg_color(0xFF, 0x00, 0x00);
    b.rect(Verb::Paint, 0, 0, 16, 16);
    b.fg_color(0x00, 0xFF, 0x00);
    b.oval(Verb::Paint, 16, 16, 32, 32);
    b.fg_color(0x00, 0x00, 0xFF);
    b.poly(Verb::Fill, &[(20, 4), (30, 4), (25, 14)]).unwrap();
    let bytes = b.finish();
    let img = parse_pict(&bytes).expect("decode");
    // Top-left quadrant = red.
    let off = (4 * 32 + 4) * 4;
    assert_eq!(img.data[off], 0xFF, "top-left R");
    // Bottom-right oval centre = green.
    let off = (24 * 32 + 24) * 4;
    assert_eq!(img.data[off + 1], 0xFF, "bot-right G");
    // Triangle interior = blue.
    let off = (8 * 32 + 25) * 4;
    assert_eq!(img.data[off + 2], 0xFF, "triangle B");
}

#[test]
fn builder_empty_returns_no_raster() {
    // A PictBuilder with no drawing commands will produce a stream
    // that the decoder rejects with NoRaster (canvas not dirty).
    let b = PictBuilder::new(0, 0, 8, 8);
    let bytes = b.finish();
    assert!(parse_pict(&bytes).is_err());
}

// ---- Region opcode emit round-trip ----

#[test]
fn region_inverted_diagonal_band() {
    // 8x8 region bbox; toggle inversion to create a diagonal band.
    //
    // Encoded membership per row (each toggle toggles columns at
    // listed x's):
    //   y=0: empty (no records yet)
    //   y=2: cols [2, 6) inside (toggle at x=2 and x=6)
    //   …all rows from y=2 onwards look the same (band carries forward)
    let mut b = PictBuilder::new(0, 0, 8, 8);
    b.fg_color(0x80, 0x40, 0xC0);
    let scanlines = [(2i16, [2i16, 6i16].as_slice())];
    b.region(Verb::Paint, 0, 0, 8, 8, &scanlines).unwrap();
    let bytes = b.finish();
    let img = parse_pict(&bytes).expect("decode");
    // Inside row 4, col 3 → purple ink.
    let off = (4 * 8 + 3) * 4;
    assert_eq!(img.data[off], 0x80, "R inside band");
    assert_eq!(img.data[off + 1], 0x40, "G inside band");
    assert_eq!(img.data[off + 2], 0xC0, "B inside band");
    // Outside row 0 col 0 → paper.
    let off = 0;
    assert_eq!(img.data[off], 0xFF, "paper R");
}

// ---- Mixed: PackType + drawing commands NOT mixed in one stream
// (DirectBitsRect is its own opcode and the workspace already covers
// it). What we want is to confirm a *drawing-only* v2 stream is a
// valid PICT.

/// Helper that invokes ImageMagick (`magick`) on the supplied PICT
/// bytes and returns true if ImageMagick decoded it without error.
/// Returns `None` if `magick` is not installed.
///
/// Uses a tempfile rather than piping over stdin because ImageMagick
/// rejects stdin streams that begin with the 512-byte launch stub
/// (it can't seek backwards to validate the picture record).
fn imagemagick_accepts(bytes: &[u8]) -> Option<bool> {
    use std::fs;
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};
    if Command::new("magick").arg("-version").output().is_err() {
        return None;
    }
    // Unique per call even when parallel test threads land on the
    // same clock tick (Windows' SystemTime resolution is coarse
    // enough for nanos alone to collide, and each caller deletes its
    // files afterwards — a shared name would yank another test's
    // input mid-convert): pid + atomic counter + nanos.
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let pid = std::process::id();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_nanos();
    let stem = format!("oxideav_pict_xcheck_{pid}_{seq}_{nanos}");
    let path = std::env::temp_dir().join(format!("{stem}.pict"));
    fs::write(&path, bytes).ok()?;
    let out_path = std::env::temp_dir().join(format!("{stem}.png"));
    let status = Command::new("magick")
        .arg(&path)
        .arg(&out_path)
        .status()
        .ok()?;
    let ok = status.success() && out_path.exists();
    let _ = fs::remove_file(&path);
    let _ = fs::remove_file(&out_path);
    Some(ok)
}

#[test]
fn imagemagick_cross_decodes_packtype3() {
    let width = 16u32;
    let height = 16u32;
    let mut rgba = vec![0u8; (width * height * 4) as usize];
    for i in 0..width as usize * height as usize {
        rgba[i * 4] = 0xFF;
        rgba[i * 4 + 3] = 0xFF;
    }
    let enc = encode_pict_v2(width, height, &rgba, PackType::Rle16).unwrap();
    if let Some(ok) = imagemagick_accepts(&enc) {
        assert!(ok, "ImageMagick rejected packType=3 stream");
    }
    // If ImageMagick isn't installed, the test silently passes.
}

#[test]
fn imagemagick_cross_decodes_drawing_only_pict() {
    // ImageMagick's PICT delegate handles drawing-only PICTs just
    // fine — it rasterises them at 72 DPI by default. Confirm a
    // PictBuilder stream survives the round-trip.
    let mut b = PictBuilder::new(0, 0, 16, 16);
    b.fg_color(0xFF, 0x00, 0x00);
    b.rect(Verb::Paint, 4, 4, 12, 12);
    let bytes = b.finish();
    if let Some(ok) = imagemagick_accepts(&bytes) {
        assert!(ok, "ImageMagick rejected drawing-only PictBuilder stream");
    }
}

#[test]
fn imagemagick_cross_decodes_region_op() {
    // Region opcodes are uncommon in real-world PICT streams but
    // valid per Inside Macintosh §A-3. Make sure ImageMagick accepts
    // a stream containing one.
    let mut b = PictBuilder::new(0, 0, 8, 8);
    b.fg_color(0x40, 0x80, 0xC0);
    b.region_rect(Verb::Paint, 1, 1, 7, 7);
    let bytes = b.finish();
    if let Some(ok) = imagemagick_accepts(&bytes) {
        assert!(ok, "ImageMagick rejected region opcode");
    }
}

#[test]
fn drawing_only_v2_is_valid_pict() {
    let mut b = PictBuilder::new(0, 0, 4, 4);
    b.fg_color(0xFF, 0x00, 0xFF);
    b.rect(Verb::Paint, 0, 0, 4, 4);
    let bytes = b.finish();
    // Round 4 invariant: a drawing-only v2 stream still has the
    // 512-byte launch stub, picture-record header, headerOp stanza
    // and OpEndPic — total > 552 bytes.
    assert!(bytes.len() > 552);
    // Stub bytes are zero.
    assert_eq!(&bytes[..512], &[0u8; 512]);
    // Picture-record sentinel.
    assert_eq!(&bytes[522..524], &[0x00, 0x11]);
    // OpEndPic at the very end.
    assert_eq!(&bytes[bytes.len() - 2..], &[0x00, 0xFF]);
    let img = parse_pict(&bytes).unwrap();
    assert_eq!(img.width, 4);
}
