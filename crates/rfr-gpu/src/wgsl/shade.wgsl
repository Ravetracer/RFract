// SPDX-License-Identifier: LGPL-3.0-or-later
//
// Shading shared by lit layers (lighting.wgsl) and 3D object layers (object.wgsl), appended
// to kernel.wgsl: random numbers, the environment, the light model at a surface normal and
// the blending into the layer's scratch slot (rfr_core::render::light).

fn l_rnd(state: ptr<function, u32>) -> f32 {
    *state = hash(*state);
    return f32(*state >> 8u) / 16777216.0;
}

// rfr_core::render::light::LightPlan::environment
fn l_environment(r: vec3<f32>) -> vec3<f32> {
    if U.lenv.y >= 0.0 {
        let u = atan2(r.y, r.x) / 6.283185307179586 + 0.5;
        let v = 0.5 - asin(clamp(r.z, -1.0, 1.0)) / 3.141592653589793;
        let c = k_image_color(u32(U.lenv.y), c_f(2.0 * u - 1.0, 1.0 - 2.0 * v));
        return vec3<f32>(srgb_dec(c.x), srgb_dec(c.y), srgb_dec(c.z));
    }
    let t = clamp(r.z, 0.0, 1.0);
    return mix(U.lsky0.xyz, U.lsky1.xyz, t);
}

// rfr_core::render::light::shade_normal
fn l_shade_n(color: vec4<f32>, n: vec3<f32>, seen: array<f32, 4>, open: f32, v: vec3<f32>) -> vec4<f32> {
    // The denoiser's normal image (LightPlan::normal_output).
    if U.lenv.z > 0.5 {
        return vec4<f32>(n * 0.5 + vec3<f32>(0.5), color.w);
    }
    let nv = max(dot(n, v), 0.05);
    let a = U.light.w * U.light.w;
    var diffuse = vec3<f32>(U.light.x * open);
    var spec = vec3<f32>(0.0);
    for (var i = 0u; i < U.lighti.y; i = i + 1u) {
        if seen[i] <= 0.0 {
            continue;
        }
        let dir = U.ldir[i].xyz;
        let nl = dot(n, dir);
        if nl <= 0.0 {
            continue;
        }
        let h = normalize(dir + v);
        let nh = max(dot(n, h), 0.0);
        var s = 0.0;
        if U.lighti.z == 0u {
            let e = max(2.0 / (a * a) - 2.0, 1.0);
            s = (e + 8.0) / (8.0 * 3.141592653589793) * pow(max(nh, 1e-6), e) * nl;
        } else {
            let a2 = a * a;
            let t = nh * nh * (a2 - 1.0) + 1.0;
            s = a2 / (3.141592653589793 * t * t) / (4.0 * nv);
        }
        let rad = U.lrad[i].xyz * seen[i];
        diffuse = diffuse + rad * nl;
        spec = spec + rad * s;
    }
    var out = color.xyz * diffuse + U.light.z * spec;
    // Reflection of the environment, stronger at grazing angles.
    let refl = U.lenv.x;
    if refl > 0.0 {
        let r = 2.0 * dot(n, v) * n - v;
        let f = refl + (1.0 - refl) * refl * pow(1.0 - nv, 5.0);
        out = out * (1.0 - f) + l_environment(r) * f;
    }
    return vec4<f32>(out, color.w);
}

// Blends a lit or object layer's color into its scratch slot like kernel `main`.
fn l_blend(gid: vec3<u32>, x: u32, col: vec4<f32>) {
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
