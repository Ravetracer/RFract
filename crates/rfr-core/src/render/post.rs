// SPDX-License-Identifier: LGPL-3.0-or-later

//! Effects on the finished image ([`crate::doc::Post`]), applied to the averaged image when
//! it is shown or saved. The GPU preview (rfr-gpu `display.wgsl`, `bloom.wgsl`) runs the same
//! steps in f32, so the preview and the export agree.
//!
//! 1. The image as premultiplied linear light; sharpening (unsharp mask over 3 × 3 pixels).
//! 2. Bloom: the light above the threshold is averaged into a pyramid (4 × 4 blocks, then
//!    halving), each level blurred with a 5-tap binomial filter, and the levels are added back
//!    with weights that shift toward the coarse ones as the size grows. Streaks: the light
//!    above the streak threshold is smeared along each direction with Kawase's streak filter
//!    (three passes of four taps). Glow over transparent parts raises their alpha.
//! 3. Lens distortion and chromatic aberration resample the image (red farther out, blue
//!    closer to the center).
//! 4. Per pixel on straight colors: exposure and white balance, tone mapping, saturation,
//!    split toning, sRGB encoding, contrast, vignette and film grain.
//!
//! Bloom and streaks come from RMandelbulb3D (`render/bloom.rs`).

use rayon::prelude::*;

use super::hash;
use crate::doc::{Post, ToneMap};

/// Levels of the bloom pyramid at most.
pub const LEVELS: usize = 6;

/// Sizes of the bloom pyramid levels for a `w` × `h` image.
pub fn level_sizes(w: u32, h: u32) -> Vec<(u32, u32)> {
    let mut out = vec![(w.div_ceil(4).max(1), h.div_ceil(4).max(1))];
    while out.len() < LEVELS {
        let (pw, ph) = out[out.len() - 1];
        if pw == 1 && ph == 1 {
            break;
        }
        out.push((pw.div_ceil(2).max(1), ph.div_ceil(2).max(1)));
    }
    out
}

/// The longest side of the light sources' image (see [`light_size`]).
pub const LIGHT_SIDE: u32 = 2048;

/// Size of the light sources' image ([`crate::flare::image`]) for a `w` × `h` image: the
/// image's own size up to [`LIGHT_SIDE`] on the long side (preview and exports alike).
pub fn light_size(w: u32, h: u32) -> (u32, u32) {
    let k = (LIGHT_SIDE as f64 / w.max(h).max(1) as f64).min(1.0);
    (((w as f64 * k).round() as u32).max(1), ((h as f64 * k).round() as u32).max(1))
}

/// Weights of the levels: `size^k`, normalized.
pub fn level_weights(size: f64, levels: usize) -> Vec<f32> {
    let s = size.clamp(0.0, 1.0);
    let w: Vec<f64> = (0..levels).map(|k| s.powi(k as i32)).collect();
    let sum: f64 = w.iter().sum::<f64>().max(1e-12);
    w.iter().map(|v| (v / sum) as f32).collect()
}

/// Share of a pixel's light that blooms: the part of its luminance (after `exposure`, a
/// factor) above `threshold`.
pub fn bright_factor(c: [f32; 3], exposure: f32, threshold: f32) -> f32 {
    let y = luma(c) * exposure;
    (y - threshold).max(0.0) / y.max(1e-6)
}

/// The streak directions as screen vectors (x right, y down): `count` directions spread over
/// half a turn from `angle` (degrees, counterclockwise), each both ways.
pub fn streak_dirs(angle: f64, count: u32) -> Vec<[f32; 2]> {
    let n = count.clamp(1, 4);
    let mut out = Vec::new();
    for i in 0..n {
        let a = (angle + i as f64 * 180.0 / n as f64).to_radians();
        let d = [a.cos() as f32, -a.sin() as f32];
        out.push(d);
        out.push([-d[0], -d[1]]);
    }
    out
}

/// Step of the first streak pass and the falloff per texel for a streak of `length` (share
/// of the width) on a level `lw` texels wide.
pub fn streak_params(length: f64, lw: u32) -> (f32, f32) {
    let px = (length * lw as f64).max(1.0);
    ((px / 63.0) as f32, 0.01f64.powf(1.0 / px) as f32)
}

/// Weights of the four taps of a streak pass (normalized).
pub fn streak_weights(step: f32, a: f32) -> [f32; 4] {
    let w: [f32; 4] = std::array::from_fn(|k| a.powf(k as f32 * step));
    let sum: f32 = w.iter().sum();
    w.map(|v| v / sum)
}

fn luma(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// One level of the pyramid (or the streaks).
struct Level {
    w: usize,
    h: usize,
    px: Vec<[f32; 3]>,
}

/// Average of `src` (`sw` × `sh`) over the block of `f` × `f` pixels at block (`i`, `j`).
fn block_mean(src: &[[f32; 3]], sw: usize, sh: usize, i: usize, j: usize, f: usize) -> [f32; 3] {
    let mut s = [0.0f32; 3];
    let mut n = 0.0f32;
    for y in j * f..((j + 1) * f).min(sh) {
        for x in i * f..((i + 1) * f).min(sw) {
            let c = src[y * sw + x];
            for k in 0..3 {
                s[k] += c[k];
            }
            n += 1.0;
        }
    }
    s.map(|v| v / n.max(1.0))
}

/// 5-tap binomial blur along x or y, edges clamped.
fn blur(l: &Level, horizontal: bool) -> Vec<[f32; 3]> {
    const W: [f32; 5] = [1.0 / 16.0, 4.0 / 16.0, 6.0 / 16.0, 4.0 / 16.0, 1.0 / 16.0];
    let mut out = vec![[0.0f32; 3]; l.w * l.h];
    out.par_chunks_mut(l.w).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let mut s = [0.0f32; 3];
            for (t, w) in W.iter().enumerate() {
                let (sx, sy) = if horizontal {
                    ((x as i64 + t as i64 - 2).clamp(0, l.w as i64 - 1) as usize, y)
                } else {
                    (x, (y as i64 + t as i64 - 2).clamp(0, l.h as i64 - 1) as usize)
                };
                let c = l.px[sy * l.w + sx];
                for k in 0..3 {
                    s[k] += c[k] * w;
                }
            }
            *o = s;
        }
    });
    out
}

/// Bilinear sample of level `l` at the image position (`u`, `v`) in 0..1 (edges clamped).
fn sample(l: &Level, u: f32, v: f32) -> [f32; 3] {
    let fx = (u * l.w as f32 - 0.5).clamp(0.0, l.w as f32 - 1.0);
    let fy = (v * l.h as f32 - 0.5).clamp(0.0, l.h as f32 - 1.0);
    let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(l.w - 1), (y0 + 1).min(l.h - 1));
    let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
    let p = |x: usize, y: usize| l.px[y * l.w + x];
    let (a, b, c, d) = (p(x0, y0), p(x1, y0), p(x0, y1), p(x1, y1));
    std::array::from_fn(|k| {
        let top = a[k] + (b[k] - a[k]) * tx;
        let bottom = c[k] + (d[k] - c[k]) * tx;
        top + (bottom - top) * ty
    })
}

/// Bilinear sample of `l` at texel position (`fx`, `fy`); taps outside count as black.
fn sample_zero(l: &Level, fx: f32, fy: f32) -> [f32; 3] {
    let (x0, y0) = (fx.floor(), fy.floor());
    let (tx, ty) = (fx - x0, fy - y0);
    let p = |x: f32, y: f32| if x < 0.0 || y < 0.0 || x >= l.w as f32 || y >= l.h as f32 { [0.0; 3] } else { l.px[y as usize * l.w + x as usize] };
    let (a, b, c, d) = (p(x0, y0), p(x0 + 1.0, y0), p(x0, y0 + 1.0), p(x0 + 1.0, y0 + 1.0));
    std::array::from_fn(|k| {
        let top = a[k] + (b[k] - a[k]) * tx;
        let bottom = c[k] + (d[k] - c[k]) * tx;
        top + (bottom - top) * ty
    })
}

/// One streak pass: per texel the weighted sum of four taps `k · step` along `dir`.
fn streak_pass(l: &Level, dir: [f32; 2], step: f32, a: f32) -> Vec<[f32; 3]> {
    let w = streak_weights(step, a);
    let mut out = vec![[0.0f32; 3]; l.w * l.h];
    out.par_chunks_mut(l.w).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let mut s = [0.0f32; 3];
            for (k, wk) in w.iter().enumerate() {
                let off = k as f32 * step;
                let c = sample_zero(l, x as f32 + dir[0] * off, y as f32 + dir[1] * off);
                for i in 0..3 {
                    s[i] += c[i] * wk;
                }
            }
            *o = s;
        }
    });
    out
}

/// The light of `rad` above `threshold` (after the `exposure` factor), averaged into the first
/// pyramid level.
fn bright_level(rad: &[[f32; 3]], w: usize, h: usize, exposure: f32, threshold: f32) -> Level {
    let bright: Vec<[f32; 3]> = rad
        .iter()
        .map(|c| {
            let f = bright_factor(*c, exposure, threshold);
            c.map(|v| v * f)
        })
        .collect();
    let (lw, lh) = level_sizes(w as u32, h as u32)[0];
    let (lw, lh) = (lw as usize, lh as usize);
    Level { w: lw, h: lh, px: (0..lw * lh).into_par_iter().map(|i| block_mean(&bright, w, h, i % lw, i / lw, 4)).collect() }
}

/// The blurred bloom pyramid.
fn bloom_levels(rad: &[[f32; 3]], w: usize, h: usize, post: &Post) -> Vec<Level> {
    let first = bright_level(rad, w, h, 2f32.powf(post.exposure as f32), post.bloom_threshold as f32);
    let sizes = level_sizes(w as u32, h as u32);
    let mut levels: Vec<Level> = vec![first];
    for &(lw, lh) in &sizes[1..] {
        let (lw, lh) = (lw as usize, lh as usize);
        let p = &levels[levels.len() - 1];
        let px: Vec<[f32; 3]> = (0..lw * lh).into_par_iter().map(|i| block_mean(&p.px, p.w, p.h, i % lw, i / lw, 2)).collect();
        levels.push(Level { w: lw, h: lh, px });
    }
    for l in &mut levels {
        let t = Level { w: l.w, h: l.h, px: blur(l, true) };
        l.px = blur(&t, false);
    }
    levels
}

/// The streaks (None when they are off).
fn streaks(rad: &[[f32; 3]], w: usize, h: usize, post: &Post) -> Option<Level> {
    if post.streak <= 0.0 {
        return None;
    }
    let src = bright_level(rad, w, h, 2f32.powf(post.exposure as f32), post.streak_threshold as f32);
    let (step0, a) = streak_params(post.streak_length, src.w as u32);
    let mut acc = vec![[0.0f32; 3]; src.w * src.h];
    for dir in streak_dirs(post.streak_angle, post.streak_count) {
        let mut l = Level { w: src.w, h: src.h, px: src.px.clone() };
        for p in 0..3 {
            l.px = streak_pass(&l, dir, step0 * 4f32.powi(p), a);
        }
        for (o, c) in acc.iter_mut().zip(&l.px) {
            for k in 0..3 {
                o[k] += c[k];
            }
        }
    }
    Some(Level { w: src.w, h: src.h, px: acc })
}

/// God rays at image position (u, v): the bright light of `l` along the way toward the light
/// position, weighted down with the distance.
fn god_rays(l: &Level, u: f32, v: f32, post: &Post) -> [f32; 3] {
    let (lx, ly) = (post.rays_position[0] as f32, post.rays_position[1] as f32);
    let len = post.rays_length as f32;
    let (mut s, mut wsum) = ([0.0f32; 3], 0.0f32);
    for i in 0..RAY_SAMPLES {
        let f = i as f32 / RAY_SAMPLES as f32;
        let wt = 1.0 - f;
        let c = sample(l, u + (lx - u) * f * len, v + (ly - v) * f * len);
        for k in 0..3 {
            s[k] += c[k] * wt;
        }
        wsum += wt;
    }
    s.map(|x| x / wsum * post.rays as f32)
}

/// White balance factors (temperature: red against blue, tint: green), scaled so gray keeps
/// its brightness.
pub fn white_balance(post: &Post) -> [f32; 3] {
    let (t, g) = (post.temperature.clamp(-1.0, 1.0) as f32, post.tint.clamp(-1.0, 1.0) as f32);
    let f = [1.0 + 0.3 * t, 1.0 - 0.3 * g, 1.0 - 0.3 * t];
    let l = luma(f).max(1e-6);
    f.map(|x| x / l)
}

/// Tone mapping of linear light (0 and up) into 0..1.
pub fn tone_map(t: ToneMap, c: [f32; 3]) -> [f32; 3] {
    match t {
        ToneMap::None => c,
        ToneMap::Reinhard => c.map(|x| x / (1.0 + x)),
        ToneMap::Aces => c.map(|x| ((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14)).clamp(0.0, 1.0)),
        ToneMap::Filmic => {
            // Hable's curve, exposure bias 2, white point 11.2.
            let f = |x: f32| ((x * (0.15 * x + 0.05) + 0.004) / (x * (0.15 * x + 0.5) + 0.06)) - 0.02 / 0.3;
            let white = f(11.2);
            c.map(|x| (f(2.0 * x) / white).clamp(0.0, 1.0))
        }
        ToneMap::Agx => agx(c),
    }
}

/// AgX (the minimal fit of the base look): inset into a wider space, log2 encoding between
/// -12.47 and 4.03 stops, a sigmoid, then back to linear.
#[allow(clippy::excessive_precision)] // the published matrix values
fn agx(c: [f32; 3]) -> [f32; 3] {
    // Columns of the inset and outset matrices.
    const IN: [[f32; 3]; 3] =
        [[0.842_479_06, 0.042_328_24, 0.042_375_655], [0.078_433_6, 0.878_468_6, 0.078_433_6], [0.079_223_745, 0.079_166_13, 0.879_142_97]];
    const OUT: [[f32; 3]; 3] =
        [[1.196_879, -0.052_896_85, -0.052_971_635], [-0.098_020_88, 1.151_903_1, -0.098_043_45], [-0.099_029_74, -0.098_961_18, 1.151_073_7]];
    let mul = |m: &[[f32; 3]; 3], v: [f32; 3]| -> [f32; 3] { std::array::from_fn(|r| m[0][r] * v[0] + m[1][r] * v[1] + m[2][r] * v[2]) };
    let (lo, hi) = (-12.473_93f32, 4.026_069f32);
    let v = mul(&IN, c).map(|x| ((x.max(1e-10).log2().clamp(lo, hi)) - lo) / (hi - lo));
    let v = v.map(|x| {
        let x2 = x * x;
        let x4 = x2 * x2;
        15.5 * x4 * x2 - 40.14 * x4 * x + 31.96 * x4 - 6.868 * x2 * x + 0.4298 * x2 + 0.1191 * x - 0.00232
    });
    mul(&OUT, v).map(|x| x.max(0.0).powf(2.2))
}

/// Split toning: `c` (0..1) toward the shadow color in the dark and the highlight color in the
/// bright parts, each color scaled to the brightness of gray.
fn split_tone(c: [f32; 3], post: &Post) -> [f32; 3] {
    if post.shadow_amount <= 0.0 && post.highlight_amount <= 0.0 {
        return c;
    }
    let l = luma(c).clamp(0.0, 1.0);
    let tone = |col: [f32; 3], w: f32| {
        let lc = luma(col).max(1e-3);
        col.map(|x| 1.0 + (x / lc - 1.0) * w)
    };
    let ts = tone(post.shadow_color, post.shadow_amount.clamp(0.0, 1.0) as f32 * (1.0 - l) * (1.0 - l));
    let th = tone(post.highlight_color, post.highlight_amount.clamp(0.0, 1.0) as f32 * l * l);
    [0, 1, 2].map(|k| c[k] * ts[k] * th[k])
}

fn srgb(v: f32) -> f32 {
    let c = v.clamp(0.0, 1.0);
    if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
}

/// Levels on a display value: black and white point, midtones.
pub fn levels(t: f32, post: &Post) -> f32 {
    let (b, w) = (post.levels_black as f32, post.levels_white as f32);
    ((t - b) / (w - b).max(1e-4)).clamp(0.0, 1.0).powf(1.0 / (post.levels_gamma as f32).max(0.01))
}

/// The tone curve at a display value: Catmull-Rom through the outputs at 0, ¼, ½, ¾ and 1
/// (the ends continued straight).
pub fn curve(t: f32, post: &Post) -> f32 {
    let c = post.curve.map(|v| v as f32);
    let p = |i: i32| -> f32 {
        match i {
            -1 => 2.0 * c[0] - c[1],
            5 => 2.0 * c[4] - c[3],
            i => c[i.clamp(0, 4) as usize],
        }
    };
    let x = t.clamp(0.0, 1.0) * 4.0;
    let i = (x.floor() as i32).min(3);
    let u = x - i as f32;
    let (p0, p1, p2, p3) = (p(i - 1), p(i), p(i + 1), p(i + 2));
    let v = 0.5 * (2.0 * p1 + (p2 - p0) * u + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * u * u + (3.0 * p1 - p0 - 3.0 * p2 + p3) * u * u * u);
    v.clamp(0.0, 1.0)
}

/// Samples of a god ray and the spectral chromatic aberration.
pub const RAY_SAMPLES: u32 = 48;
pub const SPECTRAL_SAMPLES: u32 = 7;

/// The weights of red, green and blue for the wavelength at `t` (-1 blue … 1 red).
pub fn spectral_weights(t: f32) -> [f32; 3] {
    [(1.0 - (t - 1.0).abs()).max(0.0), (1.0 - t.abs()).max(0.0), (1.0 - (t + 1.0).abs()).max(0.0)]
}

/// The display color (sRGB-encoded, 0..1) of a straight linear color at pixel (x, y) of a
/// `w` × `h` image.
pub fn grade(c: [f32; 3], x: u32, y: u32, w: u32, h: u32, post: &Post) -> [f32; 3] {
    let e = 2f32.powf(post.exposure as f32);
    let wb = white_balance(post);
    let c = [0, 1, 2].map(|k| (c[k] * e * wb[k]).max(0.0));
    let c = tone_map(post.tone_map, c);
    let l = luma(c);
    let s = post.saturation as f32;
    let c = split_tone(c.map(|v| l + (v - l) * s), post);
    let (du, dv) = ((x as f32 + 0.5) / w as f32 - 0.5, (y as f32 + 0.5) / h as f32 - 0.5);
    let vig = (1.0 - post.vignette as f32 * (du * du + dv * dv) * 2.0).max(0.0);
    let n = ((hash(x ^ hash(y ^ 0x5F37_59DF)) >> 8) as f32 / 16_777_216.0 - 0.5) * post.grain as f32 * 0.25;
    c.map(|v| {
        let t = (srgb(v) - 0.5) * post.contrast as f32 + 0.5;
        (curve(levels(t, post), post) * vig + n).clamp(0.0, 1.0)
    })
}

/// Premultiplied light of linear, straight-alpha pixels (not-finite values as 0).
fn premultiplied(lin: &[[f32; 4]]) -> Vec<[f32; 4]> {
    let ok = |v: f32| if v.is_finite() { v } else { 0.0 };
    lin.iter()
        .map(|c| {
            let a = ok(c[3]).clamp(0.0, 1.0);
            [ok(c[0]).max(0.0) * a, ok(c[1]).max(0.0) * a, ok(c[2]).max(0.0) * a, a]
        })
        .collect()
}

/// The light effects that spread over the image (bloom, streaks, god rays, the glows and lens
/// flares of the light sources): computed from the whole image (for tiled posters from a
/// smaller copy of it) and looked up by the position in the image (0..1).
pub struct Glow {
    bloom: Vec<Level>,
    weights: Vec<f32>,
    streak: Option<Level>,
    rays: Option<Level>,
    /// The light sources ([`crate::flare::image`]).
    lights: Option<Level>,
}

impl Glow {
    /// The glow of the averaged image `lin` (linear, straight alpha, `w` × `h`).
    pub fn new(lin: &[[f32; 4]], w: u32, h: u32, post: &Post) -> Glow {
        let (wu, hu) = (w as usize, h as usize);
        let rad: Vec<[f32; 3]> = premultiplied(lin).iter().map(|c| [c[0], c[1], c[2]]).collect();
        let bloom = if post.bloom > 0.0 { bloom_levels(&rad, wu, hu, post) } else { Vec::new() };
        let weights = level_weights(post.bloom_size, bloom.len());
        let exposure = 2f32.powf(post.exposure as f32);
        Glow {
            streak: streaks(&rad, wu, hu, post),
            rays: (post.rays > 0.0).then(|| bright_level(&rad, wu, hu, exposure, post.rays_threshold as f32)),
            lights: None,
            bloom,
            weights,
        }
    }

    /// A glow that adds nothing.
    pub fn empty() -> Glow {
        Glow { bloom: Vec::new(), weights: Vec::new(), streak: None, rays: None, lights: None }
    }

    /// The glow with the document's light sources added (for a `w` × `h` image; their image
    /// at [`light_size`]).
    pub fn with_lights(mut self, doc: &crate::doc::Document, w: u32, h: u32) -> Glow {
        let (lw, lh) = light_size(w, h);
        self.lights = crate::flare::image(doc, lw, lh).map(|px| Level { w: lw as usize, h: lh as usize, px });
        self
    }

    /// No glow at all (the effects that spread are off; light sources are not counted).
    pub fn none(post: &Post) -> bool {
        post.bloom <= 0.0 && post.streak <= 0.0 && post.rays <= 0.0
    }

    /// The glow at image position (`u`, `v`), 0..1.
    fn at(&self, u: f32, v: f32, post: &Post) -> [f32; 3] {
        let strength = post.bloom as f32;
        let tint = post.streak_tint.map(|t| t * post.streak as f32);
        let mut glow = [0.0f32; 3];
        for (l, wt) in self.bloom.iter().zip(&self.weights) {
            let s = sample(l, u, v);
            for k in 0..3 {
                glow[k] += s[k] * wt * strength;
            }
        }
        if let Some(l) = &self.streak {
            let s = sample(l, u, v);
            for k in 0..3 {
                glow[k] += s[k] * tint[k];
            }
        }
        if let Some(l) = &self.rays {
            let g = god_rays(l, u, v, post);
            for k in 0..3 {
                glow[k] += g[k];
            }
        }
        if let Some(l) = &self.lights {
            let g = sample(l, u, v);
            for k in 0..3 {
                glow[k] += g[k];
            }
        }
        glow
    }
}

/// A rectangle of an image in pixels: x, y, width, height.
pub type Rect = [u32; 4];

/// The display image: per pixel sRGB-encoded red, green, blue (0..1) and alpha, from the
/// averaged image `lin` (linear, straight alpha).
pub fn apply(lin: &[[f32; 4]], w: u32, h: u32, post: &Post) -> Vec<[f32; 4]> {
    let glow = Glow::new(lin, w, h, post);
    apply_region(lin, [0, 0, w, h], [0, 0, w, h], w, h, &glow, post)
}

/// Margin in pixels around `tile` of a `w` × `h` image that [`apply_region`] needs: one pixel
/// for sharpening, and how far lens distortion and chromatic aberration reach.
pub fn margin(tile: Rect, w: u32, h: u32, post: &Post) -> u32 {
    let (dist, ca) = (post.distortion as f32, (post.chromatic as f32).abs());
    if dist == 0.0 && ca == 0.0 {
        return 2;
    }
    let (cx, cy) = (0.5 * (w as f32 - 1.0), 0.5 * (h as f32 - 1.0));
    let mut m = 0.0f32;
    let (x0, y0, x1, y1) = (tile[0] as f32, tile[1] as f32, (tile[0] + tile[2]) as f32, (tile[1] + tile[3]) as f32);
    for (x, y) in [(x0, y0), (x1, y0), (x0, y1), (x1, y1), ((x0 + x1) * 0.5, y0), ((x0 + x1) * 0.5, y1), (x0, (y0 + y1) * 0.5), (x1, (y0 + y1) * 0.5)]
    {
        let (dx, dy) = (x - cx, y - cy);
        let r2 = (dx * dx + dy * dy) / (cx * cx + cy * cy).max(1e-6);
        let base = 1.0 + dist * r2;
        let s = ((base * (1.0 + ca) - 1.0).abs()).max((base * (1.0 - ca) - 1.0).abs());
        m = m.max(dx.hypot(dy) * s);
    }
    m.ceil() as u32 + 3
}

/// The display pixels of `tile` (a part of a `w` × `h` image), from `lin`: the averaged image
/// (linear, straight alpha) in `region`, which holds the tile with its [`margin`] (clamped to
/// the image). `glow` comes from the whole image. The same result as [`apply`] on the whole
/// image, cut to the tile.
pub fn apply_region(lin: &[[f32; 4]], region: Rect, tile: Rect, w: u32, h: u32, glow: &Glow, post: &Post) -> Vec<[f32; 4]> {
    let [rx, ry, rw, rh] = region.map(|v| v as usize);
    let pre = premultiplied(lin);
    let sharpen = post.sharpen as f32;
    // Sharpened light plus glow over the region.
    let mut mid = vec![[0.0f32; 4]; rw * rh];
    mid.par_chunks_mut(rw.max(1)).enumerate().for_each(|(y, row)| {
        let v = ((ry + y) as f32 + 0.5) / h as f32;
        for (x, o) in row.iter_mut().enumerate() {
            let c = pre[y * rw + x];
            let mut rgb = [c[0], c[1], c[2]];
            if sharpen != 0.0 {
                let mut m = [0.0f32; 3];
                for dy in -1i64..=1 {
                    for dx in -1i64..=1 {
                        let q = pre[(y as i64 + dy).clamp(0, rh as i64 - 1) as usize * rw + (x as i64 + dx).clamp(0, rw as i64 - 1) as usize];
                        for k in 0..3 {
                            m[k] += q[k] / 9.0;
                        }
                    }
                }
                for k in 0..3 {
                    rgb[k] = (rgb[k] + sharpen * (rgb[k] - m[k])).max(0.0);
                }
            }
            let u = ((rx + x) as f32 + 0.5) / w as f32;
            let glow = glow.at(u, v, post);
            let a = c[3] + (1.0 - c[3]) * glow[0].max(glow[1]).max(glow[2]).clamp(0.0, 1.0);
            *o = [rgb[0] + glow[0], rgb[1] + glow[1], rgb[2] + glow[2], a];
        }
    });
    // Lens distortion and chromatic aberration (in image coordinates), then the grading.
    let (cx, cy) = (0.5 * (w as f32 - 1.0), 0.5 * (h as f32 - 1.0));
    let (dist, ca) = (post.distortion as f32, post.chromatic as f32);
    let bilinear = |fx: f32, fy: f32| -> [f32; 4] {
        let fx = fx.clamp(0.0, w as f32 - 1.0).clamp(rx as f32, (rx + rw - 1) as f32) - rx as f32;
        let fy = fy.clamp(0.0, h as f32 - 1.0).clamp(ry as f32, (ry + rh - 1) as f32) - ry as f32;
        let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(rw - 1), (y0 + 1).min(rh - 1));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let p = |x: usize, y: usize| mid[y * rw + x];
        let (a, b, c, d) = (p(x0, y0), p(x1, y0), p(x0, y1), p(x1, y1));
        std::array::from_fn(|k| {
            let top = a[k] + (b[k] - a[k]) * tx;
            let bottom = c[k] + (d[k] - c[k]) * tx;
            top + (bottom - top) * ty
        })
    };
    let [tx0, ty0, tw, th] = tile.map(|v| v as usize);
    let mut out = vec![[0.0f32; 4]; tw * th];
    out.par_chunks_mut(tw.max(1)).enumerate().for_each(|(j, row)| {
        let y = ty0 + j;
        for (i, o) in row.iter_mut().enumerate() {
            let x = tx0 + i;
            let c = if dist == 0.0 && ca == 0.0 {
                mid[(y - ry) * rw + (x - rx)]
            } else {
                let (dx, dy) = (x as f32 - cx, y as f32 - cy);
                let r2 = (dx * dx + dy * dy) / (cx * cx + cy * cy).max(1e-6);
                let base = 1.0 + dist * r2;
                let at = |s: f32| bilinear(cx + dx * s, cy + dy * s);
                let g = at(base);
                if post.spectral && ca != 0.0 {
                    // Seven wavelengths from blue (inward) to red (outward).
                    let (mut acc, mut sum) = ([0.0f32; 3], [0.0f32; 3]);
                    for k in 0..SPECTRAL_SAMPLES {
                        let t = -1.0 + 2.0 * k as f32 / (SPECTRAL_SAMPLES - 1) as f32;
                        let s = at(base * (1.0 + ca * t));
                        let wt = spectral_weights(t);
                        for c in 0..3 {
                            acc[c] += s[c] * wt[c];
                            sum[c] += wt[c];
                        }
                    }
                    [acc[0] / sum[0], acc[1] / sum[1], acc[2] / sum[2], g[3]]
                } else {
                    [at(base * (1.0 + ca))[0], g[1], at(base * (1.0 - ca))[2], g[3]]
                }
            };
            let a = c[3];
            let straight = if a > 1e-6 { [c[0] / a, c[1] / a, c[2] / a] } else { [0.0; 3] };
            let d = grade(straight, x as u32, y as u32, w, h, post);
            *o = [d[0], d[1], d[2], a.clamp(0.0, 1.0)];
        }
    });
    out
}

/// The finished 8-bit sRGB RGBA image (straight alpha) of the averaged image `lin` (linear,
/// straight alpha) with the effects of `post`.
pub fn finish(lin: &[[f32; 4]], w: u32, h: u32, post: &Post) -> Vec<u8> {
    if post.is_neutral() || w == 0 || h == 0 {
        return super::cpu::rgba8(lin);
    }
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    apply(lin, w, h, post).iter().flat_map(|c| [q(c[0]), q(c[1]), q(c[2]), q(c[3])]).collect()
}

/// The denoiser's helper images of a render: albedo (surface colors without lighting) and
/// normals (linear, straight alpha, like the image).
pub type Aux = (Vec<[f32; 4]>, Vec<[f32; 4]>);

/// The documents whose renders are the denoiser's helper images, when `doc` has lit layers or
/// 3D objects (else None): the albedo (lights replaced by ambient light 1, no highlights,
/// reflections, shadows, occlusion or layer effects) and the normals (lit layers and objects
/// show their surface normals, other layers are hidden, the background is the flat normal).
/// Both without effects.
pub fn aux_documents(doc: &crate::doc::Document) -> Option<[crate::doc::Document; 2]> {
    use crate::doc::{Layer, LayerEffects, LayerKind};
    fn lit(l: &Layer) -> bool {
        l.kind == LayerKind::Object || (l.kind == LayerKind::Fractal && l.light.enabled)
    }
    fn any(list: &[Layer]) -> bool {
        list.iter().any(|l| l.visible && (lit(l) || any(&l.children)))
    }
    fn albedo(list: &mut [Layer]) {
        for l in list {
            if lit(l) {
                let li = &mut l.light;
                li.ambient = 1.0;
                li.lights.clear();
                li.specular = 0.0;
                li.reflection = 0.0;
                li.shadows = false;
                li.occlusion = 0.0;
            }
            l.effects = LayerEffects::default();
            albedo(&mut l.children);
        }
    }
    fn normals(list: &mut [Layer]) {
        for l in list {
            if lit(l) {
                l.light.normal_output = true;
                l.light.shadows = false;
                l.light.occlusion = 0.0;
            } else if l.kind != LayerKind::Group {
                l.visible = false;
            }
            l.effects = LayerEffects::default();
            normals(&mut l.children);
        }
    }
    if !any(&doc.layers) {
        return None;
    }
    let mut a = doc.clone();
    a.post = Post::default();
    albedo(&mut a.layers);
    let mut n = doc.clone();
    n.post = Post::default();
    n.image.background = [0.5, 0.5, 1.0, 1.0];
    normals(&mut n.layers);
    Some([a, n])
}

/// Denoises the averaged image `lin` (linear, straight alpha) in place with OIDN (its light,
/// premultiplied), helped by the albedo and normal images when given, mixed with the original
/// by `blend`. Returns the device used.
pub fn denoise(lin: &mut [[f32; 4]], w: u32, h: u32, blend: f64, aux: Option<&Aux>) -> Result<super::oidn::DeviceType, String> {
    let ok = |v: f32| if v.is_finite() { v.max(0.0) } else { 0.0 };
    let orig: Vec<f32> = lin.iter().flat_map(|c| [ok(c[0]) * c[3], ok(c[1]) * c[3], ok(c[2]) * c[3]]).collect();
    let mut color = orig.clone();
    let albedo: Option<Vec<f32>> = aux.map(|(a, _)| a.iter().flat_map(|c| [c[0], c[1], c[2]].map(|v| (ok(v) * c[3]).clamp(0.0, 1.0))).collect());
    // Normals from the colors (n / 2 + ½), unit length; transparent parts face the viewer.
    let normal: Option<Vec<f32>> = aux.map(|(_, n)| {
        n.iter()
            .flat_map(|c| {
                let v = [c[0] * 2.0 - 1.0, c[1] * 2.0 - 1.0, c[2] * 2.0 - 1.0];
                let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
                if c[3] > 1e-3 && l > 1e-3 && l.is_finite() { [v[0] / l, v[1] / l, v[2] / l] } else { [0.0, 0.0, 1.0] }
            })
            .collect()
    });
    let used = super::oidn::denoise(
        &mut color,
        w as usize,
        h as usize,
        true,
        false,
        super::oidn::DeviceType::Default,
        albedo.as_deref(),
        normal.as_deref(),
    )?;
    let k = blend.clamp(0.0, 1.0) as f32;
    for (i, c) in lin.iter_mut().enumerate() {
        let a = c[3];
        if a > 1e-6 {
            for ch in 0..3 {
                let v = orig[i * 3 + ch] + (color[i * 3 + ch] - orig[i * 3 + ch]) * k;
                c[ch] = v.max(0.0) / a;
            }
        }
    }
    Ok(used)
}

/// The finished image for saving: denoised first when the effects ask for it (with the
/// helper images of [`aux_documents`] when given), then the effects of `doc.post` and the
/// document's light sources. The note says how the denoising went (None without it).
pub fn finish_export(mut lin: Vec<[f32; 4]>, w: u32, h: u32, doc: &crate::doc::Document, aux: Option<&Aux>) -> (Vec<u8>, Option<String>) {
    let post = &doc.post;
    let note = post.denoise.then(|| match denoise(&mut lin, w, h, post.denoise_blend, aux) {
        Ok(dev) => format!("denoised ({}{})", dev.label(), if aux.is_some() { ", with albedo and normals" } else { "" }),
        Err(e) => format!("not denoised: {e}"),
    });
    if !crate::flare::any(doc) {
        return (finish(&lin, w, h, post), note);
    }
    let glow = Glow::new(&lin, w, h, post).with_lights(doc, w, h);
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    let out = apply_region(&lin, [0, 0, w, h], [0, 0, w, h], w, h, &glow, post);
    (out.iter().flat_map(|c| [q(c[0]), q(c[1]), q(c[2]), q(c[3])]).collect(), note)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(w: u32, h: u32, f: impl Fn(u32, u32) -> [f32; 4]) -> Vec<[f32; 4]> {
        (0..h).flat_map(|y| (0..w).map(move |x| (x, y))).map(|(x, y)| f(x, y)).collect()
    }

    #[test]
    fn neutral_settings_change_nothing() {
        let img = image(16, 8, |x, y| [x as f32 / 16.0, y as f32 / 8.0, 0.5, 1.0]);
        let raw = super::super::cpu::rgba8(&img);
        assert_eq!(finish(&img, 16, 8, &Post::default()), raw);
        // The full path with neutral numbers gives the same up to rounding.
        let full: Vec<u8> = apply(&img, 16, 8, &Post::default()).iter().flat_map(|c| c.map(|v| (v * 255.0 + 0.5) as u8)).collect();
        assert!(full.iter().zip(&raw).all(|(a, b)| a.abs_diff(*b) <= 1));
    }

    #[test]
    fn bloom_spreads_bright_light() {
        let (w, h) = (64u32, 48u32);
        let img = image(w, h, |x, y| if (x, y) == (32, 24) { [50.0, 50.0, 50.0, 1.0] } else { [0.0, 0.0, 0.0, 1.0] });
        let post = Post { bloom: 1.0, ..Post::default() };
        let out = apply(&img, w, h, &post);
        let at = |x: u32, y: u32| out[(y * w + x) as usize][0];
        assert!(at(36, 24) > 0.05 && at(36, 24) > at(44, 24), "{} {}", at(36, 24), at(44, 24));
        assert_eq!(level_sizes(64, 48), vec![(16, 12), (8, 6), (4, 3), (2, 2), (1, 1)]);
    }

    #[test]
    fn tone_maps_keep_order_and_range() {
        for t in ToneMap::ALL.into_iter().skip(1) {
            let (a, b, c) = (tone_map(t, [0.1; 3])[0], tone_map(t, [1.0; 3])[0], tone_map(t, [20.0; 3])[0]);
            assert!(a < b && b < c && c <= 1.001 && a >= 0.0, "{t:?}: {a} {b} {c}");
        }
    }

    #[test]
    fn grading_moves_colors() {
        let p = Post { exposure: 1.0, ..Post::default() };
        assert!(grade([0.2; 3], 0, 0, 4, 4, &p)[0] > grade([0.2; 3], 0, 0, 4, 4, &Post::default())[0]);
        let warm = Post { temperature: 1.0, ..Post::default() };
        let g = grade([0.3; 3], 0, 0, 4, 4, &warm);
        assert!(g[0] > g[2]);
        let v = Post { vignette: 1.0, ..Post::default() };
        assert!(grade([0.5; 3], 0, 0, 64, 64, &v)[0] < grade([0.5; 3], 32, 32, 64, 64, &v)[0]);
    }

    #[test]
    fn denoising_reduces_noise_when_available() {
        if let Err(e) = super::super::oidn::library() {
            eprintln!("OIDN not available, skipped ({e})");
            return;
        }
        let (w, h) = (96u32, 64u32);
        let clean = image(w, h, |x, y| [x as f32 / w as f32, y as f32 / h as f32, 0.5, 1.0]);
        let noisy: Vec<[f32; 4]> = clean
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let n = |k: u32| (hash(i as u32 * 3 + k) >> 8) as f32 / 16_777_216.0 - 0.5;
                [c[0] + n(0) * 0.4, c[1] + n(1) * 0.4, c[2] + n(2) * 0.4, 1.0]
            })
            .collect();
        let rms = |a: &[[f32; 4]]| {
            (a.iter().zip(&clean).map(|(p, q)| (0..3).map(|k| (p[k] - q[k]).powi(2)).sum::<f32>()).sum::<f32>() / a.len() as f32).sqrt()
        };
        let mut den = noisy.clone();
        denoise(&mut den, w, h, 1.0, None).unwrap();
        assert!(rms(&den) < rms(&noisy) * 0.5, "{} -> {}", rms(&noisy), rms(&den));
        // Half the amount lands between.
        let mut half = noisy.clone();
        denoise(&mut half, w, h, 0.5, None).unwrap();
        assert!(rms(&half) < rms(&noisy) && rms(&half) > rms(&den));
    }

    #[test]
    fn helper_documents_for_lit_layers_only() {
        let mut doc = crate::doc::Document::default();
        assert!(aux_documents(&doc).is_none(), "no lit layer, no helper images");
        doc.layers[0].light.enabled = true;
        doc.layers[0].light.shadows = true;
        doc.post.bloom = 1.0;
        let [a, n] = aux_documents(&doc).unwrap();
        assert!(a.layers[0].light.lights.is_empty() && a.layers[0].light.ambient == 1.0 && a.post.is_neutral());
        assert!(n.layers[0].light.normal_output && !n.layers[0].light.shadows);
        // The normal image of a lit Mandelbrot: unit normals facing up everywhere (encoded
        // n / 2 + ½); the set itself is flat.
        let r = crate::render::cpu::render(&n, 48, 32, 1).unwrap().linear();
        for c in &r {
            let v = [c[0] * 2.0 - 1.0, c[1] * 2.0 - 1.0, c[2] * 2.0 - 1.0];
            let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
            assert!((l - 1.0).abs() < 0.02 && v[2] > 0.0, "{c:?}");
        }
        assert!(r.iter().any(|c| c[2] > 0.999), "the flat set faces straight up");
    }

    #[test]
    fn tiles_give_the_whole_image() {
        let (w, h) = (97u32, 61u32);
        let img = image(w, h, |x, y| {
            let v = ((x * 7 + y * 13) % 17) as f32 / 4.0;
            [v, (x as f32 / 30.0).sin().abs() * 3.0, (y % 5) as f32, if x % 11 == 0 { 0.5 } else { 1.0 }]
        });
        let post = Post { bloom: 0.8, sharpen: 0.6, distortion: 0.15, chromatic: 0.03, spectral: true, vignette: 0.4, grain: 0.2, ..Post::default() };
        let whole = apply(&img, w, h, &post);
        let glow = Glow::new(&img, w, h, &post);
        let mut stitched = vec![[0.0f32; 4]; (w * h) as usize];
        for ty in (0..h).step_by(20) {
            for tx in (0..w).step_by(32) {
                let tile = [tx, ty, 32.min(w - tx), 20.min(h - ty)];
                let m = margin(tile, w, h, &post);
                let (x0, y0) = (tx.saturating_sub(m), ty.saturating_sub(m));
                let (x1, y1) = ((tile[0] + tile[2] + m).min(w), (tile[1] + tile[3] + m).min(h));
                let region = [x0, y0, x1 - x0, y1 - y0];
                let part: Vec<[f32; 4]> = (y0..y1).flat_map(|y| (x0..x1).map(move |x| (x, y))).map(|(x, y)| img[(y * w + x) as usize]).collect();
                let out = apply_region(&part, region, tile, w, h, &glow, &post);
                for j in 0..tile[3] {
                    for i in 0..tile[2] {
                        stitched[((ty + j) * w + tx + i) as usize] = out[(j * tile[2] + i) as usize];
                    }
                }
            }
        }
        assert_eq!(stitched, whole);
    }
}
