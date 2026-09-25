//! Typed parsing of the QuickTime picture opcodes' payload interiors.
//!
//! Inside Macintosh: Imaging With QuickDraw §A-3 Table A-2 declares the
//! `CompressedQuickTime` (`$8200`) / `UncompressedQuickTime` (`$8201`)
//! payload bytes "private to QuickTime" — but the layout *is* published,
//! in **Inside Macintosh: QuickTime** (1993), Chapter 3 "Image
//! Compression Manager":
//!
//! * Table 3-1 (page 3-26) — the `$8200` fixed header: `Version`, a 3×3
//!   `Fixed` transformation matrix, `MatteSize` / `MatteRect`, transfer
//!   `Mode`, `SrcRect`, preferred `Accuracy`, `MaskSize` — followed by
//!   five variable fields (matte image description, matte data, mask
//!   region, image description, image data) gated on `MatteSize` /
//!   `MaskSize`.
//! * Table 3-2 (page 3-27) — the `$8201` fixed header (stops after
//!   `MatteRect`), followed by the matte fields and one embedded
//!   Color-QuickDraw pixel-data **subopcode** (`$98` / `$99` / `$9A` /
//!   `$9B`) whose bytes sit wholly inside the `$8200`-style `Size`
//!   window.
//! * Pages 3-49 – 3-51 — the [`ImageDescription`] structure both
//!   opcodes embed: a self-sizing (`idSize`-prefixed) record carrying
//!   the compressor FourCC (`cType`), source dimensions, resolution,
//!   `dataSize`, frame count, display name, depth and colour-table id.
//!
//! The same chapter (page 3-26) warns 1993 Mac applications not to read
//! the opcode directly and to honour the `Size` field even when the
//! payload cannot be decoded — a machine without QuickTime "ignores the
//! new opcodes". This module therefore parses defensively: the caller
//! (the opcode walker) has already bounded the payload by `Size`, and a
//! payload whose interior doesn't match the published layout degrades
//! to the verbatim capture rather than failing the whole picture.
//!
//! The compressed image data itself is **not** decoded here: the codec
//! named by [`ImageDescription::codec`] is a CODEC-tag boundary. The
//! decoder hands it to a [`crate::qtimage::QuickTimeImageDecoder`]
//! (`'raw '` built in, `'jpeg'` through `oxideav-mjpeg` with the
//! `registry` feature, anything else through a caller-supplied
//! decoder or `oxideav-core` registry) and composites the pixels with
//! the [`QuickTimeMatrix`] / matte / mask / mode carried by the
//! wrapper; a compressor nobody decodes leaves the payload available
//! as typed bytes and is reported on
//! [`PictQuickTime::render`](crate::PictQuickTime::render).

use crate::error::{PictError, Result};
use crate::header::Fixed;
use crate::reader::Reader;
use crate::state::RectI32;

/// The 3×3 transformation matrix carried by both QuickTime picture
/// opcodes (Inside Macintosh: QuickTime, Table 3-1 / 3-2: "3 by 3
/// fixed transformation matrix", 36 bytes).
///
/// Stored row-major as nine 32-bit values exactly as read off disk, in
/// the serialised order `a, b, u, c, d, v, tx, ty, w` (Inside
/// Macintosh: QuickTime Figure 2-19, book page 2-26). The first two
/// columns (`a b / c d / tx ty`) are [`Fixed`] 16.16; the third column
/// (`u v w`, longs 2 / 5 / 8) is `Fract` 2.30 (book page 2-28: "All of
/// the elements in the first two columns of a matrix are represented
/// by `Fixed` values. Values in the third column are represented as
/// `Fract` values"), so the emitter-written identity ends in
/// `0x40000000`. [`is_identity`](Self::is_identity) also accepts the
/// all-16.16 identity that pre-dates that reading.
///
/// Coordinates are **row vectors** multiplied on the left (Figure
/// 2-19): `x' = a·x + c·y + tx`, `y' = b·x + d·y + ty`, translation in
/// the bottom row. See [`transform_point`](Self::transform_point) and
/// [`transform_rect`](Self::transform_rect).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuickTimeMatrix(pub [Fixed; 9]);

impl QuickTimeMatrix {
    /// 16.16 representation of 1.0.
    const ONE_16_16: i32 = 0x0001_0000;
    /// 2.30 (`Fract`) representation of 1.0.
    const ONE_2_30: i32 = 0x4000_0000;

    /// The identity matrix in all-16.16 form.
    pub const IDENTITY: Self = Self([
        Fixed(Self::ONE_16_16),
        Fixed(0),
        Fixed(0),
        Fixed(0),
        Fixed(Self::ONE_16_16),
        Fixed(0),
        Fixed(0),
        Fixed(0),
        Fixed(Self::ONE_16_16),
    ]);

    /// The identity matrix as QuickTime writes it: `a = d = 1.0` in
    /// 16.16 and `w = 1.0` in `Fract` 2.30 (`0x40000000`) — the
    /// byte-exact form observed in emitter-written `$8200` opcodes.
    pub const IDENTITY_FRACT: Self = Self([
        Fixed(Self::ONE_16_16),
        Fixed(0),
        Fixed(0),
        Fixed(0),
        Fixed(Self::ONE_16_16),
        Fixed(0),
        Fixed(0),
        Fixed(0),
        Fixed(Self::ONE_2_30),
    ]);

    /// Matrix element at `row`, `col` (0-based, row-major).
    pub fn at(&self, row: usize, col: usize) -> Fixed {
        self.0[row * 3 + col]
    }

    /// A translate-and-scale matrix: `a = sx`, `d = sy`, `tx`, `ty`
    /// (Figures 2-21 / 2-22), with the identity third column. This is
    /// the shape `RectMatrix` (book page 2-351) produces to map one
    /// rectangle onto another.
    pub fn scale_translate(sx: Fixed, sy: Fixed, tx: Fixed, ty: Fixed) -> Self {
        Self([
            sx,
            Fixed(0),
            Fixed(0),
            Fixed(0),
            sy,
            Fixed(0),
            tx,
            ty,
            Fixed(Self::ONE_2_30),
        ])
    }

    /// The `RectMatrix` relationship (book page 2-351): the matrix that
    /// maps `src` onto `dst` by translating and scaling, so that
    /// [`transform_rect`](Self::transform_rect)`(src) == dst` (page
    /// 2-352). Returns `None` when `src` has a zero side.
    pub fn rect_matrix(src: RectI32, dst: RectI32) -> Option<Self> {
        let sw = i64::from(src.right) - i64::from(src.left);
        let sh = i64::from(src.bottom) - i64::from(src.top);
        if sw == 0 || sh == 0 {
            return None;
        }
        let dw = i64::from(dst.right) - i64::from(dst.left);
        let dh = i64::from(dst.bottom) - i64::from(dst.top);
        let sx = (dw << 16) / sw;
        let sy = (dh << 16) / sh;
        // tx = dst.left − src.left·sx (all 16.16), likewise ty.
        let tx = (i64::from(dst.left) << 16) - i64::from(src.left) * sx;
        let ty = (i64::from(dst.top) << 16) - i64::from(src.top) * sy;
        let fits = |v: i64| i32::try_from(v).ok().map(Fixed);
        Some(Self::scale_translate(
            fits(sx)?,
            fits(sy)?,
            fits(tx)?,
            fits(ty)?,
        ))
    }

    /// The third-column (`Fract` 2.30) value of `w`, tolerating the
    /// legacy all-16.16 encoding of 1.0.
    fn w_as_f64(&self) -> f64 {
        let w = self.0[8].0;
        if w == Self::ONE_16_16 {
            1.0
        } else {
            f64::from(w) / f64::from(Self::ONE_2_30)
        }
    }

    /// `true` when the third column is the documented `u = v = 0`,
    /// `w = 1` (book page 2-26: "QuickTime assumes that the values of
    /// the matrix elements u and v are always 0.0, and the value of
    /// matrix element w is always 1.0"), i.e. the matrix is affine.
    pub fn is_affine(&self) -> bool {
        self.0[2].0 == 0
            && self.0[5].0 == 0
            && (self.0[8].0 == Self::ONE_2_30 || self.0[8].0 == Self::ONE_16_16)
    }

    /// `true` when the matrix only translates and/or scales (`b = c =
    /// 0`, affine third column) — the `identityMatrixType` /
    /// `translateMatrixType` / `scaleMatrixType` /
    /// `scaleTranslateMatrixType` classes of `GetMatrixType` (book
    /// page 2-342). For these, [`transform_rect`](Self::transform_rect)
    /// is exact and the blit is a plain rectangle-to-rectangle copy.
    pub fn is_scale_translate(&self) -> bool {
        self.is_affine() && self.0[1].0 == 0 && self.0[3].0 == 0
    }

    /// The matrix as `f64` cells in row-major `[[a, b, u], [c, d, v],
    /// [tx, ty, w]]` order, each converted from its own fixed-point
    /// format (16.16 for the first two columns, 2.30 for the third).
    pub fn to_f64(&self) -> [[f64; 3]; 3] {
        let fx = |i: usize| f64::from(self.0[i].0) / 65536.0;
        let fr = |i: usize| f64::from(self.0[i].0) / f64::from(Self::ONE_2_30);
        [
            [fx(0), fx(1), fr(2)],
            [fx(3), fx(4), fr(5)],
            [fx(6), fx(7), self.w_as_f64()],
        ]
    }

    /// Map a point through the affine part of the matrix with the
    /// published equations (book page 2-26) `x' = a·x + c·y + tx`,
    /// `y' = b·x + d·y + ty`, in 16.16 arithmetic, rounding the result
    /// half-up to integer QuickDraw coordinates. (`TransformPoints`,
    /// page 2-347, leaves the integer reduction unspecified; half-up is
    /// this crate's choice and is stated in the README.)
    pub fn transform_point(&self, x: i32, y: i32) -> (i32, i32) {
        let m = |i: usize| i64::from(self.0[i].0);
        let (x, y) = (i64::from(x), i64::from(y));
        let xp = m(0) * x + m(3) * y + m(6);
        let yp = m(1) * x + m(4) * y + m(7);
        let round =
            |v: i64| ((v + 0x8000) >> 16).clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
        (round(xp), round(yp))
    }

    /// `TransformRect` (book pages 2-348 – 2-349): map the rectangle's
    /// corners through the matrix and return their bounding box. For a
    /// scale/translate matrix this is the exact destination rectangle;
    /// for rotations / skews it is the bounding rectangle the book
    /// describes. The result is normalised (`left <= right`, `top <=
    /// bottom`) so a negative scale factor — "by making the x factor
    /// negative, you can flip the image" (page 2-27) — yields the same
    /// rectangle with the flip recorded by the matrix itself.
    pub fn transform_rect(&self, r: RectI32) -> RectI32 {
        let corners = [
            self.transform_point(r.left, r.top),
            self.transform_point(r.right, r.top),
            self.transform_point(r.left, r.bottom),
            self.transform_point(r.right, r.bottom),
        ];
        let left = corners.iter().map(|c| c.0).min().unwrap_or(0);
        let right = corners.iter().map(|c| c.0).max().unwrap_or(0);
        let top = corners.iter().map(|c| c.1).min().unwrap_or(0);
        let bottom = corners.iter().map(|c| c.1).max().unwrap_or(0);
        RectI32 {
            top,
            left,
            bottom,
            right,
        }
    }

    /// Map a destination-space point back to source space by inverting
    /// the full 3×3 (row-vector convention: `[x y 1] = [x' y' 1] · M⁻¹`,
    /// then the homogeneous divide). Returns `None` for a singular
    /// matrix, or when the point maps behind the projection plane
    /// (`w' <= 0`). The homogeneous divide for a non-affine third
    /// column is the standard projective reading of the `Fract` column
    /// (see `docs/image/quickdraw/pict-quicktime-matrix.md` §4 — an
    /// inference, not a quotation); for the documented affine case it
    /// reduces exactly to the inverse of the published equations.
    pub fn inverse_map(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        let m = self.to_f64();
        let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
            - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
        if !det.is_finite() || det.abs() < 1e-12 {
            return None;
        }
        // Adjugate / det.
        let inv = [
            [
                (m[1][1] * m[2][2] - m[1][2] * m[2][1]) / det,
                (m[0][2] * m[2][1] - m[0][1] * m[2][2]) / det,
                (m[0][1] * m[1][2] - m[0][2] * m[1][1]) / det,
            ],
            [
                (m[1][2] * m[2][0] - m[1][0] * m[2][2]) / det,
                (m[0][0] * m[2][2] - m[0][2] * m[2][0]) / det,
                (m[0][2] * m[1][0] - m[0][0] * m[1][2]) / det,
            ],
            [
                (m[1][0] * m[2][1] - m[1][1] * m[2][0]) / det,
                (m[0][1] * m[2][0] - m[0][0] * m[2][1]) / det,
                (m[0][0] * m[1][1] - m[0][1] * m[1][0]) / det,
            ],
        ];
        // Row vector [x' y' 1] × inv.
        let sx = x * inv[0][0] + y * inv[1][0] + inv[2][0];
        let sy = x * inv[0][1] + y * inv[1][1] + inv[2][1];
        let sw = x * inv[0][2] + y * inv[1][2] + inv[2][2];
        if sw.is_nan() || sw <= 1e-12 {
            return None;
        }
        Some((sx / sw, sy / sw))
    }

    /// Bounding box of `r` mapped through the full matrix in `f64`
    /// (including any perspective divide), rounded outward to integer
    /// pixels. `None` when a corner projects behind the plane or the
    /// result is not finite.
    pub fn transform_rect_f64(&self, r: RectI32) -> Option<RectI32> {
        let m = self.to_f64();
        let mut left = f64::INFINITY;
        let mut right = f64::NEG_INFINITY;
        let mut top = f64::INFINITY;
        let mut bottom = f64::NEG_INFINITY;
        for (x, y) in [
            (r.left, r.top),
            (r.right, r.top),
            (r.left, r.bottom),
            (r.right, r.bottom),
        ] {
            let (x, y) = (f64::from(x), f64::from(y));
            let w = x * m[0][2] + y * m[1][2] + m[2][2];
            if w.is_nan() || w <= 1e-12 {
                return None;
            }
            let xp = (x * m[0][0] + y * m[1][0] + m[2][0]) / w;
            let yp = (x * m[0][1] + y * m[1][1] + m[2][1]) / w;
            if !xp.is_finite() || !yp.is_finite() {
                return None;
            }
            left = left.min(xp);
            right = right.max(xp);
            top = top.min(yp);
            bottom = bottom.max(yp);
        }
        let clamp = |v: f64| v.clamp(f64::from(i32::MIN / 2), f64::from(i32::MAX / 2)) as i32;
        Some(RectI32 {
            top: clamp(top.floor()),
            left: clamp(left.floor()),
            bottom: clamp(bottom.ceil()),
            right: clamp(right.ceil()),
        })
    }

    /// `true` when the matrix maps coordinates unchanged.
    ///
    /// Accepts the bottom-right element as either the 16.16 identity
    /// (`0x00010000`) or the 2.30 `Fract` identity (`0x40000000`) —
    /// the book leaves the third column's number format open (Table
    /// 3-1 documents the whole field only as "fixed"), and both
    /// conventions denote 1.0.
    pub fn is_identity(&self) -> bool {
        let off_diag_zero = [1usize, 2, 3, 5, 6, 7].iter().all(|&i| self.0[i].0 == 0);
        off_diag_zero
            && self.0[0].0 == Self::ONE_16_16
            && self.0[4].0 == Self::ONE_16_16
            && (self.0[8].0 == Self::ONE_16_16 || self.0[8].0 == Self::ONE_2_30)
    }

    fn parse(r: &mut Reader<'_>) -> Result<Self> {
        let mut m = [Fixed(0); 9];
        for slot in &mut m {
            *slot = Fixed(r.read_i32()?);
        }
        Ok(Self(m))
    }

    /// Serialise back to the 36-byte on-disk form.
    pub fn to_wire(&self) -> [u8; 36] {
        let mut out = [0u8; 36];
        for (i, f) in self.0.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&f.0.to_be_bytes());
        }
        out
    }
}

/// The QuickTime `ImageDescription` structure (Inside Macintosh:
/// QuickTime, pages 3-49 – 3-51) as embedded in the `$8200` / `$8201`
/// picture opcodes.
///
/// Self-sizing: the leading `idSize` long covers the whole record
/// **including** any trailing extension / custom colour-table bytes
/// (`idSize − 86` of them — the fixed part is 86 bytes). The reserved
/// `resvd1` / `resvd2` / `dataRefIndex` fields ("must be 0") are
/// walked over but not stored; a defensive reader does not reject a
/// record over reserved bits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageDescription {
    /// Total on-disk size of the record including extensions
    /// (`idSize`, page 3-50). Always `>= 86`.
    pub id_size: u32,
    /// Compressor FourCC (`cType`): `b"jpeg"`, `b"rle "`, `b"rpza"`,
    /// … Stored exactly as read; see [`Self::codec_str`] for a
    /// printable form.
    pub codec: [u8; 4],
    /// Version of the compressed data (`version`).
    pub version: u16,
    /// Version of the compressor that created it (`revisionLevel`).
    pub revision_level: u16,
    /// Compressor developer FourCC (`vendor`).
    pub vendor: [u8; 4],
    /// `CodecQ` temporal quality — sequences only, 0 for stills.
    pub temporal_quality: u32,
    /// `CodecQ` spatial quality.
    pub spatial_quality: u32,
    /// Source image width in pixels.
    pub width: u16,
    /// Source image height in pixels.
    pub height: u16,
    /// Horizontal resolution, dpi (`Fixed`).
    pub h_res: Fixed,
    /// Vertical resolution, dpi (`Fixed`).
    pub v_res: Fixed,
    /// Size of the compressed image data in bytes. Page 3-51: still
    /// images only, and "Set this field to 0 if the size is unknown"
    /// — in that case the payload's `Size` window bounds the data
    /// (see [`parse_compressed_quicktime`]).
    pub data_size: u32,
    /// Number of frames in the image data (`frameCount`; 1 for a
    /// still).
    pub frame_count: u16,
    /// The `Str31 name` field verbatim: 1 length byte + 31 content
    /// bytes, "always takes up 32 bytes no matter how long the string
    /// is" (page 3-51). [`Self::name`] decodes it.
    pub name_raw: [u8; 32],
    /// Colour depth: 1/2/4/8/16/24/32; the special values 34 / 36 /
    /// 40 mean 2-, 4- and 8-bit **grayscale** (page 3-51).
    pub depth: u16,
    /// Colour-table ID (`clutID`).
    pub clut_id: i16,
    /// The `idSize − 86` extension bytes (image-description
    /// extensions and/or a custom colour table when `clut_id` calls
    /// for one). Empty when `idSize == 86`.
    pub extension: Vec<u8>,
}

/// Fixed (pre-extension) byte length of an [`ImageDescription`].
#[doc(hidden)] // internal wire plumbing; not part of the stable API
pub const IMAGE_DESCRIPTION_FIXED_LEN: usize = 86;

impl ImageDescription {
    /// The compressor FourCC as a lossy printable string (e.g.
    /// `"jpeg"`).
    pub fn codec_str(&self) -> String {
        self.codec.iter().map(|&b| b as char).collect()
    }

    /// The `name` Pascal string decoded to at most 31 bytes
    /// (Mac-Roman range folded through a lossy char cast).
    pub fn name(&self) -> String {
        let len = (self.name_raw[0] as usize).min(31);
        self.name_raw[1..1 + len]
            .iter()
            .map(|&b| b as char)
            .collect()
    }

    /// `true` when [`Self::depth`] is one of the grayscale encodings
    /// (34 / 36 / 40 = 2-, 4-, 8-bit grayscale per page 3-51).
    pub fn is_grayscale(&self) -> bool {
        matches!(self.depth, 34 | 36 | 40)
    }

    /// Parse one `ImageDescription` at the cursor. Consumes exactly
    /// `idSize` bytes (including the extension tail).
    pub fn parse(r: &mut Reader<'_>) -> Result<Self> {
        let start = r.pos;
        let id_size = r.read_u32()?;
        if (id_size as usize) < IMAGE_DESCRIPTION_FIXED_LEN {
            return Err(PictError::invalid(format!(
                "ImageDescription idSize {id_size} smaller than the {IMAGE_DESCRIPTION_FIXED_LEN}-byte fixed part"
            )));
        }
        let mut codec = [0u8; 4];
        codec.copy_from_slice(r.read_bytes(4)?);
        // resvd1 (long) + resvd2 (short) + dataRefIndex (short):
        // reserved, "must be 0" — walked, not stored, not enforced.
        r.skip(8)?;
        let version = r.read_u16()?;
        let revision_level = r.read_u16()?;
        let mut vendor = [0u8; 4];
        vendor.copy_from_slice(r.read_bytes(4)?);
        let temporal_quality = r.read_u32()?;
        let spatial_quality = r.read_u32()?;
        let width = r.read_u16()?;
        let height = r.read_u16()?;
        let h_res = Fixed(r.read_i32()?);
        let v_res = Fixed(r.read_i32()?);
        let data_size = r.read_u32()?;
        let frame_count = r.read_u16()?;
        let mut name_raw = [0u8; 32];
        name_raw.copy_from_slice(r.read_bytes(32)?);
        let depth = r.read_u16()?;
        let clut_id = r.read_i16()?;
        debug_assert_eq!(r.pos - start, IMAGE_DESCRIPTION_FIXED_LEN);
        let ext_len = id_size as usize - IMAGE_DESCRIPTION_FIXED_LEN;
        let extension = r.read_bytes(ext_len)?.to_vec();
        Ok(Self {
            id_size,
            codec,
            version,
            revision_level,
            vendor,
            temporal_quality,
            spatial_quality,
            width,
            height,
            h_res,
            v_res,
            data_size,
            frame_count,
            name_raw,
            depth,
            clut_id,
            extension,
        })
    }

    /// Serialise to the on-disk form. `id_size` is recomputed from
    /// the extension length (86 + `extension.len()`), so a
    /// parse → emit round-trip of a conforming record is
    /// byte-identical.
    pub fn to_bytes(&self) -> Vec<u8> {
        let id_size = (IMAGE_DESCRIPTION_FIXED_LEN + self.extension.len()) as u32;
        let mut out = Vec::with_capacity(id_size as usize);
        out.extend_from_slice(&id_size.to_be_bytes());
        out.extend_from_slice(&self.codec);
        out.extend_from_slice(&[0u8; 8]); // resvd1 + resvd2 + dataRefIndex
        out.extend_from_slice(&self.version.to_be_bytes());
        out.extend_from_slice(&self.revision_level.to_be_bytes());
        out.extend_from_slice(&self.vendor);
        out.extend_from_slice(&self.temporal_quality.to_be_bytes());
        out.extend_from_slice(&self.spatial_quality.to_be_bytes());
        out.extend_from_slice(&self.width.to_be_bytes());
        out.extend_from_slice(&self.height.to_be_bytes());
        out.extend_from_slice(&self.h_res.0.to_be_bytes());
        out.extend_from_slice(&self.v_res.0.to_be_bytes());
        out.extend_from_slice(&self.data_size.to_be_bytes());
        out.extend_from_slice(&self.frame_count.to_be_bytes());
        out.extend_from_slice(&self.name_raw);
        out.extend_from_slice(&self.depth.to_be_bytes());
        out.extend_from_slice(&self.clut_id.to_be_bytes());
        out.extend_from_slice(&self.extension);
        out
    }
}

/// A matte: its own [`ImageDescription`] plus the compressed matte
/// data (the first two of the `$8200` variable fields, page 3-26;
/// also carried by `$8201`). Present only when `MatteSize != 0`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuickTimeMatte {
    /// The matte's image description.
    pub description: ImageDescription,
    /// The compressed matte data (`MatteSize` bytes).
    pub data: Vec<u8>,
}

/// Typed form of a `CompressedQuickTime` (`$8200`) payload — the bytes
/// after the 4-byte `Size` field, per Inside Macintosh: QuickTime
/// Table 3-1 (page 3-26).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuickTimeCompressed {
    /// `Version` — version of this opcode.
    pub version: u16,
    /// The 3×3 display matrix.
    pub matrix: QuickTimeMatrix,
    /// `MatteRect` — rectangle for the matte data. Meaningful only
    /// when a matte is present.
    pub matte_rect: RectI32,
    /// `Mode` — QuickDraw transfer mode for the draw.
    pub mode: u16,
    /// `SrcRect` — source rectangle.
    pub src_rect: RectI32,
    /// `Accuracy` — preferred decompression accuracy (`CodecQ`-style
    /// long).
    pub accuracy: u32,
    /// Matte image description + data, present iff `MatteSize != 0`.
    pub matte: Option<QuickTimeMatte>,
    /// Raw mask-region bytes (a QuickDraw `Region`, `MaskSize` of
    /// them), present iff `MaskSize != 0`. Kept raw: the region's own
    /// leading `rgnSize` word should agree with `MaskSize` (the
    /// parser checks that the interior is at least self-consistent
    /// enough to walk, but stores the verbatim bytes).
    pub mask_region: Option<Vec<u8>>,
    /// The image description for the compressed image data.
    pub image_description: ImageDescription,
    /// The compressed image data. Length comes from the image
    /// description's `dataSize`, or — when that is 0, "if the size is
    /// unknown" (page 3-51) — from the remainder of the `Size`-bounded
    /// payload.
    pub image_data: Vec<u8>,
}

/// Fixed byte length of the `$8200` payload before the variable
/// fields (Table 3-1 sizes summed, excluding the 2-byte opcode and
/// 4-byte `Size`): `Version` 2 + `Matrix` 36 + `MatteSize` 4 +
/// `MatteRect` 8 + `Mode` 2 + `SrcRect` 8 + `Accuracy` 4 +
/// `MaskSize` 4.
#[doc(hidden)] // internal wire plumbing; not part of the stable API
pub const COMPRESSED_QT_FIXED_LEN: usize = 68;

/// Fixed byte length of the `$8201` payload before the variable
/// fields (Table 3-2 stops after `MatteRect`): `Version` 2 +
/// `Matrix` 36 + `MatteSize` 4 + `MatteRect` 8.
#[doc(hidden)] // internal wire plumbing; not part of the stable API
pub const UNCOMPRESSED_QT_FIXED_LEN: usize = 50;

/// Typed form of an `UncompressedQuickTime` (`$8201`) payload — the
/// bytes after the 4-byte `Size` field, per Inside Macintosh:
/// QuickTime Table 3-2 (page 3-27).
///
/// The image itself travels as one embedded Color-QuickDraw pixel-data
/// subopcode (`$0098` `PackBitsRect` / `$0099` `PackBitsRgn` / `$009A`
/// `DirectBitsRect` / `$009B` `DirectBitsRgn`) whose bytes are wholly
/// inside the wrapper's `Size` window — "hence it is not included if
/// the QuickTime opcode is skipped", which is the intended fallback on
/// a machine without QuickTime. The crate's decoder re-enters its
/// normal raster dispatch on [`Self::sub_data`] and blits the result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuickTimeUncompressed {
    /// `Version` — version of this opcode.
    pub version: u16,
    /// The 3×3 display matrix.
    pub matrix: QuickTimeMatrix,
    /// `MatteRect` — rectangle for the matte data.
    pub matte_rect: RectI32,
    /// Matte image description + data, present iff `MatteSize != 0`.
    pub matte: Option<QuickTimeMatte>,
    /// The embedded subopcode word — one of `0x0098` / `0x0099` /
    /// `0x009A` / `0x009B`.
    pub subopcode: u16,
    /// The subopcode's data: everything between the subopcode word
    /// and the end of the `Size`-bounded payload, verbatim.
    pub sub_data: Vec<u8>,
}

impl QuickTimeUncompressed {
    /// `true` when [`Self::subopcode`] is one of the four documented
    /// pixel-data subopcodes.
    pub fn subopcode_in_range(&self) -> bool {
        (0x0098..=0x009B).contains(&self.subopcode)
    }
}

impl QuickTimeCompressed {
    /// Convenience constructor for the common still-image case: an
    /// identity display matrix, `srcCopy` transfer mode, a source
    /// rectangle spanning the description's `width × height`, no
    /// matte, no mask. `image_description.data_size` is synchronised
    /// to `image_data.len()`.
    pub fn still(mut image_description: ImageDescription, image_data: Vec<u8>) -> Self {
        image_description.data_size = image_data.len() as u32;
        let src_rect = RectI32 {
            top: 0,
            left: 0,
            bottom: image_description.height as i32,
            right: image_description.width as i32,
        };
        Self {
            version: 0,
            matrix: QuickTimeMatrix::IDENTITY,
            matte_rect: RectI32::default(),
            mode: 0, // srcCopy
            src_rect,
            accuracy: 0,
            matte: None,
            mask_region: None,
            image_description,
            image_data,
        }
    }

    /// Serialise to the on-disk `$8200` payload form (the bytes after
    /// the 4-byte `Size` field) per Table 3-1. `MatteSize` /
    /// `MaskSize` are derived from the optional fields' lengths.
    ///
    /// Errors when the structure cannot round-trip: a matte with
    /// empty data (its gate would read as absent), a mask region
    /// shorter than the 10-byte `Region` header, or a non-zero
    /// `image_description.data_size` disagreeing with
    /// `image_data.len()` (zero stays zero — the documented "size
    /// unknown" form, recovered from the `Size` window on parse).
    pub fn to_payload_bytes(&self) -> Result<Vec<u8>> {
        if let Some(m) = &self.matte {
            if m.data.is_empty() {
                return Err(PictError::invalid(
                    "QuickTime matte with empty data: MatteSize 0 gates the matte off entirely",
                ));
            }
        }
        if let Some(rgn) = &self.mask_region {
            if rgn.len() < 10 {
                return Err(PictError::invalid(format!(
                    "QuickTime mask region of {} bytes is smaller than the 10-byte Region header",
                    rgn.len()
                )));
            }
        }
        if self.image_description.data_size != 0
            && self.image_description.data_size as usize != self.image_data.len()
        {
            return Err(PictError::invalid(format!(
                "ImageDescription dataSize {} disagrees with image data length {}",
                self.image_description.data_size,
                self.image_data.len()
            )));
        }
        let mut p = Vec::new();
        p.extend_from_slice(&self.version.to_be_bytes());
        p.extend_from_slice(&self.matrix.to_wire());
        let matte_size = self.matte.as_ref().map_or(0, |m| m.data.len() as u32);
        p.extend_from_slice(&matte_size.to_be_bytes());
        write_rect(&mut p, &self.matte_rect);
        p.extend_from_slice(&self.mode.to_be_bytes());
        write_rect(&mut p, &self.src_rect);
        p.extend_from_slice(&self.accuracy.to_be_bytes());
        let mask_size = self.mask_region.as_ref().map_or(0, |r| r.len() as u32);
        p.extend_from_slice(&mask_size.to_be_bytes());
        if let Some(m) = &self.matte {
            p.extend_from_slice(&m.description.to_bytes());
            p.extend_from_slice(&m.data);
        }
        if let Some(rgn) = &self.mask_region {
            p.extend_from_slice(rgn);
        }
        p.extend_from_slice(&self.image_description.to_bytes());
        p.extend_from_slice(&self.image_data);
        Ok(p)
    }
}

impl QuickTimeUncompressed {
    /// Convenience constructor wrapping one already-built `$98`–`$9B`
    /// opcode chunk (opcode word first, as produced by
    /// `build_direct_bits_rect_op` and friends): identity matrix, no
    /// matte. Errors when the chunk is shorter than the 2-byte
    /// subopcode word or its opcode is outside the documented
    /// `$98`–`$9B` set.
    pub fn wrapping(sub_chunk: &[u8]) -> Result<Self> {
        if sub_chunk.len() < 2 {
            return Err(PictError::invalid(
                "UncompressedQuickTime sub-chunk shorter than the 2-byte subopcode word",
            ));
        }
        let subopcode = u16::from_be_bytes([sub_chunk[0], sub_chunk[1]]);
        if !(0x0098..=0x009B).contains(&subopcode) {
            return Err(PictError::invalid(format!(
                "UncompressedQuickTime subopcode 0x{subopcode:04X} outside the documented $98–$9B set"
            )));
        }
        Ok(Self {
            version: 0,
            matrix: QuickTimeMatrix::IDENTITY,
            matte_rect: RectI32::default(),
            matte: None,
            subopcode,
            sub_data: sub_chunk[2..].to_vec(),
        })
    }

    /// Serialise to the on-disk `$8201` payload form (the bytes after
    /// the 4-byte `Size` field) per Table 3-2. Errors mirror
    /// [`QuickTimeCompressed::to_payload_bytes`] for the matte gate,
    /// plus the `$98`–`$9B` subopcode range check.
    pub fn to_payload_bytes(&self) -> Result<Vec<u8>> {
        if let Some(m) = &self.matte {
            if m.data.is_empty() {
                return Err(PictError::invalid(
                    "QuickTime matte with empty data: MatteSize 0 gates the matte off entirely",
                ));
            }
        }
        if !self.subopcode_in_range() {
            return Err(PictError::invalid(format!(
                "UncompressedQuickTime subopcode 0x{:04X} outside the documented $98–$9B set",
                self.subopcode
            )));
        }
        let mut p = Vec::new();
        p.extend_from_slice(&self.version.to_be_bytes());
        p.extend_from_slice(&self.matrix.to_wire());
        let matte_size = self.matte.as_ref().map_or(0, |m| m.data.len() as u32);
        p.extend_from_slice(&matte_size.to_be_bytes());
        write_rect(&mut p, &self.matte_rect);
        if let Some(m) = &self.matte {
            p.extend_from_slice(&m.description.to_bytes());
            p.extend_from_slice(&m.data);
        }
        p.extend_from_slice(&self.subopcode.to_be_bytes());
        p.extend_from_slice(&self.sub_data);
        Ok(p)
    }
}

/// Write a QuickDraw `Rect` (top, left, bottom, right as big-endian
/// i16) from the crate's widened [`RectI32`] form. Values outside the
/// i16 range saturate — on-disk rects are i16 by definition.
fn write_rect(out: &mut Vec<u8>, r: &RectI32) {
    for v in [r.top, r.left, r.bottom, r.right] {
        let clamped = v.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
        out.extend_from_slice(&clamped.to_be_bytes());
    }
}

/// Typed view of a QuickTime picture-opcode payload, attached to
/// [`crate::PictQuickTime::image`] when the payload interior matched
/// the published layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuickTimePayload {
    /// `$8200` — compressed image; see [`QuickTimeCompressed`].
    Compressed(QuickTimeCompressed),
    /// `$8201` — uncompressed image wrapping a pixel-data subopcode;
    /// see [`QuickTimeUncompressed`].
    Uncompressed(QuickTimeUncompressed),
}

impl QuickTimePayload {
    /// The compressor FourCC when one is present (`$8200`'s main image
    /// description). `$8201` carries no image description of its own
    /// (its image is raw QuickDraw pixel data).
    pub fn codec(&self) -> Option<[u8; 4]> {
        match self {
            Self::Compressed(c) => Some(c.image_description.codec),
            Self::Uncompressed(_) => None,
        }
    }

    /// The display matrix (both variants carry one).
    pub fn matrix(&self) -> &QuickTimeMatrix {
        match self {
            Self::Compressed(c) => &c.matrix,
            Self::Uncompressed(u) => &u.matrix,
        }
    }
}

/// Parse the matte pair (`ImageDescription` + `matte_size` data
/// bytes) that both opcodes place first in their variable part.
fn parse_matte(r: &mut Reader<'_>, matte_size: u32) -> Result<Option<QuickTimeMatte>> {
    if matte_size == 0 {
        return Ok(None);
    }
    let description = ImageDescription::parse(r)?;
    let data = r.read_bytes(matte_size as usize)?.to_vec();
    Ok(Some(QuickTimeMatte { description, data }))
}

/// Parse a `CompressedQuickTime` (`$8200`) payload — `payload` is the
/// `Size`-bounded byte run following the 4-byte `Size` field (i.e.
/// exactly [`crate::PictQuickTime::data`]).
///
/// Every variable-length interior field is bounded by `payload.len()`
/// (truncation inside the window errors; it never over-reads), so a
/// hostile payload cannot allocate beyond the bytes actually present.
pub fn parse_compressed_quicktime(payload: &[u8]) -> Result<QuickTimeCompressed> {
    let mut r = Reader::new(payload);
    let version = r.read_u16()?;
    let matrix = QuickTimeMatrix::parse(&mut r)?;
    let matte_size = r.read_u32()?;
    let mr = r.read_rect()?;
    let matte_rect = RectI32::from_be(mr.0, mr.1, mr.2, mr.3);
    let mode = r.read_u16()?;
    let sr = r.read_rect()?;
    let src_rect = RectI32::from_be(sr.0, sr.1, sr.2, sr.3);
    let accuracy = r.read_u32()?;
    let mask_size = r.read_u32()?;
    debug_assert_eq!(r.pos, COMPRESSED_QT_FIXED_LEN);

    let matte = parse_matte(&mut r, matte_size)?;
    let mask_region = if mask_size != 0 {
        let bytes = r.read_bytes(mask_size as usize)?;
        // Cross-check: a QuickDraw Region leads with its own 2-byte
        // rgnSize, which should agree with MaskSize. Tolerated when it
        // doesn't (the bytes are kept verbatim either way), but a
        // region shorter than its own 10-byte header is malformed.
        if bytes.len() < 10 {
            return Err(PictError::invalid(format!(
                "QuickTime mask region of {} bytes is smaller than the 10-byte Region header",
                bytes.len()
            )));
        }
        Some(bytes.to_vec())
    } else {
        None
    };
    let image_description = ImageDescription::parse(&mut r)?;
    // Page 3-51: dataSize "still images only"; 0 = unknown, in which
    // case the Size window bounds the data — take the remainder.
    let data_len = if image_description.data_size != 0 {
        image_description.data_size as usize
    } else {
        r.remaining()
    };
    let image_data = r.read_bytes(data_len)?.to_vec();
    Ok(QuickTimeCompressed {
        version,
        matrix,
        matte_rect,
        mode,
        src_rect,
        accuracy,
        matte,
        mask_region,
        image_description,
        image_data,
    })
}

/// Parse an `UncompressedQuickTime` (`$8201`) payload — `payload` is
/// the `Size`-bounded byte run following the 4-byte `Size` field.
///
/// The subopcode's pixel data is *not* interpreted here — it is
/// returned verbatim in [`QuickTimeUncompressed::sub_data`] for the
/// decoder to re-enter its normal `$98`–`$9B` dispatch on. A
/// subopcode outside the documented `$98`–`$9B` set is an error (the
/// wrapper's whole point is to carry one of those four).
pub fn parse_uncompressed_quicktime(payload: &[u8]) -> Result<QuickTimeUncompressed> {
    let mut r = Reader::new(payload);
    let version = r.read_u16()?;
    let matrix = QuickTimeMatrix::parse(&mut r)?;
    let matte_size = r.read_u32()?;
    let mr = r.read_rect()?;
    let matte_rect = RectI32::from_be(mr.0, mr.1, mr.2, mr.3);
    debug_assert_eq!(r.pos, UNCOMPRESSED_QT_FIXED_LEN);

    let matte = parse_matte(&mut r, matte_size)?;
    let subopcode = r.read_u16()?;
    if !(0x0098..=0x009B).contains(&subopcode) {
        return Err(PictError::invalid(format!(
            "UncompressedQuickTime subopcode 0x{subopcode:04X} outside the documented $98–$9B set"
        )));
    }
    let sub_data = r.read_bytes(r.remaining())?.to_vec();
    Ok(QuickTimeUncompressed {
        version,
        matrix,
        matte_rect,
        matte,
        subopcode,
        sub_data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal conforming ImageDescription (idSize = 86, no
    /// extension) with a recognisable FourCC.
    fn sample_desc(data_size: u32) -> ImageDescription {
        let mut name_raw = [0u8; 32];
        name_raw[0] = 5;
        name_raw[1..6].copy_from_slice(b"Photo");
        ImageDescription {
            id_size: 86,
            codec: *b"jpeg",
            version: 1,
            revision_level: 1,
            vendor: *b"appl",
            temporal_quality: 0,
            spatial_quality: 0x0200,
            width: 64,
            height: 48,
            h_res: Fixed::SEVENTY_TWO_DPI,
            v_res: Fixed::SEVENTY_TWO_DPI,
            data_size,
            frame_count: 1,
            name_raw,
            depth: 24,
            clut_id: -1,
            extension: Vec::new(),
        }
    }

    fn compressed_payload(desc: &ImageDescription, image_data: &[u8]) -> Vec<u8> {
        let mut p = Vec::new();
        p.extend_from_slice(&1u16.to_be_bytes()); // version
        p.extend_from_slice(&QuickTimeMatrix::IDENTITY.to_wire());
        p.extend_from_slice(&0u32.to_be_bytes()); // matteSize
        p.extend_from_slice(&[0u8; 8]); // matteRect
        p.extend_from_slice(&0u16.to_be_bytes()); // mode = srcCopy
        for v in [0i16, 0, 48, 64] {
            p.extend_from_slice(&v.to_be_bytes()); // srcRect
        }
        p.extend_from_slice(&0u32.to_be_bytes()); // accuracy
        p.extend_from_slice(&0u32.to_be_bytes()); // maskSize
        p.extend_from_slice(&desc.to_bytes());
        p.extend_from_slice(image_data);
        p
    }

    #[test]
    fn image_description_round_trips() {
        let desc = sample_desc(1234);
        let bytes = desc.to_bytes();
        assert_eq!(bytes.len(), 86);
        let mut r = Reader::new(&bytes);
        let back = ImageDescription::parse(&mut r).unwrap();
        assert_eq!(back, desc);
        assert_eq!(back.codec_str(), "jpeg");
        assert_eq!(back.name(), "Photo");
        assert!(!back.is_grayscale());
    }

    #[test]
    fn image_description_extension_bytes_round_trip() {
        let mut desc = sample_desc(0);
        desc.extension = vec![0xAA; 10];
        let bytes = desc.to_bytes();
        assert_eq!(bytes.len(), 96);
        assert_eq!(&bytes[0..4], &96u32.to_be_bytes());
        let mut r = Reader::new(&bytes);
        let back = ImageDescription::parse(&mut r).unwrap();
        assert_eq!(back.id_size, 96);
        assert_eq!(back.extension, vec![0xAA; 10]);
    }

    #[test]
    fn image_description_rejects_undersized_id_size() {
        let desc = sample_desc(0);
        let mut bytes = desc.to_bytes();
        bytes[0..4].copy_from_slice(&85u32.to_be_bytes());
        let mut r = Reader::new(&bytes);
        assert!(ImageDescription::parse(&mut r).is_err());
    }

    #[test]
    fn grayscale_depths_recognised() {
        for (depth, gray) in [
            (1u16, false),
            (8, false),
            (34, true),
            (36, true),
            (40, true),
        ] {
            let mut d = sample_desc(0);
            d.depth = depth;
            assert_eq!(d.is_grayscale(), gray, "depth {depth}");
        }
    }

    #[test]
    fn parse_compressed_minimal() {
        let data = vec![0x11u8, 0x22, 0x33];
        let desc = sample_desc(data.len() as u32);
        let payload = compressed_payload(&desc, &data);
        let qt = parse_compressed_quicktime(&payload).unwrap();
        assert_eq!(qt.version, 1);
        assert!(qt.matrix.is_identity());
        assert!(qt.matte.is_none());
        assert!(qt.mask_region.is_none());
        assert_eq!(qt.mode, 0);
        assert_eq!(qt.src_rect, RectI32::from_be(0, 0, 48, 64));
        assert_eq!(qt.image_description.codec, *b"jpeg");
        assert_eq!(qt.image_data, data);
    }

    #[test]
    fn parse_compressed_zero_data_size_takes_remainder() {
        // dataSize = 0 ("if the size is unknown", p. 3-51): the image
        // data is everything left inside the Size window.
        let data = vec![0xDEu8; 40];
        let desc = sample_desc(0);
        let payload = compressed_payload(&desc, &data);
        let qt = parse_compressed_quicktime(&payload).unwrap();
        assert_eq!(qt.image_description.data_size, 0);
        assert_eq!(qt.image_data, data);
    }

    #[test]
    fn parse_compressed_data_size_overrun_errors() {
        let data = vec![0x11u8; 4];
        let desc = sample_desc(400); // claims more than present
        let payload = compressed_payload(&desc, &data);
        assert!(parse_compressed_quicktime(&payload).is_err());
    }

    #[test]
    fn parse_compressed_truncated_fixed_part_errors() {
        let desc = sample_desc(0);
        let payload = compressed_payload(&desc, &[]);
        for cut in [0, 1, 10, 37, 67] {
            assert!(
                parse_compressed_quicktime(&payload[..cut]).is_err(),
                "cut at {cut} should error"
            );
        }
    }

    #[test]
    fn parse_compressed_with_matte_and_mask() {
        let matte_desc = sample_desc(6);
        let matte_data = [0x0Fu8; 6];
        // A raw 10-byte rectangular Region: rgnSize 10, bbox.
        let mut mask = Vec::new();
        mask.extend_from_slice(&10u16.to_be_bytes());
        for v in [0i16, 0, 8, 8] {
            mask.extend_from_slice(&v.to_be_bytes());
        }
        let img_data = [0x77u8; 12];
        let img_desc = sample_desc(img_data.len() as u32);

        let mut p = Vec::new();
        p.extend_from_slice(&1u16.to_be_bytes());
        p.extend_from_slice(&QuickTimeMatrix::IDENTITY.to_wire());
        p.extend_from_slice(&(matte_data.len() as u32).to_be_bytes()); // matteSize
        for v in [0i16, 0, 8, 8] {
            p.extend_from_slice(&v.to_be_bytes()); // matteRect
        }
        p.extend_from_slice(&0u16.to_be_bytes()); // mode
        for v in [0i16, 0, 48, 64] {
            p.extend_from_slice(&v.to_be_bytes()); // srcRect
        }
        p.extend_from_slice(&0u32.to_be_bytes()); // accuracy
        p.extend_from_slice(&(mask.len() as u32).to_be_bytes()); // maskSize
        p.extend_from_slice(&matte_desc.to_bytes());
        p.extend_from_slice(&matte_data);
        p.extend_from_slice(&mask);
        p.extend_from_slice(&img_desc.to_bytes());
        p.extend_from_slice(&img_data);

        let qt = parse_compressed_quicktime(&p).unwrap();
        let matte = qt.matte.expect("matte present");
        assert_eq!(matte.description, matte_desc);
        assert_eq!(matte.data, matte_data);
        assert_eq!(qt.mask_region.as_deref(), Some(mask.as_slice()));
        assert_eq!(qt.matte_rect, RectI32::from_be(0, 0, 8, 8));
        assert_eq!(qt.image_data, img_data);
    }

    #[test]
    fn parse_compressed_undersized_mask_region_errors() {
        // maskSize = 4 < the 10-byte Region header minimum.
        let desc = sample_desc(0);
        let mut p = Vec::new();
        p.extend_from_slice(&1u16.to_be_bytes());
        p.extend_from_slice(&QuickTimeMatrix::IDENTITY.to_wire());
        p.extend_from_slice(&0u32.to_be_bytes());
        p.extend_from_slice(&[0u8; 8]);
        p.extend_from_slice(&0u16.to_be_bytes());
        p.extend_from_slice(&[0u8; 8]);
        p.extend_from_slice(&0u32.to_be_bytes());
        p.extend_from_slice(&4u32.to_be_bytes()); // maskSize = 4
        p.extend_from_slice(&[0u8; 4]);
        p.extend_from_slice(&desc.to_bytes());
        assert!(parse_compressed_quicktime(&p).is_err());
    }

    #[test]
    fn parse_uncompressed_minimal() {
        let sub_data = [0xABu8; 20];
        let mut p = Vec::new();
        p.extend_from_slice(&1u16.to_be_bytes());
        p.extend_from_slice(&QuickTimeMatrix::IDENTITY.to_wire());
        p.extend_from_slice(&0u32.to_be_bytes()); // matteSize
        p.extend_from_slice(&[0u8; 8]); // matteRect
        p.extend_from_slice(&0x009Au16.to_be_bytes()); // DirectBitsRect
        p.extend_from_slice(&sub_data);
        let qt = parse_uncompressed_quicktime(&p).unwrap();
        assert_eq!(qt.version, 1);
        assert!(qt.matte.is_none());
        assert_eq!(qt.subopcode, 0x009A);
        assert!(qt.subopcode_in_range());
        assert_eq!(qt.sub_data, sub_data);
    }

    #[test]
    fn parse_uncompressed_rejects_out_of_range_subopcode() {
        let mut p = Vec::new();
        p.extend_from_slice(&1u16.to_be_bytes());
        p.extend_from_slice(&QuickTimeMatrix::IDENTITY.to_wire());
        p.extend_from_slice(&0u32.to_be_bytes());
        p.extend_from_slice(&[0u8; 8]);
        p.extend_from_slice(&0x0090u16.to_be_bytes()); // BitsRect: not in $98–$9B
        assert!(parse_uncompressed_quicktime(&p).is_err());
    }

    #[test]
    fn matrix_identity_accepts_fract_third_column() {
        let mut m = QuickTimeMatrix::IDENTITY;
        assert!(m.is_identity());
        m.0[8] = Fixed(0x4000_0000); // Fract 1.0
        assert!(m.is_identity());
        m.0[8] = Fixed(0x0002_0000); // 2.0 — not identity
        assert!(!m.is_identity());
        m.0[8] = Fixed(0x0001_0000);
        m.0[1] = Fixed(1);
        assert!(!m.is_identity());
    }

    #[test]
    fn matrix_transform_point_uses_row_vector_equations() {
        // x' = a·x + c·y + tx ; y' = b·x + d·y + ty (book page 2-26).
        let mut m = QuickTimeMatrix::IDENTITY_FRACT;
        m.0[0] = Fixed(0x0002_0000); // a = 2
        m.0[1] = Fixed(0x0000_8000); // b = 0.5
        m.0[3] = Fixed(0x0001_0000); // c = 1
        m.0[4] = Fixed(0x0003_0000); // d = 3
        m.0[6] = Fixed(0x000A_0000); // tx = 10
        m.0[7] = Fixed(-0x0001_0000); // ty = −1
        assert_eq!(m.transform_point(4, 2), (2 * 4 + 2 + 10, 2 + 6 - 1));
        assert!(!m.is_scale_translate());
        assert!(m.is_affine());
        // Half-up rounding of the 16.16 result: 0.5·3 = 1.5 → 2.
        let mut h = QuickTimeMatrix::IDENTITY_FRACT;
        h.0[0] = Fixed(0x0000_8000);
        assert_eq!(h.transform_point(3, 0), (2, 0));
        assert_eq!(h.transform_point(-3, 0), (-1, 0));
    }

    #[test]
    fn rect_matrix_round_trips_through_transform_rect() {
        let src = RectI32::from_be(0, 0, 48, 64);
        let dst = RectI32::from_be(10, 20, 106, 148);
        let m = QuickTimeMatrix::rect_matrix(src, dst).unwrap();
        assert!(m.is_scale_translate());
        assert_eq!(m.transform_rect(src), dst);
        assert_eq!(m.transform_rect_f64(src), Some(dst));
        // Inverse map of a destination pixel centre lands inside src.
        let (sx, sy) = m.inverse_map(20.5, 10.5).unwrap();
        assert!(
            (0.0..1.0).contains(&sx) && (0.0..1.0).contains(&sy),
            "{sx},{sy}"
        );
        assert!(QuickTimeMatrix::rect_matrix(RectI32::default(), dst).is_none());
    }

    #[test]
    fn negative_scale_normalises_and_perspective_is_not_affine() {
        let flip = QuickTimeMatrix::scale_translate(
            Fixed(-0x0001_0000),
            Fixed(0x0001_0000),
            Fixed(0x0008_0000),
            Fixed(0),
        );
        let r = flip.transform_rect(RectI32::from_be(0, 0, 4, 8));
        assert_eq!(r, RectI32::from_be(0, 0, 4, 8));
        let mut p = QuickTimeMatrix::IDENTITY_FRACT;
        p.0[2] = Fixed(1 << 20); // u ≠ 0
        assert!(!p.is_affine());
        assert!(p.inverse_map(1.0, 1.0).is_some());
        let mut singular = QuickTimeMatrix::IDENTITY_FRACT;
        singular.0[0] = Fixed(0);
        assert!(singular.inverse_map(1.0, 1.0).is_none());
        assert_eq!(QuickTimeMatrix::IDENTITY_FRACT.to_f64()[2][2], 1.0);
        assert_eq!(QuickTimeMatrix::IDENTITY.to_f64()[2][2], 1.0);
    }

    #[test]
    fn matrix_round_trips_and_indexes() {
        let mut m = QuickTimeMatrix::IDENTITY;
        m.0[2] = Fixed(0x0005_8000); // 5.5 translation-ish slot
        let wire = m.to_wire();
        let mut r = Reader::new(&wire);
        let back = QuickTimeMatrix::parse(&mut r).unwrap();
        assert_eq!(back, m);
        assert_eq!(back.at(0, 2), Fixed(0x0005_8000));
        assert_eq!(back.at(1, 1), Fixed(0x0001_0000));
    }
}
