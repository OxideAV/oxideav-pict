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

use crate::state::{Pattern, PixPattern, Rgba};

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

    /// Read the RGBA pixel at `(x, y)` (canvas-local). Returns `None`
    /// when the coordinates fall outside the canvas — clip masks do
    /// not filter reads, since callers (the pattern transfer-mode
    /// primitives) need the unfiltered destination pixel to compute the
    /// new value before re-writing through [`Canvas::put`] which itself
    /// honours the clip mask.
    #[inline]
    pub fn pixel_at(&self, x: i32, y: i32) -> Option<Rgba> {
        if x < 0 || y < 0 || (x as u32) >= self.width || (y as u32) >= self.height {
            return None;
        }
        let off = ((y as u32 * self.width + x as u32) * 4) as usize;
        Some(Rgba {
            r: self.data[off],
            g: self.data[off + 1],
            b: self.data[off + 2],
            a: self.data[off + 3],
        })
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

    /// Invert a horizontal span `[x0, x1)` at row `y` per the §3-44
    /// QuickDraw invert-verb contract — every covered pixel has its RGB
    /// channels bitwise-NOTed (alpha preserved). Coords are clipped to
    /// the canvas; out-of-range spans are no-ops. Pixels outside the
    /// active clip mask (if any) are skipped.
    pub fn invert_span(&mut self, y: i32, x0: i32, x1: i32) {
        if y < 0 || (y as u32) >= self.height {
            return;
        }
        let lo = x0.max(0).min(self.width as i32);
        let hi = x1.max(0).min(self.width as i32);
        if lo >= hi {
            return;
        }
        let row = y as u32 * self.width;
        let mut any = false;
        for x in lo..hi {
            let idx = (row + x as u32) as usize;
            if let Some(mask) = &self.clip {
                if !mask[idx] {
                    continue;
                }
            }
            let off = idx * 4;
            self.data[off] = !self.data[off];
            self.data[off + 1] = !self.data[off + 1];
            self.data[off + 2] = !self.data[off + 2];
            any = true;
        }
        if any {
            self.dirty = true;
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

    /// [`Canvas::blit`] obeying a `CopyBits` source transfer mode
    /// (Inside Macintosh: Imaging With QuickDraw §3 Table 3-1 /
    /// §4 Table 4-1 — book pages 3-9 and 4-33). Every destination
    /// pixel is combined with its source pixel through
    /// [`blend_source`] under the active foreground / background
    /// colours.
    ///
    /// `SrcCopy` with a black foreground and a white background is
    /// the §4-34 identity case (*"Drawing into a white background
    /// with a black foreground always reproduces the source image,
    /// regardless of the pixel depth"*) and short-circuits to the
    /// plain [`Canvas::blit`] fast path, bit-for-bit.
    pub fn blit_mode(
        &mut self,
        src_rgba: &[u8],
        src_w: u32,
        src_h: u32,
        dst_top: i32,
        dst_left: i32,
        dst_bot: i32,
        dst_right: i32,
        mode: SourceMode,
        fg: Rgba,
        bg: Rgba,
    ) {
        if mode.is_identity_copy(fg, bg) {
            self.blit(
                src_rgba, src_w, src_h, dst_top, dst_left, dst_bot, dst_right,
            );
            return;
        }
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
                let src = Rgba::new(
                    src_rgba[s_off],
                    src_rgba[s_off + 1],
                    src_rgba[s_off + 2],
                    src_rgba[s_off + 3],
                );
                let d_off = ((cy as u32 * self.width + cx as u32) * 4) as usize;
                let dst = Rgba::new(
                    self.data[d_off],
                    self.data[d_off + 1],
                    self.data[d_off + 2],
                    self.data[d_off + 3],
                );
                let out = blend_source(mode, src, dst, fg, bg);
                self.data[d_off] = out.r;
                self.data[d_off + 1] = out.g;
                self.data[d_off + 2] = out.b;
                self.data[d_off + 3] = out.a;
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

/// QuickDraw Boolean pattern-transfer modes (`PnMode` opcode payload).
///
/// Inside Macintosh: Imaging With QuickDraw §3 "QuickDraw Drawing
/// Reference" (`PenMode` procedure, book page 3-44) defines eight
/// pattern modes (`patCopy = 8` … `notPatBic = 15`) plus the parallel
/// eight source modes (`srcCopy = 0` … `notSrcBic = 7`). Pattern-fill
/// verbs (frame / paint / erase / fill of rect / round-rect / oval /
/// arc / poly / region) consume the pattern modes; the source modes
/// apply to `CopyBits` rasters (which always render `srcCopy` in this
/// crate — handled by [`Canvas::put`]).
///
/// Per §3-44, each pattern mode performs a per-pixel Boolean operation
/// where the "source" is the pattern bit (1 = foreground / on, 0 =
/// background / off) and the destination is the existing canvas pixel:
///
/// | Mode             | Code | Pattern-bit-1 cell        | Pattern-bit-0 cell        |
/// | ---------------- | ---- | ------------------------- | ------------------------- |
/// | `patCopy`        | 8    | write `fg`                | write `bg`                |
/// | `patOr`          | 9    | write `fg`                | leave unchanged           |
/// | `patXor`         | 10   | invert destination        | leave unchanged           |
/// | `patBic`         | 11   | write `bg`                | leave unchanged           |
/// | `notPatCopy`     | 12   | write `bg`                | write `fg`                |
/// | `notPatOr`       | 13   | leave unchanged           | write `fg`                |
/// | `notPatXor`      | 14   | leave unchanged           | invert destination        |
/// | `notPatBic`      | 15   | leave unchanged           | write `bg`                |
///
/// (§3-44: `patOr`: *"where pattern pixel is black, invert destination
/// pixel"*. The §3-44 wording is "invert" but the Pascal `BitOR` and
/// `BitXOR` semantics — see §3 Figure 3-4 — coincide on a 1-bit display:
/// a destination bit `OR` foreground-black is forced to black. In our
/// true-colour pipeline we honour `patOr` as "write `fg`" rather than
/// invert — matching the §3 description: *"to OR is to apply the
/// foreground"*.)
///
/// Modes outside `8..=15` fall back to `patCopy` for the pattern-fill
/// path; numeric source-mode codes (`0..=7`) hit when a producer set
/// `PnMode srcCopy` deliberately (Inside Macintosh §A-3 Listing A-5 /
/// A-6 are silent on whether this is legal but several real-world PICTs
/// do it — we route to `patCopy` rather than refuse the picture).
///
/// The arithmetic transfer modes (32..39 — `blend`, `addPin`, …) are
/// carried by the [`PatternMode::Arith`] variant, decoded from the
/// `PnMode` opcode plus the active `OpColor` / background colour via
/// [`PatternMode::from_pn_mode_with`] (round 273). The bare
/// [`PatternMode::from_pn_mode`] constructor still folds them to
/// `patCopy` for callers that have no colour context to supply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PatternMode {
    /// `patCopy = 8` — fg where pattern bit is 1, bg where 0.
    /// Default per Inside Macintosh §3-44: *"the initial pattern mode
    /// value is patCopy."*
    #[default]
    PatCopy,
    /// `patOr = 9` — fg where pattern bit is 1, destination unchanged
    /// where pattern bit is 0.
    PatOr,
    /// `patXor = 10` — invert destination where pattern bit is 1,
    /// unchanged where pattern bit is 0.
    PatXor,
    /// `patBic = 11` — bg where pattern bit is 1, destination unchanged
    /// where pattern bit is 0. *"Bit clear"* — the foreground role is
    /// silenced.
    PatBic,
    /// `notPatCopy = 12` — bg where pattern bit is 1, fg where 0
    /// (inverted-pattern copy).
    NotPatCopy,
    /// `notPatOr = 13` — unchanged where pattern bit is 1, fg where 0.
    NotPatOr,
    /// `notPatXor = 14` — unchanged where pattern bit is 1, invert
    /// destination where 0.
    NotPatXor,
    /// `notPatBic = 15` — unchanged where pattern bit is 1, bg where 0.
    NotPatBic,
    /// One of the Color QuickDraw arithmetic transfer modes
    /// (`blend = 32` … `adMin = 39`, Inside Macintosh §4 pages
    /// 4-38..4-40). Every cell combines its *source* colour (the
    /// pattern's on-bit fg / off-bit bg) with the existing destination
    /// pixel per [`ArithMode`], parameterised by the active `OpColor`
    /// (`op_color`) and — for `transparent` — the background colour
    /// (`bg_key`). Built by [`PatternMode::from_pn_mode_with`].
    Arith {
        /// The specific §4 arithmetic operation.
        mode: ArithMode,
        /// `OpColor` — the max-pin (`addPin`), min-pin (`subPin`) or
        /// per-channel blend weight (`blend`). Ignored by the other
        /// arithmetic modes.
        op_color: Rgba,
        /// Background colour used as the transparent-mode key (a source
        /// cell equal to this colour leaves the destination unchanged).
        bg_key: Rgba,
    },
}

/// The eight Color QuickDraw arithmetic transfer modes from Inside
/// Macintosh: Imaging With QuickDraw §4 ("Color QuickDraw"), pages
/// 4-38..4-40. Each combines a *source* RGB colour with the existing
/// *destination* pixel on a per-channel basis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArithMode {
    /// `blend = 32` — weighted average per §4-39:
    /// `dest = src·w/MAX + dst·(1 − w/MAX)`, where `w` is the per-channel
    /// `OpColor` weight. (On this crate's 8-bit canvas `MAX = 255`.)
    Blend,
    /// `addPin = 33` — `dst = min(src + dst, opColor)` per channel; the
    /// `OpColor` supplies the per-channel maximum (white in a basic
    /// port).
    AddPin,
    /// `addOver = 34` — `dst = (src + dst) mod (MAX + 1)` per channel
    /// (wrapping add).
    AddOver,
    /// `subPin = 35` — `dst = max(dst − src, opColor)` per channel; the
    /// `OpColor` supplies the per-channel minimum (black in a basic
    /// port).
    SubPin,
    /// `transparent = 36` — `dst = src` unless the source colour equals
    /// the background colour, in which case the destination is left
    /// unchanged.
    Transparent,
    /// `addMax = 37` — `dst = max(src, dst)` per channel (greater
    /// saturation of each component wins).
    AddMax,
    /// `subOver = 38` — `dst = (dst − src) mod (MAX + 1)` per channel
    /// (wrapping subtract — negative results wrap up).
    SubOver,
    /// `adMin = 39` — `dst = min(src, dst)` per channel (lesser
    /// saturation of each component wins).
    AdMin,
}

impl ArithMode {
    /// Map a `PnMode` integer in `32..=39` to its arithmetic mode.
    /// Codes outside that band return `None`.
    pub const fn from_code(code: i16) -> Option<Self> {
        Some(match code {
            32 => Self::Blend,
            33 => Self::AddPin,
            34 => Self::AddOver,
            35 => Self::SubPin,
            36 => Self::Transparent,
            37 => Self::AddMax,
            38 => Self::SubOver,
            39 => Self::AdMin,
            _ => return None,
        })
    }
}

/// Combine a source colour with the destination pixel per the §4
/// arithmetic transfer-mode formulas (worked at 8-bit channel
/// precision — the §4 "truncated RGB" direct-pixel path). `op_color`
/// supplies the per-channel pin / weight; `bg_key` is the
/// transparent-mode background key. Alpha is taken from the source for
/// modes that write a fresh colour and preserved for pin / wrap modes
/// (the canvas is alpha-opaque throughout, so the choice is cosmetic).
#[inline]
pub fn blend_arith(mode: ArithMode, src: Rgba, dst: Rgba, op_color: Rgba, bg_key: Rgba) -> Rgba {
    // Per-channel arithmetic at u16 working width to avoid overflow.
    let ch = |s: u8, d: u8, o: u8| -> u8 {
        let (s, d, o) = (s as i32, d as i32, o as i32);
        match mode {
            // dest = src·w/255 + dst·(255 − w)/255, rounded to nearest.
            ArithMode::Blend => {
                let v = (s * o + d * (255 - o) + 127) / 255;
                v.clamp(0, 255) as u8
            }
            // sum pinned to the OpColor per-channel maximum.
            ArithMode::AddPin => (s + d).min(o) as u8,
            // wrapping add (mod 256).
            ArithMode::AddOver => ((s + d) & 0xFF) as u8,
            // difference pinned to the OpColor per-channel minimum.
            ArithMode::SubPin => (d - s).max(o) as u8,
            // greater saturation wins.
            ArithMode::AddMax => s.max(d) as u8,
            // wrapping subtract (mod 256).
            ArithMode::SubOver => ((d - s) & 0xFF) as u8,
            // lesser saturation wins.
            ArithMode::AdMin => s.min(d) as u8,
            // handled below — never reached per channel.
            ArithMode::Transparent => d as u8,
        }
    };
    if let ArithMode::Transparent = mode {
        // Whole-pixel decision: source pixels equal to the background
        // colour are holes (destination unchanged); others copy through.
        if src.r == bg_key.r && src.g == bg_key.g && src.b == bg_key.b {
            return dst;
        }
        return Rgba {
            r: src.r,
            g: src.g,
            b: src.b,
            a: dst.a,
        };
    }
    Rgba {
        r: ch(src.r, dst.r, op_color.r),
        g: ch(src.g, dst.g, op_color.g),
        b: ch(src.b, dst.b, op_color.b),
        a: dst.a,
    }
}

impl PatternMode {
    /// Decode a `PnMode` integer from the §A-3 `PnMode` opcode payload.
    ///
    /// `8..=15` map to the eight Boolean pattern modes; any other value
    /// (including the source modes `0..=7` and the arithmetic transfer
    /// modes `32..=39`) falls back to [`PatternMode::PatCopy`]. Use
    /// [`PatternMode::from_pn_mode_with`] when the active `OpColor` /
    /// background colour are available so the arithmetic modes can be
    /// honoured instead of folded to `patCopy`.
    pub const fn from_pn_mode(code: i16) -> Self {
        match code {
            8 => Self::PatCopy,
            9 => Self::PatOr,
            10 => Self::PatXor,
            11 => Self::PatBic,
            12 => Self::NotPatCopy,
            13 => Self::NotPatOr,
            14 => Self::NotPatXor,
            15 => Self::NotPatBic,
            _ => Self::PatCopy,
        }
    }

    /// Decode a `PnMode` integer with the colour context the arithmetic
    /// transfer modes (`32..=39`) need. `8..=15` resolve to the Boolean
    /// pattern modes exactly as [`PatternMode::from_pn_mode`]; `32..=39`
    /// resolve to [`PatternMode::Arith`] carrying `op_color` (the
    /// `OpColor` pin / blend weight) and `bg_key` (the transparent-mode
    /// background key); every other code falls back to `patCopy`.
    ///
    /// Per §4-39/4-40 a missing `OpColor` defaults to the basic-port
    /// pins: white for the max-pin / blend modes (`addPin`, `blend`),
    /// black for the min-pin mode (`subPin`). When `op_color` is `None`
    /// the per-mode default is substituted so an absent `OpColor` opcode
    /// produces the no-clamp behaviour the §4 basic-port text describes
    /// (max pin = white ⇒ never clamps a sum down; min pin = black ⇒
    /// never clamps a difference up).
    pub fn from_pn_mode_with(code: i16, op_color: Option<Rgba>, bg_key: Rgba) -> Self {
        if let Some(mode) = ArithMode::from_code(code) {
            let op_color = op_color.unwrap_or(match mode {
                // min-pin: basic-port minimum is black (§4-39).
                ArithMode::SubPin => Rgba::BLACK,
                // blend: basic-port weight is "50 percent gray" (§4-39),
                // i.e. equal weights of source and destination.
                ArithMode::Blend => Rgba::new(128, 128, 128, 255),
                // max-pin + everything else: basic-port maximum is white.
                // (Modes that ignore OpColor are unaffected.)
                _ => Rgba::WHITE,
            });
            Self::Arith {
                mode,
                op_color,
                bg_key,
            }
        } else {
            Self::from_pn_mode(code)
        }
    }

    /// Returns `true` when this mode is `patCopy` — the all-cells-write
    /// shape that lets the rasteriser take its existing fast paths
    /// (solid-fg / solid-bg pattern collapses straight to `fill_rect`).
    #[inline]
    pub const fn is_pat_copy(self) -> bool {
        matches!(self, Self::PatCopy)
    }
}

/// A `CopyBits` source transfer mode, resolved with the colour context
/// the §4 semantics need.
///
/// Inside Macintosh: Imaging With QuickDraw §3 ("QuickDraw Drawing")
/// pages 3-113..3-114 define the eight Boolean source modes
/// (`srcCopy = 0`, `srcOr = 1`, `srcXor = 2`, `srcBic = 3`,
/// `notSrcCopy = 4`, `notSrcOr = 5`, `notSrcXor = 6`, `notSrcBic = 7`)
/// consumed by the `CopyBits` family — in PICT terms, the `mode` word
/// every `BitsRect` / `BitsRgn` / `PackBitsRect` / `PackBitsRgn` /
/// `DirectBitsRect` / `DirectBitsRgn` record carries between `dstRect`
/// and the pixel data (§A-3 Listings A-2 / A-3 — *"mode: Mode;
/// {transfer mode}"*).
///
/// On destinations deeper than 1 bit the Boolean ops take the §4
/// Table 4-1 (book page 4-33) colour shape: the source pixel's
/// per-channel closeness to black applies that portion of the
/// *foreground* colour (or *background* for the BIC ops), and "any
/// other color" applies weighted portions per §4-33's worked
/// `CopyBits` description. The §4 arithmetic transfer modes
/// (`blend = 32` … `adMin = 39`) are legal in the same mode word
/// (§4-40 Note — *"your application can pass them in parameters to
/// the PenMode, CopyBits, CopyDeepMask, and TextMode routines"*) and
/// are carried by [`SourceMode::Arith`], reusing the round-273
/// [`blend_arith`] combiner with the decoded raster pixel as the
/// source colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SourceMode {
    /// `srcCopy = 0` — §4 Table 4-1: black source applies the
    /// foreground colour, white applies the background colour, any
    /// other colour applies weighted portions of both. With the
    /// fresh-GrafPort black-fg / white-bg this reproduces the source
    /// image exactly (§4-34).
    #[default]
    SrcCopy,
    /// `notSrcCopy = 4` — `srcCopy` with the foreground and background
    /// roles reversed (§4-34: *"the notSrcCopy mode reverses the
    /// foreground and background colors"*).
    NotSrcCopy,
    /// `srcOr = 1` — black source applies the foreground colour, white
    /// leaves the destination alone, other colours apply weighted
    /// portions of the foreground.
    SrcOr,
    /// `notSrcOr = 5` — white source applies the foreground colour,
    /// black leaves the destination alone.
    NotSrcOr,
    /// `srcXor = 2` — a black source pixel inverts the destination
    /// pixel; white and coloured source pixels leave it alone
    /// (§4 Table 4-1 marks the invert *"undefined for colored
    /// destination pixel"* — we honour it as the channel-wise NOT the
    /// rest of this crate's invert paths use).
    SrcXor,
    /// `notSrcXor = 6` — a white source pixel inverts the destination
    /// pixel; black and coloured source pixels leave it alone.
    NotSrcXor,
    /// `srcBic = 3` — black source applies the *background* colour
    /// ("bit clear"), white leaves the destination alone, other
    /// colours apply weighted portions of the background.
    SrcBic,
    /// `notSrcBic = 7` — white source applies the background colour,
    /// black leaves the destination alone.
    NotSrcBic,
    /// One of the §4 arithmetic transfer modes (`blend = 32` …
    /// `adMin = 39`) applied to the blit: every destination pixel is
    /// `blend_arith(mode, src, dst, op_color, bg_key)` with the decoded
    /// raster pixel as `src`.
    Arith {
        /// The specific §4 arithmetic operation.
        mode: ArithMode,
        /// `OpColor` — max-pin (`addPin`), min-pin (`subPin`) or
        /// per-channel blend weight (`blend`); ignored by the rest.
        op_color: Rgba,
        /// Background colour used as the transparent-mode key.
        bg_key: Rgba,
    },
}

impl SourceMode {
    /// `ditherCopy = 64` — §3-114 / §4-37: additive on any source
    /// mode to request dithering. Dithering approximates colours on
    /// *indexed* destinations; this crate's canvas is always
    /// true-colour RGBA, so the bit is recognised and stripped (the
    /// §4-37 contract — mix existing colours to approximate one the
    /// destination can't represent — is satisfied exactly by writing
    /// the requested colour itself).
    pub const DITHER_COPY: i16 = 64;

    /// Resolve a raster opcode's on-disk `mode` word.
    ///
    /// `0..=7` map to the eight §3-113 Boolean source modes;
    /// `32..=39` resolve to [`SourceMode::Arith`] carrying `op_color`
    /// (the declared `OpColor`, defaulting per §4-39/4-40 when absent:
    /// max-pin → white, min-pin → black, blend → 50 % gray) and
    /// `bg_key` (the transparent-mode background key). The additive
    /// `ditherCopy = 64` bit is stripped first. Any other code falls
    /// back to `srcCopy` — the total-function posture the round-247
    /// pattern path established.
    pub fn from_mode_word(code: i16, op_color: Option<Rgba>, bg_key: Rgba) -> Self {
        let base = code & !Self::DITHER_COPY;
        if let Some(mode) = ArithMode::from_code(base) {
            let op_color = op_color.unwrap_or(match mode {
                // min-pin: basic-port minimum is black (§4-40).
                ArithMode::SubPin => Rgba::BLACK,
                // blend: basic-port weight is "50 percent gray" (§4-40).
                ArithMode::Blend => Rgba::new(128, 128, 128, 255),
                // max-pin + everything else: basic-port maximum is white.
                _ => Rgba::WHITE,
            });
            return Self::Arith {
                mode,
                op_color,
                bg_key,
            };
        }
        match base {
            0 => Self::SrcCopy,
            1 => Self::SrcOr,
            2 => Self::SrcXor,
            3 => Self::SrcBic,
            4 => Self::NotSrcCopy,
            5 => Self::NotSrcOr,
            6 => Self::NotSrcXor,
            7 => Self::NotSrcBic,
            _ => Self::SrcCopy,
        }
    }

    /// Returns `true` when this mode is the §4-34 identity shape —
    /// `srcCopy` with a black foreground and a white background —
    /// which *"always reproduces the source image, regardless of the
    /// pixel depth"* and therefore short-circuits to the raw
    /// [`Canvas::blit`] fast path.
    #[inline]
    pub fn is_identity_copy(self, fg: Rgba, bg: Rgba) -> bool {
        matches!(self, Self::SrcCopy) && fg == Rgba::BLACK && bg == Rgba::WHITE
    }
}

/// Combine one source pixel with the destination pixel per the
/// `CopyBits` source transfer-mode semantics of §4 Table 4-1 (worked
/// at 8-bit channel precision).
///
/// The weighted-portion shape follows §4-33's `CopyBits` description:
/// per channel, the source's closeness to black (`255 − src`) selects
/// that relative amount of the mode's "apply" colour (foreground for
/// COPY / OR, background for BIC), and the remainder keeps the mode's
/// "leave" colour (the background for the COPY ops, the existing
/// destination for OR / BIC). The `not*` variants swap the black /
/// white roles. XOR is a whole-pixel decision per Table 4-1 — only an
/// exactly-black (`srcXor`) or exactly-white (`notSrcXor`) source
/// pixel inverts the destination; *"any other color"* leaves it alone.
///
/// Alpha: the COPY modes take the source's alpha (matching the raw
/// blit they generalise); every other mode preserves the
/// destination's.
#[inline]
pub fn blend_source(mode: SourceMode, src: Rgba, dst: Rgba, fg: Rgba, bg: Rgba) -> Rgba {
    // `w/255` of `a` + `(255 − w)/255` of `b`, rounded to nearest.
    #[inline]
    fn mix(w: u8, a: u8, b: u8) -> u8 {
        ((w as u32 * a as u32 + (255 - w as u32) * b as u32 + 127) / 255) as u8
    }
    // Apply `mix` channel-wise: weight = closeness of the source
    // channel to black (`to_black = true`) or to white, applying that
    // portion of `apply` and the remainder of the per-channel `keep`.
    #[inline]
    fn mix_rgb(src: Rgba, apply: Rgba, keep: Rgba, to_black: bool, alpha: u8) -> Rgba {
        let w = |s: u8| if to_black { 255 - s } else { s };
        Rgba {
            r: mix(w(src.r), apply.r, keep.r),
            g: mix(w(src.g), apply.g, keep.g),
            b: mix(w(src.b), apply.b, keep.b),
            a: alpha,
        }
    }
    let is_black = src.r == 0 && src.g == 0 && src.b == 0;
    let is_white = src.r == 0xFF && src.g == 0xFF && src.b == 0xFF;
    match mode {
        SourceMode::SrcCopy => mix_rgb(src, fg, bg, true, src.a),
        SourceMode::NotSrcCopy => mix_rgb(src, bg, fg, true, src.a),
        SourceMode::SrcOr => mix_rgb(src, fg, dst, true, dst.a),
        SourceMode::NotSrcOr => mix_rgb(src, fg, dst, false, dst.a),
        SourceMode::SrcBic => mix_rgb(src, bg, dst, true, dst.a),
        SourceMode::NotSrcBic => mix_rgb(src, bg, dst, false, dst.a),
        SourceMode::SrcXor => {
            if is_black {
                invert_rgba(dst)
            } else {
                dst
            }
        }
        SourceMode::NotSrcXor => {
            if is_white {
                invert_rgba(dst)
            } else {
                dst
            }
        }
        SourceMode::Arith {
            mode,
            op_color,
            bg_key,
        } => blend_arith(mode, src, dst, op_color, bg_key),
    }
}

/// Invert the destination pixel — per Inside Macintosh §3-44
/// *"invert destination pixel."*
///
/// On a 1-bit display the inversion is the literal Boolean NOT; on our
/// true-colour pipeline we honour the §3-44 wording by complementing
/// every colour channel (and preserving alpha so the canvas stays
/// alpha-opaque).
#[inline]
fn invert_rgba(c: Rgba) -> Rgba {
    Rgba {
        r: !c.r,
        g: !c.g,
        b: !c.b,
        a: c.a,
    }
}

#[inline]
fn plot_pattern_pixel(canvas: &mut Canvas, x: i32, y: i32, pat: Pattern, fg: Rgba, bg: Rgba) {
    plot_pattern_pixel_mode(canvas, x, y, pat, fg, bg, PatternMode::PatCopy);
}

/// Public single-cell §3-44 patterned plot used by the region fill
/// path. Same semantics as the in-module `plot_pattern_pixel_mode` but
/// callable from `decoder::paint_region_pattern` where the
/// per-region-cell `contains()` walk does its own iteration and only
/// needs the per-pixel op.
#[inline]
pub fn plot_region_cell_mode(
    canvas: &mut Canvas,
    x: i32,
    y: i32,
    pat: Pattern,
    fg: Rgba,
    bg: Rgba,
    mode: PatternMode,
) {
    plot_pattern_pixel_mode(canvas, x, y, pat, fg, bg, mode);
}

/// Same as [`plot_pattern_pixel`] but obeys a [`PatternMode`] —
/// each cell may write `fg`, `bg`, the inverted destination, or
/// leave the destination unchanged per Inside Macintosh §3-44.
#[inline]
fn plot_pattern_pixel_mode(
    canvas: &mut Canvas,
    x: i32,
    y: i32,
    pat: Pattern,
    fg: Rgba,
    bg: Rgba,
    mode: PatternMode,
) {
    let on = pat.sample(x, y);
    match mode {
        PatternMode::PatCopy => {
            canvas.put(x, y, if on { fg } else { bg });
        }
        PatternMode::PatOr => {
            if on {
                canvas.put(x, y, fg);
            }
        }
        PatternMode::PatXor => {
            if on {
                if let Some(d) = canvas.pixel_at(x, y) {
                    canvas.put(x, y, invert_rgba(d));
                }
            }
        }
        PatternMode::PatBic => {
            if on {
                canvas.put(x, y, bg);
            }
        }
        PatternMode::NotPatCopy => {
            canvas.put(x, y, if on { bg } else { fg });
        }
        PatternMode::NotPatOr => {
            if !on {
                canvas.put(x, y, fg);
            }
        }
        PatternMode::NotPatXor => {
            if !on {
                if let Some(d) = canvas.pixel_at(x, y) {
                    canvas.put(x, y, invert_rgba(d));
                }
            }
        }
        PatternMode::NotPatBic => {
            if !on {
                canvas.put(x, y, bg);
            }
        }
        PatternMode::Arith {
            mode,
            op_color,
            bg_key,
        } => {
            // The pattern still selects the *source* colour per cell
            // (on-bit ⇒ fg, off-bit ⇒ bg); the §4 arithmetic mode then
            // combines that source with the existing destination pixel.
            let src = if on { fg } else { bg };
            if let Some(d) = canvas.pixel_at(x, y) {
                canvas.put(x, y, blend_arith(mode, src, d, op_color, bg_key));
            }
        }
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

// ---------------------------------------------------------------------------
// Pattern + transfer-mode variants (round 247 — Boolean pattern modes).
//
// Inside Macintosh: Imaging With QuickDraw §3-44 (`PenMode` procedure)
// + §A-3 Table A-2 (`PnMode $0008`): pattern-fill verbs honour the
// active `PnMode` (`patCopy 8` … `notPatBic 15`). Each `*_pattern_mode`
// primitive shares its boundary computation with the round-8
// `*_pattern` variant but routes every cell write through
// [`plot_pattern_pixel_mode`] so the §3-44 Boolean op applies.
//
// `mode = PatCopy` (the §3 default) collapses to the existing round-8
// pattern path bit-for-bit — every `_pattern_mode` shape simply forwards
// to the original `_pattern` primitive in that case. The other seven
// modes go through the per-cell read-modify-write path; the solid-fg /
// solid-bg pattern collapses don't fire because the mode changes what
// "off" cells do (write `fg` instead of `bg`, leave destination
// unchanged, etc) and the existing solid-colour fast paths assume
// patCopy semantics.
// ---------------------------------------------------------------------------

/// Mode-aware rectangle fill — see [`fill_rect_pattern`] for the
/// `patCopy` baseline shape.
pub fn fill_rect_pattern_mode(
    canvas: &mut Canvas,
    top: i32,
    left: i32,
    bottom: i32,
    right: i32,
    pat: Pattern,
    fg: Rgba,
    bg: Rgba,
    mode: PatternMode,
) {
    if mode.is_pat_copy() {
        fill_rect_pattern(canvas, top, left, bottom, right, pat, fg, bg);
        return;
    }
    if right <= left || bottom <= top {
        return;
    }
    for y in top..bottom {
        for x in left..right {
            plot_pattern_pixel_mode(canvas, x, y, pat, fg, bg, mode);
        }
    }
}

/// Mode-aware oval fill — see [`fill_oval_pattern`].
pub fn fill_oval_pattern_mode(
    canvas: &mut Canvas,
    top: i32,
    left: i32,
    bottom: i32,
    right: i32,
    pat: Pattern,
    fg: Rgba,
    bg: Rgba,
    mode: PatternMode,
) {
    if mode.is_pat_copy() {
        fill_oval_pattern(canvas, top, left, bottom, right, pat, fg, bg);
        return;
    }
    if right <= left || bottom <= top {
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
            plot_pattern_pixel_mode(canvas, x, y, pat, fg, bg, mode);
        }
    }
}

/// Mode-aware round-rectangle fill — see [`fill_round_rect_pattern`].
pub fn fill_round_rect_pattern_mode(
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
    mode: PatternMode,
) {
    if mode.is_pat_copy() {
        fill_round_rect_pattern(
            canvas, top, left, bottom, right, oval_w, oval_h, pat, fg, bg,
        );
        return;
    }
    if right <= left || bottom <= top {
        return;
    }
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
            plot_pattern_pixel_mode(canvas, left + sx as i32, top + sy as i32, pat, fg, bg, mode);
        }
    }
}

/// Mode-aware polygon fill — see [`fill_polygon_pattern`].
pub fn fill_polygon_pattern_mode(
    canvas: &mut Canvas,
    vertices: &[(i32, i32)],
    pat: Pattern,
    fg: Rgba,
    bg: Rgba,
    mode: PatternMode,
) {
    if mode.is_pat_copy() {
        fill_polygon_pattern(canvas, vertices, pat, fg, bg);
        return;
    }
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
            plot_pattern_pixel_mode(
                canvas,
                min_x + sx as i32,
                min_y + sy as i32,
                pat,
                fg,
                bg,
                mode,
            );
        }
    }
}

/// Mode-aware patterned-frame rectangle — see
/// [`frame_rect_pattern_thick`].
pub fn frame_rect_pattern_thick_mode(
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
    mode: PatternMode,
) {
    if mode.is_pat_copy() {
        frame_rect_pattern_thick(canvas, top, left, bottom, right, pen_h, pen_v, pat, fg, bg);
        return;
    }
    if right <= left || bottom <= top {
        return;
    }
    let ph = pen_h.max(1);
    let pv = pen_v.max(1);
    for dy in 0..pv {
        for x in left..right {
            plot_pattern_pixel_mode(canvas, x, top + dy, pat, fg, bg, mode);
            plot_pattern_pixel_mode(canvas, x, bottom - 1 - dy, pat, fg, bg, mode);
        }
    }
    for y in (top + pv)..(bottom - pv) {
        for dx in 0..ph {
            plot_pattern_pixel_mode(canvas, left + dx, y, pat, fg, bg, mode);
            plot_pattern_pixel_mode(canvas, right - 1 - dx, y, pat, fg, bg, mode);
        }
    }
}

// ---------------------------------------------------------------------------
// PixPattern variants (round 91 — PixPat / colour 8×8 tile).
//
// Each helper mirrors the matching `*_pattern` primitive but renders
// each cell from the colour-pixmap tile directly (no fg / bg
// substitution). The tile wraps modulo 8 along both axes; the
// QuickDraw origin maps to cell `[0][0]`.
// ---------------------------------------------------------------------------

#[inline]
fn plot_pix_pattern(canvas: &mut Canvas, x: i32, y: i32, pp: &PixPattern) {
    canvas.put(x, y, pp.sample(x, y));
}

/// Colour-pixmap rectangle fill.
pub fn fill_rect_pix_pattern(
    canvas: &mut Canvas,
    top: i32,
    left: i32,
    bottom: i32,
    right: i32,
    pp: &PixPattern,
) {
    if right <= left || bottom <= top {
        return;
    }
    for y in top..bottom {
        for x in left..right {
            plot_pix_pattern(canvas, x, y, pp);
        }
    }
}

/// Colour-pixmap ellipse fill.
pub fn fill_oval_pix_pattern(
    canvas: &mut Canvas,
    top: i32,
    left: i32,
    bottom: i32,
    right: i32,
    pp: &PixPattern,
) {
    if right <= left || bottom <= top {
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
            plot_pix_pattern(canvas, x, y, pp);
        }
    }
}

/// Colour-pixmap round-rectangle fill.
pub fn fill_round_rect_pix_pattern(
    canvas: &mut Canvas,
    top: i32,
    left: i32,
    bottom: i32,
    right: i32,
    oval_w: i32,
    oval_h: i32,
    pp: &PixPattern,
) {
    if right <= left || bottom <= top {
        return;
    }
    // Render via the marker-pixel trick used by `fill_round_rect_pattern`:
    // rasterise the shape onto a scratch canvas in a marker colour, then
    // re-plot from the colour tile wherever the marker is present.
    let w = (right - left) as u32;
    let h = (bottom - top) as u32;
    let marker = Rgba {
        r: 1,
        g: 2,
        b: 3,
        a: 4,
    };
    let mut scratch = Canvas::new(w.max(1), h.max(1), Rgba::new(0, 0, 0, 0));
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
            plot_pix_pattern(canvas, left + sx as i32, top + sy as i32, pp);
        }
    }
}

/// Colour-pixmap polygon fill.
pub fn fill_polygon_pix_pattern(canvas: &mut Canvas, vertices: &[(i32, i32)], pp: &PixPattern) {
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
            plot_pix_pattern(canvas, min_x + sx as i32, min_y + sy as i32, pp);
        }
    }
}

/// Colour-pixmap frame-rect with a `pen_h × pen_v` brush.
pub fn frame_rect_pix_pattern_thick(
    canvas: &mut Canvas,
    top: i32,
    left: i32,
    bottom: i32,
    right: i32,
    pen_h: i32,
    pen_v: i32,
    pp: &PixPattern,
) {
    if right <= left || bottom <= top {
        return;
    }
    let ph = pen_h.max(1);
    let pv = pen_v.max(1);
    for dy in 0..pv {
        for x in left..right {
            plot_pix_pattern(canvas, x, top + dy, pp);
            plot_pix_pattern(canvas, x, bottom - 1 - dy, pp);
        }
    }
    for y in (top + pv)..(bottom - pv) {
        for dx in 0..ph {
            plot_pix_pattern(canvas, left + dx, y, pp);
            plot_pix_pattern(canvas, right - 1 - dx, y, pp);
        }
    }
}

// ---------------------------------------------------------------------------
// Invert-verb shapes — round 252.
// ---------------------------------------------------------------------------
//
// Inside Macintosh: Imaging With QuickDraw §3 ("QuickDraw Drawing
// Reference") `InvertRect` / `InvertOval` / `InvertRoundRect` /
// `InvertArc` / `InvertPoly` (and the corresponding §A-3 Table A-2
// opcodes `$0033` / `$0053` / `$0043` / `$0063` / `$0073`) toggle every
// pixel in the shape's interior — on a 1-bit display, the literal
// Boolean NOT; on our true-colour canvas, channel-wise NOT per
// `invert_rgba`. Round 252 wires the rounded-rect / oval / arc / poly
// verbs to honour the spec; the rect verb already routed through
// `invert_rect` at the decoder level (round 2).
//
// Each helper computes the same per-row coverage the matching `fill_*`
// helper produces, then inverts the covered spans instead of writing a
// fixed colour. This keeps the geometric kernel exactly synchronised
// with the fill side so a round-trip (`InvertVerb` then `InvertVerb`
// again) restores the canvas pixel-for-pixel — the §3 spec contract.

/// Invert every pixel of the filled-ellipse interior fitted to
/// `(top, left, bottom, right)` per Inside Macintosh §3 `InvertOval`.
pub fn invert_oval(canvas: &mut Canvas, top: i32, left: i32, bottom: i32, right: i32) {
    if right <= left || bottom <= top {
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
        canvas.invert_span(top + i as i32, *lo, *hi + 1);
    }
}

/// Invert every pixel of the filled-round-rectangle interior per
/// Inside Macintosh §3 `InvertRoundRect`.
pub fn invert_round_rect(
    canvas: &mut Canvas,
    top: i32,
    left: i32,
    bottom: i32,
    right: i32,
    oval_w: i32,
    oval_h: i32,
) {
    if right <= left || bottom <= top {
        return;
    }
    let ow = oval_w.max(0).min(right - left);
    let oh = oval_h.max(0).min(bottom - top);
    let ry = oh / 2;
    // Middle band: full width.
    for y in (top + ry)..(bottom - ry) {
        canvas.invert_span(y, left, right);
    }
    // Top + bottom strips: width modulated by corner ellipses.
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
        canvas.invert_span(top + i as i32, *lo, *hi + 1);
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
        canvas.invert_span(bottom - 1 - i as i32, *lo, *hi + 1);
    }
}

/// Invert every pixel of the polygon interior (even-odd parity) per
/// Inside Macintosh §3 `InvertPoly`. Vertices in `(x, y)` order; the
/// polygon is implicitly closed.
pub fn invert_polygon(canvas: &mut Canvas, vertices: &[(i32, i32)]) {
    if vertices.len() < 3 {
        return;
    }
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
    let scan_hi = y_max.min(canvas.height as i32 - 1);
    let n = vertices.len();
    for y in scan_lo..=scan_hi {
        let yf = y as f64 + 0.5;
        let mut xs = Vec::new();
        for i in 0..n {
            let (x0, y0) = vertices[i];
            let (x1, y1) = vertices[(i + 1) % n];
            let y0f = y0 as f64;
            let y1f = y1 as f64;
            if (y0f <= yf && y1f > yf) || (y1f <= yf && y0f > yf) {
                let t = (yf - y0f) / (y1f - y0f);
                let x = x0 as f64 + t * (x1 - x0) as f64;
                xs.push(x);
            }
        }
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal));
        let mut i = 0;
        while i + 1 < xs.len() {
            let x0 = xs[i].floor() as i32;
            let x1 = (xs[i + 1].ceil() as i32).max(x0 + 1);
            canvas.invert_span(y, x0, x1);
            i += 2;
        }
    }
}

/// Invert every pixel of the filled-arc wedge per Inside Macintosh §3
/// `InvertArc`. Mirrors [`fill_arc`]'s polygon-approximation shape
/// (centre + sampled boundary along the wedge).
pub fn invert_arc(
    canvas: &mut Canvas,
    top: i32,
    left: i32,
    bottom: i32,
    right: i32,
    start_deg: i32,
    arc_deg: i32,
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
    invert_polygon(canvas, &poly);
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
