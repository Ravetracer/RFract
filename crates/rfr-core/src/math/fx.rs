// SPDX-License-Identifier: LGPL-3.0-or-later

//! Floats with an extended exponent ("floatexp"): an `f64` mantissa and an `i32` power of two,
//! for view sizes and perturbation deltas far below the smallest `f64` (about 1e-308).

use std::cmp::Ordering;
use std::fmt;
use std::ops::{Add, Div, Mul, MulAssign, Neg, Sub};

/// `m · 2^e` with `1 ≤ |m| < 2` (or `m = 0`, `e = 0`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fx {
    pub m: f64,
    pub e: i32,
}

impl Fx {
    pub const ZERO: Fx = Fx { m: 0.0, e: 0 };
    pub const ONE: Fx = Fx { m: 1.0, e: 0 };

    /// Normalizes `m · 2^e`.
    pub fn new(m: f64, e: i32) -> Fx {
        if m == 0.0 || !m.is_finite() {
            return Fx { m: if m.is_finite() { 0.0 } else { m }, e: 0 };
        }
        let bits = m.to_bits();
        let exp = ((bits >> 52) & 0x7ff) as i32;
        if exp == 0 {
            // Subnormal mantissa: scale it up first.
            return Fx::new(m * 2f64.powi(64), e - 64);
        }
        let shift = exp - 1023;
        let mant = f64::from_bits((bits & !(0x7ff << 52)) | (1023u64 << 52));
        Fx { m: mant, e: e.saturating_add(shift) }
    }

    pub fn from_f64(v: f64) -> Fx {
        Fx::new(v, 0)
    }

    /// The value as `f64` (0 or ±inf outside its range).
    pub fn to_f64(self) -> f64 {
        if self.e > 1023 {
            return self.m * f64::INFINITY;
        }
        if self.e < -1074 {
            return 0.0;
        }
        // Two steps keep subnormal results right.
        let half = self.e / 2;
        self.m * 2f64.powi(half) * 2f64.powi(self.e - half)
    }

    pub fn is_zero(self) -> bool {
        self.m == 0.0
    }

    pub fn abs(self) -> Fx {
        Fx { m: self.m.abs(), e: self.e }
    }

    /// log10 of the absolute value (−inf for zero).
    pub fn log10(self) -> f64 {
        if self.m == 0.0 {
            return f64::NEG_INFINITY;
        }
        self.m.abs().log10() + self.e as f64 * std::f64::consts::LOG10_2
    }

    /// log2 of the absolute value (−inf for zero).
    pub fn log2(self) -> f64 {
        if self.m == 0.0 {
            return f64::NEG_INFINITY;
        }
        self.m.abs().log2() + self.e as f64
    }

    pub fn sqrt(self) -> Fx {
        if self.m <= 0.0 {
            return Fx::ZERO;
        }
        if self.e % 2 == 0 { Fx::new(self.m.sqrt(), self.e / 2) } else { Fx::new((self.m * 2.0).sqrt(), (self.e - 1) / 2) }
    }

    /// `10^x` for any real exponent.
    pub fn pow10(x: f64) -> Fx {
        let l2 = x / std::f64::consts::LOG10_2;
        let e = l2.floor();
        Fx::new(2f64.powf(l2 - e), e as i32)
    }

    /// `10^n` by binary exponentiation (a few ulps of error at any size).
    pub fn pow10i(n: i64) -> Fx {
        let mut result = Fx::ONE;
        let mut base = Fx::from_f64(10.0);
        let mut k = n.unsigned_abs();
        while k > 0 {
            if k & 1 == 1 {
                result = result * base;
            }
            base = base * base;
            k >>= 1;
        }
        if n < 0 { Fx::ONE / result } else { result }
    }

    /// Parses `1.5e-1000`, `0.25` and the like.
    pub fn parse(s: &str) -> Option<Fx> {
        let s = s.trim();
        let (mant, exp) = match s.find(['e', 'E']) {
            Some(i) => (&s[..i], s[i + 1..].parse::<i64>().ok()?),
            None => (s, 0),
        };
        let m: f64 = mant.parse().ok()?;
        if m == 0.0 {
            return Some(Fx::ZERO);
        }
        Some(Fx::from_f64(m) * Fx::pow10i(exp))
    }
}

impl From<f64> for Fx {
    fn from(v: f64) -> Fx {
        Fx::from_f64(v)
    }
}

impl Mul for Fx {
    type Output = Fx;
    fn mul(self, o: Fx) -> Fx {
        Fx::new(self.m * o.m, self.e.saturating_add(o.e))
    }
}

impl Mul<f64> for Fx {
    type Output = Fx;
    fn mul(self, o: f64) -> Fx {
        self * Fx::from_f64(o)
    }
}

impl MulAssign<f64> for Fx {
    fn mul_assign(&mut self, o: f64) {
        *self = *self * o;
    }
}

impl Div for Fx {
    type Output = Fx;
    fn div(self, o: Fx) -> Fx {
        Fx::new(self.m / o.m, self.e.saturating_sub(o.e))
    }
}

impl Div<f64> for Fx {
    type Output = Fx;
    fn div(self, o: f64) -> Fx {
        self / Fx::from_f64(o)
    }
}

impl Add for Fx {
    type Output = Fx;
    fn add(self, o: Fx) -> Fx {
        if self.m == 0.0 {
            return o;
        }
        if o.m == 0.0 {
            return self;
        }
        let (big, small) = if self.e >= o.e { (self, o) } else { (o, self) };
        let d = big.e - small.e;
        if d > 60 {
            return big;
        }
        Fx::new(big.m + small.m * 2f64.powi(-d), big.e)
    }
}

impl Neg for Fx {
    type Output = Fx;
    fn neg(self) -> Fx {
        Fx { m: -self.m, e: self.e }
    }
}

impl Sub for Fx {
    type Output = Fx;
    fn sub(self, o: Fx) -> Fx {
        self + -o
    }
}

impl PartialOrd for Fx {
    fn partial_cmp(&self, o: &Fx) -> Option<Ordering> {
        (*self - *o).m.partial_cmp(&0.0)
    }
}

impl fmt::Display for Fx {
    /// Scientific notation with 17 significant digits (`1.5e-1000`).
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if self.m == 0.0 {
            return write!(f, "0");
        }
        if (-1000..1000).contains(&self.e) {
            return write!(f, "{}", self.to_f64());
        }
        let mut e10 = self.abs().log10().floor() as i64;
        let mut mant = (self.abs() / Fx::pow10i(e10)).to_f64();
        if mant >= 10.0 {
            mant /= 10.0;
            e10 += 1;
        } else if mant < 1.0 {
            mant *= 10.0;
            e10 -= 1;
        }
        let sign = if self.m < 0.0 { "-" } else { "" };
        let mant = format!("{mant:.15}");
        let mant = mant.trim_end_matches('0').trim_end_matches('.');
        write!(f, "{sign}{mant}e{e10}")
    }
}

impl serde::Serialize for Fx {
    /// A number while `f64` holds it (documents stay as they were), else a string.
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        if (-1000..1000).contains(&self.e) { s.serialize_f64(self.to_f64()) } else { s.serialize_str(&self.to_string()) }
    }
}

impl<'de> serde::Deserialize<'de> for Fx {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Fx, D::Error> {
        #[derive(serde::Deserialize)]
        #[serde(untagged)]
        enum V {
            N(f64),
            S(String),
        }
        match V::deserialize(d)? {
            V::N(v) => Ok(Fx::from_f64(v)),
            V::S(s) => Fx::parse(&s).ok_or_else(|| serde::de::Error::custom(format!("not a number: {s}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arithmetic_beyond_f64() {
        let a = Fx::parse("1.5e-1000").unwrap();
        assert!((a.log10() - (1.5f64.log10() - 1000.0)).abs() < 1e-12);
        let b = a * a;
        assert!((b.log10() - 2.0 * a.log10()).abs() < 1e-9);
        assert!(((a + a) / a).to_f64() - 2.0 < 1e-15);
        assert_eq!((a - a).to_f64(), 0.0);
        assert!(a > Fx::parse("1e-1001").unwrap());
        assert_eq!(a.to_string(), "1.5e-1000");
        assert_eq!(Fx::from_f64(0.25).to_f64(), 0.25);
        assert_eq!(Fx::from_f64(-3.0).to_string(), "-3");
        assert!((Fx::from_f64(1e-310).to_f64() - 1e-310).abs() < 1e-320);
        let s = serde_json::to_string(&a).unwrap();
        assert_eq!(serde_json::from_str::<Fx>(&s).unwrap().log10(), a.log10());
        assert_eq!(serde_json::to_string(&Fx::from_f64(0.5)).unwrap(), "0.5");
        assert!((a.sqrt().log10() - a.log10() / 2.0).abs() < 1e-12);
    }
}
