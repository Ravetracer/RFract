// SPDX-License-Identifier: LGPL-3.0-or-later

//! RFract core: everything that does not need a GPU or a window. For now the platform folders,
//! the runtime-loaded denoiser and system information; the document model, the formula
//! language and the CPU renderer follow in stage 1 (see PLAN.md).

pub mod paths;
pub mod render;

/// The program version (one version for all crates).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// "RFract x.y.z".
pub fn name_version() -> String {
    format!("RFract {VERSION}")
}
