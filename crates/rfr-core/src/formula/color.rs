// SPDX-License-Identifier: LGPL-3.0-or-later
//! Color values of formulas: RGBA with sRGB-encoded channels in 0…1 and straight alpha. Hue
//! runs from 0 to 6 (red, yellow, green, cyan, blue, magenta). Mirrored by the `k_*`
//! functions of `rfr-gpu/src/wgsl/cstd.wgsl`.

use crate::doc::MergeMode;

pub type Rgba = [f64; 4];

/// HSL (hue 0…6, saturation and lightness 0…1) to RGB.
pub fn hsl_to_rgb(h: f64, s: f64, l: f64) -> [f64; 3] {
    let h = h.rem_euclid(6.0);
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s.clamp(0.0, 1.0);
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let m = l - 0.5 * c;
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    [r + m, g + m, b + m]
}

/// RGB to HSL (hue 0…6; grays have hue 0).
pub fn rgb_to_hsl(c: Rgba) -> [f64; 3] {
    let (r, g, b) = (c[0], c[1], c[2]);
    let mx = r.max(g).max(b);
    let mn = r.min(g).min(b);
    let l = 0.5 * (mx + mn);
    let d = mx - mn;
    if d <= 1e-12 {
        return [0.0, 0.0, l];
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs()).max(1e-12);
    let h = if mx == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if mx == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    [h, s, l]
}

/// `blend(a, b, t)`: linear interpolation of all four channels.
pub fn blend(a: Rgba, b: Rgba, t: f64) -> Rgba {
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
}

/// `compose(bottom, top, amount)`: `top` over `bottom` with the top's alpha times `amount`.
pub fn compose(bottom: Rgba, top: Rgba, amount: f64) -> Rgba {
    let a = top[3] * amount;
    let rgb: [f64; 3] = std::array::from_fn(|i| bottom[i] + (top[i] - bottom[i]) * a);
    [rgb[0], rgb[1], rgb[2], bottom[3] + a * (1.0 - bottom[3])]
}

/// `merge…(bottom, top)`: the blend mode applied to the colors; the top's alpha.
pub fn merge(mode: MergeMode, bottom: Rgba, top: Rgba) -> Rgba {
    let f = |c: Rgba| [c[0] as f32, c[1] as f32, c[2] as f32];
    let m = crate::render::composite::blend(mode, f(bottom), f(top));
    [m[0] as f64, m[1] as f64, m[2] as f64, top[3]]
}

/// Merge mode of a UFM `merge…` function name (lowercase).
pub fn merge_mode(name: &str) -> Option<MergeMode> {
    Some(match name {
        "mergenormal" => MergeMode::Normal,
        "mergemultiply" => MergeMode::Multiply,
        "mergescreen" => MergeMode::Screen,
        "mergeoverlay" => MergeMode::Overlay,
        "mergesoftlight" => MergeMode::SoftLight,
        "mergehardlight" => MergeMode::HardLight,
        "mergecolordodge" => MergeMode::ColorDodge,
        "mergecolorburn" => MergeMode::ColorBurn,
        "mergedarken" => MergeMode::Darken,
        "mergelighten" => MergeMode::Lighten,
        "mergedifference" => MergeMode::Difference,
        "mergeexclusion" => MergeMode::Exclusion,
        "mergeaddition" => MergeMode::Add,
        "mergesubtraction" => MergeMode::Subtract,
        "mergehue" => MergeMode::Hue,
        "mergesaturation" => MergeMode::Saturation,
        "mergecolor" => MergeMode::Color,
        "mergeluminance" => MergeMode::Luminosity,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsl_round_trip() {
        for c in [[1.0, 0.0, 0.0, 1.0], [0.2, 0.7, 0.4, 1.0], [0.9, 0.85, 0.1, 1.0], [0.3, 0.3, 0.8, 1.0]] {
            let [h, s, l] = rgb_to_hsl(c);
            let back = hsl_to_rgb(h, s, l);
            for i in 0..3 {
                assert!((back[i] - c[i]).abs() < 1e-9, "{c:?} -> {back:?}");
            }
        }
        assert_eq!(hsl_to_rgb(2.0, 1.0, 0.5), [0.0, 1.0, 0.0]);
    }
}
