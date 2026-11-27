// SPDX-License-Identifier: LGPL-3.0-or-later

//! 3D object layers: a fractal given by a distance estimate, sphere traced from a camera that
//! orbits the origin, lit like a lit layer ([`super::light`]).
//!
//! Coordinates: x to the right, y away from the default camera, z up (the lights' azimuth and
//! elevation mean the same as on a terrain). The surface is reached when the distance falls
//! below the pixel footprint times the detail; the normal comes from distance differences.
//! Shadows march toward each light and keep the smallest clearance per distance (soft
//! shadows), ambient occlusion samples the distance along the normal. The color is the outside
//! gradient at an orbit trap (the smallest |z| of the orbit). The GPU (`object.wgsl`) follows
//! the same steps in f32.

use super::light::{LightPlan, dot, normalize, rnd, shade_normal};
use crate::doc::{Object3D, ObjectShape};

/// A 3D object layer, resolved for rendering.
#[derive(Clone, Debug)]
pub struct ObjectPlan {
    pub shape: ObjectShape,
    pub power: f32,
    pub scale: f32,
    pub iterations: u32,
    pub julia: bool,
    pub seed: [f32; 4],
    /// Camera direction and pitch (radians), distance, tan(vertical field of view / 2).
    pub camera: [f32; 4],
    pub detail: f32,
    pub light: LightPlan,
}

/// Sphere tracing steps, shadow steps, occlusion samples.
pub const MARCH_STEPS: u32 = 300;
pub const SHADOW_STEPS: u32 = 64;
pub const AO_SAMPLES: u32 = 5;

impl ObjectPlan {
    pub fn new(o: &Object3D, light: &crate::doc::Lighting) -> ObjectPlan {
        let c = &o.camera;
        ObjectPlan {
            shape: o.shape,
            power: o.power.clamp(1.0, 32.0) as f32,
            scale: o.scale as f32,
            iterations: o.iterations.clamp(1, 64),
            julia: o.julia || o.shape == ObjectShape::Quaternion,
            seed: o.seed.map(|v| v as f32),
            camera: [
                c.yaw.to_radians() as f32,
                c.pitch.clamp(-89.0, 89.0).to_radians() as f32,
                c.distance.max(0.01) as f32,
                (c.fov.clamp(1.0, 170.0).to_radians() * 0.5).tan() as f32,
            ],
            detail: o.detail.clamp(0.05, 20.0) as f32,
            light: LightPlan::new(light),
        }
    }

    /// The radius of a sphere around the origin that holds the whole object.
    pub fn bound(&self) -> f32 {
        match self.shape {
            ObjectShape::Mandelbulb | ObjectShape::Quaternion => 2.0,
            // The box the Mandelbox fills has half the size 2 (s + 1) / (s - 1); the sphere
            // holds its corners.
            ObjectShape::Mandelbox => {
                let s = self.scale.abs();
                let half = if s > 1.05 { (2.0 * (s + 1.0) / (s - 1.0)).min(16.0) } else { 16.0 };
                half * 3f32.sqrt()
            }
            ObjectShape::Menger => 1.8,
        }
    }

    /// The distance estimate at `p` and the orbit trap.
    pub fn de(&self, p: [f32; 3]) -> (f32, f32) {
        match self.shape {
            ObjectShape::Mandelbulb => self.bulb(p),
            ObjectShape::Quaternion => self.quaternion(p),
            ObjectShape::Mandelbox => self.mandelbox(p),
            ObjectShape::Menger => self.menger(p),
        }
    }

    fn c3(&self, p: [f32; 3]) -> [f32; 3] {
        if self.julia { [self.seed[0], self.seed[1], self.seed[2]] } else { p }
    }

    fn bulb(&self, p: [f32; 3]) -> (f32, f32) {
        let n = self.power;
        let c = self.c3(p);
        let add = if self.julia { 0.0 } else { 1.0 };
        let mut z = p;
        let mut dr = 1.0f32;
        let mut trap = f32::MAX;
        let mut r = len(z);
        for _ in 0..self.iterations {
            r = len(z);
            if r > 2.0 {
                break;
            }
            trap = trap.min(r);
            let theta = if r > 1e-12 { (z[2] / r).clamp(-1.0, 1.0).acos() * n } else { 0.0 };
            let phi = z[1].atan2(z[0]) * n;
            dr = r.powf(n - 1.0) * n * dr + add;
            let zr = r.powf(n);
            z = [zr * theta.sin() * phi.cos() + c[0], zr * theta.sin() * phi.sin() + c[1], zr * theta.cos() + c[2]];
            r = len(z);
        }
        (0.5 * r.max(1e-12).ln() * r / dr.max(1e-12), trap.min(r))
    }

    fn quaternion(&self, p: [f32; 3]) -> (f32, f32) {
        let c = if self.julia { self.seed } else { [p[0], p[1], p[2], 0.0] };
        let mut q = [p[0], p[1], p[2], 0.0f32];
        let mut dq = [1.0f32, 0.0, 0.0, 0.0];
        let mut trap = f32::MAX;
        for _ in 0..self.iterations {
            let r2 = q.iter().map(|v| v * v).sum::<f32>();
            if r2 > 16.0 {
                break;
            }
            trap = trap.min(r2.sqrt());
            // dq = 2 q dq; q = q² + c
            let m = qmul(q, dq);
            dq = [2.0 * m[0], 2.0 * m[1], 2.0 * m[2], 2.0 * m[3]];
            let s = qmul(q, q);
            q = [s[0] + c[0], s[1] + c[1], s[2] + c[2], s[3] + c[3]];
        }
        let r = q.iter().map(|v| v * v).sum::<f32>().sqrt();
        let dr = dq.iter().map(|v| v * v).sum::<f32>().sqrt();
        (0.5 * r * r.max(1e-12).ln() / dr.max(1e-12), trap.min(r))
    }

    fn mandelbox(&self, p: [f32; 3]) -> (f32, f32) {
        let s = self.scale;
        let c = self.c3(p);
        let add = if self.julia { 0.0 } else { 1.0 };
        let (min_r2, fixed_r2) = (0.25f32, 1.0f32);
        let mut z = p;
        let mut dr = 1.0f32;
        let mut trap = f32::MAX;
        for _ in 0..self.iterations {
            for v in &mut z {
                *v = v.clamp(-1.0, 1.0) * 2.0 - *v;
            }
            let r2 = dot(z, z);
            trap = trap.min(r2.sqrt());
            let k = if r2 < min_r2 {
                fixed_r2 / min_r2
            } else if r2 < fixed_r2 {
                fixed_r2 / r2
            } else {
                1.0
            };
            z = [z[0] * k * s + c[0], z[1] * k * s + c[1], z[2] * k * s + c[2]];
            dr = dr * k * s.abs() + add;
            if dot(z, z) > 1e8 {
                break;
            }
        }
        (len(z) / dr.abs().max(1e-12), trap)
    }

    fn menger(&self, p: [f32; 3]) -> (f32, f32) {
        let mut z = p;
        let mut scale = 1.0f32;
        let mut trap = f32::MAX;
        for _ in 0..self.iterations {
            z = [z[0].abs(), z[1].abs(), z[2].abs()];
            if z[0] < z[1] {
                z.swap(0, 1);
            }
            if z[0] < z[2] {
                z.swap(0, 2);
            }
            if z[1] < z[2] {
                z.swap(1, 2);
            }
            z = [3.0 * z[0] - 2.0, 3.0 * z[1] - 2.0, 3.0 * z[2] - 2.0];
            if z[2] < -1.0 {
                z[2] += 2.0;
            }
            scale *= 3.0;
            trap = trap.min(len(z) / scale);
        }
        // The distance to the unit box, scaled back.
        let q = [z[0].abs() - 1.0, z[1].abs() - 1.0, z[2].abs() - 1.0];
        let outside = len([q[0].max(0.0), q[1].max(0.0), q[2].max(0.0)]);
        let inside = q[0].max(q[1]).max(q[2]).min(0.0);
        ((outside + inside) / scale, trap * 4.0)
    }

    /// The camera ray through image point (x, y) of a `width` × `height` image.
    pub fn ray(&self, width: u32, height: u32, x: f32, y: f32) -> ([f32; 3], [f32; 3]) {
        let (w, h) = (width as f32, height as f32);
        let [yaw, pitch, dist, tan] = self.camera;
        let back = [yaw.sin() * pitch.cos(), -yaw.cos() * pitch.cos(), pitch.sin()];
        let o = [back[0] * dist, back[1] * dist, back[2] * dist];
        let f = [-back[0], -back[1], -back[2]];
        let r = normalize([f[1], -f[0], 0.0]);
        let u = [r[1] * f[2] - r[2] * f[1], r[2] * f[0] - r[0] * f[2], r[0] * f[1] - r[1] * f[0]];
        let sx = (x / w * 2.0 - 1.0) * (w / h) * tan;
        let sy = (1.0 - y / h * 2.0) * tan;
        (o, normalize([f[0] + r[0] * sx + u[0] * sy, f[1] + r[1] * sx + u[1] * sy, f[2] + r[2] * sx + u[2] * sy]))
    }

    /// Where the ray meets the surface: the distance along it and the orbit trap there.
    pub fn trace(&self, o: [f32; 3], d: [f32; 3], footprint: f32) -> Option<(f32, f32)> {
        // The part of the ray inside the bounding sphere.
        let r = self.bound();
        let b = dot(o, d);
        let disc = b * b - (dot(o, o) - r * r);
        if disc < 0.0 {
            return None;
        }
        let sq = disc.sqrt();
        let (t0, t1) = ((-b - sq).max(0.0), -b + sq);
        if t1 <= 0.0 {
            return None;
        }
        let mut t = t0;
        for _ in 0..MARCH_STEPS {
            let p = at(o, d, t);
            let (dist, trap) = self.de(p);
            if dist < footprint * t.max(1e-3) * self.detail {
                return Some((t, trap));
            }
            t += dist * 0.9;
            if t > t1 {
                return None;
            }
        }
        None
    }

    /// The surface normal at `p` (central differences of the distance over `e`).
    pub fn normal(&self, p: [f32; 3], e: f32) -> [f32; 3] {
        let g = |k: usize| {
            let (mut a, mut b) = (p, p);
            a[k] += e;
            b[k] -= e;
            self.de(a).0 - self.de(b).0
        };
        normalize([g(0), g(1), g(2)])
    }

    /// The share of the light from direction `l` reaching `p` (sphere traced toward it; soft
    /// edges by the softness, hard when it is 0).
    fn shadow(&self, p: [f32; 3], l: [f32; 3], eps: f32, jitter: f32) -> f32 {
        let soft = self.light.softness;
        let reach = self.light.shadow * 2.0 * self.bound();
        let mut t = eps * (4.0 + jitter);
        let mut seen = 1.0f32;
        for _ in 0..SHADOW_STEPS {
            if t > reach {
                break;
            }
            let (dist, _) = self.de(at(p, l, t));
            let v = if soft > 0.0 {
                (dist / (soft * t)).clamp(0.0, 1.0)
            } else if dist < eps {
                0.0
            } else {
                1.0
            };
            seen = seen.min(v);
            if seen <= 0.0 {
                return 0.0;
            }
            t += dist.max(eps);
        }
        seen
    }

    /// The open share of the sky at `p`: the distance along the normal compared with the step.
    fn occlusion(&self, p: [f32; 3], n: [f32; 3], jitter: f32) -> f32 {
        let step = self.light.occlusion_radius * self.bound() / AO_SAMPLES as f32;
        let mut occ = 0.0;
        let mut w = 0.5;
        for i in 1..=AO_SAMPLES {
            let h = step * (i as f32 - 0.5 + jitter);
            let (dist, _) = self.de(at(p, n, h));
            occ += w * ((h - dist) / h).max(0.0);
            w *= 0.5;
        }
        (1.0 - self.light.occlusion * occ * 2.0).clamp(0.0, 1.0)
    }

    /// The layer's color (linear RGB, straight alpha) at image point (x, y) (pixels with the
    /// sample offset) of a `width` × `height` image; `gradient` colors the orbit trap; `rng`
    /// seeds the shadow and occlusion jitter. Transparent where the ray misses.
    pub fn shade(&self, width: u32, height: u32, x: f32, y: f32, gradient: impl Fn(f64) -> [f32; 4], mut rng: u32) -> [f32; 4] {
        let (o, d) = self.ray(width, height, x, y);
        let footprint = 2.0 * self.camera[3] / height as f32;
        let Some((t, trap)) = self.trace(o, d, footprint) else { return [0.0; 4] };
        let p = at(o, d, t);
        let eps = footprint * t.max(1e-3) * self.detail;
        let n = self.normal(p, eps * 0.5);
        // Start the secondary rays just off the surface.
        let q = at(p, n, eps * 2.0);
        let l = &self.light;
        let mut seen = [1.0f32; crate::doc::MAX_LIGHTS];
        if l.shadow > 0.0 {
            for (i, (dir, _)) in l.lights.iter().enumerate() {
                seen[i] = if dot(n, *dir) > 0.0 { self.shadow(q, *dir, eps, rnd(&mut rng)) } else { 0.0 };
            }
        }
        let open = if l.occlusion > 0.0 && l.occlusion_radius > 0.0 { self.occlusion(q, n, rnd(&mut rng)) } else { 1.0 };
        let color = gradient(trap as f64 * 4.0);
        shade_normal(l, color, n, &seen, open, [-d[0], -d[1], -d[2]], &[])
    }
}

fn len(v: [f32; 3]) -> f32 {
    dot(v, v).sqrt()
}

fn at(o: [f32; 3], d: [f32; 3], t: f32) -> [f32; 3] {
    [o[0] + d[0] * t, o[1] + d[1] * t, o[2] + d[2] * t]
}

fn qmul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [
        a[0] * b[0] - a[1] * b[1] - a[2] * b[2] - a[3] * b[3],
        a[0] * b[1] + a[1] * b[0] + a[2] * b[3] - a[3] * b[2],
        a[0] * b[2] - a[1] * b[3] + a[2] * b[0] + a[3] * b[1],
        a[0] * b[3] + a[1] * b[2] - a[2] * b[1] + a[3] * b[0],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::Lighting;

    fn plan(shape: ObjectShape) -> ObjectPlan {
        ObjectPlan::new(&Object3D { shape, ..Object3D::default() }, &Lighting::default())
    }

    /// The distance estimates are lower bounds near the surface and grow far away.
    #[test]
    fn distances_are_plausible() {
        for shape in ObjectShape::ALL {
            let p = plan(shape);
            let r = p.bound() + 4.0;
            let far = p.de([0.0, -r, 0.0]).0;
            assert!(far > 0.5 && far < r + 1.0, "{shape:?}: {far} at distance {r}");
            // March in from the side: the surface is found inside the bounding sphere.
            // (The sponge has tunnels everywhere; along its edges it is solid at every level.)
            let (x, z) = if shape == ObjectShape::Menger { (0.99999, 0.99999) } else { (0.0, 0.1) };
            let hit = p.trace([x, -r, z], [0.0, 1.0, 0.0], 1e-4);
            let (t, _) = hit.unwrap_or_else(|| panic!("{shape:?}: no surface along the y axis"));
            assert!(t > r - p.bound() && t < r, "{shape:?}: surface at {t}");
        }
    }

    #[test]
    fn camera_rays_look_at_the_origin() {
        let p = plan(ObjectShape::Mandelbulb);
        let (o, d) = p.ray(200, 100, 100.0, 50.0);
        // The middle ray passes the origin.
        let t = -dot(o, d);
        let closest = len(at(o, d, t));
        assert!(closest < 1e-4, "{closest}");
        // Pixels to the right look to the right of the camera.
        let (_, dr) = p.ray(200, 100, 190.0, 50.0);
        let right = normalize([d[1], -d[0], 0.0]);
        assert!(dot(dr, right) > 0.0);
    }
}
