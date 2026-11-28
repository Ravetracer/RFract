// SPDX-License-Identifier: LGPL-3.0-or-later
//
// 3D object layers (rfr_core::render::object), appended to the part of kernel.wgsl before
// its main and shade.wgsl: `main` sphere-traces the camera ray of each pixel of a band to the
// shape's surface, lights it and blends it like a layer kernel. Uniforms: obj (power, scale,
// detail, bounding radius), obji (shape, iterations, Julia mode, unused), objc (seed), lcam
// (camera direction and pitch in radians, distance, tan(fov / 2)).

fn o_c3(p: vec3<f32>) -> vec3<f32> {
    if U.obji.z != 0u {
        return U.objc.xyz;
    }
    return p;
}

fn o_qmul(a: vec4<f32>, b: vec4<f32>) -> vec4<f32> {
    return vec4<f32>(
        a.x * b.x - a.y * b.y - a.z * b.z - a.w * b.w,
        a.x * b.y + a.y * b.x + a.z * b.w - a.w * b.z,
        a.x * b.z - a.y * b.w + a.z * b.x + a.w * b.y,
        a.x * b.w + a.y * b.z - a.z * b.y + a.w * b.x,
    );
}

// The distance estimate and the orbit trap (ObjectPlan::de).
fn o_de(p: vec3<f32>) -> vec2<f32> {
    let iters = U.obji.y;
    var trap = 3.4e38;
    switch U.obji.x {
        case 1u: {
            var c = vec4<f32>(p, 0.0);
            if U.obji.z != 0u {
                c = U.objc;
            }
            var q = vec4<f32>(p, 0.0);
            var dq = vec4<f32>(1.0, 0.0, 0.0, 0.0);
            for (var i = 0u; i < iters; i = i + 1u) {
                let r2 = dot(q, q);
                if r2 > 16.0 {
                    break;
                }
                trap = min(trap, sqrt(r2));
                dq = 2.0 * o_qmul(q, dq);
                q = o_qmul(q, q) + c;
            }
            let r = length(q);
            let dr = length(dq);
            return vec2<f32>(0.5 * r * log(max(r, 1e-12)) / max(dr, 1e-12), min(trap, r));
        }
        case 2u: {
            let s = U.obj.y;
            let c = o_c3(p);
            var add = 1.0;
            if U.obji.z != 0u {
                add = 0.0;
            }
            var z = p;
            var dr = 1.0;
            for (var i = 0u; i < iters; i = i + 1u) {
                z = clamp(z, vec3<f32>(-1.0), vec3<f32>(1.0)) * 2.0 - z;
                let r2 = dot(z, z);
                trap = min(trap, sqrt(r2));
                var k = 1.0;
                if r2 < 0.25 {
                    k = 4.0;
                } else if r2 < 1.0 {
                    k = 1.0 / r2;
                }
                z = z * k * s + c;
                dr = dr * k * abs(s) + add;
                if dot(z, z) > 1e8 {
                    break;
                }
            }
            return vec2<f32>(length(z) / max(abs(dr), 1e-12), trap);
        }
        case 3u: {
            var z = p;
            var scale = 1.0;
            for (var i = 0u; i < iters; i = i + 1u) {
                z = abs(z);
                if z.x < z.y {
                    z = vec3<f32>(z.y, z.x, z.z);
                }
                if z.x < z.z {
                    z = vec3<f32>(z.z, z.y, z.x);
                }
                if z.y < z.z {
                    z = vec3<f32>(z.x, z.z, z.y);
                }
                z = 3.0 * z - vec3<f32>(2.0);
                if z.z < -1.0 {
                    z.z = z.z + 2.0;
                }
                scale = scale * 3.0;
                trap = min(trap, length(z) / scale);
            }
            let q = abs(z) - vec3<f32>(1.0);
            let d = length(max(q, vec3<f32>(0.0))) + min(max(q.x, max(q.y, q.z)), 0.0);
            return vec2<f32>(d / scale, trap * 4.0);
        }
        default: {
            let n = U.obj.x;
            let c = o_c3(p);
            var add = 1.0;
            if U.obji.z != 0u {
                add = 0.0;
            }
            var z = p;
            var dr = 1.0;
            var r = length(z);
            for (var i = 0u; i < iters; i = i + 1u) {
                r = length(z);
                if r > 2.0 {
                    break;
                }
                trap = min(trap, r);
                var theta = 0.0;
                if r > 1e-12 {
                    theta = acos(clamp(z.z / r, -1.0, 1.0)) * n;
                }
                let phi = atan2(z.y, z.x) * n;
                dr = pow(r, n - 1.0) * n * dr + add;
                let zr = pow(r, n);
                z = zr * vec3<f32>(sin(theta) * cos(phi), sin(theta) * sin(phi), cos(theta)) + c;
                r = length(z);
            }
            return vec2<f32>(0.5 * log(max(r, 1e-12)) * r / max(dr, 1e-12), min(trap, r));
        }
    }
}

// ObjectPlan::ray: origin and direction through image point (x, y).
fn o_ray(x: f32, y: f32) -> array<vec3<f32>, 2> {
    let w = f32(U.win.z);
    let h = f32(U.win.w);
    let yaw = U.lcam.x;
    let pitch = U.lcam.y;
    let back = vec3<f32>(sin(yaw) * cos(pitch), -cos(yaw) * cos(pitch), sin(pitch));
    let o = back * U.lcam.z;
    let f = -back;
    let r = normalize(vec3<f32>(f.y, -f.x, 0.0));
    let u = cross(r, f);
    let sx = (x / w * 2.0 - 1.0) * (w / h) * U.lcam.w;
    let sy = (1.0 - y / h * 2.0) * U.lcam.w;
    return array<vec3<f32>, 2>(o, normalize(f + r * sx + u * sy));
}

// ObjectPlan::trace: (distance along the ray, trap, 1) or z = 0 for a miss.
fn o_trace(o: vec3<f32>, d: vec3<f32>, footprint: f32) -> vec3<f32> {
    let rb = U.obj.w;
    let b = dot(o, d);
    let disc = b * b - (dot(o, o) - rb * rb);
    if disc < 0.0 {
        return vec3<f32>(0.0);
    }
    let sq = sqrt(disc);
    let t1 = -b + sq;
    if t1 <= 0.0 {
        return vec3<f32>(0.0);
    }
    var t = max(-b - sq, 0.0);
    for (var i = 0u; i < 300u; i = i + 1u) {
        let e = o_de(o + d * t);
        if e.x < footprint * max(t, 1e-3) * U.obj.z {
            return vec3<f32>(t, e.y, 1.0);
        }
        t = t + e.x * 0.9;
        if t > t1 {
            return vec3<f32>(0.0);
        }
    }
    return vec3<f32>(0.0);
}

fn o_normal(p: vec3<f32>, e: f32) -> vec3<f32> {
    let x = vec3<f32>(e, 0.0, 0.0);
    let y = vec3<f32>(0.0, e, 0.0);
    let z = vec3<f32>(0.0, 0.0, e);
    return normalize(vec3<f32>(o_de(p + x).x - o_de(p - x).x, o_de(p + y).x - o_de(p - y).x, o_de(p + z).x - o_de(p - z).x));
}

// ObjectPlan::shadow
fn o_shadow(p: vec3<f32>, l: vec3<f32>, eps: f32, jitter: f32, max_t: f32) -> f32 {
    let soft = U.lshadow.y;
    let reach = min(U.lshadow.x * 2.0 * U.obj.w, max_t);
    var t = eps * (4.0 + jitter);
    var seen = 1.0;
    for (var i = 0u; i < 64u; i = i + 1u) {
        if t > reach {
            break;
        }
        let dist = o_de(p + l * t).x;
        var v = 1.0;
        if soft > 0.0 {
            v = clamp(dist / (soft * t), 0.0, 1.0);
        } else if dist < eps {
            v = 0.0;
        }
        seen = min(seen, v);
        if seen <= 0.0 {
            return 0.0;
        }
        t = t + max(dist, eps);
    }
    return seen;
}

// ObjectPlan::occlusion
fn o_occlusion(p: vec3<f32>, n: vec3<f32>, jitter: f32) -> f32 {
    let step = U.lshadow.w * U.obj.w / 5.0;
    var occ = 0.0;
    var w = 0.5;
    for (var i = 1u; i <= 5u; i = i + 1u) {
        let h = step * (f32(i) - 0.5 + jitter);
        let dist = o_de(p + n * h).x;
        occ = occ + w * max((h - dist) / h, 0.0);
        w = w * 0.5;
    }
    return clamp(1.0 - U.lshadow.z * occ * 2.0, 0.0, 1.0);
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y + U.size.w;
    if x >= U.size.x || gid.y >= U.band.x || y >= U.size.y {
        return;
    }
    // The sample position (render::sample_offset) and the jitter of the secondary rays.
    let s = U.size.z;
    // The pixel in the whole image (the window lies inside it).
    let xf = i32(x) + U.win.x;
    let yf = i32(y) + U.win.y;
    let ha = hash(u32(xf) ^ hash(u32(yf) + 0x68E31DA4u));
    let hb = hash(ha);
    let ju = f32(ha + s * 0xC13FA9A9u) / 4294967296.0;
    let jv = f32(hb + s * 0x91E10DA6u) / 4294967296.0;
    var rng = hash(u32(xf) ^ hash(u32(yf) ^ hash(s ^ 0x3C6EF372u)));
    let ray = o_ray(f32(xf) + ju, f32(yf) + jv);
    let o = ray[0];
    let d = ray[1];
    let footprint = 2.0 * U.lcam.w / f32(U.win.w);
    let hit = o_trace(o, d, footprint);
    if hit.z == 0.0 {
        l_blend(gid, x, vec4<f32>(0.0));
        return;
    }
    let t = hit.x;
    let p = o + d * t;
    let eps = footprint * max(t, 1e-3) * U.obj.z;
    let n = o_normal(p, eps * 0.5);
    let q = p + n * (eps * 2.0);
    var seen = array<f32, 4>(1.0, 1.0, 1.0, 1.0);
    if U.lshadow.x > 0.0 {
        for (var i = 0u; i < U.lighti.y; i = i + 1u) {
            let dir = U.ldir[i].xyz;
            if dot(n, dir) > 0.0 {
                seen[i] = o_shadow(q, dir, eps, l_rnd(&rng), 3.0e38);
            } else {
                seen[i] = 0.0;
            }
        }
    }
    // Point lights (world positions, ObjectPlan::shade).
    var pts = array<vec4<f32>, 4>();
    for (var i = 0u; i < u32(U.lenv.w); i = i + 1u) {
        let v = U.lpos[i].xyz - q;
        let dir = normalize(v);
        var sn = 1.0;
        if U.lshadow.x > 0.0 {
            if dot(n, dir) > 0.0 {
                sn = o_shadow(q, dir, eps, l_rnd(&rng), length(v));
            } else {
                sn = 0.0;
            }
        }
        let reach = U.lpos[i].w;
        if reach > 0.0 {
            sn = sn / (1.0 + dot(v, v) / (reach * reach));
        }
        pts[i] = vec4<f32>(dir, sn);
    }
    var open = 1.0;
    if U.lshadow.z > 0.0 && U.lshadow.w > 0.0 {
        open = o_occlusion(q, n, l_rnd(&rng));
    }
    let color = region_color(false, hit.y * 4.0);
    l_blend(gid, x, l_shade_n(color, n, seen, open, -d, pts));
}
