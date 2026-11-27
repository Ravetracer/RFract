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

// rfr_core::render::light::shade
fn l_shade(color: vec4<f32>, gx: f32, gy: f32) -> vec4<f32> {
    let k = U.light.y;
    let n = normalize(vec3<f32>(-k * gx, -k * gy, 1.0));
    let nv = max(n.z, 0.05);
    let a = U.light.w * U.light.w;
    var diffuse = vec3<f32>(U.light.x);
    var spec = vec3<f32>(0.0);
    for (var i = 0u; i < U.lighti.y; i = i + 1u) {
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
        let rad = U.lrad[i].xyz;
        diffuse = diffuse + rad * nl;
        spec = spec + rad * s;
    }
    return vec4<f32>(color.xyz * diffuse + U.light.z * spec, color.w);
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
    let col = l_shade(color, gx, gy);
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
