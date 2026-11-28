// SPDX-License-Identifier: LGPL-3.0-or-later
//
// Lit layers (rfr_core::render::light), appended to kernel.wgsl: `main` with operation 2
// writes the layer's fields (color in 16-bit fixed point, height; the near and the far grid of
// light::Grids), `lit` shades a band from them with the normals of the neighboring heights and
// blends it like `main`.

// Per cell: red and green, blue and alpha (16-bit fixed point, 0 to 1), height and distance to
// the set in pixels (f32 bits). The near grid's cells first, then the far grid's (a 3D
// terrain: its one grid, then the height maxima).
@group(0) @binding(7) var<storage, read_write> field: array<vec4<u32>>;

// The pixel being shaded (whole-image pixels): lookups near it read the near grid.
var<private> lp0: vec2<f32>;

fn l_store(at: u32, col: vec4<f32>, h: f32, de: f32) {
    var hh = h;
    if !(abs(hh) <= 3.4e38) {
        hh = 0.0;
    }
    field[at] = vec4<u32>(pack2x16unorm(col.xy), pack2x16unorm(col.zw), bitcast<u32>(hh), bitcast<u32>(de));
}

// rfr_core::render::light::Fields::pick: the far grid unless the point is within the near
// margin of the shaded pixel (per axis).
fn l_far(x: f32, y: f32) -> bool {
    return U.fnear.z == 0 || max(abs(x - lp0.x), abs(y - lp0.y)) > f32(U.fstep.y);
}

// A point in the cells of the near or far grid (a terrain is shaded in its cells).
fn l_local(x: f32, y: f32, far: bool) -> vec2<f32> {
    if U.lighti.w != 0u {
        return vec2<f32>(x, y);
    }
    if far {
        return vec2<f32>((x - f32(U.ffar.x)) / f32(U.fstep.x), (y - f32(U.ffar.y)) / f32(U.fstep.x));
    }
    return vec2<f32>(x - f32(U.fnear.x), y - f32(U.fnear.y));
}

// A cell of the near or far grid, the border continued.
fn l_cell(i: i32, j: i32, far: bool) -> vec4<u32> {
    var g = U.fnear;
    var base = 0u;
    if far {
        g = U.ffar;
        base = U.fstep.z;
    }
    let ii = u32(clamp(i, 0, g.z - 1));
    let jj = u32(clamp(j, 0, g.w - 1));
    return field[base + jj * u32(g.z) + ii];
}

// rfr_core::render::light::Field::blurred_alpha
fn l_blurred_alpha(x: f32, y: f32, sigma: f32, rng: ptr<function, u32>) -> f32 {
    if sigma < 0.25 {
        return l_color_at(x, y).w;
    }
    var sum = 0.0;
    for (var i = 0u; i < 16u; i = i + 1u) {
        let u1 = max(l_rnd(rng), 1e-7);
        let u2 = l_rnd(rng);
        let r = sigma * sqrt(-2.0 * log(u1));
        let a = 6.283185307179586 * u2;
        sum = sum + l_color_at(x + r * cos(a), y + r * sin(a)).w;
    }
    return sum / 16.0;
}

// rfr_core::render::light::Field::effects
fn l_effects(c: vec4<f32>, x: f32, y: f32, de: f32, rng: ptr<function, u32>) -> vec4<f32> {
    var lc = c.xyz;
    let la = c.w;
    if U.fx1.z > 0.0 {
        let ig = clamp(U.fx1.z * (1.0 - l_blurred_alpha(x, y, U.fx1.w, rng)) * la, 0.0, 1.0);
        lc = lc + (U.fxc[2].xyz - lc) * ig;
    }
    if U.fx2.x > 0.0 {
        lc = lc + U.fxc[3].xyz * (U.fx2.x * exp(-de / U.fx2.y));
    }
    var g = 0.0;
    if U.fx1.x > 0.0 {
        g = clamp(U.fx1.x * l_blurred_alpha(x, y, U.fx1.y, rng), 0.0, 1.0);
    }
    var s = 0.0;
    if U.fx0.x > 0.0 {
        s = clamp(U.fx0.x * l_blurred_alpha(x - U.fx0.y, y - U.fx0.z, U.fx0.w, rng), 0.0, 1.0);
    }
    let ua = g + s * (1.0 - g);
    let a = la + ua * (1.0 - la);
    if a <= 1e-6 {
        return vec4<f32>(0.0);
    }
    let under = U.fxc[1].xyz * g + U.fxc[0].xyz * s * (1.0 - g);
    return vec4<f32>((lc * la + under * (1.0 - la)) / a, a);
}

// A height of the far grid (a 3D terrain's one grid).
fn l_height(x: i32, y: i32) -> f32 {
    return bitcast<f32>(l_cell(x, y, true).z);
}

fn l_height_in(x: i32, y: i32, far: bool) -> f32 {
    return bitcast<f32>(l_cell(x, y, far).z);
}

fn l_color_in(x: i32, y: i32, far: bool) -> vec4<f32> {
    let c = l_cell(x, y, far);
    return vec4<f32>(unpack2x16unorm(c.x), unpack2x16unorm(c.y));
}

// rfr_core::render::light::Fields::height_at: bilinear in the grid the point reads, the border
// continued (image pixels; a terrain's grid starts at 0).
fn l_height_at(x: f32, y: f32) -> f32 {
    let far = l_far(x, y);
    let p = l_local(x, y, far);
    let x0 = floor(p.x);
    let y0 = floor(p.y);
    let fx = p.x - x0;
    let fy = p.y - y0;
    let xi = i32(x0);
    let yi = i32(y0);
    let top = l_height_in(xi, yi, far) + (l_height_in(xi + 1, yi, far) - l_height_in(xi, yi, far)) * fx;
    let bottom = l_height_in(xi, yi + 1, far) + (l_height_in(xi + 1, yi + 1, far) - l_height_in(xi, yi + 1, far)) * fx;
    return top + (bottom - top) * fy;
}

// rfr_core::render::light::Fields::color_at
fn l_color_at(x: f32, y: f32) -> vec4<f32> {
    let far = l_far(x, y);
    let p = l_local(x, y, far);
    let x0 = floor(p.x);
    let y0 = floor(p.y);
    let xi = i32(x0);
    let yi = i32(y0);
    let top = mix(l_color_in(xi, yi, far), l_color_in(xi + 1, yi, far), p.x - x0);
    let bottom = mix(l_color_in(xi, yi + 1, far), l_color_in(xi + 1, yi + 1, far), p.x - x0);
    return mix(top, bottom, p.y - y0);
}

// Height maxima (rfr_core::render::light::Field::mips) after a terrain's grid, one f32 per
// cell, four per vec4: the cell counts and the first word of each level.
fn l_mip_size(level: u32) -> vec2<u32> {
    let s = (1u << level) - 1u;
    return vec2<u32>((max(u32(U.ffar.z), 1u) + s) >> level, (max(u32(U.ffar.w), 1u) + s) >> level);
}

fn l_mip_base(level: u32) -> u32 {
    var base = u32(U.ffar.z) * u32(U.ffar.w) * 4u;
    for (var l = 0u; l < level; l = l + 1u) {
        let n = l_mip_size(l);
        base = base + n.x * n.y;
    }
    return base;
}

fn l_mip_levels() -> u32 {
    var l = 0u;
    loop {
        let n = l_mip_size(l);
        if n.x == 1u && n.y == 1u {
            break;
        }
        l = l + 1u;
    }
    return l + 1u;
}

fn l_mip_get(base: u32, level: u32, x: i32, y: i32) -> f32 {
    let n = l_mip_size(level);
    let xx = u32(clamp(x, 0, i32(n.x) - 1));
    let yy = u32(clamp(y, 0, i32(n.y) - 1));
    let m = base + yy * n.x + xx;
    return bitcast<f32>(field[m / 4u][m % 4u]);
}

// One level of the height maxima (operation 3, level in band.z): level 0 from the four
// heights around each cell (times the slope scale), higher levels from 2 × 2 cells below.
@compute @workgroup_size(8, 8)
fn lmip(@builtin(global_invocation_id) gid: vec3<u32>) {
    let level = U.band.z;
    let n = l_mip_size(level);
    if gid.x >= n.x || gid.y >= n.y {
        return;
    }
    let x = i32(gid.x);
    let y = i32(gid.y);
    var v: f32;
    if level == 0u {
        let k = U.light.y;
        v = max(max(k * l_height(x, y), k * l_height(x + 1, y)), max(k * l_height(x, y + 1), k * l_height(x + 1, y + 1)));
    } else {
        let pb = l_mip_base(level - 1u);
        let p = l_mip_size(level - 1u);
        let x2 = 2 * x;
        let y2 = 2 * y;
        let xb = min(x2 + 1, i32(p.x) - 1);
        let yb = min(y2 + 1, i32(p.y) - 1);
        v = max(max(l_mip_get(pb, level - 1u, x2, y2), l_mip_get(pb, level - 1u, xb, y2)), max(l_mip_get(pb, level - 1u, x2, yb), l_mip_get(pb, level - 1u, xb, yb)));
    }
    let m = l_mip_base(level) + gid.y * n.x + gid.x;
    field[m / 4u][m % 4u] = bitcast<u32>(v);
}

// rfr_core::render::light::camera_ray_in: origin and direction (field cells, y down, z up)
// through image point (x, y) (whole-image pixels).
fn l_camera_ray(zc: f32, x: f32, y: f32) -> array<vec3<f32>, 2> {
    let fw = f32(U.ffar.z);
    let fh = f32(U.ffar.w);
    let w = f32(U.win.z);
    let h = f32(U.win.w);
    let yaw = U.lcam.x;
    let pitch = U.lcam.y;
    let dist = U.lcam.z;
    let tn = U.lcam.w;
    let target_ = vec3<f32>((fw - 1.0) * 0.5, (fh - 1.0) * 0.5, zc);
    let back = vec3<f32>(sin(yaw) * cos(pitch), cos(yaw) * cos(pitch), sin(pitch));
    let o = target_ + back * dist * fh;
    let f = -back;
    let r = normalize(vec3<f32>(-f.y, f.x, 0.0));
    let u = cross(f, r);
    let sx = (x / w * 2.0 - 1.0) * (w / h) * tn;
    let sy = (1.0 - y / h * 2.0) * tn;
    return array<vec3<f32>, 2>(o, normalize(f + r * sx + u * sy));
}

fn l_above(o: vec3<f32>, d: vec3<f32>, k: f32, t: f32) -> f32 {
    return o.z + d.z * t - k * l_height_at(o.x + d.x * t, o.y + d.y * t);
}

// rfr_core::render::light::Field::trace: the hit (x, y, 1), or z = 0 for a miss.
fn l_trace(o: vec3<f32>, d: vec3<f32>, k: f32) -> vec3<f32> {
    let levels = l_mip_levels();
    let top = levels - 1u;
    var bases: array<u32, 32>;
    for (var l = 0u; l < levels; l = l + 1u) {
        bases[l] = l_mip_base(l);
    }
    let xmax = max(f32(U.ffar.z) - 1.0, 0.0);
    let ymax = max(f32(U.ffar.w) - 1.0, 0.0);
    var t0 = 0.0;
    var t1 = 3.4e38;
    let oa = vec2<f32>(o.x, o.y);
    let da = vec2<f32>(d.x, d.y);
    let hi = vec2<f32>(xmax, ymax);
    for (var a = 0; a < 2; a = a + 1) {
        if abs(da[a]) < 1e-12 {
            if oa[a] < 0.0 || oa[a] > hi[a] {
                return vec3<f32>(0.0);
            }
        } else {
            let ta = -oa[a] / da[a];
            let tb = (hi[a] - oa[a]) / da[a];
            t0 = max(t0, min(ta, tb));
            t1 = min(t1, max(ta, tb));
        }
    }
    let zt = l_mip_get(bases[top], top, 0, 0);
    if o.z > zt {
        if d.z >= 0.0 {
            return vec3<f32>(0.0);
        }
        t0 = max(t0, (zt - o.z) / d.z);
    }
    if t0 > t1 {
        return vec3<f32>(0.0);
    }
    // Entering below the surface: through a side or from underneath (no walls).
    if l_above(o, d, k, t0) < -1e-3 * (1.0 + abs(zt) + abs(o.z)) {
        return vec3<f32>(0.0);
    }
    var t = t0;
    var level = top;
    let steps = 4u * u32(U.ffar.z + U.ffar.w) + 64u;
    for (var it = 0u; it < steps; it = it + 1u) {
        if t > t1 {
            return vec3<f32>(0.0);
        }
        let p = oa + da * t;
        let size = f32(1u << level);
        let c = floor(p / size);
        var te = t1;
        for (var a = 0; a < 2; a = a + 1) {
            if da[a] > 1e-12 {
                te = min(te, ((c[a] + 1.0) * size - oa[a]) / da[a]);
            } else if da[a] < -1e-12 {
                te = min(te, (c[a] * size - oa[a]) / da[a]);
            }
        }
        te = max(te, t);
        let zmax = l_mip_get(bases[level], level, i32(c.x), i32(c.y));
        if min(o.z + d.z * t, o.z + d.z * te) > zmax {
            t = te + 1e-3;
            level = min(level + 1u, top);
            continue;
        }
        if level > 0u {
            level = level - 1u;
            continue;
        }
        if l_above(o, d, k, t) <= 0.0 {
            return vec3<f32>(p, 1.0);
        }
        var a = t;
        for (var j = 1u; j <= 4u; j = j + 1u) {
            let b = t + (te - t) * f32(j) / 4.0;
            if l_above(o, d, k, b) <= 0.0 {
                var lo = a;
                var hi2 = b;
                for (var r = 0u; r < 8u; r = r + 1u) {
                    let m = 0.5 * (lo + hi2);
                    if l_above(o, d, k, m) <= 0.0 {
                        hi2 = m;
                    } else {
                        lo = m;
                    }
                }
                return vec3<f32>(oa + da * hi2, 1.0);
            }
            a = b;
        }
        t = te + 1e-3;
        level = min(1u, top);
    }
    // Out of steps: the ray skims the surface here.
    if t <= t1 {
        return vec3<f32>(oa + da * t, 1.0);
    }
    return vec3<f32>(0.0);
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

// rfr_core::render::light::shade (normal from the slopes; shade.wgsl)
fn l_shade(color: vec4<f32>, gx: f32, gy: f32, seen: array<f32, 4>, open: f32, v: vec3<f32>, pts: array<vec4<f32>, 4>) -> vec4<f32> {
    let k = U.light.y;
    return l_shade_n(color, normalize(vec3<f32>(-k * gx, -k * gy, 1.0)), seen, open, v, pts);
}

@compute @workgroup_size(8, 8)
fn lit(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y + U.size.w;
    if x >= U.size.x || gid.y >= U.band.x || y >= U.size.y {
        return;
    }
    let k = U.light.y;
    // The pixel in the whole image (random streams and lookups).
    let xi = i32(x) + U.win.x;
    let yi = i32(y) + U.win.y;
    lp0 = vec2<f32>(f32(xi), f32(yi));
    var rng = hash(u32(xi) ^ hash(u32(yi) ^ hash(U.size.z ^ 0x3C6EF372u)));
    var color: vec4<f32>;
    var px: f32;
    var py: f32;
    var h0: f32;
    var gx: f32;
    var gy: f32;
    var view = vec3<f32>(0.0, 0.0, 1.0);
    var de = 3.0e38;
    if U.lighti.w == 0u {
        px = lp0.x;
        py = lp0.y;
        let far = l_far(px, py);
        let q = l_local(px, py, far);
        let c = l_cell(i32(q.x), i32(q.y), far);
        de = bitcast<f32>(c.w);
        color = vec4<f32>(unpack2x16unorm(c.x), unpack2x16unorm(c.y));
        gx = (l_height_at(px + 1.0, py) - l_height_at(px - 1.0, py)) * 0.5;
        gy = (l_height_at(px, py - 1.0) - l_height_at(px, py + 1.0)) * 0.5;
        h0 = bitcast<f32>(c.z);
    } else {
        // 3D terrain: the camera ray through the pixel's sample position
        // (render::sample_offset), the field the whole view's ground.
        let ha = hash(u32(xi) ^ hash(u32(yi) + 0x68E31DA4u));
        let hb = hash(ha);
        let ju = f32(ha + U.size.z * 0xC13FA9A9u) / 4294967296.0;
        let jv = f32(hb + U.size.z * 0x91E10DA6u) / 4294967296.0;
        let zc = k * l_height_at((f32(U.ffar.z) - 1.0) * 0.5, (f32(U.ffar.w) - 1.0) * 0.5);
        let ray = l_camera_ray(zc, f32(xi) + ju, f32(yi) + jv);
        let hit = l_trace(ray[0], ray[1], k);
        if hit.z == 0.0 {
            l_blend(gid, x, vec4<f32>(0.0));
            return;
        }
        px = hit.x;
        py = hit.y;
        color = l_color_at(px, py);
        h0 = l_height_at(px, py);
        gx = (l_height_at(px + 1.0, py) - l_height_at(px - 1.0, py)) * 0.5;
        gy = (l_height_at(px, py - 1.0) - l_height_at(px, py + 1.0)) * 0.5;
        let d = ray[1];
        view = vec3<f32>(-d.x, d.y, -d.z);
    }
    // A lit layer is shaded (shadows and occlusion with the same random numbers as the CPU);
    // a layer with effects only keeps its colors.
    var col = color;
    if U.fx2.z > 0.5 || U.lighti.w != 0u {
        // Shadows and ambient occlusion (the same random numbers as the CPU).
        let z0 = k * h0;
        var seen = array<f32, 4>(1.0, 1.0, 1.0, 1.0);
        if U.lshadow.x > 0.0 {
            for (var i = 0u; i < U.lighti.y; i = i + 1u) {
                seen[i] = l_shadow(U.ldir[i].xyz, px, py, z0, k, U.lshadow.x, U.lshadow.y, l_rnd(&rng));
            }
        }
        // Point lights: the direction from here; shadows reach no farther than the light
        // (light::point_dir).
        var pts = array<vec4<f32>, 4>();
        for (var i = 0u; i < u32(U.lenv.w); i = i + 1u) {
            let d = vec3<f32>(U.lpos[i].x - px, py - U.lpos[i].y, U.lpos[i].z - z0);
            let dir = normalize(d);
            var sn = 1.0;
            if U.lshadow.x > 0.0 {
                sn = l_shadow(dir, px, py, z0, k, min(U.lshadow.x, length(d.xy)), U.lshadow.y, l_rnd(&rng));
            }
            // light::falloff
            let reach = U.lpos[i].w;
            if reach > 0.0 {
                sn = sn / (1.0 + dot(d, d) / (reach * reach));
            }
            pts[i] = vec4<f32>(dir, sn);
        }
        var open = 1.0;
        if U.lshadow.z > 0.0 && U.lshadow.w > 0.0 {
            let turn = l_rnd(&rng);
            let jitter = l_rnd(&rng);
            open = l_occlusion(px, py, z0, k, U.lshadow.w, U.lshadow.z, turn, jitter);
        }
        col = l_shade(color, gx, gy, seen, open, view, pts);
    }
    if U.lighti.w == 0u && U.fx2.w > 0.5 {
        col = l_effects(col, px, py, de, &rng);
    }
    l_blend(gid, x, col);
}
