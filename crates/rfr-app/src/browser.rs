// SPDX-License-Identifier: LGPL-3.0-or-later

//! The formula browser in the left panel (below the layer tree), with a search. A click puts
//! the formula on the selected layer or mask.

use egui::{RichText, Ui};
use rfr_core::doc::{Document, LayerKind};
use rfr_core::formula::library::{Entry, Set, library};

use crate::layers::{Selection, target, target_mut};
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
    pub fn show(&mut self, ui: &mut Ui, doc: &Document, sel: &Selection) -> Option<String> {
        let mut picked = None;
        let current = target(doc, sel).map(|l| l.formula.clone()).unwrap_or_default();
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

/// The document with formula `id` on the selected layer (or mask): its parameters reset, its
/// suggested iterations; colors, transformations and image settings stay. A top-level layer
/// also takes the formula's suggested view.
pub fn with_formula(doc: &Document, sel: &Selection, id: &str) -> Result<Document, String> {
    let mut d = doc.clone();
    let fresh = Document::for_formula(id);
    let f = fresh.layers.first().ok_or("unknown formula")?;
    let is_bottom = sel.path == [0] && !sel.mask;
    let l = target_mut(&mut d, sel).ok_or("select a layer first")?;
    if l.kind == LayerKind::Group {
        return Err("Select a fractal layer to give it a formula".into());
    }
    l.formula = f.formula.clone();
    l.params.clear();
    l.max_iter = f.max_iter;
    l.julia = f.julia;
    if f.julia {
        l.seed = f.seed;
    }
    if !sel.mask && l.name.starts_with("Layer") || l.name == "Mandelbrot" {
        l.name = f.name.clone();
    }
    if is_bottom || doc.layers.len() == 1 {
        d.view = fresh.view;
    }
    Ok(d)
}
