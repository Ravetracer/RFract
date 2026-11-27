// SPDX-License-Identifier: LGPL-3.0-or-later

//! Automatic iteration count: layers with *Auto* get their limit raised when the view, the
//! formula or the image settles in a state that needs more (`rfr_core::render::iterations`).
//! The probe runs in the background on a copy of the document.

use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use rfr_core::doc::{Document, Layer, LayerKind};

/// How long the state must stay unchanged before a probe starts.
const SETTLE: Duration = Duration::from_millis(350);

#[derive(Default)]
pub struct AutoIterations {
    /// The state last seen and since when.
    seen: Option<(String, Instant)>,
    /// The state the last probe ran for.
    probed: Option<String>,
    rx: Option<Receiver<Vec<Probe>>>,
}

struct Probe {
    path: Vec<usize>,
    key: String,
    result: Result<u32, String>,
}

/// The layers with *Auto* (visible fractal layers) and their state.
fn auto_layers(doc: &Document) -> Vec<(Vec<usize>, String)> {
    fn walk(list: &[Layer], prefix: &mut Vec<usize>, doc: &Document, out: &mut Vec<(Vec<usize>, String)>) {
        for (i, l) in list.iter().enumerate() {
            prefix.push(i);
            if l.kind == LayerKind::Group {
                walk(&l.children, prefix, doc, out);
            } else if l.kind == LayerKind::Fractal && l.visible && l.auto_iterations {
                out.push((prefix.clone(), key(doc, l)));
            }
            prefix.pop();
        }
    }
    let mut out = Vec::new();
    walk(&doc.layers, &mut Vec::new(), doc, &mut out);
    out
}

/// What the probe of a layer depends on.
fn key(doc: &Document, l: &Layer) -> String {
    format!(
        "{:?}|{}x{}|{}|{:?}|{}|{}|{:?}|{:?}",
        doc.view, doc.image.width, doc.image.height, l.formula, l.params, l.max_iter, l.julia, l.seed, l.transforms
    )
}

impl AutoIterations {
    /// Starts probes when the state settled, applies finished ones; returns what changed.
    pub fn update(&mut self, doc: &mut Document, ctx: &egui::Context, busy: bool) -> Vec<String> {
        let mut messages = Vec::new();
        if let Some(rx) = &self.rx
            && let Ok(results) = rx.try_recv()
        {
            self.rx = None;
            for p in results {
                let current = doc.layer(&p.path).map(|l| key(doc, l));
                let Some(l) = doc.layer_mut(&p.path) else { continue };
                match p.result {
                    // Only while the layer is still in the probed state.
                    Ok(n) if current.as_deref() == Some(p.key.as_str()) && n > l.max_iter => {
                        messages.push(format!("{}: iterations raised from {} to {n} for this view (Auto)", l.name, l.max_iter));
                        l.max_iter = n;
                    }
                    Ok(_) => {}
                    Err(e) => messages.push(format!("{}: automatic iterations skipped: {e}", l.name)),
                }
            }
        }
        let layers = auto_layers(doc);
        if layers.is_empty() {
            self.seen = None;
            return messages;
        }
        let state: String = layers.iter().map(|(_, k)| k.as_str()).collect::<Vec<_>>().join("\n");
        let since = match &self.seen {
            Some((s, t)) if *s == state => *t,
            _ => {
                self.seen = Some((state.clone(), Instant::now()));
                Instant::now()
            }
        };
        if self.probed.as_ref() == Some(&state) || self.rx.is_some() {
            return messages;
        }
        if busy || since.elapsed() < SETTLE {
            ctx.request_repaint_after(SETTLE);
            return messages;
        }
        self.probed = Some(state);
        let (tx, rx) = channel();
        let snapshot = doc.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let results =
                layers.into_iter().map(|(path, key)| Probe { result: rfr_core::render::iterations::suggest(&snapshot, &path), path, key }).collect();
            let _ = tx.send(results);
            ctx.request_repaint();
        });
        self.rx = Some(rx);
        messages
    }
}
