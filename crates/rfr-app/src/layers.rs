// SPDX-License-Identifier: LGPL-3.0-or-later

//! The layer tree in the left panel: layers and groups (top of the stack first, as image
//! editors show it), their masks, visibility, and the commands to add, duplicate, delete,
//! move and group layers. The selection says which layer (or mask) the browser and the
//! inspector edit.

use egui::{RichText, Ui};
use rfr_core::doc::{Document, Layer, LayerKind, Mask};

use crate::{icons, theme, widgets};

/// The edited layer: its path in the tree, or its mask.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    pub path: Vec<usize>,
    pub mask: bool,
}

impl Selection {
    pub fn layer(path: Vec<usize>) -> Selection {
        Selection { path, mask: false }
    }
}

/// The layer the selection edits (the mask's layer for a mask selection).
pub fn target<'a>(doc: &'a Document, sel: &Selection) -> Option<&'a Layer> {
    let l = doc.layer(&sel.path)?;
    if sel.mask { l.mask.as_ref().map(|m| &*m.layer) } else { Some(l) }
}

pub fn target_mut<'a>(doc: &'a mut Document, sel: &Selection) -> Option<&'a mut Layer> {
    let l = doc.layer_mut(&sel.path)?;
    if sel.mask { l.mask.as_mut().map(|m| &mut *m.layer) } else { Some(l) }
}

/// Repairs a selection that no longer points at a layer (after undo, open, delete).
pub fn fix(doc: &Document, sel: &mut Selection) {
    if doc.layer(&sel.path).is_none() {
        *sel = Selection::layer(vec![doc.layers.len().saturating_sub(1)]);
    }
    if sel.mask && doc.layer(&sel.path).is_none_or(|l| l.mask.is_none()) {
        sel.mask = false;
    }
}

/// A menu entry: the command it makes, its text and its hover text.
type MenuItem = (fn() -> Cmd, &'static str, &'static str);

enum Cmd {
    Select(Selection),
    ToggleVisible(Vec<usize>),
    AddLayer,
    AddDensity,
    AddGroup,
    Duplicate,
    Delete,
    Up,
    Down,
    Group,
    Ungroup,
    AddMask,
}

fn name_of(l: &Layer) -> String {
    match l.kind {
        LayerKind::Group => format!("{}  {}", icons::FOLDER, l.name),
        LayerKind::Fractal => l.name.clone(),
        LayerKind::Density => format!("{}  {}", icons::DENSITY, l.name),
    }
}

fn rows(ui: &mut Ui, list: &[Layer], prefix: &[usize], depth: usize, sel: &Selection, cmds: &mut Vec<Cmd>) {
    // Top of the stack first.
    for (i, l) in list.iter().enumerate().rev() {
        let mut path = prefix.to_vec();
        path.push(i);
        ui.horizontal(|ui| {
            ui.add_space(depth as f32 * 14.0);
            let eye = if l.visible { icons::EYE } else { icons::EYE_SLASH };
            if widgets::icon_button(ui, eye, if l.visible { "Hide this layer" } else { "Show this layer" }).clicked() {
                cmds.push(Cmd::ToggleVisible(path.clone()));
            }
            let selected = sel.path == path && !sel.mask;
            let mut text = RichText::new(name_of(l));
            if !l.visible {
                text = text.color(theme::TEXT_MUTED);
            }
            let detail = format!("{} · {:.0} %", l.merge.label(), l.opacity * 100.0);
            if ui.selectable_label(selected, text).on_hover_text(format!("{detail}\nClick to edit this layer")).clicked() {
                cmds.push(Cmd::Select(Selection::layer(path.clone())));
            }
            if l.merge != rfr_core::doc::MergeMode::Normal || l.opacity < 1.0 {
                ui.label(RichText::new(detail).size(theme::fs(10.5)).color(theme::TEXT_MUTED));
            }
        });
        if let Some(m) = &l.mask {
            ui.horizontal(|ui| {
                ui.add_space(depth as f32 * 14.0 + 30.0);
                let selected = sel.path == path && sel.mask;
                let mut text = RichText::new(format!("{}  Mask{}", icons::MASK, if m.invert { " (inverted)" } else { "" })).size(theme::fs(12.0));
                if !m.enabled {
                    text = text.color(theme::TEXT_MUTED);
                }
                if ui
                    .selectable_label(selected, text)
                    .on_hover_text("The mask's fractal: its brightness (or alpha) limits where the layer shows. Click to edit it.")
                    .clicked()
                {
                    cmds.push(Cmd::Select(Selection { path: path.clone(), mask: true }));
                }
            });
        }
        if l.kind == LayerKind::Group {
            rows(ui, &l.children, &path, depth + 1, sel, cmds);
        }
    }
}

/// Draws the layer tree and its buttons; applies the commands. Returns a message.
pub fn show(ui: &mut Ui, doc: &mut Document, sel: &mut Selection) -> Option<String> {
    let mut cmds = Vec::new();
    ui.horizontal(|ui| {
        ui.label(RichText::new(icons::LAYERS).color(theme::ACCENT).size(theme::fs(15.0)));
        ui.label(RichText::new("Layers").strong());
    });
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.menu_button(icons::DOTS, |ui| {
                ui.set_min_width(200.0);
                let items: [MenuItem; 4] = [
                    (|| Cmd::Duplicate, "Duplicate", "A copy of the selected layer above it"),
                    (|| Cmd::Group, "Group", "Put the selected layer into a new group"),
                    (|| Cmd::Ungroup, "Ungroup", "Move the selected group's layers out of it and remove the group"),
                    (|| Cmd::AddMask, "Add mask", "Give the selected layer or group a mask (a fractal that limits where it shows)"),
                ];
                for (make, text, tip) in items {
                    if ui.button(text).on_hover_text(tip).clicked() {
                        cmds.push(make());
                        ui.close();
                    }
                }
            })
            .response
            .on_hover_text("More layer commands");
            if widgets::icon_button(ui, icons::TRASH, "Delete the selected layer (or its mask)").clicked() {
                cmds.push(Cmd::Delete);
            }
            if widgets::icon_button(ui, icons::ARROW_DOWN, "Move the selected layer down").clicked() {
                cmds.push(Cmd::Down);
            }
            if widgets::icon_button(ui, icons::ARROW_UP, "Move the selected layer up").clicked() {
                cmds.push(Cmd::Up);
            }
            if widgets::icon_button(ui, icons::FOLDER_PLUS, "Add a group above the selected layer").clicked() {
                cmds.push(Cmd::AddGroup);
            }
            if widgets::icon_button(ui, icons::DENSITY, "Add a density layer above the selected layer: the orbits of its formula, counted per pixel (Buddhabrot; strange attractors with the formulas of the Attractors family)").clicked() {
                cmds.push(Cmd::AddDensity);
            }
            if widgets::icon_button(ui, icons::PLUS, "Add a fractal layer above the selected layer").clicked() {
                cmds.push(Cmd::AddLayer);
            }
        });
    });
    egui::Frame::new().fill(theme::BG).corner_radius(6).inner_margin(4).show(ui, |ui| {
        ui.set_width(ui.available_width());
        egui::ScrollArea::vertical().id_salt("layer-tree").max_height(220.0).show(ui, |ui| {
            rows(ui, &doc.layers, &[], 0, sel, &mut cmds);
        });
    });
    let mut message = None;
    for c in cmds {
        message = apply(doc, sel, c).or(message);
    }
    fix(doc, sel);
    message
}

/// A free layer name like "Layer 3".
fn new_name(doc: &Document, base: &str) -> String {
    fn count(list: &[Layer]) -> usize {
        list.iter().map(|l| 1 + count(&l.children)).sum()
    }
    format!("{base} {}", count(&doc.layers) + 1)
}

fn apply(doc: &mut Document, sel: &mut Selection, c: Cmd) -> Option<String> {
    match c {
        Cmd::Select(s) => *sel = s,
        Cmd::ToggleVisible(p) => {
            if let Some(l) = doc.layer_mut(&p) {
                l.visible = !l.visible;
            }
        }
        Cmd::AddLayer | Cmd::AddGroup | Cmd::AddDensity => {
            let layer = if matches!(c, Cmd::AddGroup) {
                Layer { name: new_name(doc, "Group"), kind: LayerKind::Group, ..Layer::default() }
            } else if matches!(c, Cmd::AddDensity) {
                // A Buddhabrot that adds its light to the layers below.
                let mut l =
                    Layer { name: new_name(doc, "Density"), kind: LayerKind::Density, merge: rfr_core::doc::MergeMode::Screen, ..Layer::default() };
                l.outside.gradient = rfr_core::gradient::preset("Ember").unwrap_or_default();
                l
            } else {
                // A new layer copies the selected layer's colors, but starts as a Mandelbrot
                // in Screen mode so it shows through.
                let mut l = target(doc, &Selection::layer(sel.path.clone())).filter(|l| l.kind == LayerKind::Fractal).cloned().unwrap_or_default();
                l.name = new_name(doc, "Layer");
                l.mask = None;
                l.transforms.clear();
                l.merge = rfr_core::doc::MergeMode::Screen;
                l.opacity = 1.0;
                l.visible = true;
                l
            };
            let (list, at) = insert_point(doc, sel);
            list.insert(at, layer);
            let mut p = sel.path.clone();
            if let Some(last) = p.last_mut() {
                *last = at;
            } else {
                p = vec![at];
            }
            *sel = Selection::layer(p);
            return Some("Added a layer (Ctrl+Z goes back)".into());
        }
        Cmd::Duplicate => {
            let l = doc.layer(&sel.path)?.clone();
            let i = *sel.path.last()?;
            let list = doc.siblings_mut(&sel.path)?;
            let mut copy = l;
            copy.name = format!("{} copy", copy.name);
            list.insert(i + 1, copy);
            *sel.path.last_mut()? = i + 1;
            sel.mask = false;
        }
        Cmd::Delete => {
            if sel.mask {
                doc.layer_mut(&sel.path)?.mask = None;
                sel.mask = false;
                return Some("Removed the mask".into());
            }
            let total: usize = doc.layers.len();
            if sel.path.len() == 1 && total <= 1 {
                return Some("The document needs at least one layer".into());
            }
            let i = *sel.path.last()?;
            let list = doc.siblings_mut(&sel.path)?;
            list.remove(i);
            if list.is_empty() {
                // An emptied group stays selected.
                sel.path.pop();
            } else {
                *sel.path.last_mut()? = i.saturating_sub(1).min(list.len() - 1);
            }
            return Some("Deleted the layer (Ctrl+Z brings it back)".into());
        }
        Cmd::Up | Cmd::Down => {
            let i = *sel.path.last()?;
            let list = doc.siblings_mut(&sel.path)?;
            let j = if matches!(c, Cmd::Up) { i + 1 } else { i.checked_sub(1)? };
            if j >= list.len() {
                return None;
            }
            list.swap(i, j);
            *sel.path.last_mut()? = j;
        }
        Cmd::Group => {
            let i = *sel.path.last()?;
            let list = doc.siblings_mut(&sel.path)?;
            let l = list.remove(i);
            let g = Layer { name: format!("{} group", l.name), kind: LayerKind::Group, children: vec![l], ..Layer::default() };
            list.insert(i, g);
            sel.path.push(0);
            sel.mask = false;
        }
        Cmd::Ungroup => {
            let l = doc.layer(&sel.path)?.clone();
            if l.kind != LayerKind::Group {
                return Some("Select a group to ungroup".into());
            }
            let i = *sel.path.last()?;
            let list = doc.siblings_mut(&sel.path)?;
            list.remove(i);
            let n = l.children.len();
            for (k, c) in l.children.into_iter().enumerate() {
                list.insert(i + k, c);
            }
            if n == 0 {
                *sel.path.last_mut()? = i.saturating_sub(1);
            } else {
                *sel.path.last_mut()? = i + n - 1;
            }
        }
        Cmd::AddMask => {
            let l = doc.layer_mut(&sel.path)?;
            if l.mask.is_none() {
                l.mask = Some(Mask::default());
            }
            sel.mask = true;
            return Some("Added a mask: pick its formula below, its brightness decides where the layer shows".into());
        }
    }
    None
}

/// Where a new layer goes: above the selection (into a selected group's top).
fn insert_point<'a>(doc: &'a mut Document, sel: &mut Selection) -> (&'a mut Vec<Layer>, usize) {
    let is_group = doc.layer(&sel.path).is_some_and(|l| l.kind == LayerKind::Group);
    if is_group && !sel.mask {
        let len = doc.layer(&sel.path).map_or(0, |l| l.children.len());
        sel.path.push(len);
        let parent: Vec<usize> = sel.path[..sel.path.len() - 1].to_vec();
        return (&mut doc.layer_mut(&parent).expect("group").children, len);
    }
    let at = sel.path.last().map_or(doc.layers.len(), |i| i + 1);
    if sel.path.len() <= 1 {
        let n = doc.layers.len();
        return (&mut doc.layers, at.min(n));
    }
    let parent: Vec<usize> = sel.path[..sel.path.len() - 1].to_vec();
    let list = &mut doc.layer_mut(&parent).expect("parent").children;
    let n = list.len();
    (list, at.min(n))
}
