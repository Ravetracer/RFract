// SPDX-License-Identifier: LGPL-3.0-or-later

//! Help window: the user manual (docs/manual), shortcuts, about.

use egui::RichText;

use crate::theme;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Manual,
    Shortcuts,
    About,
}

#[derive(Default)]
pub struct Help {
    pub open: bool,
    pub tab: Tab,
    manual: crate::manual::Manual,
}

const SHORTCUTS: &[(&str, &str)] = &[
    ("Ctrl+N", "New document"),
    ("Ctrl+O", "Open (.rfr, PNG rendered by RFract)"),
    ("Ctrl+S / Ctrl+Shift+S", "Save / Save as"),
    ("Ctrl+E", "Export image"),
    ("Ctrl+Z / Ctrl+Shift+Z", "Undo / Redo (view changes included)"),
    ("Drag", "Move the view"),
    ("Wheel", "Zoom at the cursor"),
    ("Shift+drag, right drag", "Zoom into a box"),
    ("Ctrl+drag", "Rotate the view (a 3D terrain: orbit its camera)"),
    ("Shift+Ctrl+drag", "3D terrain: move the camera up and down"),
    ("Double-click / right click", "Zoom in at the point / zoom out"),
    ("Alt+click", "Julia set of the point (again: back to the Mandelbrot mode)"),
    ("J", "Julia set of the point under the cursor, or back"),
    ("O", "Orbit display on or off"),
    ("I", "Julia preview inset on or off"),
    ("F", "Fly mode on or off"),
    ("Home", "Reset the view to the formula's suggestion"),
    ("F1", "Help"),
    ("Ctrl+,", "Preferences"),
    ("Ctrl+Q", "Quit"),
];

impl Help {
    /// Opens the manual at the chapter whose file name starts with `prefix`.
    pub fn open_chapter(&mut self, prefix: &str) {
        self.open = true;
        self.tab = Tab::Manual;
        self.manual.select(prefix);
    }

    pub fn show(&mut self, ctx: &egui::Context) {
        if !self.open {
            return;
        }
        let mut open = self.open;
        egui::Window::new("Help").open(&mut open).default_size([980.0, 700.0]).resizable(true).collapsible(false).show(ctx, |ui| {
            ui.horizontal(|ui| {
                for (t, l, tip) in [
                    (Tab::Manual, "Manual", "The user manual"),
                    (Tab::Shortcuts, "Shortcuts", "Keyboard shortcuts"),
                    (Tab::About, "About", "Version, license and credits"),
                ] {
                    ui.selectable_value(&mut self.tab, t, l).on_hover_text(tip);
                }
            });
            ui.separator();
            if self.tab == Tab::Manual {
                self.manual.show(ui);
                return;
            }
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| match self.tab {
                Tab::Manual => {}
                Tab::Shortcuts => {
                    egui::Grid::new("keys").num_columns(2).spacing([16.0, 6.0]).striped(true).show(ui, |ui| {
                        for (k, d) in SHORTCUTS {
                            ui.label(RichText::new(*k).monospace().color(theme::ACCENT));
                            ui.label(*d);
                            ui.end_row();
                        }
                    });
                }
                Tab::About => about(ui),
            });
        });
        self.open = open;
    }
}

fn about(ui: &mut egui::Ui) {
    ui.label(RichText::new(rfr_core::name_version()).size(theme::fs(18.0)).strong());
    ui.label("A 2D fractal generator in Rust: layered fractals, GPU rendering, denoising and animation.");
    ui.label("GPU rendering with wgpu; user interface with egui; denoising with Intel Open Image Denoise.");
    ui.label("License: GNU LGPL 3.0 or later.");
    ui.add_space(8.0);
    ui.label(RichText::new("Bundled material").strong());
    ui.horizontal_wrapped(|ui| {
        ui.label("Built-in formulas from");
        ui.hyperlink_to("FractalPark", "https://github.com/noodle-bag/fractalpark");
        ui.label("(MIT license).");
    });
    ui.label("Intel Open Image Denoise and oneTBB (Apache License 2.0).");
    ui.label(RichText::new("License texts: THIRD_PARTY_NOTICES.md in the program folder.").color(theme::TEXT_MUTED));
    ui.hyperlink_to("Source code and releases", "https://github.com/Ravetracer/RFract");
}
