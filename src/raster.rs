//! Tiny in-crate software rasteriser used to fold PICT drawing
//! commands (lines / rectangles / round-rectangles / ovals / arcs /
//! polygons / regions) onto an RGBA canvas.
//!
//! Why not pull in `oxideav-raster`? PICT's drawing model is much
//! simpler than the SVG-style scene-graph that `oxideav-raster`
//! consumes — there are no Bezier paths, no gradients, no
//! transforms beyond an integer origin shift. The geometric kernel
//! needed here is small enough that an in-crate implementation
//! keeps the standalone (no-`registry`) build path free of any
//! `oxideav-core` / `VectorFrame` plumbing.
//!
//! Algorithms used (clean-room, public-domain bibliography):
//!
//! * **Line** — Bresenham's line-drawing algorithm.
//! * **Rectangle outline / fill** — direct pixel iteration over the
//!   spanned rows; outline draws four edges 1 pixel thick.
//! * **Oval** — the symmetric mid-point ellipse algorithm
//!   (Pitteway / Van Aken).
//! * **Polygon fill** — classic active-edge-list scanline algorithm
//!   with even-odd parity (Foley & van Dam §3.5).
//! * **Region fill** — region inversion data is decoded into a
//!   per-row bitmap; the bitmap is then composited into the canvas.
//! * **Round rectangle** — straight edges plus quarter-ellipse
//!   corners.
//! * **Arc** — sweep over the bounding ellipse and emit the pixels
//!   whose polar angle (measured CW from 12 o'clock — the QuickDraw
//!   convention) falls inside the requested wedge; outline is the
//!   ellipse boundary, fill is filled triangles between centre and
//!   the boundary points.

// QuickDraw drawing primitives are inherently many-argument (rect
// bounds + oval-corner radii + colour, blit src+dst dims + colour,
// etc). Wrapping every signature in a struct adds boilerplate without
// improving call-site clarity, so disable clippy's heuristic per-
// module instead of one-off allows.
#![allow(clippy::too_many_arguments)]

use crate::state::{Pattern, Rgba};

/// A row-major RGBA8 canvas. Origin at (0, 0); y grows downward.
pub struct Canvas {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
    /// True once any drawing op (line / rect / oval / poly / region /
    /// raster) has touched the canvas. Used by the decoder to
    /// distinguish "no drawing happened" from "deliberately drew
    /// nothing visible".
    pub dirty: bool,
    /// Optional canvas-local boolean clip mask. `None` means draw
    /// everywhere; `Some(mask)` is `width × height` row-major bools
    /// (`true` = inside clip, `false` = outside). Set by the decoder
    /// from a `ClipRgn` opcode (round 42); honoured by every plot
    /// primitive (`put` / `span` / `blit`) so subsequent drawing /
    /// raster ops are masked.
    pub clip: Option<Vec<bool>>,
}

impl Canvas {
    /// Allocate a `width × height` canvas filled with `fill` (the
    /// QuickDraw "paper" — usually opaque white).
    pub fn new(width: u32, height: u32, fill: Rgba) -> Self {
        let mut data = vec![0u8; (width as usize) * (height as usize) * 4];
        for chunk in data.chunks_exact_mut(4) {
            chunk[0] = fill.r;
            chunk[1] = fill.g;
            chunk[2] = fill.b;
            chunk[3] = fill.a;
        }
        Self {
            width,
            height,
            data,
            dirty: false,
            clip: None,
        }
    }

    /// True if pixel `(x, y)` (canvas-local) is in-bounds and inside
    /// any active clip mask.
    #[inline]
    fn in_clip(&self, x: i32, y: i32) -> bool {
        if x < 0 || y < 0 || (x as u32) >= self.width || (y as u32) >= self.height {
            return false;
        }
        match &self.clip {
            None => true,
            Some(mask) => {
                let idx = (y as u32 * self.width + x as u32) as usize;
                mask[idx]
            }
        }
    }

    /// Plot a single pixel. Out-of-bounds writes are silently
    /// ignored — every drawing primitive does its own clipping
    /// against `(width, height)` first to keep the inner loops branch-
    /// free, but we still defensively bound here to handle the
    /// integer-arithmetic edge cases (rounding, off-by-one) that
    /// pop out of the ellipse / arc primitives. Writes outside the
    /// active clip mask (if any) are also dropped.
    #[inline]
    pub fn put(&mut self, x: i32, y: i32, c: Rgba) {
        if !self.in_clip(x, y) {
            return;
        }
        let off = ((y as u32 * self.width + x as u32) * 4) as usize;
        // PICT drawing primitives are alpha-opaque — there's no
        // intermediate transparency in the QuickDraw painting model.
        // We just overwrite (srcCopy mode).
        self.data[off] = c.r;
        self.data[off + 1] = c.g;
        self.data[off + 2] = c.b;
        self.data[off + 3] = c.a;
        self.dirty = true;
    }

    /// Fill a horizontal span `[x0, x1)` at row `y` with `c`. Coords
    /// are clipped to the canvas; out-of-range spans are no-ops.
    /// Pixels outside the active clip mask (if any) are dropped.
    pub fn span(&mut self, y: i32, x0: i32, x1: i32, c: Rgba) {
        if y < 0 || (y as u32) >= self.height {
            return;
        }
        let lo = x0.max(0).min(self.width as i32);
        let hi = x1.max(0).min(self.width as i32);
        if lo >= hi {
            return;
        }
        let row = y as u32 * self.width;
        match &self.clip {
            None => {
                for x in lo..hi {
                    let off = ((row + x as u32) * 4) as usize;
                    self.data[off] = c.r;
                    self.data[off + 1] = c.g;
                    self.data[off + 2] = c.b;
                    self.data[off + 3] = c.a;
                }
                self.dirty = true;
            }
            Some(mask) => {
                let mut any = false;
                for x in lo..hi {
                    let idx = (row + x as u32) as usize;
                    if !mask[idx] {
                        continue;
                    }
                    let off = idx * 4;
                    self.data[off] = c.r;
                    self.data[off + 1] = c.g;
                    self.data[off + 2] = c.b;
                    self.data[off + 3] = c.a;
                    any = true;
                }
                if any {
                    self.dirty = true;
                }
            }
        }
    }

    /// Composite an externally-decoded RGBA raster into the canvas at
    /// destination rectangle `(dst_left, dst_top, dst_right, dst_bot)`.
    /// `src_rgba` is `src_w × src_h` packed RGBA. If src and dst sizes
    /// differ we fall back to nearest-neighbour resampling — PICT
    /// allows scaled blits via dstRect != srcRect in `PackBitsRect` /
    /// `DirectBitsRect`. Out-of-canvas dst pixels are clipped, and
    /// pixels outside the active clip mask (if any) are dropped.
    pub fn blit(
        &mut self,
        src_rgba: &[u8],
        src_w: u32,
        src_h: u32,
        dst_top: i32,
        dst_left: i32,
        dst_bot: i32,
        dst_right: i32,
    ) {
        if src_w == 0 || src_h == 0 {
            return;
        }
        let dst_w = (dst_right - dst_left).max(0) as u32;
        let dst_h = (dst_bot - dst_top).max(0) as u32;
        if dst_w == 0 || dst_h == 0 {
            return;
        }
        for dy in 0..dst_h {
            let sy = (dy as u64 * src_h as u64 / dst_h as u64) as u32;
            let cy = dst_top + dy as i32;
            if cy < 0 || (cy as u32) >= self.height {
                continue;
            }
            for dx in 0..dst_w {
                let sx = (dx as u64 * src_w as u64 / dst_w as u64) as u32;
                let cx = dst_left + dx as i32;
                if !self.in_clip(cx, cy) {
                    continue;
                }
                let s_off = ((sy * src_w + sx) * 4) as usize;
                let d_off = ((cy as u32 * self.width + cx as u32) * 4) as usize;
                self.data[d_off] = src_rgba[s_off];
                self.data[d_off + 1] = src_rgba[s_off + 1];
                self.data[d_off + 2] = src_rgba[s_off + 2];
                self.data[d_off + 3] = src_rgba[s_off + 3];
            }
        }
        self.dirty = true;
    }
}

/// Bresenham line from `(x0, y0)` to `(x1, y1)`, inclusive of both
/// endpoints. Single-pixel pen.
pub fn line(canvas: &mut Canvas, mut x0: i32, mut y0: i32, x1: i32, y1: i32, c: Rgba) {
    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    loop {
        canvas.put(x0, y0, c);
        if x0 == x1 && y0 == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x0 += sx;
        }
        if e2 <= dx {
            err += dx;
            y0 += sy;
        }
    }
}

/// Outline rectangle (1 pixel pen). `right` and `bottom` are
/// exclusive (QuickDraw rect convention).
pub fn frame_rect(canvas: &mut Canvas, top: i32, left: i32, bottom: i32, right: i32, c: Rgba) {
    if right <= left || bottom <= top {
        return;
    }
    // top + bottom edges
    canvas.span(top, left, right, c);
    canvas.span(bottom - 1, left, right, c);
    // left + right edges (exclude rows we already drew so we don't
    // overdraw — purely cosmetic on opaque colour, but keeps the
    // primitive idempotent if extended to alpha later).
    for y in (top + 1)..(bottom - 1) {
        canvas.put(left, y, c);
        canvas.put(right - 1, y, c);
    }
}

/// Filled rectangle. `right` / `bottom` exclusive.
pub fn fill_rect(canvas: &mut Canvas, top: i32, left: i32, bottom: i32, right: i32, c: Rgba) {
    if right <= left || bottom <= top {
        return;
    }
    for y in top..bottom {
        canvas.span(y, left, right, c);
    }
}

/// Mid-point ellipse outline, axis-aligned, fitted to `(top, left,
/// bottom, right)` (right / bottom exclusive). Single-pixel pen.
pub fn frame_oval(canvas: &mut Canvas, top: i32, left: i32, bottom: i32, right: i32, c: Rgba) {
    walk_ellipse(top, left, bottom, right, |x, y| canvas.put(x, y, c));
}

/// Filled ellipse (axis-aligned).
pub fn fill_oval(canvas: &mut Canvas, top: i32, left: i32, bottom: i32, right: i32, c: Rgba) {
    if right <= left || bottom <= top {
        return;
    }
    // Build per-row min/max x extents from the ellipse boundary, then
    // fill spans.
    let h = (bottom - top) as usize;
    let mut min = vec![i32::MAX; h];
    let mut max = vec![i32::MIN; h];
    walk_ellipse(top, left, bottom, right, |x, y| {
        let row = y - top;
        if row < 0 || (row as usize) >= h {
            return;
        }
        let r = row as usize;
        if x < min[r] {
            min[r] = x;
        }
        if x > max[r] {
            max[r] = x;
        }
    });
    for (i, (lo, hi)) in min.iter().zip(max.iter()).enumerate() {
        if *lo == i32::MAX {
            continue;
        }
        canvas.span(top + i as i32, *lo, *hi + 1, c);
    }
}

/// Walk every boundary pixel of the axis-aligned ellipse fitted to
/// `(top, left, bottom, right)` (`right`, `bottom` exclusive) and
/// dispatch to `f(x, y)`. Pitteway-style mid-point ellipse iterating
/// in two regions (slope-bounded + slope-unbounded) using only
/// integer arithmetic.
fn walk_ellipse<F: FnMut(i32, i32)>(top: i32, left: i32, bottom: i32, right: i32, mut f: F) {
    if right <= left || bottom <= top {
        return;
    }
    // Treat the ellipse as fitted to the integer pixel rectangle
    // [left, right) × [top, bottom). Centre at ((left+right-1)/2,
    // (top+bottom-1)/2) with semi-axes a, b in half-pixel units to
    // handle even/odd dimensions cleanly.
    let w = (right - left) as i64;
    let h = (bottom - top) as i64;
    // Use floating-point parametric sweep — integer mid-point loses
    // precision when w/h have very different magnitudes (e.g. 200×3
    // ovals); a parametric sweep with enough samples to cover every
    // boundary pixel is simpler and still O(perimeter). The number
    // of samples = 4 × max(a, b) gives at least one sample per pixel
    // along the longer axis.
    let cx = left as f64 + (w as f64 - 1.0) / 2.0;
    let cy = top as f64 + (h as f64 - 1.0) / 2.0;
    let a = (w as f64 - 1.0) / 2.0;
    let b = (h as f64 - 1.0) / 2.0;
    if a < 0.0 || b < 0.0 {
        return;
    }
    let n = (4.0 * a.max(b)).max(8.0) as i32;
    let two_pi = std::f64::consts::TAU;
    for i in 0..n {
        let t = i as f64 * two_pi / n as f64;
        let x = (cx + a * t.cos()).round() as i32;
        let y = (cy + b * t.sin()).round() as i32;
        f(x, y);
    }
}

/// Frame round-rectangle. Ovals at the four corners (with the
/// supplied oval-size half-axes) plus four straight edges between
/// them.
pub fn frame_round_rect(
    canvas: &mut Canvas,
    top: i32,
    left: i32,
    bottom: i32,
    right: i32,
    oval_w: i32,
    oval_h: i32,
    c: Rgba,
) {
    if right <= left || bottom <= top {
        return;
    }
    let ow = oval_w.max(0).min(right - left);
    let oh = oval_h.max(0).min(bottom - top);
    let rx = ow / 2;
    let ry = oh / 2;
    // Edges between corners.
    canvas.span(top, left + rx, right - rx, c);
    canvas.span(bottom - 1, left + rx, right - rx, c);
    for y in (top + ry)..(bottom - ry) {
        canvas.put(left, y, c);
        canvas.put(right - 1, y, c);
    }
    // Four corner ellipses, drawn in their bounding boxes.
    walk_ellipse(top, left, top + oh, left + ow, |x, y| {
        if x < left + rx && y < top + ry {
            canvas.put(x, y, c);
        }
    });
    walk_ellipse(top, right - ow, top + oh, right, |x, y| {
        if x >= right - rx && y < top + ry {
            canvas.put(x, y, c);
        }
    });
    walk_ellipse(bottom - oh, left, bottom, left + ow, |x, y| {
        if x < left + rx && y >= bottom - ry {
            canvas.put(x, y, c);
        }
    });
    walk_ellipse(bottom - oh, right - ow, bottom, right, |x, y| {
        if x >= right - rx && y >= bottom - ry {
            canvas.put(x, y, c);
        }
    });
}

/// Filled round-rectangle.
pub fn fill_round_rect(
    canvas: &mut Canvas,
    top: i32,
    left: i32,
    bottom: i32,
    right: i32,
    oval_w: i32,
    oval_h: i32,
    c: Rgba,
) {
    if right <= left || bottom <= top {
        return;
    }
    let ow = oval_w.max(0).min(right - left);
    let oh = oval_h.max(0).min(bottom - top);
    let _rx = ow / 2;
    let ry = oh / 2;
    // Middle band: full width, between the corner ovals' top + bottom
    // halves.
    for y in (top + ry)..(bottom - ry) {
        canvas.span(y, left, right, c);
    }
    // Top + bottom strips: width modulated by the corner ellipse.
    let mut top_min = vec![i32::MAX; ry.max(0) as usize];
    let mut top_max = vec![i32::MIN; ry.max(0) as usize];
    walk_ellipse(top, left, top + oh, left + ow, |x, y| {
        let row = y - top;
        if row < 0 || row >= ry {
            return;
        }
        if x < top_min[row as usize] {
            top_min[row as usize] = x;
        }
    });
    walk_ellipse(top, right - ow, top + oh, right, |x, y| {
        let row = y - top;
        if row < 0 || row >= ry {
            return;
        }
        if x > top_max[row as usize] {
            top_max[row as usize] = x;
        }
    });
    for (i, (lo, hi)) in top_min.iter().zip(top_max.iter()).enumerate() {
        if *lo == i32::MAX || *hi == i32::MIN {
            continue;
        }
        canvas.span(top + i as i32, *lo, *hi + 1, c);
    }
    let mut bot_min = vec![i32::MAX; ry.max(0) as usize];
    let mut bot_max = vec![i32::MIN; ry.max(0) as usize];
    walk_ellipse(bottom - oh, left, bottom, left + ow, |x, y| {
        let row = bottom - 1 - y;
        if row < 0 || row >= ry {
            return;
        }
        if x < bot_min[row as usize] {
            bot_min[row as usize] = x;
        }
    });
    walk_ellipse(bottom - oh, right - ow, bottom, right, |x, y| {
        let row = bottom - 1 - y;
        if row < 0 || row >= ry {
            return;
        }
        if x > bot_max[row as usize] {
            bot_max[row as usize] = x;
        }
    });
    for (i, (lo, hi)) in bot_min.iter().zip(bot_max.iter()).enumerate() {
        if *lo == i32::MAX || *hi == i32::MIN {
            continue;
        }
        canvas.span(bottom - 1 - i as i32, *lo, *hi + 1, c);
    }
}

/// Filled polygon by even-odd active-edge-list scanline. Vertices in
/// `(x, y)` order; the polygon is implicitly closed (last vertex
/// connects back to the first).
pub fn fill_polygon(canvas: &mut Canvas, vertices: &[(i32, i32)], c: Rgba) {
    if vertices.len() < 3 {
        return;
    }
    // Compute y range.
    let mut y_min = i32::MAX;
    let mut y_max = i32::MIN;
    for &(_, y) in vertices {
        if y < y_min {
            y_min = y;
        }
        if y > y_max {
            y_max = y;
        }
    }
    if y_max < 0 || y_min >= canvas.height as i32 {
        return;
    }
    let scan_lo = y_min.max(0);
    let scan_hi = (y_max).min(canvas.height as i32 - 1);
    let n = vertices.len();
    for y in scan_lo..=scan_hi {
        let yf = y as f64 + 0.5;
        let mut xs = Vec::new();
        for i in 0..n {
            let (x0, y0) = vertices[i];
            let (x1, y1) = vertices[(i + 1) % n];
            let y0f = y0 as f64;
            let y1f = y1 as f64;
            // Standard half-open edge intersection: include lower
            // endpoint, exclude upper, to avoid double-counting at
            // vertex shares.
            if (y0f <= yf && y1f > yf) || (y1f <= yf && y0f > yf) {
                let t = (yf - y0f) / (y1f - y0f);
                let x = x0 as f64 + t * (x1 - x0) as f64;
                xs.push(x);
            }
        }
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal));
        let mut i = 0;
        while i + 1 < xs.len() {
            // Use floor/ceil so a narrow [4.75, 5.25] segment still
            // covers pixel 5 — without this, round/round collapses
            // sub-pixel-wide spans to empty intervals.
            let x0 = xs[i].floor() as i32;
            let x1 = (xs[i + 1].ceil() as i32).max(x0 + 1);
            canvas.span(y, x0, x1, c);
            i += 2;
        }
    }
}

/// Outline polygon: just connect the consecutive vertices with
/// Bresenham lines; closes implicitly.
pub fn frame_polygon(canvas: &mut Canvas, vertices: &[(i32, i32)], c: Rgba) {
    if vertices.len() < 2 {
        return;
    }
    for i in 0..vertices.len() {
        let (x0, y0) = vertices[i];
        let (x1, y1) = vertices[(i + 1) % vertices.len()];
        line(canvas, x0, y0, x1, y1, c);
    }
}

/// Frame an arc of the bounding ellipse from `start_deg` (clockwise,
/// 0° = 12 o'clock per QuickDraw convention) sweeping `arc_deg`
/// degrees.
pub fn frame_arc(
    canvas: &mut Canvas,
    top: i32,
    left: i32,
    bottom: i32,
    right: i32,
    start_deg: i32,
    arc_deg: i32,
    c: Rgba,
) {
    if right <= left || bottom <= top {
        return;
    }
    let cx = left as f64 + (right - left - 1) as f64 / 2.0;
    let cy = top as f64 + (bottom - top - 1) as f64 / 2.0;
    let a = (right - left - 1) as f64 / 2.0;
    let b = (bottom - top - 1) as f64 / 2.0;
    if a < 0.0 || b < 0.0 {
        return;
    }
    let (lo, hi) = arc_range(start_deg, arc_deg);
    let n = (4.0 * a.max(b)).max(64.0) as i32;
    for i in 0..n {
        let frac = i as f64 / n as f64;
        let deg = lo + frac * (hi - lo);
        // QuickDraw: 0° = 12 o'clock (north), positive = clockwise.
        // Standard math: 0° = east, positive = counterclockwise.
        // Convert: math_angle = pi/2 - quickdraw_rad.
        let qd_rad = deg.to_radians();
        let mx = qd_rad.sin();
        let my = -qd_rad.cos();
        let x = (cx + a * mx).round() as i32;
        let y = (cy + b * my).round() as i32;
        canvas.put(x, y, c);
    }
}

/// Filled wedge of the bounding ellipse (paint slice).
pub fn fill_arc(
    canvas: &mut Canvas,
    top: i32,
    left: i32,
    bottom: i32,
    right: i32,
    start_deg: i32,
    arc_deg: i32,
    c: Rgba,
) {
    if right <= left || bottom <= top {
        return;
    }
    let cx = left as f64 + (right - left - 1) as f64 / 2.0;
    let cy = top as f64 + (bottom - top - 1) as f64 / 2.0;
    let a = (right - left - 1) as f64 / 2.0;
    let b = (bottom - top - 1) as f64 / 2.0;
    if a < 0.0 || b < 0.0 {
        return;
    }
    let (lo, hi) = arc_range(start_deg, arc_deg);
    // Build a polygon: centre + sampled boundary points along the
    // arc, then fill that. Number of samples scales with size to
    // keep the boundary smooth.
    let n = (a.max(b) * 4.0).max(32.0) as i32;
    let mut poly = Vec::with_capacity(n as usize + 2);
    poly.push((cx.round() as i32, cy.round() as i32));
    for i in 0..=n {
        let frac = i as f64 / n as f64;
        let deg = lo + frac * (hi - lo);
        let qd_rad = deg.to_radians();
        let mx = qd_rad.sin();
        let my = -qd_rad.cos();
        let x = (cx + a * mx).round() as i32;
        let y = (cy + b * my).round() as i32;
        poly.push((x, y));
    }
    fill_polygon(canvas, &poly, c);
}

/// Normalise QuickDraw arc start + sweep to a degree range
/// `(lo, hi)` with `hi >= lo`. Negative sweep means counter-clockwise
/// — we just swap for the iteration direction.
fn arc_range(start_deg: i32, arc_deg: i32) -> (f64, f64) {
    let a = start_deg as f64;
    let b = (start_deg + arc_deg) as f64;
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
}

// ---------------------------------------------------------------------------
// Pen-size aware drawing — round 42.
// ---------------------------------------------------------------------------
//
// QuickDraw `PnSize` (Inside Macintosh: Imaging With QuickDraw §2,
// "Setting the Pen Size") attaches a per-axis thickness `(pen_h,
// pen_v)` to the pen. Subsequent `Line` / `LineFrom` / `Frame*` ops
// extrude the geometry by a `pen_h × pen_v` brush whose top-left
// corner is the geometric pixel. We implement that as a brush stamp:
// each pen plot writes a `pen_h × pen_v` rectangle. Pen sizes of
// `(1, 1)` (the default) collapse to the original 1-pixel primitive,
// so the pen-thick variants degrade gracefully.

/// Stamp a pen-sized brush (`pen_h × pen_v` rectangle, top-left at
/// `(x, y)`) onto the canvas. Used by every pen-aware primitive.
#[inline]
fn stamp_pen(canvas: &mut Canvas, x: i32, y: i32, pen_h: i32, pen_v: i32, c: Rgba) {
    if pen_h <= 1 && pen_v <= 1 {
        canvas.put(x, y, c);
        return;
    }
    let w = pen_h.max(1);
    let h = pen_v.max(1);
    for dy in 0..h {
        canvas.span(y + dy, x, x + w, c);
    }
}

/// Bresenham line with a `pen_h × pen_v` brush stamped at every
/// rasterised pixel. Falls back to [`line`] when the pen is 1×1.
pub fn line_thick(
    canvas: &mut Canvas,
    mut x0: i32,
    mut y0: i32,
    x1: i32,
    y1: i32,
    pen_h: i32,
    pen_v: i32,
    c: Rgba,
) {
    if pen_h <= 1 && pen_v <= 1 {
        line(canvas, x0, y0, x1, y1, c);
        return;
    }
    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    loop {
        stamp_pen(canvas, x0, y0, pen_h, pen_v, c);
        if x0 == x1 && y0 == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x0 += sx;
        }
        if e2 <= dx {
            err += dx;
            y0 += sy;
        }
    }
}

/// Outline rectangle drawn with a `pen_h × pen_v` brush along each
/// edge. The brush extends inward from each edge per QuickDraw's
/// `framePen` rule (Inside Macintosh §2: "the pen size is added to
/// the rectangle's right and bottom"). Falls back to [`frame_rect`]
/// for 1×1 pens.
pub fn frame_rect_thick(
    canvas: &mut Canvas,
    top: i32,
    left: i32,
    bottom: i32,
    right: i32,
    pen_h: i32,
    pen_v: i32,
    c: Rgba,
) {
    if right <= left || bottom <= top {
        return;
    }
    if pen_h <= 1 && pen_v <= 1 {
        frame_rect(canvas, top, left, bottom, right, c);
        return;
    }
    let ph = pen_h.max(1);
    let pv = pen_v.max(1);
    // Top + bottom strips (each `pv` rows thick).
    for dy in 0..pv {
        canvas.span(top + dy, left, right, c);
        canvas.span(bottom - 1 - dy, left, right, c);
    }
    // Left + right strips (each `ph` cols thick), excluding the
    // corners we already drew.
    for y in (top + pv)..(bottom - pv) {
        for dx in 0..ph {
            canvas.put(left + dx, y, c);
            canvas.put(right - 1 - dx, y, c);
        }
    }
}

/// Frame oval with a brush of `pen_h × pen_v` stamped at every
/// boundary pixel. Approximation: stamps overlap on adjacent boundary
/// pixels; QuickDraw's exact rule is "draw the boundary as if it were
/// the outline of a `(right + ph - left) × (bottom + pv - top)`
/// ellipse" but stamping is visually close and matches the
/// rasteriser's other thick-pen primitives.
pub fn frame_oval_thick(
    canvas: &mut Canvas,
    top: i32,
    left: i32,
    bottom: i32,
    right: i32,
    pen_h: i32,
    pen_v: i32,
    c: Rgba,
) {
    if pen_h <= 1 && pen_v <= 1 {
        frame_oval(canvas, top, left, bottom, right, c);
        return;
    }
    walk_ellipse(top, left, bottom, right, |x, y| {
        stamp_pen(canvas, x, y, pen_h, pen_v, c);
    });
}

// ---------------------------------------------------------------------------
// Patterned-fill primitives (round 8 / workspace round 81).
//
// QuickDraw stippling: a `1` bit in the 8×8 `Pattern` selects the
// foreground colour, a `0` bit selects the background colour (Inside
// Macintosh: Imaging With QuickDraw §A-3 — `PnPat` / `BkPat` / `FillPat`
// opcodes). The texture tiles every 8 pixels on both axes. Sampling
// uses *canvas-local* coordinates so the tile origin lines up with the
// picture-frame top-left — this matches the behaviour real Mac apps see
// when the GrafPort origin is `(0, 0)`, the universal case for PICT
// files (no `setOrigin` recorded).
//
// Each primitive degrades to its solid-colour counterpart when the
// pattern is all-ones (foreground everywhere) or all-zeros (background
// everywhere) — `Pattern::is_solid_fg` / `is_solid_bg` provide the
// cheap check.
// ---------------------------------------------------------------------------

#[inline]
fn plot_pattern_pixel(canvas: &mut Canvas, x: i32, y: i32, pat: Pattern, fg: Rgba, bg: Rgba) {
    if pat.sample(x, y) {
        canvas.put(x, y, fg);
    } else {
        canvas.put(x, y, bg);
    }
}

/// Patterned-fill rectangle. `right` / `bottom` exclusive. Same shape
/// as [`fill_rect`] but every cell is stippled via `pat` between `fg`
/// (on bits) and `bg` (off bits). Falls back to the solid-colour
/// path when the pattern collapses.
pub fn fill_rect_pattern(
    canvas: &mut Canvas,
    top: i32,
    left: i32,
    bottom: i32,
    right: i32,
    pat: Pattern,
    fg: Rgba,
    bg: Rgba,
) {
    if right <= left || bottom <= top {
        return;
    }
    if pat.is_solid_fg() {
        fill_rect(canvas, top, left, bottom, right, fg);
        return;
    }
    if pat.is_solid_bg() {
        fill_rect(canvas, top, left, bottom, right, bg);
        return;
    }
    for y in top..bottom {
        for x in left..right {
            plot_pattern_pixel(canvas, x, y, pat, fg, bg);
        }
    }
}

/// Patterned-fill ellipse. Same boundary as [`fill_oval`] but every
/// span is stippled via `pat`.
pub fn fill_oval_pattern(
    canvas: &mut Canvas,
    top: i32,
    left: i32,
    bottom: i32,
    right: i32,
    pat: Pattern,
    fg: Rgba,
    bg: Rgba,
) {
    if right <= left || bottom <= top {
        return;
    }
    if pat.is_solid_fg() {
        fill_oval(canvas, top, left, bottom, right, fg);
        return;
    }
    if pat.is_solid_bg() {
        fill_oval(canvas, top, left, bottom, right, bg);
        return;
    }
    let h = (bottom - top) as usize;
    let mut min = vec![i32::MAX; h];
    let mut max = vec![i32::MIN; h];
    walk_ellipse(top, left, bottom, right, |x, y| {
        let row = y - top;
        if row < 0 || (row as usize) >= h {
            return;
        }
        let r = row as usize;
        if x < min[r] {
            min[r] = x;
        }
        if x > max[r] {
            max[r] = x;
        }
    });
    for (i, (lo, hi)) in min.iter().zip(max.iter()).enumerate() {
        if *lo == i32::MAX {
            continue;
        }
        let y = top + i as i32;
        for x in *lo..=*hi {
            plot_pattern_pixel(canvas, x, y, pat, fg, bg);
        }
    }
}

/// Patterned-fill round rectangle. Reuses the inner-rect + four-corner
/// quarter-oval shape from [`fill_round_rect`] but every span is
/// stippled.
pub fn fill_round_rect_pattern(
    canvas: &mut Canvas,
    top: i32,
    left: i32,
    bottom: i32,
    right: i32,
    oval_w: i32,
    oval_h: i32,
    pat: Pattern,
    fg: Rgba,
    bg: Rgba,
) {
    if right <= left || bottom <= top {
        return;
    }
    if pat.is_solid_fg() {
        fill_round_rect(canvas, top, left, bottom, right, oval_w, oval_h, fg);
        return;
    }
    if pat.is_solid_bg() {
        fill_round_rect(canvas, top, left, bottom, right, oval_w, oval_h, bg);
        return;
    }
    // Slow path: materialise the rounded mask via the solid-colour
    // primitive into a scratch single-colour canvas, then re-plot per-
    // pixel using the pattern. Two passes, no duplicated geometry
    // logic.
    let w = (right - left) as u32;
    let h = (bottom - top) as u32;
    if w == 0 || h == 0 {
        return;
    }
    let marker = Rgba {
        r: 1,
        g: 2,
        b: 3,
        a: 4,
    };
    let mut scratch = Canvas::new(w, h, Rgba::new(0, 0, 0, 0));
    fill_round_rect(
        &mut scratch,
        0,
        0,
        h as i32,
        w as i32,
        oval_w,
        oval_h,
        marker,
    );
    for sy in 0..h {
        for sx in 0..w {
            let off = ((sy * w + sx) * 4) as usize;
            if scratch.data[off] != marker.r || scratch.data[off + 1] != marker.g {
                continue;
            }
            plot_pattern_pixel(canvas, left + sx as i32, top + sy as i32, pat, fg, bg);
        }
    }
}

/// Patterned-fill polygon. Reuses the active-edge-list scan converter
/// from [`fill_polygon`] but every span is stippled.
pub fn fill_polygon_pattern(
    canvas: &mut Canvas,
    vertices: &[(i32, i32)],
    pat: Pattern,
    fg: Rgba,
    bg: Rgba,
) {
    if pat.is_solid_fg() {
        fill_polygon(canvas, vertices, fg);
        return;
    }
    if pat.is_solid_bg() {
        fill_polygon(canvas, vertices, bg);
        return;
    }
    // Same scratch-canvas trick: materialise then re-plot. Cheaper
    // than duplicating the active-edge-list machinery and still O(n)
    // in the polygon bounding box.
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
    for &(x, y) in vertices {
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x);
        max_y = max_y.max(y);
    }
    if min_x > max_x || min_y > max_y {
        return;
    }
    let w = (max_x - min_x + 2).max(1) as u32;
    let h = (max_y - min_y + 2).max(1) as u32;
    let marker = Rgba {
        r: 1,
        g: 2,
        b: 3,
        a: 4,
    };
    let mut scratch = Canvas::new(w, h, Rgba::new(0, 0, 0, 0));
    let shifted: Vec<(i32, i32)> = vertices
        .iter()
        .map(|&(x, y)| (x - min_x, y - min_y))
        .collect();
    fill_polygon(&mut scratch, &shifted, marker);
    for sy in 0..h {
        for sx in 0..w {
            let off = ((sy * w + sx) * 4) as usize;
            if scratch.data[off] != marker.r || scratch.data[off + 1] != marker.g {
                continue;
            }
            plot_pattern_pixel(canvas, min_x + sx as i32, min_y + sy as i32, pat, fg, bg);
        }
    }
}

/// Patterned-frame rectangle with a `pen_h × pen_v` brush. Each stamped
/// pen pixel respects the pattern's stipple at that coordinate.
pub fn frame_rect_pattern_thick(
    canvas: &mut Canvas,
    top: i32,
    left: i32,
    bottom: i32,
    right: i32,
    pen_h: i32,
    pen_v: i32,
    pat: Pattern,
    fg: Rgba,
    bg: Rgba,
) {
    if right <= left || bottom <= top {
        return;
    }
    if pat.is_solid_fg() {
        frame_rect_thick(canvas, top, left, bottom, right, pen_h, pen_v, fg);
        return;
    }
    if pat.is_solid_bg() {
        frame_rect_thick(canvas, top, left, bottom, right, pen_h, pen_v, bg);
        return;
    }
    let ph = pen_h.max(1);
    let pv = pen_v.max(1);
    // Top + bottom strips.
    for dy in 0..pv {
        for x in left..right {
            plot_pattern_pixel(canvas, x, top + dy, pat, fg, bg);
            plot_pattern_pixel(canvas, x, bottom - 1 - dy, pat, fg, bg);
        }
    }
    // Left + right strips, excluding the corners we already drew.
    for y in (top + pv)..(bottom - pv) {
        for dx in 0..ph {
            plot_pattern_pixel(canvas, left + dx, y, pat, fg, bg);
            plot_pattern_pixel(canvas, right - 1 - dx, y, pat, fg, bg);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn white() -> Rgba {
        Rgba {
            r: 255,
            g: 255,
            b: 255,
            a: 255,
        }
    }
    fn black() -> Rgba {
        Rgba {
            r: 0,
            g: 0,
            b: 0,
            a: 255,
        }
    }
    fn red() -> Rgba {
        Rgba {
            r: 255,
            g: 0,
            b: 0,
            a: 255,
        }
    }

    fn at(c: &Canvas, x: u32, y: u32) -> [u8; 4] {
        let off = ((y * c.width + x) * 4) as usize;
        [
            c.data[off],
            c.data[off + 1],
            c.data[off + 2],
            c.data[off + 3],
        ]
    }

    #[test]
    fn canvas_init_paper() {
        let c = Canvas::new(2, 2, white());
        assert_eq!(at(&c, 0, 0), [255, 255, 255, 255]);
        assert!(!c.dirty);
    }

    #[test]
    fn fill_rect_dirties() {
        let mut c = Canvas::new(4, 4, white());
        fill_rect(&mut c, 1, 1, 3, 3, black());
        assert!(c.dirty);
        assert_eq!(at(&c, 0, 0), [255, 255, 255, 255]);
        assert_eq!(at(&c, 1, 1), [0, 0, 0, 255]);
        assert_eq!(at(&c, 2, 2), [0, 0, 0, 255]);
        assert_eq!(at(&c, 3, 3), [255, 255, 255, 255]);
    }

    #[test]
    fn frame_rect_outlines_only() {
        let mut c = Canvas::new(5, 5, white());
        frame_rect(&mut c, 1, 1, 4, 4, black());
        // Corners drawn.
        assert_eq!(at(&c, 1, 1), [0, 0, 0, 255]);
        assert_eq!(at(&c, 3, 3), [0, 0, 0, 255]);
        // Interior NOT drawn.
        assert_eq!(at(&c, 2, 2), [255, 255, 255, 255]);
    }

    #[test]
    fn line_horizontal() {
        let mut c = Canvas::new(10, 3, white());
        line(&mut c, 1, 1, 8, 1, red());
        for x in 1..=8 {
            assert_eq!(at(&c, x as u32, 1), [255, 0, 0, 255]);
        }
        assert_eq!(at(&c, 0, 1), [255, 255, 255, 255]);
    }

    #[test]
    fn line_diagonal() {
        let mut c = Canvas::new(5, 5, white());
        line(&mut c, 0, 0, 4, 4, black());
        for i in 0..5 {
            assert_eq!(at(&c, i, i), [0, 0, 0, 255]);
        }
    }

    #[test]
    fn fill_polygon_triangle() {
        let mut c = Canvas::new(10, 10, white());
        fill_polygon(&mut c, &[(2, 2), (8, 2), (5, 8)], red());
        assert_eq!(at(&c, 5, 3), [255, 0, 0, 255]);
        assert_eq!(at(&c, 5, 7), [255, 0, 0, 255]);
        // Outside the triangle.
        assert_eq!(at(&c, 0, 0), [255, 255, 255, 255]);
        assert_eq!(at(&c, 9, 9), [255, 255, 255, 255]);
    }

    #[test]
    fn fill_oval_fills_centre() {
        let mut c = Canvas::new(20, 20, white());
        fill_oval(&mut c, 2, 2, 18, 18, black());
        assert_eq!(at(&c, 10, 10), [0, 0, 0, 255]);
        // Corner of bounding box should still be paper.
        assert_eq!(at(&c, 2, 2), [255, 255, 255, 255]);
    }

    #[test]
    fn frame_oval_outlines_only() {
        let mut c = Canvas::new(20, 20, white());
        frame_oval(&mut c, 2, 2, 18, 18, black());
        // Centre should still be paper.
        assert_eq!(at(&c, 10, 10), [255, 255, 255, 255]);
        // Top of ellipse boundary: row ~2, x ~10.
        assert_eq!(at(&c, 10, 2), [0, 0, 0, 255]);
    }

    #[test]
    fn blit_full_size() {
        let mut c = Canvas::new(4, 2, white());
        let src = vec![
            10, 20, 30, 255, 40, 50, 60, 255, 70, 80, 90, 255, 100, 110, 120, 255, // row 0
            5, 6, 7, 255, 8, 9, 10, 255, 11, 12, 13, 255, 14, 15, 16, 255, // row 1
        ];
        c.blit(&src, 4, 2, 0, 0, 2, 4);
        assert_eq!(at(&c, 0, 0), [10, 20, 30, 255]);
        assert_eq!(at(&c, 3, 1), [14, 15, 16, 255]);
    }
}
