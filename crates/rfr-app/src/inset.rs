// SPDX-License-Identifier: LGPL-3.0-or-later

//! Julia preview inset: a small image of the Julia set of the point under the cursor for the
//! selected layer, rendered on the CPU in the background. Only the newest request renders; a
//! request that comes while one renders waits for it.

use std::sync::mpsc::{Receiver, channel};

use rfr_core::doc::{Document, Layer, LayerKind, MergeMode, View};

/// Iteration limit of the preview (the whole Julia set needs fewer than deep views).
const MAX_ITER: u32 = 1000;

/// The view a Julia set opens with (`Viewport::set_julia` uses the same).
pub fn julia_view() -> View {
    View { center: ["0".into(), "0".into()], radius: 1.6.into(), rotation: 0.0 }
}

/// The document of the preview: `layer` alone in Julia mode with `seed`, `size` pixels square.
fn julia_doc(doc: &Document, layer: &Layer, seed: [f64; 2], size: u32) -> Document {
    let mut l = layer.clone();
    l.julia = true;
    l.seed = seed;
    l.mask = None;
    l.visible = true;
    l.opacity = 1.0;
    l.merge = MergeMode::Normal;
    l.light.enabled = false;
    l.effects = Default::default();
    l.auto_iterations = false;
    l.max_iter = l.max_iter.min(MAX_ITER);
    let mut d = Document { layers: vec![l], lights: Vec::new(), bookmarks: Vec::new(), animation: None, view: julia_view(), ..doc.clone() };
    d.image.width = size;
    d.image.height = size;
    d
}

/// Whether `layer` can show a Julia preview: a fractal layer, not in Julia mode, whose formula
/// has a Julia mode.
pub fn applies(layer: &Layer) -> bool {
    layer.kind == LayerKind::Fractal
        && !layer.julia
        && rfr_core::formula::library::library().get(&layer.formula).and_then(|e| e.program_with(&layer.params).ok()).is_some_and(|p| p.has_julia)
}

/// What a request depends on (besides the seed).
fn key(doc: &Document, layer: &Layer, size: u32) -> String {
    format!("{size}|{:?}|{:?}", doc.image.background, layer)
}

/// The result of a render: the side in pixels and the RGBA bytes.
type Rendered = Result<(u32, Vec<u8>), String>;

#[derive(Default)]
pub struct JuliaInset {
    /// The seed and state shown in the texture.
    shown: Option<([f64; 2], String)>,
    texture: Option<egui::TextureHandle>,
    /// The request rendering now and the one waiting.
    running: Option<([f64; 2], String, Receiver<Rendered>)>,
    waiting: Option<(Document, Layer, [f64; 2], u32)>,
    /// The last error (shown in the inset).
    pub error: Option<String>,
}

impl JuliaInset {
    /// Asks for the preview of `seed`; returns the texture to show (possibly of an older seed).
    pub fn update(&mut self, ctx: &egui::Context, doc: &Document, layer: &Layer, seed: [f64; 2], size: u32) -> Option<&egui::TextureHandle> {
        let k = key(doc, layer, size);
        if let Some((s, rk, rx)) = &self.running
            && let Ok(result) = rx.try_recv()
        {
            match result {
                Ok((n, rgba)) => {
                    let img = egui::ColorImage::from_rgba_unmultiplied([n as usize, n as usize], &rgba);
                    match &mut self.texture {
                        Some(t) => t.set(img, egui::TextureOptions::LINEAR),
                        None => self.texture = Some(ctx.load_texture("julia-inset", img, egui::TextureOptions::LINEAR)),
                    }
                    self.shown = Some((*s, rk.clone()));
                    self.error = None;
                }
                Err(e) => self.error = Some(e),
            }
            self.running = None;
        }
        let wanted = |s: &[f64; 2], rk: &str| *s == seed && rk == k;
        let current = self.shown.as_ref().is_some_and(|(s, rk)| wanted(s, rk)) || self.running.as_ref().is_some_and(|(s, rk, _)| wanted(s, rk));
        if !current {
            self.waiting = Some((doc.clone(), layer.clone(), seed, size));
        }
        if self.running.is_none()
            && let Some((d, l, s, n)) = self.waiting.take()
        {
            let (tx, rx) = channel();
            let doc = julia_doc(&d, &l, s, n);
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let r = rfr_core::render::cpu::render(&doc, n, n, 1).map(|img| (n, img.rgba8()));
                let _ = tx.send(r);
                ctx.request_repaint();
            });
            self.running = Some((s, key(&d, &l, n), rx));
        }
        self.texture.as_ref()
    }

    /// The seed of the image shown.
    pub fn shown_seed(&self) -> Option<[f64; 2]> {
        self.shown.as_ref().map(|(s, _)| *s)
    }
}
