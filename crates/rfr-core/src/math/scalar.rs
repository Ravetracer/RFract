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

/// A value with its gradient along three directions (3D objects: the derivatives of the orbit
/// by the starting point's x, y and z, see `render::object`).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Dual3 {
    pub v: f64,
    pub d: [f64; 3],
}

impl Dual3 {
    pub const fn new(v: f64, d: [f64; 3]) -> Dual3 {
        Dual3 { v, d }
    }

    /// The value with the gradient scaled by `k`.
    fn chain(self, v: f64, k: f64) -> Dual3 {
        Dual3::new(v, self.d.map(|g| g * k))
    }
}

impl Add for Dual3 {
    type Output = Dual3;
    fn add(self, o: Dual3) -> Dual3 {
        Dual3::new(self.v + o.v, std::array::from_fn(|i| self.d[i] + o.d[i]))
    }
}

impl Sub for Dual3 {
    type Output = Dual3;
    fn sub(self, o: Dual3) -> Dual3 {
        Dual3::new(self.v - o.v, std::array::from_fn(|i| self.d[i] - o.d[i]))
    }
}

// The product and quotient rules mix the operators on purpose.
#[allow(clippy::suspicious_arithmetic_impl)]
impl Mul for Dual3 {
    type Output = Dual3;
    fn mul(self, o: Dual3) -> Dual3 {
        Dual3::new(self.v * o.v, std::array::from_fn(|i| self.d[i] * o.v + self.v * o.d[i]))
    }
}

// The product and quotient rules mix the operators on purpose.
#[allow(clippy::suspicious_arithmetic_impl)]
impl Div for Dual3 {
    type Output = Dual3;
    fn div(self, o: Dual3) -> Dual3 {
        let q = self.v / o.v;
        Dual3::new(q, std::array::from_fn(|i| (self.d[i] - q * o.d[i]) / o.v))
    }
}

impl Neg for Dual3 {
    type Output = Dual3;
    fn neg(self) -> Dual3 {
        Dual3::new(-self.v, self.d.map(|g| -g))
    }
}

impl Scalar for Dual3 {
    fn from_f64(v: f64) -> Self {
        Dual3::new(v, [0.0; 3])
    }
    fn v(self) -> f64 {
        self.v
    }
    /// The gradient's length.
    fn d(self) -> f64 {
        (self.d[0] * self.d[0] + self.d[1] * self.d[1] + self.d[2] * self.d[2]).sqrt()
    }
    fn abs(self) -> Self {
        if self.v < 0.0 { -self } else { self }
    }
    fn sqrt(self) -> Self {
        let r = self.v.sqrt();
        self.chain(r, safe_div(1.0, 2.0 * r))
    }
    fn exp(self) -> Self {
        let e = self.v.exp();
        self.chain(e, e)
    }
    fn ln(self) -> Self {
        self.chain(self.v.ln(), 1.0 / self.v)
    }
    fn sin(self) -> Self {
        self.chain(self.v.sin(), self.v.cos())
    }
    fn cos(self) -> Self {
        self.chain(self.v.cos(), -self.v.sin())
    }
    fn sinh(self) -> Self {
        self.chain(self.v.sinh(), self.v.cosh())
    }
    fn cosh(self) -> Self {
        self.chain(self.v.cosh(), self.v.sinh())
    }
    fn atan2(self, x: Self) -> Self {
        let r2 = x.v * x.v + self.v * self.v;
        Dual3::new(self.v.atan2(x.v), std::array::from_fn(|i| safe_div(x.v * self.d[i] - self.v * x.d[i], r2)))
    }
    fn round(self) -> Self {
        Dual3::from_f64(self.v.round())
    }
    fn floor(self) -> Self {
        Dual3::from_f64(self.v.floor())
    }
    fn ceil(self) -> Self {
        Dual3::from_f64(self.v.ceil())
    }
    fn trunc(self) -> Self {
        Dual3::from_f64(self.v.trunc())
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

    #[test]
    fn dual3_gradients_match_central_differences() {
        // f(x, y, z) = sin(x·y) + z² / (1 + x): the three partial derivatives.
        let f = |x: Dual3, y: Dual3, z: Dual3| (x * y).sin() + z * z / (Dual3::one() + x);
        let ff = |x: f64, y: f64, z: f64| (x * y).sin() + z * z / (1.0 + x);
        let (x, y, z) = (0.4, -0.7, 1.3);
        let g = f(Dual3::new(x, [1.0, 0.0, 0.0]), Dual3::new(y, [0.0, 1.0, 0.0]), Dual3::new(z, [0.0, 0.0, 1.0]));
        let want = [numeric(|t| ff(t, y, z), x), numeric(|t| ff(x, t, z), y), numeric(|t| ff(x, y, t), z)];
        for (i, (got, want)) in g.d.iter().zip(want).enumerate() {
            assert!((got - want).abs() < 1e-6, "{i}: {got} vs {want}");
        }
    }
}
