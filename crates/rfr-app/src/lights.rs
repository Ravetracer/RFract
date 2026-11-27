// SPDX-License-Identifier: LGPL-3.0-or-later

//! The document's light sources (glows and lens flares, `rfr_core::flare`): the inspector
//! section, which light is selected (shared with the gizmos in the view) and where a light is
//! on the screen.

use egui::{Pos2, Rect, RichText, Ui};
use rfr_core::doc::{Document, LightSource};
use rfr_core::flare::{GlowShape, LensFlare};

use crate::{icons, theme, widgets};

fn sel_id() -> egui::Id {
    egui::Id::new("rfract-selected-light")
}

/// Index of the selected light (may be past the end when lights were removed).
pub fn selected(ctx: &egui::Context) -> usize {
    ctx.data(|d| d.get_temp(sel_id()).unwrap_or(0))
}

pub fn select(ctx: &egui::Context, i: usize) {
    ctx.data_mut(|d| d.insert_temp(sel_id(), i));
}

/// Screen position of `light` in the image rectangle `rect`.
pub fn screen_of(doc: &Document, rect: Rect, light: &LightSource) -> Option<Pos2> {
    let (w, h) = (doc.image.width.max(1), doc.image.height.max(1));
    let (x, y) = rfr_core::flare::screen_pos(doc, light, w, h)?;
    let p = Pos2::new(rect.min.x + (x / w as f64) as f32 * rect.width(), rect.min.y + (y / h as f64) as f32 * rect.height());
    p.is_finite().then_some(p)
}

/// The Lights section of the inspector: the list of lights and the selected one's settings.
pub fn section(ui: &mut Ui, doc: &mut Document) {
    let sec = widgets::section(ui, "Lights", false);
    if !sec.open {
        return;
    }
    let ctx = ui.ctx().clone();
    let mut sel = selected(&ctx);
    let mut remove = None;
    for (i, l) in doc.lights.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            ui.checkbox(&mut l.enabled, "").on_hover_text("Light on or off");
            let name = RichText::new(&l.name).color(if i == sel { theme::TEXT } else { theme::TEXT_MUTED });
            if ui.selectable_label(i == sel, name).on_hover_text("Show this light's settings (its gizmo in the view is highlighted)").clicked() {
                sel = i;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if widgets::icon_button(ui, icons::TRASH, "Remove this light").clicked() {
                    remove = Some(i);
                }
            });
        });
    }
    if let Some(i) = remove {
        doc.lights.remove(i);
    }
    if ui
        .button(format!("{}  Add a light", icons::PLUS))
        .on_hover_text("A new light at the center of the view, shown as a glow; drag its circle in the view to move it")
        .clicked()
    {
        let n = doc.lights.len() + 1;
        doc.lights.push(LightSource { name: format!("Light {n}"), position: doc.view.center.clone(), ..LightSource::default() });
        sel = doc.lights.len() - 1;
    }
    if doc.lights.is_empty() {
        ui.label(
            RichText::new("No lights. A light is a point in the plane that shows as a glow, optionally with a lens flare.")
                .color(theme::TEXT_MUTED)
                .size(theme::fs(11.5)),
        );
        select(&ctx, 0);
        return;
    }
    sel = sel.min(doc.lights.len() - 1);
    select(&ctx, sel);
    let l = &mut doc.lights[sel];
    let d = LightSource::default();
    widgets::grid(ui, "light-main", |ui| {
        widgets::label(ui, "Name", "Name of the light");
        ui.add(egui::TextEdit::singleline(&mut l.name).desired_width(160.0)).on_hover_text("Name of the light");
        ui.end_row();
        widgets::label(ui, "Position", "Where the light is in the plane (it moves with the fractal); drag its circle in the view");
        ui.vertical(|ui| {
            ui.add(egui::TextEdit::singleline(&mut l.position[0]).desired_width(160.0)).on_hover_text("Real part (exact decimal)");
            ui.add(egui::TextEdit::singleline(&mut l.position[1]).desired_width(160.0)).on_hover_text("Imaginary part (exact decimal)");
        });
        ui.end_row();
        widgets::label(ui, "", "");
        if ui.button("To the view center").on_hover_text("Move the light to the center of the view").clicked() {
            l.position = doc.view.center.clone();
        }
        ui.end_row();
        widgets::slider(
            ui,
            "Height",
            "Height above the plane as a share of the image height: where it lights lit layers from (Document lights) and where it hangs over a 3D terrain",
            &mut l.height,
            0.0..=2.0,
            d.height,
        );
        widgets::color(ui, "Color", "Color of the light (its glow, its flare and the light it casts)", &mut l.color);
        widgets::slider(ui, "Intensity", "Brightness of the glow and the flare together", &mut l.intensity, 0.0..=5.0, d.intensity);
        widgets::checkbox(ui, "Visible", "The light itself shows as a glow", &mut l.visible);
        if l.visible {
            widgets::label(ui, "Glow shape", "How the light itself looks");
            egui::ComboBox::from_id_salt("light-glow-shape").selected_text(l.glow_shape.label()).show_ui(ui, |ui| {
                for s in GlowShape::ALL {
                    let tip = match s {
                        GlowShape::Soft => "A bright core with a soft falloff",
                        GlowShape::Sharp => "A small sharp spot",
                        GlowShape::Ball => "A glowing ball with a bright rim",
                        GlowShape::Halo => "A halo, brightest in the middle",
                    };
                    ui.selectable_value(&mut l.glow_shape, s, s.label()).on_hover_text(tip);
                }
            });
            ui.end_row();
            widgets::slider_log(ui, "Glow size", "Radius of the glow as a share of the image height", &mut l.glow_size, 0.002..=0.5, d.glow_size);
            widgets::slider(ui, "Glow strength", "Brightness of the glow", &mut l.glow_intensity, 0.0..=5.0, d.glow_intensity);
        }
    });
    flare_ui(ui, &mut l.flare);
}

fn sub(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).color(theme::TEXT_MUTED).size(theme::fs(11.5)));
}

/// The lens flare of a light: on, presets and the parts of the flare.
fn flare_ui(ui: &mut Ui, f: &mut LensFlare) {
    sub(ui, "Lens flare");
    ui.horizontal(|ui| {
        ui.checkbox(&mut f.enabled, "Lens flare").on_hover_text("The reflections and the star a camera lens adds around a bright light");
        egui::ComboBox::from_id_salt("flare-preset").selected_text("Presets").show_ui(ui, |ui| {
            for (name, p) in LensFlare::presets() {
                if ui.selectable_label(false, name).on_hover_text("Replace the flare settings with this look (turns the flare on)").clicked() {
                    *f = p;
                }
            }
        });
    });
    if !f.enabled {
        return;
    }
    let d = LensFlare::default();
    widgets::grid(ui, "flare-main", |ui| {
        widgets::slider(ui, "Strength", "Brightness of the whole flare", &mut f.intensity, 0.0..=3.0, d.intensity);
        widgets::color(ui, "Tint", "Multiplies the light's color for the flare", &mut f.tint);
        widgets::slider(ui, "Core", "Bright core around the light", &mut f.glow, 0.0..=3.0, d.glow);
        widgets::slider_log(ui, "Core size", "Radius of the core as a share of the image height", &mut f.glow_size, 0.002..=0.3, d.glow_size);
        widgets::slider(ui, "Haze", "Wide soft glow that lifts the dark parts", &mut f.haze, 0.0..=1.0, d.haze);
        widgets::slider(ui, "Haze size", "Radius of the haze as a share of the image height", &mut f.haze_size, 0.05..=1.5, d.haze_size);
    });
    sub(ui, "Rays");
    widgets::grid(ui, "flare-rays", |ui| {
        widgets::slider_u32(ui, "Star rays", "Spikes of the diffraction star (0: none)", &mut f.rays, 0..=16, d.rays);
        if f.rays > 0 {
            widgets::slider(ui, "Ray strength", "Brightness of the star", &mut f.ray_intensity, 0.0..=2.0, d.ray_intensity);
            widgets::slider(ui, "Ray length", "Length of the spikes as a share of the image height", &mut f.ray_length, 0.02..=1.0, d.ray_length);
            widgets::slider(ui, "Ray width", "Thickness of the spikes (1: thin)", &mut f.ray_width, 0.2..=4.0, d.ray_width);
            widgets::slider(ui, "Ray angle", "Turn of the star in degrees", &mut f.ray_rotation, 0.0..=180.0, d.ray_rotation);
            widgets::slider(ui, "Ray variety", "Uneven spike lengths and brightness (0: all alike)", &mut f.ray_random, 0.0..=1.0, d.ray_random);
        }
        widgets::slider_u32(ui, "Fine spikes", "Many thin random spikes around the light (0: none)", &mut f.spikes, 0..=400, d.spikes);
        if f.spikes > 0 {
            widgets::slider(ui, "Spike strength", "Brightness of the fine spikes", &mut f.spike_intensity, 0.0..=1.0, d.spike_intensity);
            widgets::slider(
                ui,
                "Spike length",
                "Length of the fine spikes as a share of the image height",
                &mut f.spike_length,
                0.02..=1.0,
                d.spike_length,
            );
        }
    });
    sub(ui, "Streak");
    widgets::grid(ui, "flare-streak", |ui| {
        widgets::slider(ui, "Streak", "Long thin anamorphic line through the light (0: none)", &mut f.streak, 0.0..=3.0, d.streak);
        if f.streak > 0.0 {
            widgets::slider(ui, "Streak length", "Length as a share of the image height", &mut f.streak_length, 0.1..=4.0, d.streak_length);
            widgets::slider(ui, "Streak width", "Thickness of the line", &mut f.streak_width, 0.2..=4.0, d.streak_width);
            widgets::slider(ui, "Streak angle", "Direction in degrees (0: horizontal)", &mut f.streak_angle, 0.0..=180.0, d.streak_angle);
            widgets::color(ui, "Streak color", "Color of the streak and the sparkles", &mut f.streak_color);
        }
        widgets::slider_u32(ui, "Sparkles", "Small dots along the streak line (0: none)", &mut f.sparkles, 0..=200, d.sparkles);
        if f.sparkles > 0 {
            widgets::slider(ui, "Sparkle strength", "Brightness of the sparkles", &mut f.sparkle_intensity, 0.0..=1.0, d.sparkle_intensity);
        }
    });
    sub(ui, "Ghosts and ring");
    widgets::grid(ui, "flare-ghosts", |ui| {
        widgets::slider_u32(
            ui,
            "Ghosts",
            "Reflections of the aperture on the line from the light through the image center (0: none)",
            &mut f.ghosts,
            0..=32,
            d.ghosts,
        );
        if f.ghosts > 0 || !f.ghost_list.is_empty() {
            widgets::slider(ui, "Ghost strength", "Brightness of the ghosts", &mut f.ghost_intensity, 0.0..=1.5, d.ghost_intensity);
            widgets::slider(ui, "Ghost size", "Typical radius as a share of the image height", &mut f.ghost_size, 0.01..=0.4, d.ghost_size);
            widgets::slider(ui, "Ghost spread", "How far the ghosts spread along their line", &mut f.ghost_spread, 0.1..=3.0, d.ghost_spread);
            widgets::slider_u32(ui, "Aperture blades", "Corners of the ghosts (0: round)", &mut f.ghost_blades, 0..=12, d.ghost_blades);
            widgets::slider(ui, "Ghost softness", "Soft (1) or sharp (0) ghost edges", &mut f.ghost_softness, 0.0..=1.0, d.ghost_softness);
            widgets::slider(ui, "Hollow ghosts", "Share of ghosts that are rings", &mut f.ghost_rings, 0.0..=1.0, d.ghost_rings);
            widgets::slider(ui, "Ghost colors", "0: the light's color, 1: varied lens coating tints", &mut f.ghost_colors, 0.0..=1.0, d.ghost_colors);
            widgets::label(ui, "Bokeh image", "An image that shapes every ghost by its brightness (empty: the aperture blades)");
            ui.add(egui::TextEdit::singleline(&mut f.ghost_image).desired_width(160.0).hint_text("image file"))
                .on_hover_text("Path of an image (PNG, JPEG) that shapes the ghosts; empty: the aperture blades");
            ui.end_row();
        }
        widgets::slider(ui, "Chromatic", "Colored fringes of the ghosts and the ring", &mut f.chromatic, 0.0..=2.0, d.chromatic);
        widgets::slider(ui, "Ring", "Halo ring around the light (0: none)", &mut f.ring, 0.0..=1.0, d.ring);
        if f.ring > 0.0 {
            widgets::slider(ui, "Ring size", "Radius of the ring as a share of the image height", &mut f.ring_size, 0.05..=1.0, d.ring_size);
            widgets::slider(ui, "Ring width", "Thickness of the ring", &mut f.ring_width, 0.002..=0.1, d.ring_width);
            widgets::slider(ui, "Rainbow", "Rainbow colors in the ring (0: the light's color)", &mut f.rainbow, 0.0..=1.0, d.rainbow);
        }
    });
    sub(ui, "Lens dirt");
    widgets::grid(ui, "flare-dirt", |ui| {
        widgets::slider(ui, "Dirt", "Soft specks on the lens lit by the light (0: clean)", &mut f.dirt, 0.0..=2.0, d.dirt);
        if f.dirt > 0.0 {
            widgets::slider(ui, "Dirt reach", "How far from the light the dirt lights up", &mut f.dirt_reach, 0.1..=2.0, d.dirt_reach);
        }
        widgets::label(ui, "Variation", "Seed of the random spikes, ghosts, sparkles and dirt");
        ui.horizontal(|ui| {
            ui.add(egui::DragValue::new(&mut f.seed).range(0..=99_999)).on_hover_text("Random seed: another number gives another layout");
            if ui.button("New").on_hover_text("A new random layout").clicked() {
                f.seed = rfr_core::render::hash(f.seed.wrapping_add(1)) % 100_000;
            }
        });
        ui.end_row();
    });
}
