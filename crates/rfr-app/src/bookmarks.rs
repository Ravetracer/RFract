// SPDX-License-Identifier: LGPL-3.0-or-later

//! Location bookmarks: named views of the document, with thumbnails rendered in the
//! background (the explore lane's renderer).

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::mpsc::{Receiver, Sender, channel};

use egui::RichText;
use rfr_core::doc::{Bookmark, Document};
use rfr_gpu::GpuContext;

use crate::explorer::{THUMB_H, THUMB_W};
use crate::{icons, theme, widgets};

#[derive(Default)]
pub struct Bookmarks {
    pub open: bool,
    thumbs: HashMap<u64, egui::TextureHandle>,
    pending: HashSet<u64>,
    jobs: Option<Sender<(u64, Document)>>,
    results: Option<Receiver<(u64, Option<Vec<u8>>)>>,
}

/// What a bookmark's thumbnail depends on: its view and the document's look.
fn key(doc: &Document, b: &Bookmark) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    format!("{:?}|{:?}|{:?}|{:?}", b.view, doc.layers, doc.post, doc.image.background).hash(&mut h);
    h.finish()
}

/// Adds the current view as a bookmark; returns the message.
pub fn add_current(doc: &mut Document) -> String {
    let n = doc.bookmarks.len() + 1;
    let name = format!("Location {n}");
    doc.bookmarks.push(Bookmark { name: name.clone(), view: doc.view.clone() });
    format!("Bookmark '{name}' added (View › Bookmarks)")
}

impl Bookmarks {
    fn worker(&mut self, ctx: &egui::Context, gpu: Option<GpuContext>) -> Sender<(u64, Document)> {
        if let Some(tx) = &self.jobs {
            return tx.clone();
        }
        let (tx, rx) = channel::<(u64, Document)>();
        let (rtx, rrx) = channel();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let mut r = gpu.map(rfr_gpu::GpuRenderer::new);
            while let Ok((k, d)) = rx.recv() {
                let img = crate::explorer::render(&mut r, &d).map(|(img, ..)| img);
                if rtx.send((k, img)).is_err() {
                    break;
                }
                repaint.request_repaint();
            }
        });
        self.jobs = Some(tx.clone());
        self.results = Some(rrx);
        tx
    }

    /// The bookmarks window; returns a message for the status bar.
    pub fn show(&mut self, ctx: &egui::Context, doc: &mut Document, gpu: Option<GpuContext>) -> Option<String> {
        if let Some(rx) = &self.results {
            while let Ok((k, img)) = rx.try_recv() {
                self.pending.remove(&k);
                if let Some(px) = img {
                    let image = egui::ColorImage::from_rgba_unmultiplied([THUMB_W as usize, THUMB_H as usize], &px);
                    self.thumbs.insert(k, ctx.load_texture(format!("bookmark-{k}"), image, Default::default()));
                }
            }
        }
        if !self.open {
            return None;
        }
        let mut msg = None;
        let mut open = self.open;
        let (mut go, mut remove, mut swap) = (None, None, None);
        egui::Window::new("Bookmarks").open(&mut open).default_width(360.0).resizable(true).show(ctx, |ui| {
            if ui
                .button(format!("{}  Add the current view", icons::PLUS))
                .on_hover_text("Save the view as a bookmark of this document (Ctrl+B)")
                .clicked()
            {
                msg = Some(add_current(doc));
            }
            if doc.bookmarks.is_empty() {
                ui.label(
                    RichText::new("No bookmarks yet. They are saved with the document; click a thumbnail to go back to its view.")
                        .color(theme::TEXT_MUTED),
                );
                return;
            }
            let n = doc.bookmarks.len();
            let current = doc.view.clone();
            egui::ScrollArea::vertical().max_height(520.0).show(ui, |ui| {
                for i in 0..n {
                    let k = key(doc, &doc.bookmarks[i]);
                    if !self.thumbs.contains_key(&k) && !self.pending.contains(&k) {
                        let mut d = doc.clone();
                        d.view = doc.bookmarks[i].view.clone();
                        d.animation = None;
                        let _ = self.worker(ctx, gpu.clone()).send((k, d));
                        self.pending.insert(k);
                    }
                    let b = &mut doc.bookmarks[i];
                    ui.horizontal(|ui| {
                        let size = egui::vec2(THUMB_W as f32 * 0.5, THUMB_H as f32 * 0.5);
                        let r = match self.thumbs.get(&k) {
                            Some(t) => ui.add(egui::Image::new(t).fit_to_exact_size(size).sense(egui::Sense::click())),
                            None => ui.add_sized(size, egui::Button::new("…")),
                        };
                        if r.on_hover_text("Go to this view").clicked() {
                            go = Some(i);
                        }
                        ui.vertical(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut b.name).desired_width(170.0)).on_hover_text("Name of the bookmark");
                            ui.label(
                                RichText::new(format!("zoom {}", crate::app::fmt_zoom(b.view.magnification())))
                                    .color(theme::TEXT_MUTED)
                                    .size(theme::fs(11.0)),
                            );
                            ui.horizontal(|ui| {
                                if ui.small_button("Update").on_hover_text("Set this bookmark to the current view").clicked() {
                                    b.view = current.clone();
                                }
                                if i > 0 && widgets::icon_button(ui, icons::ARROW_UP, "Move up").clicked() {
                                    swap = Some((i, i - 1));
                                }
                                if i + 1 < n && widgets::icon_button(ui, icons::ARROW_DOWN, "Move down").clicked() {
                                    swap = Some((i, i + 1));
                                }
                                if widgets::icon_button(ui, icons::TRASH, "Remove this bookmark").clicked() {
                                    remove = Some(i);
                                }
                            });
                        });
                    });
                    ui.separator();
                }
            });
        });
        self.open = open;
        if let Some(i) = go {
            doc.view = doc.bookmarks[i].view.clone();
            msg = Some(format!("View: {}", doc.bookmarks[i].name));
        }
        if let Some((a, b)) = swap {
            doc.bookmarks.swap(a, b);
        }
        if let Some(i) = remove {
            let b = doc.bookmarks.remove(i);
            msg = Some(format!("Bookmark '{}' removed (Ctrl+Z brings it back)", b.name));
        }
        msg
    }
}
