//! v2 / extended-v2 `HeaderOp` (`0x0C00`) payload parser + emitter.
//!
//! Inside Macintosh: Imaging With QuickDraw §A-3 ("Version and Header
//! Opcodes", book page A-3) lays out the structure that follows the
//! 2-byte `VersionOp` (`0x0011`) + 2-byte `Version` (`0x02FF`) + 2-byte
//! `HeaderOp` (`0x0C00`) prefix:
//!
//! > "The next 24 bytes contain header information. The value of the
//! > 2-byte version opcode that follows the `HeaderOp` opcode indicates
//! > whether the picture is an extended version 2 picture or a version 2
//! > picture: the `Version` opcode has a value of –2 for an extended
//! > version 2 picture and a value of –1 for a version 2 picture. The
//! > rest of the header for an extended version 2 picture contains
//! > resolution information; the rest of the header for a version 2
//! > picture specifies a fixed-point bounding box."
//!
//! The §A-22 sample listings make the two on-disk layouts explicit:
//!
//! **Extended version 2** (Listing A-5, book page A-23):
//!
//! ```text
//! $"FFFE"                      — version; always -2 for extended v2
//! $"0000"                      — reserved (word)
//! $"0048 0000"                 — best horizontal resolution: 72 dpi
//! $"0048 0000"                 — best vertical resolution: 72 dpi
//! $"0002 0002 006E 00AA"       — optimal source rectangle (Rect i16×4)
//! $"0000"                      — reserved (word)
//! ```
//!
//! Total: 2 + 2 + 4 + 4 + 8 + 4 = 24 bytes (the last `$"0000"` is a
//! 4-byte reserved long in the listing comment "reserved", contributing
//! the remaining 4 bytes to hit 24).
//!
//! **Version 2** (Listing A-6, book page A-24):
//!
//! ```text
//! $"FFFF FFFF"                 — version; always -1 (long) for v2
//! $"0002 0000 0002 0000 00AA 0000 006E 0000"
//!                              — fixed-point bounding rectangle
//!                                (Fixed×4 = 16 bytes)
//! $"0000 0000"                 — reserved (long)
//! ```
//!
//! Total: 4 + 16 + 4 = 24 bytes.
//!
//! The two layouts are disambiguated by the first 16 bits of the
//! payload: `0xFFFE` = extended v2, `0xFFFF` = v2. (The "reserved"
//! second word of the extended-v2 layout is 0x0000, so the first
//! 16 bits unambiguously identify the variant — `0xFFFF` cannot collide
//! with extended-v2 because the spec pins extended-v2's first word to
//! `0xFFFE`.)
//!
//! Note: the §A-3 quote calls the v2 version "-1 (long)" — the listing
//! confirms it's two big-endian words `FFFF FFFF`, which is what we
//! parse here. Read as a u32 it is `0xFFFFFFFF`; the high half alone
//! (`0xFFFF`) is enough to identify the variant.

use crate::error::{PictError, Result};
use crate::reader::Reader;
use crate::state::RectI32;

/// QuickDraw `Fixed` — signed 16.16 fixed-point number (Inside Macintosh:
/// Imaging With QuickDraw §A-4 Table A-1 lists `Fixed` as 4 bytes; the
/// surrounding "Color QuickDraw" chapter documents the 16-bit integer +
/// 16-bit fraction split).
///
/// The on-disk form is a big-endian `i32`. `0x00480000` is the canonical
/// 72.0 dpi value used by the §A-22 sample listings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fixed(pub i32);

impl Fixed {
    /// Construct a `Fixed` from an integer dpi value (e.g. `72` ->
    /// `0x00480000`).
    pub const fn from_integer(n: i16) -> Self {
        Self((n as i32) << 16)
    }

    /// 72.0 dpi — the QuickDraw default and the value used in
    /// Listing A-5.
    pub const SEVENTY_TWO_DPI: Self = Self(0x0048_0000);

    /// Convert to `f32`. Exact for `|integer part| <= 2^23`.
    pub fn to_f32(self) -> f32 {
        (self.0 as f32) / 65536.0
    }

    /// Integer part (the high 16 bits).
    pub fn integer_part(self) -> i16 {
        (self.0 >> 16) as i16
    }

    /// Raw on-disk u32 (sign bit preserved via two's complement).
    pub fn as_u32(self) -> u32 {
        self.0 as u32
    }
}

/// Decoded form of the 24-byte payload that follows the v2 `HeaderOp`
/// (`0x0C00`) opcode.
///
/// Both variants are encountered in real-world PICTs: `OpenCPicture`
/// emits extended v2 (with explicit resolution and optimal source
/// rect), while `OpenPicture` in a colour graphics port emits v2 (with
/// a fixed-point bounding box). The on-disk size is the same 24 bytes
/// in either case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PictHeader {
    /// Extended version 2 header (Listing A-5, `version=-2`).
    ExtendedV2 {
        /// Best horizontal resolution (Fixed, dpi).
        hres: Fixed,
        /// Best vertical resolution (Fixed, dpi).
        vres: Fixed,
        /// Optimal source rectangle for the resolution above
        /// (`top, left, bottom, right` as i16 each).
        optimal_source_rect: RectI32,
    },
    /// Version 2 header (Listing A-6, `version=-1`).
    V2 {
        /// Fixed-point bounding rectangle:
        /// `[top, left, bottom, right]`. Each is a 4-byte Fixed in
        /// the same order the listing prints them.
        fixed_bounds: [Fixed; 4],
    },
}

impl PictHeader {
    /// Wire-format size of the payload (excluding the 2-byte HeaderOp
    /// opcode word itself). Always 24 per §A-3.
    pub const WIRE_SIZE: usize = 24;

    /// Default extended v2 header at 72.0 dpi whose optimal source
    /// rectangle equals `picFrame`. Matches `OpenCPicture` with
    /// `version=-2`, `reserved1=0`, `reserved2=0`, and
    /// `hRes=vRes=$00480000` per the Listing A-4 Pascal example.
    pub fn extended_v2_72dpi(pic_frame: RectI32) -> Self {
        Self::ExtendedV2 {
            hres: Fixed::SEVENTY_TWO_DPI,
            vres: Fixed::SEVENTY_TWO_DPI,
            optimal_source_rect: pic_frame,
        }
    }

    /// Default v2 header whose fixed-point bounding rectangle equals
    /// `picFrame` (each i16 promoted to Fixed via `Fixed::from_integer`).
    pub fn v2_from_pic_frame(pic_frame: RectI32) -> Self {
        Self::V2 {
            fixed_bounds: [
                Fixed::from_integer(pic_frame.top as i16),
                Fixed::from_integer(pic_frame.left as i16),
                Fixed::from_integer(pic_frame.bottom as i16),
                Fixed::from_integer(pic_frame.right as i16),
            ],
        }
    }

    /// Parse the 24-byte payload immediately after the `0x0C00`
    /// `HeaderOp` opcode word. Advances the reader by exactly 24 bytes.
    ///
    /// `Err` only when the reader is truncated. Unknown leading version
    /// words (anything that isn't `0xFFFE` or `0xFFFF`) are flagged as
    /// `InvalidData` per the §A-3 contract — the spec pins the value
    /// to one of those two.
    pub fn parse(r: &mut Reader<'_>) -> Result<Self> {
        let version_word = r.read_u16()?;
        match version_word {
            0xFFFE => {
                // Extended v2: 2-byte reserved + 4-byte hRes + 4-byte
                // vRes + 8-byte optimal-source-rect + 4-byte reserved.
                let _reserved1 = r.read_u16()?;
                let hres = Fixed(r.read_i32()?);
                let vres = Fixed(r.read_i32()?);
                let (top, left, bottom, right) = r.read_rect()?;
                let _reserved2 = r.read_u32()?;
                Ok(Self::ExtendedV2 {
                    hres,
                    vres,
                    optimal_source_rect: RectI32::from_be(top, left, bottom, right),
                })
            }
            0xFFFF => {
                // V2: low 16 bits of -1 (long) + 4 × 4-byte Fixed
                // bounds + 4-byte reserved.
                let _version_low = r.read_u16()?;
                let fixed_bounds = [
                    Fixed(r.read_i32()?),
                    Fixed(r.read_i32()?),
                    Fixed(r.read_i32()?),
                    Fixed(r.read_i32()?),
                ];
                let _reserved = r.read_u32()?;
                Ok(Self::V2 { fixed_bounds })
            }
            other => Err(PictError::invalid(format!(
                "unrecognised v2 header version word 0x{other:04X} \
                 (expected 0xFFFE extended-v2 or 0xFFFF v2 per §A-3)"
            ))),
        }
    }

    /// Serialise into 24 big-endian bytes, ready to follow the 2-byte
    /// `0x0C00` `HeaderOp` opcode.
    pub fn to_wire(&self) -> [u8; 24] {
        let mut out = [0u8; 24];
        match *self {
            Self::ExtendedV2 {
                hres,
                vres,
                optimal_source_rect,
            } => {
                out[0..2].copy_from_slice(&0xFFFE_u16.to_be_bytes());
                // reserved1 = 0 (bytes 2..4 already zero).
                out[4..8].copy_from_slice(&hres.as_u32().to_be_bytes());
                out[8..12].copy_from_slice(&vres.as_u32().to_be_bytes());
                out[12..14].copy_from_slice(&(optimal_source_rect.top as i16).to_be_bytes());
                out[14..16].copy_from_slice(&(optimal_source_rect.left as i16).to_be_bytes());
                out[16..18].copy_from_slice(&(optimal_source_rect.bottom as i16).to_be_bytes());
                out[18..20].copy_from_slice(&(optimal_source_rect.right as i16).to_be_bytes());
                // reserved2 = 0 (bytes 20..24 already zero).
            }
            Self::V2 { fixed_bounds } => {
                out[0..4].copy_from_slice(&0xFFFFFFFF_u32.to_be_bytes());
                out[4..8].copy_from_slice(&fixed_bounds[0].as_u32().to_be_bytes());
                out[8..12].copy_from_slice(&fixed_bounds[1].as_u32().to_be_bytes());
                out[12..16].copy_from_slice(&fixed_bounds[2].as_u32().to_be_bytes());
                out[16..20].copy_from_slice(&fixed_bounds[3].as_u32().to_be_bytes());
                // reserved long = 0 (bytes 20..24 already zero).
            }
        }
        out
    }

    /// `true` for the extended-v2 variant (`version=-2`).
    pub fn is_extended(&self) -> bool {
        matches!(self, Self::ExtendedV2 { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_72_dpi_roundtrip() {
        let f = Fixed::SEVENTY_TWO_DPI;
        assert_eq!(f.as_u32(), 0x0048_0000);
        assert_eq!(f.integer_part(), 72);
        assert!((f.to_f32() - 72.0).abs() < 1e-6);
        // `Fixed::from_integer(72)` matches the literal.
        assert_eq!(Fixed::from_integer(72), Fixed::SEVENTY_TWO_DPI);
    }

    #[test]
    fn fixed_negative_roundtrip() {
        // Sign-preservation on the 16.16 split — Inside Macintosh
        // §A-4 lists Fixed as a signed quantity.
        let f = Fixed::from_integer(-1);
        assert_eq!(f.0, -(1 << 16));
        assert_eq!(f.integer_part(), -1);
        assert!((f.to_f32() + 1.0).abs() < 1e-6);
    }

    #[test]
    fn parse_extended_v2_listing_a5() {
        // Bytes lifted verbatim from Listing A-5 (book page A-23) —
        // the 24-byte HeaderOp payload only.
        let payload: [u8; 24] = [
            0xFF, 0xFE, // version = -2
            0x00, 0x00, // reserved
            0x00, 0x48, 0x00, 0x00, // hRes = 72 dpi (Fixed)
            0x00, 0x48, 0x00, 0x00, // vRes = 72 dpi (Fixed)
            0x00, 0x02, 0x00, 0x02, 0x00, 0x6E, 0x00, 0xAA, // optimal source rect
            0x00, 0x00, 0x00, 0x00, // reserved
        ];
        let mut r = Reader::new(&payload);
        let h = PictHeader::parse(&mut r).unwrap();
        match h {
            PictHeader::ExtendedV2 {
                hres,
                vres,
                optimal_source_rect,
            } => {
                assert_eq!(hres, Fixed::SEVENTY_TWO_DPI);
                assert_eq!(vres, Fixed::SEVENTY_TWO_DPI);
                assert_eq!(optimal_source_rect.top, 2);
                assert_eq!(optimal_source_rect.left, 2);
                assert_eq!(optimal_source_rect.bottom, 0x6E);
                assert_eq!(optimal_source_rect.right, 0xAA);
            }
            other => panic!("expected ExtendedV2, got {other:?}"),
        }
        assert_eq!(r.pos, 24);
    }

    #[test]
    fn parse_v2_listing_a6() {
        // Bytes lifted verbatim from Listing A-6 (book page A-24).
        let payload: [u8; 24] = [
            0xFF, 0xFF, 0xFF, 0xFF, // version = -1 (long)
            0x00, 0x02, 0x00, 0x00, // top   = 2.0 (Fixed)
            0x00, 0x02, 0x00, 0x00, // left  = 2.0
            0x00, 0xAA, 0x00, 0x00, // bottom (listing prints in (top,left,bottom,right)
            0x00, 0x6E, 0x00, 0x00, // right  order; values from book line)
            0x00, 0x00, 0x00, 0x00, // reserved
        ];
        let mut r = Reader::new(&payload);
        let h = PictHeader::parse(&mut r).unwrap();
        match h {
            PictHeader::V2 { fixed_bounds } => {
                assert_eq!(fixed_bounds[0].integer_part(), 2);
                assert_eq!(fixed_bounds[1].integer_part(), 2);
                assert_eq!(fixed_bounds[2].integer_part(), 0xAA);
                assert_eq!(fixed_bounds[3].integer_part(), 0x6E);
            }
            other => panic!("expected V2, got {other:?}"),
        }
        assert_eq!(r.pos, 24);
    }

    #[test]
    fn unknown_version_is_invalid() {
        // Anything that isn't FFFE / FFFF is a §A-3 violation.
        let payload: [u8; 24] = [
            0x12, 0x34, // bogus version
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        let mut r = Reader::new(&payload);
        let err = PictHeader::parse(&mut r).unwrap_err();
        let msg = format!("{err:?}");
        assert!(msg.contains("0x1234"), "got: {msg}");
    }

    #[test]
    fn truncated_payload_is_error() {
        // 23 bytes — one short of the §A-3 24-byte payload.
        let payload = [
            0xFF, 0xFE, 0, 0, 0, 0x48, 0, 0, 0, 0x48, 0, 0, 0, 2, 0, 2, 0, 0x6E, 0, 0xAA, 0, 0, 0,
        ];
        let mut r = Reader::new(&payload);
        assert!(PictHeader::parse(&mut r).is_err());
    }

    #[test]
    fn roundtrip_extended_v2_default() {
        let pic_frame = RectI32::from_be(0, 0, 0x6C, 0xA8);
        let h = PictHeader::extended_v2_72dpi(pic_frame);
        let wire = h.to_wire();
        let mut r = Reader::new(&wire);
        let parsed = PictHeader::parse(&mut r).unwrap();
        assert_eq!(parsed, h);
        assert_eq!(r.pos, 24);
        // Sanity: first two bytes are the FFFE version marker.
        assert_eq!(&wire[0..2], &[0xFF, 0xFE]);
    }

    #[test]
    fn roundtrip_v2_from_pic_frame() {
        let pic_frame = RectI32::from_be(2, 2, 0x6E, 0xAA);
        let h = PictHeader::v2_from_pic_frame(pic_frame);
        let wire = h.to_wire();
        let mut r = Reader::new(&wire);
        let parsed = PictHeader::parse(&mut r).unwrap();
        assert_eq!(parsed, h);
        // Sanity: first four bytes are the -1 long.
        assert_eq!(&wire[0..4], &[0xFF, 0xFF, 0xFF, 0xFF]);
    }

    #[test]
    fn is_extended_flag() {
        let pf = RectI32::from_be(0, 0, 100, 100);
        assert!(PictHeader::extended_v2_72dpi(pf).is_extended());
        assert!(!PictHeader::v2_from_pic_frame(pf).is_extended());
    }
}
