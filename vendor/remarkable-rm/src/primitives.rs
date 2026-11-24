//! Slice-based binary reader. All multi-byte values are little-endian per
//! spec §2.
//!
//! [`Reader`] tracks a cursor into a borrowed byte slice. Sub-blocks (spec §3)
//! are read by carving a bounded child reader off the parent with
//! [`Reader::read_bounded`]; the parent advances past the full sub-block
//! length, and the child cannot read beyond its own bounds — over-reads are a
//! type-level impossibility.

use crate::error::ParseError;

#[derive(Debug, Clone)]
pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    #[must_use]
    pub const fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    #[must_use]
    pub const fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    #[must_use]
    pub const fn is_eof(&self) -> bool {
        self.pos >= self.data.len()
    }

    fn require(&self, needed: usize) -> Result<(), ParseError> {
        if self.remaining() < needed {
            return Err(ParseError::Truncated {
                needed,
                got: self.remaining(),
            });
        }
        Ok(())
    }

    pub fn read_bytes(&mut self, n: usize) -> Result<&'a [u8], ParseError> {
        self.require(n)?;
        let bytes = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(bytes)
    }

    fn read_array<const N: usize>(&mut self) -> Result<[u8; N], ParseError> {
        self.require(N)?;
        let pos = self.pos;
        self.pos += N;
        Ok(std::array::from_fn(|i| self.data[pos + i]))
    }

    pub fn read_u8(&mut self) -> Result<u8, ParseError> {
        self.read_array::<1>().map(|[b]| b)
    }

    pub fn read_u16(&mut self) -> Result<u16, ParseError> {
        self.read_array().map(u16::from_le_bytes)
    }

    pub fn read_u32(&mut self) -> Result<u32, ParseError> {
        self.read_array().map(u32::from_le_bytes)
    }

    pub fn read_i32(&mut self) -> Result<i32, ParseError> {
        self.read_array().map(i32::from_le_bytes)
    }

    pub fn read_f32(&mut self) -> Result<f32, ParseError> {
        self.read_array().map(f32::from_le_bytes)
    }

    pub fn read_f64(&mut self) -> Result<f64, ParseError> {
        self.read_array().map(f64::from_le_bytes)
    }

    pub fn read_bool(&mut self) -> Result<bool, ParseError> {
        Ok(self.read_u8()? != 0)
    }

    /// LEB128 unsigned varint, capped at 10 bytes (effectively `u64`). Spec §2.
    pub fn read_varuint(&mut self) -> Result<u64, ParseError> {
        let mut result: u64 = 0;
        for shift in (0..63).step_by(7) {
            let byte = self.read_u8()?;
            result |= u64::from(byte & 0x7F) << shift;
            if byte & 0x80 == 0 {
                return Ok(result);
            }
        }

        let byte = self.read_u8()?;
        if byte & 0x80 != 0 || byte & 0x7F > 1 {
            return Err(ParseError::VarUIntOverflow);
        }
        result |= u64::from(byte & 0x7F) << 63;
        Ok(result)
    }

    /// Take the next `n` bytes as an isolated child reader; the parent
    /// advances past them.
    pub fn read_bounded(&mut self, n: usize) -> Result<Reader<'a>, ParseError> {
        Ok(Reader::new(self.read_bytes(n)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primitives_round_trip() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&[0xAB]);
        bytes.extend_from_slice(&0x1234_u16.to_le_bytes());
        bytes.extend_from_slice(&0x1234_5678_u32.to_le_bytes());
        bytes.extend_from_slice(&(-12345_i32).to_le_bytes());
        bytes.extend_from_slice(&1.5_f32.to_le_bytes());
        bytes.extend_from_slice(&(-2.5_f64).to_le_bytes());
        bytes.extend_from_slice(&[0x00, 0x01]);

        let mut r = Reader::new(&bytes);
        assert_eq!(r.read_u8().unwrap(), 0xAB);
        assert_eq!(r.read_u16().unwrap(), 0x1234);
        assert_eq!(r.read_u32().unwrap(), 0x1234_5678);
        assert_eq!(r.read_i32().unwrap(), -12345);
        assert!((r.read_f32().unwrap() - 1.5).abs() < f32::EPSILON);
        assert!((r.read_f64().unwrap() - -2.5).abs() < f64::EPSILON);
        assert!(!r.read_bool().unwrap());
        assert!(r.read_bool().unwrap());
        assert!(r.is_eof());
    }

    #[test]
    fn varuint_single_byte() {
        let mut r = Reader::new(&[0x00]);
        assert_eq!(r.read_varuint().unwrap(), 0);
        let mut r = Reader::new(&[0x7F]);
        assert_eq!(r.read_varuint().unwrap(), 127);
    }

    #[test]
    fn varuint_multi_byte() {
        // 128 = 0x80 0x01
        let mut r = Reader::new(&[0x80, 0x01]);
        assert_eq!(r.read_varuint().unwrap(), 128);
        // 300 = 0xAC 0x02
        let mut r = Reader::new(&[0xAC, 0x02]);
        assert_eq!(r.read_varuint().unwrap(), 300);
    }

    #[test]
    fn varuint_overflow() {
        // 11 continuation bytes — must error.
        let bytes = [0xFFu8; 11];
        let mut r = Reader::new(&bytes);
        assert!(matches!(r.read_varuint(), Err(ParseError::VarUIntOverflow)));
    }

    #[test]
    fn varuint_tenth_byte_payload_overflow() {
        let bytes = [0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x02];
        let mut r = Reader::new(&bytes);
        assert!(matches!(r.read_varuint(), Err(ParseError::VarUIntOverflow)));
    }

    #[test]
    fn varuint_tenth_byte_must_terminate() {
        let bytes = [0x80; 10];
        let mut r = Reader::new(&bytes);
        assert!(matches!(r.read_varuint(), Err(ParseError::VarUIntOverflow)));
    }

    #[test]
    fn truncation_reports_needed_and_got() {
        let mut r = Reader::new(&[0xAB]);
        let err = r.read_u32().unwrap_err();
        assert!(matches!(err, ParseError::Truncated { needed: 4, got: 1 }));
    }

    #[test]
    fn read_bounded_advances_parent_and_caps_child() {
        let bytes = [0x01, 0x02, 0x03, 0x04, 0x05];
        let mut parent = Reader::new(&bytes);
        let mut child = parent.read_bounded(3).unwrap();
        assert_eq!(parent.remaining(), 2);
        assert_eq!(child.read_u8().unwrap(), 0x01);
        assert_eq!(child.read_u8().unwrap(), 0x02);
        assert_eq!(child.read_u8().unwrap(), 0x03);
        assert!(child.is_eof());
        assert!(child.read_u8().is_err());
        // Parent picks up at byte 4.
        assert_eq!(parent.read_u8().unwrap(), 0x04);
    }
}
