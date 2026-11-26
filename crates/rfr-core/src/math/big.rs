// SPDX-License-Identifier: LGPL-3.0-or-later

//! Arbitrary-precision binary floats (dashu) for view centers and reference orbits.

use std::str::FromStr;

use dashu_float::round::mode::HalfEven;
use dashu_float::{DBig, FBig};
use dashu_int::IBig;
use dashu_int::ops::BitTest;

use super::fx::Fx;

/// A binary float with round-half-even.
pub type Big = FBig<HalfEven, 2>;

/// Bits that resolve details of size `scale` on numbers around 1, with a safety margin.
pub fn bits_for(scale: Fx) -> usize {
    ((-scale.log2()).max(0.0) as usize + 96).max(128)
}

/// A decimal string as a binary float with `bits` of precision (invalid text: 0).
pub fn parse(s: &str, bits: usize) -> Big {
    match DBig::from_str(s.trim()) {
        Ok(d) => d.with_base_and_precision::<2>(bits).value().with_rounding::<HalfEven>(),
        Err(_) => Big::ZERO.with_precision(bits).value(),
    }
}

/// The value as floatexp (53 significant bits).
pub fn to_fx(x: &Big) -> Fx {
    let r = x.repr();
    let sig = r.significand();
    if *sig == IBig::ZERO {
        return Fx::ZERO;
    }
    let shift = sig.bit_len() as isize - 62;
    let top: IBig = if shift > 0 { sig >> (shift as usize) } else { sig << ((-shift) as usize) };
    let m = i64::try_from(&top).unwrap_or(0) as f64;
    let e = (r.exponent() + shift).clamp(i32::MIN as isize / 2, i32::MAX as isize / 2) as i32;
    Fx::new(m, e)
}

/// A floatexp as a binary float (exact).
pub fn from_fx(v: Fx, bits: usize) -> Big {
    if v.is_zero() {
        return Big::ZERO.with_precision(bits).value();
    }
    // m has 53 significant bits: m · 2^52 is an integer.
    let mi = (v.m * 2f64.powi(52)) as i64;
    Big::from_parts(IBig::from(mi), v.e as isize - 52).with_precision(bits).value()
}

/// Decimal text with enough digits for details of size `scale` (trailing zeros removed).
pub fn format(x: &Big, scale: Fx) -> String {
    let mag = to_fx(x).abs().log10().max(scale.log10());
    let digits = ((mag - scale.log10()).ceil().max(0.0) as usize + 8).max(17);
    let d = x.clone().with_base_and_precision::<10>(digits).value();
    let s = d.to_string();
    let s = if s.contains('.') && !s.contains(['e', 'E']) { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s };
    if s == "-0" || s.is_empty() { "0".into() } else { s }
}

/// `center + offset` on decimal text, kept exact to details of size `scale`.
pub fn add(center: &str, offset: Fx, scale: Fx) -> String {
    let bits = bits_for(scale) + (offset.abs().log2().max(0.0) as usize);
    let c = parse(center, bits);
    format(&(c + from_fx(offset, bits)), scale)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deep_offsets() {
        let scale = Fx::parse("1e-1000").unwrap();
        let c = "-1.75";
        let moved = add(c, Fx::parse("3e-1000").unwrap(), scale);
        assert!(moved.starts_with("-1.74999999") && moved.ends_with("97"), "{moved}");
        let back = add(&moved, Fx::parse("-3e-1000").unwrap(), scale);
        assert_eq!(back, "-1.75");
        let x = parse("0.1", 200);
        assert!((to_fx(&x).to_f64() - 0.1).abs() < 1e-17);
        assert_eq!(to_fx(&from_fx(Fx::parse("-2.5e-777").unwrap(), 300)).to_string(), "-2.5e-777");
        assert_eq!(add("0.5", Fx::from_f64(0.25), Fx::from_f64(0.01)), "0.75");
    }
}
