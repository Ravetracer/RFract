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
    /// Lit by the document's light sources (see [`LightPlan::set_points`]).
    pub scene_lights: bool,
    /// Point lights (the document's light sources): position in pixels (x right, y down, z
    /// up) and color times intensity; per light the distance in pixels at which it has fallen
    /// to half (0: no falloff).
    pub points: Vec<([f32; 3], [f32; 3])>,
    pub reach: Vec<f32>,
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
    /// 3D terrain: the camera's direction and pitch (radians), distance (image heights) and
    /// tan(vertical field of view / 2); the height of the point it orbits above the middle of
    /// the ground (image heights).
    pub camera: Option<[f32; 4]>,
    pub camera_target: f32,
    /// 3D terrain: the height of the highest part (share of the image height) and the height
    /// value that reaches it (see [`LightPlan::set_height_range`]).
    pub terrain_height: f32,
    pub height_range: f32,
    /// 3D terrain: walls down to a base (depth below the lowest ground as a share of the
    /// terrain height, the color at the base).
    pub walls: Option<(f32, [f32; 3])>,
    /// 3D terrain: the fields of the ground (1: the view; each more one ring twice as wide).
    pub rings: u32,
    /// 3D terrain path tracing: the bounces (0: off; see [`Fields::path`]).
    pub path: u32,
    /// Whether the layer is lit (else a layer with effects keeps its colors).
    pub shading: bool,
    /// Output the surface normal as a color (n / 2 + ½) instead of shading.
    pub normal_output: bool,
    /// The layer's effects (2D layers only).
    pub fx: Option<FxPlan>,
    /// The fields' sizes (see [`Grids`]): [`FAR_SIDE`] and [`NEAR_MARGIN`] (smaller in tests).
    pub far_side: u32,
    pub near_margin: u32,
}

/// A layer's effects, resolved for rendering (sizes as shares of the image height).
#[derive(Clone, Debug)]
pub struct FxPlan {
    pub shadow: f32,
    /// Shadow offset (x right, y down), blur.
    pub shadow_offset: [f32; 2],
    pub shadow_size: f32,
    pub shadow_color: [f32; 3],
    pub glow: f32,
    pub glow_size: f32,
    pub glow_color: [f32; 3],
    pub inner: f32,
    pub inner_size: f32,
    pub inner_color: [f32; 3],
    pub neon: f32,
    /// Neon width in pixels at 1000 pixels image height.
    pub neon_width: f32,
    pub neon_color: [f32; 3],
}

impl FxPlan {
    pub fn new(e: &crate::doc::LayerEffects) -> FxPlan {
        let a = e.shadow_angle.to_radians();
        let d = e.shadow_distance as f32;
        FxPlan {
            shadow: e.shadow.clamp(0.0, 1.0) as f32,
            shadow_offset: [d * a.cos() as f32, -d * a.sin() as f32],
            shadow_size: e.shadow_size.max(0.0) as f32,
            shadow_color: e.shadow_color,
            glow: e.glow.max(0.0) as f32,
            glow_size: e.glow_size.max(0.0) as f32,
            glow_color: e.glow_color,
            inner: e.inner_glow.max(0.0) as f32,
            inner_size: e.inner_size.max(0.0) as f32,
            inner_color: e.inner_color,
            neon: e.neon.max(0.0) as f32,
            neon_width: e.neon_width.max(0.01) as f32,
            neon_color: e.neon_color,
        }
    }
}

/// Samples per blurred alpha of an effect (and pass).
pub const FX_SAMPLES: u32 = 16;

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
            scene_lights: l.scene_lights,
            points: Vec::new(),
            reach: Vec::new(),
            shadow: if l.shadows { l.shadow_length.max(0.0) as f32 } else { 0.0 },
            softness: l.softness.max(0.0) as f32,
            occlusion: l.occlusion.max(0.0) as f32,
            occlusion_radius: l.occlusion_radius.max(0.0) as f32,
            reflection: l.reflection.clamp(0.0, 1.0) as f32,
            environment: if l.reflection > 0.0 { crate::images::get(&l.environment) } else { None },
            horizon: l.horizon,
            zenith: l.zenith,
            terrain_height: l.terrain_height.max(0.0) as f32,
            height_range: 1.0,
            walls: (l.terrain && l.walls).then_some((l.wall_depth.max(0.0) as f32, l.wall_color)),
            rings: l.ground.clamp(1, 16).ilog2() + 1,
            path: if l.terrain && l.path_tracing { l.bounces.clamp(1, 8) } else { 0 },
            shading: true,
            normal_output: l.normal_output,
            fx: None,
            far_side: FAR_SIDE,
            near_margin: NEAR_MARGIN,
            camera_target: l.camera.target as f32,
            camera: l.terrain.then(|| {
                let c = &l.camera;
                [
                    c.yaw.to_radians() as f32,
                    c.pitch.clamp(-89.0, 89.0).to_radians() as f32,
                    c.distance.max(0.01) as f32,
                    (c.fov.clamp(1.0, 170.0).to_radians() * 0.5).tan() as f32,
                ]
            }),
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

    /// Pixels per height unit (z = k · height) for normals, shadows and terrains: the relief
    /// scales with the image height, so a larger render of the same view looks the same. A 3D
    /// terrain uses its own height instead: the height range reaches the terrain height.
    pub fn slope_scale(&self, image_height: u32) -> f32 {
        match self.camera {
            Some(_) => self.terrain_height * image_height as f32 / self.height_range,
            None => self.relief * image_height as f32 * 2.0,
        }
    }

    /// The point lights of the document's light sources for an image of `doc`'s view at
    /// `w` × `h` (at most [`crate::doc::MAX_LIGHTS`], the enabled ones).
    pub fn set_points(&mut self, doc: &crate::doc::Document, w: u32, h: u32) {
        let lights: Vec<_> = if self.scene_lights {
            doc.lights
                .iter()
                .filter(|s| s.enabled && s.intensity > 0.0)
                .filter_map(|s| {
                    let (x, y) = crate::flare::pixel_of(doc, &s.position, w, h)?;
                    let k = s.intensity as f32;
                    // Field cells: pixel centers (x + ½); a terrain's cells span `step` pixels.
                    let c = if self.camera.is_some() { self.terrain_step(w, h) as f64 } else { 1.0 };
                    let pos = [((x - 0.5) / c) as f32, ((y - 0.5) / c) as f32, (s.height * h as f64 / c) as f32];
                    Some(((pos, s.color.map(|c| c * k)), (s.reach.max(0.0) * h as f64 / c) as f32))
                })
                .take(crate::doc::MAX_LIGHTS)
                .collect()
        } else {
            Vec::new()
        };
        (self.points, self.reach) = lights.into_iter().unzip();
    }

    /// [`LightPlan::set_points`] for a 3D object of bounding radius `bound`: the image plane
    /// is the ground through the object's center, one image height the object's diameter;
    /// positions and reaches in world units (x right, y away, z up).
    pub fn set_object_points(&mut self, doc: &crate::doc::Document, w: u32, h: u32, bound: f32) {
        self.set_points(doc, w, h);
        let unit = 2.0 * bound / h as f32;
        for (p, r) in self.points.iter_mut().zip(self.reach.iter_mut()) {
            let (x, y) = (p.0[0] + 0.5, p.0[1] + 0.5);
            p.0 = [(x - w as f32 * 0.5) * unit, (h as f32 * 0.5 - y) * unit, p.0[2] * unit];
            *r *= unit;
        }
    }

    /// A terrain's base for its walls (`k` · height units of a field `field_height` cells
    /// high): below the lowest ground of the height source (0, or minus the height range for
    /// distance heights) by the wall depth.
    pub fn wall_base(&self, field_height: u32) -> Option<f32> {
        let (depth, _) = self.walls?;
        let low = if self.source == HeightSource::Distance { 1.0 } else { 0.0 };
        Some(-(depth + low) * self.terrain_height * field_height as f32)
    }

    /// The size of the height values of the source for a terrain: smooth iterations up to
    /// ln(1 + iteration limit) (the set), distances down to ln(1001) (a thousand pixels away),
    /// coloring values and brightness about 1.
    pub fn set_height_range(&mut self, max_iter: u32) {
        self.height_range = match self.source {
            HeightSource::Smooth => (max_iter.max(1) as f32).ln_1p(),
            HeightSource::Distance => 1000f32.ln_1p(),
            HeightSource::Value | HeightSource::Luminance => 1.0,
        };
    }
}

/// Luminance of a linear color, times its alpha.
pub fn luminance(c: [f32; 4]) -> f32 {
    (0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]) * c[3]
}

/// The distance estimate in pixels for escaped orbits with a derivative, else very far.
pub fn distance_px(o: &OrbitResult) -> f32 {
    let r2 = o.z.norm();
    if o.escaped && r2 > 4.0 && o.dz > 0.0 && o.dz.is_finite() {
        let r = r2.sqrt();
        ((r * r.ln() / o.dz).max(0.0)) as f32
    } else {
        3.0e38
    }
}

/// The height by distance estimation: −ln(1 + distance / pixel size) for escaped orbits with
/// a derivative, 0 (the top) otherwise.
pub fn distance_height(o: &OrbitResult) -> f64 {
    let r2 = o.z.norm();
    if o.escaped && r2 > 4.0 && o.dz > 0.0 && o.dz.is_finite() {
        let r = r2.sqrt();
        -(r * r.ln() / o.dz).max(0.0).ln_1p()
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

pub fn normalize(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-20);
    [v[0] / l, v[1] / l, v[2] / l]
}

pub fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Shades `color` (linear RGB, straight alpha) at a point whose heights rise by `gx` per pixel
/// to the right and `gy` per pixel upward (`k`: [`LightPlan::slope_scale`]); `light_seen` is
/// the share of each light that reaches the point (shadows), `open` the share of the sky
/// (ambient occlusion), `v` the direction toward the viewer ((0, 0, 1) from straight above);
/// `points`: per point light the direction toward it from this point and its seen share.
#[allow(clippy::too_many_arguments)]
pub fn shade(
    l: &LightPlan,
    color: [f32; 4],
    gx: f32,
    gy: f32,
    k: f32,
    light_seen: &[f32],
    open: f32,
    v: [f32; 3],
    points: &[([f32; 3], f32)],
) -> [f32; 4] {
    shade_normal(l, color, normalize([-k * gx, -k * gy, 1.0]), light_seen, open, v, points)
}

/// The direction from the surface point (`px`, `py`, pixels, y down; `z0` up) toward a point
/// light at `pos` (x right, y up, z up), the horizontal distance to it in pixels and the
/// share of the light left at that distance (`reach`: half at that distance, 0: no falloff).
pub fn point_dir(pos: [f32; 3], px: f32, py: f32, z0: f32, reach: f32) -> ([f32; 3], f32, f32) {
    let d = [pos[0] - px, py - pos[1], pos[2] - z0];
    (normalize(d), (d[0] * d[0] + d[1] * d[1]).sqrt(), falloff(dot(d, d), reach))
}

/// The share of a point light left at squared distance `d2` (half at `reach`; 0: no falloff).
pub fn falloff(d2: f32, reach: f32) -> f32 {
    if reach > 0.0 { 1.0 / (1.0 + d2 / (reach * reach)) } else { 1.0 }
}

/// [`shade`] with the surface normal `n` (unit length; x right, y up, z toward the sky).
pub fn shade_normal(l: &LightPlan, color: [f32; 4], n: [f32; 3], light_seen: &[f32], open: f32, v: [f32; 3], points: &[([f32; 3], f32)]) -> [f32; 4] {
    if l.normal_output {
        return [n[0] * 0.5 + 0.5, n[1] * 0.5 + 0.5, n[2] * 0.5 + 0.5, color[3]];
    }
    let nv = dot(n, v).max(0.05);
    let a = l.roughness * l.roughness;
    let mut diffuse = [l.ambient * open; 3];
    let mut spec = [0.0f32; 3];
    let directional = l.lights.iter().enumerate().map(|(i, (dir, rad))| (*dir, *rad, light_seen.get(i).copied().unwrap_or(1.0)));
    let point = points.iter().zip(&l.points).map(|((dir, seen), (_, rad))| (*dir, *rad, *seen));
    for (dir, rad, seen) in directional.chain(point) {
        let dir = &dir;
        if seen <= 0.0 {
            continue;
        }
        let nl = dot(n, *dir);
        if nl <= 0.0 {
            continue;
        }
        let h = normalize([dir[0] + v[0], dir[1] + v[1], dir[2] + v[2]]);
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
        let nd = dot(n, v);
        let r = [2.0 * nd * n[0] - v[0], 2.0 * nd * n[1] - v[1], 2.0 * nd * n[2] - v[2]];
        let env = l.environment(r);
        let f = l.reflection + (1.0 - l.reflection) * l.reflection * (1.0 - nv).powi(5);
        for c in 0..3 {
            out[c] = out[c] * (1.0 - f) + env[c] * f;
        }
    }
    out[3] = color[3];
    out
}

/// Cells of a field over the whole image: cell (i, j) holds the layer at image pixel
/// (`x` + i · `step`, `y` + j · `step`) (pixel units, centers at integers); a grid may reach
/// beyond the image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Grid {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub step: u32,
}

impl Grid {
    pub fn cells(&self) -> usize {
        self.width as usize * self.height as usize
    }

    /// The image pixel of cell (i, j).
    pub fn pixel(&self, i: u32, j: u32) -> (i32, i32) {
        (self.x + (i * self.step) as i32, self.y + (j * self.step) as i32)
    }
}

/// The fields a lit layer renders for a window: the far field covers the whole image and what
/// the shading reaches beyond it, coarser when that is large; then the near field covers the
/// window and [`NEAR_MARGIN`] pixels around it at full resolution. A lookup within
/// [`NEAR_MARGIN`] pixels (per axis) of the shaded pixel reads the near field, any other the far
/// one, so a window shades exactly like the whole image. A 3D terrain has one field, the
/// window's (terrains render whole images).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Grids {
    pub near: Option<Grid>,
    pub far: Grid,
    /// A 3D terrain larger than the view: its rings around the far grid (see [`Rings`]).
    pub rings: Vec<Grid>,
}

/// The rings of a terrain larger than the view: level j (0: the view) has the view's cells,
/// `2^j` times as far apart, around the same middle cell (`cx`, `cy`); its cell i lies at cell
/// `cx + (i − cx) · 2^j` of level 0. Lookups read the finest level holding the point; the
/// outer [`RING_BAND`] of each level blends into the next, so the ground has no seams.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rings {
    pub levels: u32,
    pub width: u32,
    pub height: u32,
}

impl Rings {
    /// The middle cell.
    pub fn center(&self) -> (f32, f32) {
        (((self.width.max(1) - 1) / 2) as f32, ((self.height.max(1) - 1) / 2) as f32)
    }

    /// A level-0 point in level `j`'s cells.
    pub fn to_level(&self, j: u32, x: f32, y: f32) -> (f32, f32) {
        let (cx, cy) = self.center();
        let m = (1u32 << j) as f32;
        (cx + (x - cx) / m, cy + (y - cy) / m)
    }

    /// A point of level `j`'s cells in level 0's.
    pub fn from_level(&self, j: u32, u: f32, v: f32) -> (f32, f32) {
        let (cx, cy) = self.center();
        let m = (1u32 << j) as f32;
        (cx + (u - cx) * m, cy + (v - cy) * m)
    }

    /// The finest level holding a level-0 point (the outermost beyond them all).
    pub fn level_of(&self, x: f32, y: f32) -> u32 {
        let (xm, ym) = ((self.width.max(1) - 1) as f32, (self.height.max(1) - 1) as f32);
        for j in 0..self.levels {
            let (u, v) = self.to_level(j, x, y);
            if (0.0..=xm).contains(&u) && (0.0..=ym).contains(&v) {
                return j;
            }
        }
        self.levels - 1
    }

    /// The blend band of a level in cells: where it fades into the next coarser one.
    pub fn band(&self) -> f32 {
        let (cx, cy) = self.center();
        (cx.min(cy) * 2.0 * RING_BAND).max(2.0)
    }
}

/// Path tracing: how far (cells) a bounce starts above the surface, along its normal.
pub const PATH_EPS: f32 = 0.05;

/// A cosine-weighted direction around the unit normal `n` from two random numbers (0..1).
pub fn cosine_dir(n: [f32; 3], u1: f32, u2: f32) -> [f32; 3] {
    let r = u1.sqrt();
    let a = std::f32::consts::TAU * u2;
    let (x, y, z) = (r * a.cos(), r * a.sin(), (1.0 - u1).max(0.0).sqrt());
    let t = if n[2].abs() < 0.999 { normalize([-n[1], n[0], 0.0]) } else { [1.0, 0.0, 0.0] };
    let b = [n[1] * t[2] - n[2] * t[1], n[2] * t[0] - n[0] * t[2], n[0] * t[1] - n[1] * t[0]];
    normalize([t[0] * x + b[0] * y + n[0] * z, t[1] * x + b[1] * y + n[1] * z, t[2] * x + b[2] * y + n[2] * z])
}

/// The share of a ring level's width (from its edge) that blends into the next level.
pub const RING_BAND: f32 = 0.125;

/// Lookups this close to the shaded pixel (per axis, pixels) read the near field.
pub const NEAR_MARGIN: u32 = 256;
/// The far field's longest side at most (cells); beyond it the far field gets coarser.
pub const FAR_SIDE: u32 = 4096;

impl LightPlan {
    /// How far the shading of a pixel reads the field around it, in pixels of an image of
    /// `w` × `h`: left, top, right, bottom. Shadows only toward the lights, occlusion and
    /// effects all around (blurs to three standard deviations).
    pub fn reach(&self, w: u32, h: u32) -> [f32; 4] {
        let hf = h as f32;
        let mut r = [1.0f32; 4];
        let mut all = |v: f32| r.iter_mut().for_each(|x| *x = x.max(v));
        if self.shading && self.occlusion > 0.0 {
            all(self.occlusion_radius * hf);
        }
        if let Some(fx) = &self.fx {
            if fx.glow > 0.0 {
                all(3.0 * fx.glow_size * hf);
            }
            if fx.inner > 0.0 {
                all(3.0 * fx.inner_size * hf);
            }
        }
        let mut side = |k: usize, v: f32| r[k] = r[k].max(v);
        if let Some(fx) = self.fx.as_ref().filter(|f| f.shadow > 0.0) {
            // The shadow reads the layer at (x, y) − offset.
            let (ox, oy, b) = (fx.shadow_offset[0] * hf, fx.shadow_offset[1] * hf, 3.0 * fx.shadow_size * hf);
            side(0, ox.max(0.0) + b);
            side(2, (-ox).max(0.0) + b);
            side(1, oy.max(0.0) + b);
            side(3, (-oy).max(0.0) + b);
        }
        let len = self.shadow * hf;
        if self.shading && len > 0.0 {
            for (dir, _) in &self.lights {
                let hl = (dir[0] * dir[0] + dir[1] * dir[1]).sqrt();
                if hl < 1e-4 || dir[2] <= 0.0 {
                    continue;
                }
                let (sx, sy) = (dir[0] / hl, -dir[1] / hl);
                side(2, len * sx);
                side(0, -len * sx);
                side(3, len * sy);
                side(1, -len * sy);
            }
            // Toward point lights outside the image (rays end at the light).
            let (xmax, ymax) = ((w as f32 - 1.0).max(0.0), (hf - 1.0).max(0.0));
            for (pos, _) in &self.points {
                side(2, (pos[0] - xmax).clamp(0.0, len));
                side(0, (-pos[0]).clamp(0.0, len));
                side(3, (pos[1] - ymax).clamp(0.0, len));
                side(1, (-pos[1]).clamp(0.0, len));
            }
        }
        r
    }

    /// A 3D terrain's field: image pixels per cell (more than one when the image's longest
    /// side exceeds `far_side`: posters show the ground of a `far_side` field).
    pub fn terrain_step(&self, w: u32, h: u32) -> u32 {
        w.max(h).div_ceil(self.far_side.max(16)).max(1)
    }

    /// The fields of `window` of a `w` × `h` image (see [`Grids`]).
    pub fn grids(&self, w: u32, h: u32, window: super::Window) -> Grids {
        if self.camera.is_some() {
            // A terrain: the ground of the whole view, whatever the window shows.
            let s = self.terrain_step(w, h);
            let (fw, fh) = (w.div_ceil(s), h.div_ceil(s));
            let (cx, cy) = (((fw - 1) / 2) as i32, ((fh - 1) / 2) as i32);
            let rings = (1..self.rings)
                .map(|j| {
                    let m = 1i32 << j;
                    Grid { x: s as i32 * cx * (1 - m), y: s as i32 * cy * (1 - m), width: fw, height: fh, step: s * m as u32 }
                })
                .collect();
            return Grids { near: None, far: Grid { x: 0, y: 0, width: fw, height: fh, step: s }, rings };
        }
        let [l, t, r, b] = self.reach(w, h).map(|v| v.ceil() as u32 + 2);
        let (fw, fh) = (w + l + r, h + t + b);
        let step = fw.max(fh).div_ceil(self.far_side.max(16)).max(1);
        // One cell more than the pixels need: bilinear lookups at the far edge.
        let far = Grid { x: -(l as i32), y: -(t as i32), width: (fw - 1) / step + 2, height: (fh - 1) / step + 2, step };
        // A full-resolution far field holds every lookup.
        let m = self.near_margin as i32 + 2;
        let near = (step > 1).then(|| Grid {
            x: window.x as i32 - m,
            y: window.y as i32 - m,
            width: window.width + 2 * m as u32,
            height: window.height + 2 * m as u32,
            step: 1,
        });
        Grids { near, far, rings: Vec::new() }
    }
}

/// A lit layer's field of one pass over a [`Grid`]: color (linear RGB, straight alpha), height
/// and distance to the set per cell.
pub struct Field {
    pub grid: Grid,
    pub width: u32,
    pub height: u32,
    pub data: Vec<[f32; 6]>,
    /// 3D terrain: the highest surface point (k · height) per cell, level 0 cells spanning
    /// pixel centers (x, y) to (x + 1, y + 1), each level halving the cells (see [`mip_sizes`]).
    pub mips: Vec<Vec<f32>>,
}

/// Limits of the terrain ray: cell visits (per pixel of the field's width and height: a ray
/// skimming the heights walks the finest cells), search steps in a level-0 cell, bisections.
pub const TRACE_ITERATIONS_PER_PIXEL: u32 = 4;
pub const TRACE_STEPS: u32 = 4;
pub const TRACE_REFINE: u32 = 8;

/// The cell counts of height maxima level `level`.
pub fn mip_size(width: u32, height: u32, level: usize) -> (u32, u32) {
    let (w, h) = (width.max(1), height.max(1));
    (w.div_ceil(1 << level), h.div_ceil(1 << level))
}

/// The camera ray through image point (x, y) (pixels) of a terrain field of `width` ×
/// `height`: origin and unit direction in field units (pixels, y down, z up). The camera
/// orbits the field's middle at height `zc`.
pub fn camera_ray(cam: [f32; 4], width: u32, height: u32, zc: f32, x: f32, y: f32) -> ([f32; 3], [f32; 3]) {
    camera_ray_in(cam, (width, height), (width, height), zc, x, y)
}

/// [`camera_ray`] for a field of `field` cells seen in an image of `image` pixels: the ray
/// through image point (x, y), in field cells.
pub fn camera_ray_in(cam: [f32; 4], field: (u32, u32), image: (u32, u32), zc: f32, x: f32, y: f32) -> ([f32; 3], [f32; 3]) {
    let (fw, fh) = (field.0 as f32, field.1 as f32);
    let (w, h) = (image.0 as f32, image.1 as f32);
    let [yaw, pitch, dist, tan] = cam;
    let target = [(fw - 1.0) * 0.5, (fh - 1.0) * 0.5, zc];
    let back = [yaw.sin() * pitch.cos(), yaw.cos() * pitch.cos(), pitch.sin()];
    let o = [target[0] + back[0] * dist * fh, target[1] + back[1] * dist * fh, target[2] + back[2] * dist * fh];
    let f = [-back[0], -back[1], -back[2]];
    // Right on the screen (y down: looking north, right is +x), and up (z rising).
    let r = normalize([-f[1], f[0], 0.0]);
    let u = [f[1] * r[2] - f[2] * r[1], f[2] * r[0] - f[0] * r[2], f[0] * r[1] - f[1] * r[0]];
    let sx = (x / w * 2.0 - 1.0) * (w / h) * tan;
    let sy = (1.0 - y / h * 2.0) * tan;
    (o, normalize([f[0] + r[0] * sx + u[0] * sy, f[1] + r[1] * sx + u[1] * sy, f[2] + r[2] * sx + u[2] * sy]))
}

/// The cell counts (width, height) of the height maxima levels, down to one cell.
pub fn mip_sizes(width: u32, height: u32) -> Vec<(u32, u32)> {
    let mut out = vec![(width.max(1), height.max(1))];
    while let Some(&(w, h)) = out.last() {
        if w == 1 && h == 1 {
            break;
        }
        out.push((w.div_ceil(2), h.div_ceil(2)));
    }
    out
}

/// Steps of a shadow ray and of ambient occlusion (directions × steps).
pub const SHADOW_STEPS: u32 = 32;
pub const AO_DIRECTIONS: u32 = 8;
pub const AO_STEPS: u32 = 6;

/// A random number in 0..1 from a hash state (advances it).
pub fn rnd(state: &mut u32) -> f32 {
    *state = super::hash(*state);
    (*state >> 8) as f32 / 16_777_216.0
}

impl Field {
    /// A field over `grid` of an image `image_height` pixels high; for a 3D terrain with its
    /// height maxima.
    pub fn new(l: &LightPlan, grid: Grid, image_height: u32, data: Vec<[f32; 6]>) -> Field {
        let (width, height) = (grid.width, grid.height);
        let mut f = Field { grid, width, height, data, mips: Vec::new() };
        if l.camera.is_some() {
            f.build_mips(l.slope_scale(image_height));
        }
        f
    }

    /// A field without height maxima (see [`Field::build_mips`]).
    pub fn raw(grid: Grid, data: Vec<[f32; 6]>) -> Field {
        Field { grid, width: grid.width, height: grid.height, data, mips: Vec::new() }
    }

    /// A terrain's height maxima (`k` · height per cell; the rings divide `k` by their scale).
    pub fn build_mips(&mut self, k: f32) {
        let (width, height) = (self.width, self.height);
        let f = &*self;
        {
            let sizes = mip_sizes(width, height);
            let mut levels: Vec<Vec<f32>> = Vec::with_capacity(sizes.len());
            for (lv, &(w, h)) in sizes.iter().enumerate() {
                let mut m = vec![0.0f32; (w * h) as usize];
                for y in 0..h as i64 {
                    for x in 0..w as i64 {
                        m[(y * w as i64 + x) as usize] = if lv == 0 {
                            let z = |dx: i64, dy: i64| k * f.at(x + dx, y + dy)[4];
                            z(0, 0).max(z(1, 0)).max(z(0, 1)).max(z(1, 1))
                        } else {
                            let (pw, ph) = sizes[lv - 1];
                            let prev = &levels[lv - 1];
                            let p = |dx: i64, dy: i64| prev[((2 * y + dy).min(ph as i64 - 1) * pw as i64 + (2 * x + dx).min(pw as i64 - 1)) as usize];
                            p(0, 0).max(p(1, 0)).max(p(0, 1)).max(p(1, 1))
                        };
                    }
                }
                levels.push(m);
            }
            self.mips = levels;
        }
    }

    /// Blends the outer band of this ring level into `outer`, the next coarser one (heights
    /// and colors; see [`Rings`]).
    pub fn blend_into(&mut self, outer: &Field, rings: &Rings) {
        let (w, h) = (self.width, self.height);
        let band = rings.band();
        let (cx, cy) = rings.center();
        for y in 0..h {
            for x in 0..w {
                let d = x.min(w - 1 - x).min(y).min(h - 1 - y) as f32;
                if d >= band {
                    continue;
                }
                let t = 1.0 - d / band;
                let (u, v) = (cx + (x as f32 - cx) * 0.5, cy + (y as f32 - cy) * 0.5);
                let (c, hh) = (outer.color_at(u, v), outer.height_at(u, v));
                let cell = &mut self.data[(y * w + x) as usize];
                for k in 0..4 {
                    cell[k] += (c[k] - cell[k]) * t;
                }
                cell[4] += (hh - cell[4]) * t;
            }
        }
    }

    fn at(&self, x: i64, y: i64) -> [f32; 6] {
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

    /// The color (bilinear) at a point in pixel units.
    pub fn color_at(&self, x: f32, y: f32) -> [f32; 4] {
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let (xi, yi) = (x0 as i64, y0 as i64);
        let (a, b, c, d) = (self.at(xi, yi), self.at(xi + 1, yi), self.at(xi, yi + 1), self.at(xi + 1, yi + 1));
        std::array::from_fn(|k| {
            let top = a[k] + (b[k] - a[k]) * fx;
            let bottom = c[k] + (d[k] - c[k]) * fx;
            top + (bottom - top) * fy
        })
    }

    fn mip(&self, level: usize, x: i64, y: i64) -> f32 {
        let (w, h) = mip_size(self.width, self.height, level);
        let (w, h) = (w as i64, h as i64);
        self.mips[level][(y.clamp(0, h - 1) * w + x.clamp(0, w - 1)) as usize]
    }

    /// Where the ray from `o` in direction `d` (pixel units, y down, z up; z = k · height)
    /// first meets the surface from above, in pixel units; `None` when it misses. Cells whose
    /// highest point stays below the ray are skipped level by level; in a level-0 cell the
    /// bilinear surface is searched in steps and the crossing refined by bisection.
    pub fn trace(&self, o: [f32; 3], d: [f32; 3], k: f32, base: Option<f32>) -> Option<Hit> {
        self.trace_in(o, d, k, base, (0.0, f32::INFINITY), false)
    }

    /// [`Field::trace`] over the part `range` of the ray; `entry_hit`: a ray that comes into
    /// the range below the surface meets it there (a terrain's inner ring, see [`Rings`]).
    pub fn trace_in(&self, o: [f32; 3], d: [f32; 3], k: f32, base: Option<f32>, range: (f32, f32), entry_hit: bool) -> Option<Hit> {
        if self.mips.is_empty() {
            return None;
        }
        let top = self.mips.len() - 1;
        let (xmax, ymax) = ((self.width as f32 - 1.0).max(0.0), (self.height as f32 - 1.0).max(0.0));
        // The part of the ray over the field and below its highest point; the side it enters
        // through (axis, entering toward +).
        let mut t0 = 0.0f32;
        let mut t1 = f32::INFINITY;
        let mut side = None;
        for (axis, (oa, da, hi)) in [(o[0], d[0], xmax), (o[1], d[1], ymax)].into_iter().enumerate() {
            if da.abs() < 1e-12 {
                if oa < 0.0 || oa > hi {
                    return None;
                }
            } else {
                let (a, b) = (-oa / da, (hi - oa) / da);
                if a.min(b) > t0 {
                    t0 = a.min(b);
                    side = Some((axis, da > 0.0));
                }
                t1 = t1.min(a.max(b));
            }
        }
        if range.0 > t0 {
            t0 = range.0;
            side = None;
        }
        t1 = t1.min(range.1);
        let t_side = t0;
        let zt = self.mip(top, 0, 0);
        if o[2] > zt {
            if d[2] >= 0.0 {
                return None;
            }
            t0 = t0.max((zt - o[2]) / d[2]);
        }
        if t0 > t1 {
            return None;
        }
        let at = |t: f32| (o[0] + d[0] * t, o[1] + d[1] * t);
        let above = |t: f32| {
            let (x, y) = at(t);
            o[2] + d[2] * t - k * self.height_at(x, y)
        };
        // A ray that enters the field below the surface comes in through a side (or from
        // underneath): it meets the wall when the terrain has walls and it comes in above the
        // base, else the terrain is an open sheet. (Entering through the highest point, on a
        // plateau, is rounding, not a side.)
        if above(t0) < -1e-3 * (1.0 + zt.abs() + o[2].abs()) {
            if entry_hit {
                return Some(Hit::Surface(at(t0)));
            }
            let z = o[2] + d[2] * t0;
            return match (base, side) {
                (Some(b), Some((axis, up))) if t0 == t_side && z >= b => {
                    let (x, y) = at(t0);
                    // Outward, with y up like the lights (field y runs down).
                    let n = match (axis, up) {
                        (0, true) => [-1.0, 0.0, 0.0],
                        (0, false) => [1.0, 0.0, 0.0],
                        (_, true) => [0.0, 1.0, 0.0],
                        (_, false) => [0.0, -1.0, 0.0],
                    };
                    Some(Hit::Wall { x, y, z, n })
                }
                _ => None,
            };
        }
        let mut t = t0;
        let mut level = top;
        for _ in 0..TRACE_ITERATIONS_PER_PIXEL * (self.width + self.height) + 64 {
            if t > t1 {
                return None;
            }
            let (px, py) = at(t);
            let size = (1u32 << level) as f32;
            let (cx, cy) = ((px / size).floor(), (py / size).floor());
            // Where the ray leaves the cell.
            let mut te = t1;
            for (pa, da, c) in [(o[0], d[0], cx), (o[1], d[1], cy)] {
                if da > 1e-12 {
                    te = te.min(((c + 1.0) * size - pa) / da);
                } else if da < -1e-12 {
                    te = te.min((c * size - pa) / da);
                }
            }
            let te = te.max(t);
            let zmax = self.mip(level, cx as i64, cy as i64);
            if (o[2] + d[2] * t).min(o[2] + d[2] * te) > zmax {
                t = te + 1e-3;
                level = (level + 1).min(top);
                continue;
            }
            if level > 0 {
                level -= 1;
                continue;
            }
            // Level 0: search the bilinear surface between t and te, refine the crossing.
            if above(t) <= 0.0 {
                return Some(Hit::Surface(at(t)));
            }
            let mut a = t;
            for j in 1..=TRACE_STEPS {
                let b = t + (te - t) * j as f32 / TRACE_STEPS as f32;
                if above(b) <= 0.0 {
                    let (mut lo, mut hi) = (a, b);
                    for _ in 0..TRACE_REFINE {
                        let m = 0.5 * (lo + hi);
                        if above(m) <= 0.0 {
                            hi = m;
                        } else {
                            lo = m;
                        }
                    }
                    return Some(Hit::Surface(at(hi)));
                }
                a = b;
            }
            t = te + 1e-3;
            level = 1.min(top);
        }
        // Out of steps: the ray skims the surface here.
        (t <= t1).then(|| Hit::Surface(at(t)))
    }
}

/// Where a camera ray meets a terrain: its surface (field cells), or a wall at height `z`
/// with outward normal `n` (x right, y up).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Hit {
    Surface((f32, f32)),
    Wall { x: f32, y: f32, z: f32, n: [f32; 3] },
}

/// The fields of a lit layer for one pass (see [`Grids`]).
pub struct Fields {
    pub near: Option<Field>,
    pub far: Field,
    /// A terrain larger than the view: its outer ring levels (1, 2, …; see [`Rings`]).
    pub rings: Vec<Field>,
    /// The image's height: lengths are shares of it (a terrain: its field's height, lengths
    /// in cells).
    pub image_height: u32,
    /// The whole image's size (a terrain's camera).
    pub image: (u32, u32),
    /// Lookups are in the far field's cells (a terrain), not in image pixels.
    pub cells: bool,
    /// Lookups this close to the shaded pixel read the near field.
    pub near_margin: u32,
}

impl Fields {
    /// The fields of a terrain and its rings for one pass, from the cells of every level
    /// (level 0 first): the rings' bands blended, the height maxima built.
    pub fn terrain(l: &LightPlan, mut levels: Vec<Field>, image_height: u32, image: (u32, u32)) -> Fields {
        let rings = Rings { levels: levels.len() as u32, width: levels[0].width, height: levels[0].height };
        for j in 1..levels.len() {
            let (inner, outer) = levels.split_at_mut(j);
            inner[j - 1].blend_into(&outer[0], &rings);
        }
        let k = l.slope_scale(image_height);
        for (j, f) in levels.iter_mut().enumerate() {
            f.build_mips(k / (1u32 << j) as f32);
        }
        let mut it = levels.into_iter();
        let far = it.next().expect("level 0");
        Fields { near: None, far, rings: it.collect(), image_height, image, cells: true, near_margin: l.near_margin }
    }

    /// The field a lookup at image point (x, y) reads when shading pixel `p0`, and the point
    /// in its cells.
    fn pick(&self, x: f32, y: f32, p0: [f32; 2]) -> (&Field, f32, f32) {
        // A terrain is shaded in its field's cells (level 0), read in its finest ring.
        if self.cells {
            let r = self.ring_info();
            let j = r.level_of(x, y);
            let (u, v) = r.to_level(j, x, y);
            return (self.level(j), u, v);
        }
        if let Some(n) = &self.near
            && (x - p0[0]).abs().max((y - p0[1]).abs()) <= self.near_margin as f32
        {
            return (n, x - n.grid.x as f32, y - n.grid.y as f32);
        }
        let g = &self.far.grid;
        (&self.far, (x - g.x as f32) / g.step as f32, (y - g.y as f32) / g.step as f32)
    }

    /// A terrain's rings.
    pub fn ring_info(&self) -> Rings {
        Rings { levels: self.rings.len() as u32 + 1, width: self.far.width, height: self.far.height }
    }

    /// A terrain's ring level `j` (0: the view).
    fn level(&self, j: u32) -> &Field {
        if j == 0 { &self.far } else { &self.rings[j as usize - 1] }
    }

    /// Where a camera ray (level-0 cells) meets a terrain and its rings: the ray runs through
    /// the rings in order, each part traced in its own level; walls stand at the outermost.
    pub fn trace_terrain(&self, o: [f32; 3], d: [f32; 3], k: f32, base: Option<f32>) -> Option<Hit> {
        let r = self.ring_info();
        let top = r.levels - 1;
        // The ray's interval over each level's box (level-0 cells).
        let (xm, ym) = ((r.width.max(1) - 1) as f32, (r.height.max(1) - 1) as f32);
        let span = |j: u32| -> Option<(f32, f32)> {
            let (x0, y0) = r.from_level(j, 0.0, 0.0);
            let (x1, y1) = r.from_level(j, xm, ym);
            let (mut a, mut b) = (0.0f32, f32::INFINITY);
            for (oa, da, lo, hi) in [(o[0], d[0], x0, x1), (o[1], d[1], y0, y1)] {
                if da.abs() < 1e-12 {
                    if oa < lo || oa > hi {
                        return None;
                    }
                } else {
                    let (p, q) = ((lo - oa) / da, (hi - oa) / da);
                    a = a.max(p.min(q));
                    b = b.min(p.max(q));
                }
            }
            (a <= b).then_some((a, b))
        };
        let spans: Vec<Option<(f32, f32)>> = (0..r.levels).map(span).collect();
        // Each level's own parts: its interval without the next finer level's.
        let mut parts: Vec<(f32, f32, u32)> = Vec::new();
        for j in 0..r.levels {
            let Some((a, b)) = spans[j as usize] else { continue };
            match (j > 0).then(|| spans[j as usize - 1]).flatten() {
                Some((ia, ib)) => {
                    if ia > a {
                        parts.push((a, ia.min(b), j));
                    }
                    if ib < b {
                        parts.push((ib.max(a), b, j));
                    }
                }
                None => parts.push((a, b, j)),
            }
        }
        parts.sort_by(|p, q| p.0.total_cmp(&q.0));
        let outer_entry = spans[top as usize].map(|s| s.0);
        for (a, b, j) in parts {
            let m = (1u32 << j) as f32;
            let (ox, oy) = r.to_level(j, o[0], o[1]);
            let (oj, dj) = ([ox, oy, o[2] / m], [d[0] / m, d[1] / m, d[2] / m]);
            // Walls at the outermost level, where the ray comes into the ground; elsewhere a ray
            // that crosses into a finer ring below the surface meets it there.
            let first = j == top && Some(a) == outer_entry;
            let hit = self.level(j).trace_in(oj, dj, k / m, base.filter(|_| first).map(|b| b / m), (a, b), !first);
            if let Some(h) = hit {
                return Some(match h {
                    Hit::Surface((u, v)) => Hit::Surface(r.from_level(j, u, v)),
                    Hit::Wall { x, y, z, n } => {
                        let (x, y) = r.from_level(j, x, y);
                        Hit::Wall { x, y, z: z * m, n }
                    }
                });
            }
        }
        None
    }

    /// The height at image point (x, y), bilinear, read for pixel `p0`.
    pub fn height_at(&self, x: f32, y: f32, p0: [f32; 2]) -> f32 {
        let (f, lx, ly) = self.pick(x, y, p0);
        f.height_at(lx, ly)
    }

    /// The color at image point (x, y), bilinear, read for pixel `p0`.
    pub fn color_at(&self, x: f32, y: f32, p0: [f32; 2]) -> [f32; 4] {
        let (f, lx, ly) = self.pick(x, y, p0);
        f.color_at(lx, ly)
    }

    /// The layer's color at image pixel (x, y) in pass `s`: from straight above, or the
    /// terrain seen by the camera (its field is the whole image's).
    pub fn shade(&self, l: &LightPlan, x: u32, y: u32, s: u32) -> [f32; 4] {
        let mut rng = super::hash(x ^ super::hash(y ^ super::hash(s ^ 0x3C6E_F372)));
        let h_img = self.image_height;
        let Some(cam) = l.camera else {
            let (px, py) = (x as i32 as f32, y as i32 as f32);
            let p0 = [px, py];
            let (f, lx, ly) = self.pick(px, py, p0);
            let c = f.at(lx as i64, ly as i64);
            let h = |dx: f32, dy: f32| self.height_at(px + dx, py + dy, p0);
            let gx = (h(1.0, 0.0) - h(-1.0, 0.0)) * 0.5;
            let gy = (h(0.0, -1.0) - h(0.0, 1.0)) * 0.5;
            let color = [c[0], c[1], c[2], c[3]];
            let base = if l.shading { self.shade_point(l, color, px, py, c[4], gx, gy, [0.0, 0.0, 1.0], &mut rng) } else { color };
            return match &l.fx {
                Some(fx) => self.effects(fx, base, px, py, c[5], &mut rng),
                None => base,
            };
        };
        let t = &self.far;
        let p0 = [0.0, 0.0];
        let (u, v, _) = super::sample_offset(x, y, s);
        let (o, d) = camera_ray_in(
            cam,
            (t.width, t.height),
            self.image,
            l.slope_scale(h_img) * t.height_at((t.width as f32 - 1.0) * 0.5, (t.height as f32 - 1.0) * 0.5) + l.camera_target * t.height as f32,
            x as f32 + u as f32,
            y as f32 + v as f32,
        );
        let k = l.slope_scale(h_img);
        // Toward the viewer, with y up like the lights.
        let view = [-d[0], d[1], -d[2]];
        let base = l.wall_base(t.height);
        let (hx, hy) = match self.trace_terrain(o, d, k, base) {
            None => return [0.0; 4],
            Some(Hit::Surface(p)) => p,
            Some(Hit::Wall { x, y, z, n }) => return self.wall(l, x, y, z, n, view, k, base),
        };
        if l.path > 0 {
            return self.path(l, (hx, hy), view, k, base, &mut rng);
        }
        let h = |dx: f32, dy: f32| self.height_at(hx + dx, hy + dy, p0);
        let gx = (h(1.0, 0.0) - h(-1.0, 0.0)) * 0.5;
        let gy = (h(0.0, -1.0) - h(0.0, 1.0)) * 0.5;
        self.shade_point(l, self.color_at(hx, hy, p0), hx, hy, h(0.0, 0.0), gx, gy, view, &mut rng)
    }

    /// Shades the field's surface at (px, py) with height `h0` and slopes `gx`, `gy`: shadows,
    /// ambient occlusion, then [`shade`].
    #[allow(clippy::too_many_arguments)]
    fn shade_point(&self, l: &LightPlan, color: [f32; 4], px: f32, py: f32, h0: f32, gx: f32, gy: f32, v: [f32; 3], rng: &mut u32) -> [f32; 4] {
        let k = l.slope_scale(self.image_height);
        self.light_at(l, color, [px, py, k * h0], normalize([-k * gx, -k * gy, 1.0]), v, rng, true)
    }

    /// The light at surface point `p` (x, y and z = k · height) with normal `n`: shadows,
    /// ambient occlusion (`ao`; else no ambient light at all, path tracing), then
    /// [`shade_normal`].
    #[allow(clippy::too_many_arguments)]
    fn light_at(&self, l: &LightPlan, color: [f32; 4], p: [f32; 3], n: [f32; 3], v: [f32; 3], rng: &mut u32, ao: bool) -> [f32; 4] {
        let hf = self.image_height as f32;
        let k = l.slope_scale(self.image_height);
        let [px, py, z0] = p;
        let p0 = [px, py];
        let mut seen = [1.0f32; crate::doc::MAX_LIGHTS];
        let len = l.shadow * hf;
        if len > 0.0 {
            for (i, (dir, _)) in l.lights.iter().enumerate() {
                seen[i] = self.shadow(dir, p0, z0, k, len, l.softness, rnd(rng));
            }
        }
        // Point lights: the direction from here; shadows reach no farther than the light.
        let mut points = [([0.0f32; 3], 1.0f32); crate::doc::MAX_LIGHTS];
        for (i, (pos, _)) in l.points.iter().enumerate() {
            let (dir, dist, fall) = point_dir(*pos, px, py, z0, l.reach[i]);
            points[i].0 = dir;
            if len > 0.0 {
                points[i].1 = self.shadow(&dir, p0, z0, k, len.min(dist), l.softness, rnd(rng));
            }
            points[i].1 *= fall;
        }
        let radius = l.occlusion_radius * hf;
        let open = if !ao {
            0.0
        } else if l.occlusion > 0.0 && radius > 0.0 {
            self.occlusion(p0, z0, k, radius, l.occlusion, rnd(rng), rnd(rng))
        } else {
            1.0
        };
        shade_normal(l, color, n, &seen, open, v, &points[..l.points.len()])
    }

    /// A terrain wall at (x, y) and height `z` with outward normal `n`, seen from `v`: the
    /// edge's color fading to the wall color toward the base, lit by every light (nothing
    /// stands outside the block), without occlusion.
    #[allow(clippy::too_many_arguments)]
    fn wall(&self, l: &LightPlan, x: f32, y: f32, z: f32, n: [f32; 3], v: [f32; 3], k: f32, base: Option<f32>) -> [f32; 4] {
        let p0 = [0.0, 0.0];
        let (b, wall) = (base.unwrap_or(0.0), l.walls.map_or([0.0; 3], |w| w.1));
        let top = k * self.height_at(x, y, p0);
        let f = ((top - z) / (top - b).max(1e-6)).clamp(0.0, 1.0);
        let edge = self.color_at(x, y, p0);
        let color = [0, 1, 2].map(|c| edge[c] + (wall[c] - edge[c]) * f);
        let mut points = [([0.0f32; 3], 1.0f32); crate::doc::MAX_LIGHTS];
        for (i, (pos, _)) in l.points.iter().enumerate() {
            let (dir, _, fall) = point_dir(*pos, x, y, z, l.reach[i]);
            points[i] = (dir, fall);
        }
        let seen = [1.0f32; crate::doc::MAX_LIGHTS];
        shade_normal(l, [color[0], color[1], color[2], 1.0], n, &seen, 1.0, v, &points[..l.points.len()])
    }

    /// Path tracing a terrain from its surface point `hit` (level-0 cells) seen from `view`
    /// (y up): at every point the direct light (shadows, no ambient light), then a bounce in a
    /// cosine-weighted direction; a ray that leaves the terrain sees the sky (times the
    /// ambient strength), one that meets a wall its light. Every bounce dims the light by the
    /// surface's color.
    #[allow(clippy::too_many_arguments)]
    pub fn path(&self, l: &LightPlan, hit: (f32, f32), view: [f32; 3], k: f32, base: Option<f32>, rng: &mut u32) -> [f32; 4] {
        let p0 = [0.0, 0.0];
        let (mut px, mut py) = hit;
        let mut v = view;
        let mut beta = [1.0f32; 3];
        let mut out = [0.0f32; 3];
        let mut alpha = 1.0;
        for b in 0..=l.path {
            let h = |dx: f32, dy: f32| self.height_at(px + dx, py + dy, p0);
            let gx = (h(1.0, 0.0) - h(-1.0, 0.0)) * 0.5;
            let gy = (h(0.0, -1.0) - h(0.0, 1.0)) * 0.5;
            let n = normalize([-k * gx, -k * gy, 1.0]);
            let color = self.color_at(px, py, p0);
            if b == 0 {
                alpha = color[3];
            }
            let z0 = k * h(0.0, 0.0);
            let c = self.light_at(l, color, [px, py, z0], n, v, rng, false);
            for i in 0..3 {
                out[i] += beta[i] * c[i];
            }
            if b == l.path {
                break;
            }
            for i in 0..3 {
                beta[i] *= color[i];
            }
            // The bounce (y up), and in the field's cells (y down).
            let w = cosine_dir(n, rnd(rng), rnd(rng));
            let (wf, nf) = ([w[0], -w[1], w[2]], [n[0], -n[1], n[2]]);
            let o = [px + nf[0] * PATH_EPS, py + nf[1] * PATH_EPS, z0 + nf[2] * PATH_EPS];
            match self.trace_terrain(o, wf, k, base) {
                None => {
                    let s = l.environment(w);
                    for i in 0..3 {
                        out[i] += beta[i] * l.ambient * s[i];
                    }
                    break;
                }
                Some(Hit::Surface(p)) => {
                    (px, py) = p;
                    v = [-w[0], -w[1], -w[2]];
                }
                Some(Hit::Wall { x, y, z, n }) => {
                    let c = self.wall(l, x, y, z, n, [-w[0], -w[1], -w[2]], k, base);
                    for i in 0..3 {
                        out[i] += beta[i] * c[i];
                    }
                    break;
                }
            }
        }
        [out[0], out[1], out[2], alpha]
    }

    /// The layer's alpha blurred with a Gaussian of `sigma` pixels around (x, y), estimated from
    /// [`FX_SAMPLES`] random samples, read for pixel `p0`.
    fn blurred_alpha(&self, x: f32, y: f32, sigma: f32, p0: [f32; 2], rng: &mut u32) -> f32 {
        if sigma < 0.25 {
            return self.color_at(x, y, p0)[3];
        }
        let mut sum = 0.0;
        for _ in 0..FX_SAMPLES {
            let (u1, u2) = (rnd(rng).max(1e-7), rnd(rng));
            let r = sigma * (-2.0 * u1.ln()).sqrt();
            let a = std::f32::consts::TAU * u2;
            sum += self.color_at(x + r * a.cos(), y + r * a.sin(), p0)[3];
        }
        sum / FX_SAMPLES as f32
    }

    /// The layer's effects on `c` (straight color) at pixel (x, y) with the distance `de` (in
    /// pixels) to the set: inner glow and neon on the layer, then the layer over its outer glow
    /// and shadow.
    fn effects(&self, fx: &FxPlan, c: [f32; 4], x: f32, y: f32, de: f32, rng: &mut u32) -> [f32; 4] {
        let h = self.image_height as f32;
        let p0 = [x, y];
        let mut lc = [c[0], c[1], c[2]];
        let la = c[3];
        if fx.inner > 0.0 {
            let ig = (fx.inner * (1.0 - self.blurred_alpha(x, y, fx.inner_size * h, p0, rng)) * la).clamp(0.0, 1.0);
            for (v, t) in lc.iter_mut().zip(fx.inner_color) {
                *v += (t - *v) * ig;
            }
        }
        if fx.neon > 0.0 {
            let n = fx.neon * (-de / (fx.neon_width * h / 1000.0)).exp();
            for (v, t) in lc.iter_mut().zip(fx.neon_color) {
                *v += t * n;
            }
        }
        let g = if fx.glow > 0.0 { (fx.glow * self.blurred_alpha(x, y, fx.glow_size * h, p0, rng)).clamp(0.0, 1.0) } else { 0.0 };
        let s = if fx.shadow > 0.0 {
            let (ox, oy) = (fx.shadow_offset[0] * h, fx.shadow_offset[1] * h);
            (fx.shadow * self.blurred_alpha(x - ox, y - oy, fx.shadow_size * h, p0, rng)).clamp(0.0, 1.0)
        } else {
            0.0
        };
        // Glow over shadow, then the layer over both (straight colors).
        let ua = g + s * (1.0 - g);
        let a = la + ua * (1.0 - la);
        if a <= 1e-6 {
            return [0.0; 4];
        }
        let mut out = [0.0; 4];
        for k in 0..3 {
            let under = fx.glow_color[k] * g + fx.shadow_color[k] * s * (1.0 - g);
            out[k] = (lc[k] * la + under * (1.0 - la)) / a;
        }
        out[3] = a;
        out
    }

    /// The share of a light reaching (px, py) at height `z0` (pixel units): along the ray
    /// toward the light, the smallest clearance over the heights relative to the distance,
    /// scaled by the light's apparent size `soft` (hard shadows when 0).
    #[allow(clippy::too_many_arguments)]
    fn shadow(&self, dir: &[f32; 3], p0: [f32; 2], z0: f32, k: f32, len: f32, soft: f32, jitter: f32) -> f32 {
        let [px, py] = p0;
        let hl = (dir[0] * dir[0] + dir[1] * dir[1]).sqrt();
        if hl < 1e-4 || dir[2] <= 0.0 {
            return if dir[2] > 0.0 { 1.0 } else { 0.0 };
        }
        let (sx, sy, rise) = (dir[0] / hl, -dir[1] / hl, dir[2] / hl);
        let mut seen = 1.0f32;
        for i in 0..SHADOW_STEPS {
            let f = (i as f32 + jitter) / SHADOW_STEPS as f32;
            let t = (len * f * f).max(0.5);
            let clear = z0 + rise * t - k * self.height_at(px + sx * t, py + sy * t, p0);
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
    #[allow(clippy::too_many_arguments)]
    fn occlusion(&self, p0: [f32; 2], z0: f32, k: f32, radius: f32, strength: f32, turn: f32, jitter: f32) -> f32 {
        let [px, py] = p0;
        let mut sum = 0.0;
        for d in 0..AO_DIRECTIONS {
            let a = (d as f32 + turn) / AO_DIRECTIONS as f32 * std::f32::consts::TAU;
            let (sx, sy) = (a.cos(), a.sin());
            let mut best = 0.0f32;
            for j in 0..AO_STEPS {
                let t = (radius * (j as f32 + jitter) / AO_STEPS as f32).max(0.5);
                let rise = (k * self.height_at(px + sx * t, py + sy * t, p0) - z0) / t;
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

    /// One full-resolution field over a `w` × `h` image.
    fn fields(w: u32, h: u32, data: Vec<[f32; 6]>) -> Fields {
        let grid = Grid { x: 0, y: 0, width: w, height: h, step: 1 };
        Fields {
            near: None,
            far: Field { grid, width: w, height: h, data, mips: Vec::new() },
            rings: Vec::new(),
            image_height: h,
            image: (w, h),
            cells: false,
            near_margin: NEAR_MARGIN,
        }
    }

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
            let flat = shade(&l, grey, 0.0, 0.0, 1.0, &[], 1.0, [0.0, 0.0, 1.0], &[]);
            // Heights falling to the right face the light on the right.
            let facing = shade(&l, grey, -0.5, 0.0, 1.0, &[], 1.0, [0.0, 0.0, 1.0], &[]);
            let away = shade(&l, grey, 0.5, 0.0, 1.0, &[], 1.0, [0.0, 0.0, 1.0], &[]);
            assert!(facing[0] > flat[0] && flat[0] > away[0], "{model:?}: {facing:?} {flat:?} {away:?}");
            assert_eq!(flat[3], 1.0);
            // Without lights only the ambient part remains.
            let dark = LightPlan { lights: Vec::new(), ..l.clone() };
            assert!((shade(&dark, grey, 0.3, 0.1, 1.0, &[], 1.0, [0.0, 0.0, 1.0], &[])[1] - 0.5 * l.ambient).abs() < 1e-6);
        }
    }

    #[test]
    fn rings_nest_around_the_middle() {
        let r = Rings { levels: 3, width: 21, height: 11 };
        assert_eq!(r.center(), (10.0, 5.0));
        // Level 1 cell 0 lies 10 cells left of level 0's edge; level 2 twice as far.
        assert_eq!(r.from_level(1, 0.0, 0.0), (-10.0, -5.0));
        assert_eq!(r.from_level(2, 0.0, 0.0), (-30.0, -15.0));
        assert_eq!(r.to_level(2, -30.0, -15.0), (0.0, 0.0));
        assert_eq!(r.level_of(3.0, 4.0), 0);
        assert_eq!(r.level_of(-5.0, 4.0), 1);
        assert_eq!(r.level_of(-25.0, 4.0), 2);
        assert_eq!(r.level_of(-100.0, 4.0), 2);
        // The grids of a terrain four views wide: the same cells, wider steps.
        let mut lighting = Lighting { enabled: true, terrain: true, ground: 4, ..Lighting::default() };
        lighting.terrain_height = 0.1;
        let l = LightPlan::new(&lighting);
        let g = l.grids(21, 11, crate::render::Window::whole(21, 11));
        assert_eq!(g.rings.len(), 2);
        assert_eq!(g.rings[0], Grid { x: -10, y: -5, width: 21, height: 11, step: 2 });
        assert_eq!(g.rings[1].pixel(0, 0), (-30, -15));
    }

    #[test]
    fn rays_through_the_sides_meet_the_walls() {
        // A flat terrain at height 1 (k = 1), 20 × 10 cells; a ray coming in low through the
        // left side.
        let (w, h) = (20u32, 10u32);
        let data = vec![[0.5, 0.5, 0.5, 1.0, 1.0, 3.0e38]; (w * h) as usize];
        let mut lighting = Lighting { enabled: true, terrain: true, walls: true, wall_depth: 0.5, ..Lighting::default() };
        lighting.terrain_height = 0.1;
        let mut l = LightPlan::new(&lighting);
        l.set_height_range(100);
        let grid = Grid { x: 0, y: 0, width: w, height: h, step: 1 };
        let f = Field::new(&l, grid, h, data);
        // The ground lies at z = k (height 1), the base at −0.5 · 0.1 · 10.
        let k = l.slope_scale(h);
        let (o, d) = ([-5.0, 4.0, 0.5 * k], normalize([1.0, 0.0, 0.0]));
        let base = l.wall_base(h);
        assert_eq!(base, Some(-0.5 * 0.1 * 10.0));
        match f.trace(o, d, k, base) {
            Some(Hit::Wall { x, n, z, .. }) => {
                assert!(x.abs() < 1e-4 && n == [-1.0, 0.0, 0.0] && (z - 0.5 * k).abs() < 1e-4, "{x} {n:?} {z}");
            }
            other => panic!("expected the left wall, got {other:?}"),
        }
        // Without walls the sheet is open; below the base the ray passes under the block.
        assert_eq!(f.trace(o, d, k, None), None);
        assert_eq!(f.trace([-5.0, 4.0, -1.0], d, k, base), None);
        // From above it meets the surface.
        assert!(matches!(f.trace([5.0, 4.0, 5.0], normalize([0.2, 0.0, -1.0]), k, base), Some(Hit::Surface(_))));
    }

    #[test]
    fn field_gradients_follow_the_heights() {
        // Heights rise to the right: the normal leans left, away from a light on the right.
        let (w, h) = (5u32, 4u32);
        let data = (0..w * h).map(|i| [0.5, 0.5, 0.5, 1.0, (i % w) as f32 * 0.01, 3.0e38]).collect();
        let f = fields(w, h, data);
        let l = plan(SpecularModel::Ggx);
        let lit = f.shade(&l, 2, 2, 0);
        let flat = shade(&l, [0.5, 0.5, 0.5, 1.0], 0.0, 0.0, l.slope_scale(h), &[], 1.0, [0.0, 0.0, 1.0], &[]);
        assert!(lit[0] < flat[0], "{lit:?} vs {flat:?}");
    }

    #[test]
    fn walls_cast_shadows_and_pits_are_occluded() {
        // Flat ground with a wall at x = 10..12, light from the left (azimuth 180°), low.
        let (w, h) = (40u32, 20u32);
        let data = (0..w * h).map(|i| [0.5, 0.5, 0.5, 1.0, if (10..12).contains(&(i % w)) { 0.05 } else { 0.0 }, 3.0e38]).collect();
        let f = fields(w, h, data);
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

    #[test]
    fn effects_shadow_glow_and_inner_glow() {
        // An opaque square in the middle of a transparent field.
        let (w, h) = (60u32, 60u32);
        let inside = |i: u32| (20..40).contains(&(i % w)) && (20..40).contains(&(i / w));
        let data = (0..w * h).map(|i| if inside(i) { [0.5, 0.5, 0.5, 1.0, 0.0, 3.0e38] } else { [0.0; 6] }).collect();
        let f = fields(w, h, data);
        let shade = |e: crate::doc::LayerEffects, x: u32, y: u32| {
            let mut l = LightPlan::new(&Lighting::default());
            l.shading = false;
            l.fx = Some(FxPlan::new(&e));
            // Average over passes, like the renderer.
            let n = 64;
            let mut s = [0.0f32; 4];
            for p in 0..n {
                let c = f.shade(&l, x, y, p);
                for k in 0..4 {
                    s[k] += c[k] / n as f32;
                }
            }
            s
        };
        use crate::doc::LayerEffects;
        // Drop shadow toward the lower right (315°): dark and opaque there, nothing to the upper left.
        let sh = LayerEffects { shadow: 1.0, shadow_distance: 0.1, shadow_size: 0.01, ..LayerEffects::default() };
        assert!(shade(sh.clone(), 42, 42)[3] > 0.5, "{:?}", shade(sh.clone(), 42, 42));
        assert!(shade(sh.clone(), 17, 17)[3] < 0.05);
        // Outer glow around the square, fading with the distance.
        let gl = LayerEffects { glow: 1.0, glow_size: 0.03, ..LayerEffects::default() };
        let (near, far) = (shade(gl.clone(), 41, 30)[3], shade(gl.clone(), 50, 30)[3]);
        assert!(near > far && near > 0.2, "{near} {far}");
        // Inner glow: brighter at the square's edge than in its middle.
        let ig = LayerEffects { inner_glow: 1.0, inner_size: 0.03, ..LayerEffects::default() };
        assert!(shade(ig.clone(), 21, 30)[0] > shade(ig, 30, 30)[0] + 0.05);
    }
}
