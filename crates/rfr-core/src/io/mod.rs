// SPDX-License-Identifier: LGPL-3.0-or-later

//! Files: documents (`.rfr`) and PNG images that carry their document.

pub mod png;

use std::path::Path;

use crate::doc::Document;

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
