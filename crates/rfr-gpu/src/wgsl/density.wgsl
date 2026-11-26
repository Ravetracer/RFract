// SPDX-License-Identifier: LGPL-3.0-or-later
//
// Density layers (rfr_core::render::density), appended to the part of kernel.wgsl before its
// main: `scatter` iterates orbits of the layer's formula from random start points and counts
// their points per pixel, `dmax` finds the largest count per channel, `main` tone-maps the
// counts into the layer's color and blends it like any layer. codegen.rs fills the //@ markers.

// Counts per channel and pixel, then the largest count per channel.
@group(0) @binding(7) var<storage, read_write> hist: array<atomic<u32>>;

fn d_plot(z: Cx, mask: u32) {
    let s = U.view.z;
    let dx = s_v(z.re) - U.view.x;
    let dy = s_v(z.im) - U.view.y;
    let u = (dx * U.rot.x + dy * U.rot.y) / s;
    let v = (-dx * U.rot.y + dy * U.rot.x) / s;
    let px = u + f32(U.size.x) * 0.5;
    let py = f32(U.size.y) * 0.5 - v;
    if !(px >= 0.0 && py >= 0.0 && px < f32(U.size.x) && py < f32(U.size.y)) {
        return;
    }
    let at = u32(py) * U.size.x + u32(px);
    let plane = U.size.x * U.size.y;
    for (var c = 0u; c < U.densx.x; c = c + 1u) {
        if (mask & (1u << c)) != 0u {
            atomicAdd(&hist[c * plane + at], 1u);
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
            d_plot(c_f(p.x, p.y), 1u);
        }
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
    let x = 0u;
    let y = 0u;
    let ismand = U.iter.y == 0u;
    let center = c_f(U.view.x, U.view.y);
    let magn = s_c(U.view.w);
    let maxit = U.densx.y;
    var pixel = c_f(start.x, start.y);
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
                d_plot(l0, mask);
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
