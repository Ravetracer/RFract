// SPDX-License-Identifier: LGPL-3.0-or-later

//! Phosphor icons used in the UI.

pub use egui_phosphor::regular::{
    ARROW_COUNTER_CLOCKWISE as RESET, ARROW_DOWN, ARROW_U_UP_LEFT as UNDO, ARROW_U_UP_RIGHT as REDO, ARROW_UP, ARROWS_CLOCKWISE as REFRESH,
    CARET_DOWN, CARET_RIGHT, CIRCLE_HALF as MASK, DOTS_THREE as DOTS, DOWNLOAD_SIMPLE as EXPORT, EYE, EYE_SLASH, FILE as NEW, FLOPPY_DISK as SAVE,
    FOLDER, FOLDER_OPEN as OPEN, FOLDER_PLUS, FUNCTION, GRAPHICS_CARD as GPU, MAGNIFYING_GLASS as SEARCH, PALETTE, PLAY, PLUS, SPARKLE as DENSITY,
    STACK as LAYERS, TRASH, UPLOAD_SIMPLE as IMPORT, WARNING, X,
};

pub fn install(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    ctx.set_fonts(fonts);
}
