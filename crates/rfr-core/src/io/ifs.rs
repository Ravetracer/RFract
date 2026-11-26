// SPDX-License-Identifier: LGPL-3.0-or-later

//! FractInt `.ifs` files: named iterated function systems, one map per line (`a b c d e f p`:
//! x' = a·x + b·y + e, y' = c·x + d·y + f, p its share of the steps). Every entry becomes a
//! document with an IFS density layer, the view fitted to the attractor.

use std::path::Path;

use super::ImportedImage;
use crate::doc::{DensityMode, Document, IfsMap, LayerKind};

/// The systems of an `.ifs` file.
pub fn read(path: &Path) -> Result<Vec<ImportedImage>, String> {
    let text = super::read_text_lossy(path)?;
    let images = parse(&text);
    if images.is_empty() {
        return Err(format!("{}: no IFS entries", path.display()));
    }
    Ok(images)
}

/// The entries of `.ifs` text.
pub fn parse(text: &str) -> Vec<ImportedImage> {
    // Comments start with ';'.
    let clean: String = text.lines().map(|l| l.split(';').next().unwrap_or("")).collect::<Vec<_>>().join("\n");
    let mut out = Vec::new();
    let mut rest = clean.as_str();
    while let Some(open) = rest.find('{') {
        let head = rest[..open].trim();
        let Some(close) = rest[open..].find('}') else { break };
        let body = &rest[open + 1..open + close];
        rest = &rest[open + close + 1..];
        let name = head.split_whitespace().next().unwrap_or("ifs").to_string();
        let three_d = head.to_ascii_lowercase().contains("(3d)");
        let numbers: Vec<f64> = body.split_whitespace().filter_map(|t| t.parse().ok()).collect();
        if three_d || !numbers.len().is_multiple_of(7) || numbers.is_empty() {
            out.push(ImportedImage {
                name: name.clone(),
                doc: Document::default(),
                notes: vec![if three_d { "3D systems are not supported".into() } else { "the maps do not have 7 numbers each".into() }],
            });
            continue;
        }
        let maps: Vec<IfsMap> = numbers.chunks(7).map(|c| IfsMap::new([c[0], c[1], c[2], c[3], c[4], c[5]], c[6])).collect();
        out.push(ImportedImage { doc: document(&name, maps), name, notes: Vec::new() });
    }
    out
}

/// A document showing the IFS `maps`, the view fitted to its attractor.
pub fn document(name: &str, maps: Vec<IfsMap>) -> Document {
    let mut d = Document::default();
    d.image.background = [0.0, 0.0, 0.0, 1.0];
    let l = &mut d.layers[0];
    l.name = name.to_string();
    l.kind = LayerKind::Density;
    l.max_iter = 500;
    l.density.mode = DensityMode::Ifs;
    l.density.orbits = 16384;
    l.outside.gradient = crate::gradient::preset("Ember").unwrap_or_default();
    // The attractor's extent from a short chaos game.
    let total: f64 = maps.iter().map(|m| m.weight.max(0.0)).sum::<f64>().max(1e-12);
    let (mut x, mut y) = (0.0, 0.0);
    let mut rnd = 0x1234_5678u32;
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for it in 0..20000 {
        rnd = crate::render::hash(rnd);
        let u = rnd as f64 / 4294967296.0 * total;
        let mut acc = 0.0;
        let m = maps
            .iter()
            .find(|m| {
                acc += m.weight.max(0.0);
                u < acc
            })
            .unwrap_or(&maps[maps.len() - 1]);
        (x, y) = m.apply(x, y);
        if it > 20 && x.is_finite() && y.is_finite() {
            lo = [lo[0].min(x), lo[1].min(y)];
            hi = [hi[0].max(x), hi[1].max(y)];
        }
    }
    if lo[0].is_finite() {
        let aspect = d.image.width as f64 / d.image.height.max(1) as f64;
        let half = ((hi[1] - lo[1]) * 0.55).max((hi[0] - lo[0]) * 0.55 / aspect).max(1e-9);
        d.view.set_center([(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0]);
        d.view.radius = half.into();
        l_density_region(&mut d, lo, hi);
    }
    d.layers[0].density.ifs = maps;
    d
}

fn l_density_region(d: &mut Document, lo: [f64; 2], hi: [f64; 2]) {
    d.layers[0].density.region = [lo[0], lo[1], hi[0], hi[1]];
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_fractint_ifs() {
        let text = "fern { ; Barnsley's fern\n  0 0 0 .16 0 0 .01\n  .85 .04 -.04 .85 0 1.6 .85\n  .2 -.26 .23 .22 0 1.6 .07\n  -.15 .28 .26 .24 0 .44 .07\n}\ncube (3D) {\n 1 2 3 4 5 6 7 8 9 10 11 12 13\n}\n";
        let images = parse(text);
        assert_eq!(images.len(), 2);
        assert_eq!(images[0].name, "fern");
        let l = &images[0].doc.layers[0];
        assert_eq!(l.density.ifs.len(), 4);
        assert_eq!(l.density.ifs[1].coefs, [0.85, 0.04, -0.04, 0.85, 0.0, 1.6]);
        // The fern stands between y = 0 and 10.
        let c = images[0].doc.view.center_f64();
        assert!(c[1] > 3.0 && c[1] < 7.0, "{c:?}");
        assert!(!images[1].notes.is_empty());
    }
}
