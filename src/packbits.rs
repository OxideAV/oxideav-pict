//! PackBits RLE codec.
//!
//! PackBits is the run-length scheme used by QuickDraw's `PackBitsRect`
//! opcode for both 1-bit BitMap rows and packType=0 PixMap rows. It's
//! the same algorithm as TIFF PackBits / IFF ByteRun1, documented in
//! Inside Macintosh: Imaging With QuickDraw §A-5 ("The PackBits
//! procedure"):
//!
//! Each packet starts with a flag byte `n` (interpreted as `i8`):
//!
//! * `n` in `0..=127`  -> raw packet: copy the next `n + 1` bytes
//!   verbatim.
//! * `n` in `-127..=-1` -> run packet: replicate the next byte
//!   `1 - n` times (so `n = -1` -> 2 copies, `n = -127` -> 128 copies).
//!   Equivalently `(257 - n) as u8` copies when `n` is read as `u8`.
//! * `n == -128` (`0x80`) -> NOP. Some real-world streams use this as
//!   padding.
//!
//! Decoder consumes exactly enough source bytes to produce
//! `expected_len` output bytes. `decode_into` writes into a caller-
//! supplied row buffer to avoid an allocation per scanline; the
//! `Reader` cursor is advanced past the consumed packets.

use crate::error::{PictError, Result};
use crate::reader::Reader;

/// Decode one PackBits-compressed run into `out` (which MUST already be
/// sized to `expected_len` bytes — the function fills it from index 0).
///
/// `src` is advanced past the packets consumed. Returns
/// `InvalidData(...)` if a packet would write past `expected_len`,
/// or if `src` runs out of bytes mid-packet.
pub fn decode_into(src: &mut Reader<'_>, out: &mut [u8]) -> Result<()> {
    let expected_len = out.len();
    let mut written = 0usize;
    while written < expected_len {
        let flag = src.read_u8()?;
        // Re-interpret as i8 to make the case split obvious.
        let flag_i = flag as i8;
        if flag_i >= 0 {
            // Raw packet: copy next (flag_i + 1) bytes verbatim.
            let n = flag_i as usize + 1;
            if written + n > expected_len {
                return Err(PictError::invalid(format!(
                    "PackBits raw packet overruns row: {written} + {n} > {expected_len}"
                )));
            }
            let bytes = src.read_bytes(n)?;
            out[written..written + n].copy_from_slice(bytes);
            written += n;
        } else if flag_i == -128 {
            // NOP — Inside Macintosh §A-5 explicitly reserves -128 as
            // unused; do nothing.
            continue;
        } else {
            // Run packet: replicate next byte (1 - flag_i) times.
            let n = (1 - flag_i as i32) as usize;
            if written + n > expected_len {
                return Err(PictError::invalid(format!(
                    "PackBits run packet overruns row: {written} + {n} > {expected_len}"
                )));
            }
            let v = src.read_u8()?;
            for slot in out.iter_mut().skip(written).take(n) {
                *slot = v;
            }
            written += n;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_only() {
        // flag=2 -> 3 raw bytes: [0xAA, 0xBB, 0xCC]. Then flag=0 -> 1
        // raw byte: [0xDD]. Total 4 bytes out.
        let src = [0x02, 0xAA, 0xBB, 0xCC, 0x00, 0xDD];
        let mut r = Reader::new(&src);
        let mut out = [0u8; 4];
        decode_into(&mut r, &mut out).unwrap();
        assert_eq!(out, [0xAA, 0xBB, 0xCC, 0xDD]);
        assert_eq!(r.pos, src.len());
    }

    #[test]
    fn run_only() {
        // flag=-3 (0xFD) -> 4 copies of 0x55. Out = [0x55; 4].
        let src = [0xFD, 0x55];
        let mut r = Reader::new(&src);
        let mut out = [0u8; 4];
        decode_into(&mut r, &mut out).unwrap();
        assert_eq!(out, [0x55, 0x55, 0x55, 0x55]);
    }

    #[test]
    fn nop_skipped() {
        // NOP byte at start, then raw packet of 2 bytes.
        let src = [0x80, 0x01, 0xAA, 0xBB];
        let mut r = Reader::new(&src);
        let mut out = [0u8; 2];
        decode_into(&mut r, &mut out).unwrap();
        assert_eq!(out, [0xAA, 0xBB]);
    }

    #[test]
    fn mixed() {
        // Decoder of TIFF reference example: [0xFE 0xAA 0x02 0x80 0x00
        // 0x2A 0xFD 0xAA 0x03 0x80 0x00 0x2A 0x22 0xF7 0xAA] should
        // decode to [0xAA;3] + 0x80 0x00 0x2A + [0xAA;4] + 0x80 0x00
        // 0x2A 0x22 + [0xAA;10] = 24 bytes.
        let src = [
            0xFE, 0xAA, 0x02, 0x80, 0x00, 0x2A, 0xFD, 0xAA, 0x03, 0x80, 0x00, 0x2A, 0x22, 0xF7,
            0xAA,
        ];
        let mut r = Reader::new(&src);
        let mut out = [0u8; 24];
        decode_into(&mut r, &mut out).unwrap();
        let want: [u8; 24] = [
            0xAA, 0xAA, 0xAA, 0x80, 0x00, 0x2A, 0xAA, 0xAA, 0xAA, 0xAA, 0x80, 0x00, 0x2A, 0x22,
            0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA,
        ];
        assert_eq!(out, want);
    }

    #[test]
    fn overrun_detected() {
        let src = [0x02, 0xAA, 0xBB, 0xCC]; // claims 3 bytes, dest is 2
        let mut r = Reader::new(&src);
        let mut out = [0u8; 2];
        assert!(decode_into(&mut r, &mut out).is_err());
    }
}

/// Encode `src` bytes into a PackBits-compressed buffer.
///
/// Each row must be encoded independently; the caller is responsible
/// for slicing `src` to exactly one scanline before calling this.
///
/// Uses a greedy one-pass strategy: at each position look ahead for a
/// run of ≥ 3 identical bytes (capped at 128); if found emit a run
/// packet, otherwise collect a raw literal packet (up to 128 bytes,
/// stopping just before any ≥ 3-byte run).
pub fn encode(src: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(src.len() + src.len() / 64);
    let mut i = 0usize;
    while i < src.len() {
        // Detect a run of >= 3 identical bytes starting at i (cap 128).
        let mut run = 1usize;
        while run < 128 && i + run < src.len() && src[i + run] == src[i] {
            run += 1;
        }
        if run >= 3 {
            // Emit run packet: flag = 1 - run as i8, then byte.
            out.push((1i32 - run as i32) as i8 as u8);
            out.push(src[i]);
            i += run;
            continue;
        }
        // No usable run -> raw packet. Find the longest run of bytes
        // that don't start a >=3 run themselves, capped at 128.
        let mut raw = 1usize;
        while raw < 128 && i + raw < src.len() {
            // Check if a 3-run starts at i+raw — if so, end the raw
            // packet here so the run-detector picks it up next iter.
            let s = i + raw;
            if s + 2 < src.len() && src[s] == src[s + 1] && src[s + 1] == src[s + 2] {
                break;
            }
            raw += 1;
        }
        // raw is in 1..=128; the flag byte is `raw - 1` (0..=127).
        out.push((raw - 1) as u8);
        out.extend_from_slice(&src[i..i + raw]);
        i += raw;
    }
    out
}

#[cfg(test)]
mod encode_tests {
    use super::*;

    #[test]
    fn roundtrip_random() {
        // Mix of runs and unique bytes.
        let src: Vec<u8> = (0..200u32)
            .map(|i| if i % 7 == 0 { 0x42 } else { i as u8 })
            .collect();
        let encoded = encode(&src);
        let mut r = Reader::new(&encoded);
        let mut out = vec![0u8; src.len()];
        decode_into(&mut r, &mut out).unwrap();
        assert_eq!(out, src);
    }

    #[test]
    fn roundtrip_all_zeros() {
        let src = vec![0u8; 500];
        let encoded = encode(&src);
        let mut r = Reader::new(&encoded);
        let mut out = vec![0u8; src.len()];
        decode_into(&mut r, &mut out).unwrap();
        assert_eq!(out, src);
    }

    #[test]
    fn roundtrip_no_runs() {
        let src: Vec<u8> = (0..200u32).map(|i| ((i * 31) ^ (i >> 1)) as u8).collect();
        let encoded = encode(&src);
        let mut r = Reader::new(&encoded);
        let mut out = vec![0u8; src.len()];
        decode_into(&mut r, &mut out).unwrap();
        assert_eq!(out, src);
    }
}
