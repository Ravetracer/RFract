// SPDX-License-Identifier: LGPL-3.0-or-later
//
// Scalar layer, plain f32. cstd.wgsl is written against these functions; scalar_dual.wgsl
// provides the same names for dual numbers (value + derivative). Mirrors rfr_core::math::scalar.

alias S = f32;

fn s_c(x: f32) -> S { return x; }
fn s_cc(h: f32, l: f32) -> S { return h; }
fn s_v(a: S) -> f32 { return a; }
fn s_d(a: S) -> f32 { return 0.0; }
fn s_add(a: S, b: S) -> S { return a + b; }
fn s_sub(a: S, b: S) -> S { return a - b; }
fn s_mul(a: S, b: S) -> S { return a * b; }
fn s_div(a: S, b: S) -> S { return a / b; }
fn s_neg(a: S) -> S { return -a; }
fn s_abs(a: S) -> S { return abs(a); }
fn s_sqrt(a: S) -> S { return sqrt(a); }
fn s_exp(a: S) -> S { return exp(a); }
fn s_log(a: S) -> S { return log(a); }
fn s_sin(a: S) -> S { return sin(a); }
fn s_cos(a: S) -> S { return cos(a); }
fn s_sinh(a: S) -> S { return sinh(a); }
fn s_cosh(a: S) -> S { return cosh(a); }
fn s_atan2(y: S, x: S) -> S { return atan2(y, x); }
// Halves away from zero (WGSL's round goes to even).
fn s_round(a: S) -> S { return sign(a) * floor(abs(a) + 0.5); }
fn s_floor(a: S) -> S { return floor(a); }
fn s_ceil(a: S) -> S { return ceil(a); }
fn s_trunc(a: S) -> S { return trunc(a); }
