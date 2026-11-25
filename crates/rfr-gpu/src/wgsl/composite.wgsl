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
    // opacity, unused…
    blend: vec4<f32>,
}

@group(0) @binding(0) var<uniform> O: Op;
@group(0) @binding(1) var<storage, read_write> scratch: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read_write> accum: array<vec4<f32>>;

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
            let c = scratch[O.slots.z * stride + li];
            let gi = (gid.y + O.size.z) * O.size.x + gid.x;
            accum[gi] = accum[gi] + vec4<f32>(c.xyz * c.w, c.w);
        }
    }
}
