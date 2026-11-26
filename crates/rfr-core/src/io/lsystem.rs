// SPDX-License-Identifier: LGPL-3.0-or-later

//! FractInt `.l` files: named L-systems (`Angle n`, `Axiom …`, rules `F=…`). Every entry
//! becomes a document with an L-system density layer, the view fitted to its strokes.

use std::path::Path;

use super::ImportedImage;
use crate::doc::{DensityMode, Document, LSystem, LayerKind};

/// The L-systems of an `.l` file.
pub fn read(path: &Path) -> Result<Vec<ImportedImage>, String> {
    let text = super::read_text_lossy(path)?;
    let images = parse(&text);
    if images.is_empty() {
        return Err(format!("{}: no L-system entries", path.display()));
    }
    Ok(images)
}

/// The entries of `.l` text.
pub fn parse(text: &str) -> Vec<ImportedImage> {
    let clean: String = text.lines().map(|l| l.split(';').next().unwrap_or("")).collect::<Vec<_>>().join("\n");
    let mut out = Vec::new();
    let mut rest = clean.as_str();
    while let Some(open) = rest.find('{') {
        let name = rest[..open].split_whitespace().last().unwrap_or("lsystem").to_string();
        let Some(close) = rest[open..].find('}') else { break };
        let body = &rest[open + 1..open + close];
        rest = &rest[open + close + 1..];
        let mut sys = LSystem { axiom: String::new(), rules: String::new(), ..LSystem::default() };
        let mut rules = Vec::new();
        for line in body.lines().map(str::trim).filter(|l| !l.is_empty()) {
            let lower = line.to_ascii_lowercase();
            if let Some(v) = lower.strip_prefix("angle") {
                sys.angle = v.trim().parse().unwrap_or(6.0);
            } else if lower.starts_with("axiom") {
                sys.axiom = line[5..].trim().to_string();
            } else if line.contains('=') {
                rules.push(line.replace(' ', ""));
            }
        }
        sys.rules = rules.join("\n");
        enough_rewrites(&mut sys);
        let notes = if sys.axiom.is_empty() { vec!["no axiom".to_string()] } else { Vec::new() };
        out.push(ImportedImage { doc: document(&name, sys), name, notes });
    }
    out
}

/// A document drawing the L-system, the view fitted to its strokes.
pub fn document(name: &str, sys: LSystem) -> Document {
    document_with(name, sys)
}

/// Rewrites so that the L-system draws at least about 20 000 segments (at most 16).
pub fn enough_rewrites(sys: &mut LSystem) {
    for n in 1..=16 {
        sys.iterations = n;
        if sys.segments().len() >= 20_000 {
            break;
        }
    }
}

fn document_with(name: &str, sys: LSystem) -> Document {
    let mut d = Document::default();
    d.image.background = [0.0, 0.0, 0.0, 1.0];
    let l = &mut d.layers[0];
    l.name = name.to_string();
    l.kind = LayerKind::Density;
    l.max_iter = 500;
    l.density.mode = DensityMode::LSystem;
    l.density.orbits = 16384;
    l.outside.gradient = crate::gradient::preset("Starlight").unwrap_or_default();
    l.density.lsystem = sys;
    crate::render::density::fit_view(&mut d, &[0]);
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_fractint_lsystems() {
        let text =
            "Koch1 { ; Koch snowflake\n  Angle 6\n  Axiom F--F--F\n  F=F+F--F+F\n}\nPlant {\n Angle 16\n Axiom F\n F=FF-[-F+F+F]+[+F-F-F]\n}\n";
        let images = parse(text);
        assert_eq!(images.len(), 2);
        let s = &images[0].doc.layers[0].density.lsystem;
        assert_eq!((s.angle, s.axiom.as_str(), s.rules.as_str()), (6.0, "F--F--F", "F=F+F--F+F"));
        // Koch: rewrites until at least 20 000 segments (3 · 4⁷).
        assert_eq!(s.iterations, 7);
        assert_eq!(s.segments().len(), 3 * 4usize.pow(7));
        assert!(images[0].doc.view.radius.to_f64() > 10.0);
        assert!(images[1].doc.layers[0].density.lsystem.segments().len() > 100);
    }
}
