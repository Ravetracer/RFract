// SPDX-License-Identifier: LGPL-3.0-or-later
//
// The entry point of a 3D object layer (object.wgsl has the shape and its tracing): one
// sample per pixel of a band, traced from the layer's camera (rfr_core::render::object).

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
