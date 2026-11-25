// SPDX-License-Identifier: LGPL-3.0-or-later
//
// Accumulated premultiplied linear RGBA → sRGB-encoded 8-bit texture for the preview, over a
// checkerboard where the image is transparent (encoding as rfr_core::gradient::linear_to_srgb).

struct Disp {
    // width, height, completed samples, rows of the current sample done
    size: vec4<u32>,
}

@group(0) @binding(0) var<uniform> D: Disp;
@group(0) @binding(1) var<storage, read> accum: array<vec4<f32>>;
@group(0) @binding(2) var out_tex: texture_storage_2d<rgba8unorm, write>;

fn to_srgb(v: f32) -> f32 {
    let c = clamp(v, 0.0, 1.0);
    if c <= 0.0031308 {
        return c * 12.92;
    }
    return 1.055 * pow(c, 1.0 / 2.4) - 0.055;
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= D.size.x || gid.y >= D.size.y {
        return;
    }
    var n = D.size.z;
    if gid.y < D.size.w {
        n = n + 1u;
    }
    let a = accum[gid.y * D.size.x + gid.x] / f32(max(n, 1u));
    // Checkerboard (linear grays) behind transparent parts.
    let check = select(0.06, 0.11, ((gid.x / 8u) + (gid.y / 8u)) % 2u == 0u);
    let c = a.xyz + vec3<f32>(check) * (1.0 - clamp(a.w, 0.0, 1.0));
    textureStore(out_tex, vec2<i32>(i32(gid.x), i32(gid.y)), vec4<f32>(to_srgb(c.x), to_srgb(c.y), to_srgb(c.z), 1.0));
}
