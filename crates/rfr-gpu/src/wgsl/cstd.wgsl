// SPDX-License-Identifier: LGPL-3.0-or-later
//
// Complex numbers and the formula standard library over the scalar layer (S, s_*). Mirrors
// rfr_core::math::complex and rfr_core::formula::cpu (real_fn, real_pow, real_powi) function
// by function, in the same operation order. Change both together.

struct Cx {
    re: S,
    im: S,
}

const PI_2: f32 = 1.5707963267948966;

fn c_f(re: f32, im: f32) -> Cx { return Cx(s_c(re), s_c(im)); }
fn c_real(re: S) -> Cx { return Cx(re, s_c(0.0)); }
fn c_add(a: Cx, b: Cx) -> Cx { return Cx(s_add(a.re, b.re), s_add(a.im, b.im)); }
fn c_sub(a: Cx, b: Cx) -> Cx { return Cx(s_sub(a.re, b.re), s_sub(a.im, b.im)); }
fn c_mul(a: Cx, b: Cx) -> Cx {
    return Cx(s_sub(s_mul(a.re, b.re), s_mul(a.im, b.im)), s_add(s_mul(a.re, b.im), s_mul(a.im, b.re)));
}
fn c_norm(a: Cx) -> S { return s_add(s_mul(a.re, a.re), s_mul(a.im, a.im)); }
fn c_div(a: Cx, b: Cx) -> Cx {
    let n = c_norm(b);
    return Cx(s_div(s_add(s_mul(a.re, b.re), s_mul(a.im, b.im)), n), s_div(s_sub(s_mul(a.im, b.re), s_mul(a.re, b.im)), n));
}
fn c_neg(a: Cx) -> Cx { return Cx(s_neg(a.re), s_neg(a.im)); }
fn c_conj(a: Cx) -> Cx { return Cx(a.re, s_neg(a.im)); }
fn c_flip(a: Cx) -> Cx { return Cx(a.im, a.re); }
fn c_cabs(a: Cx) -> S { return s_sqrt(c_norm(a)); }
fn c_arg(a: Cx) -> S { return s_atan2(a.im, a.re); }

// Procedural noise (rfr_core::math::noise, in f32 on both sides).
fn n_lattice(ix: i32, iy: i32, k: u32) -> f32 {
    let h = hash(bitcast<u32>(ix) * 0x8DA6B343u ^ hash(bitcast<u32>(iy) * 0xD8163841u ^ k));
    return f32(h >> 8u) / 16777216.0;
}
fn n_value(x: f32, y: f32) -> f32 {
    let x0 = floor(x);
    let y0 = floor(y);
    let fx = x - x0;
    let fy = y - y0;
    let ix = i32(x0);
    let iy = i32(y0);
    let sx = fx * fx * (3.0 - 2.0 * fx);
    let sy = fy * fy * (3.0 - 2.0 * fy);
    let a = n_lattice(ix, iy, 0u);
    let b = n_lattice(ix + 1, iy, 0u);
    let c = n_lattice(ix, iy + 1, 0u);
    let d = n_lattice(ix + 1, iy + 1, 0u);
    let top = a + (b - a) * sx;
    let bottom = c + (d - c) * sx;
    return top + (bottom - top) * sy;
}
fn n_grad(i: i32, j: i32, dx: f32, dy: f32) -> f32 {
    let a = n_lattice(i, j, 1u) * 6.283185307179586;
    return cos(a) * dx + sin(a) * dy;
}
fn n_fade(t: f32) -> f32 { return t * t * t * (t * (t * 6.0 - 15.0) + 10.0); }
fn n_perlin(x: f32, y: f32) -> f32 {
    let x0 = floor(x);
    let y0 = floor(y);
    let fx = x - x0;
    let fy = y - y0;
    let ix = i32(x0);
    let iy = i32(y0);
    let sx = n_fade(fx);
    let sy = n_fade(fy);
    let a = n_grad(ix, iy, fx, fy);
    let b = n_grad(ix + 1, iy, fx - 1.0, fy);
    let c = n_grad(ix, iy + 1, fx, fy - 1.0);
    let d = n_grad(ix + 1, iy + 1, fx - 1.0, fy - 1.0);
    let top = a + (b - a) * sx;
    let bottom = c + (d - c) * sx;
    return (top + (bottom - top) * sy) * 1.4142135623730951;
}
fn n_voronoi(x: f32, y: f32) -> f32 {
    let ix = i32(floor(x));
    let iy = i32(floor(y));
    var best = 3.4e38;
    for (var j = -1; j <= 1; j = j + 1) {
        for (var i = -1; i <= 1; i = i + 1) {
            let cx = ix + i;
            let cy = iy + j;
            let px = f32(cx) + n_lattice(cx, cy, 2u);
            let py = f32(cy) + n_lattice(cx, cy, 3u);
            best = min(best, (px - x) * (px - x) + (py - y) * (py - y));
        }
    }
    return sqrt(best);
}
fn c_noise(a: Cx) -> S { return s_c(n_value(s_v(a.re), s_v(a.im))); }
fn c_perlin(a: Cx) -> S { return s_c(n_perlin(s_v(a.re), s_v(a.im))); }
fn c_voronoi(a: Cx) -> S { return s_c(n_voronoi(s_v(a.re), s_v(a.im))); }
fn c_scale(a: Cx, s: S) -> Cx { return Cx(s_mul(a.re, s), s_mul(a.im, s)); }
fn c_sqr(a: Cx) -> Cx {
    return Cx(s_sub(s_mul(a.re, a.re), s_mul(a.im, a.im)), s_mul(s_mul(a.re, a.im), s_c(2.0)));
}
fn c_recip(a: Cx) -> Cx {
    let n = c_norm(a);
    return Cx(s_div(a.re, n), s_neg(s_div(a.im, n)));
}
fn c_abs(a: Cx) -> Cx { return Cx(s_abs(a.re), s_abs(a.im)); }
fn c_round(a: Cx) -> Cx { return Cx(s_round(a.re), s_round(a.im)); }
fn c_floor(a: Cx) -> Cx { return Cx(s_floor(a.re), s_floor(a.im)); }
fn c_ceil(a: Cx) -> Cx { return Cx(s_ceil(a.re), s_ceil(a.im)); }
fn c_trunc(a: Cx) -> Cx { return Cx(s_trunc(a.re), s_trunc(a.im)); }
fn c_ident(a: Cx) -> Cx { return a; }
fn c_zero(a: Cx) -> Cx { return c_f(0.0, 0.0); }
fn c_exp(a: Cx) -> Cx {
    let e = s_exp(a.re);
    return Cx(s_mul(e, s_cos(a.im)), s_mul(e, s_sin(a.im)));
}
fn c_log(a: Cx) -> Cx { return Cx(s_log(c_cabs(a)), c_arg(a)); }
fn c_sqrt(a: Cx) -> Cx {
    let r = c_cabs(a);
    let h = s_c(0.5);
    // |a| ± re can round below zero next to the real axis (GPU sqrt is not exact): clamp.
    let re = s_sqrt(s_max(s_mul(s_add(r, a.re), h), s_c(0.0)));
    let im = s_sqrt(s_max(s_mul(s_sub(r, a.re), h), s_c(0.0)));
    if s_v(a.im) < 0.0 {
        return Cx(re, s_neg(im));
    }
    return Cx(re, im);
}
fn c_pow(a: Cx, w: Cx) -> Cx {
    if s_v(a.re) == 0.0 && s_v(a.im) == 0.0 {
        if s_v(w.re) == 0.0 && s_v(w.im) == 0.0 {
            return c_f(1.0, 0.0);
        }
        return c_f(0.0, 0.0);
    }
    return c_exp(c_mul(w, c_log(a)));
}
fn c_powi(a: Cx, n: i32) -> Cx {
    var r = c_f(1.0, 0.0);
    var b = a;
    var e = u32(abs(n));
    loop {
        if e == 0u {
            break;
        }
        if (e & 1u) == 1u {
            r = c_mul(r, b);
        }
        b = c_mul(b, b);
        e = e >> 1u;
    }
    if n < 0 {
        return c_recip(r);
    }
    return r;
}
fn c_sin(a: Cx) -> Cx { return Cx(s_mul(s_sin(a.re), s_cosh(a.im)), s_mul(s_cos(a.re), s_sinh(a.im))); }
fn c_cos(a: Cx) -> Cx { return Cx(s_mul(s_cos(a.re), s_cosh(a.im)), s_neg(s_mul(s_sin(a.re), s_sinh(a.im)))); }
fn c_cosxx(a: Cx) -> Cx { return Cx(s_mul(s_cos(a.re), s_cosh(a.im)), s_mul(s_sin(a.re), s_sinh(a.im))); }
fn c_tan(a: Cx) -> Cx { return c_div(c_sin(a), c_cos(a)); }
fn c_cotan(a: Cx) -> Cx { return c_div(c_cos(a), c_sin(a)); }
fn c_sinh(a: Cx) -> Cx { return Cx(s_mul(s_sinh(a.re), s_cos(a.im)), s_mul(s_cosh(a.re), s_sin(a.im))); }
fn c_cosh(a: Cx) -> Cx { return Cx(s_mul(s_cosh(a.re), s_cos(a.im)), s_mul(s_sinh(a.re), s_sin(a.im))); }
fn c_tanh(a: Cx) -> Cx { return c_div(c_sinh(a), c_cosh(a)); }
fn c_cotanh(a: Cx) -> Cx { return c_div(c_cosh(a), c_sinh(a)); }
fn c_asin(a: Cx) -> Cx {
    let i = c_f(0.0, 1.0);
    return c_neg(c_mul(i, c_log(c_add(c_mul(i, a), c_sqrt(c_sub(c_f(1.0, 0.0), c_mul(a, a)))))));
}
fn c_acos(a: Cx) -> Cx { return c_sub(c_f(PI_2, 0.0), c_asin(a)); }
fn c_atan(a: Cx) -> Cx {
    let i = c_f(0.0, 1.0);
    let one = c_f(1.0, 0.0);
    return c_div(c_sub(c_log(c_add(one, c_mul(i, a))), c_log(c_sub(one, c_mul(i, a)))), c_f(0.0, 2.0));
}
fn c_asinh(a: Cx) -> Cx { return c_log(c_add(a, c_sqrt(c_add(c_mul(a, a), c_f(1.0, 0.0))))); }
fn c_acosh(a: Cx) -> Cx {
    let one = c_f(1.0, 0.0);
    return c_log(c_add(a, c_mul(c_sqrt(c_sub(a, one)), c_sqrt(c_add(a, one)))));
}
fn c_atanh(a: Cx) -> Cx {
    let one = c_f(1.0, 0.0);
    return c_scale(c_sub(c_log(c_add(one, a)), c_log(c_sub(one, a))), s_c(0.5));
}
fn c_eq(a: Cx, b: Cx) -> bool { return s_v(a.re) == s_v(b.re) && s_v(a.im) == s_v(b.im); }
// Largest finite f32; NaN fails the comparison too.
fn c_finite(a: Cx) -> bool { return abs(s_v(a.re)) <= 3.4028234e38 && abs(s_v(a.im)) <= 3.4028234e38; }

// Real overloads (rfr_core::formula::cpu::real_fn).
fn r_abs(x: S) -> S { return s_abs(x); }
fn r_sqr(x: S) -> S { return s_mul(x, x); }
fn r_exp(x: S) -> S { return s_exp(x); }
fn r_recip(x: S) -> S { return s_div(s_c(1.0), x); }
fn r_ident(x: S) -> S { return x; }
fn r_round(x: S) -> S { return s_round(x); }
fn r_floor(x: S) -> S { return s_floor(x); }
fn r_ceil(x: S) -> S { return s_ceil(x); }
fn r_trunc(x: S) -> S { return s_trunc(x); }
fn r_sin(x: S) -> S { return s_sin(x); }
fn r_cos(x: S) -> S { return s_cos(x); }
fn r_tan(x: S) -> S { return s_div(s_sin(x), s_cos(x)); }
fn r_cotan(x: S) -> S { return s_div(s_cos(x), s_sin(x)); }
fn r_atan(x: S) -> S { return s_atan2(x, s_c(1.0)); }
fn r_sinh(x: S) -> S { return s_sinh(x); }
fn r_cosh(x: S) -> S { return s_cosh(x); }
fn r_tanh(x: S) -> S { return s_div(s_sinh(x), s_cosh(x)); }
fn r_cotanh(x: S) -> S { return s_div(s_cosh(x), s_sinh(x)); }
fn r_asinh(x: S) -> S { return s_log(s_add(x, s_sqrt(s_add(s_mul(x, x), s_c(1.0))))); }
fn r_zero(x: S) -> S { return s_c(0.0); }
fn s_min(a: S, b: S) -> S { return select(a, b, s_v(b) < s_v(a)); }
fn s_max(a: S, b: S) -> S { return select(a, b, s_v(b) > s_v(a)); }
fn s_fmod(a: S, b: S) -> S {
    if s_v(b) == 0.0 {
        return s_c(0.0);
    }
    return s_sub(a, s_mul(b, s_trunc(s_div(a, b))));
}
fn s_rpow(x: S, y: S) -> S {
    if s_v(x) > 0.0 {
        return s_exp(s_mul(y, s_log(x)));
    }
    if s_v(x) == 0.0 {
        return select(s_c(0.0), s_c(1.0), s_v(y) == 0.0);
    }
    return c_pow(c_real(x), c_real(y)).re;
}
fn s_powi(x: S, n: i32) -> S {
    var r = s_c(1.0);
    var b = x;
    var e = u32(abs(n));
    loop {
        if e == 0u {
            break;
        }
        if (e & 1u) == 1u {
            r = s_mul(r, b);
        }
        b = s_mul(b, b);
        e = e >> 1u;
    }
    if n < 0 {
        return s_div(s_c(1.0), r);
    }
    return r;
}
fn i_div(a: i32, b: i32) -> i32 {
    if b == 0 || (a == -2147483647 - 1 && b == -1) {
        return select(0, a, b == -1);
    }
    return a / b;
}
fn i_mod(a: i32, b: i32) -> i32 {
    if b == 0 || b == -1 {
        return 0;
    }
    return a % b;
}
