// SPDX-License-Identifier: LGPL-3.0-or-later

//! Scalars the formula evaluator runs on: `f64` and [`Dual`] (forward-mode automatic
//! differentiation along one direction). The complex functions in [`super::complex`] are written
//! once over [`Scalar`], so every formula gets its derivative for distance estimation without
//! hand-written rules, and the GPU's WGSL library (`cstd.wgsl`) mirrors the same algorithms.

use std::ops::{Add, Div, Mul, Neg, Sub};

pub trait Scalar:
    Copy + PartialEq + std::fmt::Debug + Add<Output = Self> + Sub<Output = Self> + Mul<Output = Self> + Div<Output = Self> + Neg<Output = Self>
{
    fn from_f64(v: f64) -> Self;
    /// The value without derivative information (comparisons, branches, finiteness).
    fn v(self) -> f64;
    /// The derivative (0 for plain numbers).
    fn d(self) -> f64 {
        0.0
    }
    /// A value with derivative `d` (dropped by plain numbers).
    fn with_d(v: f64, d: f64) -> Self {
        let _ = d;
        Self::from_f64(v)
    }
    fn zero() -> Self {
        Self::from_f64(0.0)
    }
    fn one() -> Self {
        Self::from_f64(1.0)
    }
    fn abs(self) -> Self;
    fn sqrt(self) -> Self;
    fn exp(self) -> Self;
    fn ln(self) -> Self;
    fn sin(self) -> Self;
    fn cos(self) -> Self;
    fn sinh(self) -> Self;
    fn cosh(self) -> Self;
    fn atan2(self, x: Self) -> Self;
    /// Rounds halves away from zero (as `f64::round`).
    fn round(self) -> Self;
    fn floor(self) -> Self;
    fn ceil(self) -> Self;
    fn trunc(self) -> Self;
}

impl Scalar for f64 {
    fn from_f64(v: f64) -> Self {
        v
    }
    fn v(self) -> f64 {
        self
    }
    fn abs(self) -> Self {
        f64::abs(self)
    }
    fn sqrt(self) -> Self {
        f64::sqrt(self)
    }
    fn exp(self) -> Self {
        f64::exp(self)
    }
    fn ln(self) -> Self {
        f64::ln(self)
    }
    fn sin(self) -> Self {
        f64::sin(self)
    }
    fn cos(self) -> Self {
        f64::cos(self)
    }
    fn sinh(self) -> Self {
        f64::sinh(self)
    }
    fn cosh(self) -> Self {
        f64::cosh(self)
    }
    fn atan2(self, x: Self) -> Self {
        f64::atan2(self, x)
    }
    fn round(self) -> Self {
        f64::round(self)
    }
    fn floor(self) -> Self {
        f64::floor(self)
    }
    fn ceil(self) -> Self {
        f64::ceil(self)
    }
    fn trunc(self) -> Self {
        f64::trunc(self)
    }
}

/// A value with its derivative along one direction: `v + d ε`, ε² = 0.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Dual {
    pub v: f64,
    pub d: f64,
}

impl Dual {
    pub const fn new(v: f64, d: f64) -> Dual {
        Dual { v, d }
    }
}

impl Add for Dual {
    type Output = Dual;
    fn add(self, o: Dual) -> Dual {
        Dual::new(self.v + o.v, self.d + o.d)
    }
}

impl Sub for Dual {
    type Output = Dual;
    fn sub(self, o: Dual) -> Dual {
        Dual::new(self.v - o.v, self.d - o.d)
    }
}

impl Mul for Dual {
    type Output = Dual;
    fn mul(self, o: Dual) -> Dual {
        Dual::new(self.v * o.v, self.d * o.v + self.v * o.d)
    }
}

impl Div for Dual {
    type Output = Dual;
    fn div(self, o: Dual) -> Dual {
        let q = self.v / o.v;
        Dual::new(q, (self.d - q * o.d) / o.v)
    }
}

impl Neg for Dual {
    type Output = Dual;
    fn neg(self) -> Dual {
        Dual::new(-self.v, -self.d)
    }
}

/// Derivative factor that stays finite where the function's slope is infinite (sqrt at 0).
fn safe_div(a: f64, b: f64) -> f64 {
    if b == 0.0 { 0.0 } else { a / b }
}

impl Scalar for Dual {
    fn from_f64(v: f64) -> Self {
        Dual::new(v, 0.0)
    }
    fn v(self) -> f64 {
        self.v
    }
    fn d(self) -> f64 {
        self.d
    }
    fn with_d(v: f64, d: f64) -> Self {
        Dual::new(v, d)
    }
    fn abs(self) -> Self {
        if self.v < 0.0 { -self } else { self }
    }
    fn sqrt(self) -> Self {
        let r = self.v.sqrt();
        Dual::new(r, safe_div(self.d, 2.0 * r))
    }
    fn exp(self) -> Self {
        let e = self.v.exp();
        Dual::new(e, e * self.d)
    }
    fn ln(self) -> Self {
        Dual::new(self.v.ln(), self.d / self.v)
    }
    fn sin(self) -> Self {
        Dual::new(self.v.sin(), self.v.cos() * self.d)
    }
    fn cos(self) -> Self {
        Dual::new(self.v.cos(), -self.v.sin() * self.d)
    }
    fn sinh(self) -> Self {
        Dual::new(self.v.sinh(), self.v.cosh() * self.d)
    }
    fn cosh(self) -> Self {
        Dual::new(self.v.cosh(), self.v.sinh() * self.d)
    }
    fn atan2(self, x: Self) -> Self {
        let r2 = x.v * x.v + self.v * self.v;
        Dual::new(self.v.atan2(x.v), safe_div(x.v * self.d - self.v * x.d, r2))
    }
    fn round(self) -> Self {
        Dual::new(self.v.round(), 0.0)
    }
    fn floor(self) -> Self {
        Dual::new(self.v.floor(), 0.0)
    }
    fn ceil(self) -> Self {
        Dual::new(self.v.ceil(), 0.0)
    }
    fn trunc(self) -> Self {
        Dual::new(self.v.trunc(), 0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Derivative of `f` at `x` by central differences.
    fn numeric(f: impl Fn(f64) -> f64, x: f64) -> f64 {
        let h = 1e-6;
        (f(x + h) - f(x - h)) / (2.0 * h)
    }

    #[test]
    fn dual_derivatives_match_central_differences() {
        let x = 0.7;
        type Case = (&'static str, fn(Dual) -> Dual, fn(f64) -> f64);
        let cases: [Case; 8] = [
            ("sqrt", |a| a.sqrt(), f64::sqrt),
            ("exp", |a| a.exp(), f64::exp),
            ("ln", |a| a.ln(), f64::ln),
            ("sin", |a| a.sin(), f64::sin),
            ("cos", |a| a.cos(), f64::cos),
            ("sinh", |a| a.sinh(), f64::sinh),
            ("cosh", |a| a.cosh(), f64::cosh),
            ("poly", |a| a * a * a - a / (a + Dual::one()), |a| a * a * a - a / (a + 1.0)),
        ];
        for (name, fd, ff) in cases {
            let got = fd(Dual::new(x, 1.0));
            assert!((got.v - ff(x)).abs() < 1e-12, "{name} value");
            assert!((got.d - numeric(ff, x)).abs() < 1e-6, "{name}: {} vs {}", got.d, numeric(ff, x));
        }
        let a = Dual::new(0.3, 1.0).atan2(Dual::new(-0.8, 0.0));
        assert!((a.d - numeric(|y| y.atan2(-0.8), 0.3)).abs() < 1e-6);
    }
}
