//! Number encodings. Every parameter byte has bit 7 clear (a set bit 7 starts a command), so numbers are packed
//! 7 bits per byte, most significant group first:
//!
//! - **14-bit** (2 bytes): relative coordinates (signed, two's complement, -8192..=8191 µm), power (unsigned,
//!   `0x3FFF` = 100 %), memory addresses, file numbers, the repeat tables.
//! - **35-bit** (5 bytes): absolute coordinates (µm), speeds (µm/s), times (µs), frequencies, colours, memory values.
//!   Signed values are 35-bit two's complement.
//!
//! Absolute coordinates are written as sign-extended 35-bit values and read as the low 32 bits taken as signed, as
//! MeerK40t does (`abscoord` = `decode32`), so both sign-extended and zero-extended negative values read back the same.
//!
//! Encoders refuse values they cannot represent instead of clamping or wrapping.

// Facts from meerk40t/meerk40t @ d511944 `meerk40t/ruida/rdjob.py` (`encode14`, `encode32`, `decode14`, `decode35`,
// `decode32`, `encode_power`, `parse_power`, `encode_speed`, `encode_time`, `signed14`, `signed35`), re-written in Rust
// (MIT License, Copyright (c) 2021 meerk40t; full notice in `swizzle.rs`).

use serde::{Deserialize, Serialize};

use crate::{Result, RuidaError};

pub const U14_MAX: u16 = 0x3FFF;
pub const I14_MIN: i16 = -0x2000;
pub const I14_MAX: i16 = 0x1FFF;
pub const U35_MAX: u64 = (1 << 35) - 1;
pub const I35_MIN: i64 = -(1 << 34);
pub const I35_MAX: i64 = (1 << 34) - 1;

/// The low 7 bits of `v` as a byte.
fn low7(v: u64) -> u8 {
    (v & 0x7F).to_le_bytes()[0]
}

fn check7(bytes: &[u8]) -> Result<()> {
    match bytes.iter().find(|&&b| b >= 0x80) {
        Some(&b) => Err(RuidaError::NotSevenBit(b)),
        None => Ok(()),
    }
}

pub fn encode_u14(v: u16) -> Result<[u8; 2]> {
    if v > U14_MAX {
        return Err(RuidaError::OutOfRange { what: "14-bit unsigned value", value: i64::from(v), min: 0, max: i64::from(U14_MAX) });
    }
    let v = u64::from(v);
    Ok([low7(v >> 7), low7(v)])
}

pub fn decode_u14(b: [u8; 2]) -> Result<u16> {
    check7(&b)?;
    Ok((u16::from(b[0]) << 7) | u16::from(b[1]))
}

pub fn encode_i14(v: i16) -> Result<[u8; 2]> {
    if !(I14_MIN..=I14_MAX).contains(&v) {
        return Err(RuidaError::OutOfRange {
            what: "14-bit signed value",
            value: i64::from(v),
            min: i64::from(I14_MIN),
            max: i64::from(I14_MAX),
        });
    }
    encode_u14(v.cast_unsigned() & U14_MAX)
}

pub fn decode_i14(b: [u8; 2]) -> Result<i16> {
    let u = decode_u14(b)?;
    Ok(if u & 0x2000 != 0 { (u | 0xC000).cast_signed() } else { u.cast_signed() })
}

pub fn encode_u35(v: u64) -> Result<[u8; 5]> {
    if v > U35_MAX {
        return Err(RuidaError::OutOfRange {
            what: "35-bit unsigned value",
            value: i64::try_from(v).unwrap_or(i64::MAX),
            min: 0,
            max: i64::try_from(U35_MAX).unwrap_or(i64::MAX),
        });
    }
    Ok([low7(v >> 28), low7(v >> 21), low7(v >> 14), low7(v >> 7), low7(v)])
}

pub fn decode_u35(b: [u8; 5]) -> Result<u64> {
    check7(&b)?;
    Ok(b.iter().fold(0u64, |acc, &x| (acc << 7) | u64::from(x)))
}

pub fn encode_i35(v: i64) -> Result<[u8; 5]> {
    if !(I35_MIN..=I35_MAX).contains(&v) {
        return Err(RuidaError::OutOfRange { what: "35-bit signed value", value: v, min: I35_MIN, max: I35_MAX });
    }
    encode_u35(v.cast_unsigned() & U35_MAX)
}

pub fn decode_i35(b: [u8; 5]) -> Result<i64> {
    let u = decode_u35(b)?;
    Ok(if u & (1 << 34) != 0 { (u | !U35_MAX).cast_signed() } else { u.cast_signed() })
}

/// An absolute coordinate in µm (sign-extended 35-bit).
pub fn encode_abs(v: i32) -> Result<[u8; 5]> {
    encode_i35(i64::from(v))
}

/// An absolute coordinate in µm: the low 32 bits as signed (MeerK40t's `abscoord`).
pub fn decode_abs(b: [u8; 5]) -> Result<i32> {
    let u = decode_u35(b)?;
    Ok(u32::try_from(u & 0xFFFF_FFFF).map_or(0, u32::cast_signed))
}

/// A 32-bit unsigned quantity (speed µm/s, time µs, frequency) in a 35-bit field.
pub fn encode_u32(v: u32) -> Result<[u8; 5]> {
    encode_u35(u64::from(v))
}

pub fn decode_u32(b: [u8; 5], what: &'static str) -> Result<u32> {
    let u = decode_u35(b)?;
    u32::try_from(u).map_err(|_| RuidaError::OutOfRange {
        what,
        value: i64::try_from(u).unwrap_or(i64::MAX),
        min: 0,
        max: i64::from(u32::MAX),
    })
}

/// A power setting: 14 bits, `0x3FFF` = 100 %.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize)]
pub struct Power(u16);

impl Power {
    pub const ZERO: Power = Power(0);
    pub const FULL: Power = Power(U14_MAX);

    pub fn from_raw(raw: u16) -> Result<Self> {
        if raw > U14_MAX {
            return Err(RuidaError::OutOfRange { what: "power", value: i64::from(raw), min: 0, max: i64::from(U14_MAX) });
        }
        Ok(Power(raw))
    }

    /// Nearest raw step to `percent` (0..=100); out-of-range or non-finite values are refused.
    pub fn from_percent(percent: f64) -> Result<Self> {
        if !percent.is_finite() {
            return Err(RuidaError::NonFinite { what: "power percent" });
        }
        if !(0.0..=100.0).contains(&percent) {
            return Err(RuidaError::OutOfRangeF { what: "power percent", value: percent, allowed: "0..=100" });
        }
        // In 0..=16383 after the range check, and integral.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let raw = (percent * f64::from(U14_MAX) / 100.0).round() as u16;
        Ok(Power(raw))
    }

    pub fn raw(self) -> u16 {
        self.0
    }

    pub fn percent(self) -> f64 {
        f64::from(self.0) * 100.0 / f64::from(U14_MAX)
    }

    pub fn encode(self) -> [u8; 2] {
        let v = u64::from(self.0);
        [low7(v >> 7), low7(v)]
    }

    pub fn decode(b: [u8; 2]) -> Result<Self> {
        Ok(Power(decode_u14(b)?))
    }
}

/// mm → µm, rounded to the nearest µm; refuses non-finite values and values outside `i32`.
pub fn mm_to_um(mm: f64, what: &'static str) -> Result<i32> {
    if !mm.is_finite() {
        return Err(RuidaError::NonFinite { what });
    }
    let um = (mm * 1000.0).round();
    if um < f64::from(i32::MIN) || um > f64::from(i32::MAX) {
        return Err(RuidaError::OutOfRangeF { what, value: mm, allowed: "about ±2147483 mm" });
    }
    // Exact: um is integral and in range.
    #[allow(clippy::cast_possible_truncation)]
    Ok(um as i32)
}

/// mm/s → µm/s; refuses non-finite, negative and too large values.
pub fn mm_s_to_um_s(mm_s: f64, what: &'static str) -> Result<u32> {
    if !mm_s.is_finite() {
        return Err(RuidaError::NonFinite { what });
    }
    let um = (mm_s * 1000.0).round();
    if um < 0.0 || um > f64::from(u32::MAX) {
        return Err(RuidaError::OutOfRangeF { what, value: mm_s, allowed: "0..=4294967 mm/s" });
    }
    // Exact: um is integral and in range.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Ok(um as u32)
}

pub fn um_to_mm(um: i32) -> f64 {
    f64::from(um) / 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn u14_exhaustive() {
        for v in 0..=U14_MAX {
            let b = encode_u14(v).unwrap();
            assert!(b.iter().all(|&x| x < 0x80));
            assert_eq!(decode_u14(b).unwrap(), v);
        }
        assert!(encode_u14(U14_MAX + 1).is_err());
        assert!(encode_u14(u16::MAX).is_err());
    }

    #[test]
    fn i14_exhaustive() {
        for v in I14_MIN..=I14_MAX {
            let b = encode_i14(v).unwrap();
            assert!(b.iter().all(|&x| x < 0x80));
            assert_eq!(decode_i14(b).unwrap(), v);
        }
        assert_eq!(encode_i14(-1).unwrap(), [0x7F, 0x7F]);
        assert_eq!(encode_i14(I14_MIN).unwrap(), [0x40, 0x00]);
        assert_eq!(encode_i14(I14_MAX).unwrap(), [0x3F, 0x7F]);
        assert!(encode_i14(I14_MAX + 1).is_err());
        assert!(encode_i14(I14_MIN - 1).is_err());
        // Every 2-byte 7-bit pattern decodes to exactly one i14 and re-encodes to itself.
        for hi in 0..0x80u8 {
            for lo in 0..0x80u8 {
                let v = decode_i14([hi, lo]).unwrap();
                assert_eq!(encode_i14(v).unwrap(), [hi, lo]);
            }
        }
    }

    #[test]
    fn decoders_refuse_high_bit() {
        assert_eq!(decode_u14([0x80, 0]), Err(RuidaError::NotSevenBit(0x80)));
        assert!(decode_u35([0, 0, 0, 0, 0xFF]).is_err());
        assert!(Power::decode([0x01, 0x80]).is_err());
    }

    /// Edge values plus a deterministic pseudo-random sweep (an LCG) over the whole 35-bit range.
    fn u35_samples() -> Vec<u64> {
        let mut v: Vec<u64> = vec![0, 1, 0x7F, 0x80, 0x3FFF, 0x4000, U35_MAX, U35_MAX - 1, 1 << 34, (1 << 34) - 1];
        for shift in 0..35 {
            v.push(1 << shift);
            v.push((1 << shift) - 1);
        }
        let mut x: u64 = 0x1234_5678_9ABC_DEF0;
        for _ in 0..200_000 {
            x = x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            v.push(x >> 29);
        }
        v
    }

    #[test]
    fn u35_round_trip() {
        for v in u35_samples() {
            let b = encode_u35(v).unwrap();
            assert!(b.iter().all(|&x| x < 0x80));
            assert_eq!(decode_u35(b).unwrap(), v);
        }
        assert!(encode_u35(U35_MAX + 1).is_err());
        assert_eq!(encode_u35(U35_MAX).unwrap(), [0x7F; 5]);
    }

    #[test]
    fn i35_round_trip() {
        for u in u35_samples() {
            let v = (u.cast_signed()) - (1 << 34); // shift into the signed range
            let b = encode_i35(v).unwrap();
            assert_eq!(decode_i35(b).unwrap(), v);
            // Bytes are the 35-bit two's complement.
            assert_eq!(decode_u35(b).unwrap(), v.cast_unsigned() & U35_MAX);
        }
        assert_eq!(encode_i35(-1).unwrap(), [0x7F; 5]);
        assert_eq!(encode_i35(I35_MIN).unwrap(), [0x40, 0, 0, 0, 0]);
        assert!(encode_i35(I35_MAX + 1).is_err());
        assert!(encode_i35(I35_MIN - 1).is_err());
    }

    #[test]
    fn abs_coords() {
        for v in [0, 1, -1, 1000, -1000, i32::MAX, i32::MIN, 600_000, -8193, 8192] {
            assert_eq!(decode_abs(encode_abs(v).unwrap()).unwrap(), v);
        }
        let mut x: u32 = 7;
        for _ in 0..200_000 {
            x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let v = x.cast_signed();
            assert_eq!(decode_abs(encode_abs(v).unwrap()).unwrap(), v);
        }
        // A zero-extended negative (bits 32..34 clear) reads the same as the sign-extended form.
        let zero_ext = encode_u35(u64::from((-5i32).cast_unsigned())).unwrap();
        assert_eq!(decode_abs(zero_ext).unwrap(), -5);
    }

    #[test]
    fn power_percent() {
        assert_eq!(Power::from_percent(0.0).unwrap(), Power::ZERO);
        assert_eq!(Power::from_percent(100.0).unwrap(), Power::FULL);
        assert_eq!(Power::FULL.encode(), [0x7F, 0x7F]);
        assert!((Power::FULL.percent() - 100.0).abs() < 1e-12);
        assert_eq!(Power::from_percent(50.0).unwrap().raw(), 8192); // 8191.5 rounds up
        for raw in 0..=U14_MAX {
            let p = Power::from_raw(raw).unwrap();
            assert_eq!(Power::from_percent(p.percent()).unwrap(), p);
            assert_eq!(Power::decode(p.encode()).unwrap(), p);
        }
        assert!(Power::from_percent(100.001).is_err());
        assert!(Power::from_percent(-0.1).is_err());
        assert!(Power::from_percent(f64::NAN).is_err());
        assert!(Power::from_raw(U14_MAX + 1).is_err());
    }

    #[test]
    fn unit_conversions() {
        assert_eq!(mm_to_um(1.2345, "x").unwrap(), 1235);
        assert_eq!(mm_to_um(-1.2344, "x").unwrap(), -1234);
        assert_eq!(mm_to_um(2_147_483.647, "x").unwrap(), i32::MAX);
        assert_eq!(mm_to_um(-2_147_483.648, "x").unwrap(), i32::MIN);
        assert!(mm_to_um(2_147_483.648, "x").is_err());
        assert!(mm_to_um(f64::INFINITY, "x").is_err());
        assert_eq!(mm_s_to_um_s(100.0, "s").unwrap(), 100_000);
        assert_eq!(mm_s_to_um_s(4_294_967.295, "s").unwrap(), u32::MAX);
        assert!(mm_s_to_um_s(-1.0, "s").is_err());
        assert!(mm_s_to_um_s(4_294_967.296, "s").is_err());
    }
}
