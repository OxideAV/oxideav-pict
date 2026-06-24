//! Round-366 integration tests for the QuickDraw `Region` scan-line
//! inversion decoder, exercised end-to-end through `parse_pict`.
//!
//! The region encoding (Inside Macintosh: Imaging With QuickDraw §2,
//! book page 2-15 — `rgnSize` / `rgnBBox` plus the variable inversion
//! tail) closes a horizontal run with an x-flip on the run's right
//! border. When that run reaches the region bbox's right edge, the
//! closing flip lands on `bbox.right` itself — a perfectly legal stream
//! that `PictBuilder::region` will happily emit (its validator accepts
//! `x == bbox_right`). An earlier decoder revision sized its edge
//! accumulator at `width` columns and indexed it with the bbox-local
//! flip column, so a flip at `bbox.right` (local column `width`) drove
//! an out-of-bounds index and panicked the whole decode.
//!
//! These tests pin the fixed behaviour: a region whose run reaches the
//! right border decodes cleanly and rasterises the correct interior,
//! both as a `paintRgn` fill and as a `ClipRgn` mask.

use oxideav_pict::{build_rgn_inverted_op, ops::PictBuilder, ops::Verb, parse_pict};

/// A `paintRgn` whose inside run reaches the bbox right border. The old
/// decoder panicked; the fixed one fills columns [4, 8) on every row.
#[test]
fn paint_region_run_reaching_right_border() {
    let mut b = PictBuilder::new(0, 0, 8, 8);
    b.fg_color(0x10, 0x20, 0x30);
    // y=0 opens a run [4, 8): the closing flip is at x=8 == bbox.right.
    // The run carries to the bbox bottom, covering columns 4..8 on every
    // row.
    let scanlines = [(0i16, [4i16, 8i16].as_slice())];
    b.region(Verb::Paint, 0, 0, 8, 8, &scanlines)
        .expect("encode region with right-border run");
    let bytes = b.finish();

    // The decode must not panic and must succeed.
    let img = parse_pict(&bytes).expect("decode region with right-border run");
    assert_eq!(img.width, 8);
    assert_eq!(img.height, 8);

    for y in 0..8usize {
        for x in 0..8usize {
            let off = (y * 8 + x) * 4;
            if x >= 4 {
                assert_eq!(img.data[off], 0x10, "row {y} col {x} R inside");
                assert_eq!(img.data[off + 1], 0x20, "row {y} col {x} G inside");
                assert_eq!(img.data[off + 2], 0x30, "row {y} col {x} B inside");
            } else {
                // Outside the region: paper white, untouched.
                assert_eq!(img.data[off], 0xFF, "row {y} col {x} paper R");
            }
        }
    }
}

/// A `ClipRgn` whose region run reaches the right border, then a
/// full-frame `paintRect`. Only the clipped columns [3, 8) should take
/// ink; the decoder must materialise the clip mask without panicking.
///
/// The builder has no inverted-clip helper, so the `ClipRgn` opcode
/// (`$0001`) is hand-assembled from `build_rgn_inverted_op`'s region
/// bytes (which carry their own `rgnSize`) and pushed before the paint.
#[test]
fn clip_region_run_reaching_right_border_masks_paint() {
    let mut b = PictBuilder::new(0, 0, 8, 8);

    // build_rgn_inverted_op with a frame-verb emits `[opcode][region]`;
    // we want just the region bytes. Build the region separately by
    // taking the op and stripping its leading 2-byte verb opcode.
    let scanlines = [(0i16, [3i16, 8i16].as_slice())];
    let op =
        build_rgn_inverted_op(Verb::Frame, 0, 0, 8, 8, &scanlines).expect("encode inverted region");
    let region_bytes = &op[2..]; // drop the frameRgn opcode word

    // ClipRgn opcode ($0001) followed by the region bytes.
    let mut clip = Vec::new();
    clip.extend_from_slice(&0x0001u16.to_be_bytes());
    clip.extend_from_slice(region_bytes);
    b.push(&clip);

    b.fg_color(0xAA, 0xBB, 0xCC);
    b.rect(Verb::Paint, 0, 0, 8, 8);
    let bytes = b.finish();

    let img = parse_pict(&bytes).expect("decode clipped paint");
    for y in 0..8usize {
        for x in 0..8usize {
            let off = (y * 8 + x) * 4;
            if x >= 3 {
                assert_eq!(img.data[off], 0xAA, "row {y} col {x} R clipped-in");
                assert_eq!(img.data[off + 1], 0xBB, "row {y} col {x} G clipped-in");
                assert_eq!(img.data[off + 2], 0xCC, "row {y} col {x} B clipped-in");
            } else {
                assert_eq!(img.data[off], 0xFF, "row {y} col {x} clipped-out paper");
            }
        }
    }
}

/// A multi-record region forming an L: full width for the top rows,
/// stem on the left for the bottom rows. Both the wide top run and the
/// stem close on borders. Verifies the running-parity integration across
/// several y records through the full decode path.
#[test]
fn l_shaped_region_through_decoder() {
    let mut b = PictBuilder::new(0, 0, 8, 8);
    b.fg_color(0x01, 0x02, 0x03);
    // y=0: open full width [0, 8) (flips at 0 and 8 == right border).
    // y=4: close columns [4, 8) (flips at 4 and 8) — leaves stem [0, 4).
    let scanlines = [
        (0i16, [0i16, 8i16].as_slice()),
        (4i16, [4i16, 8i16].as_slice()),
    ];
    b.region(Verb::Paint, 0, 0, 8, 8, &scanlines)
        .expect("encode L region");
    let bytes = b.finish();
    let img = parse_pict(&bytes).expect("decode L region");

    for y in 0..8usize {
        for x in 0..8usize {
            let off = (y * 8 + x) * 4;
            let inside = if y < 4 { x < 8 } else { x < 4 };
            if inside {
                assert_eq!(img.data[off], 0x01, "row {y} col {x} inside R");
            } else {
                assert_eq!(img.data[off], 0xFF, "row {y} col {x} outside paper");
            }
        }
    }
}
