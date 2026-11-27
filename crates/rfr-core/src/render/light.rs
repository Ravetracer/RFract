// SPDX-License-Identifier: LGPL-3.0-or-later

//! Lit layers: the layer's values become a height field, lit from above.
//!
//! Every pass first evaluates a lit layer over the whole image at one sub-pixel offset (the
//! same for all pixels, so neighbors form a regular grid) into a field of colors and heights.
//! The layer's color at a pixel is then its field color, shaded with the normal from the
//! neighboring heights: ambient light, diffuse light and highlights (Blinn-Phong or GGX) from
//! up to four directional lights. The view looks straight down. The GPU (`lighting.wgsl`)
//! follows the same steps in f32.

use crate::doc::{HeightSource, Lighting, SpecularModel};

/// A layer's lighting, resolved for rendering.
#[derive(Clone, Debug, PartialEq)]
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

/// The height of a sample: `smooth` is the smooth iteration count (the iteration limit for
/// points that stay), `value` the coloring's gradient position before the offset (0 for
/// solid and direct colors).
pub fn height(source: HeightSource, smooth: f64, value: f64, color: [f32; 4]) -> f32 {
    let h = match source {
        HeightSource::Smooth => smooth.max(0.0).ln_1p() as f32,
        HeightSource::Value => value as f32,
        HeightSource::Luminance => luminance(color),
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
/// to the right and `gy` per pixel upward (`k`: [`LightPlan::slope_scale`]).
pub fn shade(l: &LightPlan, color: [f32; 4], gx: f32, gy: f32, k: f32) -> [f32; 4] {
    let n = normalize([-k * gx, -k * gy, 1.0]);
    let nv = n[2].max(0.05);
    let a = l.roughness * l.roughness;
    let mut diffuse = [l.ambient; 3];
    let mut spec = [0.0f32; 3];
    for (dir, rad) in &l.lights {
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
            diffuse[c] += rad[c] * nl;
            spec[c] += rad[c] * s;
        }
    }
    let mut out = [0.0; 4];
    for c in 0..3 {
        out[c] = color[c] * diffuse[c] + l.specular * spec[c];
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

impl Field {
    /// The shaded color at pixel (x, y): central differences of the heights (one-sided at the
    /// image border).
    pub fn shade(&self, l: &LightPlan, x: u32, y: u32) -> [f32; 4] {
        let (w, h) = (self.width as i64, self.height as i64);
        let at = |x: i64, y: i64| self.data[(y.clamp(0, h - 1) * w + x.clamp(0, w - 1)) as usize];
        let (x, y) = (x as i64, y as i64);
        let c = at(x, y);
        let gx = (at(x + 1, y)[4] - at(x - 1, y)[4]) * 0.5;
        let gy = (at(x, y - 1)[4] - at(x, y + 1)[4]) * 0.5;
        shade(l, [c[0], c[1], c[2], c[3]], gx, gy, l.slope_scale(self.height))
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
            let flat = shade(&l, grey, 0.0, 0.0, 1.0);
            // Heights falling to the right face the light on the right.
            let facing = shade(&l, grey, -0.5, 0.0, 1.0);
            let away = shade(&l, grey, 0.5, 0.0, 1.0);
            assert!(facing[0] > flat[0] && flat[0] > away[0], "{model:?}: {facing:?} {flat:?} {away:?}");
            assert_eq!(flat[3], 1.0);
            // Without lights only the ambient part remains.
            let dark = LightPlan { lights: Vec::new(), ..l.clone() };
            assert!((shade(&dark, grey, 0.3, 0.1, 1.0)[1] - 0.5 * l.ambient).abs() < 1e-6);
        }
    }

    #[test]
    fn field_gradients_follow_the_heights() {
        // Heights rise to the right: the normal leans left, away from a light on the right.
        let (w, h) = (5u32, 4u32);
        let data = (0..w * h).map(|i| [0.5, 0.5, 0.5, 1.0, (i % w) as f32 * 0.01]).collect();
        let f = Field { width: w, height: h, data };
        let l = plan(SpecularModel::Ggx);
        let lit = f.shade(&l, 2, 2);
        let flat = shade(&l, [0.5, 0.5, 0.5, 1.0], 0.0, 0.0, l.slope_scale(h));
        assert!(lit[0] < flat[0], "{lit:?} vs {flat:?}");
    }
}
