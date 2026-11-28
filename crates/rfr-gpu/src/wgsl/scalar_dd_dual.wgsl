// SPDX-License-Identifier: LGPL-3.0-or-later
//
// Scalar layer, double-float with a derivative: S = (hi, lo, d), the value in double-float
// (the dd_* arithmetic of scalar_df64.wgsl, prepended by codegen.rs) and its derivative along
// the sample's direction in f32 (distance estimation in deep views; the derivative is per
// pixel, so f32 holds it). Mirrors rfr_core::math::scalar::Dual.

alias S = vec3<f32>;

fn s_c(x: f32) -> S { return S(x, 0.0, 0.0); }
fn s_cc(h: f32, l: f32) -> S { return S(h, l, 0.0); }
fn s_v(a: S) -> f32 { return a.x + a.y; }
fn s_d(a: S) -> f32 { return a.z; }
fn s_add(a: S, b: S) -> S { return S(dd_add(a.xy, b.xy), a.z + b.z); }
fn s_sub(a: S, b: S) -> S { return S(dd_sub(a.xy, b.xy), a.z - b.z); }
fn s_mul(a: S, b: S) -> S { return S(dd_mul(a.xy, b.xy), a.z * b.x + a.x * b.z); }
fn s_div(a: S, b: S) -> S {
    let q = dd_div(a.xy, b.xy);
    return S(q, (a.z - q.x * b.z) / b.x);
}
fn s_neg(a: S) -> S { return -a; }
fn s_abs(a: S) -> S { return select(a, -a, a.x < 0.0); }
fn s_safe_div(a: f32, b: f32) -> f32 { return select(a / b, 0.0, b == 0.0); }
fn s_sqrt(a: S) -> S {
    let r = dd_sqrt(a.xy);
    return S(r, s_safe_div(a.z, 2.0 * r.x));
}
fn s_exp(a: S) -> S {
    let e = dd_exp(a.xy);
    return S(e, e.x * a.z);
}
fn s_log(a: S) -> S { return S(dd_log(a.xy), a.z / a.x); }
fn s_sin(a: S) -> S { return S(dd_sin(a.xy), cos(a.x) * a.z); }
fn s_cos(a: S) -> S { return S(dd_cos(a.xy), -sin(a.x) * a.z); }
fn s_sinh(a: S) -> S { return S(dd_sinh(a.xy), cosh(a.x) * a.z); }
fn s_cosh(a: S) -> S { return S(dd_cosh(a.xy), sinh(a.x) * a.z); }
fn s_atan2(y: S, x: S) -> S {
    let r2 = x.x * x.x + y.x * y.x;
    return S(dd_atan2(y.xy, x.xy), s_safe_div(x.x * y.z - y.x * x.z, r2));
}
fn s_round(a: S) -> S { return S(dd_round(a.xy), 0.0); }
fn s_floor(a: S) -> S { return S(dd_floor(a.xy), 0.0); }
fn s_ceil(a: S) -> S { return S(dd_ceil(a.xy), 0.0); }
fn s_trunc(a: S) -> S { return S(dd_trunc(a.xy), 0.0); }
