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
    c.map(|v| (((srgb(v) - 0.5) * post.contrast as f32 + 0.5) * vig + n).clamp(0.0, 1.0))
}

/// The display image: per pixel sRGB-encoded red, green, blue (0..1) and alpha, from the
/// averaged image `lin` (linear, straight alpha).
pub fn apply(lin: &[[f32; 4]], w: u32, h: u32, post: &Post) -> Vec<[f32; 4]> {
    let (wu, hu) = (w as usize, h as usize);
    let ok = |v: f32| if v.is_finite() { v } else { 0.0 };
    // Premultiplied light.
    let pre: Vec<[f32; 4]> = lin
        .iter()
        .map(|c| {
            let a = ok(c[3]).clamp(0.0, 1.0);
            [ok(c[0]).max(0.0) * a, ok(c[1]).max(0.0) * a, ok(c[2]).max(0.0) * a, a]
        })
        .collect();
    let rad: Vec<[f32; 3]> = pre.iter().map(|c| [c[0], c[1], c[2]]).collect();
    let bloom = if post.bloom > 0.0 { bloom_levels(&rad, wu, hu, post) } else { Vec::new() };
    let weights = level_weights(post.bloom_size, bloom.len());
    let streak = streaks(&rad, wu, hu, post);
    let strength = post.bloom as f32;
    let tint = post.streak_tint.map(|t| t * post.streak as f32);
    let sharpen = post.sharpen as f32;
    // Sharpened light plus glow.
    let mut mid = vec![[0.0f32; 4]; wu * hu];
    mid.par_chunks_mut(wu.max(1)).enumerate().for_each(|(y, row)| {
        let v = (y as f32 + 0.5) / h as f32;
        for (x, o) in row.iter_mut().enumerate() {
            let c = pre[y * wu + x];
            let mut rgb = [c[0], c[1], c[2]];
            if sharpen != 0.0 {
                let mut m = [0.0f32; 3];
                for dy in -1i64..=1 {
                    for dx in -1i64..=1 {
                        let q = pre[(y as i64 + dy).clamp(0, hu as i64 - 1) as usize * wu + (x as i64 + dx).clamp(0, wu as i64 - 1) as usize];
                        for k in 0..3 {
                            m[k] += q[k] / 9.0;
                        }
                    }
                }
                for k in 0..3 {
                    rgb[k] = (rgb[k] + sharpen * (rgb[k] - m[k])).max(0.0);
                }
            }
            let u = (x as f32 + 0.5) / w as f32;
            let mut glow = [0.0f32; 3];
            for (l, wt) in bloom.iter().zip(&weights) {
                let s = sample(l, u, v);
                for k in 0..3 {
                    glow[k] += s[k] * wt * strength;
                }
            }
            if let Some(l) = &streak {
                let s = sample(l, u, v);
                for k in 0..3 {
                    glow[k] += s[k] * tint[k];
                }
            }
            let a = c[3] + (1.0 - c[3]) * glow[0].max(glow[1]).max(glow[2]).clamp(0.0, 1.0);
            *o = [rgb[0] + glow[0], rgb[1] + glow[1], rgb[2] + glow[2], a];
        }
    });
    // Lens distortion and chromatic aberration, then the grading.
    let (cx, cy) = (0.5 * (w as f32 - 1.0), 0.5 * (h as f32 - 1.0));
    let (dist, ca) = (post.distortion as f32, post.chromatic as f32);
    let bilinear = |fx: f32, fy: f32| -> [f32; 4] {
        let fx = fx.clamp(0.0, w as f32 - 1.0);
        let fy = fy.clamp(0.0, h as f32 - 1.0);
        let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(wu - 1), (y0 + 1).min(hu - 1));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let p = |x: usize, y: usize| mid[y * wu + x];
        let (a, b, c, d) = (p(x0, y0), p(x1, y0), p(x0, y1), p(x1, y1));
        std::array::from_fn(|k| {
            let top = a[k] + (b[k] - a[k]) * tx;
            let bottom = c[k] + (d[k] - c[k]) * tx;
            top + (bottom - top) * ty
        })
    };
    let mut out = vec![[0.0f32; 4]; wu * hu];
    out.par_chunks_mut(wu.max(1)).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let c = if dist == 0.0 && ca == 0.0 {
                mid[y * wu + x]
            } else {
                let (dx, dy) = (x as f32 - cx, y as f32 - cy);
                let r2 = (dx * dx + dy * dy) / (cx * cx + cy * cy).max(1e-6);
                let base = 1.0 + dist * r2;
                let at = |s: f32| bilinear(cx + dx * s, cy + dy * s);
                let g = at(base);
                [at(base * (1.0 + ca))[0], g[1], at(base * (1.0 - ca))[2], g[3]]
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
}
