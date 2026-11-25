// SPDX-License-Identifier: LGPL-3.0-or-later
//
// One sample of one layer per pixel of a row band: transformations, the fractal with its
// coloring formulas, the shading, then blending into a scratch slot (or writing a mask).
// Mirrors rfr_core::render::{sample_offset, ViewFrame::plane, smooth_iter, outside_value,
// inside_value, shade_native, shade_formula} and rfr_core::render::cpu (LayerK::eval).
// codegen.rs fills the //@ markers.

struct Uni {
    // width, height, sample index, first row of the band
    size: vec4<u32>,
    // center x, center y, plane units per pixel (radius / (height / 2)), magnification
    view: vec4<f32>,
    // cos, sin of the rotation, pixel size, 1.0 (double-float guard)
    rot: vec4<f32>,
    // max iterations, julia mode, outside method, inside method
    iter: vec4<u32>,
    // seed x, seed y, unused, unused
    seed: vec4<f32>,
    // transfer, density, offset, unused (outside, inside)
    outc: vec4<f32>,
    inc: vec4<f32>,
    // solid colors (linear RGB, alpha)
    out_solid: vec4<f32>,
    in_solid: vec4<f32>,
    // rows in this band, operation (0 layer, 1 mask), destination slot, mask slot (0xffffffff: none)
    band: vec4<u32>,
    // low parts of the center (double-float mode), unused…
    lo: vec4<f32>,
    // merge mode, slot size (pixels), mask source, mask invert
    blend: vec4<u32>,
    // opacity, unused…
    opacity: vec4<f32>,
}

struct Params {
    p: array<vec4<f32>, 256>,
}

@group(0) @binding(0) var<uniform> U: Uni;
@group(0) @binding(1) var<uniform> P: Params;
// Outside gradient in entries 0..1023, inside gradient in 1024..2047.
@group(0) @binding(2) var<storage, read> lut: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read_write> scratch: array<vec4<f32>>;

const LUT_SIZE: u32 = 1024u;
const TAU: f32 = 6.283185307179586;

fn hash(v: u32) -> u32 {
    let state = v * 747796405u + 2891336453u;
    let word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    return (word >> 22u) ^ word;
}

fn transfer(kind: u32, v: f32) -> f32 {
    switch kind {
        case 1u: { return sqrt(max(v, 0.0)); }
        case 2u: { return sign(v) * pow(abs(v), 1.0 / 3.0); }
        case 3u: { return log(1.0 + max(v, 0.0)); }
        case 4u: { return v * v; }
        default: { return v; }
    }
}

fn lut_sample(base: u32, t: f32) -> vec4<f32> {
    let x = fract(t) * f32(LUT_SIZE);
    let i = min(u32(floor(x)), LUT_SIZE - 1u);
    let j = (i + 1u) % LUT_SIZE;
    let u = x - floor(x);
    return mix(lut[base + i], lut[base + j], u);
}

fn norm2(v: vec2<f32>) -> f32 { return dot(v, v); }

fn smooth_iter(z: vec2<f32>, zp: vec2<f32>, zp2: vec2<f32>, iter: u32) -> f32 {
    let n = f32(iter);
    let r2 = norm2(z);
    let p2 = norm2(zp);
    var v: f32;
    if r2 > 4.0 && r2 > p2 {
        let lz = 0.5 * log(r2);
        let lp = 0.5 * log(p2);
        var p = 2.0;
        if lp > 1e-3 {
            p = clamp(lz / lp, 1.01, 64.0);
        }
        if abs(p - round(p)) < 0.1 {
            p = round(p);
        }
        v = n - log(max(lz, 1e-30)) / log(p);
    } else if iter >= 2u {
        let d2 = max(norm2(zp - zp2), 1e-30);
        let l = max(-0.5 * log(d2), 1e-30);
        v = n - 1.0 - log2(l);
    } else {
        let d2 = max(norm2(z - zp), 1e-30);
        let l = max(-0.5 * log(d2), 1e-30);
        v = n - log2(l);
    }
    if abs(v) <= 3.4e38 {
        return max(v + 2.0, 0.0);
    }
    return n;
}

fn finite4(c: vec4<f32>) -> vec4<f32> {
    if all(abs(c) <= vec4<f32>(3.4e38)) {
        return c;
    }
    return vec4<f32>(0.0, 0.0, 0.0, 1.0);
}

fn region_color(inside: bool, v: f32) -> vec4<f32> {
    if inside {
        return finite4(lut_sample(LUT_SIZE, transfer(u32(U.inc.x), v) * U.inc.y + U.inc.z));
    }
    return finite4(lut_sample(0u, transfer(u32(U.outc.x), v) * U.outc.y + U.outc.z));
}

// Native coloring methods: 0 iterations, 1 smooth, 2 distance, 3 solid, 4 magnitude, 5 angle.
fn shade_native(inside: bool, z: vec2<f32>, zp: vec2<f32>, zp2: vec2<f32>, iter: u32, dz: f32) -> vec4<f32> {
    var method = U.iter.z;
    if inside {
        method = U.iter.w;
    }
    var v = 0.0;
    switch method {
        case 0u: { v = f32(iter); }
        case 2u: {
            let r2 = norm2(z);
            if r2 > 4.0 && dz > 0.0 && dz <= 3.4e38 {
                let r = sqrt(r2);
                let de = r * log(r) / dz;
                v = log(1.0 + max(de / U.rot.z, 0.0));
            } else {
                v = smooth_iter(z, zp, zp2, iter);
            }
        }
        case 3u, 6u: {
            if inside {
                return U.in_solid;
            }
            return U.out_solid;
        }
        case 4u: { v = sqrt(norm2(z)); }
        case 5u: { v = atan2(z.y, z.x) / TAU + 0.5; }
        default: { v = smooth_iter(z, zp, zp2, iter); }
    }
    return region_color(inside, v);
}

fn shade_formula(inside: bool, index: f32, solid: bool) -> vec4<f32> {
    if solid {
        if inside {
            return U.in_solid;
        }
        return U.out_solid;
    }
    return region_color(inside, index);
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y + U.size.w;
    if x >= U.size.x || gid.y >= U.band.x || y >= U.size.y {
        return;
    }
//@MODE_INIT
    let s = U.size.z;
    // Sample position inside the pixel (R2 sequence in 32-bit fixed point) and derivative direction.
    let ha = hash(x ^ hash(y + 0x68E31DA4u));
    let hb = hash(ha);
    let hc = hash(hb);
    let ju = f32(ha + s * 0xC13FA9A9u) / 4294967296.0;
    let jv = f32(hb + s * 0x91E10DA6u) / 4294967296.0;
    let turn = f32(hc + s * 0x9E3779B9u) / 4294967296.0;
    let w = f32(U.size.x);
    let h = f32(U.size.y);
    let dx = (f32(x) + ju - w * 0.5) * U.view.z;
    let dy = -(f32(y) + jv - h * 0.5) * U.view.z;
    let off = vec2<f32>(dx * U.rot.x - dy * U.rot.y, dx * U.rot.y + dy * U.rot.x);
    let pr = U.view.xy + off;
//@PIXEL
    let ismand = U.iter.y == 0u;
    let center = c_f(U.view.x, U.view.y);
    let magn = s_c(U.view.w);
    let maxit = U.iter.x;
    var tsolid = false;
//@TRANSFORMS
    var col = vec4<f32>(0.0);
    if !tsolid {
        var cval = pixel;
        if !ismand {
            cval = c_f(U.seed.x, U.seed.y);
        }
//@LOCALS
        var zprev = c_f(0.0, 0.0);
        var zprev2 = c_f(0.0, 0.0);
        var lastsqr = s_c(0.0);
        var numiter = 0;
//@INIT
//@COLOR_INIT
        var escaped = false;
        var iter = 0u;
        loop {
            if iter >= maxit {
                break;
            }
            zprev2 = zprev;
            zprev = l0;
//@BODY
            iter = iter + 1u;
            numiter = i32(iter);
            lastsqr = c_norm(l0);
            if !c_finite(l0) || !(//@BAILOUT) {
                escaped = true;
                break;
            }
//@COLOR_LOOP
        }
//@FINAL
        let zv = vec2<f32>(s_v(l0.re), s_v(l0.im));
        let zpv = vec2<f32>(s_v(zprev.re), s_v(zprev.im));
        let zp2v = vec2<f32>(s_v(zprev2.re), s_v(zprev2.im));
        let dzv = sqrt(s_d(l0.re) * s_d(l0.re) + s_d(l0.im) * s_d(l0.im));
        if escaped {
            if U.iter.z == 6u {
//@OUT_FINAL
            } else {
                col = shade_native(false, zv, zpv, zp2v, iter, dzv);
            }
        } else {
            if U.iter.w == 6u {
//@IN_FINAL
            } else {
                col = shade_native(true, zv, zpv, zp2v, iter, dzv);
            }
        }
    }
    let li = gid.y * U.size.x + x;
    let dst = U.band.z * U.blend.y + li;
    if U.band.y == 1u {
        scratch[dst] = vec4<f32>(b_mask(col, U.blend.z, U.blend.w));
        return;
    }
    var scale = U.opacity.x;
    if U.band.w != 0xffffffffu {
        scale = scale * scratch[U.band.w * U.blend.y + li].x;
    }
    scratch[dst] = b_composite(scratch[dst], col, U.blend.x, scale);
}
