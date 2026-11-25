// SPDX-License-Identifier: LGPL-3.0-or-later

//! Phosphor icons used in the UI.

pub use egui_phosphor::regular::{
    ARROWS_CLOCKWISE as REFRESH, CARET_DOWN, CARET_RIGHT, CPU, FILM_STRIP as TIMELINE, GRAPHICS_CARD as GPU, MAGIC_WAND as EXPLORE, PALETTE,
    SLIDERS as INSPECTOR, STACK as LAYERS,
};

pub fn install(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    ctx.set_fonts(fonts);
}
