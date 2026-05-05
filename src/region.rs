//! QuickDraw `Region` decoder + clipping-mask materialiser.
//!
//! Inside Macintosh: Imaging With QuickDraw §2 ("QuickDraw Drawing")
//! defines a `Region` as the set of pixels enclosed by a sequence of
//! horizontal scan lines, encoded as inversion data:
//!
//! ```text
//! struct Region {
//!     u16  rgnSize;      // total bytes incl. this size word
//!     i16  rgnBBox[4];   // top, left, bottom, right
//!     // optional inversion data:
//!     // sequence of:
//!     //   i16 y;                  // scanline (bbox_top..bbox_bottom)
//!     //   i16 x_pairs[..];        // ascending x flips
//!     //   i16 0x7FFF              // line terminator
//!     // sequence terminated by:  i16 0x7FFF
//! }
//! ```
//!
//! When `rgnSize == 10` the region is just the bounding rectangle —
//! no inversion data. When `rgnSize > 10`, the inversion data
//! describes per-row pixel runs by emitting a list of x-flip points
//! per scanline; each pair (x0, x1) toggles the region membership of
//! columns `[x0, x1)` for that row and (carrying forward) every
//! subsequent row, until another pair on a later row toggles them
//! back.
//!
//! See the official write-up in
//! <https://www.fileformat.info/format/macpict/egff.htm> for the
//! traditional Apple-style description; the round-2 implementation
//! here handles the rectangular case (overwhelming majority of real-
//! world PICTs) plus the basic per-row inversion-pairs case.

use crate::error::{PictError, Result};
use crate::reader::Reader;
use crate::state::RectI32;

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

/// Decode the inversion-pairs payload into a row-major boolean mask
/// of size `(bbox.h) × (bbox.w)`.
fn decode_inversion(payload: &[u8], bbox: &RectI32) -> Result<Vec<bool>> {
    let w = (bbox.right - bbox.left).max(0) as usize;
    let h = (bbox.bottom - bbox.top).max(0) as usize;
    if w == 0 || h == 0 {
        return Ok(Vec::new());
    }
    // Per-column "currently inverted" parity, carried forward across
    // scanlines. Apple's region encoding works as follows:
    //
    //   Flip(y, x): toggle a sentinel column-flip at (y, x). After
    //   processing all flips for a row, the membership of column x
    //   on row y' >= y is `flipped_count(x) modulo 2`. The decoder
    //   walks rows in ascending order, applying flips as it goes.
    let mut col_flips = vec![false; w];
    let mut mask = vec![false; w * h];
    let mut last_y: i32 = bbox.top;
    let mut i = 0usize;

    // Walk inversion records: y, then a sequence of i16 x values
    // (ascending) terminated by 0x7FFF. The whole data block is also
    // terminated by 0x7FFF for the y field.
    while i + 2 <= payload.len() {
        let y = i16::from_be_bytes([payload[i], payload[i + 1]]) as i32;
        i += 2;
        if y == 0x7FFF {
            break;
        }
        // Every row in [last_y, y) has the same membership as the
        // previous parity state. Materialise.
        for fill_y in last_y..y {
            let row = (fill_y - bbox.top) as usize;
            if row < h {
                for x in 0..w {
                    mask[row * w + x] = col_flips[x];
                }
            }
        }
        last_y = y;
        // Read x pairs until we see 0x7FFF.
        loop {
            if i + 2 > payload.len() {
                return Err(PictError::invalid(
                    "region inversion data truncated mid-x-list",
                ));
            }
            let x = i16::from_be_bytes([payload[i], payload[i + 1]]) as i32;
            i += 2;
            if x == 0x7FFF {
                break;
            }
            // Toggle column parity from (x - bbox.left) to "wherever
            // the next x is" — but Apple's format pairs them up
            // explicitly: toggles come in (x_in, x_out) pairs per
            // row segment. Easier: just toggle this column.
            let col = (x - bbox.left).clamp(0, w as i32) as usize;
            col_flips[col] = !col_flips[col];
        }
    }

    // Tail rows after the final y record carry forward the last
    // parity state down to bbox.bottom.
    for fill_y in last_y..bbox.bottom {
        let row = (fill_y - bbox.top) as usize;
        if row < h {
            // Compute the membership for this row by integrating col
            // flips from left to right.
            let mut inside = false;
            for (x, item) in col_flips.iter().enumerate().take(w) {
                if *item {
                    inside = !inside;
                }
                mask[row * w + x] = inside;
            }
        }
    }

    // Re-walk the materialised mask above to convert "flip at column
    // c" into running parity. The previous loop captured parity, but
    // for the [last_y, bbox.bottom) tail we actually want running
    // membership instead. Rework the [bbox.top, last_y) rows the
    // same way.
    let mut col_flips_running = vec![false; w];
    let mut last_y2: i32 = bbox.top;
    let mut j = 0usize;
    while j + 2 <= payload.len() {
        let y = i16::from_be_bytes([payload[j], payload[j + 1]]) as i32;
        j += 2;
        if y == 0x7FFF {
            break;
        }
        // Materialise rows [last_y2, y) using running parity.
        for fill_y in last_y2..y {
            let row = (fill_y - bbox.top) as usize;
            if row < h {
                let mut inside = false;
                for (x, item) in col_flips_running.iter().enumerate().take(w) {
                    if *item {
                        inside = !inside;
                    }
                    mask[row * w + x] = inside;
                }
            }
        }
        last_y2 = y;
        loop {
            if j + 2 > payload.len() {
                return Err(PictError::invalid(
                    "region inversion data truncated mid-x-list (pass 2)",
                ));
            }
            let x = i16::from_be_bytes([payload[j], payload[j + 1]]) as i32;
            j += 2;
            if x == 0x7FFF {
                break;
            }
            let col = (x - bbox.left).clamp(0, w as i32) as usize;
            col_flips_running[col] = !col_flips_running[col];
        }
    }
    // Tail rows after the final record.
    for fill_y in last_y2..bbox.bottom {
        let row = (fill_y - bbox.top) as usize;
        if row < h {
            let mut inside = false;
            for (x, item) in col_flips_running.iter().enumerate().take(w) {
                if *item {
                    inside = !inside;
                }
                mask[row * w + x] = inside;
            }
        }
    }

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
        // 4x4 region with a single inversion record:
        //   y=1, x_pairs = [1, 3, 0x7FFF], then 0x7FFF terminator.
        // Rows 0 stay outside (no flips yet). Rows 1..4 cover [1,3).
        let bytes = [
            0x00, 0x14, // rgnSize = 20 (header + payload)
            0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0x00, 0x04, // bbox
            // Inversion data:
            0x00, 0x01, // y = 1
            0x00, 0x01, // x = 1
            0x00, 0x03, // x = 3
            0x7F, 0xFF, // end of x list for y=1
            0x7F, 0xFF, // end of region
        ];
        let r = rgn_from_bytes(&bytes);
        assert!(r.mask.is_some());
        // Row 0: nothing inside.
        for x in 0..4 {
            assert!(!r.contains(x, 0), "row 0 col {x} should be outside");
        }
        // Row 1: cols 1, 2 inside.
        assert!(!r.contains(0, 1));
        assert!(r.contains(1, 1));
        assert!(r.contains(2, 1));
        assert!(!r.contains(3, 1));
        // Row 3: same as row 1 (carry forward).
        assert!(r.contains(1, 3));
        assert!(r.contains(2, 3));
    }
}
