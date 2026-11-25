// SPDX-License-Identifier: LGPL-3.0-or-later

//! The formula language: front ends (FractalPark FRM-like v1, UFM syntax), a typed IR and
//! the CPU back end. The GPU back end (WGSL) lives in rfr-gpu and follows the same IR.

pub mod ast;
pub mod check;
pub mod cpu;
pub mod ir;
pub mod library;
pub mod parse;

pub use ast::{Dialect, ParamKind, ParamValue};
pub use ir::Program;
pub use parse::Error;

/// Parses and checks a FRM-like v1 definition.
pub fn compile_frm(src: &str) -> Result<Program, Error> {
    check::check(&parse::parse_frm(src)?)
}

/// Parses and checks every entry of a UFM syntax file; one result per entry.
pub fn compile_ufm_file(src: &str) -> Vec<(String, Result<Program, Error>)> {
    parse::split_entries(src).iter().map(|e| (e.name.clone(), parse::parse_ufm_entry(e).and_then(|a| check::check(&a)))).collect()
}
