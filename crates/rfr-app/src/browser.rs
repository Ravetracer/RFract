// SPDX-License-Identifier: LGPL-3.0-or-later

//! The formula browser in the left panel (below the layer tree), with a search. A click puts
//! the formula on the selected layer or mask.

use egui::{RichText, Ui};
use rfr_core::doc::{Document, LayerKind};
use rfr_core::formula::library::{Entry, Set, library};

use crate::layers::{Selection, target, target_mut};
use crate::{icons, theme};

/// Files with the ids of their matching formulas.
type FileGroups = Vec<(String, Vec<String>)>;

#[derive(Default)]
pub struct Browser {
    search: String,
    /// User formulas grouped by file: (library generation, search) → (file, entry ids).
    user_groups: Option<((u64, String), FileGroups)>,
    /// A new formula of this kind was asked for (the app creates and opens it).
    pub new_formula: Option<rfr_core::formula::ProgramKind>,
}

fn matches(e: &Entry, q: &str) -> bool {
    q.is_empty() || e.title.to_lowercase().contains(q) || e.family.to_lowercase().contains(q) || e.id.to_lowercase().contains(q)
}

fn item(ui: &mut Ui, e: &Entry, current: &str) -> bool {
    // Only formulas compiled already (listing must not compile user formulas).
    let julia = e.status() == Some(true) && e.program().is_ok_and(|p| p.has_julia);
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
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.menu_button(icons::PLUS, |ui| {
                    use rfr_core::formula::ProgramKind;
                    for (label, kind) in [
                        ("Fractal formula", ProgramKind::Fractal),
                        ("Coloring formula", ProgramKind::Coloring),
                        ("Transformation", ProgramKind::Transform),
                        ("3D formula", ProgramKind::Object),
                    ] {
                        if ui
                            .button(label)
                            .on_hover_text("Create a new formula of this kind in your first formula folder and open it in the editor")
                            .clicked()
                        {
                            self.new_formula = Some(kind);
                            ui.close();
                        }
                    }
                })
                .response
                .on_hover_text("New formula (in your first formula folder)");
            });
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
                        for e in builtin.iter().filter(|e| e.family == "RFract") {
                            if item(ui, e, &current) {
                                picked = Some(e.id.clone());
                            }
                        }
                        let fractint: Vec<&&Entry> = builtin.iter().filter(|e| e.family == "FractInt types").collect();
                        if !fractint.is_empty() {
                            egui::CollapsingHeader::new(format!("FractInt types ({})", fractint.len()))
                                .id_salt(("fractint-types", &q))
                                .default_open(!q.is_empty())
                                .show(ui, |ui| {
                                    for e in fractint {
                                        if item(ui, e, &current) {
                                            picked = Some(e.id.clone());
                                        }
                                    }
                                })
                                .header_response
                                .on_hover_text("FractInt's escape-time types with their parameter layout (for .par files)");
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
                            egui::CollapsingHeader::new(format!("{fam} ({})", list.len()))
                                .id_salt(("fam", fam, q.is_empty()))
                                .default_open(!q.is_empty())
                                .show(ui, |ui| {
                                    for e in list {
                                        if item(ui, e, &current) {
                                            picked = Some(e.id.clone());
                                        }
                                    }
                                });
                        }
                    })
                    .header_response
                    .on_hover_text("Formulas from the FractalPark project (MIT license)");
            }
            let key = (rfr_core::formula::library::generation(), q.clone());
            if self.user_groups.as_ref().is_none_or(|(k, _)| *k != key) {
                let mut groups: FileGroups = Vec::new();
                for e in lib.usable().filter(|e| e.set == Set::User && matches(e, &q)) {
                    match groups.last_mut() {
                        Some((fam, ids)) if *fam == e.family => ids.push(e.id.clone()),
                        _ => groups.push((e.family.clone(), vec![e.id.clone()])),
                    }
                }
                self.user_groups = Some((key, groups));
            }
            if let Some((_, groups)) = &self.user_groups
                && !groups.is_empty()
            {
                let total: usize = groups.iter().map(|(_, ids)| ids.len()).sum();
                egui::CollapsingHeader::new(RichText::new(format!("User formulas ({total})")).strong())
                    .id_salt("set-user")
                    .default_open(true)
                    .show(ui, |ui| {
                        for (file, ids) in groups {
                            egui::CollapsingHeader::new(format!("{file} ({})", ids.len()))
                                .id_salt(("user-file", file, &q))
                                .default_open(!q.is_empty() && total < 200)
                                .show(ui, |ui| {
                                    for id in ids {
                                        if let Some(e) = lib.get(id)
                                            && item(ui, e, &current)
                                        {
                                            picked = Some(e.id.clone());
                                        }
                                    }
                                });
                        }
                    })
                    .header_response
                    .on_hover_text("Formulas from your formula folders (File › Preferences). Each one compiles when you pick it.");
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
