// SPDX-License-Identifier: LGPL-3.0-or-later

//! Complex numbers over any [`Scalar`] and the formula standard library. The definitions follow
//! the FractalPark FRM-like v1 stdlib (principal branches, inverse functions through `log`),
//! which also matches UFM and FractInt formulas for the shared names. `cstd.wgsl` in rfr-gpu
//! implements the same functions in the same order; change both together.

use std::ops::{Add, Div, Mul, Neg, Sub};

use super::scalar::Scalar;

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Cx<S> {
    pub re: S,
    pub im: S,
}

pub type C64 = Cx<f64>;

impl<S: Scalar> Cx<S> {
    pub fn new(re: S, im: S) -> Self {
        Cx { re, im }
    }
    pub fn real(re: S) -> Self {
        Cx { re, im: S::zero() }
    }
    pub fn from_f64(re: f64, im: f64) -> Self {
        Cx { re: S::from_f64(re), im: S::from_f64(im) }
    }
    pub fn zero() -> Self {
        Cx::from_f64(0.0, 0.0)
    }
    /// Values without derivative information.
    pub fn v(self) -> C64 {
        C64 { re: self.re.v(), im: self.im.v() }
    }
    pub fn is_finite(self) -> bool {
        self.re.v().is_finite() && self.im.v().is_finite()
    }
    pub fn conj(self) -> Self {
        Cx::new(self.re, -self.im)
    }
    pub fn flip(self) -> Self {
        Cx::new(self.im, self.re)
    }
    /// Squared magnitude (UFM `|z|`).
    pub fn norm(self) -> S {
        self.re * self.re + self.im * self.im
    }
    /// Magnitude, `sqrt(re² + im²)` (FRM-like `|z|`, `cabs`).
    pub fn cabs(self) -> S {
        self.norm().sqrt()
    }
    pub fn arg(self) -> S {
        self.im.atan2(self.re)
    }
    pub fn scale(self, s: S) -> Self {
        Cx::new(self.re * s, self.im * s)
    }
    pub fn sqr(self) -> Self {
        Cx::new(self.re * self.re - self.im * self.im, (self.re * self.im) * S::from_f64(2.0))
    }
    pub fn recip(self) -> Self {
        let n = self.norm();
        Cx::new(self.re / n, -self.im / n)
    }
    /// Component-wise absolute value.
    pub fn abs(self) -> Self {
        Cx::new(self.re.abs(), self.im.abs())
    }
    pub fn round(self) -> Self {
        Cx::new(self.re.round(), self.im.round())
    }
    pub fn floor(self) -> Self {
        Cx::new(self.re.floor(), self.im.floor())
    }
    pub fn ceil(self) -> Self {
        Cx::new(self.re.ceil(), self.im.ceil())
    }
    pub fn trunc(self) -> Self {
        Cx::new(self.re.trunc(), self.im.trunc())
    }
    pub fn exp(self) -> Self {
        let e = self.re.exp();
        Cx::new(e * self.im.cos(), e * self.im.sin())
    }
    /// Principal logarithm, argument in (-π, π].
    pub fn log(self) -> Self {
        Cx::new(self.cabs().ln(), self.arg())
    }
    /// Principal square root; the negative real axis maps to the upper imaginary axis.
    pub fn sqrt(self) -> Self {
        let r = self.cabs();
        let half = S::from_f64(0.5);
        let re = ((r + self.re) * half).sqrt();
        let im = ((r - self.re) * half).sqrt();
        if self.im.v() < 0.0 { Cx::new(re, -im) } else { Cx::new(re, im) }
    }
    /// `exp(w * log(z))`; zero for a zero base.
    pub fn pow(self, w: Self) -> Self {
        if self.re.v() == 0.0 && self.im.v() == 0.0 {
            return Cx::zero();
        }
        (w * self.log()).exp()
    }
    /// Integer power by repeated multiplication (exponent between -64 and 64).
    pub fn powi(self, n: i32) -> Self {
        let mut r = Cx::from_f64(1.0, 0.0);
        let mut b = self;
        let mut e = n.unsigned_abs();
        while e > 0 {
            if e & 1 == 1 {
                r = r * b;
            }
            b = b * b;
            e >>= 1;
        }
        if n < 0 { r.recip() } else { r }
    }
    pub fn sin(self) -> Self {
        Cx::new(self.re.sin() * self.im.cosh(), self.re.cos() * self.im.sinh())
    }
    pub fn cos(self) -> Self {
        Cx::new(self.re.cos() * self.im.cosh(), -(self.re.sin() * self.im.sinh()))
    }
    /// `cos(x) cosh(y) + i sin(x) sinh(y)` (FractInt's cosxx).
    pub fn cosxx(self) -> Self {
        Cx::new(self.re.cos() * self.im.cosh(), self.re.sin() * self.im.sinh())
    }
    pub fn tan(self) -> Self {
        self.sin() / self.cos()
    }
    pub fn cotan(self) -> Self {
        self.cos() / self.sin()
    }
    pub fn sinh(self) -> Self {
        Cx::new(self.re.sinh() * self.im.cos(), self.re.cosh() * self.im.sin())
    }
    pub fn cosh(self) -> Self {
        Cx::new(self.re.cosh() * self.im.cos(), self.re.sinh() * self.im.sin())
    }
    pub fn tanh(self) -> Self {
        self.sinh() / self.cosh()
    }
    pub fn cotanh(self) -> Self {
        self.cosh() / self.sinh()
    }
    fn i() -> Self {
        Cx::from_f64(0.0, 1.0)
    }
    fn one() -> Self {
        Cx::from_f64(1.0, 0.0)
    }
    /// `-i log(i z + sqrt(1 - z²))`
    pub fn asin(self) -> Self {
        let i = Cx::i();
        -(i * (i * self + (Cx::one() - self * self).sqrt()).log())
    }
    /// `π/2 - asin(z)`
    pub fn acos(self) -> Self {
        Cx::from_f64(std::f64::consts::FRAC_PI_2, 0.0) - self.asin()
    }
    /// `(log(1 + i z) - log(1 - i z)) / (2 i)`
    pub fn atan(self) -> Self {
        let i = Cx::i();
        ((Cx::one() + i * self).log() - (Cx::one() - i * self).log()) / Cx::from_f64(0.0, 2.0)
    }
    /// `log(z + sqrt(z² + 1))`
    pub fn asinh(self) -> Self {
        (self + (self * self + Cx::one()).sqrt()).log()
    }
    /// `log(z + sqrt(z - 1) sqrt(z + 1))`
    pub fn acosh(self) -> Self {
        (self + (self - Cx::one()).sqrt() * (self + Cx::one()).sqrt()).log()
    }
    /// `(log(1 + z) - log(1 - z)) / 2`
    pub fn atanh(self) -> Self {
        ((Cx::one() + self).log() - (Cx::one() - self).log()).scale(S::from_f64(0.5))
    }
}

impl<S: Scalar> Add for Cx<S> {
    type Output = Self;
    fn add(self, o: Self) -> Self {
        Cx::new(self.re + o.re, self.im + o.im)
    }
}

impl<S: Scalar> Sub for Cx<S> {
    type Output = Self;
    fn sub(self, o: Self) -> Self {
        Cx::new(self.re - o.re, self.im - o.im)
    }
}

impl<S: Scalar> Mul for Cx<S> {
    type Output = Self;
    fn mul(self, o: Self) -> Self {
        Cx::new(self.re * o.re - self.im * o.im, self.re * o.im + self.im * o.re)
    }
}

impl<S: Scalar> Div for Cx<S> {
    type Output = Self;
    fn div(self, o: Self) -> Self {
        let n = o.norm();
        Cx::new((self.re * o.re + self.im * o.im) / n, (self.im * o.re - self.re * o.im) / n)
    }
}

impl<S: Scalar> Neg for Cx<S> {
    type Output = Self;
    fn neg(self) -> Self {
        Cx::new(-self.re, -self.im)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::scalar::Dual;

    fn close(a: C64, b: C64) -> bool {
        (a.re - b.re).abs() < 1e-10 && (a.im - b.im).abs() < 1e-10
    }

    #[test]
    fn inverse_functions_invert() {
        let z = C64::new(0.3, -0.4);
        assert!(close(z.sin().asin(), z));
        assert!(close(z.cos().acos(), z));
        assert!(close(z.tan().atan(), z));
        assert!(close(z.sinh().asinh(), z));
        assert!(close(z.cosh().acosh(), z));
        assert!(close(z.tanh().atanh(), z));
        assert!(close(z.exp().log(), z));
        assert!(close(z.sqrt().sqr(), z));
        assert!(close(z.powi(5), z * z * z * z * z));
        assert!(close(z.powi(-2), (z * z).recip()));
        assert!(close(z.pow(C64::new(3.0, 0.0)), z.powi(3)));
    }

    #[test]
    fn branch_cuts_are_principal() {
        // sqrt(-4) on the cut: upper imaginary axis.
        assert!(close(C64::new(-4.0, 0.0).sqrt(), C64::new(0.0, 2.0)));
        assert!(close(C64::new(-4.0, -1e-300).sqrt(), C64::new(0.0, -2.0)));
        assert!((C64::new(-1.0, 0.0).log().im - std::f64::consts::PI).abs() < 1e-15);
        assert_eq!(C64::zero().pow(C64::new(-2.0, 0.0)), C64::zero());
    }

    #[test]
    fn dual_complex_gives_the_holomorphic_derivative() {
        // d/dt sin(z0 + t) = cos(z0) for every holomorphic function.
        let z0 = C64::new(0.4, 0.2);
        let z = Cx::new(Dual::new(z0.re, 1.0), Dual::new(z0.im, 0.0));
        let s = z.sin();
        assert!(close(C64::new(s.re.d, s.im.d), z0.cos()));
        let p = (z * z).exp().log().sqrt();
        let num = {
            let h = 1e-7;
            let f = |a: C64| (a * a).exp().log().sqrt();
            let d = f(C64::new(z0.re + h, z0.im)) - f(C64::new(z0.re - h, z0.im));
            C64::new(d.re / (2.0 * h), d.im / (2.0 * h))
        };
        assert!((p.re.d - num.re).abs() < 1e-6 && (p.im.d - num.im).abs() < 1e-6);
    }
}
