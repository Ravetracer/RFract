// SPDX-License-Identifier: LGPL-3.0-or-later
//
// Compositing passes on the band scratch buffers: fill a slot with a color, merge a group's
// slot into its parent, accumulate the finished sample (premultiplied) into the image. Mirrors
// rfr_core::render::cpu (composite_nodes, pass).

struct Op {
    // width, rows in this band, first row of the band, slot size (pixels)
    size: vec4<u32>,
    // operation (0 fill, 1 merge, 2 accumulate), destination slot, source slot, mask slot
    // (0xffffffff: none)
    slots: vec4<u32>,
    // merge mode, mask source, mask invert, unused
    mode: vec4<u32>,
    // fill color (linear RGB, alpha)
    color: vec4<f32>,
    // opacity, adaptive sampling tolerance (0: off), unused…
    blend: vec4<f32>,
}

@group(0) @binding(0) var<uniform> O: Op;
@group(0) @binding(1) var<storage, read_write> scratch: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read_write> accum: array<vec4<f32>>;
// Per pixel: samples, Σ s, Σ s², done, with s = √luma of the sample (render::adaptive_update).
@group(0) @binding(3) var<storage, read_write> stats: array<vec4<f32>>;

const ADAPTIVE_MIN_SAMPLES: f32 = 32.0;

// render::adaptive_settled
fn settled(st: vec4<f32>, tolerance: f32) -> bool {
    let n = st.x;
    if n < ADAPTIVE_MIN_SAMPLES {
        return false;
    }
    let m = st.y / n;
    return sqrt(max(st.z / n - m * m, 0.0) / n) < tolerance;
}

// A neighbour of pixel (x, y) is settled (clamped to the image; height O.mode.z).
fn neighbour_settled(x: i32, y: i32, tolerance: f32) -> bool {
    let w = i32(O.size.x);
    let h = i32(O.mode.z);
    let st = stats[u32(clamp(y, 0, h - 1) * w + clamp(x, 0, w - 1))];
    return st.w > 0.5 || settled(st, tolerance);
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= O.size.x || gid.y >= O.size.y {
        return;
    }
    let li = gid.y * O.size.x + gid.x;
    let stride = O.size.w;
    let dst = O.slots.y * stride + li;
    switch O.slots.x {
        case 0u: {
            scratch[dst] = O.color;
        }
        case 1u: {
            var scale = O.blend.x;
            if O.slots.w != 0xffffffffu {
                scale = scale * scratch[O.slots.w * stride + li].x;
            }
            scratch[dst] = b_composite(scratch[dst], scratch[O.slots.z * stride + li], O.mode.x, scale);
        }
        default: {
            // The running mean of the premultiplied light, adaptive sampling as
            // render::adaptive_update.
            let gi = (gid.y + O.size.z) * O.size.x + gid.x;
            var st = stats[gi];
            if st.w > 0.5 {
                return;
            }
            let c = scratch[O.slots.z * stride + li];
            let p = vec4<f32>(c.xyz * c.w, c.w);
            let n = st.x + 1.0;
            accum[gi] = accum[gi] + (p - accum[gi]) / n;
            let s = sqrt(max(dot(p.xyz, vec3<f32>(0.2126, 0.7152, 0.0722)), 0.0));
            st = vec4<f32>(n, st.y + s, st.z + s * s, 0.0);
            let tol = O.blend.y;
            if tol > 0.0 && settled(st, tol) {
                let x = i32(gid.x);
                let y = i32(gid.y + O.size.z);
                if neighbour_settled(x - 1, y, tol) && neighbour_settled(x + 1, y, tol) && neighbour_settled(x, y - 1, tol)
                    && neighbour_settled(x, y + 1, tol) {
                    st.w = 1.0;
                }
            }
            stats[gi] = st;
        }
    }
}
