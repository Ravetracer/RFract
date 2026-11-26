// SPDX-License-Identifier: LGPL-3.0-or-later

//! The open document with its file and undo history.
//!
//! Edits change `doc` directly; `commit` (called once per frame while no pointer button is held)
//! records an undo step when the document differs from the last recorded state, so a slider drag
//! or a pan becomes one step. Adapted from RMandelbulb3D (same author).

use std::path::PathBuf;

use rfr_core::doc::Document;

const MAX_UNDO: usize = 200;

pub struct Doc {
    pub doc: Document,
    pub path: Option<PathBuf>,
    saved: Document,
    undo: Vec<Document>,
    redo: Vec<Document>,
    current: Document,
}

impl Doc {
    pub fn new(doc: Document) -> Doc {
        Doc { saved: doc.clone(), current: doc.clone(), doc, path: None, undo: Vec::new(), redo: Vec::new() }
    }

    /// Records an undo step if the document changed since the last one.
    pub fn commit(&mut self) {
        if self.doc != self.current {
            let prev = std::mem::replace(&mut self.current, self.doc.clone());
            self.undo.push(prev);
            if self.undo.len() > MAX_UNDO {
                self.undo.remove(0);
            }
            self.redo.clear();
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty() || self.doc != self.current
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self) -> bool {
        self.commit();
        match self.undo.pop() {
            Some(d) => {
                self.redo.push(std::mem::replace(&mut self.current, d.clone()));
                self.doc = d;
                true
            }
            None => false,
        }
    }

    pub fn redo(&mut self) -> bool {
        match self.redo.pop() {
            Some(d) => {
                self.undo.push(std::mem::replace(&mut self.current, d.clone()));
                self.doc = d;
                true
            }
            None => false,
        }
    }

    /// Replaces the document as one undoable step.
    pub fn replace(&mut self, doc: Document) {
        self.commit();
        self.doc = doc;
        self.commit();
    }

    /// Loads a new document (clears the history).
    pub fn load(&mut self, doc: Document, path: Option<PathBuf>) {
        *self = Doc::new(doc);
        self.path = path;
    }

    pub fn mark_saved(&mut self, path: PathBuf) {
        self.path = Some(path);
        self.saved = self.doc.clone();
    }

    pub fn modified(&self) -> bool {
        self.doc != self.saved
    }

    pub fn title(&self) -> String {
        let name = self.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "Untitled".into());
        format!("{}{name} – RFract", if self.modified() { "• " } else { "" })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undo_and_redo_steps() {
        let mut d = Doc::new(Document::default());
        d.doc.view.radius = 0.5.into();
        d.commit();
        d.doc.view.radius = 0.25.into();
        d.commit();
        assert!(d.undo());
        assert_eq!(d.doc.view.radius.to_f64(), 0.5);
        assert!(d.redo());
        assert_eq!(d.doc.view.radius.to_f64(), 0.25);
        assert!(d.modified());
    }
}
