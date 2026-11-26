// SPDX-License-Identifier: LGPL-3.0-or-later

//! Files: documents (`.rfr`) and PNG images that carry their document.

pub mod ifs;
pub mod lsystem;
pub mod par;
pub mod png;
pub mod upr;

use std::path::Path;

use crate::doc::Document;

/// One image of a parameter file (`.upr`, `.par`) as a document.
#[derive(Clone, Debug)]
pub struct ImportedImage {
    pub name: String,
    pub doc: Document,
    /// What could not be carried over exactly.
    pub notes: Vec<String>,
}

/// Whether `path` is a parameter file (`.upr`, `.par`, `.ifs` or `.l`).
pub fn is_parameter_file(path: &Path) -> bool {
    path.extension().is_some_and(|e| ["upr", "par", "ifs", "l"].iter().any(|x| e.eq_ignore_ascii_case(x)))
}

/// The images of a parameter file (`.upr`, `.par` or `.ifs`).
pub fn read_parameter_file(path: &Path) -> Result<Vec<ImportedImage>, String> {
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "par" => par::read(path),
        "ifs" => ifs::read(path),
        "l" => lsystem::read(path),
        _ => upr::read(path),
    }
}

/// Text of a file, Latin-1 when it is not UTF-8 (older parameter and formula files).
pub(crate) fn read_text_lossy(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => e.into_bytes().iter().map(|&b| b as char).collect(),
    })
}

/// Reads a document from an `.rfr` file or a PNG rendered by RFract.
pub fn open(path: &Path) -> Result<Document, String> {
    let is_png = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("png"));
    if is_png {
        return png::read_document(path);
    }
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Document::from_json(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Writes a document as `.rfr`.
pub fn save(doc: &Document, path: &Path) -> Result<(), String> {
    std::fs::write(path, doc.to_json()).map_err(|e| format!("{}: {e}", path.display()))
}
