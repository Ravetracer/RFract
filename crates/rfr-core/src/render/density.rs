// SPDX-License-Identifier: LGPL-3.0-or-later

//! Density layers: orbits of the layer's formula from random start points are counted per
//! pixel (Buddhabrot, anti-Buddhabrot, Nebulabrot, strange attractors), and the counts are
//! tone-mapped to a color. The GPU renderer (`density.wgsl`) follows the same steps: the same
//! start points (integer hash), the same pixel mapping, the same tone curve.

use std::sync::atomic::{AtomicU32, Ordering};

use super::{LayerPlan, ViewFrame, hash};
use crate::doc::{Density, DensityMode};
use crate::formula::cpu::{Compiled, Frame, Hook};
use crate::math::complex::{C64, Cx};

/// Counts per pixel and channel (1, or 3 for Nebulabrot) and the largest count per channel
/// (updated after every pass).
pub struct Histogram {
    pub width: u32,
    pub height: u32,
    pub channels: usize,
    pub counts: Vec<AtomicU32>,
    pub max: [AtomicU32; 3],
}

impl Histogram {
    pub fn new(width: u32, height: u32, channels: usize) -> Histogram {
        Histogram {
            width,
            height,
            channels,
            counts: (0..width as usize * height as usize * channels).map(|_| AtomicU32::new(0)).collect(),
            max: [AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0)],
        }
    }

    /// Finds the largest count per channel (after a pass).
    pub fn update_max(&self) {
        use rayon::prelude::*;
        let plane = (self.width * self.height) as usize;
        for c in 0..self.channels {
            let m = self.counts[c * plane..(c + 1) * plane].par_iter().map(|v| v.load(Ordering::Relaxed)).max().unwrap_or(0);
            self.max[c].store(m, Ordering::Relaxed);
        }
    }

    pub fn count(&self, ch: usize, x: u32, y: u32) -> u32 {
        self.counts[ch * (self.width * self.height) as usize + (y * self.width + x) as usize].load(Ordering::Relaxed)
    }
}

/// The start point of orbit `i` of pass `pass` in the density's region.
pub fn start_point(d: &Density, pass: u32, i: u32) -> C64 {
    let a = hash(i ^ hash(pass.wrapping_mul(0x9E37_79B9) ^ 0x5bd1_e995));
    let b = hash(a ^ 0x68E3_1DA4);
    let (u, v) = (a as f64 / 4294967296.0, b as f64 / 4294967296.0);
    let r = d.region;
    C64::new(r[0] + (r[2] - r[0]) * u, r[1] + (r[3] - r[1]) * v)
}

/// The pixel a plane point falls into (inverse of [`ViewFrame::plane`]).
pub fn pixel_of(f: &ViewFrame, z: C64) -> Option<(u32, u32)> {
    let s = f.radius / (f.height as f64 * 0.5);
    let (dx, dy) = (z.re - f.center[0], z.im - f.center[1]);
    let u = (dx * f.cos + dy * f.sin) / s;
    let v = (-dx * f.sin + dy * f.cos) / s;
    let px = u + f.width as f64 * 0.5;
    let py = f.height as f64 * 0.5 - v;
    if px >= 0.0 && py >= 0.0 && px < f.width as f64 && py < f.height as f64 { Some((px as u32, py as u32)) } else { None }
}

/// The iteration limit of the orbits (Nebulabrot: the largest channel limit).
pub fn max_iter(lp: &LayerPlan, d: &Density) -> u32 {
    if d.nebula { d.nebula_iter.iter().copied().max().unwrap_or(1).max(1) } else { lp.max_iter }
}

/// Channels an orbit of `n` iterations counts in (bit mask).
pub fn channel_mask(d: &Density, n: u32) -> u32 {
    if !d.nebula {
        return 1;
    }
    (0..3).filter(|&c| n <= d.nebula_iter[c]).fold(0, |m, c| m | (1 << c))
}

/// One pass of a density layer's orbits into its counts.
pub struct Scatter<'a> {
    pub fractal: &'a Compiled<f64>,
    pub lp: &'a LayerPlan,
    pub density: &'a Density,
    pub frame: &'a ViewFrame,
    pub hist: &'a Histogram,
    pub pass: u32,
}

impl Scatter<'_> {
    /// Iterates orbit `i` of the pass and counts its points.
    pub fn orbit(&self, f: &mut Frame<f64>, i: u32) {
        scatter_orbit(self, f, i)
    }
}

fn scatter_orbit(s: &Scatter, f: &mut Frame<f64>, i: u32) {
    let Scatter { fractal: k, lp, density: d, frame, hist, pass } = *s;
    let start = start_point(d, pass, i);
    let maxit = max_iter(lp, d);
    let setup = |f: &mut Frame<f64>| {
        f.pixel = Cx::new(start.re, start.im);
        f.ismand = !lp.julia;
        f.c = if lp.julia { Cx::new(lp.seed[0], lp.seed[1]) } else { f.pixel };
    };
    let (record, mask, skip) = match d.mode {
        DensityMode::Attractor => (true, 1, d.warmup),
        mode => {
            setup(f);
            let o = k.run(f, maxit);
            let keep = match mode {
                DensityMode::Escaping => o.escaped && o.iter >= d.min_iter,
                _ => !o.escaped,
            };
            let mask = if mode == DensityMode::Escaping { channel_mask(d, o.iter) } else { if d.nebula { 7 } else { 1 } };
            (keep && mask != 0, mask, 0)
        }
    };
    if !record {
        return;
    }
    setup(f);
    k.run_with(f, maxit, |h| {
        if let Hook::Iter(it, z, _) = h
            && it > skip
            && let Some((x, y)) = pixel_of(frame, C64::new(z.re, z.im))
        {
            let at = (y * hist.width + x) as usize;
            let plane = (hist.width * hist.height) as usize;
            for c in 0..hist.channels {
                if mask & (1 << c) != 0 {
                    hist.counts[c * plane + at].fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    });
}

/// Whether the accumulated image restarts after `passes` density passes for a render of
/// `target` samples: when the passes double, as long as at least half of the samples are still
/// to come (open-ended renders: up to 64 passes).
pub fn restart_due(passes: u32, target: u32) -> bool {
    passes.is_power_of_two() && passes.saturating_mul(2) <= target.min(128)
}

/// Tone curve: log density relative to the largest count, times the brightness (clamped),
/// then gamma.
pub fn tone(count: u32, max: u32, brightness: f64, gamma: f64) -> f64 {
    if count == 0 || max == 0 {
        return 0.0;
    }
    let v = ((count as f64).ln_1p() / (max as f64).ln_1p() * brightness.max(0.0)).min(1.0);
    v.powf(1.0 / gamma.max(0.05))
}

/// The layer color at pixel (x, y): linear RGB with straight alpha. One channel: the outside
/// gradient at the tone value, which is also the alpha; Nebulabrot: RGB from three channels.
pub fn color(lp: &LayerPlan, d: &Density, hist: &Histogram, x: u32, y: u32) -> [f32; 4] {
    let t = |c: usize| tone(hist.count(c, x, y), hist.max[c].load(Ordering::Relaxed), d.brightness, d.gamma) as f32;
    if hist.channels == 3 {
        let rgb = [t(0), t(1), t(2)];
        let a = rgb[0].max(rgb[1]).max(rgb[2]);
        if a <= 0.0 {
            return [0.0; 4];
        }
        return [rgb[0] / a, rgb[1] / a, rgb[2] / a, a];
    }
    let v = t(0);
    if v <= 0.0 {
        return [0.0; 4];
    }
    // The gradient's last entry at full density (the lookup wraps at 1).
    let c = crate::gradient::lut_sample(&lp.outside.lut, v as f64 * (1.0 - 1.0 / lp.outside.lut.len().max(1) as f64));
    [c[0], c[1], c[2], c[3] * v]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixel_mapping_inverts_the_view() {
        let f = ViewFrame { width: 40, height: 30, center: [-0.5, 0.2], radius: 1.5, radius_fx: 1.5.into(), cos: 0.6, sin: 0.8 };
        for (x, y) in [(3.5, 4.5), (20.5, 15.5), (39.5, 29.5)] {
            let p = f.plane(x, y);
            assert_eq!(pixel_of(&f, p), Some((x as u32, y as u32)));
        }
        assert_eq!(tone(0, 100, 1.0, 2.0), 0.0);
        assert_eq!(tone(100, 100, 1.0, 2.0), 1.0);
        assert!(tone(10, 100, 1.0, 1.0) > tone(1, 100, 1.0, 1.0));
    }
}

#[cfg(test)]
mod render_tests {
    use crate::doc::{DensityMode, Document, LayerKind};

    /// Buddhabrot: symmetric about the real axis, bright inside the view, empty far outside.
    #[test]
    fn buddhabrot_and_attractor_render() {
        let mut d = Document::for_formula("rfract:Mandelbrot");
        d.view.center = ["-0.5".into(), "0".into()];
        d.view.radius = 1.4.into();
        d.layers[0].kind = LayerKind::Density;
        d.layers[0].max_iter = 200;
        d.layers[0].density.orbits = 20000;
        let (w, h) = (48, 32);
        let img = crate::render::cpu::render(&d, w, h, 4).unwrap().rgba8();
        let px = |x: u32, y: u32| img[((y * w + x) * 4) as usize] as i32 + img[((y * w + x) * 4 + 1) as usize] as i32;
        let (mut top, mut bottom) = (0, 0);
        for y in 0..h / 2 {
            for x in 0..w {
                top += px(x, y);
                bottom += px(x, h - 1 - y);
            }
        }
        assert!(top > 1000, "the Buddhabrot shows ({top})");
        assert!((top - bottom).abs() * 5 < top, "symmetric: {top} against {bottom}");
        // An attractor from the bundled set.
        let mut a = Document::for_formula("rfract-attractor:Clifford");
        a.layers[0].kind = LayerKind::Density;
        a.layers[0].density.mode = DensityMode::Attractor;
        a.layers[0].density.orbits = 2000;
        a.image.background = [0.0; 4];
        let img = crate::render::cpu::render(&a, w, h, 2).unwrap().rgba8();
        let lit = img.chunks(4).filter(|p| p[3] > 20).count();
        assert!(lit > 100 && lit < (w * h) as usize, "{lit} pixels of the attractor");
    }
}
