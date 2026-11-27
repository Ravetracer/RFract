// SPDX-License-Identifier: LGPL-3.0-or-later

//! Histogram coloring: a region whose transfer is [`Transfer::Histogram`] maps its coloring
//! value through the distribution of the values in the view, so every part of the gradient
//! covers about the same share of the region's pixels. The distribution comes from a small
//! CPU probe of the layer at a fixed number of pixels (the same table for the preview, exports
//! of any size and poster tiles) and is kept as quantiles: `HIST_N + 1` values, the smallest,
//! the largest and the ones between at equal steps of the share. The GPU reads them from the
//! layer's gradient buffer (`kernel.wgsl` `hist_cdf`).

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, OnceLock};

use crate::doc::{Document, Layer, LayerKind, Transfer};

/// Intervals of a quantile table (it holds one value more).
pub const HIST_N: usize = 256;
/// Rows of four values a table takes in the GPU's gradient buffer.
pub const HIST_ROWS: usize = (HIST_N + 1).div_ceil(4);
/// Pixels of the probe (its sides follow the image's aspect).
const PROBE_PIXELS: f64 = 128.0 * 128.0;
/// Probes kept for reuse (views and layers seen last).
const CACHE: usize = 32;

/// Quantile tables of the outside and the inside of a layer (empty: not used or no values).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tables {
    pub outside: Vec<f32>,
    pub inside: Vec<f32>,
}

/// Whether a region of the layer is colored by its histogram.
pub fn used(l: &Layer) -> bool {
    l.kind == LayerKind::Fractal && (l.outside.transfer == Transfer::Histogram || l.inside.transfer == Transfer::Histogram)
}

/// The share (0..1) of the table's values below `v`: linear between the quantiles, the middle
/// of a run of equal quantiles for a value in it (integer values such as iteration counts).
/// An empty table leaves the value as it is.
pub fn cdf(q: &[f32], v: f64) -> f64 {
    if q.is_empty() {
        return v;
    }
    let n = q.len() - 1;
    let below = q.partition_point(|&x| (x as f64) < v);
    let upto = q.partition_point(|&x| (x as f64) <= v);
    if upto > below {
        return (below + upto - 1) as f64 * 0.5 / n as f64;
    }
    if below == 0 {
        return 0.0;
    }
    if below > n {
        return 1.0;
    }
    let k = below - 1;
    let (a, b) = (q[k] as f64, q[k + 1] as f64);
    (k as f64 + (v - a) / (b - a)) / n as f64
}

/// The quantile table of `values` (empty for fewer than two).
pub fn quantiles(mut values: Vec<f64>) -> Vec<f32> {
    values.retain(|v| v.is_finite());
    if values.len() < 2 {
        return Vec::new();
    }
    values.sort_by(f64::total_cmp);
    let last = (values.len() - 1) as f64;
    (0..=HIST_N).map(|k| values[(k as f64 * last / HIST_N as f64).round() as usize] as f32).collect()
}

/// The probe size for a `w` × `h` image.
fn probe_size(w: u32, h: u32) -> (u32, u32) {
    let aspect = w.max(1) as f64 / h.max(1) as f64;
    let pw = (PROBE_PIXELS * aspect).sqrt().round().clamp(8.0, 1024.0) as u32;
    let ph = (PROBE_PIXELS / pw as f64).round().clamp(8.0, 1024.0) as u32;
    (pw, ph)
}

type Cache = Mutex<Vec<(u64, Arc<Tables>)>>;

fn cache() -> &'static Cache {
    static C: OnceLock<Cache> = OnceLock::new();
    C.get_or_init(|| Mutex::new(Vec::new()))
}

/// The tables of layer `l` (alone, plain) in the view of `doc` at `w` × `h` pixels; empty
/// tables when the layer uses none or cannot be probed.
pub fn tables(doc: &Document, l: &Layer, w: u32, h: u32) -> Arc<Tables> {
    if !used(l) {
        return Arc::default();
    }
    let (pw, ph) = probe_size(w, h);
    let mut probe = Document { layers: Vec::new(), lights: Vec::new(), bookmarks: Vec::new(), animation: None, ..doc.clone() };
    let mut alone = l.clone();
    alone.mask = None;
    alone.visible = true;
    alone.opacity = 1.0;
    alone.light.enabled = false;
    alone.effects = Default::default();
    for (c, on) in [(&mut alone.outside, l.outside.transfer == Transfer::Histogram), (&mut alone.inside, l.inside.transfer == Transfer::Histogram)] {
        // The values before the transfer; the gradient plays no part.
        c.transfer = Transfer::Linear;
        c.gradient = Default::default();
        c.density = if on { 1.0 } else { 0.0 };
        c.offset = 0.0;
    }
    probe.layers = vec![alone];
    probe.image.width = pw;
    probe.image.height = ph;
    let key = {
        let mut s = std::collections::hash_map::DefaultHasher::new();
        serde_json::to_string(&probe).unwrap_or_default().hash(&mut s);
        s.finish()
    };
    if let Some((_, t)) = cache().lock().ok().and_then(|c| c.iter().find(|(k, _)| *k == key).cloned()) {
        return t;
    }
    let t = Arc::new(match super::Plan::sized(&probe, pw, ph) {
        Ok(plan) => {
            let mut parts: HashMap<bool, Vec<f64>> = HashMap::new();
            for (inside, v) in super::cpu::coloring_values(&plan).into_iter().flatten() {
                parts.entry(inside).or_default().push(v);
            }
            let mut table = |inside: bool, on: bool| if on { quantiles(parts.remove(&inside).unwrap_or_default()) } else { Vec::new() };
            Tables { outside: table(false, l.outside.transfer == Transfer::Histogram), inside: table(true, l.inside.transfer == Transfer::Histogram) }
        }
        Err(_) => Tables::default(),
    });
    if let Ok(mut c) = cache().lock() {
        if c.len() >= CACHE {
            c.remove(0);
        }
        c.push((key, t.clone()));
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cdf_spreads_the_values_evenly() {
        // Values crowded at the low end: the shares follow the ranks, not the values.
        let values: Vec<f64> = (0..1000).map(|i| (i as f64 / 1000.0).powi(4) * 100.0).collect();
        let q = quantiles(values.clone());
        assert_eq!(q.len(), HIST_N + 1);
        for (i, v) in values.iter().enumerate().step_by(37) {
            let share = i as f64 / 999.0;
            assert!((cdf(&q, *v) - share).abs() < 0.01, "{i} {} {share}", cdf(&q, *v));
        }
        assert_eq!(cdf(&q, -1.0), 0.0);
        assert_eq!(cdf(&q, 1e9), 1.0);
        // Equal values sit in the middle of their run.
        let q = quantiles(vec![1.0, 1.0, 1.0, 2.0]);
        assert!((0.3..0.45).contains(&cdf(&q, 1.0)), "{}", cdf(&q, 1.0));
        assert!(cdf(&q, 2.0) > 0.9);
        // No table: the value stays.
        assert_eq!(cdf(&[], 3.5), 3.5);
        assert!(quantiles(vec![1.0]).is_empty());
    }

    #[test]
    fn tables_come_from_the_view() {
        let mut doc = Document::default();
        doc.layers[0].outside.transfer = Transfer::Histogram;
        let t = tables(&doc, &doc.layers[0], 1920, 1080);
        assert_eq!(t.outside.len(), HIST_N + 1);
        assert!(t.inside.is_empty());
        assert!(t.outside.windows(2).all(|w| w[0] <= w[1]));
        // The same for any image size of the same aspect; a different view changes them.
        assert_eq!(tables(&doc, &doc.layers[0], 640, 360), t);
        doc.view.radius = 0.01.into();
        doc.view.center = ["-0.7453".into(), "0.1127".into()];
        assert_ne!(tables(&doc, &doc.layers[0], 640, 360).outside, t.outside);
    }
}
