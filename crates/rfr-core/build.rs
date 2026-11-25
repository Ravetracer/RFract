// SPDX-License-Identifier: LGPL-3.0-or-later

//! Embeds the bundled formula sets (`formulas/`) into the library: new or updated files show up
//! without code changes.

use std::fmt::Write;
use std::path::Path;

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("formulas");
    let fp = root.join("fractalpark").join("definitions");
    println!("cargo:rerun-if-changed={}", root.display());
    println!("cargo:rerun-if-changed={}", fp.display());
    let mut files: Vec<_> = std::fs::read_dir(&fp)
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "frm")).collect())
        .unwrap_or_default();
    files.sort();
    let mut out = String::from("/// FractalPark definitions (file name, source).\npub const FRACTALPARK: &[(&str, &str)] = &[\n");
    for p in &files {
        let _ = writeln!(
            out,
            "    ({:?}, include_str!({:?})),",
            p.file_name().unwrap().to_string_lossy(),
            p.canonicalize().unwrap().display().to_string()
        );
    }
    out.push_str("];\n");
    let dest = Path::new(&std::env::var("OUT_DIR").unwrap()).join("bundled_formulas.rs");
    std::fs::write(dest, out).unwrap();
}
