// SPDX-License-Identifier: LGPL-3.0-or-later

//! What a render needs, independent of CPU or GPU: the [`Plan`] (formula, parameter values,
//! view, colorings), the sample positions and the coloring math. The GPU shader (rfr-gpu)
//! implements the same functions; keep both in step.

pub mod cpu;
pub mod oidn;

use std::sync::Arc;

use crate::doc::{Coloring, Document, InsideMethod, OutsideMethod, Transfer};
use crate::formula::ir::Program;
use crate::formula::library::library;
use crate::formula::{ParamKind, ParamValue};
use crate::math::complex::C64;

/// Coloring of one region, resolved for rendering.
#[derive(Clone, Debug)]
pub struct ColorParams {
    pub lut: Vec<[f32; 4]>,
    pub transfer: Transfer,
    pub density: f64,
    pub offset: f64,
    /// Linear RGB.
    pub solid: [f32; 3],
}

impl ColorParams {
    fn new(c: &Coloring) -> ColorParams {
        ColorParams { lut: c.gradient.lut(), transfer: c.transfer, density: c.density, offset: c.offset, solid: c.solid }
    }
    /// Gradient color for a coloring value.
    pub fn color(&self, v: f64) -> [f32; 3] {
        let c = crate::gradient::lut_sample(&self.lut, self.transfer.apply(v) * self.density + self.offset);
        [c[0], c[1], c[2]]
    }
}

/// Pixel → plane mapping.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewFrame {
    pub width: u32,
    pub height: u32,
    pub center: [f64; 2],
    pub radius: f64,
    pub cos: f64,
    pub sin: f64,
}

impl ViewFrame {
    /// Plane coordinate of image position (x, y) (pixels, y down, fractional).
    pub fn plane(&self, x: f64, y: f64) -> C64 {
        let s = self.radius / (self.height as f64 * 0.5);
        let dx = (x - self.width as f64 * 0.5) * s;
        let dy = -(y - self.height as f64 * 0.5) * s;
        C64::new(self.center[0] + dx * self.cos - dy * self.sin, self.center[1] + dx * self.sin + dy * self.cos)
    }
    /// Size of one pixel in plane units.
    pub fn pixel_size(&self) -> f64 {
        2.0 * self.radius / self.height as f64
    }
}

#[derive(Clone, Debug)]
pub struct Plan {
    pub formula_id: String,
    pub program: Arc<Program>,
    /// Values in the program's parameter order.
    pub values: Vec<ParamValue>,
    pub max_iter: u32,
    pub julia: bool,
    pub seed: [f64; 2],
    pub outside_method: OutsideMethod,
    pub inside_method: InsideMethod,
    pub outside: ColorParams,
    pub inside: ColorParams,
    pub frame: ViewFrame,
}

impl Plan {
    /// The render plan of the document's active layer at the document's image size.
    pub fn new(doc: &Document) -> Result<Plan, String> {
        Plan::sized(doc, doc.image.width, doc.image.height)
    }

    pub fn sized(doc: &Document, width: u32, height: u32) -> Result<Plan, String> {
        let layer = doc.active_layer().ok_or("the document has no visible layer")?;
        let entry = library().get(&layer.formula).ok_or_else(|| format!("unknown formula '{}'", layer.formula))?;
        let program = entry.program.clone().map_err(|e| format!("formula '{}' does not compile: {e}", entry.title))?;
        let values = program.params.iter().map(|p| resolve_value(p.kind, &p.default, layer.params.get(&p.name))).collect();
        let rot = doc.view.rotation.to_radians();
        Ok(Plan {
            formula_id: layer.formula.clone(),
            values,
            max_iter: layer.max_iter.max(1),
            julia: layer.julia && program.has_julia,
            seed: layer.seed,
            outside_method: layer.outside_method,
            inside_method: layer.inside_method,
            outside: ColorParams::new(&layer.outside),
            inside: ColorParams::new(&layer.inside),
            frame: ViewFrame {
                width: width.max(1),
                height: height.max(1),
                center: doc.view.center_f64(),
                radius: doc.view.radius,
                cos: rot.cos(),
                sin: rot.sin(),
            },
            program,
        })
    }

    /// The coloring needs the orbit's derivative (distance estimation).
    pub fn needs_derivative(&self) -> bool {
        self.outside_method == OutsideMethod::Distance
    }

    /// Arithmetic the view needs: pixels must stay well above the rounding step of the
    /// coordinates (their magnitude, at least 1).
    pub fn precision(&self) -> Precision {
        let f = &self.frame;
        let scale = f.center[0].abs().max(f.center[1].abs()).max(1.0);
        let ratio = f.pixel_size() / scale;
        if ratio > SINGLE_LIMIT {
            Precision::Single
        } else if ratio > DOUBLE_LIMIT {
            Precision::Double
        } else {
            Precision::Beyond
        }
    }

    /// Magnification for `#magn` (1.5 / radius).
    pub fn magn(&self) -> f64 {
        1.5 / self.frame.radius
    }
}

/// Pixel size relative to the coordinates below which f32 (24-bit mantissa, about 6e-8 per
/// unit) shows rounding steps: about 16 rounding steps per pixel (measured: identical to the
/// f64 renderer at 16, slight staircases at 1.5).
pub const SINGLE_LIMIT: f64 = 1e-6;
/// The same for double-float (about 48 bits; the pixel offset is added exactly, so about half
/// a rounding step per pixel still renders cleanly).
pub const DOUBLE_LIMIT: f64 = 2e-15;

/// Precision a view needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Precision {
    /// f32 is enough.
    Single,
    /// Double precision (GPU double-float, CPU f64).
    Double,
    /// Deeper than double precision reaches (perturbation, a later stage): rendered with
    /// double precision, which shows rounding steps.
    Beyond,
}

/// A stored value of the right kind for a parameter, else the default.
pub fn resolve_value(kind: ParamKind, default: &ParamValue, v: Option<&ParamValue>) -> ParamValue {
    let Some(v) = v else { return default.clone() };
    match (kind, v) {
        (ParamKind::Func, ParamValue::Func(f)) if crate::formula::ir::Func::by_name(f).is_some() => v.clone(),
        (ParamKind::Func, _) => default.clone(),
        (ParamKind::Bool, ParamValue::Bool(_))
        | (ParamKind::Int, ParamValue::Int(_))
        | (ParamKind::Float, ParamValue::Float(_))
        | (ParamKind::Complex, ParamValue::Complex(_)) => v.clone(),
        (ParamKind::Bool, other) => ParamValue::Bool(other.as_f64() != 0.0),
        (ParamKind::Int, other) => ParamValue::Int(other.as_f64().round() as i64),
        (ParamKind::Float, other) => ParamValue::Float(other.as_f64()),
        (ParamKind::Complex, other) => ParamValue::Complex(other.as_complex()),
    }
}

// ---- sample positions (identical on the GPU: u32 arithmetic) ----

/// PCG hash.
pub fn hash(v: u32) -> u32 {
    let state = v.wrapping_mul(747_796_405).wrapping_add(2_891_336_453);
    let word = ((state >> ((state >> 28) + 4)) ^ state).wrapping_mul(277_803_737);
    (word >> 22) ^ word
}

/// R2 sequence steps (plastic number) and the golden ratio, as 32-bit fixed point.
const R2_A: u32 = 0xC13F_A9A9;
const R2_B: u32 = 0x91E1_0DA6;
const GOLDEN: u32 = 0x9E37_79B9;

/// Position of sample `s` inside pixel (x, y), both in 0..1, and the direction angle (0..1 of
/// a turn) of the derivative for distance estimation.
pub fn sample_offset(x: u32, y: u32, s: u32) -> (f64, f64, f64) {
    let a = hash(x ^ hash(y.wrapping_add(0x68E3_1DA4)));
    let b = hash(a);
    let c = hash(b);
    let u = |base: u32, step: u32| base.wrapping_add(s.wrapping_mul(step)) as f64 / 4_294_967_296.0;
    (u(a, R2_A), u(b, R2_B), u(c, GOLDEN))
}

// ---- coloring values ----

/// The orbit data a coloring needs.
#[derive(Clone, Copy, Debug)]
pub struct OrbitResult {
    pub z: C64,
    pub zprev: C64,
    /// `z` two iterations before the end.
    pub zprev2: C64,
    pub iter: u32,
    pub escaped: bool,
    /// |dz/dpixel| along the sample's direction (0 without derivatives).
    pub dz: f64,
}

/// Coloring value of an escaped (or converged) orbit.
pub fn outside_value(method: OutsideMethod, o: &OrbitResult, pixel_size: f64) -> f64 {
    let n = o.iter as f64;
    match method {
        OutsideMethod::Iterations | OutsideMethod::Solid => n,
        OutsideMethod::Smooth => smooth_iter(o),
        OutsideMethod::Distance => {
            let r2 = o.z.norm();
            if r2 > 4.0 && o.dz > 0.0 && o.dz.is_finite() {
                let r = r2.sqrt();
                let de = r * r.ln() / o.dz;
                (de / pixel_size).max(0.0).ln_1p()
            } else {
                smooth_iter(o)
            }
        }
    }
}

/// Continuous iteration count: divergent orbits by how far beyond the bailout they ended (with
/// the growth rate estimated from the last two radii), convergent ones by their last step.
pub fn smooth_iter(o: &OrbitResult) -> f64 {
    let n = o.iter as f64;
    let r2 = o.z.norm();
    let p2 = o.zprev.norm();
    let v = if r2 > 4.0 && r2 > p2 {
        let lz = 0.5 * r2.ln();
        let lp = 0.5 * p2.ln();
        let p = if lp > 1e-3 { (lz / lp).clamp(1.01, 64.0) } else { 2.0 };
        // The estimate is off by the constant's share; polynomial formulas grow by integer
        // powers, and an exact power keeps neighboring bands seamless.
        let p = if (p - p.round()).abs() < 0.1 { p.round() } else { p };
        n - lz.max(1e-300).ln() / p.ln()
    } else if o.iter >= 2 {
        // Convergent: the step before the last one. The last step is about its square and
        // often below the floating-point resolution; this one is above the threshold.
        let d2 = (o.zprev - o.zprev2).norm().max(1e-30);
        let l = (-0.5 * d2.ln()).max(1e-30);
        n - 1.0 - l.log2()
    } else {
        let d2 = (o.z - o.zprev).norm().max(1e-30);
        let l = (-0.5 * d2.ln()).max(1e-30);
        n - l.log2()
    };
    if v.is_finite() { (v + 2.0).max(0.0) } else { n }
}

/// Coloring value of an orbit that stayed bounded.
pub fn inside_value(method: InsideMethod, o: &OrbitResult) -> f64 {
    match method {
        InsideMethod::Solid => 0.0,
        InsideMethod::Magnitude => o.z.cabs(),
        InsideMethod::Angle => o.z.arg() / std::f64::consts::TAU + 0.5,
    }
}

/// Final linear RGB color of one sample.
pub fn shade(plan: &Plan, o: &OrbitResult) -> [f32; 3] {
    let fin = |c: [f32; 3]| if c.iter().all(|v| v.is_finite()) { c } else { [0.0; 3] };
    if o.escaped {
        if plan.outside_method == OutsideMethod::Solid {
            return plan.outside.solid;
        }
        fin(plan.outside.color(outside_value(plan.outside_method, o, plan.frame.pixel_size())))
    } else {
        if plan.inside_method == InsideMethod::Solid {
            return plan.inside.solid;
        }
        fin(plan.inside.color(inside_value(plan.inside_method, o)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smooth_iteration_is_continuous_across_bands() {
        // z -> z² + c: where the escape count changes, the smooth value must not jump.
        let run = |c: C64| {
            let mut z = C64::new(0.0, 0.0);
            let mut zp = z;
            let mut n = 0;
            while n < 1000 && z.norm() <= 1e8 {
                zp = z;
                z = z.sqr() + c;
                n += 1;
            }
            (n, smooth_iter(&OrbitResult { z, zprev: zp, zprev2: zp, iter: n, escaped: true, dz: 0.0 }))
        };
        let mut edges = 0;
        for k in 0..400 {
            let (mut a, mut b) = (0.3 + k as f64 * 1e-4, 0.3 + (k + 1) as f64 * 1e-4);
            let (na, nb) = (run(C64::new(a, 0.6)).0, run(C64::new(b, 0.6)).0);
            if na == nb || na.abs_diff(nb) > 1 {
                continue;
            }
            for _ in 0..60 {
                let m = 0.5 * (a + b);
                if run(C64::new(m, 0.6)).0 == na { a = m } else { b = m }
            }
            let (va, vb) = (run(C64::new(a, 0.6)).1, run(C64::new(b, 0.6)).1);
            assert!((va - vb).abs() < 1e-3, "seam at {a}: {va} vs {vb}");
            edges += 1;
        }
        assert!(edges > 5, "only {edges} band edges tested");
    }

    #[test]
    fn sample_offsets_stay_in_the_pixel() {
        for s in 0..64 {
            let (u, v, w) = sample_offset(17, 4, s);
            assert!((0.0..1.0).contains(&u) && (0.0..1.0).contains(&v) && (0.0..1.0).contains(&w));
        }
        assert_ne!(sample_offset(1, 2, 0), sample_offset(2, 1, 0));
    }
}
