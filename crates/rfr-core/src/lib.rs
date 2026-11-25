// SPDX-License-Identifier: LGPL-3.0-or-later

//! RFract core: everything that does not need a GPU or a window: the document model, the
//! formula language and library, gradients, the CPU reference renderer, the denoiser glue and
//! the platform folders.

pub mod doc;
pub mod formula;
pub mod gradient;
pub mod io;
pub mod math;
pub mod palette;
pub mod paths;
pub mod render;

/// The program version (one version for all crates).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// "RFract x.y.z".
pub fn name_version() -> String {
    format!("RFract {VERSION}")
}
