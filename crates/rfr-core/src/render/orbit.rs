// SPDX-License-Identifier: LGPL-3.0-or-later

//! The orbit of one point of a layer, for the viewer's orbit display: the layer's
//! transformations, then the values of `z` the fractal goes through (in f64, as the CPU
//! renderer computes them, without the early inside tests).

use super::LayerPlan;
use crate::doc::{Document, Layer};
use crate::formula::cpu::{Compiled, Hook};
use crate::math::complex::Cx;

/// Values of `z` kept for the period search at the end.
const PERIOD_WINDOW: usize = 4096;

/// A layer compiled for tracing orbits.
pub struct Tracer {
    transforms: Vec<Compiled<f64>>,
    fractal: Compiled<f64>,
    julia: bool,
    seed: [f64; 2],
    max_iter: u32,
    center: [f64; 2],
    magn: f64,
}

/// The orbit of one point.
#[derive(Clone, Debug, Default)]
pub struct Trace {
    /// The point after the transformations, then `z` after the fractal's init and after every
    /// iteration (at most the requested number of points).
    pub points: Vec<[f64; 2]>,
    /// Completed iterations.
    pub iter: u32,
    pub escaped: bool,
    /// A transformation left the point transparent (no orbit).
    pub solid: bool,
    /// Orbits that stay inside: the length of the cycle they settle in, when they settle
    /// within the iteration limit (up to 4096).
    pub period: Option<u32>,
}

impl Tracer {
    /// The tracer of `layer` in the view of `doc` (fractal layers only).
    pub fn new(doc: &Document, layer: &Layer) -> Result<Tracer, String> {
        let lp = LayerPlan::new(layer)?;
        if lp.density.is_some() || lp.object.is_some() {
            return Err("orbits are shown for fractal layers".into());
        }
        Ok(Tracer {
            transforms: lp.transforms.iter().map(|t| Compiled::new(&t.program, &t.values)).collect(),
            fractal: Compiled::new(&lp.fractal.program, &lp.fractal.values),
            julia: lp.julia,
            seed: lp.seed,
            max_iter: lp.max_iter,
            center: doc.view.center_f64(),
            magn: 1.5 / doc.view.radius.to_f64(),
        })
    }

    /// The orbit of plane point `p`, keeping at most `max_points` points.
    pub fn trace(&self, p: [f64; 2], max_points: usize) -> Trace {
        let mut pixel = Cx::<f64>::from_f64(p[0], p[1]);
        for t in &self.transforms {
            let mut f = t.frame();
            let (q, solid) = t.transform(&mut f, pixel);
            if solid {
                return Trace { solid: true, ..Trace::default() };
            }
            pixel = q;
        }
        let mut f = self.fractal.frame();
        f.pixel = pixel;
        f.c = if self.julia { Cx::from_f64(self.seed[0], self.seed[1]) } else { pixel };
        f.ismand = !self.julia;
        f.center = Cx::from_f64(self.center[0], self.center[1]);
        f.magn = self.magn;
        let mut points = vec![[pixel.re, pixel.im]];
        let mut recent = std::collections::VecDeque::with_capacity(PERIOD_WINDOW);
        let mut keep = |z: Cx<f64>| {
            if points.len() < max_points {
                points.push([z.re, z.im]);
            }
            if recent.len() == PERIOD_WINDOW {
                recent.pop_front();
            }
            recent.push_back([z.re, z.im]);
        };
        let o = self.fractal.run_with(&mut f, self.max_iter, |h| match h {
            Hook::Init(z) => keep(z),
            Hook::Iter(_, z, _) => keep(z),
        });
        if o.escaped {
            keep(o.z);
        }
        let period = (!o.escaped).then(|| period(recent.make_contiguous())).flatten();
        Trace { points, iter: o.iter, escaped: o.escaped, solid: false, period }
    }
}

/// The smallest `p` for which the last value of `z` comes back after `p` steps, within a
/// tolerance relative to its size, checked over two cycles.
fn period(z: &[[f64; 2]]) -> Option<u32> {
    let n = z.len();
    let last = *z.last()?;
    let tol = 1e-9 * (last[0].hypot(last[1])).max(1.0);
    let near = |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).hypot(a[1] - b[1]) < tol;
    (1..n / 2).find(|&p| near(z[n - 1 - p], last) && near(z[n - 1 - 2 * p], last)).map(|p| p as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orbits_of_the_mandelbrot_set() {
        let doc = Document::default();
        let t = Tracer::new(&doc, &doc.layers[0]).unwrap();
        // Escapes: c = 1 goes 0, 1, 2, 5, 26, …
        let o = t.trace([1.0, 0.0], 100);
        assert!(o.escaped);
        assert_eq!(&o.points[..5], &[[1.0, 0.0], [0.0, 0.0], [1.0, 0.0], [2.0, 0.0], [5.0, 0.0]]);
        // The main cardioid has period 1, the bulb at -1 period 2, the one at -0.122 + 0.745i
        // period 3.
        for (c, p) in [([-0.1, 0.1], 1), ([-1.0, 0.05], 2), ([-0.122, 0.745], 3)] {
            let o = t.trace(c, 50);
            assert!(!o.escaped, "{c:?}");
            assert_eq!(o.points.len(), 50);
            assert_eq!(o.period, Some(p), "{c:?}");
        }
    }

    #[test]
    fn transformations_run_first() {
        let mut doc = Document::default();
        doc.layers[0].transforms.push(crate::doc::TransformRef { formula: "rfract-transform:Mirror".into(), ..Default::default() });
        let t = Tracer::new(&doc, &doc.layers[0]).unwrap();
        // The default mirror line is the real axis: both points start at the same place.
        let (a, b) = (t.trace([0.3, 0.2], 10), t.trace([0.3, -0.2], 10));
        assert!(!a.solid);
        assert_eq!(a.points, b.points);
        assert_eq!(a.points[0][0], 0.3);
    }
}
