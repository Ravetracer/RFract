// SPDX-License-Identifier: LGPL-3.0-or-later
//
// Perturbation for deep zooms (rfr_core::formula::perturb): complex floats with one extended
// exponent for both parts, (re, im) · 2^e, max(|re|, |im|) in [0.5, 1) or zero, and complex
// helpers for the reference values (vec2: re, im). Mirrors rfr_core::math::fx::Cfx.

struct Cf {
    re: f32,
    im: f32,
    e: i32,
}

const CF0: Cf = Cf(0.0, 0.0, 0);

fn cf_make(re: f32, im: f32, e: i32) -> Cf {
    let m = max(abs(re), abs(im));
    if m == 0.0 {
        return CF0;
    }
    let k = frexp(m).exp;
    return Cf(ldexp(re, -k), ldexp(im, -k), e + k);
}

fn cf_v2(v: vec2<f32>) -> Cf {
    return cf_make(v.x, v.y, 0);
}

// The value as f32 parts (0 below their range).
fn cf_f(a: Cf) -> vec2<f32> {
    if a.e < -160 {
        return vec2<f32>(0.0);
    }
    let e = clamp(a.e, -160, 128);
    return vec2<f32>(ldexp(a.re, e), ldexp(a.im, e));
}

fn cf_neg(a: Cf) -> Cf {
    return Cf(-a.re, -a.im, a.e);
}

fn cf_conj(a: Cf) -> Cf {
    return Cf(a.re, -a.im, a.e);
}

fn cf_flip(a: Cf) -> Cf {
    return Cf(a.im, a.re, a.e);
}

fn cf_add(a: Cf, b: Cf) -> Cf {
    if a.re == 0.0 && a.im == 0.0 {
        return b;
    }
    if b.re == 0.0 && b.im == 0.0 {
        return a;
    }
    if a.e >= b.e {
        let d = a.e - b.e;
        if d > 40 {
            return a;
        }
        return cf_make(a.re + ldexp(b.re, -d), a.im + ldexp(b.im, -d), a.e);
    }
    let d = b.e - a.e;
    if d > 40 {
        return b;
    }
    return cf_make(b.re + ldexp(a.re, -d), b.im + ldexp(a.im, -d), b.e);
}

fn cf_sub(a: Cf, b: Cf) -> Cf {
    return cf_add(a, cf_neg(b));
}

fn cf_mul(a: Cf, b: Cf) -> Cf {
    return cf_make(a.re * b.re - a.im * b.im, a.re * b.im + a.im * b.re, a.e + b.e);
}

fn cf_div(a: Cf, b: Cf) -> Cf {
    let d = b.re * b.re + b.im * b.im;
    if d == 0.0 {
        return Cf(1e30, 1e30, 200);
    }
    return cf_make((a.re * b.re + a.im * b.im) / d, (a.im * b.re - a.re * b.im) / d, a.e - b.e);
}

// Squared magnitude as a real floatexp.
fn cf_norm(a: Cf) -> Cf {
    return cf_make(a.re * a.re + a.im * a.im, 0.0, 2 * a.e);
}

// a < b for non-negative reals.
fn cf_lt(a: Cf, b: Cf) -> bool {
    if b.re == 0.0 {
        return false;
    }
    if a.re == 0.0 {
        return true;
    }
    if a.e != b.e {
        return a.e < b.e;
    }
    return a.re < b.re;
}

fn cf_re(a: Cf) -> Cf {
    return cf_make(a.re, 0.0, a.e);
}

fn cf_im(a: Cf) -> Cf {
    return cf_make(a.im, 0.0, a.e);
}

// A complex from the real parts of two real floatexps.
fn cf_makec(a: Cf, b: Cf) -> Cf {
    if a.re == 0.0 {
        return Cf(0.0, b.re, b.e);
    }
    if b.re == 0.0 {
        return Cf(a.re, 0.0, a.e);
    }
    let e = max(a.e, b.e);
    return cf_make(ldexp(a.re, a.e - e), ldexp(b.re, b.e - e), e);
}

// |R + D| − |R| without cancellation (D real).
fn cf_diffabs(r: f32, d: Cf) -> Cf {
    let rd = cf_add(cf_make(r, 0.0, 0), d);
    if r >= 0.0 {
        if rd.re >= 0.0 {
            return d;
        }
        return cf_neg(cf_add(cf_make(2.0 * r, 0.0, 0), d));
    }
    if rd.re > 0.0 {
        return cf_add(cf_make(2.0 * r, 0.0, 0), d);
    }
    return cf_neg(d);
}

fn cf_absd(r: vec2<f32>, d: Cf) -> Cf {
    return cf_makec(cf_diffabs(r.x, cf_re(d)), cf_diffabs(r.y, cf_im(d)));
}

fn v2_mul(a: vec2<f32>, b: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(a.x * b.x - a.y * b.y, a.x * b.y + a.y * b.x);
}

// a^n for n >= 0.
fn v2_powi(a: vec2<f32>, n: u32) -> vec2<f32> {
    var r = vec2<f32>(1.0, 0.0);
    for (var k = 0u; k < n; k = k + 1u) {
        r = v2_mul(r, a);
    }
    return r;
}

// The parts of `a` times the signs `s` (±1 or 0 per part).
fn cf_signs(s: vec2<f32>, a: Cf) -> Cf {
    return cf_make(s.x * a.re, s.y * a.im, a.e);
}

fn v2_div(a: vec2<f32>, b: vec2<f32>) -> vec2<f32> {
    let d = b.x * b.x + b.y * b.y;
    return vec2<f32>((a.x * b.x + a.y * b.y) / d, (a.y * b.x - a.x * b.y) / d);
}

// M · v for a 2 × 2 floatexp matrix (a, e) and a complex floatexp v.
fn cf_mapply(a: vec4<f32>, e: i32, v: Cf) -> Cf {
    return cf_make(a.x * v.re + a.y * v.im, a.z * v.re + a.w * v.im, e + v.e);
}
