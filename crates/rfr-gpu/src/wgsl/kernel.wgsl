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
    // Perturbation: plane units per pixel (mantissa), view center minus reference point
    // (complex floatexp parts), unused
    pert: vec4<f32>,
    // Perturbation: plane units per pixel (power of two), reference orbit length, the center
    // offset's power of two, unused
    perti: vec4<i32>,
    // Perturbation: the reference point (f32), unused…
    pref: vec4<f32>,
    // Density layers (density.wgsl): start region x0, y0, x1, y1
    dens: vec4<f32>,
    // mode, minimum orbit length, warm-up, orbits per pass
    densi: vec4<u32>,
    // Nebulabrot limits r, g, b, pass index
    densn: vec4<u32>,
    // channels, iteration limit, Nebulabrot, unused
    densx: vec4<u32>,
    // brightness, gamma, stroke width, Metropolis
    densb: vec4<f32>,
    // Lit layers (lighting.wgsl): ambient, slope scale, specular, roughness
    light: vec4<f32>,
    // height source, number of lights, specular model, 3D terrain
    lighti: vec4<u32>,
    // per light: direction toward it (x, y, z, unused) and color times intensity (r, g, b, unused)
    ldir: array<vec4<f32>, 4>,
    lrad: array<vec4<f32>, 4>,
    // shadow length (pixels, 0: none), softness, occlusion strength, occlusion radius (pixels)
    lshadow: vec4<f32>,
    // reflection, environment image slot (-1: the sky colors), unused…
    lenv: vec4<f32>,
    // sky at the horizon and at the zenith (linear RGB)
    lsky0: vec4<f32>,
    lsky1: vec4<f32>,
    // 3D terrain camera: direction and pitch (radians), distance (image heights), tan(fov / 2)
    lcam: vec4<f32>,
}

struct Params {
    p: array<vec4<f32>, 4096>,
}

@group(0) @binding(0) var<uniform> U: Uni;
@group(0) @binding(1) var<uniform> P: Params;
// Outside gradient in entries 0..1023, inside gradient in 1024..2047.
@group(0) @binding(2) var<storage, read> lut: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read_write> scratch: array<vec4<f32>>;
// Images of image parameters (rfr_core::images::pack): per image a header (width, height,
// pixel offset, original width << 16 | original height), then packed sRGB RGBA8 pixels.
@group(0) @binding(4) var<storage, read> images: array<u32>;
// The reference orbit of a deep zoom: the state variables per iteration (rfr_core RefOrbit).
@group(0) @binding(5) var<storage, read> refo: array<vec2<f32>>;
// Linear steps of the reference orbit (rfr_core BlaTable): 8 vec4 of level offsets (u32 bits),
// then 3 vec4 per step: M, N (2 × 2, row-major), (M exponent, N exponent, r mantissa, r
// exponent) with the exponents as i32 bits.
@group(0) @binding(6) var<storage, read> bla: array<vec4<f32>>;

const LUT_SIZE: u32 = 1024u;
// The last region color's gradient position before the offset (lit layers' heights).
var<private> g_value: f32;
//@RNG_DECL
const TAU: f32 = 6.283185307179586;

fn hash(v: u32) -> u32 {
    let state = v * 747796405u + 2891336453u;
    let word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    return (word >> 22u) ^ word;
}

// One step of the generator behind #random: a value in -1..1 (rfr_core::formula::cpu::rnd_next).
fn rnd_step(s: ptr<private, u32>) -> f32 {
    let st = *s * 747796405u + 2891336453u;
    *s = st;
    let word = ((st >> ((st >> 28u) + 4u)) ^ st) * 277803737u;
    let v = (word >> 22u) ^ word;
    return f32(v >> 8u) / 8388608.0 - 1.0;
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

// Formula colors: sRGB-encoded RGBA, straight alpha, hue 0..6 (rfr_core::formula::color).
fn k_hsla(h0: f32, s: f32, l: f32, a: f32) -> vec4<f32> {
    let h = h0 - 6.0 * floor(h0 / 6.0);
    let c = (1.0 - abs(2.0 * l - 1.0)) * clamp(s, 0.0, 1.0);
    let x = c * (1.0 - abs(h - 2.0 * floor(h / 2.0) - 1.0));
    let m = l - 0.5 * c;
    var rgb: vec3<f32>;
    switch u32(h) {
        case 0u: { rgb = vec3<f32>(c, x, 0.0); }
        case 1u: { rgb = vec3<f32>(x, c, 0.0); }
        case 2u: { rgb = vec3<f32>(0.0, c, x); }
        case 3u: { rgb = vec3<f32>(0.0, x, c); }
        case 4u: { rgb = vec3<f32>(x, 0.0, c); }
        default: { rgb = vec3<f32>(c, 0.0, x); }
    }
    return vec4<f32>(rgb + vec3<f32>(m), a);
}

fn k_hsl(c: vec4<f32>) -> vec3<f32> {
    let mx = max(max(c.x, c.y), c.z);
    let mn = min(min(c.x, c.y), c.z);
    let l = 0.5 * (mx + mn);
    let d = mx - mn;
    if d <= 1e-12 {
        return vec3<f32>(0.0, 0.0, l);
    }
    let s = d / max(1.0 - abs(2.0 * l - 1.0), 1e-12);
    var h: f32;
    if mx == c.x {
        let t = (c.y - c.z) / d;
        h = t - 6.0 * floor(t / 6.0);
    } else if mx == c.y {
        h = (c.z - c.x) / d + 2.0;
    } else {
        h = (c.x - c.y) / d + 4.0;
    }
    return vec3<f32>(h, s, l);
}

fn k_compose(b: vec4<f32>, t: vec4<f32>, amount: f32) -> vec4<f32> {
    let a = t.w * amount;
    return vec4<f32>(b.xyz + (t.xyz - b.xyz) * a, b.w + a * (1.0 - b.w));
}

fn k_merge(mode: u32, b: vec4<f32>, t: vec4<f32>) -> vec4<f32> {
    return vec4<f32>(b_blend(mode, b.xyz, t.xyz), t.w);
}

fn k_img_px(off: u32, w: u32, x: u32, y: u32) -> vec4<f32> {
    return unpack4x8unorm(images[off + y * w + x]);
}

// image.getColor(z): the image covers (-1, -1)..(1, 1), top row at im = 1; bilinear;
// transparent outside (rfr_core::images::color).
fn k_image_color(slot: u32, z: Cx) -> vec4<f32> {
    let w = images[slot * 4u];
    let h = images[slot * 4u + 1u];
    let off = images[slot * 4u + 2u];
    if w == 0u || h == 0u {
        return vec4<f32>(0.0);
    }
    let fw = f32(w);
    let fh = f32(h);
    let u = (s_v(z.re) + 1.0) * 0.5 * fw;
    let v = (1.0 - s_v(z.im)) * 0.5 * fh;
    if !(u >= 0.0 && u < fw && v >= 0.0 && v < fh) {
        return vec4<f32>(0.0);
    }
    let x = u - 0.5;
    let y = v - 0.5;
    let x0 = floor(x);
    let y0 = floor(y);
    let xa = u32(clamp(x0, 0.0, fw - 1.0));
    let xb = u32(clamp(x0 + 1.0, 0.0, fw - 1.0));
    let ya = u32(clamp(y0, 0.0, fh - 1.0));
    let yb = u32(clamp(y0 + 1.0, 0.0, fh - 1.0));
    let top = mix(k_img_px(off, w, xa, ya), k_img_px(off, w, xb, ya), x - x0);
    let bottom = mix(k_img_px(off, w, xa, yb), k_img_px(off, w, xb, yb), x - x0);
    return mix(top, bottom, y - y0);
}

// image.getWidth(), getHeight(): the file's size (0: no image).
fn k_image_size(slot: u32) -> vec2<i32> {
    if images[slot * 4u] == 0u {
        return vec2<i32>(0);
    }
    let o = images[slot * 4u + 3u];
    return vec2<i32>(i32(o >> 16u), i32(o & 0xffffu));
}

// image.getPixel(x, y): file coordinates from the top left; transparent outside.
fn k_image_pixel(slot: u32, x: i32, y: i32) -> vec4<f32> {
    let w = images[slot * 4u];
    let h = images[slot * 4u + 1u];
    let size = k_image_size(slot);
    if w == 0u || x < 0 || y < 0 || x >= size.x || y >= size.y {
        return vec4<f32>(0.0);
    }
    let xs = u32(x) * w / u32(size.x);
    let ys = u32(y) * h / u32(size.y);
    return k_img_px(images[slot * 4u + 2u], w, xs, ys);
}

// The coloring's gradient (base: LUT offset; 0xffffffff: none, a gray ramp).
fn k_gradient(base: u32, t: f32) -> vec4<f32> {
    if base == 0xffffffffu {
        let g = fract(t);
        return vec4<f32>(g, g, g, 1.0);
    }
    let c = lut_sample(base, t);
    return vec4<f32>(srgb_enc(c.x), srgb_enc(c.y), srgb_enc(c.z), c.w);
}

// A formula's #color as a region color (linear RGB, alpha).
fn k_direct(k: vec4<f32>) -> vec4<f32> {
    let c = clamp(k, vec4<f32>(0.0), vec4<f32>(1.0));
    return finite4(vec4<f32>(srgb_dec(c.x), srgb_dec(c.y), srgb_dec(c.z), c.w));
}

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
        g_value = transfer(u32(U.inc.x), v) * U.inc.y;
        return finite4(lut_sample(LUT_SIZE, g_value + U.inc.z));
    }
    g_value = transfer(u32(U.outc.x), v) * U.outc.y;
    return finite4(lut_sample(0u, g_value + U.outc.z));
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
    let s = U.size.z;
    let rng_base = hash(x ^ hash(y ^ hash(s + 0x9E3779B9u)));
//@MODE_INIT
    // Sample position inside the pixel (R2 sequence in 32-bit fixed point) and derivative direction;
    // a lit layer's field takes pixel (0, 0)'s for all pixels (a regular grid).
    var jx = x;
    var jy = y;
    if U.band.y == 2u {
        jx = 0u;
        jy = 0u;
    }
    let ha = hash(jx ^ hash(jy + 0x68E31DA4u));
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
    var hgt = 0.0;
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
        g_value = 0.0;
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
//@HEIGHT
    }
//@FIELD
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
