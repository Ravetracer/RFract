// SPDX-License-Identifier: LGPL-3.0-or-later

//! The formula library: RFract's built-in formulas (`formulas/rfract/*.ufm`) and the bundled
//! FractalPark set (`formulas/fractalpark`), compiled once on first use.

use std::sync::{Arc, OnceLock};

use serde::Deserialize;

use super::ast::{Dialect, ProgramKind};
use super::ir::Program;
use super::parse::Error;

include!(concat!(env!("OUT_DIR"), "/bundled_formulas.rs"));

const RFRACT_UFM: &str = include_str!("../../formulas/rfract/rfract.ufm");
const RFRACT_UCL: &str = include_str!("../../formulas/rfract/rfract.ucl");
const RFRACT_UXF: &str = include_str!("../../formulas/rfract/rfract.uxf");
const FRACTALPARK_INDEX: &str = include_str!("../../formulas/fractalpark/index.json");

/// Where a formula comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Set {
    RFract,
    FractalPark,
}

impl Set {
    pub fn label(self) -> &'static str {
        match self {
            Set::RFract => "RFract",
            Set::FractalPark => "FractalPark",
        }
    }
}

/// The view a formula suggests: center, half the height of the view in plane units, iteration
/// limit and, for Julia-mode formulas, the seed.
#[derive(Clone, Debug, PartialEq)]
pub struct DefaultView {
    pub center: [f64; 2],
    pub radius: f64,
    pub max_iter: u32,
    pub julia: Option<[f64; 2]>,
}

#[derive(Clone, Debug)]
pub struct Entry {
    /// Stable identifier stored in documents: `rfract:<entry name>`, `fractalpark:<formula id>`.
    pub id: String,
    pub title: String,
    pub family: String,
    pub set: Set,
    pub kind: ProgramKind,
    pub dialect: Dialect,
    pub source: &'static str,
    /// Name of the entry inside a multi-entry UFM file.
    pub entry: String,
    pub program: Result<Arc<Program>, Error>,
    pub view: DefaultView,
}

pub struct Library {
    pub entries: Vec<Entry>,
}

impl Library {
    pub fn get(&self, id: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.id == id)
    }
    /// Fractal formulas that compiled.
    pub fn usable(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(|e| e.program.is_ok() && e.kind == ProgramKind::Fractal)
    }
    /// Coloring algorithms that compiled.
    pub fn colorings(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(|e| e.program.is_ok() && e.kind == ProgramKind::Coloring)
    }
    /// Transformations that compiled.
    pub fn transforms(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(|e| e.program.is_ok() && e.kind == ProgramKind::Transform)
    }
}

#[derive(Deserialize)]
struct FpIndex {
    rows: Vec<FpRow>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FpRow {
    formula_id: String,
    display_name: String,
    family: String,
    definition_path: String,
    profile: FpProfile,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FpProfile {
    mode: String,
    center: [f64; 2],
    zoom: f64,
    iterations: u32,
    #[serde(default)]
    julia_c: Option<[f64; 2]>,
}

/// The library (compiled on the first call).
pub fn library() -> &'static Library {
    static LIB: OnceLock<Library> = OnceLock::new();
    LIB.get_or_init(build)
}

/// Entries of a built-in UFM syntax file of the given kind (ids `<prefix>:<entry name>`).
fn ufm_entries(src: &'static str, kind: ProgramKind, prefix: &str, family: &str, out: &mut Vec<Entry>) {
    for e in super::parse::split_entries(src) {
        let ast = super::parse::parse_ufm_entry(&e);
        let title = ast.as_ref().ok().and_then(|a| a.title.clone()).unwrap_or_else(|| e.name.clone());
        let program = ast.and_then(|a| super::check::check_kind(&a, kind)).map(Arc::new);
        out.push(Entry {
            id: format!("{prefix}:{}", e.name),
            title,
            family: family.into(),
            set: Set::RFract,
            kind,
            dialect: Dialect::Ufm,
            source: src,
            entry: e.name.clone(),
            program,
            view: DefaultView { center: [0.0, 0.0], radius: 1.5, max_iter: 250, julia: None },
        });
    }
}

fn build() -> Library {
    let mut entries = Vec::new();
    ufm_entries(RFRACT_UCL, ProgramKind::Coloring, "rfract-coloring", "Coloring", &mut entries);
    ufm_entries(RFRACT_UXF, ProgramKind::Transform, "rfract-transform", "Transformation", &mut entries);
    for e in super::parse::split_entries(RFRACT_UFM) {
        let ast = super::parse::parse_ufm_entry(&e);
        let (title, center, magn, maxiter) = match &ast {
            Ok(a) => {
                (a.title.clone().unwrap_or_else(|| e.name.clone()), a.center.unwrap_or([0.0, 0.0]), a.magn.unwrap_or(1.0), a.maxiter.unwrap_or(250))
            }
            Err(_) => (e.name.clone(), [0.0, 0.0], 1.0, 250),
        };
        let program = ast.and_then(|a| super::check::check(&a)).map(Arc::new);
        entries.push(Entry {
            id: format!("rfract:{}", e.name),
            title,
            family: "RFract".into(),
            set: Set::RFract,
            kind: ProgramKind::Fractal,
            dialect: Dialect::Ufm,
            source: RFRACT_UFM,
            entry: e.name.clone(),
            program,
            view: DefaultView { center, radius: 1.5 / magn.max(1e-300), max_iter: maxiter, julia: None },
        });
    }
    let index: FpIndex = serde_json::from_str(FRACTALPARK_INDEX).unwrap_or(FpIndex { rows: Vec::new() });
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for row in index.rows {
        let file = row.definition_path.rsplit('/').next().unwrap_or("");
        let Some((_, src)) = FRACTALPARK.iter().find(|(n, _)| *n == file) else { continue };
        let n = seen.entry(row.display_name.clone()).or_insert(0);
        *n += 1;
        let title = if *n > 1 { format!("{} ({n})", row.display_name) } else { row.display_name.clone() };
        let p = &row.profile;
        // FractalPark frames by half the width; RFract by half the height (16:9 assumed).
        let radius = (1.0 / p.zoom.max(1e-12)) * 9.0 / 16.0;
        entries.push(Entry {
            id: format!("fractalpark:{}", row.formula_id),
            title,
            family: row.family,
            set: Set::FractalPark,
            kind: ProgramKind::Fractal,
            dialect: Dialect::Frm,
            source: src,
            entry: String::new(),
            program: super::compile_frm(src).map(Arc::new),
            view: DefaultView {
                center: p.center,
                radius,
                max_iter: p.iterations.max(1),
                julia: if p.mode == "julia" { Some(p.julia_c.unwrap_or([0.0, 0.0])) } else { None },
            },
        });
    }
    Library { entries }
}

#[cfg(test)]
mod tests {
    use super::super::cpu::Compiled;
    use super::*;
    use crate::math::complex::C64;
    use crate::math::scalar::Dual;

    #[test]
    fn every_bundled_formula_compiles_and_runs() {
        let lib = library();
        let fp = lib.entries.iter().filter(|e| e.set == Set::FractalPark).count();
        let rf = lib.entries.iter().filter(|e| e.set == Set::RFract && e.kind == ProgramKind::Fractal).count();
        assert!(lib.colorings().count() >= 13, "colorings: {}", lib.colorings().count());
        let broken: Vec<String> = lib
            .entries
            .iter()
            .filter(|e| e.kind != ProgramKind::Fractal)
            .filter_map(|e| e.program.as_ref().err().map(|err| format!("{}: {err}", e.id)))
            .collect();
        assert!(broken.is_empty(), "{}", broken.join("\n"));
        assert!(lib.transforms().count() >= 11, "transforms: {}", lib.transforms().count());
        assert_eq!(fp, 534, "FractalPark entries");
        assert!(rf >= 17, "RFract entries: {rf}");
        let failed: Vec<String> =
            lib.entries.iter().filter_map(|e| e.program.as_ref().err().map(|err| format!("{} ({}): {err}", e.title, e.id))).collect();
        assert!(failed.is_empty(), "{} formulas fail:\n{}", failed.len(), failed.join("\n"));
        // Every formula runs on the CPU, plain and with derivatives, at a few points.
        // Colorings and transformations run on a Mandelbrot orbit.
        let m = lib.get("rfract:Mandelbrot").unwrap().program.as_ref().unwrap();
        let fractal = Compiled::<f64>::new(m, &[]);
        for e in lib.colorings().chain(lib.transforms()) {
            let p = e.program.as_ref().unwrap();
            let k = Compiled::<f64>::new(p, &[]);
            let (mut ff, mut kf) = (fractal.frame(), k.frame());
            for c in [C64::new(0.3, 0.6), C64::new(-0.1, 0.2)] {
                if e.kind == ProgramKind::Transform {
                    let (q, _) = k.transform(&mut kf, c);
                    assert!(q.is_finite() || q.re.is_nan(), "{}: {q:?}", e.title);
                    continue;
                }
                ff.pixel = c;
                ff.c = c;
                kf.pixel = c;
                kf.c = c;
                let o = fractal.run(&mut ff, 200);
                let o2 = fractal.run_with(&mut ff, 200, |h| match h {
                    super::super::cpu::Hook::Init(z) => {
                        kf.zcur = z;
                        k.color_init(&mut kf, 200);
                    }
                    super::super::cpu::Hook::Iter(it, z, zp) => {
                        kf.zcur = z;
                        k.color_loop(&mut kf, it, zp);
                    }
                });
                assert_eq!(o.iter, o2.iter);
                kf.zcur = o2.z;
                let (index, _) = k.color_final(&mut kf, o2.iter, o2.zprev);
                assert!(index.is_finite(), "{}: index {index}", e.title);
            }
        }
        for e in lib.usable() {
            let p = e.program.as_ref().unwrap();
            let plain = Compiled::<f64>::new(p, &[]);
            let dual = Compiled::<Dual>::new(p, &[]);
            let (mut f, mut g) = (plain.frame(), dual.frame());
            for (k, d) in [(0.0, 0.0), (0.3, -0.2), (-1.2, 0.7)].iter().enumerate() {
                let px = C64::new(e.view.center[0] + d.0 * e.view.radius, e.view.center[1] + d.1 * e.view.radius);
                f.pixel = px;
                f.c = px;
                f.ismand = k != 2;
                let o = plain.run(&mut f, 64);
                g.pixel = crate::math::complex::Cx::new(Dual::new(px.re, 1.0), Dual::new(px.im, 0.0));
                g.c = g.pixel;
                g.ismand = k != 2;
                let od = dual.run(&mut g, 64);
                assert_eq!(o.iter, od.iter, "{}: plain and dual runs differ", e.title);
            }
        }
    }
}
