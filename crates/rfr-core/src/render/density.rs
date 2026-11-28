// SPDX-License-Identifier: LGPL-3.0-or-later

//! Density layers: orbits of the layer's formula from random start points are counted per
//! pixel (Buddhabrot, anti-Buddhabrot, Nebulabrot, strange attractors), and the counts are
//! tone-mapped to a color. The GPU renderer (`density.wgsl`) follows the same steps: the same
//! start points (integer hash), the same pixel mapping, the same tone curve.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use super::{LayerPlan, ViewFrame, hash};
use crate::doc::{Density, DensityMode};
use crate::formula::cpu::{Compiled, Frame, Hook};
use crate::math::complex::{C64, Cx};

/// Counts per pixel and channel (1, or 3 for Nebulabrot) of the render's window (pixel
/// (`x0` + x, `y0` + y) of the image) and the largest count per channel (updated after every
/// pass); with Metropolis sampling the state of every orbit's chain.
pub struct Histogram {
    pub x0: u32,
    pub y0: u32,
    pub width: u32,
    pub height: u32,
    pub channels: usize,
    pub counts: Vec<AtomicU32>,
    pub max: [AtomicU32; 3],
    pub chains: Vec<Mutex<Chain>>,
}

/// The state of a Metropolis chain: its start point, how many of its orbit's points fall into
/// the view (0: no state yet), the channels the orbit counts in and the score of the chain's
/// first state (its importance weight).
#[derive(Clone, Copy, Debug, Default)]
pub struct Chain {
    pub c: [f64; 2],
    pub score: u32,
    pub mask: u32,
    pub first: u32,
}

impl Histogram {
    /// The counts of `window`; `chains`: the orbits per pass when the layer samples with
    /// Metropolis chains, else 0.
    pub fn new(window: super::Window, channels: usize, chains: usize) -> Histogram {
        let (width, height) = (window.width, window.height);
        Histogram {
            x0: window.x,
            y0: window.y,
            width,
            height,
            channels,
            counts: (0..width as usize * height as usize * channels).map(|_| AtomicU32::new(0)).collect(),
            max: [AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0)],
            chains: (0..chains).map(|_| Mutex::new(Chain::default())).collect(),
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

    /// The count of image pixel (x, y) (in the window).
    pub fn count(&self, ch: usize, x: u32, y: u32) -> u32 {
        let (x, y) = (x - self.x0, y - self.y0);
        self.counts[ch * (self.width * self.height) as usize + (y * self.width + x) as usize].load(Ordering::Relaxed)
    }

    /// Adds `weight` to image pixel (x, y) in the channels of `mask`, when it lies in the window.
    pub fn add(&self, x: u32, y: u32, mask: u32, weight: u32) {
        let (Some(lx), Some(ly)) = (x.checked_sub(self.x0), y.checked_sub(self.y0)) else { return };
        if lx >= self.width || ly >= self.height {
            return;
        }
        let at = (ly * self.width + lx) as usize;
        let plane = (self.width * self.height) as usize;
        for c in 0..self.channels {
            if mask & (1 << c) != 0 {
                self.counts[c * plane + at].fetch_add(weight, Ordering::Relaxed);
            }
        }
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

/// Whether a layer samples its start points with Metropolis chains (escaping and captive
/// orbits only).
pub fn metropolis(d: &Density) -> bool {
    d.metropolis && matches!(d.mode, DensityMode::Escaping | DensityMode::Captive)
}

/// The random numbers (0..1) of the Metropolis step of orbit `i` in pass `pass`: whether to
/// jump anywhere, the step's size and direction, the acceptance and the weight's rounding.
pub fn mh_randoms(pass: u32, i: u32) -> [f64; 5] {
    let a = hash(i ^ hash(pass.wrapping_mul(0x9E37_79B9) ^ 0x5bd1_e995));
    let mut r = hash(a ^ 0x68E3_1DA4) ^ 0x27D4_EB2F;
    let mut out = [0.0; 5];
    for v in &mut out {
        r = hash(r);
        *v = r as f64 / 4294967296.0;
    }
    out
}

/// The proposed start point: a jump to `start` (one in ten, or without a state yet), else a
/// step from the chain's point of log-uniform size (1e-5 to 0.1 of the region) in a random
/// direction. Both proposals are symmetric, so the acceptance is the ratio of the scores.
pub fn mh_proposal(d: &Density, r: &[f64; 5], start: C64, cur: Option<[f64; 2]>) -> C64 {
    match cur {
        Some(c) if r[0] >= 0.1 => {
            let size = (d.region[2] - d.region[0]).abs().max((d.region[3] - d.region[1]).abs());
            let len = size * 0.1 * 1e-4f64.powf(r[1]);
            let a = std::f64::consts::TAU * r[2];
            C64::new(c[0] + len * a.cos(), c[1] + len * a.sin())
        }
        _ => start,
    }
}

/// Whether a point lies in the density's region.
pub fn in_region(d: &Density, c: C64) -> bool {
    let r = d.region;
    c.re >= r[0].min(r[2]) && c.re <= r[0].max(r[2]) && c.im >= r[1].min(r[3]) && c.im <= r[1].max(r[3])
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

/// Count channels: 3 for a Nebulabrot (not in IFS mode), else 1.
pub fn channels(d: &Density) -> usize {
    if d.nebula && matches!(d.mode, DensityMode::Escaping | DensityMode::Captive | DensityMode::Attractor) { 3 } else { 1 }
}

/// The IFS maps for the GPU: per map (a, b, c, d) and (e, f, cumulative share of the steps,
/// variation index).
pub fn ifs_words(d: &Density) -> Vec<[f32; 4]> {
    let total: f64 = d.ifs.iter().map(|m| m.weight.max(0.0)).sum();
    let mut acc = 0.0;
    let mut out = Vec::new();
    for m in &d.ifs {
        acc += m.weight.max(0.0);
        let c = m.coefs;
        out.push([c[0] as f32, c[1] as f32, c[2] as f32, c[3] as f32]);
        out.push([c[4] as f32, c[5] as f32, if total > 0.0 { (acc / total) as f32 } else { 0.0 }, m.variation.index() as f32]);
    }
    if out.is_empty() {
        out.push([0.0; 4]);
    }
    out
}

/// The extent of an IFS attractor (a short chaos game) or of an L-system's strokes: lower left
/// and upper right corner.
pub fn extent(d: &Density) -> Option<([f64; 2], [f64; 2])> {
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    let mut add = |x: f64, y: f64| {
        if x.is_finite() && y.is_finite() {
            lo = [lo[0].min(x), lo[1].min(y)];
            hi = [hi[0].max(x), hi[1].max(y)];
        }
    };
    match d.mode {
        DensityMode::LSystem => {
            for s in d.lsystem.segments() {
                add(s[0], s[1]);
                add(s[2], s[3]);
            }
        }
        DensityMode::Ifs if !d.ifs.is_empty() => {
            let total: f64 = d.ifs.iter().map(|m| m.weight.max(0.0)).sum::<f64>().max(1e-12);
            let (mut x, mut y) = (0.0, 0.0);
            let mut rnd = 0x1234_5678u32;
            for it in 0..20000 {
                rnd = hash(rnd);
                let u = rnd as f64 / 4294967296.0 * total;
                let mut acc = 0.0;
                let m = d
                    .ifs
                    .iter()
                    .find(|m| {
                        acc += m.weight.max(0.0);
                        u < acc
                    })
                    .unwrap_or(&d.ifs[d.ifs.len() - 1]);
                (x, y) = m.apply(x, y);
                if it > 20 {
                    add(x, y);
                }
            }
        }
        _ => return None,
    }
    lo[0].is_finite().then_some((lo, hi))
}

/// Fits the document's view (and the layer's start region) to a density layer's extent.
pub fn fit_view(doc: &mut crate::doc::Document, layer: &[usize]) -> bool {
    let Some(l) = doc.layer(layer) else { return false };
    let Some((lo, hi)) = extent(&l.density) else { return false };
    let aspect = doc.image.width as f64 / doc.image.height.max(1) as f64;
    let half = ((hi[1] - lo[1]) * 0.55).max((hi[0] - lo[0]) * 0.55 / aspect).max(1e-9);
    doc.view.set_center([(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0]);
    doc.view.radius = half.into();
    if let Some(l) = doc.layer_mut(layer) {
        l.density.region = [lo[0], lo[1], hi[0], hi[1]];
    }
    true
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

/// An L-system's segments with their cumulative lengths (normalized to end at 1).
#[derive(Clone, Debug, Default)]
pub struct Strokes {
    pub segments: Vec<[f64; 4]>,
    pub cum: Vec<f64>,
}

impl Strokes {
    pub fn new(d: &Density) -> Strokes {
        if d.mode != DensityMode::LSystem {
            return Strokes::default();
        }
        let segments = d.lsystem.segments();
        let mut acc = 0.0;
        let mut cum: Vec<f64> = segments
            .iter()
            .map(|s| {
                acc += ((s[2] - s[0]).powi(2) + (s[3] - s[1]).powi(2)).sqrt();
                acc
            })
            .collect();
        if acc > 0.0 {
            cum.iter_mut().for_each(|c| *c /= acc);
        }
        Strokes { segments, cum }
    }

    /// The segments for the GPU: (x0, y0, x1, y1) and (cumulative share, 0, 0, 0) each.
    pub fn words(&self) -> Vec<[f32; 4]> {
        let mut out: Vec<[f32; 4]> =
            self.segments.iter().zip(&self.cum).flat_map(|(s, c)| [s.map(|v| v as f32), [*c as f32, 0.0, 0.0, 0.0]]).collect();
        if out.is_empty() {
            out.push([0.0; 4]);
        }
        out
    }
}

/// One pass of a density layer's orbits into its counts.
pub struct Scatter<'a> {
    pub fractal: &'a Compiled<f64>,
    pub lp: &'a LayerPlan,
    pub density: &'a Density,
    pub frame: &'a ViewFrame,
    pub hist: &'a Histogram,
    pub pass: u32,
    pub strokes: &'a Strokes,
}

impl Scatter<'_> {
    /// Iterates orbit `i` of the pass and counts its points.
    pub fn orbit(&self, f: &mut Frame<f64>, i: u32) {
        scatter_orbit(self, f, i)
    }
}

/// The chaos game: orbit `i` of the pass picks a map per step by weight (the same hashed random
/// numbers as the GPU) and counts its points after the warm-up.
fn ifs_orbit(s: &Scatter, i: u32) {
    let d = s.density;
    let total: f64 = d.ifs.iter().map(|m| m.weight.max(0.0)).sum();
    if d.ifs.is_empty() || total <= 0.0 {
        return;
    }
    let start = start_point(d, s.pass, i);
    let (mut x, mut y) = (start.re, start.im);
    let mut rnd = hash(i ^ hash(s.pass ^ 0xA511_E9B3));
    for it in 0..s.lp.max_iter {
        rnd = hash(rnd);
        let u = rnd as f64 / 4294967296.0 * total;
        let mut acc = 0.0;
        let map = d.ifs.iter().find(|m| {
            acc += m.weight.max(0.0);
            u < acc
        });
        let m = map.unwrap_or(&d.ifs[d.ifs.len() - 1]);
        (x, y) = m.apply(x, y);
        if !(x.is_finite() && y.is_finite()) {
            return;
        }
        if it >= d.warmup
            && let Some((px, py)) = pixel_of(s.frame, C64::new(x, y))
        {
            s.hist.add(px, py, 1, 1);
        }
    }
}

/// A random walk over the group's generators (never the inverse of the step before): the
/// points gather on the limit set. The same hashed choices as the GPU.
fn kleinian_orbit(s: &Scatter, i: u32) {
    let d = s.density;
    let g = d.kleinian.generators();
    let start = start_point(d, s.pass, i);
    let mut z = start;
    let mut rnd = hash(i ^ hash(s.pass ^ 0x7F4A_7C15));
    let mut prev = 4usize;
    for it in 0..s.lp.max_iter {
        rnd = hash(rnd);
        let pick = (rnd >> 8) % 3;
        // The three generators other than the inverse of the previous one.
        let banned = if prev < 4 { (prev + 2) % 4 } else { 4 };
        let mut k = pick as usize;
        if banned < 4 && k >= banned {
            k += 1;
        }
        if banned == 4 {
            k = ((rnd >> 8) % 4) as usize;
        }
        prev = k;
        let m = &g[k];
        let den = m[2] * z + m[3];
        if den.norm() < 1e-30 {
            z = C64::new(1e10, 0.0);
            continue;
        }
        z = (m[0] * z + m[1]) / den;
        if !(z.re.is_finite() && z.im.is_finite()) {
            return;
        }
        if it >= d.warmup
            && let Some((px, py)) = pixel_of(s.frame, z)
        {
            s.hist.add(px, py, 1, 1);
        }
    }
}

/// The Kleinian generators for the GPU: per matrix (p, q) and (r, s) as (re, im, re, im).
pub fn kleinian_words(d: &Density) -> Vec<[f32; 4]> {
    d.kleinian
        .generators()
        .iter()
        .flat_map(|m| {
            [[m[0].re as f32, m[0].im as f32, m[1].re as f32, m[1].im as f32], [m[2].re as f32, m[2].im as f32, m[3].re as f32, m[3].im as f32]]
        })
        .collect()
}

/// Points on the strokes: a segment by length, a place along it and across the stroke width
/// (the same hashed numbers as the GPU, the segment found by its cumulative share).
fn lsystem_orbit(s: &Scatter, i: u32) {
    let st = s.strokes;
    let n = st.segments.len();
    if n == 0 {
        return;
    }
    let w = s.density.lsystem.width;
    let mut rnd = hash(i ^ hash(s.pass ^ 0x3C6E_F372));
    for _ in 0..s.lp.max_iter {
        rnd = hash(rnd);
        let u = rnd as f64 / 4294967296.0;
        rnd = hash(rnd);
        let t = rnd as f64 / 4294967296.0;
        rnd = hash(rnd);
        let v = rnd as f64 / 4294967296.0 - 0.5;
        // The first segment whose cumulative share passes u.
        let (mut lo, mut hi) = (0usize, n - 1);
        while lo < hi {
            let mid = (lo + hi) / 2;
            if st.cum[mid] > u { hi = mid } else { lo = mid + 1 }
        }
        let g = st.segments[lo];
        let (dx, dy) = (g[2] - g[0], g[3] - g[1]);
        let len = (dx * dx + dy * dy).sqrt().max(1e-12);
        let (nx, ny) = (-dy / len, dx / len);
        let p = C64::new(g[0] + dx * t + nx * v * w, g[1] + dy * t + ny * v * w);
        if let Some((px, py)) = pixel_of(s.frame, p) {
            s.hist.add(px, py, 1, 1);
        }
    }
}

fn scatter_orbit(s: &Scatter, f: &mut Frame<f64>, i: u32) {
    if s.density.mode == DensityMode::LSystem {
        return lsystem_orbit(s, i);
    }
    if s.density.mode == DensityMode::Ifs {
        return ifs_orbit(s, i);
    }
    if s.density.mode == DensityMode::Kleinian {
        return kleinian_orbit(s, i);
    }
    let Scatter { fractal: k, lp, density: d, frame, hist, pass, .. } = *s;
    let start = start_point(d, pass, i);
    let maxit = max_iter(lp, d);
    let setup = |f: &mut Frame<f64>, c: C64| {
        f.pixel = Cx::new(c.re, c.im);
        f.ismand = !lp.julia;
        f.c = if lp.julia { Cx::new(lp.seed[0], lp.seed[1]) } else { f.pixel };
    };
    // Counts the orbit of `c` from step `skip` on, `weight` per point.
    let plot = |f: &mut Frame<f64>, c: C64, mask: u32, skip: u32, weight: u32| {
        setup(f, c);
        k.run_with(f, maxit, |h| {
            if let Hook::Iter(it, z, _) = h
                && it > skip
                && let Some((x, y)) = pixel_of(frame, C64::new(z.re, z.im))
            {
                hist.add(x, y, mask, weight);
            }
        });
    };
    if d.mode == DensityMode::Attractor {
        return plot(f, start, 1, d.warmup, 1);
    }
    // Whether the orbit of `c` is drawn, its channels and (Metropolis) its points in the view.
    let mh = metropolis(d);
    let check = |f: &mut Frame<f64>, c: C64| -> (bool, u32, u32) {
        setup(f, c);
        let mut score = 0;
        let o = k.run_with(f, maxit, |h| {
            if mh
                && let Hook::Iter(_, z, _) = h
                && pixel_of(frame, C64::new(z.re, z.im)).is_some()
            {
                score += 1;
            }
        });
        let keep = match d.mode {
            DensityMode::Escaping => o.escaped && o.iter >= d.min_iter,
            _ => !o.escaped,
        };
        let mask = if d.mode == DensityMode::Escaping {
            channel_mask(d, o.iter)
        } else if d.nebula {
            7
        } else {
            1
        };
        (keep && mask != 0, mask, score)
    };
    if !mh {
        let (record, mask, _) = check(f, start);
        if record {
            plot(f, start, mask, 0, 1);
        }
        return;
    }
    // Metropolis: the chain's target density is the score (the orbit's points in the view); it
    // moves to the proposal with the probability of the ratio of the scores. It starts at the
    // first start point that scores, so it carries that score as importance weight (start
    // density uniform, target density the score), and every point of its current orbit
    // counts first score / current score (rounded at random). This estimates the image of
    // uniform start points without bias from the start.
    let r = mh_randoms(pass, i);
    let Ok(mut ch) = hist.chains[i as usize].lock() else { return };
    let prop = mh_proposal(d, &r, start, (ch.score > 0).then_some(ch.c));
    let (keep, mask, score) = check(f, prop);
    let score = if keep && in_region(d, prop) { score } else { 0 };
    if score > 0 && (ch.score == 0 || r[3] * (ch.score as f64) < score as f64) {
        let first = if ch.score == 0 { score } else { ch.first };
        *ch = Chain { c: [prop.re, prop.im], score, mask, first };
    }
    if ch.score == 0 {
        return;
    }
    let weight = (ch.first as f64 / ch.score as f64 + r[4]) as u32;
    if weight > 0 {
        plot(f, C64::new(ch.c[0], ch.c[1]), ch.mask, 0, weight);
    }
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

    /// Runs `passes` passes of a density layer of `doc` into a fresh histogram.
    fn counts(doc: &Document, w: u32, h: u32, passes: u32) -> Vec<f64> {
        use super::*;
        let plan = crate::render::Plan::sized(doc, w, h).unwrap();
        let lp = plan.layers()[0];
        let d = lp.density.as_ref().unwrap();
        let k = Compiled::<f64>::new(&lp.fractal.program, &lp.fractal.values);
        let hist = Histogram::new(crate::render::Window::whole(w, h), 1, if metropolis(d) { d.orbits as usize } else { 0 });
        let strokes = Strokes::new(d);
        let mut f = k.frame();
        for pass in 0..passes {
            let s = Scatter { fractal: &k, lp, density: d, frame: &plan.frame, hist: &hist, pass, strokes: &strokes };
            (0..d.orbits).for_each(|i| s.orbit(&mut f, i));
        }
        hist.counts.iter().map(|c| c.load(Ordering::Relaxed) as f64).collect()
    }

    fn zoomed(radius: f64, metropolis: bool, orbits: u32) -> Document {
        let mut d = Document::for_formula("rfract:Mandelbrot");
        d.view.center = ["-0.15".into(), "0.75".into()];
        d.view.radius = radius.into();
        d.layers[0].kind = LayerKind::Density;
        d.layers[0].max_iter = 200;
        d.layers[0].density.orbits = orbits;
        d.layers[0].density.metropolis = metropolis;
        d
    }

    /// Metropolis chains give the same density as uniform start points (the counts are
    /// weighted by the inverse of each orbit's points in the view) and fill a small view with
    /// far fewer orbits.
    #[test]
    fn metropolis_matches_uniform_sampling() {
        let (w, h) = (16, 16);
        let uniform = counts(&zoomed(0.3, false, 50_000), w, h, 40);
        let mh = counts(&zoomed(0.3, true, 2_000), w, h, 400);
        // Compare the share of each 4 × 4 block.
        let blocks = |c: &[f64]| {
            let mut b = [0.0; 16];
            for y in 0..h {
                for x in 0..w {
                    b[(y / 4 * 4 + x / 4) as usize] += c[(y * w + x) as usize];
                }
            }
            let sum: f64 = b.iter().sum();
            b.iter().map(|v| v / sum).collect::<Vec<_>>()
        };
        let (bu, bm) = (blocks(&uniform), blocks(&mh));
        let diff: f64 = bu.iter().zip(&bm).map(|(a, b)| (a - b).abs()).sum();
        assert!(diff < 0.12, "block shares differ by {diff:.3}: uniform {bu:.3?}, metropolis {bm:.3?}");
        // A small view: the same number of orbits lights far more pixels with chains.
        let lit = |c: Vec<f64>| c.iter().filter(|&&v| v > 0.0).count();
        let (u, m) = (lit(counts(&zoomed(0.01, false, 2_000), w, h, 200)), lit(counts(&zoomed(0.01, true, 2_000), w, h, 200)));
        assert!(m > 2 * u.max(10), "lit pixels: uniform {u}, metropolis {m}");
    }
}
