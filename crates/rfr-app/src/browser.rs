// SPDX-License-Identifier: LGPL-3.0-or-later

//! The left panel: the layer (one in this version) and the formula browser with a search.

use egui::{RichText, Ui};
use rfr_core::doc::Document;
use rfr_core::formula::library::{Entry, Set, library};

use crate::{icons, theme};

#[derive(Default)]
pub struct Browser {
    search: String,
}

fn matches(e: &Entry, q: &str) -> bool {
    q.is_empty() || e.title.to_lowercase().contains(q) || e.family.to_lowercase().contains(q) || e.id.to_lowercase().contains(q)
}

fn item(ui: &mut Ui, e: &Entry, current: &str) -> bool {
    let julia = e.program.as_ref().is_ok_and(|p| p.has_julia);
    let tip = format!(
        "{}\n{} · {}{}\nClick to use this formula (with its suggested view).",
        e.id,
        e.set.label(),
        e.family,
        if julia { " · has a Julia mode" } else { "" }
    );
    ui.selectable_label(e.id == current, &e.title).on_hover_text(tip).clicked()
}

impl Browser {
    /// Draws the panel; returns the formula id the user picked.
    pub fn show(&mut self, ui: &mut Ui, doc: &Document) -> Option<String> {
        let mut picked = None;
        let current = doc.active_layer().map(|l| l.formula.clone()).unwrap_or_default();
        ui.horizontal(|ui| {
            ui.label(RichText::new(icons::LAYERS).color(theme::ACCENT).size(theme::fs(15.0)));
            ui.label(RichText::new("Layer").strong());
        });
        if let Some(l) = doc.active_layer() {
            let title = library().get(&l.formula).map(|e| e.title.clone()).unwrap_or_else(|| l.formula.clone());
            egui::Frame::new().fill(theme::SURFACE).corner_radius(6).inner_margin(6).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(&title).strong()).on_hover_text("The active layer's formula; more layers come with the layer stack");
                ui.label(RichText::new(if l.julia { "Julia mode" } else { "Mandelbrot mode" }).size(theme::fs(11.0)).color(theme::TEXT_MUTED));
            });
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(icons::FUNCTION).color(theme::ACCENT).size(theme::fs(15.0)));
            ui.label(RichText::new("Formulas").strong());
        });
        ui.add(
            egui::TextEdit::singleline(&mut self.search).hint_text(format!("{}  Search name or family", icons::SEARCH)).desired_width(f32::INFINITY),
        )
        .on_hover_text("Filter the formulas by name, family or id");
        ui.add_space(4.0);
        let q = self.search.trim().to_lowercase();
        let lib = library();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let builtin: Vec<&Entry> = lib.usable().filter(|e| e.set == Set::RFract && matches(e, &q)).collect();
            if !builtin.is_empty() {
                egui::CollapsingHeader::new(RichText::new(format!("RFract ({})", builtin.len())).strong())
                    .id_salt("set-rfract")
                    .default_open(true)
                    .show(ui, |ui| {
                        for e in builtin {
                            if item(ui, e, &current) {
                                picked = Some(e.id.clone());
                            }
                        }
                    });
            }
            let mut families: Vec<&str> = lib.usable().filter(|e| e.set == Set::FractalPark).map(|e| e.family.as_str()).collect();
            families.sort();
            families.dedup();
            let total = lib.usable().filter(|e| e.set == Set::FractalPark && matches(e, &q)).count();
            if total > 0 {
                egui::CollapsingHeader::new(RichText::new(format!("FractalPark ({total})")).strong())
                    .id_salt("set-fractalpark")
                    .default_open(true)
                    .show(ui, |ui| {
                        for fam in families {
                            let mut list: Vec<&Entry> =
                                lib.usable().filter(|e| e.set == Set::FractalPark && e.family == fam && matches(e, &q)).collect();
                            if list.is_empty() {
                                continue;
                            }
                            list.sort_by_key(|e| e.title.to_lowercase());
                            egui::CollapsingHeader::new(format!("{fam} ({})", list.len())).id_salt(("fam", fam)).default_open(!q.is_empty()).show(
                                ui,
                                |ui| {
                                    for e in list {
                                        if item(ui, e, &current) {
                                            picked = Some(e.id.clone());
                                        }
                                    }
                                },
                            );
                        }
                    })
                    .header_response
                    .on_hover_text("Formulas from the FractalPark project (MIT license)");
            }
            if !q.is_empty() && picked.is_none() && lib.usable().all(|e| !matches(e, &q)) {
                ui.label(RichText::new("No formula matches.").color(theme::TEXT_MUTED));
            }
        });
        picked
    }
}

/// The document with formula `id` on the active layer: its parameters reset, its suggested view
/// and iterations; colors and image settings stay.
pub fn with_formula(doc: &Document, id: &str) -> Document {
    let mut d = doc.clone();
    let fresh = Document::for_formula(id);
    d.view = fresh.view;
    if let (Some(l), Some(f)) = (d.layers.iter_mut().find(|l| l.visible), fresh.layers.first()) {
        l.formula = f.formula.clone();
        l.params.clear();
        l.max_iter = f.max_iter;
        l.julia = f.julia;
        l.seed = if f.julia { f.seed } else { l.seed };
        l.name = f.name.clone();
    }
    d
}
