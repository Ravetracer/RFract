// SPDX-License-Identifier: LGPL-3.0-or-later
//
// Density layers (rfr_core::render::density), appended to the part of kernel.wgsl before its
// main: `scatter` iterates orbits of the layer's formula from random start points and counts
// their points per pixel, `dmax` finds the largest count per channel, `main` tone-maps the
// counts into the layer's color and blends it like any layer. codegen.rs fills the //@ markers.

// Counts per channel and pixel, then the largest count per channel.
@group(0) @binding(7) var<storage, read_write> hist: array<atomic<u32>>;

// The image position of a plane point (whole-image pixels, rfr_core::render::density::pixel_of).
fn d_pos(z: Cx) -> vec2<f32> {
    let s = U.view.z;
    let dx = s_v(z.re) - U.view.x;
    let dy = s_v(z.im) - U.view.y;
    let u = (dx * U.rot.x + dy * U.rot.y) / s;
    let v = (-dx * U.rot.y + dy * U.rot.x) / s;
    return vec2<f32>(u + f32(U.win.z) * 0.5, f32(U.win.w) * 0.5 - v);
}

// Whether a plane point falls into the image (Metropolis scores count the whole image).
fn d_in_view(z: Cx) -> bool {
    let p = d_pos(z);
    return p.x >= 0.0 && p.y >= 0.0 && p.x < f32(U.win.z) && p.y < f32(U.win.w);
}

// The index of a plane point's pixel in the window's counts (Histogram::add), or 0xffffffff
// outside the window.
fn d_pixel(z: Cx) -> u32 {
    if !d_in_view(z) {
        return 0xffffffffu;
    }
    let p = d_pos(z);
    let x = i32(u32(p.x)) - U.win.x;
    let y = i32(u32(p.y)) - U.win.y;
    if x < 0 || y < 0 || x >= i32(U.size.x) || y >= i32(U.size.y) {
        return 0xffffffffu;
    }
    return u32(y) * U.size.x + u32(x);
}

// Adds `w` to the pixel of z in the channels of `mask`.
fn d_plot(z: Cx, mask: u32, w: u32) {
    let at = d_pixel(z);
    if at == 0xffffffffu {
        return;
    }
    let plane = U.size.x * U.size.y;
    for (var c = 0u; c < U.densx.x; c = c + 1u) {
        if (mask & (1u << c)) != 0u {
            atomicAdd(&hist[c * plane + at], w);
        }
    }
}

// An IFS map's variation (rfr_core::doc::Variation::apply), theta = atan2(x, y).
fn d_var(v: u32, x: f32, y: f32) -> vec2<f32> {
    let r2 = x * x + y * y;
    let r = sqrt(r2);
    let th = atan2(x, y);
    let pi = 3.141592653589793;
    switch v {
        case 1u: { return vec2<f32>(sin(x), sin(y)); }
        case 2u: { return vec2<f32>(x, y) / (r2 + 1e-12); }
        case 3u: { return vec2<f32>(x * sin(r2) - y * cos(r2), x * cos(r2) + y * sin(r2)); }
        case 4u: { return vec2<f32>((x - y) * (x + y), 2.0 * x * y) / (r + 1e-12); }
        case 5u: { return vec2<f32>(th / pi, r - 1.0); }
        case 6u: { return vec2<f32>(r * sin(th + r), r * cos(th - r)); }
        case 7u: { return vec2<f32>(r * sin(th * r), -r * cos(th * r)); }
        case 8u: { return vec2<f32>(th / pi * sin(pi * r), th / pi * cos(pi * r)); }
        case 9u: { return vec2<f32>(cos(th) + sin(r), sin(th) - cos(r)) / (r + 1e-12); }
        default: { return vec2<f32>(x, y); }
    }
}

// The chaos game over the maps in `bla` (rfr_core::render::density::ifs_orbit): a map per step
// by the cumulative shares, the same hashed random numbers as the CPU.
fn d_ifs(i: u32, start: vec2<f32>) {
    let n = U.densx.w;
    if n == 0u {
        return;
    }
    var p = start;
    var rnd = hash(i ^ hash(U.densn.w ^ 0xA511E9B3u));
    for (var it = 0u; it < U.densx.y; it = it + 1u) {
        rnd = hash(rnd);
        let u = f32(rnd) / 4294967296.0;
        var k = n - 1u;
        for (var j = 0u; j < n; j = j + 1u) {
            if u < bla[2u * j + 1u].z {
                k = j;
                break;
            }
        }
        let m0 = bla[2u * k];
        let m1 = bla[2u * k + 1u];
        p = d_var(u32(m1.w), m0.x * p.x + m0.y * p.y + m1.x, m0.z * p.x + m0.w * p.y + m1.y);
        if !(abs(p.x) < 1e30 && abs(p.y) < 1e30) {
            return;
        }
        if it >= U.densi.z {
            d_plot(c_f(p.x, p.y), 1u, 1u);
        }
    }
}

fn c2_mul(a: vec2<f32>, b: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(a.x * b.x - a.y * b.y, a.x * b.y + a.y * b.x);
}

// A random walk over the Kleinian generators in `bla` (rfr_core::render::density::
// kleinian_orbit): never the inverse of the step before, the same hashed choices as the CPU.
fn d_klein(i: u32, start: vec2<f32>) {
    var z = start;
    var rnd = hash(i ^ hash(U.densn.w ^ 0x7F4A7C15u));
    var prev = 4u;
    for (var it = 0u; it < U.densx.y; it = it + 1u) {
        rnd = hash(rnd);
        var k = (rnd >> 8u) % 3u;
        var banned = 4u;
        if prev < 4u {
            banned = (prev + 2u) % 4u;
        }
        if banned < 4u && k >= banned {
            k = k + 1u;
        }
        if banned == 4u {
            k = (rnd >> 8u) % 4u;
        }
        prev = k;
        let m0 = bla[2u * k];
        let m1 = bla[2u * k + 1u];
        let den = c2_mul(m1.xy, z) + m1.zw;
        let dn = dot(den, den);
        if dn < 1e-30 {
            z = vec2<f32>(1e10, 0.0);
            continue;
        }
        let num = c2_mul(m0.xy, z) + m0.zw;
        z = vec2<f32>(num.x * den.x + num.y * den.y, num.y * den.x - num.x * den.y) / dn;
        if !(abs(z.x) < 1e30 && abs(z.y) < 1e30) {
            return;
        }
        if it >= U.densi.z {
            d_plot(c_f(z.x, z.y), 1u, 1u);
        }
    }
}

// Points on an L-system's strokes in `bla` (rfr_core::render::density::lsystem_orbit): a
// segment by its cumulative share, a place along it and across the stroke width.
fn d_lsys(i: u32) {
    let n = U.densx.w;
    if n == 0u {
        return;
    }
    let w = U.densb.z;
    var rnd = hash(i ^ hash(U.densn.w ^ 0x3C6EF372u));
    for (var it = 0u; it < U.densx.y; it = it + 1u) {
        rnd = hash(rnd);
        let u = f32(rnd) / 4294967296.0;
        rnd = hash(rnd);
        let t = f32(rnd) / 4294967296.0;
        rnd = hash(rnd);
        let v = f32(rnd) / 4294967296.0 - 0.5;
        var lo = 0u;
        var hi = n - 1u;
        loop {
            if lo >= hi {
                break;
            }
            let mid = (lo + hi) / 2u;
            if bla[2u * mid + 1u].x > u {
                hi = mid;
            } else {
                lo = mid + 1u;
            }
        }
        let g = bla[2u * lo];
        let d = g.zw - g.xy;
        let len = max(length(d), 1e-12);
        let nrm = vec2<f32>(-d.y, d.x) / len;
        let p = g.xy + d * t + nrm * (v * w);
        d_plot(c_f(p.x, p.y), 1u, 1u);
    }
}

@compute @workgroup_size(64)
fn scatter(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if i >= U.densi.w {
        return;
    }
    let dpass = U.densn.w;
    let ha = hash(i ^ hash(dpass * 0x9E3779B9u ^ 0x5bd1e995u));
    let hb = hash(ha ^ 0x68E31DA4u);
    let su = f32(ha) / 4294967296.0;
    let sv = f32(hb) / 4294967296.0;
    let start = vec2<f32>(U.dens.x + (U.dens.z - U.dens.x) * su, U.dens.y + (U.dens.w - U.dens.y) * sv);
    if U.densi.x == 3u {
        d_ifs(i, start);
        return;
    }
    if U.densi.x == 4u {
        d_klein(i, start);
        return;
    }
    if U.densi.x == 5u {
        d_lsys(i);
        return;
    }
    // Metropolis chains (rfr_core::render::density, scatter_orbit): the state of orbit i lies
    // after the counts and maxima (start point bits, points in the view, channels in the low
    // three bits and the score of the chain's first state above them).
    let mh = U.densb.w > 0.5 && U.densi.x < 2u;
    var r = array<f32, 5>(0.0, 0.0, 0.0, 0.0, 0.0);
    var rr = hb ^ 0x27D4EB2Fu;
    for (var k = 0u; k < 5u; k = k + 1u) {
        rr = hash(rr);
        r[k] = f32(rr) / 4294967296.0;
    }
    let chain_at = U.densx.x * U.size.x * U.size.y + 4u + 4u * i;
    var cur = vec2<f32>(0.0);
    var cur_score = 0u;
    var cur_mask = 0u;
    var cur_first = 0u;
    var prop = start;
    if mh {
        cur = vec2<f32>(bitcast<f32>(atomicLoad(&hist[chain_at])), bitcast<f32>(atomicLoad(&hist[chain_at + 1u])));
        cur_score = atomicLoad(&hist[chain_at + 2u]);
        let packed = atomicLoad(&hist[chain_at + 3u]);
        cur_mask = packed & 7u;
        cur_first = packed >> 3u;
        if cur_score > 0u && r[0] >= 0.1 {
            let size = max(abs(U.dens.z - U.dens.x), abs(U.dens.w - U.dens.y));
            let len = size * 0.1 * pow(1e-4, r[1]);
            let a = 6.283185307179586 * r[2];
            prop = cur + len * vec2<f32>(cos(a), sin(a));
        }
    }
    var score = 0u;
    var weight = 1u;
    let x = 0u;
    let y = 0u;
    let ismand = U.iter.y == 0u;
    let center = c_f(U.view.x, U.view.y);
    let magn = s_c(U.view.w);
    let maxit = U.densx.y;
    var pixel = c_f(prop.x, prop.y);
    var cval = pixel;
    if !ismand {
        cval = c_f(U.seed.x, U.seed.y);
    }
    let rng_base = hash(i ^ 0x2545F491u);
    var record = U.densi.x == 2u;
    var mask = 1u;
    var skip = 0u;
    if U.densi.x == 2u {
        skip = U.densi.z;
    } else {
//@MODE_INIT
//@LOCALS
        var zprev = c_f(0.0, 0.0);
        var zprev2 = c_f(0.0, 0.0);
        var lastsqr = s_c(0.0);
        var numiter = 0;
//@INIT
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
            if mh && d_in_view(l0) {
                score = score + 1u;
            }
        }
        if U.densi.x == 0u {
            record = escaped && iter >= U.densi.y;
            if U.densx.z != 0u {
                mask = 0u;
                for (var c = 0u; c < 3u; c = c + 1u) {
                    if iter <= U.densn[c] {
                        mask = mask | (1u << c);
                    }
                }
            }
        } else {
            record = !escaped;
            if U.densx.z != 0u {
                mask = 7u;
            }
        }
        record = record && mask != 0u;
        if mh {
            let lo = min(U.dens.xy, U.dens.zw);
            let hi = max(U.dens.xy, U.dens.zw);
            if !(record && all(prop >= lo) && all(prop <= hi)) {
                score = 0u;
            }
            if score > 0u && (cur_score == 0u || r[3] * f32(cur_score) < f32(score)) {
                if cur_score == 0u {
                    cur_first = score;
                }
                cur = prop;
                cur_score = score;
                cur_mask = mask;
                atomicStore(&hist[chain_at], bitcast<u32>(cur.x));
                atomicStore(&hist[chain_at + 1u], bitcast<u32>(cur.y));
                atomicStore(&hist[chain_at + 2u], cur_score);
                atomicStore(&hist[chain_at + 3u], cur_mask | (cur_first << 3u));
            }
            record = false;
            if cur_score > 0u {
                weight = u32(f32(cur_first) / f32(cur_score) + r[4]);
                record = weight > 0u;
            }
            mask = cur_mask;
            pixel = c_f(cur.x, cur.y);
            if ismand {
                cval = pixel;
            }
        }
    }
    if record {
//@MODE_INIT
//@LOCALS
        var zprev = c_f(0.0, 0.0);
        var zprev2 = c_f(0.0, 0.0);
        var lastsqr = s_c(0.0);
        var numiter = 0;
//@INIT
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
                break;
            }
            if iter > skip {
                d_plot(l0, mask, weight);
            }
        }
    }
}

var<workgroup> wmax: array<u32, 256>;

@compute @workgroup_size(256)
fn dmax(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let plane = U.size.x * U.size.y;
    for (var c = 0u; c < U.densx.x; c = c + 1u) {
        var v = 0u;
        if gid.x < plane {
            v = atomicLoad(&hist[c * plane + gid.x]);
        }
        wmax[li] = v;
        workgroupBarrier();
        for (var s = 128u; s > 0u; s = s >> 1u) {
            if li < s {
                wmax[li] = max(wmax[li], wmax[li + s]);
            }
            workgroupBarrier();
        }
        if li == 0u {
            atomicMax(&hist[U.densx.x * plane + c], wmax[0]);
        }
        workgroupBarrier();
    }
}

// Log density relative to the largest count, times the brightness (clamped), then gamma.
fn d_tone(count: u32, mx: u32) -> f32 {
    if count == 0u || mx == 0u {
        return 0.0;
    }
    let v = min(log(1.0 + f32(count)) / log(1.0 + f32(mx)) * max(U.densb.x, 0.0), 1.0);
    return pow(v, 1.0 / max(U.densb.y, 0.05));
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y + U.size.w;
    if x >= U.size.x || gid.y >= U.band.x || y >= U.size.y {
        return;
    }
    let plane = U.size.x * U.size.y;
    let at = y * U.size.x + x;
    let ch = U.densx.x;
    var col = vec4<f32>(0.0);
    if ch == 3u {
        let rgb = vec3<f32>(
            d_tone(atomicLoad(&hist[at]), atomicLoad(&hist[3u * plane])),
            d_tone(atomicLoad(&hist[plane + at]), atomicLoad(&hist[3u * plane + 1u])),
            d_tone(atomicLoad(&hist[2u * plane + at]), atomicLoad(&hist[3u * plane + 2u])),
        );
        let a = max(rgb.x, max(rgb.y, rgb.z));
        if a > 0.0 {
            col = vec4<f32>(rgb / a, a);
        }
    } else {
        let v = d_tone(atomicLoad(&hist[at]), atomicLoad(&hist[plane]));
        if v > 0.0 {
            // The gradient's last entry at full density (the lookup wraps at 1).
            let c = lut_sample(0u, v * (1.0 - 1.0 / f32(LUT_SIZE)));
            col = vec4<f32>(c.xyz, c.w * v);
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
