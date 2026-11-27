// SPDX-License-Identifier: LGPL-3.0-or-later

//! Lit layers: the layer's values become a height field, lit from above.
//!
//! Every pass first evaluates a lit layer over the whole image at one sub-pixel offset (the
//! same for all pixels, so neighbors form a regular grid) into a field of colors and heights.
//! The layer's color at a pixel is then its field color, shaded with the normal from the
//! neighboring heights: ambient light, diffuse light and highlights (Blinn-Phong or GGX) from
//! up to four directional lights. The view looks straight down. The GPU (`lighting.wgsl`)
//! follows the same steps in f32.
//!
//! Shadows march from the point toward each light over the heights (bilinear) and keep the
//! smallest clearance per distance (soft shadows); ambient occlusion looks for the highest
//! horizon in eight directions. Both jitter their steps per pixel and pass, so the passes
//! average the steps out. Lengths are shares of the image height.

use std::sync::Arc;

use super::OrbitResult;
use crate::doc::{HeightSource, Lighting, SpecularModel};
use crate::images::ImageData;

/// A layer's lighting, resolved for rendering.
#[derive(Clone, Debug)]
pub struct LightPlan {
    pub source: HeightSource,
    pub relief: f32,
    pub ambient: f32,
    pub specular: f32,
    pub roughness: f32,
    pub model: SpecularModel,
    /// Per light: the direction toward it (x right, y up, z toward the viewer) and its color
    /// times its intensity.
    pub lights: Vec<([f32; 3], [f32; 3])>,
    /// Shadow length as a share of the image height (0: no shadows) and the light's apparent
    /// size (0: hard shadows).
    pub shadow: f32,
    pub softness: f32,
    /// Ambient occlusion strength (0: off) and radius (share of the image height).
    pub occlusion: f32,
    pub occlusion_radius: f32,
    /// Reflection strength, the environment image (equirectangular) or the sky colors.
    pub reflection: f32,
    pub environment: Option<Arc<ImageData>>,
    pub horizon: [f32; 3],
    pub zenith: [f32; 3],
}

impl LightPlan {
    pub fn new(l: &Lighting) -> LightPlan {
        let lights = l
            .lights
            .iter()
            .take(crate::doc::MAX_LIGHTS)
            .map(|li| {
                let (az, el) = (li.azimuth.to_radians(), li.elevation.clamp(-90.0, 90.0).to_radians());
                let dir = [(el.cos() * az.cos()) as f32, (el.cos() * az.sin()) as f32, el.sin() as f32];
                let k = li.intensity.max(0.0) as f32;
                (dir, [li.color[0] * k, li.color[1] * k, li.color[2] * k])
            })
            .collect();
        LightPlan {
            source: l.height,
            relief: l.relief as f32,
            ambient: l.ambient.max(0.0) as f32,
            specular: l.specular.max(0.0) as f32,
            roughness: l.roughness.clamp(0.02, 1.0) as f32,
            model: l.model,
            lights,
            shadow: if l.shadows { l.shadow_length.max(0.0) as f32 } else { 0.0 },
            softness: l.softness.max(0.0) as f32,
            occlusion: l.occlusion.max(0.0) as f32,
            occlusion_radius: l.occlusion_radius.max(0.0) as f32,
            reflection: l.reflection.clamp(0.0, 1.0) as f32,
            environment: if l.reflection > 0.0 { crate::images::get(&l.environment) } else { None },
            horizon: l.horizon,
            zenith: l.zenith,
        }
    }

    /// The environment seen in direction `r` (x right, y up, z toward the viewer): the image
    /// as longitude and latitude (zenith at the top), else the sky from the horizon (and
    /// below) to the zenith. Linear RGB.
    pub fn environment(&self, r: [f32; 3]) -> [f32; 3] {
        match &self.environment {
            Some(img) => {
                let u = r[1].atan2(r[0]) / std::f32::consts::TAU + 0.5;
                let v = 0.5 - r[2].clamp(-1.0, 1.0).asin() / std::f32::consts::PI;
                let c = crate::images::color(Some(img), (2.0 * u - 1.0) as f64, (1.0 - 2.0 * v) as f64);
                std::array::from_fn(|k| super::composite::srgb_decode(c[k] as f32))
            }
            None => {
                let t = r[2].clamp(0.0, 1.0);
                std::array::from_fn(|k| self.horizon[k] + (self.zenith[k] - self.horizon[k]) * t)
            }
        }
    }

    /// Height units per pixel step for normals: the relief scales with the image height, so a
    /// larger render of the same view looks the same.
    pub fn slope_scale(&self, image_height: u32) -> f32 {
        self.relief * image_height as f32 * 2.0
    }
}

/// Luminance of a linear color, times its alpha.
pub fn luminance(c: [f32; 4]) -> f32 {
    (0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]) * c[3]
}

/// The height by distance estimation: −ln(1 + distance / pixel size) for escaped orbits with
/// a derivative, 0 (the top) otherwise.
pub fn distance_height(o: &OrbitResult, pixel_size: f64) -> f64 {
    let r2 = o.z.norm();
    if o.escaped && r2 > 4.0 && o.dz > 0.0 && o.dz.is_finite() {
        let r = r2.sqrt();
        -(r * r.ln() / o.dz / pixel_size).max(0.0).ln_1p()
    } else {
        0.0
    }
}

/// The height of a sample: `smooth` is the smooth iteration count (the iteration limit for
/// points that stay), `value` the coloring's gradient position before the offset (0 for
/// solid and direct colors), `distance` the [`distance_height`].
pub fn height(source: HeightSource, smooth: f64, value: f64, distance: f64, color: [f32; 4]) -> f32 {
    let h = match source {
        HeightSource::Smooth => smooth.max(0.0).ln_1p() as f32,
        HeightSource::Value => value as f32,
        HeightSource::Luminance => luminance(color),
        HeightSource::Distance => distance as f32,
    };
    if h.is_finite() { h } else { 0.0 }
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-20);
    [v[0] / l, v[1] / l, v[2] / l]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Shades `color` (linear RGB, straight alpha) at a point whose heights rise by `gx` per pixel
/// to the right and `gy` per pixel upward (`k`: [`LightPlan::slope_scale`]); `light_seen` is
/// the share of each light that reaches the point (shadows), `open` the share of the sky
/// (ambient occlusion).
pub fn shade(l: &LightPlan, color: [f32; 4], gx: f32, gy: f32, k: f32, light_seen: &[f32], open: f32) -> [f32; 4] {
    let n = normalize([-k * gx, -k * gy, 1.0]);
    let nv = n[2].max(0.05);
    let a = l.roughness * l.roughness;
    let mut diffuse = [l.ambient * open; 3];
    let mut spec = [0.0f32; 3];
    for (i, (dir, rad)) in l.lights.iter().enumerate() {
        let seen = light_seen.get(i).copied().unwrap_or(1.0);
        if seen <= 0.0 {
            continue;
        }
        let nl = dot(n, *dir);
        if nl <= 0.0 {
            continue;
        }
        let h = normalize([dir[0], dir[1], dir[2] + 1.0]);
        let nh = dot(n, h).max(0.0);
        let s = match l.model {
            SpecularModel::BlinnPhong => {
                let e = (2.0 / (a * a) - 2.0).max(1.0);
                (e + 8.0) / (8.0 * std::f32::consts::PI) * nh.powf(e) * nl
            }
            SpecularModel::Ggx => {
                let a2 = a * a;
                let t = nh * nh * (a2 - 1.0) + 1.0;
                a2 / (std::f32::consts::PI * t * t) / (4.0 * nv)
            }
        };
        for c in 0..3 {
            diffuse[c] += rad[c] * nl * seen;
            spec[c] += rad[c] * s * seen;
        }
    }
    let mut out = [0.0; 4];
    for c in 0..3 {
        out[c] = color[c] * diffuse[c] + l.specular * spec[c];
    }
    // Reflection of the environment, stronger at grazing angles.
    if l.reflection > 0.0 {
        let r = [2.0 * n[2] * n[0], 2.0 * n[2] * n[1], 2.0 * n[2] * n[2] - 1.0];
        let env = l.environment(r);
        let f = l.reflection + (1.0 - l.reflection) * l.reflection * (1.0 - nv).powi(5);
        for c in 0..3 {
            out[c] = out[c] * (1.0 - f) + env[c] * f;
        }
    }
    out[3] = color[3];
    out
}

/// A lit layer's field of one pass: color (linear RGB, straight alpha) and height per pixel.
pub struct Field {
    pub width: u32,
    pub height: u32,
    pub data: Vec<[f32; 5]>,
}

/// Steps of a shadow ray and of ambient occlusion (directions × steps).
pub const SHADOW_STEPS: u32 = 32;
pub const AO_DIRECTIONS: u32 = 8;
pub const AO_STEPS: u32 = 6;

/// A random number in 0..1 from a hash state (advances it).
fn rnd(state: &mut u32) -> f32 {
    *state = super::hash(*state);
    (*state >> 8) as f32 / 16_777_216.0
}

impl Field {
    fn at(&self, x: i64, y: i64) -> [f32; 5] {
        let (w, h) = (self.width as i64, self.height as i64);
        self.data[(y.clamp(0, h - 1) * w + x.clamp(0, w - 1)) as usize]
    }

    /// The height at a point in pixel units (pixel centers at integers), bilinear, the border
    /// heights continued outside.
    pub fn height_at(&self, x: f32, y: f32) -> f32 {
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let (xi, yi) = (x0 as i64, y0 as i64);
        let h = |dx: i64, dy: i64| self.at(xi + dx, yi + dy)[4];
        let top = h(0, 0) + (h(1, 0) - h(0, 0)) * fx;
        let bottom = h(0, 1) + (h(1, 1) - h(0, 1)) * fx;
        top + (bottom - top) * fy
    }

    /// The shaded color at pixel (x, y) in pass `s`: central differences of the heights
    /// (one-sided at the image border), shadows and ambient occlusion.
    pub fn shade(&self, l: &LightPlan, x: u32, y: u32, s: u32) -> [f32; 4] {
        let (xi, yi) = (x as i64, y as i64);
        let c = self.at(xi, yi);
        let gx = (self.at(xi + 1, yi)[4] - self.at(xi - 1, yi)[4]) * 0.5;
        let gy = (self.at(xi, yi - 1)[4] - self.at(xi, yi + 1)[4]) * 0.5;
        let k = l.slope_scale(self.height);
        let mut rng = super::hash(x ^ super::hash(y ^ super::hash(s ^ 0x3C6E_F372)));
        let (px, py) = (x as f32, y as f32);
        let z0 = k * c[4];
        let mut seen = [1.0f32; crate::doc::MAX_LIGHTS];
        let len = l.shadow * self.height as f32;
        if len > 0.0 {
            for (i, (dir, _)) in l.lights.iter().enumerate() {
                seen[i] = self.shadow(dir, px, py, z0, k, len, l.softness, rnd(&mut rng));
            }
        }
        let radius = l.occlusion_radius * self.height as f32;
        let open =
            if l.occlusion > 0.0 && radius > 0.0 { self.occlusion(px, py, z0, k, radius, l.occlusion, rnd(&mut rng), rnd(&mut rng)) } else { 1.0 };
        shade(l, [c[0], c[1], c[2], c[3]], gx, gy, k, &seen, open)
    }

    /// The share of a light reaching (px, py) at height `z0` (pixel units): along the ray
    /// toward the light, the smallest clearance over the heights relative to the distance,
    /// scaled by the light's apparent size `soft` (hard shadows when 0).
    #[allow(clippy::too_many_arguments)]
    fn shadow(&self, dir: &[f32; 3], px: f32, py: f32, z0: f32, k: f32, len: f32, soft: f32, jitter: f32) -> f32 {
        let hl = (dir[0] * dir[0] + dir[1] * dir[1]).sqrt();
        if hl < 1e-4 || dir[2] <= 0.0 {
            return if dir[2] > 0.0 { 1.0 } else { 0.0 };
        }
        let (sx, sy, rise) = (dir[0] / hl, -dir[1] / hl, dir[2] / hl);
        let mut seen = 1.0f32;
        for i in 0..SHADOW_STEPS {
            let f = (i as f32 + jitter) / SHADOW_STEPS as f32;
            let t = (len * f * f).max(0.5);
            let clear = z0 + rise * t - k * self.height_at(px + sx * t, py + sy * t);
            let v = if soft > 0.0 {
                (clear / (soft * t)).clamp(0.0, 1.0)
            } else if clear < 0.0 {
                0.0
            } else {
                1.0
            };
            seen = seen.min(v);
            if seen <= 0.0 {
                break;
            }
        }
        seen
    }

    /// The open share of the sky at (px, py): one minus `strength` times the mean sine of the
    /// highest horizon in [`AO_DIRECTIONS`] directions (turned by `turn`) within `radius`.
    #[allow(clippy::too_many_arguments)]
    fn occlusion(&self, px: f32, py: f32, z0: f32, k: f32, radius: f32, strength: f32, turn: f32, jitter: f32) -> f32 {
        let mut sum = 0.0;
        for d in 0..AO_DIRECTIONS {
            let a = (d as f32 + turn) / AO_DIRECTIONS as f32 * std::f32::consts::TAU;
            let (sx, sy) = (a.cos(), a.sin());
            let mut best = 0.0f32;
            for j in 0..AO_STEPS {
                let t = (radius * (j as f32 + jitter) / AO_STEPS as f32).max(0.5);
                let rise = (k * self.height_at(px + sx * t, py + sy * t) - z0) / t;
                best = best.max(rise);
            }
            sum += best / (1.0 + best * best).sqrt();
        }
        (1.0 - strength * sum / AO_DIRECTIONS as f32).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::Light;

    fn plan(model: SpecularModel) -> LightPlan {
        LightPlan::new(&Lighting {
            enabled: true,
            model,
            lights: vec![Light { azimuth: 0.0, elevation: 45.0, ..Light::default() }],
            ..Lighting::default()
        })
    }

    #[test]
    fn slopes_toward_the_light_are_brighter() {
        for model in [SpecularModel::BlinnPhong, SpecularModel::Ggx] {
            let l = plan(model);
            let grey = [0.5, 0.5, 0.5, 1.0];
            let flat = shade(&l, grey, 0.0, 0.0, 1.0, &[], 1.0);
            // Heights falling to the right face the light on the right.
            let facing = shade(&l, grey, -0.5, 0.0, 1.0, &[], 1.0);
            let away = shade(&l, grey, 0.5, 0.0, 1.0, &[], 1.0);
            assert!(facing[0] > flat[0] && flat[0] > away[0], "{model:?}: {facing:?} {flat:?} {away:?}");
            assert_eq!(flat[3], 1.0);
            // Without lights only the ambient part remains.
            let dark = LightPlan { lights: Vec::new(), ..l.clone() };
            assert!((shade(&dark, grey, 0.3, 0.1, 1.0, &[], 1.0)[1] - 0.5 * l.ambient).abs() < 1e-6);
        }
    }

    #[test]
    fn field_gradients_follow_the_heights() {
        // Heights rise to the right: the normal leans left, away from a light on the right.
        let (w, h) = (5u32, 4u32);
        let data = (0..w * h).map(|i| [0.5, 0.5, 0.5, 1.0, (i % w) as f32 * 0.01]).collect();
        let f = Field { width: w, height: h, data };
        let l = plan(SpecularModel::Ggx);
        let lit = f.shade(&l, 2, 2, 0);
        let flat = shade(&l, [0.5, 0.5, 0.5, 1.0], 0.0, 0.0, l.slope_scale(h), &[], 1.0);
        assert!(lit[0] < flat[0], "{lit:?} vs {flat:?}");
    }

    #[test]
    fn walls_cast_shadows_and_pits_are_occluded() {
        // Flat ground with a wall at x = 10..12, light from the left (azimuth 180°), low.
        let (w, h) = (40u32, 20u32);
        let data = (0..w * h).map(|i| [0.5, 0.5, 0.5, 1.0, if (10..12).contains(&(i % w)) { 0.05 } else { 0.0 }]).collect();
        let f = Field { width: w, height: h, data };
        let mut lighting = Lighting { enabled: true, shadows: true, softness: 0.0, ..Lighting::default() };
        lighting.lights = vec![Light { azimuth: 180.0, elevation: 20.0, ..Light::default() }];
        let l = LightPlan::new(&lighting);
        let behind = f.shade(&l, 14, 10, 0);
        let before = f.shade(&l, 6, 10, 0);
        assert!(behind[0] < before[0] * 0.6, "shadow behind the wall: {behind:?} vs {before:?}");
        // Without shadows both sides are flat ground and look the same.
        let open = LightPlan { shadow: 0.0, ..l.clone() };
        assert_eq!(f.shade(&open, 14, 10, 0), f.shade(&open, 6, 10, 0));
        // A pit: the ground next to the walls sees less sky than far away.
        let ao = LightPlan { occlusion: 1.0, occlusion_radius: 0.4, lights: Vec::new(), ..l.clone() };
        let near = f.shade(&ao, 13, 10, 0);
        let far = f.shade(&ao, 30, 10, 0);
        assert!(near[0] < far[0], "occlusion near the wall: {near:?} vs {far:?}");
    }
}
