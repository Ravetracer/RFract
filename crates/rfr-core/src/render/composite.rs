// SPDX-License-Identifier: LGPL-3.0-or-later

//! Layer compositing: merge modes after the W3C "Compositing and Blending" specification,
//! applied to sRGB-encoded colors (as image editors do), with straight alpha. kernel.wgsl and
//! composite.wgsl mirror these functions; change both together.

use crate::doc::{MaskSource, MergeMode};

pub fn srgb_encode(v: f32) -> f32 {
    let c = v.clamp(0.0, 1.0);
    if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
}

pub fn srgb_decode(v: f32) -> f32 {
    let c = v.clamp(0.0, 1.0);
    if c <= 0.040_45 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn lum(c: [f32; 3]) -> f32 {
    0.3 * c[0] + 0.59 * c[1] + 0.11 * c[2]
}

fn clip_color(c: [f32; 3]) -> [f32; 3] {
    let l = lum(c);
    let n = c[0].min(c[1]).min(c[2]);
    let x = c[0].max(c[1]).max(c[2]);
    let mut out = c;
    if n < 0.0 {
        for v in &mut out {
            *v = l + (*v - l) * l / (l - n).max(1e-12);
        }
    }
    if x > 1.0 {
        for v in &mut out {
            *v = l + (*v - l) * (1.0 - l) / (x - l).max(1e-12);
        }
    }
    out
}

fn set_lum(c: [f32; 3], l: f32) -> [f32; 3] {
    let d = l - lum(c);
    clip_color([c[0] + d, c[1] + d, c[2] + d])
}

fn sat(c: [f32; 3]) -> f32 {
    c[0].max(c[1]).max(c[2]) - c[0].min(c[1]).min(c[2])
}

fn set_sat(c: [f32; 3], s: f32) -> [f32; 3] {
    let mx = c[0].max(c[1]).max(c[2]);
    let mn = c[0].min(c[1]).min(c[2]);
    if mx > mn { c.map(|v| (v - mn) * s / (mx - mn)) } else { [0.0; 3] }
}

fn soft_light_d(cb: f32) -> f32 {
    if cb <= 0.25 { ((16.0 * cb - 12.0) * cb + 4.0) * cb } else { cb.sqrt() }
}

fn hard_light(cb: f32, cs: f32) -> f32 {
    if cs <= 0.5 {
        cb * 2.0 * cs
    } else {
        let s = 2.0 * cs - 1.0;
        cb + s - cb * s
    }
}

/// One channel of a separable merge mode (both values sRGB-encoded, 0..1).
fn channel(mode: MergeMode, cb: f32, cs: f32) -> f32 {
    match mode {
        MergeMode::Multiply => cb * cs,
        MergeMode::Screen => cb + cs - cb * cs,
        MergeMode::Overlay => hard_light(cs, cb),
        MergeMode::SoftLight => {
            if cs <= 0.5 {
                cb - (1.0 - 2.0 * cs) * cb * (1.0 - cb)
            } else {
                cb + (2.0 * cs - 1.0) * (soft_light_d(cb) - cb)
            }
        }
        MergeMode::HardLight => hard_light(cb, cs),
        MergeMode::ColorDodge => {
            if cb == 0.0 {
                0.0
            } else if cs >= 1.0 {
                1.0
            } else {
                (cb / (1.0 - cs)).min(1.0)
            }
        }
        MergeMode::ColorBurn => {
            if cb >= 1.0 {
                1.0
            } else if cs <= 0.0 {
                0.0
            } else {
                1.0 - ((1.0 - cb) / cs).min(1.0)
            }
        }
        MergeMode::Darken => cb.min(cs),
        MergeMode::Lighten => cb.max(cs),
        MergeMode::Difference => (cb - cs).abs(),
        MergeMode::Exclusion => cb + cs - 2.0 * cb * cs,
        MergeMode::Add => (cb + cs).min(1.0),
        MergeMode::Subtract => (cb - cs).max(0.0),
        _ => cs,
    }
}

/// The merge function B(backdrop, source) on sRGB-encoded colors.
pub fn blend(mode: MergeMode, cb: [f32; 3], cs: [f32; 3]) -> [f32; 3] {
    match mode {
        MergeMode::Hue => set_lum(set_sat(cs, sat(cb)), lum(cb)),
        MergeMode::Saturation => set_lum(set_sat(cb, sat(cs)), lum(cb)),
        MergeMode::Color => set_lum(cs, lum(cb)),
        MergeMode::Luminosity => set_lum(cb, lum(cs)),
        _ => [channel(mode, cb[0], cs[0]), channel(mode, cb[1], cs[1]), channel(mode, cb[2], cs[2])],
    }
}

/// `top` over `below` (both linear RGB with straight alpha); `scale` multiplies the top's
/// alpha (opacity × mask). Returns linear RGB with straight alpha.
pub fn composite(below: [f32; 4], top: [f32; 4], mode: MergeMode, scale: f32) -> [f32; 4] {
    let a_s = (top[3] * scale).clamp(0.0, 1.0);
    let a_b = below[3].clamp(0.0, 1.0);
    if a_s <= 0.0 {
        return below;
    }
    let cb = [srgb_encode(below[0]), srgb_encode(below[1]), srgb_encode(below[2])];
    let cs = [srgb_encode(top[0]), srgb_encode(top[1]), srgb_encode(top[2])];
    let b = blend(mode, cb, cs);
    let a_o = a_s + a_b * (1.0 - a_s);
    let mut out = [0.0f32; 4];
    for k in 0..3 {
        let mixed = (1.0 - a_b) * cs[k] + a_b * b[k];
        let co = a_s * mixed + (1.0 - a_s) * a_b * cb[k];
        out[k] = srgb_decode(if a_o > 0.0 { co / a_o } else { 0.0 });
    }
    out[3] = a_o;
    out
}

/// Value of a mask color (linear RGB, straight alpha): luminance × alpha or alpha, inverted on
/// request.
pub fn mask_value(c: [f32; 4], source: MaskSource, invert: bool) -> f32 {
    let m = match source {
        MaskSource::Luminance => (0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]) * c[3],
        MaskSource::Alpha => c[3],
    }
    .clamp(0.0, 1.0);
    if invert { 1.0 - m } else { m }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_and_opacity() {
        let below = [1.0, 0.0, 0.0, 1.0];
        let top = [0.0, 0.0, 1.0, 1.0];
        assert_eq!(composite(below, top, MergeMode::Normal, 1.0), top);
        assert_eq!(composite(below, top, MergeMode::Normal, 0.0), below);
        let half = composite(below, top, MergeMode::Normal, 0.5);
        assert!((srgb_encode(half[0]) - 0.5).abs() < 1e-4 && (srgb_encode(half[2]) - 0.5).abs() < 1e-4, "{half:?}");
        // Over a transparent backdrop the layer keeps its color and alpha.
        let t = composite([0.0; 4], [0.2, 0.4, 0.6, 0.5], MergeMode::Multiply, 1.0);
        assert!((t[0] - 0.2).abs() < 1e-4 && (t[3] - 0.5).abs() < 1e-6, "{t:?}");
    }

    #[test]
    fn merge_modes_known_values() {
        let w = [1.0; 3];
        let g = [0.5; 3];
        assert_eq!(blend(MergeMode::Multiply, w, g), g);
        assert_eq!(blend(MergeMode::Screen, [0.0; 3], g), g);
        assert_eq!(blend(MergeMode::Difference, w, g), g);
        assert!(blend(MergeMode::Luminosity, [1.0, 0.0, 0.0], [0.0; 3]).iter().all(|v| v.abs() < 1e-6));
        for m in MergeMode::ALL {
            let r = blend(m, [0.2, 0.5, 0.9], [0.7, 0.3, 0.1]);
            assert!(r.iter().all(|v| (-1e-5..=1.0 + 1e-5).contains(v)), "{m:?}: {r:?}");
        }
    }
}
