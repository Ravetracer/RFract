// SPDX-License-Identifier: LGPL-3.0-or-later

//! The formula language: front ends (FractalPark FRM-like v1, UFM syntax), a typed IR and
//! the CPU back end. The GPU back end (WGSL) lives in rfr-gpu and follows the same IR.

pub mod ast;
pub mod check;
pub mod color;
pub mod cpu;
pub mod deriv;
pub mod ir;
pub mod library;
pub mod parse;
pub mod period;
pub mod perturb;

pub use ast::{Dialect, ParamKind, ParamValue, ProgramKind};
pub use ir::Program;
pub use parse::Error;

/// Parses and checks a FRM-like v1 definition.
pub fn compile_frm(src: &str) -> Result<Program, Error> {
    check::check(&parse::parse_frm(src)?)
}

/// Parses and checks every entry of a UFM syntax file of fractal formulas; one result per entry.
pub fn compile_ufm_file(src: &str) -> Vec<(String, Result<Program, Error>)> {
    compile_ufm_file_kind(src, ProgramKind::Fractal)
}

/// Parses and checks every entry of a UFM syntax file of the given kind (`.ufm` fractals,
/// `.ucl` colorings, `.uxf` transformations).
pub fn compile_ufm_file_kind(src: &str, kind: ProgramKind) -> Vec<(String, Result<Program, Error>)> {
    parse::split_entries(src).iter().map(|e| (e.name.clone(), parse::parse_ufm_entry(e).and_then(|a| check::check_kind(&a, kind)))).collect()
}
