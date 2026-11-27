// SPDX-License-Identifier: LGPL-3.0-or-later
//
// Accumulated premultiplied linear RGBA → sRGB-encoded 8-bit texture for the preview, over a
// checkerboard where the image is transparent. With effects (rfr_core::render::post::apply,
// step by step): sharpening, bloom and streaks (levels from bloom.wgsl), lens distortion and
// chromatic aberration, then the grading of straight colors.

struct Disp {
    // width, height, completed samples, rows of the current sample done
    size: vec4<u32>,
    // exposure factor, contrast, saturation, tone map (0 none, 1 Reinhard, 2 ACES, 3 filmic, 4 AgX)
    a: vec4<f32>,
    // vignette, grain, sharpen, distortion
    b: vec4<f32>,
    // white balance factors, chromatic aberration
    wb: vec4<f32>,
    // split toning: shadow color and amount, highlight color and amount
    shadow: vec4<f32>,
    highlight: vec4<f32>,
    // bloom strength (0: off), pyramid levels, unused, effects on (0: plain display)
    bloom: vec4<f32>,
    // bloom levels: first texel, width, height; their weights
    lv: array<vec4<u32>, 6>,
    lw: array<vec4<f32>, 6>,
    // streak tint times strength, on; streak level
    streak: vec4<f32>,
    slv: vec4<u32>,
}

@group(0) @binding(0) var<uniform> D: Disp;
@group(0) @binding(1) var<storage, read> accum: array<vec4<f32>>;
@group(0) @binding(2) var out_tex: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(3) var<storage, read> levels: array<vec4<f32>>;

fn to_srgb(v: f32) -> f32 {
    let c = clamp(v, 0.0, 1.0);
    if c <= 0.0031308 {
        return c * 12.92;
    }
    return 1.055 * pow(c, 1.0 / 2.4) - 0.055;
}

// rfr_core::render::hash (PCG).
fn hash(v: u32) -> u32 {
    let state = v * 747796405u + 2891336453u;
    let word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    return (word >> 22u) ^ word;
}

// The averaged premultiplied light of pixel (x, y), as CpuRenderer::linear then premultiplied.
fn pre(x: u32, y: u32) -> vec4<f32> {
    var n = D.size.z;
    if y < D.size.w {
        n = n + 1u;
    }
    let s = accum[y * D.size.x + x] / f32(max(n, 1u));
    let a = clamp(s.w, 0.0, 1.0);
    if !(s.w > 1e-6) {
        return vec4<f32>(0.0);
    }
    if !(abs(s.x) < 3.4e38 && abs(s.y) < 3.4e38 && abs(s.z) < 3.4e38) {
        return vec4<f32>(0.0, 0.0, 0.0, a);
    }
    // straight × clamped alpha
    return vec4<f32>(max(s.xyz / s.w, vec3<f32>(0.0)) * a, a);
}

fn level_sample(l: vec4<u32>, u: f32, v: f32) -> vec3<f32> {
    let fx = clamp(u * f32(l.y) - 0.5, 0.0, f32(l.y) - 1.0);
    let fy = clamp(v * f32(l.z) - 0.5, 0.0, f32(l.z) - 1.0);
    let x0 = u32(floor(fx));
    let y0 = u32(floor(fy));
    let x1 = min(x0 + 1u, l.y - 1u);
    let y1 = min(y0 + 1u, l.z - 1u);
    let tx = fx - f32(x0);
    let ty = fy - f32(y0);
    let a = levels[l.x + y0 * l.y + x0].xyz;
    let b = levels[l.x + y0 * l.y + x1].xyz;
    let c = levels[l.x + y1 * l.y + x0].xyz;
    let d = levels[l.x + y1 * l.y + x1].xyz;
    let top = a + (b - a) * tx;
    let bottom = c + (d - c) * tx;
    return top + (bottom - top) * ty;
}

// Sharpened light plus glow (post::apply `mid`).
fn mid(x: u32, y: u32) -> vec4<f32> {
    let c = pre(x, y);
    var rgb = c.xyz;
    let sharpen = D.b.z;
    if sharpen != 0.0 {
        var m = vec3<f32>(0.0);
        for (var dy = -1; dy <= 1; dy = dy + 1) {
            for (var dx = -1; dx <= 1; dx = dx + 1) {
                let qx = u32(clamp(i32(x) + dx, 0, i32(D.size.x) - 1));
                let qy = u32(clamp(i32(y) + dy, 0, i32(D.size.y) - 1));
                m = m + pre(qx, qy).xyz / 9.0;
            }
        }
        rgb = max(rgb + sharpen * (rgb - m), vec3<f32>(0.0));
    }
    let u = (f32(x) + 0.5) / f32(D.size.x);
    let v = (f32(y) + 0.5) / f32(D.size.y);
    var glow = vec3<f32>(0.0);
    if D.bloom.x > 0.0 {
        for (var k = 0u; k < u32(D.bloom.y); k = k + 1u) {
            glow = glow + level_sample(D.lv[k], u, v) * D.lw[k].x * D.bloom.x;
        }
    }
    if D.streak.w > 0.5 {
        glow = glow + level_sample(D.slv, u, v) * D.streak.xyz;
    }
    let a = c.w + (1.0 - c.w) * clamp(max(glow.x, max(glow.y, glow.z)), 0.0, 1.0);
    return vec4<f32>(rgb + glow, a);
}

fn mid_bilinear(fx_in: f32, fy_in: f32) -> vec4<f32> {
    let fx = clamp(fx_in, 0.0, f32(D.size.x) - 1.0);
    let fy = clamp(fy_in, 0.0, f32(D.size.y) - 1.0);
    let x0 = u32(floor(fx));
    let y0 = u32(floor(fy));
    let x1 = min(x0 + 1u, D.size.x - 1u);
    let y1 = min(y0 + 1u, D.size.y - 1u);
    let tx = fx - f32(x0);
    let ty = fy - f32(y0);
    let top = mix(mid(x0, y0), mid(x1, y0), tx);
    let bottom = mix(mid(x0, y1), mid(x1, y1), tx);
    return mix(top, bottom, ty);
}

fn luma(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

fn hable(x: f32) -> f32 {
    return ((x * (0.15 * x + 0.05) + 0.004) / (x * (0.15 * x + 0.5) + 0.06)) - 0.02 / 0.3;
}

// post::tone_map
fn tone_map(c: vec3<f32>) -> vec3<f32> {
    switch u32(D.a.w) {
        case 1u: { return c / (vec3<f32>(1.0) + c); }
        case 2u: { return clamp((c * (2.51 * c + 0.03)) / (c * (2.43 * c + 0.59) + 0.14), vec3<f32>(0.0), vec3<f32>(1.0)); }
        case 3u: {
            let white = hable(11.2);
            return clamp(vec3<f32>(hable(2.0 * c.x), hable(2.0 * c.y), hable(2.0 * c.z)) / white, vec3<f32>(0.0), vec3<f32>(1.0));
        }
        case 4u: {
            let m_in = mat3x3<f32>(
                vec3<f32>(0.84247906, 0.04232824, 0.042375655),
                vec3<f32>(0.0784336, 0.8784686, 0.0784336),
                vec3<f32>(0.079223745, 0.07916613, 0.87914297),
            );
            let m_out = mat3x3<f32>(
                vec3<f32>(1.196879, -0.05289685, -0.052971635),
                vec3<f32>(-0.09802088, 1.1519031, -0.09804345),
                vec3<f32>(-0.09902974, -0.09896118, 1.1510737),
            );
            let lo = -12.47393;
            let hi = 4.026069;
            var v = (clamp(log2(max(m_in * c, vec3<f32>(1e-10))), vec3<f32>(lo), vec3<f32>(hi)) - lo) / (hi - lo);
            let x2 = v * v;
            let x4 = x2 * x2;
            v = 15.5 * x4 * x2 - 40.14 * x4 * v + 31.96 * x4 - 6.868 * x2 * v + 0.4298 * x2 + 0.1191 * v - 0.00232;
            return pow(max(m_out * v, vec3<f32>(0.0)), vec3<f32>(2.2));
        }
        default: { return c; }
    }
}

fn split_tone(c: vec3<f32>) -> vec3<f32> {
    if D.shadow.w <= 0.0 && D.highlight.w <= 0.0 {
        return c;
    }
    let l = clamp(luma(c), 0.0, 1.0);
    let ts = vec3<f32>(1.0) + (D.shadow.xyz / max(luma(D.shadow.xyz), 1e-3) - vec3<f32>(1.0)) * (clamp(D.shadow.w, 0.0, 1.0) * (1.0 - l) * (1.0 - l));
    let th = vec3<f32>(1.0) + (D.highlight.xyz / max(luma(D.highlight.xyz), 1e-3) - vec3<f32>(1.0)) * (clamp(D.highlight.w, 0.0, 1.0) * l * l);
    return c * ts * th;
}

// post::grade
fn grade(c_in: vec3<f32>, x: u32, y: u32) -> vec3<f32> {
    var c = max(c_in * D.a.x * D.wb.xyz, vec3<f32>(0.0));
    c = tone_map(c);
    let l = luma(c);
    c = split_tone(vec3<f32>(l) + (c - vec3<f32>(l)) * D.a.z);
    let du = (f32(x) + 0.5) / f32(D.size.x) - 0.5;
    let dv = (f32(y) + 0.5) / f32(D.size.y) - 0.5;
    let vig = max(1.0 - D.b.x * (du * du + dv * dv) * 2.0, 0.0);
    let n = (f32(hash(x ^ hash(y ^ 0x5F3759DFu)) >> 8u) / 16777216.0 - 0.5) * D.b.y * 0.25;
    let s = vec3<f32>(to_srgb(c.x), to_srgb(c.y), to_srgb(c.z));
    return clamp(((s - vec3<f32>(0.5)) * D.a.y + vec3<f32>(0.5)) * vig + vec3<f32>(n), vec3<f32>(0.0), vec3<f32>(1.0));
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= D.size.x || gid.y >= D.size.y {
        return;
    }
    // Checkerboard (linear grays) behind transparent parts.
    let check = select(0.06, 0.11, ((gid.x / 8u) + (gid.y / 8u)) % 2u == 0u);
    if D.bloom.w < 0.5 {
        var n = D.size.z;
        if gid.y < D.size.w {
            n = n + 1u;
        }
        let a = accum[gid.y * D.size.x + gid.x] / f32(max(n, 1u));
        let c = a.xyz + vec3<f32>(check) * (1.0 - clamp(a.w, 0.0, 1.0));
        textureStore(out_tex, vec2<i32>(i32(gid.x), i32(gid.y)), vec4<f32>(to_srgb(c.x), to_srgb(c.y), to_srgb(c.z), 1.0));
        return;
    }
    var c: vec4<f32>;
    let dist = D.b.w;
    let ca = D.wb.w;
    if dist == 0.0 && ca == 0.0 {
        c = mid(gid.x, gid.y);
    } else {
        let cx = 0.5 * (f32(D.size.x) - 1.0);
        let cy = 0.5 * (f32(D.size.y) - 1.0);
        let dx = f32(gid.x) - cx;
        let dy = f32(gid.y) - cy;
        let r2 = (dx * dx + dy * dy) / max(cx * cx + cy * cy, 1e-6);
        let base = 1.0 + dist * r2;
        let g = mid_bilinear(cx + dx * base, cy + dy * base);
        let r = mid_bilinear(cx + dx * base * (1.0 + ca), cy + dy * base * (1.0 + ca)).x;
        let b = mid_bilinear(cx + dx * base * (1.0 - ca), cy + dy * base * (1.0 - ca)).z;
        c = vec4<f32>(r, g.y, b, g.w);
    }
    let a = clamp(c.w, 0.0, 1.0);
    var straight = vec3<f32>(0.0);
    if c.w > 1e-6 {
        straight = c.xyz / c.w;
    }
    let d = grade(straight, gid.x, gid.y);
    // Over the checkerboard in display values (the export keeps the alpha).
    let o = d * a + vec3<f32>(to_srgb(check)) * (1.0 - a);
    textureStore(out_tex, vec2<i32>(i32(gid.x), i32(gid.y)), vec4<f32>(o, 1.0));
}
