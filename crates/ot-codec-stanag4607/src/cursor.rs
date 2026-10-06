//! A bounds-checked big-endian reader plus the STANAG 4607 numeric forms
//! (Annex B, paragraph 6: I/S integers, B16/B32/H32 signed binary decimals,
//! BA16/BA32 binary angles and SA16/SA32 signed binary angles).
//!
//! Every read is checked; running off the end yields [`Error::Truncated`]
//! with `need` = the absolute offset the read required and `have` = the
//! buffer length.
//!
//! Source: STANAG 4607 Ed. 3 (AEDP-7, 2013), from the NSG Standards
//! Registry, https://nsgreg.nga.mil/doc/view?i=5568 (see this crate's README, "Source").

use crate::Error;

#[derive(Debug, Clone)]
pub(crate) struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    pub(crate) fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    pub(crate) fn remaining(&self) -> usize {
        self.buf.len().saturating_sub(self.pos)
    }

    pub(crate) fn take(&mut self, n: usize) -> Result<&'a [u8], Error> {
        let end = self.pos.checked_add(n).ok_or(Error::Truncated {
            need: usize::MAX,
            have: self.buf.len(),
        })?;
        let s = self.buf.get(self.pos..end).ok_or(Error::Truncated {
            need: end,
            have: self.buf.len(),
        })?;
        self.pos = end;
        Ok(s)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], Error> {
        let s = self.take(N)?;
        let mut a = [0u8; N];
        a.copy_from_slice(s);
        Ok(a)
    }

    /// I8 / E8 / FL8.
    pub(crate) fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.array::<1>()?[0])
    }
    /// I16 / FL16.
    pub(crate) fn u16(&mut self) -> Result<u16, Error> {
        Ok(u16::from_be_bytes(self.array()?))
    }
    /// I32.
    pub(crate) fn u32(&mut self) -> Result<u32, Error> {
        Ok(u32::from_be_bytes(self.array()?))
    }
    /// S8.
    pub(crate) fn i8(&mut self) -> Result<i8, Error> {
        Ok(i8::from_be_bytes(self.array()?))
    }
    /// S16.
    pub(crate) fn i16(&mut self) -> Result<i16, Error> {
        Ok(i16::from_be_bytes(self.array()?))
    }
    /// S32.
    pub(crate) fn i32(&mut self) -> Result<i32, Error> {
        Ok(i32::from_be_bytes(self.array()?))
    }
    /// An existence mask of `n` bytes (FL40 / FL64), returned left-aligned in
    /// a u64 so that bit 63 is the first field after the mask.
    pub(crate) fn mask(&mut self, n: usize) -> Result<u64, Error> {
        let s = self.take(n)?;
        let mut v = 0u64;
        for &b in s {
            v = (v << 8) | u64::from(b);
        }
        Ok(v << (8 * (8 - n.min(8))))
    }
    /// BA16: unsigned binary angle, degrees in [0, 360).
    pub(crate) fn ba16(&mut self) -> Result<f64, Error> {
        Ok(ba16(self.u16()?))
    }
    /// BA32: unsigned binary angle, degrees in [0, 360).
    pub(crate) fn ba32(&mut self) -> Result<f64, Error> {
        Ok(ba32(self.u32()?))
    }
    /// SA16: signed binary angle, degrees in [-90, 90).
    pub(crate) fn sa16(&mut self) -> Result<f64, Error> {
        Ok(sa16(self.i16()?))
    }
    /// SA32: signed binary angle, degrees in [-90, 90).
    pub(crate) fn sa32(&mut self) -> Result<f64, Error> {
        Ok(sa32(self.i32()?))
    }
    /// B16: sign-magnitude, 8-bit integer part, 7-bit fraction.
    pub(crate) fn b16(&mut self) -> Result<f64, Error> {
        Ok(b16(self.u16()?))
    }
    /// B32: sign-magnitude, 8-bit integer part, 23-bit fraction.
    pub(crate) fn b32(&mut self) -> Result<f64, Error> {
        Ok(b32(self.u32()?))
    }
    /// H32: sign-magnitude, 15-bit integer part, 16-bit fraction.
    pub(crate) fn h32(&mut self) -> Result<f64, Error> {
        Ok(h32(self.u32()?))
    }
    /// Alphanumeric (A) field of `n` bytes: BCS/ECS bytes mapped one-to-one to
    /// characters (Latin-1), with trailing spaces and NULs trimmed.
    pub(crate) fn text(&mut self, n: usize) -> Result<String, Error> {
        Ok(text(self.take(n)?))
    }
}

pub(crate) fn text(bytes: &[u8]) -> String {
    let s: String = bytes.iter().map(|&b| char::from(b)).collect();
    s.trim_end_matches([' ', '\0']).to_string()
}

pub(crate) fn ba16(raw: u16) -> f64 {
    f64::from(raw) * 360.0 / 65_536.0
}
pub(crate) fn ba32(raw: u32) -> f64 {
    f64::from(raw) * 360.0 / 4_294_967_296.0
}
pub(crate) fn sa16(raw: i16) -> f64 {
    f64::from(raw) * 180.0 / 65_536.0
}
pub(crate) fn sa32(raw: i32) -> f64 {
    f64::from(raw) * 180.0 / 4_294_967_296.0
}
pub(crate) fn b16(raw: u16) -> f64 {
    let mag = f64::from(raw & 0x7FFF) / 128.0;
    if raw & 0x8000 != 0 { -mag } else { mag }
}
pub(crate) fn b32(raw: u32) -> f64 {
    let mag = f64::from(raw & 0x7FFF_FFFF) / 8_388_608.0;
    if raw & 0x8000_0000 != 0 { -mag } else { mag }
}
pub(crate) fn h32(raw: u32) -> f64 {
    let mag = f64::from(raw & 0x7FFF_FFFF) / 65_536.0;
    if raw & 0x8000_0000 != 0 { -mag } else { mag }
}

/// Normalise a longitude in degrees (BA32 gives [0, 360)) to [-180, 180).
pub(crate) fn norm_lon(deg: f64) -> f64 {
    let mut d = deg % 360.0;
    if d >= 180.0 {
        d -= 360.0;
    } else if d < -180.0 {
        d += 360.0;
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_examples() {
        // Annex B 6.6 example: BA16 0101100100011100 = 125.31006 deg.
        assert!((ba16(0b0101_1001_0001_1100) - 125.31006).abs() < 1e-5);
        // Annex B 6.7 example: -34.873352 deg ~ SA16 1100111001100110. The
        // printed bit pattern is one LSB from the rounded value (it decodes to
        // -34.876099), so compare within 1.5 LSB.
        assert!((sa16(0b1100_1110_0110_0110u16 as i16) + 34.873352).abs() < 1.5 * 180.0 / 65_536.0);
        assert_eq!(b16(0x8080), -1.0);
        assert_eq!(b16(0x0140), 2.5);
        assert_eq!(h32(0x8001_8000), -1.5);
        assert_eq!(norm_lon(350.0), -10.0);
        assert_eq!(norm_lon(180.0), -180.0);
    }

    #[test]
    fn cursor_bounds() {
        let mut c = Cursor::new(&[1, 2, 3]);
        assert_eq!(c.u16().unwrap(), 0x0102);
        assert!(matches!(
            c.u16(),
            Err(Error::Truncated { need: 4, have: 3 })
        ));
        assert_eq!(c.u8().unwrap(), 3);
        assert!(c.take(usize::MAX).is_err());
        let mut m = Cursor::new(&[0x80, 0, 0, 0, 1]);
        assert_eq!(m.mask(5).unwrap(), 0x8000_0000_0100_0000);
    }
}
