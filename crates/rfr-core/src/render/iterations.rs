// SPDX-License-Identifier: LGPL-3.0-or-later

//! Automatic iteration count: a small CPU probe of one layer finds how many iterations its
//! view needs. While a noticeable share of the probe's pixels escape only in the second half
//! of the limit and some stay inside, more of those would escape with more iterations: the
//! limit doubles.

use crate::doc::{Document, InsideMethod, LayerKind, OutsideMethod};

/// Probe width in pixels (the height follows the image's aspect).
pub const PROBE_W: u32 = 192;
/// The highest limit the probe suggests.
pub const MAX_ITERATIONS: u32 = 1_000_000;
/// Share of the probe's pixels that may escape in the second half of the limit.
const LATE_SHARE: f64 = 0.01;

/// The iteration limit the layer at `path` needs for the document's view: its current limit
/// or more (doubled while late escapes are frequent, see the module notes).
pub fn suggest(doc: &Document, path: &[usize]) -> Result<u32, String> {
    let layer = doc.layer(path).ok_or("no such layer")?;
    if layer.kind != LayerKind::Fractal {
        return Err(format!("{} is not a fractal layer", layer.name));
    }
    // The layer alone, plain: only its orbits count.
    let mut probe = doc.clone();
    let mut l = layer.clone();
    l.mask = None;
    l.visible = true;
    l.outside_method = OutsideMethod::Smooth;
    l.inside_method = InsideMethod::Solid;
    l.light.enabled = false;
    l.effects = Default::default();
    probe.layers = vec![l];
    probe.animation = None;
    let w = PROBE_W;
    let h = ((w as f64 * doc.image.height as f64 / doc.image.width.max(1) as f64).round() as u32).max(16);
    let mut n = layer.max_iter.clamp(1, MAX_ITERATIONS);
    loop {
        probe.layers[0].max_iter = n;
        let plan = super::Plan::sized(&probe, w, h)?;
        let counts = super::cpu::iteration_counts(&plan);
        let inside = counts.iter().filter(|c| c.is_none()).count();
        let late = counts.iter().filter(|c| c.is_some_and(|i| i > n / 2)).count();
        if inside == 0 || (late as f64) < LATE_SHARE * counts.len() as f64 || n >= MAX_ITERATIONS {
            return Ok(n);
        }
        n = n.saturating_mul(2).min(MAX_ITERATIONS);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raises_the_limit_near_the_boundary() {
        // The whole set needs few iterations; a view at the seahorse valley needs more.
        let mut doc = Document::default();
        doc.layers[0].max_iter = 50;
        let whole = suggest(&doc, &[0]).unwrap();
        doc.view.center = ["-0.7453".into(), "0.1127".into()];
        doc.view.radius = 0.0005.into();
        let deep = suggest(&doc, &[0]).unwrap();
        assert!(whole < deep, "{whole} {deep}");
        // Never below the current limit.
        doc.layers[0].max_iter = 4000;
        assert!(suggest(&doc, &[0]).unwrap() >= 4000);
    }
}
