// SPDX-License-Identifier: LGPL-3.0-or-later

//! CPU reference renderer: one sample per pixel per pass. Every layer runs its transformations,
//! the fractal with its coloring formulas and the shading; the layer tree is composited over the
//! background; the result is accumulated as premultiplied linear RGBA. The GPU renderer follows
//! it step by step (same sample positions, same colorings, same compositing).

use std::sync::atomic::{AtomicBool, Ordering};

use rayon::prelude::*;

use super::composite::{composite, mask_value};
use super::{
    ColorMethod, LayerPlan, Node, OrbitResult, Plan, Precision, density, light, native_value, sample_offset, shade_formula, shade_native, smooth_iter,
};
use crate::formula::cpu::{Compiled, Frame, Hook, Orbit};
use crate::formula::perturb::{BlaTable, Deep, Perturbation, RefOrbit};
use crate::gradient::linear_to_srgb;
use crate::math::complex::{C64, Cx};
use crate::math::fx::Cfx;
use crate::math::scalar::{Dual, Scalar};

/// A sample's offset from the view center (deep zooms) and the derivative of its pixel
/// (distance estimation in perturbation views).
type Seed = (Cfx, Option<Cfx>);

/// The compiled programs of one layer.
struct LayerK<S> {
    fractal: Compiled<S>,
    /// Deep zooms: the perturbation of the fractal, its reference orbit and the linear steps
    /// (when no coloring needs every iteration).
    pert: Option<(Perturbation, RefOrbit, Option<BlaTable>)>,
    transforms: Vec<Compiled<S>>,
    post: Vec<Compiled<S>>,
    outside: Option<Compiled<S>>,
    inside: Option<Compiled<S>>,
}

/// Per-thread frames of one layer.
#[derive(Clone)]
struct LayerF<S> {
    fractal: Frame<S>,
    transforms: Vec<Frame<S>>,
    post: Vec<Frame<S>>,
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
            post: lp.post_transforms.iter().map(|t| Compiled::new(&t.program, &t.values)).collect(),
            outside: coloring(&lp.outside),
            inside: coloring(&lp.inside),
        }
    }

    fn frames(&self) -> LayerF<S> {
        LayerF {
            fractal: self.fractal.frame(),
            transforms: self.transforms.iter().map(|t| t.frame()).collect(),
            post: self.post.iter().map(|t| t.frame()).collect(),
            outside: self.outside.as_ref().map(|c| c.frame()),
            inside: self.inside.as_ref().map(|c| c.frame()),
        }
    }

    /// Color (linear RGB, straight alpha) of the layer at plane point `p` of pixel `pix`
    /// (x, y, sample index), its height when the layer is lit (else 0), and whether the orbit
    /// stayed inside for at least [`GUESS_MIN_ITER`] iterations (see [`guess_tile`]). `deep`:
    /// see [`LayerK::orbit`].
    fn eval(&self, f: &mut LayerF<S>, lp: &LayerPlan, plan: &Plan, p: Cx<S>, deep: Seed, pix: (u32, u32, u32)) -> ([f32; 4], [f32; 2], bool) {
        let Some((o, post)) = self.orbit(f, lp, plan, p, deep, pix) else {
            return ([0.0; 4], [0.0, 3.0e38], false);
        };
        let maxit = lp.max_iter;
        let region = if o.escaped { &lp.outside } else { &lp.inside };
        let r = Self::result(&o);
        // The color and the gradient position before the offset (lit layers' heights).
        let (color, raw) = self.shade(f, lp, &o, &post, &r);
        let value = raw.map_or(0.0, |v| region.position(v));
        // Lit layers and layers with effects: the height and the distance to the set in pixels.
        let field = lp.light.as_ref().map_or([0.0, 3.0e38], |l| {
            let smooth = if o.escaped { smooth_iter(&r) } else { maxit as f64 };
            let distance = if l.source == crate::doc::HeightSource::Distance { light::distance_height(&r) } else { 0.0 };
            [light::height(l.source, smooth, value, distance, color), light::distance_px(&r)]
        });
        (color, field, !o.escaped && maxit >= GUESS_MIN_ITER)
    }

    fn result(o: &Orbit<S>) -> OrbitResult {
        let dz = (o.z.re.d() * o.z.re.d() + o.z.im.d() * o.z.im.d()).sqrt();
        OrbitResult { z: o.z.v(), zprev: o.zprev.v(), zprev2: o.zprev2.v(), iter: o.iter, escaped: o.escaped, dz }
    }

    /// Applies the post-transformations to `z` in order (each one only while the ones
    /// before left the point alive); returns the result and whether one made it solid.
    fn post_z(post: &[Compiled<S>], frames: &mut [Frame<S>], mut z: Cx<S>) -> (Cx<S>, bool) {
        for (t, tf) in post.iter().zip(frames.iter_mut()) {
            let (q, solid) = t.transform(tf, z);
            if solid {
                return (z, true);
            }
            z = q;
        }
        (z, false)
    }

    /// The color of an orbit by its region's coloring and the coloring value (before the
    /// transfer; none for solid colors and formulas that set the color directly).
    fn shade(&self, f: &mut LayerF<S>, lp: &LayerPlan, o: &Orbit<S>, post: &PostZ<S>, r: &OrbitResult) -> ([f32; 4], Option<f64>) {
        let region = if o.escaped { &lp.outside } else { &lp.inside };
        match region.method {
            ColorMethod::Formula(_) => {
                let (k, fr) = if o.escaped { (self.outside.as_ref(), f.outside.as_mut()) } else { (self.inside.as_ref(), f.inside.as_mut()) };
                match (k, fr) {
                    (Some(k), Some(fr)) => {
                        let (z, zprev) = if post.on { (post.z, post.zprev) } else { (o.z, o.zprev) };
                        fr.zcur = z;
                        let (index, solid) = k.color_final(fr, o.iter, zprev);
                        let solid = solid || post.solid;
                        let direct = k.direct_color(fr);
                        let value = (!solid && direct.is_none()).then(|| index.v());
                        (shade_formula(region, index.v(), solid, direct), value)
                    }
                    _ => (region.solid, None),
                }
            }
            _ => (shade_native(region, r), native_value(region, r)),
        }
    }

    /// The orbit of the sample at plane point `p` of pixel `pix` (its transformations, the
    /// fractal with the colorings' hooks) and `z` as the colorings see it at the end; none
    /// when a transformation makes it solid. Deep zooms: `deep` is the offset from the view
    /// center and the derivative of the pixel (distance estimation).
    fn orbit(&self, f: &mut LayerF<S>, lp: &LayerPlan, plan: &Plan, mut p: Cx<S>, deep: Seed, pix: (u32, u32, u32)) -> Option<(Orbit<S>, PostZ<S>)> {
        let (d, dseed) = deep;
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
        for (k, tf) in f.post.iter_mut().enumerate() {
            screen(tf, 200 + k as u32);
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
                return None;
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
        // Early inside: the main bulbs of z² + c, periodicity (not in deep zooms).
        f.fractal.period =
            if lp.early.periodicity && self.pert.is_none() { super::period_tolerance(plan.frame.pixel_size(), PERIOD_FLOOR) } else { 0.0 };
        let bulb = lp.early.bulbs && self.pert.is_none() && crate::formula::period::in_main_bulbs(c.re.v(), c.im.v());
        // `z` as the colorings see it: every value of the orbit they get goes through the
        // post-transformations once (kernel.wgsl `pz`, `pzp`, `psolid`).
        let mut post = PostZ { on: !self.post.is_empty(), z: Cx::zero(), zprev: Cx::zero(), solid: false };
        let post_k = &self.post;
        let post_f = &mut f.post;
        let mut push = |post: &mut PostZ<S>, z: Cx<S>| {
            if post.on {
                post.zprev = post.z;
                (post.z, post.solid) = Self::post_z(post_k, post_f, z);
            }
        };
        let o = if bulb {
            // The kernel's colorings start after the fractal's init (z = 0 here).
            push(&mut post, Cx::zero());
            Orbit { z: Cx::zero(), zprev: Cx::zero(), zprev2: Cx::zero(), iter: maxit, escaped: false }
        } else {
            let pairs = [(self.outside.as_ref(), f.outside.as_mut()), (self.inside.as_ref(), f.inside.as_mut())];
            let mut colorings: Vec<(&Compiled<S>, &mut Frame<S>)> = pairs.into_iter().filter_map(|(k, fr)| Some((k?, fr?))).collect();
            for (_, fr) in colorings.iter_mut() {
                setup(fr);
            }
            let hook = |h: Hook<S>| match h {
                Hook::Init(z) => {
                    push(&mut post, z);
                    let z = if post.on { post.z } else { z };
                    for (k, fr) in colorings.iter_mut() {
                        fr.zcur = z;
                        k.color_init(fr, maxit);
                    }
                }
                Hook::Iter(it, z, zp) => {
                    push(&mut post, z);
                    let (z, zp) = if post.on { (post.z, post.zprev) } else { (z, zp) };
                    for (k, fr) in colorings.iter_mut() {
                        fr.zcur = z;
                        k.color_loop(fr, it, zp);
                    }
                }
            };
            match &self.pert {
                Some((pert, orbit, bla)) => {
                    self.fractal.run_perturbed(&mut f.fractal, maxit, Deep { pert, orbit, bla: bla.as_ref() }, d, dseed, hook)
                }
                None => self.fractal.run_with(&mut f.fractal, maxit, hook),
            }
        };
        // The step that escaped ran no hook.
        if o.escaped {
            push(&mut post, o.z);
        }
        Some((o, post))
    }
}

/// `z` and the `z` before it as the coloring formulas see them after the post-transformations
/// (`on`: the layer has some), and whether one of them made the last value solid.
#[derive(Clone, Copy)]
struct PostZ<S> {
    on: bool,
    z: Cx<S>,
    zprev: Cx<S>,
    solid: bool,
}

/// A layer compiled plain or with derivatives (distance estimation), or a density layer.
enum Kernel {
    Plain(LayerK<f64>),
    Dual(LayerK<Dual>),
    Density(Box<DensityK>),
    /// A 3D object layer (its plan holds everything).
    Object,
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
        if lp.object.is_some() {
            return Kernel::Object;
        }
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
        // Deeper than double precision: perturbation when the formula allows it (with
        // derivatives for distance estimation).
        if plan.precision() == Precision::Beyond
            && lp.transforms.is_empty()
            && let Ok(pert) = Perturbation::new(&lp.fractal.program, &lp.fractal.values, !lp.julia, lp.seed)
        {
            fn deep<S: Scalar + Send + Sync + 'static>(lp: &LayerPlan, plan: &Plan, pert: Perturbation) -> LayerK<S> {
                let mut k = LayerK::<S>::new(lp);
                let bits = crate::math::big::bits_for(plan.frame.pixel_size_fx());
                let orbit = k.fractal.reference_orbit(&pert, &plan.center_text, bits, lp.max_iter);
                let bla = if lp.every_iteration() {
                    None
                } else {
                    pert.bla_table(&orbit, plan.frame.max_offset_fx(), crate::formula::perturb::BLA_EPS_CPU)
                };
                k.pert = Some((pert, orbit, bla));
                k
            }
            return if lp.needs_derivative() { Kernel::Dual(deep(lp, plan, pert)) } else { Kernel::Plain(deep(lp, plan, pert)) };
        }
        if lp.needs_derivative() { Kernel::Dual(LayerK::new(lp)) } else { Kernel::Plain(LayerK::new(lp)) }
    }
    fn frames(&self) -> KernelF {
        match self {
            Kernel::Plain(k) => KernelF::Plain(Box::new(k.frames())),
            Kernel::Dual(k) => KernelF::Dual(Box::new(k.frames())),
            Kernel::Density(_) | Kernel::Object => KernelF::None,
        }
    }
    /// The layer's color at plane point `p` and its height (lit layers); `turn` is the
    /// derivative's direction (a turn). `at`: the sample's plane point and its offset from the
    /// view center (deep zooms).
    fn eval(&self, f: &mut KernelF, lp: &LayerPlan, plan: &Plan, at: (C64, Cfx), turn: f64, pix: (u32, u32, u32)) -> ([f32; 4], [f32; 2], bool) {
        let (p, d) = at;
        if let Kernel::Density(k) = self {
            return (lp.density.as_ref().map_or([0.0; 4], |dn| density::color(lp, dn, &k.hist, pix.0, pix.1)), [0.0, 3.0e38], false);
        }
        if let (Kernel::Object, Some(o)) = (self, &lp.object) {
            // The camera ray through the sample position (the same as the plane point's).
            let (u, v, _) = sample_offset(pix.0, pix.1, pix.2);
            let (x, y) = (pix.0 as f32 + u as f32, pix.1 as f32 + v as f32);
            let rng = super::hash(pix.0 ^ super::hash(pix.1 ^ super::hash(pix.2 ^ 0x3C6E_F372)));
            return (o.shade(plan.frame.width, plan.frame.height, x, y, |t| lp.outside.color(t), rng), [0.0, 3.0e38], false);
        }
        match (self, f) {
            (Kernel::Plain(k), KernelF::Plain(f)) => k.eval(f, lp, plan, p, (d, None), pix),
            (Kernel::Dual(k), KernelF::Dual(f)) => {
                // Derivative along a direction that turns from sample to sample (non-analytic
                // formulas average over directions); |dz| is per pixel.
                let (s, c) = (turn * std::f64::consts::TAU).sin_cos();
                let (p, seed) = dual_pixel(plan, p, c, s);
                k.eval(f, lp, plan, p, (d, Some(seed)), pix)
            }
            _ => unreachable!("kernel and frames differ"),
        }
    }
}

pub struct CpuRenderer {
    plan: Plan,
    /// One kernel per layer of `plan.layers()` (same order).
    kernels: Vec<Kernel>,
    /// Premultiplied linear RGBA means per pixel, row-major.
    accum: Vec<[f32; 4]>,
    /// Per pixel: samples, Σ s, Σ s², done (see [`super::adaptive_update`]).
    stats: Vec<[f32; 4]>,
    samples: u32,
    /// Passes of the density layers' orbits so far.
    density_passes: u32,
    /// Samples the render aims at (density layers restart the image while it fills).
    pub target_samples: u32,
    /// Lit layers: the fields of the current pass (same order as `kernels`).
    fields: Vec<Option<light::Fields>>,
    /// Fill blocks inside the set without computing them (see [`guess_tile`]); starts from
    /// [`set_guessing`].
    pub guessing: bool,
}

/// One sample's evaluation state: the layers are evaluated in `plan.layers()` order.
struct Ctx<'a> {
    kernels: &'a [Kernel],
    fields: &'a [Option<light::Fields>],
    frames: &'a mut [KernelF],
    next: usize,
    plan: &'a Plan,
    p: C64,
    /// The sample's offset from the view center (deep zooms).
    d: Cfx,
    turn: f64,
    /// The pixel in the whole image and the sample index.
    pix: (u32, u32, u32),
    /// Every layer evaluated so far stayed inside (see [`LayerK::eval`]).
    inside: bool,
}

impl Ctx<'_> {
    fn layer(&mut self, lp: &LayerPlan) -> [f32; 4] {
        let i = self.next;
        self.next += 1;
        if let (Some(field), Some(l)) = (&self.fields[i], &lp.light) {
            self.inside = false;
            return field.shade(l, self.pix.0, self.pix.1, self.pix.2);
        }
        let (c, _, inside) = self.kernels[i].eval(&mut self.frames[i], lp, self.plan, (self.p, self.d), self.turn, self.pix);
        self.inside &= inside;
        c
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
        let kernels: Vec<Kernel> = plan.layers().iter().map(|l| Kernel::new(l, &plan)).collect();
        let n = plan.window.pixels();
        let fields = kernels.iter().map(|_| None).collect();
        CpuRenderer {
            plan,
            kernels,
            accum: vec![[0.0; 4]; n],
            stats: vec![[0.0; 4]; n],
            samples: 0,
            density_passes: 0,
            target_samples: u32::MAX,
            fields,
            guessing: GUESSING.load(Ordering::Relaxed),
        }
    }

    pub fn plan(&self) -> &Plan {
        &self.plan
    }

    pub fn samples(&self) -> u32 {
        self.samples
    }

    /// Color (linear RGB, straight alpha) of the sample `s` of pixel (x, y) of the window, and
    /// whether every layer stayed inside there.
    fn sample(&self, frames: &mut [KernelF], x: u32, y: u32, s: u32) -> ([f32; 4], bool) {
        // Positions and random streams of the pixel in the whole image.
        let (ix, iy) = (x + self.plan.window.x, y + self.plan.window.y);
        let (u, v, turn) = sample_offset(ix, iy, s);
        let (px, py) = (ix as f64 + u, iy as f64 + v);
        let (p, d) = (self.plan.frame.plane(px, py), self.plan.frame.offset_fx(px, py));
        let mut ctx =
            Ctx { kernels: &self.kernels, fields: &self.fields, frames, next: 0, plan: &self.plan, p, d, turn, pix: (ix, iy, s), inside: true };
        let c = composite_nodes(&self.plan.nodes, self.plan.background, &mut ctx);
        (c, ctx.inside)
    }

    /// Guessing applies: switched on and no density or 3D object layers (their pixels are
    /// cheap, and isolated details would be lost).
    fn guesses(&self) -> bool {
        self.guessing && self.kernels.iter().all(|k| matches!(k, Kernel::Plain(_) | Kernel::Dual(_)))
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
                self.stats.iter_mut().for_each(|a| *a = [0.0; 4]);
                self.samples = 0;
            }
        }
    }

    /// Lit layers: their fields for this pass (see [`light::Grids`]), every cell at the pass's
    /// sub-pixel offset of pixel (0, 0) (a regular grid for the normals); a 3D terrain's field
    /// stays at the pixel centers (moving it would move the ground).
    fn light_pass(&mut self) {
        let s = self.samples;
        let (u, v, turn) = sample_offset(0, 0, s);
        let (w, h) = (self.plan.frame.width, self.plan.frame.height);
        let layers = self.plan.layers();
        let mut fields: Vec<Option<light::Fields>> = Vec::with_capacity(layers.len());
        for (k, lp) in self.kernels.iter().zip(&layers) {
            let Some(l) = lp.light.as_ref().filter(|_| !matches!(k, Kernel::Density(_))) else {
                fields.push(None);
                continue;
            };
            let (u, v, turn) = if l.camera.is_some() { (0.5, 0.5, 0.0) } else { (u, v, turn) };
            let plan = &self.plan;
            let proto = k.frames();
            let grids = l.grids(w, h, plan.window);
            // A terrain measures in its field's cells.
            let lh = if l.camera.is_some() { grids.far.height } else { h };
            let field = |g: light::Grid| {
                let mut data = vec![[0.0f32; 6]; g.cells()];
                data.par_chunks_mut(g.width as usize).enumerate().for_each_init(
                    || proto.clone(),
                    |f, (j, row)| {
                        for (i, out) in row.iter_mut().enumerate() {
                            let (ix, iy) = g.pixel(i as u32, j as u32);
                            let (px, py) = (ix as f64 + u, iy as f64 + v);
                            let at = (plan.frame.plane(px, py), plan.frame.offset_fx(px, py));
                            // Cells beyond the image's top or left: the same wrapped seeds as the GPU.
                            let (c, [hgt, de], _) = k.eval(f, lp, plan, at, turn, (ix as u32, iy as u32, s));
                            // Colors from 0 to 1 (the GPU keeps them in 16-bit fixed point).
                            let k = |v: f32| v.clamp(0.0, 1.0);
                            *out = [k(c[0]), k(c[1]), k(c[2]), k(c[3]), hgt, de];
                        }
                    },
                );
                light::Field::new(l, g, lh, data)
            };
            fields.push(Some(light::Fields {
                near: grids.near.map(&field),
                far: field(grids.far),
                image_height: lh,
                image: (w, h),
                cells: l.camera.is_some(),
                near_margin: l.near_margin,
            }));
        }
        self.fields = fields;
    }

    /// Adds one sample to every pixel.
    pub fn pass(&mut self) {
        self.density_pass();
        self.light_pass();
        let w = self.plan.window.width as usize;
        let s = self.samples;
        let proto: Vec<KernelF> = self.kernels.iter().map(|k| k.frames()).collect();
        let mut accum = std::mem::take(&mut self.accum);
        let mut stats = std::mem::take(&mut self.stats);
        let this = &*self;
        let tolerance = self.plan.adaptive;
        // The neighbours as they were before this pass.
        let before = if tolerance > 0.0 { stats.clone() } else { Vec::new() };
        let h = self.plan.window.height as usize;
        let around = |x: usize, y: usize| {
            if before.is_empty() {
                return [[0.0; 4]; 4];
            }
            let at = |dx: i64, dy: i64| {
                let (nx, ny) = ((x as i64 + dx).clamp(0, w as i64 - 1) as usize, (y as i64 + dy).clamp(0, h as i64 - 1) as usize);
                before[ny * w + nx]
            };
            [at(-1, 0), at(1, 0), at(0, -1), at(0, 1)]
        };
        let add = |acc: &mut [f32; 4], st: &mut [f32; 4], c: [f32; 4], x: usize, y: usize| {
            super::adaptive_update(acc, st, premultiply(c), tolerance, around(x, y));
        };
        if self.guesses() {
            // Guessing: tiles of GUESS_TILE pixels on the whole image's grid (a window guesses
            // like the whole image), each tile subdivided (see `guess_tile`).
            let win = self.plan.window;
            let (gx, gy) = (win.x as usize % GUESS_TILE, win.y as usize % GUESS_TILE);
            let mut bands = Vec::new();
            let (mut acc_rest, mut st_rest) = (&mut accum[..], &mut stats[..]);
            let mut y0 = 0;
            while y0 < h {
                let th = (GUESS_TILE - if y0 == 0 { gy } else { 0 }).min(h - y0);
                let (a, ar) = std::mem::take(&mut acc_rest).split_at_mut(th * w);
                let (b, br) = std::mem::take(&mut st_rest).split_at_mut(th * w);
                bands.push((y0, a, b));
                (acc_rest, st_rest) = (ar, br);
                y0 += th;
            }
            let starts: Vec<usize> = std::iter::once(0).chain((GUESS_TILE - gx..w).step_by(GUESS_TILE).filter(|&x| x > 0)).collect();
            bands.into_par_iter().for_each_init(
                || (proto.clone(), Vec::new()),
                |(frames, cells), (y0, acc_rows, st_rows)| {
                    let th = acc_rows.len() / w;
                    for (k, &x0) in starts.iter().enumerate() {
                        let tw = starts.get(k + 1).copied().unwrap_or(w) - x0;
                        let at = |lx: usize, ly: usize| ly * w + x0 + lx;
                        cells.clear();
                        for ly in 0..th {
                            cells.extend((0..tw).map(|lx| {
                                let i = at(lx, ly);
                                if st_rows[i][3] > 0.5 { Cell::Done(acc_rows[i]) } else { Cell::Todo }
                            }));
                        }
                        guess_tile(cells, tw, th, |lx, ly| this.sample(frames, (x0 + lx) as u32, (y0 + ly) as u32, s));
                        for ly in 0..th {
                            for lx in 0..tw {
                                if let Some(c) = cells[ly * tw + lx].color() {
                                    let i = at(lx, ly);
                                    add(&mut acc_rows[i], &mut st_rows[i], c, x0 + lx, y0 + ly);
                                }
                            }
                        }
                    }
                },
            );
        } else {
            accum.par_chunks_mut(w).zip(stats.par_chunks_mut(w)).enumerate().for_each_init(
                || proto.clone(),
                |frames, (y, (row, st_row))| {
                    for (x, (acc, st)) in row.iter_mut().zip(st_row.iter_mut()).enumerate() {
                        // Adaptive sampling: done pixels take no more samples.
                        if st[3] > 0.5 {
                            continue;
                        }
                        let (c, _) = this.sample(frames, x as u32, y as u32, s);
                        add(acc, st, c, x, y);
                    }
                },
            );
        }
        self.accum = accum;
        self.stats = stats;
        self.samples += 1;
    }

    /// Averaged linear RGB with straight alpha per pixel.
    pub fn linear(&self) -> Vec<[f32; 4]> {
        self.accum.iter().map(|a| unpremultiply(*a, 1.0)).collect()
    }

    /// Adaptive sampling: the share of the pixels that are done.
    pub fn done_share(&self) -> f64 {
        self.stats.iter().filter(|s| s[3] > 0.5).count() as f64 / self.stats.len().max(1) as f64
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
        Some(self.kernels[0].eval(&mut f, lp, &self.plan, (p, d), 0.0, (0, 0, 0)).0)
    }
}

/// Straight alpha → premultiplied.
fn premultiply(c: [f32; 4]) -> [f32; 4] {
    [c[0] * c[3], c[1] * c[3], c[2] * c[3], c[3]]
}

// ---- guessing ----

/// Whether new CPU renderers guess (on by default).
static GUESSING: AtomicBool = AtomicBool::new(true);

/// Switches guessing on or off for the CPU renderers made from now on (`rfr-cli --no-guess`).
pub fn set_guessing(on: bool) {
    GUESSING.store(on, Ordering::Relaxed);
}

/// Periodicity tolerance floor of f64 orbits (see [`super::period_tolerance`]).
const PERIOD_FLOOR: f64 = 3.6e-15;

/// Tile size of guessing in pixels.
const GUESS_TILE: usize = 16;

/// Layers with fewer iterations never count as inside for guessing: they are cheap, and
/// patterns such as the carpet or image layers have details inside solid areas.
const GUESS_MIN_ITER: u32 = 32;

/// A pixel of a guessing tile.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Cell {
    Todo,
    /// Adaptive sampling is done with it; its mean (premultiplied).
    Done([f32; 4]),
    /// Computed: the color and whether every layer stayed inside.
    Sampled([f32; 4], bool),
    /// Filled from the border of its block.
    Guessed([f32; 4]),
}

impl Cell {
    /// The new sample's color (none for done pixels).
    fn color(self) -> Option<[f32; 4]> {
        match self {
            Cell::Sampled(c, _) | Cell::Guessed(c) => Some(c),
            Cell::Todo | Cell::Done(_) => None,
        }
    }
}

/// Solid guessing of one `w` × `h` tile: a block whose border pixels all stayed inside with
/// exactly the same color is filled with it; other blocks are cut into four (sharing their
/// middle lines) down to single pixels. Inside regions of a set without holes (the usual
/// escape-time sets) are exact; others may lose details smaller than a tile, so guessing can
/// be switched off. `cells` (row-major) comes with the done pixels marked and ends with every
/// pixel's result; `sample(x, y)` computes a pixel.
fn guess_tile(cells: &mut [Cell], w: usize, h: usize, mut sample: impl FnMut(usize, usize) -> ([f32; 4], bool)) {
    let mut get = |cells: &mut [Cell], x: usize, y: usize| {
        let c = &mut cells[y * w + x];
        if *c == Cell::Todo {
            let (color, inside) = sample(x, y);
            *c = Cell::Sampled(color, inside);
        }
        *c
    };
    let mut blocks = vec![(0, 0, w - 1, h - 1)];
    while let Some((x0, y0, x1, y1)) = blocks.pop() {
        // The border, and whether it is one inside color (done pixels must agree by their mean).
        let mut color: Option<[f32; 4]> = None;
        let mut uniform = true;
        let border = (x0..=x1).flat_map(|x| [(x, y0), (x, y1)]).chain((y0 + 1..y1).flat_map(|y| [(x0, y), (x1, y)]));
        let mut done = Vec::new();
        for (x, y) in border {
            match get(cells, x, y) {
                Cell::Sampled(c, true) => match color {
                    None => color = Some(c),
                    Some(k) => uniform &= k.map(f32::to_bits) == c.map(f32::to_bits),
                },
                Cell::Done(mean) => done.push(mean),
                _ => uniform = false,
            }
        }
        if x1 - x0 < 2 || y1 - y0 < 2 {
            continue;
        }
        if let (true, Some(c)) = (uniform, color)
            && done.iter().all(|m| m.map(f32::to_bits) == premultiply(c).map(f32::to_bits))
        {
            for y in y0 + 1..y1 {
                for x in x0 + 1..x1 {
                    let cell = &mut cells[y * w + x];
                    if *cell == Cell::Todo {
                        *cell = Cell::Guessed(c);
                    }
                }
            }
            continue;
        }
        let (mx, my) = ((x0 + x1) / 2, (y0 + y1) / 2);
        blocks.extend([(x0, y0, mx, my), (mx, y0, x1, my), (x0, my, mx, y1), (mx, my, x1, y1)]);
    }
}

/// The height value (the lit field's, before scaling) of the plan's first layer at the view
/// center, as a 3D terrain's camera aims at it; none for layers without lighting.
pub fn center_height(plan: &Plan) -> Option<f32> {
    let (x, y) = (plan.frame.width as f64 * 0.5, plan.frame.height as f64 * 0.5);
    heights_at(plan, &[(x, y)]).first().copied()
}

/// The height values of the plan's first (lit) layer at image positions `pts` (pixels, y
/// down); empty for layers without lighting.
pub fn heights_at(plan: &Plan, pts: &[(f64, f64)]) -> Vec<f32> {
    let layers = plan.layers();
    let Some(lp) = layers.first().filter(|lp| lp.light.is_some()) else { return Vec::new() };
    let k = Kernel::new(lp, plan);
    let mut f = k.frames();
    pts.iter().map(|&(x, y)| k.eval(&mut f, lp, plan, (plan.frame.plane(x, y), plan.frame.offset_fx(x, y)), 0.0, (0, 0, 0)).1[0]).collect()
}

/// A plane point with its derivative along direction (`c`, `s`) per pixel, and that
/// derivative with an extended exponent (perturbation, deeper than `f64` reaches).
fn dual_pixel(plan: &Plan, p: C64, c: f64, s: f64) -> (Cx<Dual>, Cfx) {
    let px = plan.frame.pixel_size();
    let seed = Cfx::from_c64(c, s) * Cfx::from_fx(plan.frame.pixel_size_fx(), crate::math::fx::Fx::ZERO);
    (Cx::new(Dual::new(p.re, c * px), Dual::new(p.im, s * px)), seed)
}

/// Coloring values (before the transfer) of the plan's first layer at the pixel centers,
/// row-major, with whether the orbit stayed inside; none for pixels colored without a value
/// (solid colors, direct formula colors). Density and 3D object layers give nothing.
pub fn coloring_values(plan: &Plan) -> Vec<Option<(bool, f64)>> {
    let layers = plan.layers();
    let Some(lp) = layers.first() else { return Vec::new() };
    fn run<S: Scalar + Send + Sync + 'static>(
        k: &LayerK<S>,
        lp: &LayerPlan,
        plan: &Plan,
        lift: impl Fn(C64) -> (Cx<S>, Option<Cfx>) + Sync,
    ) -> Vec<Option<(bool, f64)>> {
        let (w, h) = (plan.frame.width as usize, plan.frame.height as usize);
        let mut out = vec![None; w * h];
        out.par_chunks_mut(w).enumerate().for_each_init(
            || k.frames(),
            |f, (y, row)| {
                for (x, out) in row.iter_mut().enumerate() {
                    let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
                    let (p, d) = (plan.frame.plane(px, py), plan.frame.offset_fx(px, py));
                    let (p, seed) = lift(p);
                    *out = k.orbit(f, lp, plan, p, (d, seed), (x as u32, y as u32, 0)).and_then(|(o, post)| {
                        let r = LayerK::<S>::result(&o);
                        k.shade(f, lp, &o, &post, &r).1.filter(|v| v.is_finite()).map(|v| (!o.escaped, v))
                    });
                }
            },
        );
        out
    }
    match Kernel::new(lp, plan) {
        Kernel::Plain(k) => run(&k, lp, plan, |p| (p, None)),
        Kernel::Dual(k) => run(&k, lp, plan, |p| {
            let (p, seed) = dual_pixel(plan, p, 1.0, 0.0);
            (p, Some(seed))
        }),
        Kernel::Density(_) | Kernel::Object => Vec::new(),
    }
}

/// Iteration counts of the plan's first layer at the pixel centers, row-major: the iterations
/// of escaped orbits, none for orbits that stay inside (or pixels a transformation makes
/// solid). Density and 3D object layers give nothing.
pub fn iteration_counts(plan: &Plan) -> Vec<Option<u32>> {
    let layers = plan.layers();
    let Some(lp) = layers.first() else { return Vec::new() };
    fn run<S: Scalar + Send + Sync + 'static>(
        k: &LayerK<S>,
        lp: &LayerPlan,
        plan: &Plan,
        lift: impl Fn(C64) -> (Cx<S>, Option<Cfx>) + Sync,
    ) -> Vec<Option<u32>> {
        let (w, h) = (plan.frame.width as usize, plan.frame.height as usize);
        let mut out = vec![None; w * h];
        out.par_chunks_mut(w).enumerate().for_each_init(
            || k.frames(),
            |f, (y, row)| {
                for (x, o) in row.iter_mut().enumerate() {
                    let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
                    let (p, d) = (plan.frame.plane(px, py), plan.frame.offset_fx(px, py));
                    let (p, seed) = lift(p);
                    *o = k.orbit(f, lp, plan, p, (d, seed), (x as u32, y as u32, 0)).filter(|(o, _)| o.escaped).map(|(o, _)| o.iter);
                }
            },
        );
        out
    }
    match Kernel::new(lp, plan) {
        Kernel::Plain(k) => run(&k, lp, plan, |p| (p, None)),
        Kernel::Dual(k) => run(&k, lp, plan, |p| {
            let (p, seed) = dual_pixel(plan, p, 1.0, 0.0);
            (p, Some(seed))
        }),
        Kernel::Density(_) | Kernel::Object => Vec::new(),
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
    fn post_transformations_change_only_coloring_formulas() {
        let mut doc = Document::default();
        doc.layers[0].outside_method = OutsideMethod::Formula;
        doc.layers[0].outside.formula = "rfract-coloring:StripeAverage".into();
        let plain = render(&doc, 48, 27, 1).unwrap().rgba8();
        let mut post = doc.clone();
        post.layers[0].post_transforms.push(TransformRef { formula: "rfract-transform:Twirl".into(), ..TransformRef::default() });
        let changed = render(&post, 48, 27, 1).unwrap().rgba8();
        assert_ne!(plain, changed, "the coloring sees the transformed z");
        // The set stays the same: inside pixels are where they were.
        let inside = |img: &[u8]| img.chunks(4).map(|p| p[..3] == [0, 0, 0]).collect::<Vec<_>>();
        assert_eq!(inside(&plain), inside(&changed));
        // Native colorings ignore them (and the plan drops them).
        let mut native = post.clone();
        native.layers[0].outside_method = OutsideMethod::Smooth;
        assert!(LayerPlan::new(&native.layers[0]).unwrap().post_transforms.is_empty());
        doc.layers[0].outside_method = OutsideMethod::Smooth;
        assert_eq!(render(&doc, 48, 27, 1).unwrap().rgba8(), render(&native, 48, 27, 1).unwrap().rgba8());
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

    #[test]
    fn guessing_fills_inside_blocks_exactly() {
        // A disc inside (one color) in a varying outside: the same result with fewer samples.
        let (w, h) = (16, 13);
        let f = |x: usize, y: usize| {
            let (dx, dy) = (x as f32 - 7.0, y as f32 - 6.0);
            if dx * dx + dy * dy < 30.0 { ([0.0, 0.0, 0.0, 1.0], true) } else { ([x as f32 / 16.0, y as f32 / 16.0, 0.5, 1.0], false) }
        };
        let mut cells = vec![Cell::Todo; w * h];
        cells[0] = Cell::Done([0.25; 4]);
        let mut n = 0;
        guess_tile(&mut cells, w, h, |x, y| {
            n += 1;
            f(x, y)
        });
        assert!(n < w * h - 20, "{n} samples");
        assert_eq!(cells[0], Cell::Done([0.25; 4]), "done pixels stay done");
        for (i, c) in cells.iter().enumerate().skip(1) {
            assert_eq!(c.color(), Some(f(i % w, i / w).0), "pixel {i}");
        }
        assert!(cells.iter().any(|c| matches!(c, Cell::Guessed(_))));
    }

    #[test]
    fn guessing_keeps_the_image() {
        let mut doc = Document::default();
        doc.image.adaptive = 0.0;
        let (w, h) = (96, 54);
        let image = |guessing: bool| {
            let mut r = CpuRenderer::new(Plan::sized(&doc, w, h).unwrap());
            r.guessing = guessing;
            assert!(r.guesses() == guessing);
            r.pass();
            r.pass();
            r.rgba8()
        };
        assert_eq!(image(true), image(false));
    }

    #[test]
    fn early_inside_keeps_the_image() {
        // Mandelbrot (bulbs and periodicity) whole and near the seahorse valley; a Julia set
        // (periodicity); a magnitude inside (no shortcuts).
        let mut docs = vec![Document::default()];
        let mut d = Document::default();
        d.view.center = ["-0.7453".into(), "0.1127".into()];
        d.view.radius = 0.0065.into();
        docs.push(d);
        let mut d = Document::default();
        d.layers[0].julia = true;
        d.layers[0].seed = [-0.123, 0.745];
        d.view.center = ["0".into(), "0".into()];
        docs.push(d);
        let mut d = Document::default();
        d.layers[0].inside_method = InsideMethod::Magnitude;
        docs.push(d);
        for (k, doc) in docs.iter().enumerate() {
            let plan = Plan::sized(doc, 64, 36).unwrap();
            let early = plan.layers()[0].early;
            assert_eq!(early.periodicity, k < 3, "doc {k}");
            assert_eq!(early.bulbs, k < 2, "doc {k}");
            let mut off = plan.clone();
            for n in off.nodes.iter_mut() {
                if let Node::Layer { layer, .. } = n {
                    layer.early = Default::default();
                }
            }
            let image = |p: Plan| {
                let mut r = CpuRenderer::new(p);
                r.guessing = false;
                r.pass();
                r.rgba8()
            };
            assert_eq!(image(plan), image(off), "doc {k}");
        }
    }

    #[test]
    fn document_lights_light_lit_layers() {
        // No directional lights and no ambient light: dark unless the document's light reaches.
        let mut doc = Document::default();
        doc.image.adaptive = 0.0;
        let l = &mut doc.layers[0].light;
        l.enabled = true;
        l.ambient = 0.0;
        l.lights.clear();
        doc.lights.push(crate::doc::LightSource { position: ["-1.5".into(), "0.8".into()], height: 0.4, visible: false, ..Default::default() });
        let mean = |d: &Document| {
            let img = render(d, 64, 36, 1).unwrap().rgba8();
            img.chunks(4).map(|p| p[0] as f64 + p[1] as f64 + p[2] as f64).sum::<f64>() / (img.len() / 4) as f64
        };
        let off = mean(&doc);
        doc.layers[0].light.scene_lights = true;
        let on = mean(&doc);
        assert!(off < 1.0 && on > 5.0, "off {off}, on {on}");
        // A 3D object: the light in front of it, between the camera and the bulb.
        let mut obj = doc.clone();
        obj.layers[0].kind = crate::doc::LayerKind::Object;
        obj.layers[0].object.camera.distance = 2.8;
        obj.lights[0].position = ["-0.5".into(), "-0.6".into()];
        obj.lights[0].height = 0.8;
        let on = mean(&obj);
        obj.layers[0].light.scene_lights = false;
        let off = mean(&obj);
        assert!(off < 1.0 && on > 5.0, "object: off {off}, on {on}");
    }

    #[test]
    fn adaptive_sampling_finishes_flat_pixels_early() {
        let mut doc = Document::for_formula("rfract:Mandelbrot");
        doc.image.adaptive = 0.0;
        let (w, h, n) = (64u32, 36u32, 48u32);
        let mut full = CpuRenderer::new(crate::render::Plan::sized(&doc, w, h).unwrap());
        let mut adaptive = CpuRenderer::new(crate::render::Plan::sized(&doc, w, h).unwrap().with_adaptive(0.005));
        for _ in 0..n {
            full.pass();
            adaptive.pass();
        }
        assert_eq!(full.done_share(), 0.0);
        let done = adaptive.done_share();
        assert!(done > 0.3, "only {done} done");
        let (a, b) = (full.rgba8(), adaptive.rgba8());
        let diff = a.iter().zip(&b).map(|(x, y)| x.abs_diff(*y) as f64).sum::<f64>() / a.len() as f64;
        assert!(diff < 1.0, "{diff}");
    }
}
