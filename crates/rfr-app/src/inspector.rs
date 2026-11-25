// SPDX-License-Identifier: LGPL-3.0-or-later

//! The right panel: settings of the active layer (formula parameters, iterations, Julia mode,
//! outside and inside coloring), the view and the image.

use egui::{RichText, Ui};
use rfr_core::doc::{Coloring, Document, InsideMethod, OutsideMethod, Transfer};
use rfr_core::formula::ir::{Func, Param};
use rfr_core::formula::library::library;
use rfr_core::formula::{ParamKind, ParamValue};
use rfr_core::render::resolve_value;

use crate::{theme, widgets};

/// What the inspector asks the app to do.
pub enum Action {
    Message(String),
    /// Start picking a Julia seed in the view.
    PickJulia,
    /// Switch the Julia mode on (with the current seed) or off.
    Julia(bool),
    /// Open the gradient editor of the outside (false) or inside (true) coloring.
    EditGradient(bool),
}

/// Function names offered for function parameters.
fn func_names() -> impl Iterator<Item = &'static str> {
    Func::UNARY.iter().map(|(n, _)| *n).filter(|n| *n != "identity")
}

fn param_widget(ui: &mut Ui, p: &Param, value: &mut ParamValue) -> bool {
    let tip = if p.hint.is_empty() { p.caption.clone() } else { p.hint.clone() };
    widgets::label(ui, &p.caption, &tip);
    let before = value.clone();
    match (p.kind, &mut *value) {
        (ParamKind::Float, ParamValue::Float(v)) => {
            let r = match (p.min, p.max) {
                (Some(lo), Some(hi)) if hi > lo => ui.add(egui::Slider::new(v, lo..=hi).clamping(egui::SliderClamping::Never).max_decimals(6)),
                _ => {
                    let speed = (v.abs() * 0.01).max(1e-4);
                    let mut d = egui::DragValue::new(v).speed(speed).max_decimals(10);
                    if let Some(lo) = p.min {
                        d = d.range(lo..=f64::INFINITY);
                    }
                    ui.add(d)
                }
            };
            r.on_hover_text(&tip);
        }
        (ParamKind::Int, ParamValue::Int(v)) if !p.items.is_empty() => {
            let i = (*v).clamp(0, p.items.len() as i64 - 1) as usize;
            egui::ComboBox::from_id_salt(("param-enum", &p.name)).selected_text(&p.items[i]).show_ui(ui, |ui| {
                for (k, item) in p.items.iter().enumerate() {
                    ui.selectable_value(v, k as i64, item);
                }
            });
        }
        (ParamKind::Int, ParamValue::Int(v)) => {
            let lo = p.min.map_or(i64::MIN, |m| m as i64);
            let hi = p.max.map_or(i64::MAX, |m| m as i64);
            ui.add(egui::DragValue::new(v).speed(0.05).range(lo..=hi)).on_hover_text(&tip);
        }
        (ParamKind::Complex, ParamValue::Complex(c)) => {
            ui.horizontal(|ui| {
                let speed = (c[0].abs().max(c[1].abs()) * 0.01).max(1e-4);
                ui.add(egui::DragValue::new(&mut c[0]).speed(speed).max_decimals(10).prefix("re ")).on_hover_text(&tip);
                ui.add(egui::DragValue::new(&mut c[1]).speed(speed).max_decimals(10).prefix("im ")).on_hover_text(&tip);
            });
        }
        (ParamKind::Bool, ParamValue::Bool(b)) => {
            ui.checkbox(b, "").on_hover_text(&tip);
        }
        (ParamKind::Func, ParamValue::Func(f)) => {
            egui::ComboBox::from_id_salt(("param-func", &p.name)).selected_text(f.as_str()).height(400.0).show_ui(ui, |ui| {
                for n in func_names() {
                    ui.selectable_value(f, n.to_string(), n);
                }
            });
        }
        _ => {
            ui.label(RichText::new("?").color(theme::WARN));
        }
    }
    ui.end_row();
    *value != before
}

fn coloring(ui: &mut Ui, id: &str, c: &mut Coloring, inside: bool, actions: &mut Vec<Action>) {
    widgets::label(ui, "Gradient", "Colors along the coloring value. Click to edit.");
    let w = ui.available_width().clamp(120.0, 260.0);
    if widgets::gradient_strip(ui, &c.gradient, w, 18.0).on_hover_text("Click to edit the gradient").clicked() {
        actions.push(Action::EditGradient(inside));
    }
    ui.end_row();
    widgets::label(
        ui,
        "Transfer",
        "Maps the coloring value before density and offset: square root and logarithm spread the colors where values grow quickly",
    );
    egui::ComboBox::from_id_salt((id, "transfer")).selected_text(c.transfer.label()).show_ui(ui, |ui| {
        for t in Transfer::ALL {
            ui.selectable_value(&mut c.transfer, t, t.label());
        }
    });
    ui.end_row();
    let d = Coloring::default();
    widgets::slider_log(
        ui,
        "Density",
        "Gradient repetitions per unit of the transferred value: higher values give more, narrower color bands",
        &mut c.density,
        0.001..=100.0,
        d.density,
    );
    widgets::slider(ui, "Offset", "Shifts the colors along the gradient (1 is a full turn)", &mut c.offset, 0.0..=1.0, 0.0);
}

/// Draws the inspector; returns requests for the app.
pub fn show(ui: &mut Ui, doc: &mut Document) -> Vec<Action> {
    let mut actions = Vec::new();
    let Some(li) = doc.layers.iter().position(|l| l.visible) else {
        ui.label("No visible layer.");
        return actions;
    };
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        let layer = &mut doc.layers[li];
        let entry = library().get(&layer.formula);
        // ---- formula ----
        let sec = widgets::section(ui, "Formula", true);
        if sec.reset {
            layer.params.clear();
            actions.push(Action::Message("Formula parameters reset to their defaults".into()));
        }
        if sec.open {
            match entry {
                Some(e) => {
                    ui.label(RichText::new(&e.title).strong().size(theme::fs(14.5)));
                    ui.label(RichText::new(format!("{} · {}", e.set.label(), e.family)).size(theme::fs(11.0)).color(theme::TEXT_MUTED))
                        .on_hover_text(&e.id);
                }
                None => {
                    ui.label(RichText::new(format!("Unknown formula '{}'", layer.formula)).color(theme::WARN));
                }
            }
            let program = entry.and_then(|e| e.program.as_ref().ok()).cloned();
            widgets::grid(ui, "formula-params", |ui| {
                if let Some(p) = &program {
                    for param in &p.params {
                        let mut v = resolve_value(param.kind, &param.default, layer.params.get(&param.name));
                        if param_widget(ui, param, &mut v) {
                            layer.params.insert(param.name.clone(), v);
                        }
                    }
                }
                widgets::label(ui, "Iterations", "Most iterations per point. Deep views and thin structures need more; more iterations take longer");
                let r = ui.add(egui::Slider::new(&mut layer.max_iter, 10..=100_000).logarithmic(true).clamping(egui::SliderClamping::Never));
                r.on_hover_text("Most iterations per point (double-click: 500)");
                ui.end_row();
            });
            if let Some(p) = &program
                && p.has_julia
            {
                widgets::grid(ui, "formula-julia", |ui| {
                    widgets::label(
                        ui,
                        "Julia mode",
                        "Iterate every point with the fixed seed below instead of the point itself (the Julia set of the seed)",
                    );
                    let mut on = layer.julia;
                    if ui.checkbox(&mut on, "").on_hover_text("Julia set of the seed (J in the view switches)").changed() {
                        actions.push(Action::Julia(on));
                    }
                    ui.end_row();
                    if layer.julia {
                        widgets::label(ui, "Seed", "The constant of the Julia set");
                        ui.horizontal(|ui| {
                            ui.add(egui::DragValue::new(&mut layer.seed[0]).speed(0.001).max_decimals(12).prefix("re "))
                                .on_hover_text("Real part of the seed");
                            ui.add(egui::DragValue::new(&mut layer.seed[1]).speed(0.001).max_decimals(12).prefix("im "))
                                .on_hover_text("Imaginary part of the seed");
                        });
                        ui.end_row();
                    }
                });
                if ui
                    .button("Pick a Julia seed in the view")
                    .on_hover_text("The next click in the view shows the Julia set of that point (Alt-click does it directly)")
                    .clicked()
                {
                    actions.push(Action::PickJulia);
                }
            }
        }
        // ---- outside ----
        let sec = widgets::section(ui, "Outside", true);
        if sec.reset {
            let d = rfr_core::doc::Layer::default();
            layer.outside_method = d.outside_method;
            layer.outside = d.outside;
        }
        if sec.open {
            widgets::segmented(
                ui,
                &mut layer.outside_method,
                &[
                    (OutsideMethod::Smooth, "Smooth", "Continuous iteration count: smooth color bands"),
                    (OutsideMethod::Iterations, "Bands", "Iteration count: one band per iteration"),
                    (
                        OutsideMethod::Distance,
                        "Distance",
                        "Estimated distance to the set (relative to the pixel size): bright thin filaments; deep views fall back to Smooth",
                    ),
                    (OutsideMethod::Solid, "Solid", "One color"),
                ],
            );
            widgets::grid(ui, "outside-grid", |ui| {
                if layer.outside_method == OutsideMethod::Solid {
                    widgets::color(ui, "Color", "Color of every point that escapes", &mut layer.outside.solid);
                } else {
                    coloring(ui, "outside", &mut layer.outside, false, &mut actions);
                }
            });
        }
        // ---- inside ----
        let sec = widgets::section(ui, "Inside", true);
        if sec.reset {
            let d = rfr_core::doc::Layer::default();
            layer.inside_method = d.inside_method;
            layer.inside = d.inside;
        }
        if sec.open {
            widgets::segmented(
                ui,
                &mut layer.inside_method,
                &[
                    (InsideMethod::Solid, "Solid", "One color for the set"),
                    (InsideMethod::Magnitude, "Magnitude", "|z| after the last iteration"),
                    (InsideMethod::Angle, "Angle", "Direction of z after the last iteration"),
                ],
            );
            widgets::grid(ui, "inside-grid", |ui| {
                if layer.inside_method == InsideMethod::Solid {
                    widgets::color(ui, "Color", "Color of the points that never escape", &mut layer.inside.solid);
                } else {
                    coloring(ui, "inside", &mut layer.inside, true, &mut actions);
                }
            });
        }
        // ---- view ----
        let sec = widgets::section(ui, "View", true);
        if sec.reset
            && let Some(e) = library().get(&doc.layers[li].formula)
        {
            doc.view.radius = e.view.radius;
            doc.view.set_center(e.view.center);
            doc.view.rotation = 0.0;
            if doc.layers[li].julia {
                doc.view.set_center([0.0, 0.0]);
                doc.view.radius = 1.6;
            }
        }
        if sec.open {
            widgets::grid(ui, "view-grid", |ui| {
                for (k, axis) in ["Center x", "Center y"].iter().enumerate() {
                    widgets::label(ui, axis, "Center of the view (any number of digits)");
                    let mut s = doc.view.center[k].clone();
                    let r = ui.add(egui::TextEdit::singleline(&mut s).desired_width(f32::INFINITY).font(egui::TextStyle::Monospace));
                    if r.changed() && s.trim().parse::<f64>().is_ok() {
                        doc.view.center[k] = s.trim().to_string();
                    }
                    r.on_hover_text("Center of the view; type a number");
                    ui.end_row();
                }
                widgets::label(ui, "Zoom", "Magnification: 1 shows about ±1.5 vertically");
                let mut zoom = 1.5 / doc.view.radius;
                let speed = zoom * 0.01;
                let r = ui.add(egui::DragValue::new(&mut zoom).speed(speed).range(1e-6..=1e300).custom_formatter(|v, _| format!("{v:.4e}×")));
                if r.changed() && zoom > 0.0 {
                    doc.view.radius = 1.5 / zoom;
                }
                r.on_hover_text("Magnification (drag to change; the wheel in the view zooms at the cursor)");
                ui.end_row();
                let mut rot = if doc.view.rotation > 180.0 { doc.view.rotation - 360.0 } else { doc.view.rotation };
                if widgets::slider(ui, "Rotation", "Rotation of the view in degrees (Ctrl-drag in the view)", &mut rot, -180.0..=180.0, 0.0).changed()
                {
                    doc.view.rotation = rot.rem_euclid(360.0);
                }
            });
        }
        // ---- image ----
        let sec = widgets::section(ui, "Image", false);
        if sec.open {
            widgets::grid(ui, "image-grid", |ui| {
                widgets::label(ui, "Size", "Size of exported images in pixels; the view shows this aspect ratio");
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut doc.image.width).range(16..=32768).speed(4)).on_hover_text("Width in pixels");
                    ui.label("×");
                    ui.add(egui::DragValue::new(&mut doc.image.height).range(16..=32768).speed(4)).on_hover_text("Height in pixels");
                    ui.menu_button("…", |ui| {
                        for (w, h, name) in [
                            (1280, 720, "HD 720p"),
                            (1920, 1080, "Full HD"),
                            (2560, 1440, "QHD"),
                            (3840, 2160, "4K UHD"),
                            (7680, 4320, "8K"),
                            (1080, 1080, "Square 1080"),
                            (2160, 3840, "4K portrait"),
                            (2480, 3508, "A4 300 dpi"),
                        ] {
                            if ui.button(format!("{name}  {w} × {h}")).clicked() {
                                doc.image.width = w;
                                doc.image.height = h;
                                ui.close();
                            }
                        }
                    })
                    .response
                    .on_hover_text("Common sizes");
                });
                ui.end_row();
                widgets::label(ui, "Samples", "Samples per pixel of exported images: more samples give smoother edges (anti-aliasing)");
                ui.add(egui::Slider::new(&mut doc.image.samples, 1..=4096).logarithmic(true)).on_hover_text("Samples per pixel for exports");
                ui.end_row();
            });
        }
    });
    actions
}
