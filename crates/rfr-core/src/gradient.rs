// SPDX-License-Identifier: LGPL-3.0-or-later

//! Color gradients: stops (position, linear RGB) blended in OKLab and repeating cyclically.
//! The renderers sample a [`LUT_SIZE`]-entry table made here, so CPU and GPU colors agree.

use serde::{Deserialize, Serialize};

use crate::palette::{from_oklab, to_oklab};

pub const LUT_SIZE: usize = 1024;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stop {
    pub pos: f64,
    /// Linear RGB, 0..1.
    pub color: [f32; 3],
}

impl Stop {
    pub fn new(pos: f64, color: [f32; 3]) -> Stop {
        Stop { pos, color }
    }
    /// A stop from 8-bit sRGB.
    pub fn srgb(pos: f64, r: u8, g: u8, b: u8) -> Stop {
        Stop { pos, color: [srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b)] }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Blend {
    /// Straight blends between neighboring stops.
    Linear,
    /// Eased blends (no visible corners at the stops).
    #[default]
    Smooth,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Gradient {
    pub stops: Vec<Stop>,
    pub blend: Blend,
    /// Opacity along the gradient (its own positions, linear blends, cyclic); empty: opaque.
    pub alpha: Vec<AlphaStop>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AlphaStop {
    pub pos: f64,
    pub alpha: f32,
}

/// Opacity at `t` (wrapped): linear blends between the stops, cyclic like the colors.
pub fn sample_alpha(stops: &[AlphaStop], t: f64) -> f32 {
    match stops.len() {
        0 => return 1.0,
        1 => return stops[0].alpha,
        _ => {}
    }
    let mut s = stops.to_vec();
    s.sort_by(|a, b| a.pos.total_cmp(&b.pos));
    let t = t.rem_euclid(1.0);
    if let Some(i) = s.windows(2).position(|w| t >= w[0].pos && t <= w[1].pos) {
        let (a, b) = (&s[i], &s[i + 1]);
        let span = b.pos - a.pos;
        let u = if span > 0.0 { ((t - a.pos) / span) as f32 } else { 0.0 };
        return a.alpha + (b.alpha - a.alpha) * u;
    }
    let (last, first) = (&s[s.len() - 1], &s[0]);
    let span = first.pos + 1.0 - last.pos;
    let tt = if t < first.pos { t + 1.0 } else { t };
    let u = if span > 0.0 { ((tt - last.pos) / span) as f32 } else { 0.0 };
    last.alpha + (first.alpha - last.alpha) * u
}

impl Default for Gradient {
    fn default() -> Self {
        preset("Ice and fire").unwrap_or(Gradient {
            stops: vec![Stop::new(0.0, [0.0; 3]), Stop::new(1.0, [1.0; 3])],
            blend: Blend::Smooth,
            alpha: Vec::new(),
        })
    }
}

pub fn srgb_to_linear(c: u8) -> f32 {
    let v = c as f32 / 255.0;
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

pub fn linear_to_srgb(v: f32) -> u8 {
    let v = v.clamp(0.0, 1.0);
    let s = if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
    (s * 255.0 + 0.5) as u8
}

/// Stops as (position, sRGB red, green, blue).
type PresetStops = &'static [(f64, u8, u8, u8)];

/// Named gradients for new layers and the gradient editor.
const PRESETS: &[(&str, PresetStops)] = &[
    (
        "Ice and fire",
        &[
            (0.0, 8, 12, 40),
            (0.18, 30, 90, 200),
            (0.36, 120, 220, 255),
            (0.5, 255, 250, 235),
            (0.66, 255, 170, 40),
            (0.82, 140, 20, 40),
            (1.0, 8, 12, 40),
        ],
    ),
    ("Gold", &[(0.0, 20, 8, 0), (0.3, 140, 70, 10), (0.55, 255, 210, 110), (0.7, 255, 250, 220), (0.85, 170, 100, 20), (1.0, 20, 8, 0)]),
    ("Ocean", &[(0.0, 0, 7, 30), (0.3, 0, 60, 110), (0.55, 20, 160, 180), (0.75, 200, 240, 230), (1.0, 0, 7, 30)]),
    ("Neon", &[(0.0, 5, 0, 15), (0.2, 255, 0, 160), (0.35, 20, 0, 40), (0.6, 0, 240, 255), (0.75, 10, 10, 50), (0.9, 255, 230, 0), (1.0, 5, 0, 15)]),
    ("Forest", &[(0.0, 5, 15, 5), (0.35, 40, 110, 40), (0.6, 190, 210, 120), (0.8, 90, 60, 20), (1.0, 5, 15, 5)]),
    ("Grayscale", &[(0.0, 0, 0, 0), (0.5, 255, 255, 255), (1.0, 0, 0, 0)]),
    // Dark to bright (density layers: few points dark, many bright).
    ("Ember", &[(0.0, 0, 0, 0), (0.3, 90, 10, 20), (0.55, 220, 80, 20), (0.8, 255, 200, 80), (1.0, 255, 255, 235)]),
    ("Starlight", &[(0.0, 0, 0, 0), (0.3, 20, 30, 110), (0.6, 60, 150, 230), (0.85, 200, 235, 255), (1.0, 255, 255, 255)]),
];

pub fn preset_names() -> impl Iterator<Item = &'static str> {
    PRESETS.iter().map(|(n, _)| *n)
}

pub fn preset(name: &str) -> Option<Gradient> {
    PRESETS.iter().find(|(n, _)| *n == name).map(|(_, s)| Gradient {
        stops: s.iter().map(|&(p, r, g, b)| Stop::srgb(p, r, g, b)).collect(),
        blend: Blend::Smooth,
        alpha: Vec::new(),
    })
}

fn sorted(stops: &[Stop]) -> Vec<Stop> {
    let mut s = stops.to_vec();
    s.sort_by(|a, b| a.pos.total_cmp(&b.pos));
    s
}

fn mix(a: [f32; 3], b: [f32; 3], u: f64) -> [f32; 3] {
    if a == b || u <= 0.0 {
        return a;
    }
    if u >= 1.0 {
        return b;
    }
    let (la, lb) = (to_oklab(a), to_oklab(b));
    let m = [la[0] + (lb[0] - la[0]) * u, la[1] + (lb[1] - la[1]) * u, la[2] + (lb[2] - la[2]) * u];
    from_oklab(m).map(|v| v.clamp(0.0, 1.0) as f32)
}

/// Color at `t` (wrapped into 0..1). Before the first and after the last stop the gradient
/// blends from the last stop to the first, so it repeats without a seam.
pub fn sample_blend(stops: &[Stop], t: f64, blend: Blend) -> [f32; 3] {
    match stops.len() {
        0 => return [0.0; 3],
        1 => return stops[0].color,
        _ => {}
    }
    let s = sorted(stops);
    let t = t.rem_euclid(1.0);
    let ease = |u: f64| if blend == Blend::Smooth { u * u * (3.0 - 2.0 * u) } else { u };
    if let Some(i) = s.windows(2).position(|w| t >= w[0].pos && t <= w[1].pos) {
        let (a, b) = (&s[i], &s[i + 1]);
        let span = b.pos - a.pos;
        let u = if span > 0.0 { (t - a.pos) / span } else { 0.0 };
        return mix(a.color, b.color, ease(u));
    }
    let (last, first) = (&s[s.len() - 1], &s[0]);
    let span = first.pos + 1.0 - last.pos;
    let tt = if t < first.pos { t + 1.0 } else { t };
    let u = if span > 0.0 { (tt - last.pos) / span } else { 0.0 };
    mix(last.color, first.color, ease(u))
}

/// [`sample_blend`] with linear blends.
pub fn sample(stops: &[Stop], t: f64) -> [f32; 3] {
    sample_blend(stops, t, Blend::Linear)
}

impl Gradient {
    pub fn sample(&self, t: f64) -> [f32; 3] {
        sample_blend(&self.stops, t, self.blend)
    }

    /// The lookup table the renderers use (linear RGB plus alpha 1).
    pub fn lut(&self) -> Vec<[f32; 4]> {
        (0..LUT_SIZE)
            .map(|i| {
                let t = i as f64 / LUT_SIZE as f64;
                let c = self.sample(t);
                [c[0], c[1], c[2], sample_alpha(&self.alpha, t)]
            })
            .collect()
    }
}

/// Reads a gradient table at `t` (wrapped), blending between neighboring entries; the GPU's
/// `gradient()` does the same.
pub fn lut_sample(lut: &[[f32; 4]], t: f64) -> [f32; 4] {
    let n = lut.len();
    let x = t.rem_euclid(1.0) * n as f64;
    let i = (x.floor() as usize).min(n - 1);
    let j = (i + 1) % n;
    let u = (x - x.floor()) as f32;
    let (a, b) = (lut[i], lut[j]);
    [a[0] + (b[0] - a[0]) * u, a[1] + (b[1] - a[1]) * u, a[2] + (b[2] - a[2]) * u, a[3] + (b[3] - a[3]) * u]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_hit_the_stops_and_wrap() {
        let g = Gradient { stops: vec![Stop::new(0.25, [1.0, 0.0, 0.0]), Stop::new(0.75, [0.0, 0.0, 1.0])], blend: Blend::Linear, alpha: Vec::new() };
        assert_eq!(g.sample(0.25), [1.0, 0.0, 0.0]);
        assert_eq!(g.sample(0.75), [0.0, 0.0, 1.0]);
        // Wrap segment: 0.75 → 1.25 (blue → red); 0.0 is its middle, like 1.0.
        let a = g.sample(0.0);
        let b = g.sample(1.0);
        assert_eq!(a, b);
        assert!(a[0] > 0.05 && a[2] > 0.05, "{a:?}");
        assert_eq!(g.lut().len(), LUT_SIZE);
        assert!(preset_names().all(|n| preset(n).is_some()));
        let a = [AlphaStop { pos: 0.0, alpha: 0.0 }, AlphaStop { pos: 0.5, alpha: 1.0 }];
        assert_eq!(sample_alpha(&a, 0.25), 0.5);
        assert_eq!(sample_alpha(&a, 0.75), 0.5);
        assert_eq!(sample_alpha(&[], 0.3), 1.0);
    }

    #[test]
    fn srgb_round_trip() {
        for v in [0u8, 1, 10, 100, 128, 200, 255] {
            assert_eq!(linear_to_srgb(srgb_to_linear(v)), v);
        }
    }
}
