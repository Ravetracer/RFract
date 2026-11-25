// SPDX-License-Identifier: LGPL-3.0-or-later

//! CPU reference renderer: one sample per pixel per pass, accumulated in linear RGB. The GPU
//! renderer follows it step by step (same sample positions, same coloring); its output is the
//! reference for the parity tests and the fallback where the GPU lacks precision.

use rayon::prelude::*;

use super::{OrbitResult, Plan, sample_offset, shade};
use crate::formula::cpu::{Compiled, Frame};
use crate::gradient::linear_to_srgb;
use crate::math::complex::{C64, Cx};
use crate::math::scalar::{Dual, Scalar};

enum Kernel {
    Plain(Compiled<f64>),
    Dual(Compiled<Dual>),
}

pub struct CpuRenderer {
    plan: Plan,
    kernel: Kernel,
    /// Linear RGB sums per pixel, row-major.
    accum: Vec<[f32; 3]>,
    samples: u32,
}

/// Sets up a frame for one sample and runs the orbit.
fn run_sample<S: Scalar + Send + Sync + 'static>(k: &Compiled<S>, f: &mut Frame<S>, plan: &Plan, pixel: Cx<S>) -> crate::formula::cpu::Orbit<S> {
    f.pixel = pixel;
    f.ismand = !plan.julia;
    f.c = if plan.julia { Cx::from_f64(plan.seed[0], plan.seed[1]) } else { pixel };
    f.center = Cx::from_f64(plan.frame.center[0], plan.frame.center[1]);
    f.magn = S::from_f64(plan.magn());
    k.run(f, plan.max_iter)
}

/// Linear RGB of the sample at image position (px, py) with derivative direction `turn`.
fn sample(kernel: &Kernel, frames: &mut (Frame<f64>, Frame<Dual>), plan: &Plan, px: f64, py: f64, turn: f64) -> [f32; 3] {
    let p = plan.frame.plane(px, py);
    let o = match kernel {
        Kernel::Plain(k) => {
            let o = run_sample(k, &mut frames.0, plan, p);
            OrbitResult { z: o.z, zprev: o.zprev, iter: o.iter, escaped: o.escaped, dz: 0.0 }
        }
        Kernel::Dual(k) => {
            // Derivative along a direction that turns from sample to sample (non-analytic
            // formulas average over directions); |dz| is per plane unit.
            let (s, c) = (turn * std::f64::consts::TAU).sin_cos();
            let pixel = Cx::new(Dual::new(p.re, c), Dual::new(p.im, s));
            let o = run_sample(k, &mut frames.1, plan, pixel);
            let dz = (o.z.re.d * o.z.re.d + o.z.im.d * o.z.im.d).sqrt();
            OrbitResult { z: o.z.v(), zprev: o.zprev.v(), iter: o.iter, escaped: o.escaped, dz }
        }
    };
    shade(plan, &o)
}

impl CpuRenderer {
    pub fn new(plan: Plan) -> CpuRenderer {
        let kernel = if plan.needs_derivative() {
            Kernel::Dual(Compiled::new(&plan.program, &plan.values))
        } else {
            Kernel::Plain(Compiled::new(&plan.program, &plan.values))
        };
        let n = (plan.frame.width * plan.frame.height) as usize;
        CpuRenderer { plan, kernel, accum: vec![[0.0; 3]; n], samples: 0 }
    }

    pub fn plan(&self) -> &Plan {
        &self.plan
    }

    pub fn samples(&self) -> u32 {
        self.samples
    }

    fn frames(&self) -> (Frame<f64>, Frame<Dual>) {
        let empty = || Compiled::<f64>::new(&self.plan.program, &self.plan.values).frame();
        match &self.kernel {
            Kernel::Plain(k) => (k.frame(), Compiled::<Dual>::new(&self.plan.program, &self.plan.values).frame()),
            Kernel::Dual(k) => (empty(), k.frame()),
        }
    }

    /// Adds one sample to every pixel.
    pub fn pass(&mut self) {
        let w = self.plan.frame.width as usize;
        let s = self.samples;
        let plan = &self.plan;
        let kernel = &self.kernel;
        let proto = self.frames();
        self.accum.par_chunks_mut(w).enumerate().for_each_init(
            || proto.clone(),
            |frames, (y, row)| {
                for (x, acc) in row.iter_mut().enumerate() {
                    let (u, v, turn) = sample_offset(x as u32, y as u32, s);
                    let c = sample(kernel, frames, plan, x as f64 + u, y as f64 + v, turn);
                    acc[0] += c[0];
                    acc[1] += c[1];
                    acc[2] += c[2];
                }
            },
        );
        self.samples += 1;
    }

    /// Averaged linear RGB per pixel.
    pub fn linear(&self) -> Vec<[f32; 3]> {
        let k = 1.0 / self.samples.max(1) as f32;
        self.accum.iter().map(|a| [a[0] * k, a[1] * k, a[2] * k]).collect()
    }

    /// The image as 8-bit sRGB RGBA.
    pub fn rgba8(&self) -> Vec<u8> {
        self.linear().iter().flat_map(|c| [linear_to_srgb(c[0]), linear_to_srgb(c[1]), linear_to_srgb(c[2]), 255]).collect()
    }

    /// Orbit of a single plane point (for the orbit display and tests).
    pub fn orbit_at(&self, p: C64) -> OrbitResult {
        let mut frames = self.frames();
        match &self.kernel {
            Kernel::Plain(k) => {
                let o = run_sample(k, &mut frames.0, &self.plan, p);
                OrbitResult { z: o.z, zprev: o.zprev, iter: o.iter, escaped: o.escaped, dz: 0.0 }
            }
            Kernel::Dual(k) => {
                let o = run_sample(k, &mut frames.1, &self.plan, Cx::new(Dual::new(p.re, 1.0), Dual::new(p.im, 0.0)));
                OrbitResult { z: o.z.v(), zprev: o.zprev.v(), iter: o.iter, escaped: o.escaped, dz: (o.z.re.d.powi(2) + o.z.im.d.powi(2)).sqrt() }
            }
        }
    }
}

/// Renders `samples` passes of the document's active layer at `width` × `height`.
pub fn render(doc: &crate::doc::Document, width: u32, height: u32, samples: u32) -> Result<CpuRenderer, String> {
    let mut r = CpuRenderer::new(Plan::sized(doc, width, height)?);
    for _ in 0..samples.max(1) {
        r.pass();
    }
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::{Document, OutsideMethod};

    #[test]
    fn renders_the_mandelbrot_set() {
        let doc = Document::default();
        let r = render(&doc, 64, 36, 2).unwrap();
        let img = r.rgba8();
        assert_eq!(img.len(), 64 * 36 * 4);
        // The center of the default view (-0.5, 0) is inside: solid black.
        let i = (18 * 64 + 32) * 4;
        assert_eq!(&img[i..i + 3], &[0, 0, 0]);
        // The corners are outside: colored.
        assert!(img[0..3].iter().any(|&v| v > 0));
    }

    #[test]
    fn distance_estimation_uses_derivatives() {
        let mut doc = Document::default();
        doc.layers[0].outside_method = OutsideMethod::Distance;
        let r = render(&doc, 48, 27, 1).unwrap();
        let o = r.orbit_at(C64::new(0.5, 0.5));
        assert!(o.escaped && o.dz > 0.0, "{o:?}");
    }
}
