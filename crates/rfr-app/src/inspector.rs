// SPDX-License-Identifier: LGPL-3.0-or-later

//! The right panel: settings of the selected layer, group or mask (merge mode, opacity, mask;
//! formula parameters, iterations, Julia mode, transformations, outside and inside coloring),
//! the view and the image.

use std::collections::BTreeMap;

use egui::{RichText, Ui};
use rfr_core::doc::{Coloring, Document, InsideMethod, Layer, LayerKind, MaskSource, MergeMode, OutsideMethod, Transfer, TransformRef};
use rfr_core::formula::ir::{Func, Param, Program};
use rfr_core::formula::library::{Entry, Set, library};
use rfr_core::formula::{ParamKind, ParamValue};
use rfr_core::render::resolve_value;

use crate::layers::{Selection, target_mut};
use crate::{icons, theme, widgets};

/// What the inspector asks the app to do.
pub enum Action {
    Message(String),
    /// Start picking a Julia seed in the view.
    PickJulia,
    /// Switch the Julia mode on (with the current seed) or off.
    Julia(bool),
    /// Open the gradient editor of the outside (false) or inside (true) coloring.
    EditGradient(bool),
    /// Select this item in the layer tree.
    Select(Selection),
    /// Open the formula editor on this library entry.
    EditFormula(String),
    /// Fit the view to the selected density layer's attractor or strokes.
    FitDensity,
}

/// Function names offered for function parameters.
fn func_names() -> impl Iterator<Item = &'static str> {
    Func::UNARY.iter().map(|(n, _)| *n).filter(|n| *n != "identity")
}

fn param_widget(ui: &mut Ui, salt: &str, p: &Param, value: &mut ParamValue) -> bool {
    let tip = if p.hint.is_empty() { p.caption.clone() } else { p.hint.clone() };
    // Parameters of plugin objects (`formulaClass.power`) are indented under their object.
    let depth = p.name.matches('.').count();
    let caption = format!("{}{}", "    ".repeat(depth), p.caption);
    widgets::label(ui, &caption, &tip);
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
            egui::ComboBox::from_id_salt((salt, "param-enum", &p.name)).selected_text(&p.items[i]).show_ui(ui, |ui| {
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
            // Color function parameters choose a merge mode.
            let merge = matches!(&p.default, ParamValue::Func(d) if rfr_core::formula::color::merge_mode(d).is_some());
            egui::ComboBox::from_id_salt((salt, "param-func", &p.name)).selected_text(f.as_str()).height(400.0).show_ui(ui, |ui| {
                if merge {
                    for n in rfr_core::formula::color::MERGE_NAMES {
                        ui.selectable_value(f, n.to_string(), n);
                    }
                } else {
                    for n in func_names() {
                        ui.selectable_value(f, n.to_string(), n);
                    }
                }
            });
        }
        (ParamKind::Class, ParamValue::Class { class }) => {
            // An object parameter: the classes derived from its base class.
            let reg = rfr_core::formula::library::class_registry();
            let base = p.base.clone().unwrap_or_default();
            let choices = reg.derived(&base);
            let label = if class.is_empty() { "(none)".to_string() } else { class.clone() };
            let search_id = egui::Id::new((salt, "param-class-search", &p.name));
            egui::ComboBox::from_id_salt((salt, "param-class", &p.name))
                .selected_text(label)
                .height(400.0)
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                .show_ui(ui, |ui| {
                    let mut q: String = ui.data_mut(|d| d.get_temp(search_id).unwrap_or_default());
                    ui.add(egui::TextEdit::singleline(&mut q).hint_text(format!("{}  Search", icons::SEARCH)).desired_width(240.0))
                        .on_hover_text("Filter the classes by name");
                    ui.data_mut(|d| d.insert_temp(search_id, q.clone()));
                    let q = q.to_lowercase();
                    for c in choices.iter().filter(|c| q.is_empty() || c.to_lowercase().contains(&q)) {
                        let file = reg.file_of(c).unwrap_or("").to_string();
                        if ui.selectable_value(class, c.clone(), c).on_hover_text(format!("{c} ({file})")).clicked() {
                            ui.close();
                        }
                    }
                })
                .response
                .on_hover_text(format!("{tip}\nThe class (plugin) to use: any class derived from {base}; its parameters follow below."));
        }
        (ParamKind::Image, ParamValue::Image { path }) => {
            ui.horizontal(|ui| {
                let name = if path.is_empty() { "(none)".to_string() } else { path.rsplit(['/', '\\']).next().unwrap_or(path).to_string() };
                let hover = if path.is_empty() {
                    format!("{tip}\nChoose an image file (PNG or JPEG).")
                } else {
                    format!("{tip}\n{path}\nClick to choose another image.")
                };
                if ui.button(format!("{} {name}", icons::OPEN)).on_hover_text(hover).clicked()
                    && let Some(f) = rfd::FileDialog::new().add_filter("Images", &["png", "jpg", "jpeg"]).pick_file()
                {
                    *path = f.display().to_string();
                }
                if !path.is_empty() {
                    if ui.small_button(icons::X).on_hover_text("Remove the image (formulas then see an empty image)").clicked() {
                        path.clear();
                    } else if let Err(e) = rfr_core::images::load(path) {
                        ui.label(RichText::new(icons::WARNING).color(theme::WARN)).on_hover_text(format!("{e}\nThe formula sees an empty image."));
                    }
                }
            });
        }
        (ParamKind::Color, ParamValue::Color(c)) => {
            // Formula colors are sRGB-encoded; the picker works on linear values.
            use rfr_core::render::composite::{srgb_decode, srgb_encode};
            let mut lin = [srgb_decode(c[0] as f32), srgb_decode(c[1] as f32), srgb_decode(c[2] as f32), c[3].clamp(0.0, 1.0) as f32];
            let before_lin = lin;
            ui.color_edit_button_rgba_unmultiplied(&mut lin).on_hover_text(&tip);
            if lin != before_lin {
                *c = [srgb_encode(lin[0]) as f64, srgb_encode(lin[1]) as f64, srgb_encode(lin[2]) as f64, lin[3] as f64];
            }
        }
        _ => {
            ui.label(RichText::new("?").color(theme::WARN));
        }
    }
    ui.end_row();
    *value != before
}

/// Controls for every parameter of `prog`, storing changed values in `map`.
fn params_ui(ui: &mut Ui, salt: &str, prog: &Program, map: &mut BTreeMap<String, ParamValue>) {
    widgets::grid(ui, &format!("{salt}-params"), |ui| {
        for param in &prog.params {
            // Class libraries keep internal version numbers as parameters.
            if param.caption.starts_with("Version (") {
                continue;
            }
            let mut v = resolve_value(param.kind, &param.default, map.get(&param.name));
            if param_widget(ui, salt, param, &mut v) {
                map.insert(param.name.clone(), v);
            }
        }
    });
}

fn solid_color(ui: &mut Ui, tip: &str, c: &mut Coloring) {
    widgets::color(ui, "Color", tip, &mut c.solid);
    let mut a = c.solid_alpha as f64;
    if widgets::slider(ui, "Opacity", "Opacity of the solid color (0 leaves these points transparent)", &mut a, 0.0..=1.0, 1.0).changed() {
        c.solid_alpha = a as f32;
    }
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

/// Entries grouped by their file (consecutive entries of one family).
fn by_file<'a>(entries: impl Iterator<Item = &'a Entry>) -> Vec<(String, Vec<&'a Entry>)> {
    let mut out: Vec<(String, Vec<&Entry>)> = Vec::new();
    for e in entries {
        match out.last_mut() {
            Some((f, list)) if *f == e.family => list.push(e),
            _ => out.push((e.family.clone(), vec![e])),
        }
    }
    out
}

/// The coloring formula picker and its parameters (Formula method).
fn coloring_formula(ui: &mut Ui, id: &str, c: &mut Coloring, actions: &mut Vec<Action>) {
    let current = library().get(&c.formula).map(|e| e.title.clone()).unwrap_or_else(|| c.formula.clone());
    widgets::grid(ui, &format!("{id}-formula"), |ui| {
        widgets::label(ui, "Algorithm", "The coloring formula; it runs along every orbit and picks the gradient position");
        egui::ComboBox::from_id_salt((id, "coloring-formula")).selected_text(current).height(400.0).width(200.0).show_ui(ui, |ui| {
            let lib = library();
            let mut pick = |ui: &mut Ui, e: &Entry| {
                if ui.selectable_label(c.formula == e.id, &e.title).clicked() && c.formula != e.id {
                    c.formula = e.id.clone();
                    c.params.clear();
                    // Coloring formulas give positions of about 0..1.
                    c.transfer = Transfer::Linear;
                    c.density = 1.0;
                }
            };
            for e in lib.colorings().filter(|e| e.set != Set::User) {
                pick(ui, e);
            }
            // User colorings, one collapsible group per file.
            for (file, list) in by_file(lib.colorings().filter(|e| e.set == Set::User)) {
                egui::CollapsingHeader::new(format!("{file} ({})", list.len())).id_salt((id, "user-coloring", file)).show(ui, |ui| {
                    for e in list {
                        pick(ui, e);
                    }
                });
            }
        });
        ui.end_row();
    });
    if let Some(e) = library().get(&c.formula) {
        ui.horizontal(|ui| {
            if ui.small_button("Source").on_hover_text("Show the coloring formula's source (and edit user formulas)").clicked() {
                actions.push(Action::EditFormula(e.id.clone()));
            }
            if let Err(err) = e.program_with(&c.params) {
                ui.label(RichText::new(format!("Does not compile: {err}")).color(theme::WARN));
            }
        });
    }
    if let Some(p) = library().get(&c.formula).and_then(|e| e.program_with(&c.params).or_else(|_| e.program()).ok()) {
        params_ui(ui, &format!("{id}-coloring"), &p, &mut c.params);
    }
}

fn layer_settings(ui: &mut Ui, l: &mut Layer, sel: &Selection, actions: &mut Vec<Action>) {
    let sec = widgets::section(ui, if l.kind == LayerKind::Group { "Group" } else { "Layer" }, false);
    if !sec.open {
        return;
    }
    widgets::grid(ui, "layer-grid", |ui| {
        widgets::label(ui, "Name", "Name of the layer in the layer list");
        ui.add(egui::TextEdit::singleline(&mut l.name).desired_width(f32::INFINITY)).on_hover_text("Name of the layer");
        ui.end_row();
        widgets::label(ui, "Merge", "How the layer combines with the layers below it");
        egui::ComboBox::from_id_salt("merge-mode")
            .selected_text(l.merge.label())
            .height(500.0)
            .show_ui(ui, |ui| {
                for m in MergeMode::ALL {
                    ui.selectable_value(&mut l.merge, m, m.label()).on_hover_text(m.tip());
                }
            })
            .response
            .on_hover_text(l.merge.tip());
        ui.end_row();
        widgets::slider(ui, "Opacity", "How strongly the layer covers what is below (0 hides it)", &mut l.opacity, 0.0..=1.0, 1.0);
    });
    match &mut l.mask {
        Some(m) => {
            ui.horizontal(|ui| {
                ui.checkbox(&mut m.enabled, "Mask").on_hover_text("Use the mask: its fractal's brightness (or alpha) limits where the layer shows");
                ui.checkbox(&mut m.invert, "Invert").on_hover_text("Show the layer where the mask is dark instead of bright");
                egui::ComboBox::from_id_salt("mask-source")
                    .selected_text(if m.source == MaskSource::Luminance { "Brightness" } else { "Alpha" })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut m.source, MaskSource::Luminance, "Brightness")
                            .on_hover_text("The mask's brightness (times its alpha) decides");
                        ui.selectable_value(&mut m.source, MaskSource::Alpha, "Alpha")
                            .on_hover_text("The mask's alpha decides (transparent gradient parts)");
                    });
            });
            if ui
                .button(format!("{}  Edit the mask", icons::MASK))
                .on_hover_text("Select the mask's fractal to change its formula and colors")
                .clicked()
            {
                actions.push(Action::Select(Selection { path: sel.path.clone(), mask: true }));
            }
        }
        None => {
            if ui.button(format!("{}  Add a mask", icons::MASK)).on_hover_text("A fractal whose brightness limits where this layer shows").clicked() {
                l.mask = Some(rfr_core::doc::Mask::default());
                actions.push(Action::Select(Selection { path: sel.path.clone(), mask: true }));
            }
        }
    }
}

fn transforms_ui(ui: &mut Ui, l: &mut Layer, actions: &mut Vec<Action>) {
    let sec = widgets::section(ui, "Transformations", false);
    if !sec.open {
        return;
    }
    if l.transforms.is_empty() {
        ui.label(RichText::new("None: the points go to the formula unchanged.").size(theme::fs(11.5)).color(theme::TEXT_MUTED));
    }
    let mut remove = None;
    let mut swap = None;
    let n = l.transforms.len();
    for (k, t) in l.transforms.iter_mut().enumerate() {
        let title = library().get(&t.formula).map(|e| e.title.clone()).unwrap_or_else(|| t.formula.clone());
        egui::Frame::new().fill(theme::SURFACE).corner_radius(6).inner_margin(6).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.checkbox(&mut t.enabled, RichText::new(&title).strong()).on_hover_text("Apply this transformation");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if widgets::icon_button(ui, icons::X, "Remove this transformation").clicked() {
                        remove = Some(k);
                    }
                    if k + 1 < n && widgets::icon_button(ui, icons::ARROW_DOWN, "Apply later").clicked() {
                        swap = Some((k, k + 1));
                    }
                    if k > 0 && widgets::icon_button(ui, icons::ARROW_UP, "Apply earlier").clicked() {
                        swap = Some((k, k - 1));
                    }
                });
            });
            if let Some(p) = library().get(&t.formula).and_then(|e| e.program_with(&t.params).or_else(|_| e.program()).ok()) {
                params_ui(ui, &format!("transform-{k}"), &p, &mut t.params);
            }
        });
    }
    if let Some(k) = remove {
        l.transforms.remove(k);
    }
    if let Some((a, b)) = swap {
        l.transforms.swap(a, b);
    }
    ui.menu_button(format!("{}  Add a transformation", icons::PLUS), |ui| {
        let lib = library();
        let mut add = |ui: &mut Ui, e: &Entry| {
            if ui.button(&e.title).on_hover_text("Changes the points before the formula runs; transformations apply in order").clicked() {
                l.transforms.push(TransformRef { formula: e.id.clone(), ..TransformRef::default() });
                actions.push(Action::Message(format!("Added the transformation {}", e.title)));
                ui.close();
            }
        };
        for e in lib.transforms().filter(|e| e.set != Set::User) {
            add(ui, e);
        }
        for (file, list) in by_file(lib.transforms().filter(|e| e.set == Set::User)) {
            ui.menu_button(format!("{file} ({})", list.len()), |ui| {
                egui::ScrollArea::vertical().max_height(400.0).show(ui, |ui| {
                    for e in list {
                        add(ui, e);
                    }
                });
            });
        }
    })
    .response
    .on_hover_text("Bend, mirror or repeat the plane before the fractal is computed");
}

/// A 3D object layer: shape, parameters, camera, colors and lighting.
fn object_sections(ui: &mut Ui, l: &mut Layer, actions: &mut Vec<Action>) {
    use rfr_core::doc::{Object3D, ObjectShape};
    let sec = widgets::section(ui, "3D object", true);
    if sec.reset {
        l.object = Object3D::default();
    }
    if sec.open {
        let o = &mut l.object;
        let before = o.shape;
        widgets::segmented(
            ui,
            &mut o.shape,
            &[
                (ObjectShape::Mandelbulb, "Mandelbulb", "z → zⁿ + c in spherical coordinates: the 3D Mandelbrot analogue"),
                (ObjectShape::Quaternion, "Quaternion", "The Julia set of q² + c over quaternions, cut at w = 0"),
                (ObjectShape::Mandelbox, "Mandelbox", "Box and sphere folds, scaled: cubes full of folded detail"),
                (ObjectShape::Menger, "Menger", "The Menger sponge: a cube with its middle thirds removed again and again"),
            ],
        );
        if o.shape != before {
            // Each shape has its own size: move the camera to see it whole.
            o.camera.distance = match o.shape {
                ObjectShape::Mandelbulb => 2.8,
                ObjectShape::Quaternion => 3.0,
                ObjectShape::Mandelbox => (2.0 * (o.scale.abs() + 1.0) / (o.scale.abs() - 1.0).max(0.05)).min(16.0) * 3.5,
                ObjectShape::Menger => 4.5,
            };
            o.iterations = if o.shape == ObjectShape::Menger { 5 } else { 10 };
        }
        let d = Object3D::default();
        widgets::grid(ui, "object-grid", |ui| {
            match o.shape {
                ObjectShape::Mandelbulb => {
                    widgets::slider(ui, "Power", "The exponent n of z → zⁿ + c (8: the classic bulb)", &mut o.power, 2.0..=16.0, d.power);
                }
                ObjectShape::Mandelbox => {
                    widgets::slider(
                        ui,
                        "Scale",
                        "The scale after the folds (2: the classic box; negative values give other shapes)",
                        &mut o.scale,
                        -3.0..=3.0,
                        d.scale,
                    );
                }
                _ => {}
            }
            widgets::slider_u32(ui, "Iterations", "Steps of the formula per point: more give finer detail", &mut o.iterations, 1..=40, d.iterations);
            if o.shape != ObjectShape::Menger {
                if o.shape != ObjectShape::Quaternion {
                    widgets::checkbox(ui, "Julia mode", "Use the seed below as the constant instead of the point itself", &mut o.julia);
                }
                if o.julia || o.shape == ObjectShape::Quaternion {
                    widgets::label(ui, "Seed", "The constant c (x, y, z; w for quaternions)");
                    ui.horizontal(|ui| {
                        let n = if o.shape == ObjectShape::Quaternion { 4 } else { 3 };
                        for (k, v) in o.seed.iter_mut().take(n).enumerate() {
                            ui.add(egui::DragValue::new(v).speed(0.002).max_decimals(4)).on_hover_text(["x", "y", "z", "w"][k]);
                        }
                    });
                    ui.end_row();
                }
            }
            widgets::slider_log(
                ui,
                "Detail",
                "How close the rays get to the surface, in pixels (smaller: finer, slower)",
                &mut o.detail,
                0.1..=10.0,
                d.detail,
            );
            let c = d.camera.clone();
            let cam = &mut o.camera;
            widgets::slider(
                ui,
                "Camera direction",
                "Where the camera looks from, in degrees around the vertical axis",
                &mut cam.yaw,
                -180.0..=180.0,
                c.yaw,
            );
            widgets::slider(ui, "Camera height", "The camera's angle above the horizontal plane in degrees", &mut cam.pitch, -89.0..=89.0, c.pitch);
            widgets::slider_log(ui, "Camera distance", "Distance of the camera from the origin", &mut cam.distance, 0.1..=100.0, c.distance);
            widgets::slider(ui, "Field of view", "Vertical opening angle of the camera in degrees", &mut cam.fov, 5.0..=120.0, c.fov);
        });
    }
    let sec = widgets::section(ui, "Colors", true);
    if sec.reset {
        l.outside = Layer::default().outside;
    }
    if sec.open {
        widgets::grid(ui, "object-colors", |ui| coloring(ui, "outside", &mut l.outside, false, actions));
    }
    lighting_section(ui, l);
}

fn fractal_sections(ui: &mut Ui, l: &mut Layer, actions: &mut Vec<Action>) {
    if l.kind == LayerKind::Object {
        object_sections(ui, l, actions);
        return;
    }
    // IFS and Kleinian density layers do not use their formula.
    if l.kind == LayerKind::Density
        && matches!(l.density.mode, rfr_core::doc::DensityMode::Ifs | rfr_core::doc::DensityMode::Kleinian | rfr_core::doc::DensityMode::LSystem)
    {
        density_section(ui, l, actions);
        return;
    }
    let lib = library();
    let entry = lib.get(&l.formula);
    let sec = widgets::section(ui, "Formula", true);
    if sec.reset {
        l.params.clear();
        actions.push(Action::Message("Formula parameters reset to their defaults".into()));
    }
    if sec.open {
        match entry {
            Some(e) => {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&e.title).strong().size(theme::fs(14.5)));
                    if ui
                        .small_button("Source")
                        .on_hover_text(if e.set == Set::User {
                            "Show and edit the formula's source"
                        } else {
                            "Show the formula's source (built-in formulas can be copied to change them)"
                        })
                        .clicked()
                    {
                        actions.push(Action::EditFormula(e.id.clone()));
                    }
                });
                ui.label(RichText::new(format!("{} · {}", e.set.label(), e.family)).size(theme::fs(11.0)).color(theme::TEXT_MUTED))
                    .on_hover_text(&e.id);
                if let Err(err) = e.program_with(&l.params) {
                    ui.label(RichText::new(format!("Does not compile: {err}")).color(theme::WARN));
                }
            }
            None => {
                ui.label(RichText::new(format!("Unknown formula '{}'", l.formula)).color(theme::WARN));
            }
        }
        // A plugin choice that does not compile: the default program's parameters stay
        // visible so the choice can be changed back.
        let program = entry.and_then(|e| e.program_with(&l.params).or_else(|_| e.program()).ok());
        if let Some(p) = &program {
            params_ui(ui, "formula", p, &mut l.params);
        }
        widgets::grid(ui, "formula-iter", |ui| {
            widgets::label(ui, "Iterations", "Most iterations per point. Deep views and thin structures need more; more iterations take longer");
            ui.horizontal(|ui| {
                ui.add(egui::Slider::new(&mut l.max_iter, 10..=100_000).logarithmic(true).clamping(egui::SliderClamping::Never))
                    .on_hover_text("Most iterations per point");
                ui.checkbox(&mut l.auto_iterations, "Auto").on_hover_text(
                    "Raise the iterations automatically when the view needs more (checked on a small preview after the view settles; never lowered)",
                );
            });
            ui.end_row();
        });
        if program.as_ref().is_some_and(|p| p.has_julia) {
            widgets::grid(ui, "formula-julia", |ui| {
                widgets::label(
                    ui,
                    "Julia mode",
                    "Iterate every point with the fixed seed below instead of the point itself (the Julia set of the seed)",
                );
                let mut on = l.julia;
                if ui.checkbox(&mut on, "").on_hover_text("Julia set of the seed (J in the view switches)").changed() {
                    actions.push(Action::Julia(on));
                }
                ui.end_row();
                if l.julia {
                    widgets::label(ui, "Seed", "The constant of the Julia set");
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut l.seed[0]).speed(0.001).max_decimals(12).prefix("re "))
                            .on_hover_text("Real part of the seed");
                        ui.add(egui::DragValue::new(&mut l.seed[1]).speed(0.001).max_decimals(12).prefix("im "))
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
    if l.kind == LayerKind::Density {
        density_section(ui, l, actions);
        return;
    }
    transforms_ui(ui, l, actions);
    let sec = widgets::section(ui, "Outside", true);
    if sec.reset {
        let d = Layer::default();
        l.outside_method = d.outside_method;
        l.outside = d.outside;
    }
    if sec.open {
        widgets::segmented(
            ui,
            &mut l.outside_method,
            &[
                (OutsideMethod::Smooth, "Smooth", "Continuous iteration count: smooth color bands"),
                (OutsideMethod::Iterations, "Bands", "Iteration count: one band per iteration"),
                (
                    OutsideMethod::Distance,
                    "Distance",
                    "Estimated distance to the set (relative to the pixel size): bright thin filaments; deep views fall back to Smooth",
                ),
                (OutsideMethod::Formula, "Formula", "A coloring algorithm (orbit traps, averages, decompositions, …)"),
                (OutsideMethod::Solid, "Solid", "One color"),
            ],
        );
        if l.outside_method == OutsideMethod::Formula {
            coloring_formula(ui, "outside", &mut l.outside, actions);
        }
        widgets::grid(ui, "outside-grid", |ui| {
            if l.outside_method == OutsideMethod::Solid {
                solid_color(ui, "Color of every point that escapes", &mut l.outside);
            } else {
                coloring(ui, "outside", &mut l.outside, false, actions);
                if l.outside_method == OutsideMethod::Formula {
                    solid_color(ui, "Color where the coloring formula marks points as solid", &mut l.outside);
                }
            }
        });
    }
    let sec = widgets::section(ui, "Inside", true);
    if sec.reset {
        let d = Layer::default();
        l.inside_method = d.inside_method;
        l.inside = d.inside;
    }
    if sec.open {
        widgets::segmented(
            ui,
            &mut l.inside_method,
            &[
                (InsideMethod::Solid, "Solid", "One color for the set"),
                (InsideMethod::Magnitude, "Magnitude", "|z| after the last iteration"),
                (InsideMethod::Angle, "Angle", "Direction of z after the last iteration"),
                (InsideMethod::Formula, "Formula", "A coloring algorithm for the inside (orbit traps, closest approach, …)"),
            ],
        );
        if l.inside_method == InsideMethod::Formula {
            coloring_formula(ui, "inside", &mut l.inside, actions);
        }
        widgets::grid(ui, "inside-grid", |ui| {
            if l.inside_method == InsideMethod::Solid {
                solid_color(ui, "Color of the points that never escape", &mut l.inside);
            } else {
                coloring(ui, "inside", &mut l.inside, true, actions);
                if l.inside_method == InsideMethod::Formula {
                    solid_color(ui, "Color where the coloring formula marks points as solid", &mut l.inside);
                }
            }
        });
    }
    lighting_section(ui, l);
    layer_effects_section(ui, l);
}

/// Effects of a fractal layer: drop shadow, outer and inner glow, neon edges.
fn layer_effects_section(ui: &mut Ui, l: &mut Layer) {
    use rfr_core::doc::LayerEffects;
    let sec = widgets::section(ui, "Layer effects", true);
    if sec.reset {
        l.effects = LayerEffects::default();
    }
    if !sec.open {
        return;
    }
    if l.light.enabled && l.light.terrain {
        ui.label(RichText::new("Layer effects apply to flat layers, not to 3D terrains.").color(theme::TEXT_MUTED));
        return;
    }
    let e = &mut l.effects;
    let d = LayerEffects::default();
    widgets::grid(ui, "layer-fx", |ui| {
        widgets::slider(ui, "Drop shadow", "Opacity of a shadow the layer casts on the layers below (0: off)", &mut e.shadow, 0.0..=1.0, d.shadow);
        if e.shadow > 0.0 {
            widgets::slider(
                ui,
                "Shadow distance",
                "How far the shadow falls, as a share of the image height",
                &mut e.shadow_distance,
                0.0..=0.2,
                d.shadow_distance,
            );
            widgets::slider(
                ui,
                "Shadow direction",
                "Where the shadow falls, in degrees counterclockwise (315: lower right)",
                &mut e.shadow_angle,
                0.0..=360.0,
                d.shadow_angle,
            );
            widgets::slider(
                ui,
                "Shadow blur",
                "Softness of the shadow, as a share of the image height",
                &mut e.shadow_size,
                0.0..=0.1,
                d.shadow_size,
            );
            widgets::color(ui, "Shadow color", "Color of the shadow", &mut e.shadow_color);
        }
        widgets::slider(ui, "Outer glow", "Light around the layer's shapes, over the layers below (0: off)", &mut e.glow, 0.0..=3.0, d.glow);
        if e.glow > 0.0 {
            widgets::slider(ui, "Glow size", "How far the glow reaches, as a share of the image height", &mut e.glow_size, 0.001..=0.2, d.glow_size);
            widgets::color(ui, "Glow color", "Color of the outer glow", &mut e.glow_color);
        }
        widgets::slider(ui, "Inner glow", "Light along the inside of the layer's edges (0: off)", &mut e.inner_glow, 0.0..=3.0, d.inner_glow);
        if e.inner_glow > 0.0 {
            widgets::slider(ui, "Inner glow size", "How far the inner glow reaches into the shapes", &mut e.inner_size, 0.001..=0.2, d.inner_size);
            widgets::color(ui, "Inner glow color", "Color of the inner glow", &mut e.inner_color);
        }
        widgets::slider(ui, "Neon", "Light along the outside of the set, by the distance estimate (0: off)", &mut e.neon, 0.0..=5.0, d.neon);
        if e.neon > 0.0 {
            widgets::slider_log(
                ui,
                "Neon width",
                "Width of the neon light in pixels (at 1000 pixels image height)",
                &mut e.neon_width,
                0.2..=100.0,
                d.neon_width,
            );
            widgets::color(ui, "Neon color", "Color of the neon light", &mut e.neon_color);
        }
    });
}

/// Lighting of a layer as a height field: height source, relief, lights, highlights.
fn lighting_section(ui: &mut Ui, l: &mut Layer) {
    use rfr_core::doc::{HeightSource, Light, Lighting, MAX_LIGHTS, SpecularModel};
    let sec = widgets::section(ui, "Lighting", true);
    if sec.reset {
        l.light = Lighting { enabled: l.light.enabled, ..Lighting::default() };
    }
    if !sec.open {
        return;
    }
    // 3D objects are always lit and have no relief.
    let object = l.kind == LayerKind::Object;
    let li = &mut l.light;
    let def = Lighting::default();
    if !object {
        widgets::grid(ui, "light-grid", |ui| {
            widgets::checkbox(ui, "Lit", "Shade the layer as a relief: its values become heights, lit by the lights below", &mut li.enabled);
        });
        if !li.enabled {
            return;
        }
    }
    if !object {
        widgets::segmented(
            ui,
            &mut li.height,
            &[
                (HeightSource::Smooth, "Iterations", "Heights from the smooth iteration count (logarithmic); the set is a plateau"),
                (HeightSource::Value, "Coloring", "Heights from the coloring value (the gradient position): the relief follows the color bands"),
                (HeightSource::Luminance, "Brightness", "Heights from the layer's brightness: bump maps from images and colorings"),
                (HeightSource::Distance, "Distance", "Heights from the estimated distance to the set (logarithmic): smooth slopes rising to the set"),
            ],
        );
    }
    widgets::grid(ui, "light-grid2", |ui| {
        if !object && !li.terrain {
            widgets::slider(ui, "Relief", "Steepness of the relief (negative values turn it inside out)", &mut li.relief, -10.0..=10.0, def.relief);
        }
        if !object && li.terrain {
            widgets::slider(
                ui,
                "Terrain height",
                "Height of the terrain's highest parts as a share of the image height (0: flat)",
                &mut li.terrain_height,
                0.0..=1.0,
                def.terrain_height,
            );
        }
        widgets::slider(ui, "Ambient", "Light from all directions: brightens the shadowed sides", &mut li.ambient, 0.0..=2.0, def.ambient);
        widgets::slider(ui, "Highlights", "Strength of the highlights", &mut li.specular, 0.0..=3.0, def.specular);
        widgets::slider(ui, "Roughness", "Small: sharp, glossy highlights; large: wide, dull ones", &mut li.roughness, 0.02..=1.0, def.roughness);
        widgets::label(ui, "Highlight model", "How the highlights fall off");
        ui.horizontal(|ui| {
            ui.radio_value(&mut li.model, SpecularModel::Ggx, "GGX").on_hover_text("Physically based highlights with soft tails");
            ui.radio_value(&mut li.model, SpecularModel::BlinnPhong, "Blinn-Phong").on_hover_text("Classic highlights with a tighter falloff");
        });
        ui.end_row();
        widgets::checkbox(
            ui,
            "Document lights",
            "The light sources of the Lights section also light this layer, as points at their position and height (up to four); for 3D objects the view's plane is the ground through the object's center",
            &mut li.scene_lights,
        );
        widgets::checkbox(ui, "Shadows", "Higher parts of the relief cast shadows away from each light", &mut li.shadows);
        if li.shadows {
            widgets::slider(
                ui,
                "Shadow length",
                "How far shadows reach, as a share of the image height",
                &mut li.shadow_length,
                0.01..=1.0,
                def.shadow_length,
            );
            widgets::slider(
                ui,
                "Softness",
                "Size of the lights: 0 gives sharp shadow edges, larger values soft ones",
                &mut li.softness,
                0.0..=0.5,
                def.softness,
            );
        }
        widgets::slider(
            ui,
            "Occlusion",
            "Darkens hollows and the foot of slopes, where less light comes from all around (0: off)",
            &mut li.occlusion,
            0.0..=2.0,
            def.occlusion,
        );
        if li.occlusion > 0.0 {
            widgets::slider(
                ui,
                "Occlusion radius",
                "How far around each point the occlusion looks, as a share of the image height",
                &mut li.occlusion_radius,
                0.005..=0.2,
                def.occlusion_radius,
            );
        }
        if !object {
            widgets::checkbox(ui, "3D terrain", "Show the relief as a landscape seen by a camera instead of from straight above", &mut li.terrain);
        }
        if li.terrain && !object {
            use rfr_core::doc::Camera;
            let c = Camera::default();
            let cam = &mut li.camera;
            widgets::slider(
                ui,
                "Camera direction",
                "Where the camera looks from, in degrees (0: from the bottom edge of the image)",
                &mut cam.yaw,
                -180.0..=180.0,
                c.yaw,
            );
            widgets::slider(
                ui,
                "Camera height",
                "The camera's angle above the ground in degrees (90: straight down)",
                &mut cam.pitch,
                1.0..=89.0,
                c.pitch,
            );
            widgets::slider_log(
                ui,
                "Camera distance",
                "Distance of the camera from the middle of the terrain, in image heights",
                &mut cam.distance,
                0.05..=10.0,
                c.distance,
            );
            widgets::slider(
                ui,
                "Field of view",
                "Vertical opening angle of the camera in degrees (small: telephoto, large: wide angle)",
                &mut cam.fov,
                5.0..=120.0,
                c.fov,
            );
        }
        widgets::slider(
            ui,
            "Reflection",
            "How much the relief mirrors its surroundings (0: not at all, 1: a mirror)",
            &mut li.reflection,
            0.0..=1.0,
            def.reflection,
        );
        if li.reflection > 0.0 {
            widgets::label(ui, "Environment", "What the relief reflects: a panorama image (2:1, equirectangular) or the sky colors below");
            ui.horizontal(|ui| {
                let path = &mut li.environment;
                let name = if path.is_empty() { "Sky colors".to_string() } else { path.rsplit(['/', '\\']).next().unwrap_or(path).to_string() };
                let hover = if path.is_empty() {
                    "Choose a panorama image (PNG or JPEG)".to_string()
                } else {
                    format!("{path}\nClick to choose another panorama.")
                };
                if ui.button(format!("{} {name}", icons::OPEN)).on_hover_text(hover).clicked()
                    && let Some(f) = rfd::FileDialog::new().add_filter("Images", &["png", "jpg", "jpeg"]).pick_file()
                {
                    *path = f.display().to_string();
                }
                if !path.is_empty() {
                    if ui.small_button(icons::X).on_hover_text("Reflect the sky colors instead").clicked() {
                        path.clear();
                    } else if let Err(e) = rfr_core::images::load(path) {
                        ui.label(RichText::new(icons::WARNING).color(theme::WARN))
                            .on_hover_text(format!("{e}\nThe sky colors are reflected instead."));
                    }
                }
            });
            ui.end_row();
            if li.environment.is_empty() {
                widgets::color(ui, "Sky horizon", "Color of the sky at the horizon (reflected by steep slopes)", &mut li.horizon);
                widgets::color(ui, "Sky zenith", "Color of the sky straight up (reflected by flat parts)", &mut li.zenith);
            }
        }
    });
    let mut remove = None;
    for (k, light) in li.lights.iter_mut().enumerate() {
        ui.separator();
        ui.horizontal(|ui| {
            ui.label(format!("Light {}", k + 1)).on_hover_text("A light far away (like the sun)");
            if widgets::icon_button(ui, icons::X, "Remove this light").clicked() {
                remove = Some(k);
            }
        });
        let d = Light::default();
        widgets::grid(ui, &format!("light-{k}"), |ui| {
            widgets::slider(
                ui,
                "Direction",
                "Where the light comes from, in degrees counterclockwise from the right edge",
                &mut light.azimuth,
                0.0..=360.0,
                d.azimuth,
            );
            widgets::slider(
                ui,
                "Height",
                "Angle of the light above the image in degrees (low: long, raking light)",
                &mut light.elevation,
                1.0..=90.0,
                d.elevation,
            );
            widgets::slider(ui, "Intensity", "Brightness of the light", &mut light.intensity, 0.0..=4.0, d.intensity);
            widgets::color(ui, "Color", "Color of the light", &mut light.color);
        });
    }
    if let Some(k) = remove {
        li.lights.remove(k);
    }
    if li.lights.len() < MAX_LIGHTS && ui.button(format!("{} Light", icons::PLUS)).on_hover_text("Add a light from the opposite side").clicked() {
        let az = li.lights.last().map_or(135.0, |l| (l.azimuth + 180.0) % 360.0);
        li.lights.push(Light { azimuth: az, intensity: 0.5, ..Light::default() });
    }
}

/// The maps of an IFS density layer: coefficients, weight, variation; presets and fitting.
fn ifs_maps(ui: &mut Ui, d: &mut rfr_core::doc::Density, actions: &mut Vec<Action>) {
    use rfr_core::doc::{IfsMap, Variation};
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt("ifs-preset").selected_text("Presets").show_ui(ui, |ui| {
            for (name, maps) in rfr_core::doc::ifs_presets() {
                if ui.selectable_label(false, name).on_hover_text(format!("Replace the maps with the {name}")).clicked() {
                    d.ifs = maps;
                    actions.push(Action::FitDensity);
                }
            }
        });
        if ui.button("Fit view").on_hover_text("Fit the view and the start region to the attractor").clicked() {
            actions.push(Action::FitDensity);
        }
        if ui.button(format!("{} Map", icons::PLUS)).on_hover_text("Add a map (half size, no shift)").clicked() {
            d.ifs.push(IfsMap::default());
        }
    });
    let mut remove = None;
    for (k, m) in d.ifs.iter_mut().enumerate() {
        ui.separator();
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("Map {}", k + 1)).strong());
            egui::ComboBox::from_id_salt(("ifs-var", k)).selected_text(m.variation.label()).show_ui(ui, |ui| {
                for v in Variation::ALL {
                    ui.selectable_value(&mut m.variation, v, v.label());
                }
            });
            ui.add(egui::DragValue::new(&mut m.weight).range(0.0..=100.0).speed(0.01).prefix("weight "))
                .on_hover_text("Share of the steps that use this map");
            if ui.small_button(icons::TRASH).on_hover_text("Remove this map").clicked() {
                remove = Some(k);
            }
        });
        ui.horizontal_wrapped(|ui| {
            for (j, name) in ["a", "b", "c", "d", "e", "f"].iter().enumerate() {
                ui.add(egui::DragValue::new(&mut m.coefs[j]).speed(0.005).max_decimals(4).prefix(format!("{name} ")))
                    .on_hover_text("x' = a·x + b·y + e, y' = c·x + d·y + f");
            }
        });
    }
    if let Some(k) = remove {
        d.ifs.remove(k);
    }
    ui.separator();
}

/// The L-system of a density layer: axiom, rules, angle, rewrites, stroke width, presets.
fn lsystem_ui(ui: &mut Ui, s: &mut rfr_core::doc::LSystem, actions: &mut Vec<Action>) {
    use rfr_core::doc::LSystem;
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt("lsys-preset").selected_text("Presets").show_ui(ui, |ui| {
            let presets: [(&str, LSystem); 5] = [
                ("Koch snowflake", LSystem::default()),
                ("Plant", LSystem { axiom: "F".into(), rules: "F=FF-[-F+F+F]+[+F-F-F]".into(), angle: 16.0, iterations: 4, width: 0.4 }),
                ("Dragon curve", LSystem { axiom: "FX".into(), rules: "X=X+YF+\nY=-FX-Y".into(), angle: 4.0, iterations: 12, width: 0.15 }),
                (
                    "Hilbert curve",
                    LSystem { axiom: "X".into(), rules: "X=-YF+XFX+FY-\nY=+XF-YFY-FX+".into(), angle: 4.0, iterations: 6, width: 0.15 },
                ),
                (
                    "Sierpinski arrowhead",
                    LSystem { axiom: "YF".into(), rules: "X=YF+XF+Y\nY=XF-YF-X".into(), angle: 6.0, iterations: 7, width: 0.15 },
                ),
            ];
            for (name, p) in presets {
                if ui.selectable_label(false, name).on_hover_text(format!("The {name}")).clicked() {
                    *s = p;
                    actions.push(Action::FitDensity);
                }
            }
        });
        if ui.button("Fit view").on_hover_text("Fit the view to the strokes").clicked() {
            actions.push(Action::FitDensity);
        }
    });
    widgets::grid(ui, "lsys-grid", |ui| {
        widgets::label(ui, "Axiom", "The starting string");
        ui.add(egui::TextEdit::singleline(&mut s.axiom).font(egui::TextStyle::Monospace))
            .on_hover_text("F, D: forward with a line; G, M, f: forward; + −: turn; |: turn around; !: swap turns; [ ]: save, restore");
        ui.end_row();
        widgets::label(ui, "Rules", "One rule per line: a letter, '=' and what it becomes");
        ui.add(egui::TextEdit::multiline(&mut s.rules).font(egui::TextStyle::Monospace).desired_rows(3)).on_hover_text("For example F=F+F--F+F");
        ui.end_row();
        widgets::label(ui, "Angle", "Turns as divisions of the full circle (6: 60°, 4: 90°)");
        ui.add(egui::DragValue::new(&mut s.angle).range(1.0..=360.0).speed(0.05).max_decimals(3)).on_hover_text("Divisions of 360°");
        ui.end_row();
        widgets::label(ui, "Rewrites", "How often the rules are applied (the strokes grow fast)");
        ui.add(egui::DragValue::new(&mut s.iterations).range(0..=20)).on_hover_text("Rewrites of the axiom");
        ui.end_row();
        widgets::label(ui, "Stroke width", "Width of the lines in turtle steps");
        ui.add(egui::DragValue::new(&mut s.width).range(0.0..=10.0).speed(0.005).max_decimals(3)).on_hover_text("Line width");
        ui.end_row();
    });
}

/// The group of a Kleinian density layer: recipe, traces, presets.
fn kleinian_ui(ui: &mut Ui, k: &mut rfr_core::doc::Kleinian) {
    use rfr_core::doc::{Kleinian, Recipe};
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt("klein-preset").selected_text("Presets").show_ui(ui, |ui| {
            let presets: [(&str, Kleinian); 3] = [
                ("Apollonian gasket", Kleinian { recipe: Recipe::Grandma, ta: [2.0, 0.0], tb: [2.0, 0.0], other_root: false }),
                ("Quasi-Fuchsian", Kleinian::default()),
                ("Spiral", Kleinian { recipe: Recipe::Grandma, ta: [1.87, 0.1], tb: [1.87, -0.1], other_root: false }),
            ];
            for (name, p) in presets {
                if ui.selectable_label(false, name).on_hover_text(format!("The {name} group")).clicked() {
                    *k = p;
                }
            }
        });
    });
    widgets::segmented(
        ui,
        &mut k.recipe,
        &[
            (Recipe::Grandma, "Two traces", "Grandma's recipe from the traces of a and b (2 and 2: the Apollonian gasket)"),
            (Recipe::Riley, "Riley", "a = [1 2; 0 1], b = [1 0; c 1] with c from the first trace"),
        ],
    );
    widgets::grid(ui, "klein-grid", |ui| {
        widgets::label(ui, if k.recipe == Recipe::Riley { "c" } else { "Trace of a" }, "A complex number; small changes reshape the limit set");
        ui.horizontal(|ui| {
            ui.add(egui::DragValue::new(&mut k.ta[0]).speed(0.002).max_decimals(5).prefix("re "));
            ui.add(egui::DragValue::new(&mut k.ta[1]).speed(0.002).max_decimals(5).prefix("im "));
        });
        ui.end_row();
        if k.recipe == Recipe::Grandma {
            widgets::label(ui, "Trace of b", "A complex number; 2 gives Maskit's groups");
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut k.tb[0]).speed(0.002).max_decimals(5).prefix("re "));
                ui.add(egui::DragValue::new(&mut k.tb[1]).speed(0.002).max_decimals(5).prefix("im "));
            });
            ui.end_row();
            widgets::label(ui, "Other root", "The recipe solves a quadratic for the trace of ab: use its other root");
            ui.checkbox(&mut k.other_root, "").on_hover_text("The other solution for the trace of ab");
            ui.end_row();
        }
    });
}

/// Settings of a density layer: which orbits, where they start, how the counts look.
fn density_section(ui: &mut Ui, l: &mut Layer, actions: &mut Vec<Action>) {
    use rfr_core::doc::{Density, DensityMode};
    let sec = widgets::section(ui, "Density", true);
    if sec.reset {
        l.density = Density::default();
    }
    if !sec.open {
        return;
    }
    let d = &mut l.density;
    widgets::segmented(
        ui,
        &mut d.mode,
        &[
            (DensityMode::Escaping, "Escaping", "Paths of the points that escape (Buddhabrot)"),
            (DensityMode::Captive, "Captive", "Paths of the points that stay (anti-Buddhabrot)"),
            (DensityMode::Attractor, "Attractor", "Every orbit after a warm-up: strange attractors (formulas of the Attractors family)"),
            (DensityMode::Ifs, "IFS", "The chaos game over the affine maps below (iterated function systems; the formula is not used)"),
            (DensityMode::Kleinian, "Kleinian", "The limit set of a group of two Möbius maps (Indra's pearls; the formula is not used)"),
            (DensityMode::LSystem, "L-system", "The strokes of an L-system's turtle (the formula is not used)"),
        ],
    );
    if d.mode == DensityMode::LSystem {
        lsystem_ui(ui, &mut d.lsystem, actions);
    }
    if d.mode == DensityMode::Ifs {
        ifs_maps(ui, d, actions);
    }
    if d.mode == DensityMode::Kleinian {
        kleinian_ui(ui, &mut d.kleinian);
    }
    let def = Density::default();
    widgets::grid(ui, "density-grid", |ui| {
        if d.mode == DensityMode::Escaping {
            widgets::slider_u32(
                ui,
                "Shortest orbit",
                "Orbits that escape sooner are not drawn (they only add a haze)",
                &mut d.min_iter,
                0..=1000,
                def.min_iter,
            );
        }
        if matches!(d.mode, DensityMode::Attractor | DensityMode::Ifs | DensityMode::Kleinian) {
            widgets::slider_u32(
                ui,
                "Warm-up",
                "Iterations before the points are counted (the orbit settles on the attractor)",
                &mut d.warmup,
                0..=1000,
                def.warmup,
            );
        }
        widgets::label(ui, "Start region", "Where the start points are drawn from: x from, y from, x to, y to (plane units)");
        ui.horizontal(|ui| {
            for (k, v) in d.region.iter_mut().enumerate() {
                ui.add(egui::DragValue::new(v).speed(0.01).max_decimals(6)).on_hover_text(["x from", "y from", "x to", "y to"][k]);
            }
        });
        ui.end_row();
        widgets::label(ui, "Orbits per pass", "Start points per render pass: more fill the image faster, each pass takes longer");
        ui.add(egui::DragValue::new(&mut d.orbits).range(256..=4_000_000).speed(256.0)).on_hover_text("Start points per pass");
        ui.end_row();
        if matches!(d.mode, DensityMode::Escaping | DensityMode::Captive) {
            widgets::label(ui, "Metropolis", "Start points from Metropolis chains: favors orbits that pass through the view");
            ui.checkbox(&mut d.metropolis, "").on_hover_text(
                "For zoomed views: each orbit slot keeps a start point and moves it in small steps, keeping steps whose orbits \
                 cross the view more often. The image shows the same density as with uniform start points, much sooner.",
            );
            ui.end_row();
        }
        widgets::label(ui, "Nebulabrot", "Red, green and blue count orbits up to their own iteration limits");
        ui.checkbox(&mut d.nebula, "").on_hover_text("Three channels with their own iteration limits instead of the gradient");
        ui.end_row();
        if d.nebula {
            widgets::label(ui, "Limits (R G B)", "Longest orbit counted in red, green and blue");
            ui.horizontal(|ui| {
                for (k, v) in d.nebula_iter.iter_mut().enumerate() {
                    ui.add(egui::DragValue::new(v).range(1..=1_000_000)).on_hover_text(["Red", "Green", "Blue"][k]);
                }
            });
            ui.end_row();
        }
        widgets::slider_log(
            ui,
            "Brightness",
            "Scales the log density (higher: more of the image at full brightness)",
            &mut d.brightness,
            0.05..=20.0,
            def.brightness,
        );
        widgets::slider(ui, "Gamma", "Lifts the faint parts (higher) or darkens them (lower)", &mut d.gamma, 0.2..=5.0, def.gamma);
        if !d.nebula {
            widgets::label(ui, "Gradient", "Colors from few points (left) to many (right). Click to edit.");
            let w = ui.available_width().clamp(120.0, 260.0);
            if widgets::gradient_strip(ui, &l.outside.gradient, w, 18.0).on_hover_text("Click to edit the gradient").clicked() {
                actions.push(Action::EditGradient(false));
            }
            ui.end_row();
        }
    });
}

/// Effects on the finished image (`rfr_core::render::post`).
fn effects_section(ui: &mut Ui, p: &mut rfr_core::doc::Post) {
    use rfr_core::doc::{Post, ToneMap};
    let sec = widgets::section(ui, "Effects", true);
    if sec.reset {
        *p = Post::default();
    }
    if !sec.open {
        return;
    }
    let d = Post::default();
    let sub = |ui: &mut Ui, text: &str| {
        ui.label(RichText::new(text).color(theme::TEXT_MUTED).size(theme::fs(11.5)));
    };
    sub(ui, "Tone");
    widgets::grid(ui, "fx-tone", |ui| {
        widgets::slider(ui, "Exposure", "Brightness in stops (each stop doubles the light)", &mut p.exposure, -5.0..=5.0, d.exposure);
        widgets::label(ui, "Tone mapping", "How light brighter than white is brought into range");
        egui::ComboBox::from_id_salt("fx-tonemap").selected_text(p.tone_map.label()).show_ui(ui, |ui| {
            for t in ToneMap::ALL {
                ui.selectable_value(&mut p.tone_map, t, t.label()).on_hover_text(match t {
                    ToneMap::None => "Values above white are cut off",
                    ToneMap::Reinhard => "Soft, keeps colors but flattens bright parts",
                    ToneMap::Aces => "Film-like curve with rich contrast",
                    ToneMap::Filmic => "Hable's filmic curve: gentle shoulder, lifted shadows",
                    ToneMap::Agx => "Smooth highlights that desaturate toward white like film",
                });
            }
        });
        ui.end_row();
        widgets::slider(ui, "Contrast", "Contrast around middle gray (1: unchanged)", &mut p.contrast, 0.0..=3.0, d.contrast);
        widgets::slider(ui, "Saturation", "Color intensity (0: gray, 1: unchanged)", &mut p.saturation, 0.0..=3.0, d.saturation);
    });
    sub(ui, "Color");
    widgets::grid(ui, "fx-color", |ui| {
        widgets::slider(ui, "Temperature", "White balance: cooler (blue) to warmer (orange)", &mut p.temperature, -1.0..=1.0, d.temperature);
        widgets::slider(ui, "Tint", "White balance: toward green (left) or magenta (right)", &mut p.tint, -1.0..=1.0, d.tint);
        widgets::color(ui, "Shadow tone", "Color the dark parts take on (split toning)", &mut p.shadow_color);
        widgets::slider(ui, "Shadow amount", "How strongly the dark parts take on the shadow tone", &mut p.shadow_amount, 0.0..=1.0, d.shadow_amount);
        widgets::color(ui, "Highlight tone", "Color the bright parts take on (split toning)", &mut p.highlight_color);
        widgets::slider(
            ui,
            "Highlight amount",
            "How strongly the bright parts take on the highlight tone",
            &mut p.highlight_amount,
            0.0..=1.0,
            d.highlight_amount,
        );
    });
    sub(ui, "Glow");
    widgets::grid(ui, "fx-glow", |ui| {
        widgets::slider(ui, "Bloom", "Soft glow around bright parts (0: off)", &mut p.bloom, 0.0..=3.0, d.bloom);
        if p.bloom > 0.0 {
            widgets::slider(
                ui,
                "Bloom threshold",
                "Brightness above which parts glow (after exposure; 1: white)",
                &mut p.bloom_threshold,
                0.0..=4.0,
                d.bloom_threshold,
            );
            widgets::slider(ui, "Bloom size", "Small, tight glow (0) to wide haze (1)", &mut p.bloom_size, 0.0..=1.0, d.bloom_size);
        }
        widgets::slider(ui, "Streaks", "Lines of light through very bright parts, like a star filter (0: off)", &mut p.streak, 0.0..=3.0, d.streak);
        if p.streak > 0.0 {
            widgets::slider(
                ui,
                "Streak threshold",
                "Brightness above which streaks start (after exposure)",
                &mut p.streak_threshold,
                0.0..=4.0,
                d.streak_threshold,
            );
            widgets::slider(
                ui,
                "Streak length",
                "Length of the streaks as a share of the image width",
                &mut p.streak_length,
                0.01..=1.0,
                d.streak_length,
            );
            widgets::slider(ui, "Streak angle", "Direction of the first streak in degrees", &mut p.streak_angle, 0.0..=180.0, d.streak_angle);
            widgets::slider_u32(ui, "Streak directions", "1: a line, 2: a cross, 3 and 4: stars", &mut p.streak_count, 1..=4, d.streak_count);
            widgets::color(ui, "Streak tint", "Color of the streaks", &mut p.streak_tint);
        }
        widgets::slider(
            ui,
            "God rays",
            "Shafts of light from bright parts toward a light position (0: off); strongest with bright shapes in front of darkness",
            &mut p.rays,
            0.0..=3.0,
            d.rays,
        );
        if p.rays > 0.0 {
            widgets::slider(
                ui,
                "Rays threshold",
                "Brightness above which parts send out rays (after exposure)",
                &mut p.rays_threshold,
                0.0..=4.0,
                d.rays_threshold,
            );
            widgets::label(ui, "Light position", "Where the rays come from, as a share of the image width and height from the top left");
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut p.rays_position[0]).speed(0.005).range(-1.0..=2.0).max_decimals(3))
                    .on_hover_text("x (0: left edge, 1: right edge)");
                ui.add(egui::DragValue::new(&mut p.rays_position[1]).speed(0.005).range(-1.0..=2.0).max_decimals(3))
                    .on_hover_text("y (0: top edge, 1: bottom edge)");
            });
            ui.end_row();
            widgets::slider(
                ui,
                "Rays length",
                "How far the rays reach toward the light position (1: all the way)",
                &mut p.rays_length,
                0.05..=1.0,
                d.rays_length,
            );
        }
    });
    sub(ui, "Levels and curve");
    widgets::grid(ui, "fx-levels", |ui| {
        widgets::slider(ui, "Black point", "Displayed values at or below this become black", &mut p.levels_black, 0.0..=0.9, d.levels_black);
        widgets::slider(ui, "White point", "Displayed values at or above this become white", &mut p.levels_white, 0.1..=1.0, d.levels_white);
        widgets::slider(ui, "Midtones", "Brightens (above 1) or darkens (below 1) the middle tones", &mut p.levels_gamma, 0.2..=5.0, d.levels_gamma);
        for (k, name) in ["Curve: black", "Curve: shadows", "Curve: middle", "Curve: lights", "Curve: white"].iter().enumerate() {
            let tip = format!("Output of the tone curve at input {} (the straight line leaves the image unchanged)", ["0", "¼", "½", "¾", "1"][k]);
            widgets::slider(ui, name, &tip, &mut p.curve[k], 0.0..=1.0, rfr_core::doc::IDENTITY_CURVE[k]);
        }
        widgets::label(ui, "", "");
        curve_preview(ui, p);
        ui.end_row();
    });
    sub(ui, "Denoise");
    widgets::grid(ui, "fx-denoise", |ui| {
        widgets::checkbox(
            ui,
            "Denoise exports",
            "Exported images (and command line renders) go through Intel Open Image Denoise before the other effects; the preview is not denoised",
            &mut p.denoise,
        );
        if p.denoise {
            widgets::slider(
                ui,
                "Denoise amount",
                "Share of the denoised image (1: only denoised, lower values keep some grain)",
                &mut p.denoise_blend,
                0.0..=1.0,
                d.denoise_blend,
            );
        }
    });
    sub(ui, "Lens and film");
    widgets::grid(ui, "fx-lens", |ui| {
        widgets::slider(ui, "Sharpen", "Crisper edges (unsharp mask over 3 × 3 pixels)", &mut p.sharpen, 0.0..=3.0, d.sharpen);
        widgets::slider(ui, "Distortion", "Barrel (right) or pincushion (left) lens distortion", &mut p.distortion, -0.5..=0.5, d.distortion);
        widgets::slider(
            ui,
            "Chromatic aberration",
            "Red and blue drift apart toward the edges, like a simple lens",
            &mut p.chromatic,
            0.0..=0.05,
            d.chromatic,
        );
        if p.chromatic > 0.0 {
            widgets::checkbox(ui, "Spectral", "A rainbow of seven wavelengths instead of three color channels: smoother fringes", &mut p.spectral);
        }
        widgets::slider(ui, "Vignette", "Darker corners", &mut p.vignette, 0.0..=2.0, d.vignette);
        widgets::slider(ui, "Grain", "Film grain (a fixed noise pattern)", &mut p.grain, 0.0..=1.0, d.grain);
    });
}

/// A small plot of the tone curve with its levels (display values in and out).
fn curve_preview(ui: &mut Ui, p: &rfr_core::doc::Post) {
    let size = egui::vec2(120.0, 80.0);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, theme::BG);
    let pt = |x: f32, y: f32| egui::pos2(rect.left() + x * rect.width(), rect.bottom() - y * rect.height());
    painter.line_segment([pt(0.0, 0.0), pt(1.0, 1.0)], egui::Stroke::new(1.0, theme::TEXT_MUTED.gamma_multiply(0.4)));
    let points: Vec<egui::Pos2> = (0..=64)
        .map(|k| {
            let x = k as f32 / 64.0;
            pt(x, rfr_core::render::post::curve(rfr_core::render::post::levels(x, p), p))
        })
        .collect();
    painter.add(egui::Shape::line(points, egui::Stroke::new(1.5, theme::ACCENT)));
    resp.on_hover_text("The levels and the tone curve: input (across) to output (up)");
}

fn view_and_image(ui: &mut Ui, doc: &mut Document, sel: &Selection) {
    let sec = widgets::section(ui, "View", true);
    if sec.reset {
        let formula = crate::layers::target(doc, sel).map(|l| (l.formula.clone(), l.julia));
        if let Some((f, julia)) = formula
            && let Some(e) = library().get(&f)
        {
            doc.view.radius = if julia { 1.6 } else { e.view.radius }.into();
            doc.view.set_center(if julia { [0.0, 0.0] } else { e.view.center });
            doc.view.rotation = 0.0;
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
            // Magnification as text: any depth (1e-1000 and beyond).
            let id = egui::Id::new("view-zoom-text");
            let shown = doc.view.magnification().to_string();
            let mut text: String = ui.data_mut(|d| d.get_temp::<(String, String)>(id)).filter(|(s, _)| *s == shown).map_or(shown.clone(), |(_, t)| t);
            let r = ui.add(egui::TextEdit::singleline(&mut text).desired_width(f32::INFINITY).font(egui::TextStyle::Monospace));
            if r.changed() {
                if let Some(z) = rfr_core::math::fx::Fx::parse(&text).filter(|z| z.m > 0.0) {
                    doc.view.radius = rfr_core::math::fx::Fx::from_f64(1.5) / z;
                }
                ui.data_mut(|d| d.insert_temp(id, (doc.view.magnification().to_string(), text.clone())));
            }
            r.on_hover_text("Magnification (type a number such as 1e250; the wheel in the view zooms at the cursor)");
            ui.end_row();
            let mut rot = if doc.view.rotation > 180.0 { doc.view.rotation - 360.0 } else { doc.view.rotation };
            if widgets::slider(ui, "Rotation", "Rotation of the view in degrees (Ctrl-drag in the view)", &mut rot, -180.0..=180.0, 0.0).changed() {
                doc.view.rotation = rot.rem_euclid(360.0);
            }
        });
    }
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
            widgets::slider(
                ui,
                "Adaptive",
                "Adaptive sampling: a pixel stops taking samples once it and its neighbours are smooth (noise below this, after at least 32 samples). Flat parts finish early, edges get all samples: renders with many samples are several times faster. 0: every pixel takes every sample",
                &mut doc.image.adaptive,
                0.0..=0.02,
                rfr_core::doc::Image::default().adaptive,
            );
            let mut bg = [doc.image.background[0], doc.image.background[1], doc.image.background[2]];
            if widgets::color(ui, "Background", "Color below all layers", &mut bg).changed() {
                doc.image.background[0] = bg[0];
                doc.image.background[1] = bg[1];
                doc.image.background[2] = bg[2];
            }
            let mut a = doc.image.background[3] as f64;
            if widgets::slider(
                ui,
                "Background opacity",
                "0 makes the background transparent: exported PNGs keep the transparency",
                &mut a,
                0.0..=1.0,
                1.0,
            )
            .changed()
            {
                doc.image.background[3] = a as f32;
            }
        });
    }
}

/// Draws the inspector for the selection; returns requests for the app.
pub fn show(ui: &mut Ui, doc: &mut Document, sel: &Selection) -> Vec<Action> {
    let mut actions = Vec::new();
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        if sel.mask {
            if let Some(l) = doc.layer_mut(&sel.path)
                && let Some(m) = &mut l.mask
            {
                let sec = widgets::section(ui, "Mask", false);
                if sec.open {
                    ui.label(RichText::new(format!("Mask of {}", l.name)).strong());
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut m.enabled, "Enabled").on_hover_text("Use the mask");
                        ui.checkbox(&mut m.invert, "Invert").on_hover_text("Show the layer where the mask is dark instead of bright");
                    });
                    widgets::segmented(
                        ui,
                        &mut m.source,
                        &[
                            (MaskSource::Luminance, "Brightness", "The mask's brightness (times its alpha) decides where the layer shows"),
                            (MaskSource::Alpha, "Alpha", "The mask's alpha decides"),
                        ],
                    );
                    if ui.button("Back to the layer").on_hover_text("Select the layer this mask belongs to").clicked() {
                        actions.push(Action::Select(Selection::layer(sel.path.clone())));
                    }
                }
            }
        } else if let Some(l) = doc.layer_mut(&sel.path) {
            layer_settings(ui, l, sel, &mut actions);
        }
        if let Some(l) = target_mut(doc, sel)
            && matches!(l.kind, LayerKind::Fractal | LayerKind::Density | LayerKind::Object)
        {
            fractal_sections(ui, l, &mut actions);
        }
        view_and_image(ui, doc, sel);
        crate::lights::section(ui, doc);
        effects_section(ui, &mut doc.post);
    });
    actions
}
