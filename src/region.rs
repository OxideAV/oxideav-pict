//! QuickDraw `Region` decoder + clipping-mask materialiser.
//!
//! Inside Macintosh: Imaging With QuickDraw §2 ("About QuickDraw")
//! documents the *fixed* part of a `Region` (book page 2-15):
//!
//! ```text
//! TYPE Region = RECORD
//!     rgnSize: Integer;   {size in bytes, including this word}
//!     rgnBBox: Rect;      {enclosing rectangle (top, left, bottom, right)}
//!     {more data if region is not rectangular}
//! END;
//! ```
//!
//! The book states (book page 2-15): *"For rectangular regions (or empty
//! regions), the `rgnSize` field contains 10. The data for more complex
//! regions is stored in a proprietary format."* So a `rgnSize == 10`
//! region is exactly its bounding rectangle and carries no further data —
//! that case is fully spec-determined.
//!
//! The variable-length tail for a non-rectangular region is the classic
//! QuickDraw scan-line **inversion encoding**: a sequence of per-row
//! records, each `[y][x0][x1]…[xN][0x7FFF]`, with the whole sequence
//! terminated by a top-level `[0x7FFF]`. Each `x` in a record toggles a
//! vertical region edge at column `x`, in effect for that scan line and
//! every later scan line until another record at a lower `y` toggles it
//! back. A scan line's membership is recovered by integrating the edge
//! toggles left-to-right (running parity): an odd number of edges to the
//! left of column `c` means `c` is inside the region.
//!
//! ## Decoder contract
//!
//! * `rgnSize == 10` → `mask == None`; the whole bbox is the region.
//! * `rgnSize > 10` → the inversion tail is decoded into a row-major
//!   boolean coverage mask in a single forward pass.
//! * Edge `x` coordinates run over `[bbox.left ..= bbox.right]`
//!   **inclusive** — the closing edge of a run that reaches the right
//!   border lands exactly on `bbox.right`, so the edge accumulator spans
//!   `width + 1` columns. (An earlier revision sized it at `width` and
//!   panicked on that extremely common case.)
//! * Coordinates outside the bbox, out-of-order `y` records, and a
//!   missing top-level terminator are all tolerated: anything below the
//!   last record carries the running edge parity down to `bbox.bottom`
//!   (well-formed regions cancel all edges by then and leave it empty).

use crate::error::{PictError, Result};
use crate::reader::Reader;
use crate::state::RectI32;

/// QuickDraw's scan-line / x-list terminator sentinel.
const RGN_END: i16 = 0x7FFF;

/// A QuickDraw region: bbox plus an optional per-row coverage mask
/// (`true` = inside region, `false` = outside).
#[derive(Debug, Clone)]
pub struct Region {
    pub bbox: RectI32,
    /// `Some(mask)` if inversion data was supplied; `mask` is
    /// `(bbox.bottom - bbox.top) × (bbox.right - bbox.left)` row-major
    /// booleans. `None` means the entire bbox is the region.
    pub mask: Option<Vec<bool>>,
}

impl Region {
    /// Width of the region's bbox.
    pub fn width(&self) -> i32 {
        self.bbox.right - self.bbox.left
    }
    /// Height of the region's bbox.
    pub fn height(&self) -> i32 {
        self.bbox.bottom - self.bbox.top
    }
    /// True if pixel `(x, y)` (in picture-frame coordinates) is
    /// inside the region.
    pub fn contains(&self, x: i32, y: i32) -> bool {
        if x < self.bbox.left || x >= self.bbox.right || y < self.bbox.top || y >= self.bbox.bottom
        {
            return false;
        }
        match &self.mask {
            None => true,
            Some(mask) => {
                let lx = (x - self.bbox.left) as usize;
                let ly = (y - self.bbox.top) as usize;
                let w = self.width().max(0) as usize;
                mask[ly * w + lx]
            }
        }
    }
}

/// Parse a `Region` starting at the cursor's current position.
/// Consumes exactly the byte count claimed by `rgnSize`.
pub fn parse_region(r: &mut Reader<'_>) -> Result<Region> {
    let rgn_size = r.read_u16()? as usize;
    if rgn_size < 10 {
        return Err(PictError::invalid(format!(
            "region size {rgn_size} smaller than the 10-byte header"
        )));
    }
    let bbox = r.read_rect()?;
    let bbox = RectI32::from_be(bbox.0, bbox.1, bbox.2, bbox.3);
    let payload_len = rgn_size - 10;
    if payload_len == 0 {
        return Ok(Region { bbox, mask: None });
    }
    // Pull all the inversion data into a local buffer so we can
    // index over it without contorting the Reader.
    let payload = r.read_bytes(payload_len)?;
    let mask = decode_inversion(payload, &bbox)?;
    Ok(Region {
        bbox,
        mask: Some(mask),
    })
}

/// Decode the scan-line inversion tail into a row-major boolean coverage
/// mask of size `(bbox.h) × (bbox.w)`.
///
/// Single forward pass over the records. `edge[c]` is the running parity
/// of vertical region edges at bbox-local column `c` (`0 ..= w`); a row's
/// membership at column `x` is the parity of the edges in `0..=x`, i.e. a
/// left-to-right XOR scan. `edge` is `w + 1` wide because a run that ends
/// on the right border toggles an edge at `bbox.right` (local column `w`).
fn decode_inversion(payload: &[u8], bbox: &RectI32) -> Result<Vec<bool>> {
    let w = (bbox.right - bbox.left).max(0) as usize;
    let h = (bbox.bottom - bbox.top).max(0) as usize;
    if w == 0 || h == 0 {
        return Ok(Vec::new());
    }

    // edge[c] toggles when a vertical region boundary crosses local
    // column c; sized w + 1 so a closing flip at bbox.right is in range.
    let mut edge = vec![false; w + 1];
    let mut mask = vec![false; w * h];
    // The first record's rows start at the bbox top; everything above the
    // first y stays outside (no edges yet, which the integration below
    // produces anyway).
    let mut prev_y: i32 = bbox.top;
    let mut i = 0usize;

    // Materialise rows [from, to) into `mask` using the current `edge`
    // parity. `to` is clamped to the bbox so an out-of-range y can't write
    // past the buffer.
    let emit_rows = |mask: &mut [bool], edge: &[bool], from: i32, to: i32| {
        let from = from.max(bbox.top);
        let to = to.min(bbox.bottom);
        for fy in from..to {
            let row = (fy - bbox.top) as usize;
            let base = row * w;
            let mut inside = false;
            for x in 0..w {
                // The edge at local column x flips membership *before*
                // column x is plotted (an edge sits on the left border of
                // the column it turns on).
                if edge[x] {
                    inside = !inside;
                }
                mask[base + x] = inside;
            }
        }
    };

    while i + 2 <= payload.len() {
        let y = i16::from_be_bytes([payload[i], payload[i + 1]]);
        i += 2;
        if y == RGN_END {
            break;
        }
        let y = y as i32;
        // Rows from the previous record's y up to (but not including)
        // this one share the previous edge parity.
        emit_rows(&mut mask, &edge, prev_y, y);
        prev_y = y;

        // Apply this record's x flips to the edge accumulator.
        loop {
            if i + 2 > payload.len() {
                return Err(PictError::invalid(
                    "region inversion data truncated mid-x-list",
                ));
            }
            let x = i16::from_be_bytes([payload[i], payload[i + 1]]);
            i += 2;
            if x == RGN_END {
                break;
            }
            // Clamp to the bbox-local edge range [0, w]; coordinates
            // outside the declared bbox are a malformed-but-survivable
            // input rather than a panic.
            let col = (x as i32 - bbox.left).clamp(0, w as i32) as usize;
            edge[col] = !edge[col];
        }
    }

    // Rows below the final record carry the running parity to the bbox
    // bottom. For a well-formed region every edge has cancelled by now, so
    // these rows come out empty; materialising them anyway keeps a
    // truncated / malformed tail from leaving stale `false`s that happen
    // to be correct only by luck.
    emit_rows(&mut mask, &edge, prev_y, bbox.bottom);

    Ok(mask)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgn_from_bytes(bytes: &[u8]) -> Region {
        let mut r = Reader::new(bytes);
        parse_region(&mut r).expect("region parse failed")
    }

    #[test]
    fn rectangular_region() {
        // rgnSize = 10, bbox = (0, 0, 4, 4). No inversion data.
        let bytes = [
            0x00, 0x0A, // rgnSize = 10
            0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0x00, 0x04, // bbox
        ];
        let r = rgn_from_bytes(&bytes);
        assert_eq!(r.bbox.right - r.bbox.left, 4);
        assert!(r.mask.is_none());
        for x in 0..4 {
            for y in 0..4 {
                assert!(r.contains(x, y));
            }
        }
        assert!(!r.contains(-1, 0));
        assert!(!r.contains(4, 0));
    }

    #[test]
    fn non_rectangular_region_simple() {
        // 4x4 region. Record y=1 opens a run [1,3); a later record y=4
        // closes it (edges at 1 and 3 toggled back). Rows 1..4 cover
        // columns 1,2.
        let bytes = [
            0x00, 0x1A, // rgnSize = 26 (header + payload)
            0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0x00, 0x04, // bbox
            // Record: y = 1, x = 1, x = 3
            0x00, 0x01, 0x00, 0x01, 0x00, 0x03, 0x7F, 0xFF,
            // Record: y = 4, x = 1, x = 3 (close the run)
            0x00, 0x04, 0x00, 0x01, 0x00, 0x03, 0x7F, 0xFF, //
            0x7F, 0xFF, // end of region
        ];
        let r = rgn_from_bytes(&bytes);
        assert!(r.mask.is_some());
        // Row 0: nothing inside.
        for x in 0..4 {
            assert!(!r.contains(x, 0), "row 0 col {x} should be outside");
        }
        // Rows 1..4: cols 1, 2 inside.
        for y in 1..4 {
            assert!(!r.contains(0, y));
            assert!(r.contains(1, y), "row {y} col 1");
            assert!(r.contains(2, y), "row {y} col 2");
            assert!(!r.contains(3, y));
        }
    }

    #[test]
    fn run_reaching_right_border_does_not_panic() {
        // 4x4 region whose run extends to the right border: y=0 opens
        // a run [2, 4) — the closing edge lands exactly on bbox.right
        // (local column w == 4). Earlier revisions panicked here.
        let bytes = [
            0x00, 0x1A, // rgnSize = 26
            0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0x00, 0x04, // bbox 0,0,4,4
            // y = 0: edges at x=2 and x=4 (== right border)
            0x00, 0x00, 0x00, 0x02, 0x00, 0x04, 0x7F, 0xFF, // y = 4: close
            0x00, 0x04, 0x00, 0x02, 0x00, 0x04, 0x7F, 0xFF, //
            0x7F, 0xFF,
        ];
        let r = rgn_from_bytes(&bytes);
        for y in 0..4 {
            assert!(!r.contains(0, y));
            assert!(!r.contains(1, y));
            assert!(r.contains(2, y), "row {y} col 2 inside");
            assert!(r.contains(3, y), "row {y} col 3 inside");
        }
    }

    #[test]
    fn l_shaped_region_two_segments() {
        // 4-tall, 4-wide L: rows 0..2 cover cols [0,4); rows 2..4 cover
        // cols [0,2). Encoded as:
        //   y=0: x=0, x=4        (open full width)
        //   y=2: x=2, x=4        (close cols [2,4) — leaves [0,2))
        //   y=4: x=0, x=2        (close the stem)
        let bytes = [
            0x00, 0x22, // rgnSize = 34
            0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0x00, 0x04, // bbox 0,0,4,4
            0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0x7F, 0xFF, // y=0 [0,4)
            0x00, 0x02, 0x00, 0x02, 0x00, 0x04, 0x7F, 0xFF, // y=2 close [2,4)
            0x00, 0x04, 0x00, 0x00, 0x00, 0x02, 0x7F, 0xFF, // y=4 close [0,2)
            0x7F, 0xFF,
        ];
        let r = rgn_from_bytes(&bytes);
        // Rows 0,1: full width.
        for y in 0..2 {
            for x in 0..4 {
                assert!(r.contains(x, y), "row {y} col {x} should be inside");
            }
        }
        // Rows 2,3: only cols 0,1.
        for y in 2..4 {
            assert!(r.contains(0, y), "row {y} col 0");
            assert!(r.contains(1, y), "row {y} col 1");
            assert!(!r.contains(2, y), "row {y} col 2 should be outside");
            assert!(!r.contains(3, y), "row {y} col 3 should be outside");
        }
    }

    #[test]
    fn truncated_x_list_is_error() {
        // A record opens but the x-list runs off the end with no 0x7FFF.
        let bytes = [
            0x00, 0x10, // rgnSize = 16
            0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0x00, 0x04, // bbox
            0x00, 0x01, 0x00, 0x01, // y=1, x=1, then EOF (no terminator)
        ];
        let mut r = Reader::new(&bytes);
        assert!(parse_region(&mut r).is_err());
    }

    #[test]
    fn region_smaller_than_header_rejected() {
        let bytes = [0x00, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x04];
        let mut r = Reader::new(&bytes);
        assert!(parse_region(&mut r).is_err());
    }

    #[test]
    fn empty_bbox_region_has_empty_mask() {
        // Degenerate bbox (zero width) with a payload: mask is empty,
        // contains() is always false.
        let bytes = [
            0x00, 0x0E, // rgnSize = 14
            0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, // bbox 0,0,4,0 (w=0)
            0x7F, 0xFF, 0x7F, 0xFF, // payload: just terminators
        ];
        let r = rgn_from_bytes(&bytes);
        assert!(r.mask.is_some());
        assert!(!r.contains(0, 0));
    }
}
