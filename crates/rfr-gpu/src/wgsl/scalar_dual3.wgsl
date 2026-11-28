// SPDX-License-Identifier: LGPL-3.0-or-later
//
// Scalar layer, dual numbers with three derivatives: x = value, yzw = its gradient by the
// starting point's x, y and z (3D formulas: the Jacobian of the orbit for the distance
// estimate). Mirrors rfr_core::math::scalar::Dual3.

alias S = vec4<f32>;

fn s_c(x: f32) -> S { return S(x, 0.0, 0.0, 0.0); }
fn s_cc(h: f32, l: f32) -> S { return S(h, 0.0, 0.0, 0.0); }
fn s_v(a: S) -> f32 { return a.x; }
fn s_d(a: S) -> f32 { return length(a.yzw); }
fn s_add(a: S, b: S) -> S { return a + b; }
fn s_sub(a: S, b: S) -> S { return a - b; }
fn s_mul(a: S, b: S) -> S { return S(a.x * b.x, a.yzw * b.x + a.x * b.yzw); }
fn s_div(a: S, b: S) -> S {
    let q = a.x / b.x;
    return S(q, (a.yzw - q * b.yzw) / b.x);
}
fn s_neg(a: S) -> S { return -a; }
fn s_abs(a: S) -> S { return select(a, -a, a.x < 0.0); }
fn s_safe_div(a: f32, b: f32) -> f32 { return select(a / b, 0.0, b == 0.0); }
fn s_sqrt(a: S) -> S {
    let r = sqrt(a.x);
    return S(r, a.yzw * s_safe_div(1.0, 2.0 * r));
}
fn s_exp(a: S) -> S {
    let e = exp(a.x);
    return S(e, e * a.yzw);
}
fn s_log(a: S) -> S { return S(log(a.x), a.yzw / a.x); }
fn s_sin(a: S) -> S { return S(sin(a.x), cos(a.x) * a.yzw); }
fn s_cos(a: S) -> S { return S(cos(a.x), -sin(a.x) * a.yzw); }
fn s_sinh(a: S) -> S { return S(sinh(a.x), cosh(a.x) * a.yzw); }
fn s_cosh(a: S) -> S { return S(cosh(a.x), sinh(a.x) * a.yzw); }
fn s_atan2(y: S, x: S) -> S {
    let r2 = x.x * x.x + y.x * y.x;
    return S(atan2(y.x, x.x), select((x.x * y.yzw - y.x * x.yzw) / r2, vec3<f32>(0.0), r2 == 0.0));
}
fn s_round(a: S) -> S { return S(sign(a.x) * floor(abs(a.x) + 0.5), 0.0, 0.0, 0.0); }
fn s_floor(a: S) -> S { return S(floor(a.x), 0.0, 0.0, 0.0); }
fn s_ceil(a: S) -> S { return S(ceil(a.x), 0.0, 0.0, 0.0); }
fn s_trunc(a: S) -> S { return S(trunc(a.x), 0.0, 0.0, 0.0); }
