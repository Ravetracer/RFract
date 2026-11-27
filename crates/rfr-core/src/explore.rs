// SPDX-License-Identifier: LGPL-3.0-or-later

//! Starting points for the explore lane: random documents and mutations of a document. Random
//! documents come from formulas that give good images, with a random palette, and are framed
//! by zooming step by step into the busiest parts of a small CPU preview. Adapted from
//! RMandelbulb3D (`explore.rs`).

use crate::doc::{Document, OutsideMethod, Transfer};
use crate::formula::ast::ParamValue;
use crate::palette::{Adjust, Harmony, Order, PaletteOptions};

/// Kinds of random documents.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RandomKind {
    Any,
    /// Escape-time fractals in their Mandelbrot mode.
    Escape,
    /// Julia sets of escape-time fractals.
    Julia,
    /// Root finding (Newton, Nova, magnets).
    Convergent,
    /// Escape-time fractals as lit reliefs.
    Relief,
}

impl RandomKind {
    pub const ALL: [RandomKind; 5] = [RandomKind::Any, RandomKind::Escape, RandomKind::Julia, RandomKind::Convergent, RandomKind::Relief];

    pub fn label(self) -> &'static str {
        match self {
            RandomKind::Any => "Any",
            RandomKind::Escape => "Escape time",
            RandomKind::Julia => "Julia sets",
            RandomKind::Convergent => "Root finding",
            RandomKind::Relief => "Lit reliefs",
        }
    }
}

/// What a mutation changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mutation {
    All,
    Parameters,
    Colors,
    View,
    Formula,
}

impl Mutation {
    pub const ALL: [Mutation; 5] = [Mutation::All, Mutation::Parameters, Mutation::Colors, Mutation::View, Mutation::Formula];

    pub fn label(self) -> &'static str {
        match self {
            Mutation::All => "Anything",
            Mutation::Parameters => "Formula parameters",
            Mutation::Colors => "Colors",
            Mutation::View => "View",
            Mutation::Formula => "Formula and Julia",
        }
    }
}

/// Small deterministic random stream (splitmix64).
pub struct Rand(u64);

impl Rand {
    pub fn new(seed: u64) -> Rand {
        Rand(seed ^ 0x9e37_79b9_7f4a_7c15)
    }

    pub fn unit(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^= z >> 31;
        (z >> 11) as f64 / (1u64 << 53) as f64
    }

    pub fn range(&mut self, a: f64, b: f64) -> f64 {
        a + (b - a) * self.unit()
    }

    pub fn chance(&mut self, p: f64) -> bool {
        self.unit() < p
    }

    pub fn pick<'a, T>(&mut self, list: &'a [T]) -> &'a T {
        &list[((self.unit() * list.len() as f64) as usize).min(list.len() - 1)]
    }

    pub fn seed(&mut self) -> u64 {
        (self.unit() * 9.0e15) as u64
    }
}

/// A formula that gives good images, with parameter ranges and how deep a random view may
/// zoom (steps of 3–8×).
struct Good {
    id: &'static str,
    params: &'static [(&'static str, f64, f64)],
    depth: usize,
}

const ESCAPE: &[Good] = &[
    Good { id: "rfract:Mandelbrot", params: &[], depth: 6 },
    Good { id: "rfract:Mandelbrot", params: &[], depth: 6 },
    Good { id: "rfract:Multibrot", params: &[("power", 2.5, 6.0)], depth: 4 },
    Good { id: "rfract:BurningShip", params: &[], depth: 5 },
    Good { id: "rfract:Tricorn", params: &[], depth: 4 },
    Good { id: "rfract:Celtic", params: &[], depth: 4 },
    Good { id: "rfract:Buffalo", params: &[], depth: 4 },
    Good { id: "rfract:Perpendicular", params: &[], depth: 4 },
    Good { id: "rfract:Phoenix", params: &[], depth: 3 },
    Good { id: "rfract:Spider", params: &[], depth: 4 },
    Good { id: "rfract:Manowar", params: &[], depth: 4 },
    Good { id: "rfract:Lambda", params: &[], depth: 4 },
];

const CONVERGENT: &[Good] = &[
    Good { id: "rfract:Newton", params: &[], depth: 3 },
    Good { id: "rfract:Nova", params: &[], depth: 4 },
    Good { id: "rfract:Magnet1", params: &[], depth: 4 },
    Good { id: "rfract:Magnet2", params: &[], depth: 3 },
    Good { id: "rfract:RootFinding", params: &[], depth: 3 },
];

/// Preview size used to frame random documents.
const PROBE_W: u32 = 48;
const PROBE_H: u32 = 27;

/// Brightness of an RGBA8 image per pixel (0..1).
fn luminance(rgba: &[u8]) -> Vec<f64> {
    rgba.as_chunks::<4>().0.iter().map(|p| (0.2126 * p[0] as f64 + 0.7152 * p[1] as f64 + 0.0722 * p[2] as f64) / 255.0).collect()
}

/// A small CPU preview of `doc` (RGBA8, effects not applied) in a neutral coloring: gray by
/// the logarithm of the smooth iteration count, the inside black. Its brightness changes
/// fastest near the fractal's edge, whatever the document's palette.
fn probe(doc: &Document) -> Option<Vec<u8>> {
    use crate::gradient::Stop;
    let mut d = doc.clone();
    for l in &mut d.layers {
        l.light.enabled = false;
        if l.outside_method != OutsideMethod::Formula {
            l.outside_method = OutsideMethod::Smooth;
            l.outside.gradient.stops = vec![Stop { pos: 0.0, color: [0.05; 3] }, Stop { pos: 1.0, color: [1.0; 3] }];
            l.outside.gradient.alpha.clear();
            l.outside.transfer = Transfer::Log;
            l.outside.density = 0.14;
            l.outside.offset = 0.0;
        }
        if l.inside_method != crate::doc::InsideMethod::Formula {
            l.inside_method = crate::doc::InsideMethod::Solid;
            l.inside.solid = [0.0; 3];
        }
    }
    d.post = Default::default();
    probe_as_is(&d)
}

/// A small CPU preview of `doc` with its own colors (RGBA8, effects not applied).
fn probe_as_is(doc: &Document) -> Option<Vec<u8>> {
    let mut d = doc.clone();
    for l in &mut d.layers {
        l.max_iter = l.max_iter.min(600);
    }
    let r = crate::render::cpu::render(&d, PROBE_W, PROBE_H, 1).ok()?;
    Some(crate::render::cpu::rgba8(&r.linear()))
}

/// How busy every pixel of a probe is: the brightness differences to its neighbours.
fn busyness(lum: &[f64]) -> Vec<f64> {
    let (w, h) = (PROBE_W as usize, PROBE_H as usize);
    (0..w * h)
        .map(|i| {
            let (x, y) = (i % w, i / w);
            let mut s = 0.0;
            for (dx, dy) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
                let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                if nx >= 0 && ny >= 0 && (nx as usize) < w && (ny as usize) < h {
                    s += (lum[ny as usize * w + nx as usize] - lum[i]).abs();
                }
            }
            s
        })
        .collect()
}

/// Contrast of an image (standard deviation of the brightness, 0..0.5).
pub fn contrast(rgba: &[u8]) -> f64 {
    let l = luminance(rgba);
    if l.is_empty() {
        return 0.0;
    }
    let mean = l.iter().sum::<f64>() / l.len() as f64;
    (l.iter().map(|v| (v - mean) * (v - mean)).sum::<f64>() / l.len() as f64).sqrt()
}

/// Share of the pixels with detail (busy) in a probe image.
fn detail_share(rgba: &[u8]) -> f64 {
    let b = busyness(&luminance(rgba));
    b.iter().filter(|v| **v > 0.08).count() as f64 / b.len().max(1) as f64
}

/// Frames the view: zooms `steps` times by 3–8× into a random one of the busiest pixels of a
/// small preview, so the view shows the fractal's edge with detail. False when there is
/// nothing to see.
pub fn frame(doc: &mut Document, steps: usize, rnd: &mut Rand) -> bool {
    for _ in 0..steps {
        let Some(img) = probe(doc) else { return false };
        let b = busyness(&luminance(&img));
        let mut order: Vec<usize> = (0..b.len()).collect();
        order.sort_by(|x, y| b[*y].total_cmp(&b[*x]));
        if b[order[0]] < 0.05 {
            break;
        }
        // One of the busiest tenth, away from the border.
        let top: Vec<usize> = order
            .iter()
            .take(b.len() / 10)
            .copied()
            .filter(|i| {
                let (x, y) = (i % PROBE_W as usize, i / PROBE_W as usize);
                x > 1 && y > 1 && x + 2 < PROBE_W as usize && y + 2 < PROBE_H as usize
            })
            .collect();
        if top.is_empty() {
            break;
        }
        let i = *rnd.pick(&top);
        let (x, y) = ((i % PROBE_W as usize) as f64 + rnd.unit(), (i / PROBE_W as usize) as f64 + rnd.unit());
        // Offset from the center in radii (half the view height), rotation applied.
        let s = 2.0 / PROBE_H as f64;
        let (dx, dy) = ((x - PROBE_W as f64 / 2.0) * s, -(y - PROBE_H as f64 / 2.0) * s);
        let (sin, cos) = doc.view.rotation.to_radians().sin_cos();
        doc.view.pan([dx * cos - dy * sin, dx * sin + dy * cos]);
        doc.view.radius *= 1.0 / rnd.range(3.0, 8.0);
        // Deeper views need more iterations.
        for l in &mut doc.layers {
            l.max_iter = ((l.max_iter as f64 * 1.3) as u32).min(5000);
        }
    }
    // Detail, but not everywhere (that is noise finer than the pixels).
    probe(doc).is_some_and(|img| contrast(&img) > 0.06 && (0.04..0.7).contains(&detail_share(&img)))
}

/// The document's own colors work: contrast, not noise.
fn looks_good(doc: &Document) -> bool {
    probe_as_is(doc).is_some_and(|img| contrast(&img) > 0.07 && detail_share(&img) < 0.75)
}

/// New colors: a generated palette for the outside coloring, its transfer, density and
/// offset.
pub fn randomize_colors(doc: &mut Document, rnd: &mut Rand) {
    let opts = PaletteOptions {
        colors: 3 + (rnd.unit() * 5.0) as usize,
        harmony: *rnd.pick(&Harmony::ALL),
        random_hue: true,
        hue_jitter: rnd.range(0.0, 25.0),
        order: *rnd.pick(&[Order::Alternate, Order::Harmony, Order::Lightness]),
        hardness: if rnd.chance(0.15) { rnd.range(0.5, 1.0) } else { 0.0 },
        lightness: [rnd.range(0.05, 0.3), rnd.range(0.7, 0.95)],
        cyclic: true,
        seed: rnd.seed() | 1,
        ..PaletteOptions::default()
    };
    let stops = crate::palette::generate(&opts);
    for l in &mut doc.layers {
        l.outside.gradient.stops = stops.clone();
        l.outside.gradient.alpha.clear();
        if l.outside_method != OutsideMethod::Formula {
            l.outside_method = if rnd.chance(0.2) { OutsideMethod::Distance } else { OutsideMethod::Smooth };
        }
        l.outside.transfer = *rnd.pick(&[Transfer::Linear, Transfer::Sqrt, Transfer::Sqrt, Transfer::Cbrt, Transfer::Log]);
        l.outside.density = 2f64.powf(rnd.range(-3.0, 0.5));
        l.outside.offset = rnd.unit();
    }
}

/// Sets the formula parameters of `good` at random within their ranges.
fn set_params(doc: &mut Document, good: &Good, rnd: &mut Rand) {
    for (name, a, b) in good.params {
        doc.layers[0].params.insert((*name).to_string(), ParamValue::Float(rnd.range(*a, *b)));
    }
}

/// A random point near the edge of the formula's Mandelbrot-mode set (for Julia seeds).
fn edge_point(id: &str, rnd: &mut Rand) -> [f64; 2] {
    let mut d = Document::for_formula(id);
    d.layers[0].julia = false;
    frame(&mut d, 1, rnd);
    d.view.center_f64()
}

/// A random document of `kind` (not yet framed).
fn random_doc(kind: RandomKind, rnd: &mut Rand) -> (Document, usize) {
    let kind = if kind == RandomKind::Any {
        *rnd.pick(&[RandomKind::Escape, RandomKind::Escape, RandomKind::Julia, RandomKind::Julia, RandomKind::Convergent, RandomKind::Relief])
    } else {
        kind
    };
    let good = rnd.pick(if kind == RandomKind::Convergent { CONVERGENT } else { ESCAPE });
    let mut d = Document::for_formula(good.id);
    d.image.samples = d.image.samples.min(64);
    set_params(&mut d, good, rnd);
    let mut depth = good.depth;
    if kind == RandomKind::Julia {
        d.layers[0].seed = edge_point(good.id, rnd);
        d.layers[0].julia = true;
        d.view = crate::doc::View { center: ["0".into(), "0".into()], radius: 1.6.into(), rotation: 0.0 };
        depth = 3;
    }
    randomize_colors(&mut d, rnd);
    if kind == RandomKind::Relief {
        let light = &mut d.layers[0].light;
        light.enabled = true;
        light.relief = rnd.range(0.5, 2.0);
        light.specular = rnd.range(0.1, 0.6);
        light.shadows = rnd.chance(0.5);
        light.occlusion = if rnd.chance(0.5) { rnd.range(0.2, 0.8) } else { 0.0 };
        if let Some(l) = light.lights.first_mut() {
            l.azimuth = rnd.range(-180.0, 180.0);
        }
    }
    if rnd.chance(0.3) {
        d.view.rotation = rnd.range(0.0, 360.0);
    }
    let steps = (rnd.unit() * (depth + 1) as f64) as usize;
    (d, steps)
}

/// A random document that frames well (up to `tries` attempts; the last attempt otherwise).
pub fn random_framed(kind: RandomKind, seed: u64, tries: usize) -> Document {
    let mut rnd = Rand::new(seed);
    let mut last = Document::default();
    for _ in 0..tries.max(1) {
        let (mut d, steps) = random_doc(kind, &mut rnd);
        if frame(&mut d, steps, &mut rnd) && looks_good(&d) {
            return d;
        }
        last = d;
    }
    last
}

/// How a thumbnail looks, from two renders of it (RGBA8): `half` after half the samples and
/// `full` after all. Returns `contrast` (standard deviation of the brightness, 0..0.5) and
/// `noise` (mean brightness change between the two, relative to the contrast). Real detail
/// stays when more samples come in; sub-pixel dust and grain change, so dusty and grainy
/// images have much noise, empty or flat ones little contrast.
pub fn thumb_quality(half: &[u8], full: &[u8]) -> (f64, f64) {
    let n = (full.len() / 4).min(half.len() / 4);
    if n == 0 {
        return (0.0, 0.0);
    }
    let (lh, lf) = (luminance(&half[..n * 4]), luminance(&full[..n * 4]));
    let c = contrast(&full[..n * 4]);
    let change = lh.iter().zip(&lf).map(|(a, b)| (a - b).abs()).sum::<f64>() / n as f64;
    (c, change / (c + 0.02))
}

/// A variant of `base`: `strength` (0..1) scales how far it goes.
pub fn mutate(base: &Document, what: Mutation, strength: f64, seed: u64) -> Document {
    let mut rnd = Rand::new(seed);
    let mut d = base.clone();
    let k = strength.clamp(0.0, 1.0);
    let what = if what == Mutation::All { *rnd.pick(&Mutation::ALL[1..]) } else { what };
    match what {
        Mutation::Parameters | Mutation::All => {
            for l in &mut d.layers {
                let Some(program) = crate::formula::library::library().get(&l.formula).and_then(|e| e.program_with(&l.params).ok()) else {
                    continue;
                };
                let mut changed = false;
                for p in &program.params {
                    if !rnd.chance(0.6) {
                        continue;
                    }
                    let cur = l.params.get(&p.name).unwrap_or(&p.default).clone();
                    let span = |v: f64| match (p.min, p.max) {
                        (Some(a), Some(b)) if b > a => (b - a) * 0.1,
                        _ => v.abs().max(0.5) * 0.2,
                    };
                    let new = match cur {
                        ParamValue::Float(v) => ParamValue::Float(v + rnd.range(-1.0, 1.0) * span(v) * k),
                        ParamValue::Complex([a, b]) => {
                            let s = span(a.hypot(b));
                            ParamValue::Complex([a + rnd.range(-1.0, 1.0) * s * k, b + rnd.range(-1.0, 1.0) * s * k])
                        }
                        _ => continue,
                    };
                    l.params.insert(p.name.clone(), new);
                    changed = true;
                }
                if l.julia || !changed {
                    // Julia seeds move a little relative to the set's size.
                    let s = 0.05 * k;
                    l.seed = [l.seed[0] + rnd.range(-s, s), l.seed[1] + rnd.range(-s, s)];
                }
            }
        }
        Mutation::Colors => {
            if rnd.chance(0.5) {
                randomize_colors(&mut d, &mut rnd);
            } else {
                for l in &mut d.layers {
                    let a = Adjust { hue: rnd.range(-120.0, 120.0) * k, saturation: rnd.range(-0.3, 0.3) * k, ..Default::default() };
                    l.outside.gradient.stops = crate::palette::adjust(&l.outside.gradient.stops, a);
                    l.outside.offset = (l.outside.offset + rnd.range(-0.5, 0.5) * k).rem_euclid(1.0);
                    l.outside.density *= 2f64.powf(rnd.range(-1.0, 1.0) * k);
                }
            }
        }
        Mutation::View => {
            // A few tries for a view that still shows the edge.
            for _ in 0..4 {
                let mut v = base.clone();
                let r = rnd.range(-0.6, 0.6) * k;
                v.view.pan([r * rnd.range(-1.0, 1.0), r * rnd.range(-1.0, 1.0)]);
                v.view.radius *= 2f64.powf(rnd.range(-2.0, 2.0) * k);
                if rnd.chance(0.4) {
                    v.view.rotation = (v.view.rotation + rnd.range(-40.0, 40.0) * k).rem_euclid(360.0);
                }
                d = v;
                if frame(&mut d.clone(), 0, &mut rnd) {
                    break;
                }
            }
        }
        Mutation::Formula => {
            let l = &mut d.layers[0];
            let center = base.view.center_f64();
            if rnd.chance(0.5) && !l.julia {
                // The Julia set of the view's center.
                l.julia = true;
                l.seed = center;
                d.view = crate::doc::View { center: ["0".into(), "0".into()], radius: 1.6.into(), rotation: d.view.rotation };
            } else {
                // Another formula of the same family, keeping view and colors.
                let pool = if CONVERGENT.iter().any(|g| g.id == l.formula) { CONVERGENT } else { ESCAPE };
                let others: Vec<&Good> = pool.iter().filter(|g| g.id != l.formula).collect();
                let g = *rnd.pick(&others);
                l.formula = g.id.to_string();
                l.params.clear();
                set_params(&mut d, g, &mut rnd);
            }
        }
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_documents_use_known_formulas_and_frame() {
        for g in ESCAPE.iter().chain(CONVERGENT) {
            assert!(crate::formula::library::library().get(g.id).is_some_and(|e| e.program().is_ok()), "{} does not compile", g.id);
        }
        let mut framed = 0;
        for seed in 0..8 {
            let d = random_framed(RandomKind::Any, seed, 3);
            assert!(crate::render::Plan::sized(&d, 32, 18).is_ok());
            if probe(&d).is_some_and(|img| contrast(&img) > 0.06) {
                framed += 1;
            }
        }
        assert!(framed >= 6, "only {framed} of 8 random documents have contrast");
    }

    #[test]
    fn mutations_change_the_document() {
        let base = random_framed(RandomKind::Escape, 3, 3);
        for what in Mutation::ALL {
            let changed = (0..4).any(|seed| mutate(&base, what, 0.8, seed) != base);
            assert!(changed, "{what:?} changes nothing");
        }
    }

    #[test]
    fn quality_tells_noise_from_detail() {
        let flat = vec![128u8; 64 * 4];
        assert!(thumb_quality(&flat, &flat).0 < 1e-9);
        let detail: Vec<u8> = (0..64 * 4).map(|i| if (i / 4) % 2 == 0 { 255 } else { 0 }).collect();
        let (c, n) = thumb_quality(&detail, &detail);
        assert!(c > 0.4 && n < 1e-9);
        let shifted: Vec<u8> = (0..64 * 4).map(|i| if (i / 4) % 2 == 1 { 255 } else { 0 }).collect();
        assert!(thumb_quality(&shifted, &detail).1 > 1.0);
    }
}
