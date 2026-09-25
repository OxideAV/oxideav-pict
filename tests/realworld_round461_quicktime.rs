//! Round 461 — the genuine QuickTime-emitted `$8200` sample.
//!
//! `docs/image/quickdraw/pict-8200-real-world-fixtures.md` records
//! `JDSnowyBlog.pct` (59 624 bytes, SHA-256
//! `6ead634b3309446c94b2e31d9d617c2d4d74b598de0e5d77d822bb67f65bc991`),
//! a 500×281 "Photo - JPEG" picture written by Apple's Image
//! Compression Manager, with its complete structural parse. The bytes
//! are not redistributable and are **not** vendored; this test runs
//! only when `OXIDEAV_PICT_SAMPLES` names a directory holding the file
//! (fetch recipe in the note, §2.1) and is a silent pass otherwise.
//!
//! What it pins, all from the note or from a black-box render:
//!
//! * §2.4 / §2.5 wrapper and `ImageDescription` fields;
//! * the image renders (`'jpeg'` through `oxideav-mjpeg`) over the
//!   whole 500×281 frame with the identity matrix;
//! * the three "QuickTime™ and a / Photo - JPEG decompressor / are
//!   needed to see this picture." `LongText` lines (§2.3, §2.6 signal
//!   5) are suppressed;
//! * the canvas mean matches ImageMagick's render of the file
//!   (0.375555 over RGB, measured 2026-09-25; the crate's own render
//!   measured 0.375545, 45.6 dB PSNR against it).

use std::path::PathBuf;

use oxideav_pict::state::RectI32;
use oxideav_pict::{parse_pict, probe_pict, QuickTimePayload, QuickTimeRender};

fn sample() -> Option<Vec<u8>> {
    let dir = std::env::var_os("OXIDEAV_PICT_SAMPLES")?;
    let path = PathBuf::from(dir).join("JDSnowyBlog.pct");
    let bytes = std::fs::read(&path).ok()?;
    if bytes.len() != 59_624 {
        eprintln!(
            "skipping: {} is not the documented 59 624-byte sample",
            path.display()
        );
        return None;
    }
    Some(bytes)
}

#[test]
fn jd_snowy_blog_structure_matches_the_fixtures_note() {
    let Some(bytes) = sample() else {
        return;
    };
    let probe = probe_pict(&bytes).unwrap();
    assert_eq!(probe.compressed_quicktime_count, 1);
    let img = parse_pict(&bytes).unwrap();
    assert_eq!((img.width, img.height), (500, 281));
    let qt = &img.quicktime[0];
    assert_eq!(qt.data.len(), 58_912, "§2.4 Size");
    let Some(QuickTimePayload::Compressed(c)) = &qt.image else {
        panic!("typed $8200 expected");
    };
    assert_eq!(c.version, 0);
    assert!(c.matrix.is_identity());
    assert_eq!(c.matrix.0[8].0, 0x4000_0000, "w is Fract 1.0");
    assert!(c.matte.is_none());
    assert_eq!(c.matte_rect, RectI32::default());
    assert_eq!(c.mode, 0x0040, "srcCopy + ditherCopy");
    assert_eq!(c.src_rect, RectI32::from_be(0, 0, 281, 500));
    assert_eq!(
        c.accuracy, 0x300,
        "codecHighQuality, derived from mode bit 7"
    );
    assert!(c.mask_region.is_none());
    let d = &c.image_description;
    assert_eq!(d.id_size, 86);
    assert_eq!(&d.codec, b"jpeg");
    assert_eq!(&d.vendor, b"appl");
    assert_eq!((d.version, d.revision_level), (1, 1));
    assert_eq!((d.temporal_quality, d.spatial_quality), (0, 0x3FF));
    assert_eq!((d.width, d.height), (500, 281));
    assert_eq!(d.data_size, 58_758);
    assert_eq!(d.frame_count, 1);
    assert_eq!(d.name(), "Photo - JPEG");
    assert_eq!((d.depth, d.clut_id), (24, -1));
    assert_eq!(&c.image_data[..2], &[0xFF, 0xD8]);
    assert_eq!(&c.image_data[c.image_data.len() - 2..], &[0xFF, 0xD9]);
}

#[cfg(feature = "registry")]
#[test]
fn jd_snowy_blog_renders_and_hides_the_warning_lines() {
    let Some(bytes) = sample() else {
        return;
    };
    let img = parse_pict(&bytes).unwrap();
    assert_eq!(
        img.quicktime[0].render,
        QuickTimeRender::Rendered {
            dst: RectI32::from_be(0, 0, 281, 500),
            matte_skipped: None,
            placeholder_skipped: 3,
        }
    );
    // Mean over RGB vs the black-box render (0.375555). A 0.005
    // tolerance is far tighter than the warning text would allow
    // (drawing it moved the mean by more than that) yet loose enough
    // for a different JPEG decoder's rounding.
    let sum: u64 = img
        .data
        .chunks_exact(4)
        .map(|p| u64::from(p[0]) + u64::from(p[1]) + u64::from(p[2]))
        .sum();
    let mean = sum as f64 / (img.data.len() as f64 / 4.0 * 3.0 * 255.0);
    assert!((mean - 0.375_555).abs() < 0.005, "mean {mean}");
    // Not a flat image: the photo has real contrast.
    let (mut lo, mut hi) = (255u8, 0u8);
    for p in img.data.chunks_exact(4) {
        lo = lo.min(p[1]);
        hi = hi.max(p[1]);
    }
    assert!(hi - lo > 128, "contrast {lo}..{hi}");
}
