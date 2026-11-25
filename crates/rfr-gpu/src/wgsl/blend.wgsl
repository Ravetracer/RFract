// SPDX-License-Identifier: LGPL-3.0-or-later
//
// Layer compositing: merge modes after the W3C "Compositing and Blending" specification on
// sRGB-encoded colors, straight alpha. Mirrors rfr_core::render::composite; change both
// together. Mode numbers follow rfr_core::doc::MergeMode::ALL.

fn srgb_enc(v: f32) -> f32 {
    let c = clamp(v, 0.0, 1.0);
    if c <= 0.0031308 {
        return c * 12.92;
    }
    return 1.055 * pow(c, 1.0 / 2.4) - 0.055;
}

fn srgb_dec(v: f32) -> f32 {
    let c = clamp(v, 0.0, 1.0);
    if c <= 0.04045 {
        return c / 12.92;
    }
    return pow((c + 0.055) / 1.055, 2.4);
}

fn b_lum(c: vec3<f32>) -> f32 { return 0.3 * c.x + 0.59 * c.y + 0.11 * c.z; }

fn b_clip(c: vec3<f32>) -> vec3<f32> {
    let l = b_lum(c);
    let n = min(min(c.x, c.y), c.z);
    let x = max(max(c.x, c.y), c.z);
    var o = c;
    if n < 0.0 {
        o = vec3<f32>(l) + (o - vec3<f32>(l)) * l / max(l - n, 1e-12);
    }
    if x > 1.0 {
        o = vec3<f32>(l) + (o - vec3<f32>(l)) * (1.0 - l) / max(x - l, 1e-12);
    }
    return o;
}

fn b_set_lum(c: vec3<f32>, l: f32) -> vec3<f32> { return b_clip(c + vec3<f32>(l - b_lum(c))); }

fn b_sat(c: vec3<f32>) -> f32 { return max(max(c.x, c.y), c.z) - min(min(c.x, c.y), c.z); }

fn b_set_sat(c: vec3<f32>, s: f32) -> vec3<f32> {
    let mx = max(max(c.x, c.y), c.z);
    let mn = min(min(c.x, c.y), c.z);
    if mx > mn {
        return (c - vec3<f32>(mn)) * s / (mx - mn);
    }
    return vec3<f32>(0.0);
}

fn b_hard(cb: f32, cs: f32) -> f32 {
    if cs <= 0.5 {
        return cb * 2.0 * cs;
    }
    let s = 2.0 * cs - 1.0;
    return cb + s - cb * s;
}

fn b_soft_d(cb: f32) -> f32 {
    if cb <= 0.25 {
        return ((16.0 * cb - 12.0) * cb + 4.0) * cb;
    }
    return sqrt(cb);
}

fn b_channel(mode: u32, cb: f32, cs: f32) -> f32 {
    switch mode {
        case 1u: { return cb * cs; }
        case 2u: { return cb + cs - cb * cs; }
        case 3u: { return b_hard(cs, cb); }
        case 4u: {
            if cs <= 0.5 {
                return cb - (1.0 - 2.0 * cs) * cb * (1.0 - cb);
            }
            return cb + (2.0 * cs - 1.0) * (b_soft_d(cb) - cb);
        }
        case 5u: { return b_hard(cb, cs); }
        case 6u: {
            if cb == 0.0 {
                return 0.0;
            }
            if cs >= 1.0 {
                return 1.0;
            }
            return min(cb / (1.0 - cs), 1.0);
        }
        case 7u: {
            if cb >= 1.0 {
                return 1.0;
            }
            if cs <= 0.0 {
                return 0.0;
            }
            return 1.0 - min((1.0 - cb) / cs, 1.0);
        }
        case 8u: { return min(cb, cs); }
        case 9u: { return max(cb, cs); }
        case 10u: { return abs(cb - cs); }
        case 11u: { return cb + cs - 2.0 * cb * cs; }
        case 12u: { return min(cb + cs, 1.0); }
        case 13u: { return max(cb - cs, 0.0); }
        default: { return cs; }
    }
}

fn b_blend(mode: u32, cb: vec3<f32>, cs: vec3<f32>) -> vec3<f32> {
    switch mode {
        case 14u: { return b_set_lum(b_set_sat(cs, b_sat(cb)), b_lum(cb)); }
        case 15u: { return b_set_lum(b_set_sat(cb, b_sat(cs)), b_lum(cb)); }
        case 16u: { return b_set_lum(cs, b_lum(cb)); }
        case 17u: { return b_set_lum(cb, b_lum(cs)); }
        default: { return vec3<f32>(b_channel(mode, cb.x, cs.x), b_channel(mode, cb.y, cs.y), b_channel(mode, cb.z, cs.z)); }
    }
}

// `top` over `below` (linear RGB, straight alpha); `scale` multiplies the top's alpha.
fn b_composite(below: vec4<f32>, top: vec4<f32>, mode: u32, scale: f32) -> vec4<f32> {
    let a_s = clamp(top.w * scale, 0.0, 1.0);
    let a_b = clamp(below.w, 0.0, 1.0);
    if a_s <= 0.0 {
        return below;
    }
    let cb = vec3<f32>(srgb_enc(below.x), srgb_enc(below.y), srgb_enc(below.z));
    let cs = vec3<f32>(srgb_enc(top.x), srgb_enc(top.y), srgb_enc(top.z));
    let b = b_blend(mode, cb, cs);
    let a_o = a_s + a_b * (1.0 - a_s);
    let mixed = (1.0 - a_b) * cs + a_b * b;
    let co = a_s * mixed + (1.0 - a_s) * a_b * cb;
    var c = vec3<f32>(0.0);
    if a_o > 0.0 {
        c = co / a_o;
    }
    return vec4<f32>(srgb_dec(c.x), srgb_dec(c.y), srgb_dec(c.z), a_o);
}

// Value of a mask color: luminance × alpha (source 0) or alpha (1), inverted on request.
fn b_mask(c: vec4<f32>, source: u32, invert: u32) -> f32 {
    var m = c.w;
    if source == 0u {
        m = (0.2126 * c.x + 0.7152 * c.y + 0.0722 * c.z) * c.w;
    }
    m = clamp(m, 0.0, 1.0);
    if invert != 0u {
        return 1.0 - m;
    }
    return m;
}
