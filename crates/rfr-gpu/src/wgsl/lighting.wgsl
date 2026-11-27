// SPDX-License-Identifier: LGPL-3.0-or-later
//
// Lit layers (rfr_core::render::light), appended to kernel.wgsl: `main` with operation 2
// writes the layer's field for the whole image (color in 16-bit fixed point, height), `lit` shades a
// band from the field with the normals of the neighboring heights and blends it like `main`.

// Per pixel: red and green, blue and alpha (16-bit fixed point, 0 to 1), height (f32 bits), unused.
@group(0) @binding(7) var<storage, read_write> field: array<vec4<u32>>;

fn l_store(x: u32, y: u32, col: vec4<f32>, h: f32) {
    var hh = h;
    if !(abs(hh) <= 3.4e38) {
        hh = 0.0;
    }
    field[y * U.size.x + x] = vec4<u32>(pack2x16unorm(col.xy), pack2x16unorm(col.zw), bitcast<u32>(hh), 0u);
}

fn l_height(x: i32, y: i32) -> f32 {
    let xx = u32(clamp(x, 0, i32(U.size.x) - 1));
    let yy = u32(clamp(y, 0, i32(U.size.y) - 1));
    return bitcast<f32>(field[yy * U.size.x + xx].z);
}

// rfr_core::render::light::Field::height_at: bilinear, the border continued.
fn l_height_at(x: f32, y: f32) -> f32 {
    let x0 = floor(x);
    let y0 = floor(y);
    let fx = x - x0;
    let fy = y - y0;
    let xi = i32(x0);
    let yi = i32(y0);
    let top = l_height(xi, yi) + (l_height(xi + 1, yi) - l_height(xi, yi)) * fx;
    let bottom = l_height(xi, yi + 1) + (l_height(xi + 1, yi + 1) - l_height(xi, yi + 1)) * fx;
    return top + (bottom - top) * fy;
}

fn l_rnd(state: ptr<function, u32>) -> f32 {
    *state = hash(*state);
    return f32(*state >> 8u) / 16777216.0;
}

// rfr_core::render::light::Field::shadow
fn l_shadow(dir: vec3<f32>, px: f32, py: f32, z0: f32, k: f32, len: f32, soft: f32, jitter: f32) -> f32 {
    let hl = length(dir.xy);
    if hl < 1e-4 || dir.z <= 0.0 {
        if dir.z > 0.0 {
            return 1.0;
        }
        return 0.0;
    }
    let sx = dir.x / hl;
    let sy = -dir.y / hl;
    let rise = dir.z / hl;
    var seen = 1.0;
    for (var i = 0u; i < 32u; i = i + 1u) {
        let f = (f32(i) + jitter) / 32.0;
        let t = max(len * f * f, 0.5);
        let clear = z0 + rise * t - k * l_height_at(px + sx * t, py + sy * t);
        var v = 1.0;
        if soft > 0.0 {
            v = clamp(clear / (soft * t), 0.0, 1.0);
        } else if clear < 0.0 {
            v = 0.0;
        }
        seen = min(seen, v);
        if seen <= 0.0 {
            break;
        }
    }
    return seen;
}

// rfr_core::render::light::Field::occlusion
fn l_occlusion(px: f32, py: f32, z0: f32, k: f32, radius: f32, strength: f32, turn: f32, jitter: f32) -> f32 {
    var sum = 0.0;
    for (var d = 0u; d < 8u; d = d + 1u) {
        let a = (f32(d) + turn) / 8.0 * 6.283185307179586;
        let sx = cos(a);
        let sy = sin(a);
        var best = 0.0;
        for (var j = 0u; j < 6u; j = j + 1u) {
            let t = max(radius * (f32(j) + jitter) / 6.0, 0.5);
            let rise = (k * l_height_at(px + sx * t, py + sy * t) - z0) / t;
            best = max(best, rise);
        }
        sum = sum + best / sqrt(1.0 + best * best);
    }
    return clamp(1.0 - strength * sum / 8.0, 0.0, 1.0);
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

// rfr_core::render::light::shade
fn l_shade(color: vec4<f32>, gx: f32, gy: f32, seen: array<f32, 4>, open: f32) -> vec4<f32> {
    let k = U.light.y;
    let n = normalize(vec3<f32>(-k * gx, -k * gy, 1.0));
    let nv = max(n.z, 0.05);
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
        let h = normalize(dir + vec3<f32>(0.0, 0.0, 1.0));
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
        let r = vec3<f32>(2.0 * n.z * n.x, 2.0 * n.z * n.y, 2.0 * n.z * n.z - 1.0);
        let f = refl + (1.0 - refl) * refl * pow(1.0 - nv, 5.0);
        out = out * (1.0 - f) + l_environment(r) * f;
    }
    return vec4<f32>(out, color.w);
}

@compute @workgroup_size(8, 8)
fn lit(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y + U.size.w;
    if x >= U.size.x || gid.y >= U.band.x || y >= U.size.y {
        return;
    }
    let c = field[y * U.size.x + x];
    let color = vec4<f32>(unpack2x16unorm(c.x), unpack2x16unorm(c.y));
    let xi = i32(x);
    let yi = i32(y);
    let gx = (l_height(xi + 1, yi) - l_height(xi - 1, yi)) * 0.5;
    let gy = (l_height(xi, yi - 1) - l_height(xi, yi + 1)) * 0.5;
    // Shadows and ambient occlusion (the same random numbers as the CPU).
    let k = U.light.y;
    let z0 = k * bitcast<f32>(c.z);
    let px = f32(x);
    let py = f32(y);
    var rng = hash(x ^ hash(y ^ hash(U.size.z ^ 0x3C6EF372u)));
    var seen = array<f32, 4>(1.0, 1.0, 1.0, 1.0);
    if U.lshadow.x > 0.0 {
        for (var i = 0u; i < U.lighti.y; i = i + 1u) {
            seen[i] = l_shadow(U.ldir[i].xyz, px, py, z0, k, U.lshadow.x, U.lshadow.y, l_rnd(&rng));
        }
    }
    var open = 1.0;
    if U.lshadow.z > 0.0 && U.lshadow.w > 0.0 {
        let turn = l_rnd(&rng);
        let jitter = l_rnd(&rng);
        open = l_occlusion(px, py, z0, k, U.lshadow.w, U.lshadow.z, turn, jitter);
    }
    let col = l_shade(color, gx, gy, seen, open);
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
