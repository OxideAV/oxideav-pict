//! Tiny big-endian byte cursor used by the PICT parser.
//!
//! Inside Macintosh: Imaging With QuickDraw §A-3 documents PICT as a
//! big-endian byte stream throughout (Mac native order). We never
//! reach for `byteorder` because the integer widths involved are
//! tiny and explicit `from_be_bytes` is clearer.

use crate::error::{PictError, Result};

/// Stateful reader over a borrowed PICT byte buffer.
///
/// All reads bump `pos` and return `InvalidData("truncated …")` if
/// the underlying slice doesn't have enough bytes left. The cursor
/// supports word alignment (PICT v2 requires 16-bit aligned opcodes
/// when the byte count read for the previous opcode is odd).
pub struct Reader<'a> {
    pub bytes: &'a [u8],
    pub pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    pub fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.pos)
    }

    pub fn at_eof(&self) -> bool {
        self.pos >= self.bytes.len()
    }

    pub fn skip(&mut self, n: usize) -> Result<()> {
        if self.remaining() < n {
            return Err(PictError::invalid(format!(
                "truncated: need {n} more bytes at offset {}",
                self.pos
            )));
        }
        self.pos += n;
        Ok(())
    }

    pub fn read_bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.remaining() < n {
            return Err(PictError::invalid(format!(
                "truncated: need {n} more bytes at offset {}",
                self.pos
            )));
        }
        let s = &self.bytes[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    pub fn read_u8(&mut self) -> Result<u8> {
        if self.remaining() < 1 {
            return Err(PictError::invalid(format!(
                "truncated: need u8 at offset {}",
                self.pos
            )));
        }
        let b = self.bytes[self.pos];
        self.pos += 1;
        Ok(b)
    }

    pub fn read_u16(&mut self) -> Result<u16> {
        let b = self.read_bytes(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }

    pub fn read_i16(&mut self) -> Result<i16> {
        let b = self.read_bytes(2)?;
        Ok(i16::from_be_bytes([b[0], b[1]]))
    }

    pub fn read_u32(&mut self) -> Result<u32> {
        let b = self.read_bytes(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// Big-endian signed 32-bit integer. Used by the v2 `HeaderOp`
    /// payload (`Fixed` resolution / bounding-box values).
    pub fn read_i32(&mut self) -> Result<i32> {
        let b = self.read_bytes(4)?;
        Ok(i32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// Read a QuickDraw `Rect` (8 bytes: top, left, bottom, right as
    /// big-endian i16 each). Returned as `(top, left, bottom, right)`.
    pub fn read_rect(&mut self) -> Result<(i16, i16, i16, i16)> {
        let top = self.read_i16()?;
        let left = self.read_i16()?;
        let bottom = self.read_i16()?;
        let right = self.read_i16()?;
        Ok((top, left, bottom, right))
    }

    /// Pad the cursor to the next even byte if currently odd. v2
    /// opcode boundaries are word-aligned per Inside Macintosh §A-3.
    pub fn align_word(&mut self) -> Result<()> {
        if self.pos % 2 == 0 {
            return Ok(());
        }
        self.skip(1)
    }
}
