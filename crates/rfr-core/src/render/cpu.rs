// SPDX-License-Identifier: LGPL-3.0-or-later

//! CPU reference renderer: one sample per pixel per pass. Every layer runs its transformations,
//! the fractal with its coloring formulas and the shading; the layer tree is composited over the
//! background; the result is accumulated as premultiplied linear RGBA. The GPU renderer follows
//! it step by step (same sample positions, same colorings, same compositing).

use rayon::prelude::*;

use super::composite::{composite, mask_value};
use super::{ColorMethod, LayerPlan, Node, OrbitResult, Plan, Precision, density, sample_offset, shade_formula, shade_native};
use crate::formula::cpu::{Compiled, Frame, Hook};
use crate::formula::perturb::{BlaTable, Deep, Perturbation, RefOrbit};
use crate::gradient::linear_to_srgb;
use crate::math::complex::{C64, Cx};
use crate::math::fx::Cfx;
use crate::math::scalar::{Dual, Scalar};

/// The compiled programs of one layer.
struct LayerK<S> {
    fractal: Compiled<S>,
    /// Deep zooms: the perturbation of the fractal, its reference orbit and the linear steps
    /// (when no coloring needs every iteration).
    pert: Option<(Perturbation, RefOrbit, Option<BlaTable>)>,
    transforms: Vec<Compiled<S>>,
    outside: Option<Compiled<S>>,
    inside: Option<Compiled<S>>,
}

/// Per-thread frames of one layer.
#[derive(Clone)]
struct LayerF<S> {
    fractal: Frame<S>,
    transforms: Vec<Frame<S>>,
    outside: Option<Frame<S>>,
    inside: Option<Frame<S>>,
}

impl<S: Scalar + Send + Sync + 'static> LayerK<S> {
    fn new(lp: &LayerPlan) -> LayerK<S> {
        let coloring = |c: &super::ColorPlan| c.formula().map(|p| Compiled::with_gradient(&p.program, &p.values, c.lut.clone()));
        LayerK {
            fractal: Compiled::new(&lp.fractal.program, &lp.fractal.values),
            pert: None,
            transforms: lp.transforms.iter().map(|t| Compiled::new(&t.program, &t.values)).collect(),
            outside: coloring(&lp.outside),
            inside: coloring(&lp.inside),
        }
    }

    fn frames(&self) -> LayerF<S> {
        LayerF {
            fractal: self.fractal.frame(),
            transforms: self.transforms.iter().map(|t| t.frame()).collect(),
            outside: self.outside.as_ref().map(|c| c.frame()),
            inside: self.inside.as_ref().map(|c| c.frame()),
        }
    }

    /// Color (linear RGB, straight alpha) of the layer at plane point `p` of pixel `pix`
    /// (x, y, sample index).
    fn eval(&self, f: &mut LayerF<S>, lp: &LayerPlan, plan: &Plan, mut p: Cx<S>, d: Cfx, pix: (u32, u32, u32)) -> [f32; 4] {
        // Screen values and one random stream per program (kernel.wgsl seeds the same way).
        let base = super::hash(pix.0 ^ super::hash(pix.1 ^ super::hash(pix.2.wrapping_add(0x9E37_79B9))));
        let screen = |fr: &mut Frame<S>, tag: u32| {
            fr.px = pix.0 as i64;
            fr.py = pix.1 as i64;
            fr.width = plan.frame.width as i64;
            fr.height = plan.frame.height as i64;
            fr.angle = plan.frame.sin.atan2(plan.frame.cos);
            fr.rng.set(super::hash(base ^ tag));
        };
        screen(&mut f.fractal, 1);
        for (k, tf) in f.transforms.iter_mut().enumerate() {
            screen(tf, 2 + k as u32);
        }
        if let Some(fr) = f.outside.as_mut() {
            screen(fr, 100);
        }
        if let Some(fr) = f.inside.as_mut() {
            screen(fr, 101);
        }
        for (t, tf) in self.transforms.iter().zip(f.transforms.iter_mut()) {
            let (q, solid) = t.transform(tf, p);
            if solid {
                return [0.0; 4];
            }
            p = q;
        }
        let c = if lp.julia { Cx::from_f64(lp.seed[0], lp.seed[1]) } else { p };
        let center = Cx::from_f64(plan.frame.center[0], plan.frame.center[1]);
        let magn = S::from_f64(plan.magn());
        let setup = |fr: &mut Frame<S>| {
            fr.pixel = p;
            fr.c = c;
            fr.ismand = !lp.julia;
            fr.center = center;
            fr.magn = magn;
        };
        setup(&mut f.fractal);
        let maxit = lp.max_iter;
        let o = {
            let pairs = [(self.outside.as_ref(), f.outside.as_mut()), (self.inside.as_ref(), f.inside.as_mut())];
            let mut colorings: Vec<(&Compiled<S>, &mut Frame<S>)> = pairs.into_iter().filter_map(|(k, fr)| Some((k?, fr?))).collect();
            for (_, fr) in colorings.iter_mut() {
                setup(fr);
            }
            let hook = |h: Hook<S>| match h {
                Hook::Init(z) => {
                    for (k, fr) in colorings.iter_mut() {
                        fr.zcur = z;
                        k.color_init(fr, maxit);
                    }
                }
                Hook::Iter(it, z, zp) => {
                    for (k, fr) in colorings.iter_mut() {
                        fr.zcur = z;
                        k.color_loop(fr, it, zp);
                    }
                }
            };
            match &self.pert {
                Some((pert, orbit, bla)) => self.fractal.run_perturbed(&mut f.fractal, maxit, Deep { pert, orbit, bla: bla.as_ref() }, d, hook),
                None => self.fractal.run_with(&mut f.fractal, maxit, hook),
            }
        };
        let region = if o.escaped { &lp.outside } else { &lp.inside };
        if let ColorMethod::Formula(_) = region.method {
            let (k, fr) = if o.escaped { (self.outside.as_ref(), f.outside.as_mut()) } else { (self.inside.as_ref(), f.inside.as_mut()) };
            if let (Some(k), Some(fr)) = (k, fr) {
                fr.zcur = o.z;
                let (index, solid) = k.color_final(fr, o.iter, o.zprev);
                return shade_formula(region, index.v(), solid, k.direct_color(fr));
            }
            return region.solid;
        }
        let dz = (o.z.re.d() * o.z.re.d() + o.z.im.d() * o.z.im.d()).sqrt();
        let r = OrbitResult { z: o.z.v(), zprev: o.zprev.v(), zprev2: o.zprev2.v(), iter: o.iter, escaped: o.escaped, dz };
        shade_native(region, &r, plan.frame.pixel_size())
    }
}

/// A layer compiled plain or with derivatives (distance estimation), or a density layer.
enum Kernel {
    Plain(LayerK<f64>),
    Dual(LayerK<Dual>),
    Density(Box<DensityK>),
}

/// A density layer: its formula, the counts of its orbits and its strokes (L-systems).
struct DensityK {
    fractal: Compiled<f64>,
    hist: density::Histogram,
    strokes: density::Strokes,
}

#[derive(Clone)]
enum KernelF {
    Plain(Box<LayerF<f64>>),
    Dual(Box<LayerF<Dual>>),
    None,
}

impl Kernel {
    fn new(lp: &LayerPlan, plan: &Plan) -> Kernel {
        if let Some(d) = &lp.density {
            let channels = density::channels(d);
            return Kernel::Density(Box::new(DensityK {
                fractal: Compiled::new(&lp.fractal.program, &lp.fractal.values),
                hist: density::Histogram::new(
                    plan.frame.width,
                    plan.frame.height,
                    channels,
                    if density::metropolis(d) { d.orbits as usize } else { 0 },
                ),
                strokes: density::Strokes::new(d),
            }));
        }
        // Deeper than double precision: perturbation when the formula allows it.
        if plan.precision() == Precision::Beyond
            && lp.transforms.is_empty()
            && let Ok(pert) = Perturbation::new(&lp.fractal.program, &lp.fractal.values, !lp.julia, lp.seed)
        {
            let mut k = LayerK::<f64>::new(lp);
            let bits = crate::math::big::bits_for(plan.frame.pixel_size_fx());
            let orbit = k.fractal.reference_orbit(&pert, &plan.center_text, bits, lp.max_iter);
            let bla =
                if lp.coloring_loops() { None } else { pert.bla_table(&orbit, plan.frame.max_offset_fx(), crate::formula::perturb::BLA_EPS_CPU) };
            k.pert = Some((pert, orbit, bla));
            return Kernel::Plain(k);
        }
        if lp.needs_derivative() { Kernel::Dual(LayerK::new(lp)) } else { Kernel::Plain(LayerK::new(lp)) }
    }
    fn frames(&self) -> KernelF {
        match self {
            Kernel::Plain(k) => KernelF::Plain(Box::new(k.frames())),
            Kernel::Dual(k) => KernelF::Dual(Box::new(k.frames())),
            Kernel::Density(_) => KernelF::None,
        }
    }
    /// The layer's color at plane point `p`; `turn` is the derivative's direction (a turn).
    /// `at`: the sample's plane point and its offset from the view center (deep zooms).
    fn eval(&self, f: &mut KernelF, lp: &LayerPlan, plan: &Plan, at: (C64, Cfx), turn: f64, pix: (u32, u32, u32)) -> [f32; 4] {
        let (p, d) = at;
        if let Kernel::Density(k) = self {
            return lp.density.as_ref().map_or([0.0; 4], |dn| density::color(lp, dn, &k.hist, pix.0, pix.1));
        }
        match (self, f) {
            (Kernel::Plain(k), KernelF::Plain(f)) => k.eval(f, lp, plan, p, d, pix),
            (Kernel::Dual(k), KernelF::Dual(f)) => {
                // Derivative along a direction that turns from sample to sample (non-analytic
                // formulas average over directions); |dz| is per plane unit.
                let (s, c) = (turn * std::f64::consts::TAU).sin_cos();
                k.eval(f, lp, plan, Cx::new(Dual::new(p.re, c), Dual::new(p.im, s)), d, pix)
            }
            _ => unreachable!("kernel and frames differ"),
        }
    }
}

pub struct CpuRenderer {
    plan: Plan,
    /// One kernel per layer of `plan.layers()` (same order).
    kernels: Vec<Kernel>,
    /// Premultiplied linear RGBA sums per pixel, row-major.
    accum: Vec<[f32; 4]>,
    samples: u32,
    /// Passes of the density layers' orbits so far.
    density_passes: u32,
    /// Samples the render aims at (density layers restart the image while it fills).
    pub target_samples: u32,
}

/// One sample's evaluation state: the layers are evaluated in `plan.layers()` order.
struct Ctx<'a> {
    kernels: &'a [Kernel],
    frames: &'a mut [KernelF],
    next: usize,
    plan: &'a Plan,
    p: C64,
    /// The sample's offset from the view center (deep zooms).
    d: Cfx,
    turn: f64,
    pix: (u32, u32, u32),
}

impl Ctx<'_> {
    fn layer(&mut self, lp: &LayerPlan) -> [f32; 4] {
        let i = self.next;
        self.next += 1;
        self.kernels[i].eval(&mut self.frames[i], lp, self.plan, (self.p, self.d), self.turn, self.pix)
    }
}

/// Composites the node list over `below`.
fn composite_nodes(nodes: &[Node], below: [f32; 4], ctx: &mut Ctx) -> [f32; 4] {
    let mut acc = below;
    for n in nodes {
        let (mask, blend) = match n {
            Node::Layer { mask, blend, .. } | Node::Group { mask, blend, .. } => (mask, blend),
        };
        let m = mask.as_ref().map(|m| {
            let c = ctx.layer(&m.layer);
            mask_value(c, m.source, m.invert)
        });
        let top = match n {
            Node::Layer { layer, .. } => ctx.layer(layer),
            Node::Group { children, .. } => composite_nodes(children, [0.0; 4], ctx),
        };
        acc = composite(acc, top, blend.mode, blend.opacity * m.unwrap_or(1.0));
    }
    acc
}

impl CpuRenderer {
    pub fn new(plan: Plan) -> CpuRenderer {
        let kernels = plan.layers().iter().map(|l| Kernel::new(l, &plan)).collect();
        let n = (plan.frame.width * plan.frame.height) as usize;
        CpuRenderer { plan, kernels, accum: vec![[0.0; 4]; n], samples: 0, density_passes: 0, target_samples: u32::MAX }
    }

    pub fn plan(&self) -> &Plan {
        &self.plan
    }

    pub fn samples(&self) -> u32 {
        self.samples
    }

    /// Color (linear RGB, straight alpha) of one sample at image position (x, y).
    fn sample(&self, frames: &mut [KernelF], x: f64, y: f64, turn: f64, pix: (u32, u32, u32)) -> [f32; 4] {
        let (p, d) = (self.plan.frame.plane(x, y), self.plan.frame.offset_fx(x, y));
        let mut ctx = Ctx { kernels: &self.kernels, frames, next: 0, plan: &self.plan, p, d, turn, pix };
        composite_nodes(&self.plan.nodes, self.plan.background, &mut ctx)
    }

    /// Density layers: one pass of orbits into their counts. While the counts fill up, the
    /// accumulated image restarts whenever the passes double, so it never keeps samples made
    /// from counts below half of the current ones.
    fn density_pass(&mut self) {
        let layers = self.plan.layers();
        let mut any = false;
        for (k, lp) in self.kernels.iter().zip(&layers) {
            let (Kernel::Density(dk), Some(d)) = (k, &lp.density) else { continue };
            any = true;
            let frame = self.plan.frame;
            let s = density::Scatter {
                fractal: &dk.fractal,
                lp,
                density: d,
                frame: &frame,
                hist: &dk.hist,
                pass: self.density_passes,
                strokes: &dk.strokes,
            };
            (0..d.orbits).into_par_iter().for_each_init(|| dk.fractal.frame(), |f, i| s.orbit(f, i));
            dk.hist.update_max();
        }
        if any {
            self.density_passes += 1;
            if density::restart_due(self.density_passes, self.target_samples) {
                self.accum.iter_mut().for_each(|a| *a = [0.0; 4]);
                self.samples = 0;
            }
        }
    }

    /// Adds one sample to every pixel.
    pub fn pass(&mut self) {
        self.density_pass();
        let w = self.plan.frame.width as usize;
        let s = self.samples;
        let proto: Vec<KernelF> = self.kernels.iter().map(|k| k.frames()).collect();
        let mut accum = std::mem::take(&mut self.accum);
        let this = &*self;
        accum.par_chunks_mut(w).enumerate().for_each_init(
            || proto.clone(),
            |frames, (y, row)| {
                for (x, acc) in row.iter_mut().enumerate() {
                    let (u, v, turn) = sample_offset(x as u32, y as u32, s);
                    let c = this.sample(frames, x as f64 + u, y as f64 + v, turn, (x as u32, y as u32, s));
                    acc[0] += c[0] * c[3];
                    acc[1] += c[1] * c[3];
                    acc[2] += c[2] * c[3];
                    acc[3] += c[3];
                }
            },
        );
        self.accum = accum;
        self.samples += 1;
    }

    /// Averaged linear RGB with straight alpha per pixel.
    pub fn linear(&self) -> Vec<[f32; 4]> {
        let k = 1.0 / self.samples.max(1) as f32;
        self.accum.iter().map(|a| unpremultiply(*a, k)).collect()
    }

    /// The image as 8-bit sRGB RGBA (straight alpha).
    pub fn rgba8(&self) -> Vec<u8> {
        rgba8(&self.linear())
    }

    /// Color of the bottom layer at plane point `p` without compositing (tests, probes).
    pub fn layer_color_at(&self, p: C64) -> Option<[f32; 4]> {
        let layers = self.plan.layers();
        let lp = layers.first()?;
        let mut f = self.kernels.first()?.frames();
        let d = Cfx::from_c64(p.re - self.plan.frame.center[0], p.im - self.plan.frame.center[1]);
        Some(self.kernels[0].eval(&mut f, lp, &self.plan, (p, d), 0.0, (0, 0, 0)))
    }
}

/// Premultiplied sums (scaled by `k` = 1 / samples) → straight alpha.
pub fn unpremultiply(a: [f32; 4], k: f32) -> [f32; 4] {
    let alpha = a[3] * k;
    if alpha <= 1e-6 {
        return [0.0; 4];
    }
    [a[0] * k / alpha, a[1] * k / alpha, a[2] * k / alpha, alpha.min(1.0)]
}

/// Linear RGB, straight alpha → 8-bit sRGB RGBA.
pub fn rgba8(lin: &[[f32; 4]]) -> Vec<u8> {
    lin.iter().flat_map(|c| [linear_to_srgb(c[0]), linear_to_srgb(c[1]), linear_to_srgb(c[2]), (c[3].clamp(0.0, 1.0) * 255.0 + 0.5) as u8]).collect()
}

/// Renders `samples` passes of the document at `width` × `height`.
pub fn render(doc: &crate::doc::Document, width: u32, height: u32, samples: u32) -> Result<CpuRenderer, String> {
    let mut r = CpuRenderer::new(Plan::sized(doc, width, height)?);
    r.target_samples = samples.max(1);
    for _ in 0..samples.max(1) {
        r.pass();
    }
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::{Document, InsideMethod, Layer, LayerKind, Mask, MergeMode, OutsideMethod, TransformRef};

    #[test]
    fn renders_the_mandelbrot_set() {
        let doc = Document::default();
        let r = render(&doc, 64, 36, 2).unwrap();
        let img = r.rgba8();
        assert_eq!(img.len(), 64 * 36 * 4);
        // The center of the default view (-0.5, 0) is inside: solid black.
        let i = (18 * 64 + 32) * 4;
        assert_eq!(&img[i..i + 4], &[0, 0, 0, 255]);
        assert!(img[0..3].iter().any(|&v| v > 0));
    }

    #[test]
    fn distance_estimation_uses_derivatives() {
        let mut doc = Document::default();
        doc.layers[0].outside_method = OutsideMethod::Distance;
        let r = render(&doc, 48, 27, 1).unwrap();
        assert!(matches!(r.kernels[0], Kernel::Dual(_)));
    }

    #[test]
    fn layers_groups_masks_transforms_and_colorings() {
        let mut doc = Document::default();
        doc.image.background = [0.0; 4];
        // Bottom: Mandelbrot with coloring formulas inside and out.
        doc.layers[0].outside_method = OutsideMethod::Formula;
        doc.layers[0].inside_method = InsideMethod::Formula;
        doc.layers[0].inside.formula = "rfract-coloring:Bof60".into();
        // A group with a kaleidoscope Julia layer at half opacity, masked by a Newton fractal.
        let mut julia = Layer { name: "Julia".into(), julia: true, merge: MergeMode::Screen, ..Layer::default() };
        julia.transforms.push(TransformRef { formula: "rfract-transform:Kaleidoscope".into(), ..TransformRef::default() });
        let mut group = Layer { name: "Group".into(), kind: LayerKind::Group, opacity: 0.5, children: vec![julia], ..Layer::default() };
        let mut mask = Mask::default();
        mask.layer.formula = "rfract:Newton".into();
        group.mask = Some(mask);
        doc.layers.push(group);
        let plan = Plan::sized(&doc, 32, 18).unwrap();
        assert_eq!(plan.layers().len(), 3, "base, mask, julia");
        let img = render(&doc, 32, 18, 2).unwrap().rgba8();
        assert!(img.chunks(4).all(|p| p[3] > 0), "the base layer covers every pixel");
        let mut alone = doc.clone();
        alone.layers.truncate(1);
        assert_ne!(render(&alone, 32, 18, 2).unwrap().rgba8(), img, "the group changes the image");
    }
}
