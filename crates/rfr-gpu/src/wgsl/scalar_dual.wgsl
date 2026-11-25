// SPDX-License-Identifier: LGPL-3.0-or-later
//
// Scalar layer, dual numbers: x = value, y = derivative along the sample's direction
// (automatic differentiation for distance estimation). Mirrors rfr_core::math::scalar::Dual.

alias S = vec2<f32>;

fn s_c(x: f32) -> S { return vec2<f32>(x, 0.0); }
fn s_cc(h: f32, l: f32) -> S { return vec2<f32>(h, 0.0); }
fn s_v(a: S) -> f32 { return a.x; }
fn s_d(a: S) -> f32 { return a.y; }
fn s_add(a: S, b: S) -> S { return a + b; }
fn s_sub(a: S, b: S) -> S { return a - b; }
fn s_mul(a: S, b: S) -> S { return vec2<f32>(a.x * b.x, a.y * b.x + a.x * b.y); }
fn s_div(a: S, b: S) -> S {
    let q = a.x / b.x;
    return vec2<f32>(q, (a.y - q * b.y) / b.x);
}
fn s_neg(a: S) -> S { return -a; }
fn s_abs(a: S) -> S { return select(a, -a, a.x < 0.0); }
fn s_safe_div(a: f32, b: f32) -> f32 { return select(a / b, 0.0, b == 0.0); }
fn s_sqrt(a: S) -> S {
    let r = sqrt(a.x);
    return vec2<f32>(r, s_safe_div(a.y, 2.0 * r));
}
fn s_exp(a: S) -> S {
    let e = exp(a.x);
    return vec2<f32>(e, e * a.y);
}
fn s_log(a: S) -> S { return vec2<f32>(log(a.x), a.y / a.x); }
fn s_sin(a: S) -> S { return vec2<f32>(sin(a.x), cos(a.x) * a.y); }
fn s_cos(a: S) -> S { return vec2<f32>(cos(a.x), -sin(a.x) * a.y); }
fn s_sinh(a: S) -> S { return vec2<f32>(sinh(a.x), cosh(a.x) * a.y); }
fn s_cosh(a: S) -> S { return vec2<f32>(cosh(a.x), sinh(a.x) * a.y); }
fn s_atan2(y: S, x: S) -> S {
    let r2 = x.x * x.x + y.x * y.x;
    return vec2<f32>(atan2(y.x, x.x), s_safe_div(x.x * y.y - y.x * x.y, r2));
}
fn s_round(a: S) -> S { return vec2<f32>(sign(a.x) * floor(abs(a.x) + 0.5), 0.0); }
fn s_floor(a: S) -> S { return vec2<f32>(floor(a.x), 0.0); }
fn s_ceil(a: S) -> S { return vec2<f32>(ceil(a.x), 0.0); }
fn s_trunc(a: S) -> S { return vec2<f32>(trunc(a.x), 0.0); }
