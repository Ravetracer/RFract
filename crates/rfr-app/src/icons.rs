// SPDX-License-Identifier: LGPL-3.0-or-later

//! Phosphor icons used in the UI.

pub use egui_phosphor::regular::{
    ARROW_COUNTER_CLOCKWISE as RESET, ARROW_U_UP_LEFT as UNDO, ARROW_U_UP_RIGHT as REDO, ARROWS_CLOCKWISE as REFRESH, CARET_DOWN, CARET_RIGHT,
    DOWNLOAD_SIMPLE as EXPORT, FILE as NEW, FLOPPY_DISK as SAVE, FOLDER_OPEN as OPEN, FUNCTION, GRAPHICS_CARD as GPU, MAGNIFYING_GLASS as SEARCH,
    PALETTE, PLUS, STACK as LAYERS, WARNING, X,
};

pub fn install(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    ctx.set_fonts(fonts);
}
