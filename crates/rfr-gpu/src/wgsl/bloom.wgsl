// SPDX-License-Identifier: LGPL-3.0-or-later
//
// Bloom pyramid and streaks of rfr_core::render::post (from RMandelbulb3D): the light above
// the threshold averaged into levels (4 × 4 blocks, then 2 × 2), each blurred with the 5-tap
// binomial filter; streaks smeared along their directions. display.wgsl adds them to the image.

struct BP {
    dst: vec4<u32>, // first texel, width, height of the level written
    src: vec4<u32>, // first texel, width, height of the level read
    img: vec4<u32>, // image width, height, completed samples, rows of the current sample done
    prm: vec4<f32>, // exposure factor, threshold, streak: 0 write / 1 add
    prm2: vec4<f32>, // streak direction (x, y), step, falloff per texel
}

@group(0) @binding(0) var<uniform> bp: BP;
@group(0) @binding(1) var<storage, read> accum: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read_write> levels: array<vec4<f32>>;
@group(0) @binding(4) var<storage, read_write> tmp: array<vec4<f32>>;

// The premultiplied light display.wgsl `pre` shows (invalid values as 0).
fn radiance(x: u32, y: u32) -> vec3<f32> {
    let s = accum[y * bp.img.x + x];
    if !(s.w > 1e-6) || !(abs(s.x) < 3.4e38 && abs(s.y) < 3.4e38 && abs(s.z) < 3.4e38) {
        return vec3<f32>(0.0);
    }
    return max(s.xyz / s.w, vec3<f32>(0.0)) * clamp(s.w, 0.0, 1.0);
}

// post::bright_factor
fn bright(c: vec3<f32>) -> vec3<f32> {
    let y = dot(c, vec3<f32>(0.2126, 0.7152, 0.0722)) * bp.prm.x;
    return c * (max(y - bp.prm.y, 0.0) / max(y, 1.0e-6));
}

@compute @workgroup_size(8, 8)
fn bloom_bright(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= bp.dst.y || gid.y >= bp.dst.z {
        return;
    }
    var s = vec3<f32>(0.0);
    var n = 0.0;
    for (var y = gid.y * 4u; y < min(gid.y * 4u + 4u, bp.img.y); y = y + 1u) {
        for (var x = gid.x * 4u; x < min(gid.x * 4u + 4u, bp.img.x); x = x + 1u) {
            s = s + bright(radiance(x, y));
            n = n + 1.0;
        }
    }
    levels[bp.dst.x + gid.y * bp.dst.y + gid.x] = vec4<f32>(s / max(n, 1.0), 0.0);
}

@compute @workgroup_size(8, 8)
fn bloom_down(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= bp.dst.y || gid.y >= bp.dst.z {
        return;
    }
    var s = vec3<f32>(0.0);
    var n = 0.0;
    for (var y = gid.y * 2u; y < min(gid.y * 2u + 2u, bp.src.z); y = y + 1u) {
        for (var x = gid.x * 2u; x < min(gid.x * 2u + 2u, bp.src.y); x = x + 1u) {
            s = s + levels[bp.src.x + y * bp.src.y + x].xyz;
            n = n + 1.0;
        }
    }
    levels[bp.dst.x + gid.y * bp.dst.y + gid.x] = vec4<f32>(s / max(n, 1.0), 0.0);
}

const TAPS = array<f32, 5>(0.0625, 0.25, 0.375, 0.25, 0.0625);

@compute @workgroup_size(8, 8)
fn bloom_blur_h(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= bp.dst.y || gid.y >= bp.dst.z {
        return;
    }
    var s = vec3<f32>(0.0);
    for (var t = 0; t < 5; t = t + 1) {
        let x = u32(clamp(i32(gid.x) + t - 2, 0, i32(bp.dst.y) - 1));
        s = s + levels[bp.dst.x + gid.y * bp.dst.y + x].xyz * TAPS[t];
    }
    tmp[bp.dst.x + gid.y * bp.dst.y + gid.x] = vec4<f32>(s, 0.0);
}

@compute @workgroup_size(8, 8)
fn bloom_blur_v(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= bp.dst.y || gid.y >= bp.dst.z {
        return;
    }
    var s = vec3<f32>(0.0);
    for (var t = 0; t < 5; t = t + 1) {
        let y = u32(clamp(i32(gid.y) + t - 2, 0, i32(bp.dst.z) - 1));
        s = s + tmp[bp.dst.x + y * bp.dst.y + gid.x].xyz * TAPS[t];
    }
    levels[bp.dst.x + gid.y * bp.dst.y + gid.x] = vec4<f32>(s, 0.0);
}

// post::sample_zero on the level `src` (taps outside are black).
fn src_zero(fx: f32, fy: f32) -> vec3<f32> {
    let x0 = floor(fx);
    let y0 = floor(fy);
    let tx = fx - x0;
    let ty = fy - y0;
    var c: array<vec3<f32>, 4>;
    for (var k = 0; k < 4; k = k + 1) {
        let x = x0 + f32(k & 1);
        let y = y0 + f32(k >> 1u);
        if x < 0.0 || y < 0.0 || x >= f32(bp.src.y) || y >= f32(bp.src.z) {
            c[k] = vec3<f32>(0.0);
        } else {
            c[k] = levels[bp.src.x + u32(y) * bp.src.y + u32(x)].xyz;
        }
    }
    let top = c[0] + (c[1] - c[0]) * tx;
    let bottom = c[2] + (c[3] - c[2]) * tx;
    return top + (bottom - top) * ty;
}

// post::streak_pass (and streak_weights)
@compute @workgroup_size(8, 8)
fn bloom_streak(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= bp.dst.y || gid.y >= bp.dst.z {
        return;
    }
    let step = bp.prm2.z;
    var w: array<f32, 4>;
    var sum = 0.0;
    for (var k = 0; k < 4; k = k + 1) {
        w[k] = pow(bp.prm2.w, f32(k) * step);
        sum = sum + w[k];
    }
    var s = vec3<f32>(0.0);
    for (var k = 0; k < 4; k = k + 1) {
        let off = f32(k) * step;
        s = s + src_zero(f32(gid.x) + bp.prm2.x * off, f32(gid.y) + bp.prm2.y * off) * (w[k] / sum);
    }
    let i = bp.dst.x + gid.y * bp.dst.y + gid.x;
    if bp.prm.z > 0.5 {
        s = s + levels[i].xyz;
    }
    levels[i] = vec4<f32>(s, 0.0);
}
