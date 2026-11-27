// SPDX-License-Identifier: LGPL-3.0-or-later

//! What a render needs, independent of CPU or GPU: the [`Plan`] (a tree of layers and groups
//! with their formulas, transformations, colorings, merge modes and masks), the sample
//! positions, the coloring math and the compositing. The GPU renderer (rfr-gpu) implements the
//! same functions; keep both in step.

pub mod composite;
pub mod cpu;
pub mod density;
pub mod light;
pub mod object;
pub mod oidn;
pub mod post;

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::doc::{Coloring, Document, InsideMethod, Layer, LayerKind, MaskSource, MergeMode, OutsideMethod, Transfer};
use crate::formula::ast::ProgramKind;
use crate::formula::ir::Program;
use crate::formula::library::library;
use crate::formula::{ParamKind, ParamValue};
use crate::math::complex::C64;

/// A library program with its parameter values in the program's order.
#[derive(Clone, Debug)]
pub struct Prog {
    pub id: String,
    pub program: Arc<Program>,
    pub values: Vec<ParamValue>,
}

impl Prog {
    /// Resolves library entry `id` of `kind` with the stored parameter values.
    pub fn resolve(id: &str, params: &BTreeMap<String, ParamValue>, kind: ProgramKind) -> Result<Prog, String> {
        let lib = library();
        let entry = lib.get(id).ok_or_else(|| format!("unknown formula '{id}'"))?;
        if entry.kind != kind {
            return Err(format!("'{}' is not a {}", entry.title, kind_name(kind)));
        }
        let program = entry.program_with(params).map_err(|e| format!("'{}' does not compile: {e}", entry.title))?;
        let values = program.params.iter().map(|p| resolve_value(p.kind, &p.default, params.get(&p.name))).collect();
        Ok(Prog { id: id.to_string(), program, values })
    }
}

fn kind_name(k: ProgramKind) -> &'static str {
    match k {
        ProgramKind::Fractal => "fractal formula",
        ProgramKind::Coloring => "coloring formula",
        ProgramKind::Transform => "transformation",
    }
}

/// How a region gets its gradient position.
#[derive(Clone, Debug)]
pub enum ColorMethod {
    Smooth,
    Iterations,
    Distance,
    Solid,
    Magnitude,
    Angle,
    /// A coloring formula: its `#index` and `#solid`.
    Formula(Prog),
}

/// Coloring of one region, resolved for rendering.
#[derive(Clone, Debug)]
pub struct ColorPlan {
    pub method: ColorMethod,
    pub lut: Vec<[f32; 4]>,
    pub transfer: Transfer,
    pub density: f64,
    pub offset: f64,
    /// Linear RGB and alpha.
    pub solid: [f32; 4],
}

impl ColorPlan {
    fn new(method: ColorMethod, c: &Coloring) -> ColorPlan {
        ColorPlan {
            method,
            lut: c.gradient.lut(),
            transfer: c.transfer,
            density: c.density,
            offset: c.offset,
            solid: [c.solid[0], c.solid[1], c.solid[2], c.solid_alpha.clamp(0.0, 1.0)],
        }
    }

    fn outside(m: OutsideMethod, c: &Coloring) -> Result<ColorPlan, String> {
        let method = match m {
            OutsideMethod::Smooth => ColorMethod::Smooth,
            OutsideMethod::Iterations => ColorMethod::Iterations,
            OutsideMethod::Distance => ColorMethod::Distance,
            OutsideMethod::Solid => ColorMethod::Solid,
            OutsideMethod::Formula => ColorMethod::Formula(Prog::resolve(&c.formula, &c.params, ProgramKind::Coloring)?),
        };
        Ok(ColorPlan::new(method, c))
    }

    fn inside(m: InsideMethod, c: &Coloring) -> Result<ColorPlan, String> {
        let method = match m {
            InsideMethod::Solid => ColorMethod::Solid,
            InsideMethod::Magnitude => ColorMethod::Magnitude,
            InsideMethod::Angle => ColorMethod::Angle,
            InsideMethod::Formula => ColorMethod::Formula(Prog::resolve(&c.formula, &c.params, ProgramKind::Coloring)?),
        };
        Ok(ColorPlan::new(method, c))
    }

    /// The gradient position of a coloring value before the offset (lit layers' heights).
    pub fn position(&self, v: f64) -> f64 {
        self.transfer.apply(v) * self.density
    }

    /// Gradient color (linear RGB, alpha) for a coloring value.
    pub fn color(&self, v: f64) -> [f32; 4] {
        crate::gradient::lut_sample(&self.lut, self.transfer.apply(v) * self.density + self.offset)
    }

    pub fn formula(&self) -> Option<&Prog> {
        match &self.method {
            ColorMethod::Formula(p) => Some(p),
            _ => None,
        }
    }

    /// Index of the method in the shader's `switch` (formula methods share one value).
    pub fn method_index(&self) -> u32 {
        match self.method {
            ColorMethod::Iterations => 0,
            ColorMethod::Smooth => 1,
            ColorMethod::Distance => 2,
            ColorMethod::Solid => 3,
            ColorMethod::Magnitude => 4,
            ColorMethod::Angle => 5,
            ColorMethod::Formula(_) => 6,
        }
    }
}

/// Pixel → plane mapping.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewFrame {
    pub width: u32,
    pub height: u32,
    pub center: [f64; 2],
    /// Half the height in plane units (`f64`; 0 below its range, see `radius_fx`).
    pub radius: f64,
    pub radius_fx: crate::math::fx::Fx,
    pub cos: f64,
    pub sin: f64,
}

impl ViewFrame {
    /// Size of one pixel in plane units at any depth.
    pub fn pixel_size_fx(&self) -> crate::math::fx::Fx {
        self.radius_fx * (2.0 / self.height as f64)
    }

    /// The largest offset of a pixel from the center (half the image diagonal), any depth.
    pub fn max_offset_fx(&self) -> crate::math::fx::Fx {
        let aspect = self.width as f64 / self.height as f64;
        self.radius_fx * (1.0 + aspect * aspect).sqrt()
    }

    /// Offset of image position (x, y) from the center in plane units, at any depth.
    pub fn offset_fx(&self, x: f64, y: f64) -> crate::math::fx::Cfx {
        let dx = x - self.width as f64 * 0.5;
        let dy = -(y - self.height as f64 * 0.5);
        let s = self.radius_fx / (self.height as f64 * 0.5);
        let (re, im) = (dx * self.cos - dy * self.sin, dx * self.sin + dy * self.cos);
        crate::math::fx::Cfx::from_fx(s * re, s * im)
    }

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

/// One fractal layer (or mask) resolved for rendering.
#[derive(Clone, Debug)]
pub struct LayerPlan {
    pub fractal: Prog,
    pub max_iter: u32,
    pub julia: bool,
    pub seed: [f64; 2],
    pub transforms: Vec<Prog>,
    pub outside: ColorPlan,
    pub inside: ColorPlan,
    /// Density layers: the orbits are counted per pixel instead of colored per pixel.
    pub density: Option<crate::doc::Density>,
    /// Lit layers: their colors are shaded as a height field (see [`light`]).
    pub light: Option<light::LightPlan>,
    /// 3D object layers: the raymarched shape with its own lighting (see [`object`]).
    pub object: Option<object::ObjectPlan>,
}

impl LayerPlan {
    /// A coloring formula runs code after every iteration (orbit traps and the like): deep
    /// zooms cannot skip iterations for it.
    pub fn coloring_loops(&self) -> bool {
        [&self.outside, &self.inside].iter().any(|c| c.formula().is_some_and(|p| !p.program.body.is_empty()))
    }

    pub fn new(l: &Layer) -> Result<LayerPlan, String> {
        let fractal = Prog::resolve(&l.formula, &l.params, ProgramKind::Fractal).map_err(|e| format!("{}: {e}", l.name))?;
        let transforms = l
            .transforms
            .iter()
            .filter(|t| t.enabled)
            .map(|t| Prog::resolve(&t.formula, &t.params, ProgramKind::Transform))
            .collect::<Result<_, _>>()
            .map_err(|e| format!("{}: {e}", l.name))?;
        Ok(LayerPlan {
            max_iter: l.max_iter.max(1),
            julia: l.julia && fractal.program.has_julia,
            seed: l.seed,
            transforms,
            outside: ColorPlan::outside(l.outside_method, &l.outside).map_err(|e| format!("{}: {e}", l.name))?,
            inside: ColorPlan::inside(l.inside_method, &l.inside).map_err(|e| format!("{}: {e}", l.name))?,
            density: (l.kind == LayerKind::Density).then(|| l.density.clone()),
            light: (l.light.enabled && l.kind == LayerKind::Fractal).then(|| light::LightPlan::new(&l.light)),
            object: (l.kind == LayerKind::Object).then(|| object::ObjectPlan::new(&l.object, &l.light)),
            fractal,
        })
    }

    /// The outside coloring or the lighting needs the orbit's derivative (distance estimation).
    pub fn needs_derivative(&self) -> bool {
        matches!(self.outside.method, ColorMethod::Distance) || self.light.as_ref().is_some_and(|l| l.source == crate::doc::HeightSource::Distance)
    }
}

/// Merge mode and opacity of a layer or group.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Blend {
    pub mode: MergeMode,
    pub opacity: f32,
}

#[derive(Clone, Debug)]
pub struct MaskPlan {
    pub layer: LayerPlan,
    pub invert: bool,
    pub source: MaskSource,
}

#[derive(Clone, Debug)]
pub enum Node {
    Layer { layer: Box<LayerPlan>, blend: Blend, mask: Option<MaskPlan> },
    Group { children: Vec<Node>, blend: Blend, mask: Option<MaskPlan> },
}

#[derive(Clone, Debug)]
pub struct Plan {
    pub frame: ViewFrame,
    /// The view center as exact decimal text (reference orbits for deep zooms).
    pub center_text: [String; 2],
    /// Linear RGB and alpha below all layers.
    pub background: [f32; 4],
    /// Bottom first.
    pub nodes: Vec<Node>,
}

fn mask_plan(l: &Layer) -> Result<Option<MaskPlan>, String> {
    match &l.mask {
        Some(m) if m.enabled => {
            Ok(Some(MaskPlan { layer: LayerPlan::new(&m.layer).map_err(|e| format!("mask of {e}"))?, invert: m.invert, source: m.source }))
        }
        _ => Ok(None),
    }
}

fn nodes(list: &[Layer]) -> Result<Vec<Node>, String> {
    let mut out = Vec::new();
    for l in list.iter().filter(|l| l.visible && l.opacity > 0.0) {
        let blend = Blend { mode: l.merge, opacity: l.opacity.clamp(0.0, 1.0) as f32 };
        match l.kind {
            LayerKind::Fractal | LayerKind::Density | LayerKind::Object => {
                out.push(Node::Layer { layer: Box::new(LayerPlan::new(l)?), blend, mask: mask_plan(l)? })
            }
            LayerKind::Group => {
                let children = nodes(&l.children)?;
                if !children.is_empty() {
                    out.push(Node::Group { children, blend, mask: mask_plan(l)? });
                }
            }
        }
    }
    Ok(out)
}

impl Plan {
    /// The render plan of the document at its image size.
    pub fn new(doc: &Document) -> Result<Plan, String> {
        Plan::sized(doc, doc.image.width, doc.image.height)
    }

    pub fn sized(doc: &Document, width: u32, height: u32) -> Result<Plan, String> {
        let rot = doc.view.rotation.to_radians();
        Ok(Plan {
            frame: ViewFrame {
                width: width.max(1),
                height: height.max(1),
                center: doc.view.center_f64(),
                radius: doc.view.radius.to_f64(),
                radius_fx: doc.view.radius,
                cos: rot.cos(),
                sin: rot.sin(),
            },
            center_text: doc.view.center.clone(),
            background: doc.image.background,
            nodes: nodes(&doc.layers)?,
        })
    }

    /// Every layer of the plan (masks included), in render order.
    pub fn layers(&self) -> Vec<&LayerPlan> {
        fn walk<'a>(list: &'a [Node], out: &mut Vec<&'a LayerPlan>) {
            for n in list {
                match n {
                    Node::Layer { layer, mask, .. } => {
                        if let Some(m) = mask {
                            out.push(&m.layer);
                        }
                        out.push(layer);
                    }
                    Node::Group { children, mask, .. } => {
                        if let Some(m) = mask {
                            out.push(&m.layer);
                        }
                        walk(children, out);
                    }
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.nodes, &mut out);
        out
    }

    /// Some layer's coloring needs derivatives.
    pub fn needs_derivative(&self) -> bool {
        self.layers().iter().any(|l| l.needs_derivative())
    }

    /// Arithmetic the view needs: pixels must stay well above the rounding step of the
    /// coordinates (their magnitude, at least 1).
    pub fn precision(&self) -> Precision {
        let f = &self.frame;
        let scale = f.center[0].abs().max(f.center[1].abs()).max(1.0);
        let ratio = (f.pixel_size_fx() / scale).to_f64();
        if ratio > SINGLE_LIMIT {
            Precision::Single
        } else if ratio > DOUBLE_LIMIT {
            Precision::Double
        } else {
            Precision::Beyond
        }
    }

    /// Shifts every layer's gradients by `by` (one is a full turn): color cycling. Masks keep
    /// their colors.
    pub fn shift_colors(&mut self, by: f64) {
        fn walk(nodes: &mut [Node], by: f64) {
            for n in nodes {
                match n {
                    Node::Layer { layer, .. } => {
                        layer.outside.offset += by;
                        layer.inside.offset += by;
                    }
                    Node::Group { children, .. } => walk(children, by),
                }
            }
        }
        walk(&mut self.nodes, by);
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
        (ParamKind::Func, ParamValue::Func(f))
            if crate::formula::ir::Func::by_name(f).is_some() || crate::formula::color::merge_mode(f).is_some() =>
        {
            v.clone()
        }
        (ParamKind::Func, _) => default.clone(),
        (ParamKind::Bool, ParamValue::Bool(_))
        | (ParamKind::Int, ParamValue::Int(_))
        | (ParamKind::Float, ParamValue::Float(_))
        | (ParamKind::Complex, ParamValue::Complex(_))
        | (ParamKind::Color, ParamValue::Color(_)) => v.clone(),
        (ParamKind::Bool, other) => ParamValue::Bool(other.as_f64() != 0.0),
        (ParamKind::Int, other) => ParamValue::Int(other.as_f64().round() as i64),
        (ParamKind::Float, other) => ParamValue::Float(other.as_f64()),
        (ParamKind::Complex, other) => ParamValue::Complex(other.as_complex()),
        (ParamKind::Color, other) => ParamValue::Color(other.as_color()),
        (ParamKind::Class, ParamValue::Class { .. }) | (ParamKind::Image, ParamValue::Image { .. }) => v.clone(),
        (ParamKind::Class | ParamKind::Image, _) => default.clone(),
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
        OutsideMethod::Iterations | OutsideMethod::Solid | OutsideMethod::Formula => n,
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
        InsideMethod::Solid | InsideMethod::Formula => 0.0,
        InsideMethod::Magnitude => o.z.cabs(),
        InsideMethod::Angle => o.z.arg() / std::f64::consts::TAU + 0.5,
    }
}

/// The coloring value of a region by its native method (`None` for solid and formula colors).
pub fn native_value(c: &ColorPlan, o: &OrbitResult, pixel_size: f64) -> Option<f64> {
    Some(match &c.method {
        ColorMethod::Solid | ColorMethod::Formula(_) => return None,
        ColorMethod::Smooth => smooth_iter(o),
        ColorMethod::Iterations => o.iter as f64,
        ColorMethod::Distance => outside_value(OutsideMethod::Distance, o, pixel_size),
        ColorMethod::Magnitude => inside_value(InsideMethod::Magnitude, o),
        ColorMethod::Angle => inside_value(InsideMethod::Angle, o),
    })
}

/// Color (linear RGB, straight alpha) of one region by its native method (not Formula).
pub fn shade_native(c: &ColorPlan, o: &OrbitResult, pixel_size: f64) -> [f32; 4] {
    match native_value(c, o, pixel_size) {
        Some(v) => finite(c.color(v)),
        None => c.solid,
    }
}

/// Color of a region colored by a formula's `#index` / `#solid`, or by its `#color`
/// (`direct`, sRGB-encoded RGBA) when it sets one.
pub fn shade_formula(c: &ColorPlan, index: f64, solid: bool, direct: Option<[f64; 4]>) -> [f32; 4] {
    if solid {
        return c.solid;
    }
    match direct {
        Some(k) => {
            let d = |v: f64| composite::srgb_decode(v.clamp(0.0, 1.0) as f32);
            finite([d(k[0]), d(k[1]), d(k[2]), k[3].clamp(0.0, 1.0) as f32])
        }
        None => finite(c.color(index)),
    }
}

fn finite(c: [f32; 4]) -> [f32; 4] {
    if c.iter().all(|v| v.is_finite()) { c } else { [0.0, 0.0, 0.0, 1.0] }
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
