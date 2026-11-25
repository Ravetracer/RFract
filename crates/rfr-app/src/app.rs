// SPDX-License-Identifier: LGPL-3.0-or-later

//! The application window: top bar, layer panel (left), inspector (right), timeline and
//! explore lane (bottom), viewport (center), status bar. Stage 0 is the empty shell in its
//! final layout; the panels get their content with the stages in PLAN.md.

use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use egui::{RichText, Ui};

use crate::help::{Help, Tab};
use crate::prefs::Prefs;
use crate::{icons, theme, widgets};

pub struct App {
    prefs: Prefs,
    prefs_open: bool,
    help: Help,
    show_timeline: bool,
    show_explore: bool,
    /// The GPU eframe renders with (also the one the renderer will use).
    adapter: String,
    /// Denoiser status, found on a worker thread (loading the library takes a moment).
    denoiser: Option<String>,
    denoiser_rx: Option<Receiver<String>>,
    status: Option<(String, Instant, bool)>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let prefs = Prefs::load();
        widgets::set_closed_sections(&prefs.closed_sections);
        icons::install(&cc.egui_ctx);
        theme::apply(&cc.egui_ctx, prefs.font_percent);
        cc.egui_ctx.set_zoom_factor(prefs.ui_scale_percent as f32 / 100.0);
        let adapter = cc.wgpu_render_state.as_ref().map(|rs| {
            let info = rs.adapter.get_info();
            format!("{} ({:?})", info.name, info.backend)
        });
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let s = match rfr_core::render::oidn::library() {
                Ok(p) => format!("Denoiser: {}", p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()),
                Err(e) => format!("Denoiser not available: {e}"),
            };
            let _ = tx.send(s);
        });
        App {
            prefs,
            prefs_open: false,
            help: Help::default(),
            show_timeline: false,
            show_explore: false,
            adapter: adapter.unwrap_or_else(|| "no GPU".into()),
            denoiser: None,
            denoiser_rx: Some(rx),
            status: None,
        }
    }

    fn notify(&mut self, msg: impl Into<String>, error: bool) {
        self.status = Some((msg.into(), Instant::now(), error));
    }

    fn save_prefs(&mut self) {
        if let Err(e) = self.prefs.save() {
            self.notify(format!("Could not save the preferences: {e}"), true);
        }
    }

    fn open_help(&mut self, tab: Tab) {
        self.help.open = true;
        self.help.tab = tab;
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        let (help, prefs, quit) = ctx.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, egui::Key::F1),
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::Comma),
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::Q),
            )
        });
        if help {
            self.help.open = !self.help.open;
        }
        if prefs {
            self.prefs_open = true;
        }
        if quit {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn top_bar(&mut self, ui: &mut Ui) {
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.label(RichText::new(icons::PALETTE).color(theme::ACCENT).size(theme::fs(19.0)));
            ui.label(RichText::new("RFract").strong().size(theme::fs(15.0)));
            ui.add_space(10.0);
            ui.menu_button("File", |ui| {
                ui.set_min_width(220.0);
                if ui.add(egui::Button::new("Preferences…").shortcut_text("Ctrl+,")).on_hover_text("Interface size and font size").clicked() {
                    self.prefs_open = true;
                }
                ui.separator();
                if ui.add(egui::Button::new("Quit").shortcut_text("Ctrl+Q")).on_hover_text("Close RFract").clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            ui.menu_button("View", |ui| {
                ui.set_min_width(220.0);
                ui.checkbox(&mut self.show_timeline, "Timeline").on_hover_text("Show the animation timeline below the view");
                ui.checkbox(&mut self.show_explore, "Explore lane")
                    .on_hover_text("Show the explore lane (presets, random documents, mutations) below the view");
            });
            ui.menu_button("Help", |ui| {
                ui.set_min_width(220.0);
                if ui.add(egui::Button::new("Manual").shortcut_text("F1")).on_hover_text("The user manual inside the program").clicked() {
                    self.open_help(Tab::Manual);
                }
                if ui.button("PDF manual").on_hover_text("Open the PDF version of the manual with the system's viewer").clicked() {
                    match crate::manual::open_pdf() {
                        Ok(p) => self.notify(format!("Opened {}", p.display()), false),
                        Err(e) => self.notify(e, true),
                    }
                }
                if ui.button("Shortcuts").on_hover_text("Keyboard shortcuts").clicked() {
                    self.open_help(Tab::Shortcuts);
                }
                if ui.button("About RFract").on_hover_text("Version, license and credits").clicked() {
                    self.open_help(Tab::About);
                }
            });
        });
    }

    fn status_bar(&mut self, ui: &mut Ui) {
        ui.horizontal_centered(|ui| {
            if self.status.as_ref().is_some_and(|(_, t, err)| t.elapsed() > Duration::from_secs(if *err { 15 } else { 6 })) {
                self.status = None;
            }
            if let Some((m, _, err)) = &self.status {
                ui.label(RichText::new(m).color(if *err { theme::WARN } else { theme::TEXT }).size(theme::fs(12.0)));
                ui.ctx().request_repaint_after(Duration::from_secs(1));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let small = |t: &str| RichText::new(t).size(theme::fs(11.5)).color(theme::TEXT_MUTED);
                ui.label(small(&format!("{} {}", icons::GPU, self.adapter))).on_hover_text("The graphics card RFract renders with");
                ui.separator();
                ui.label(small(&rfr_core::name_version()));
            });
        });
    }

    /// A panel placeholder: title and what the panel will hold.
    fn placeholder(ui: &mut Ui, icon: &str, title: &str, text: &str) {
        ui.horizontal(|ui| {
            ui.label(RichText::new(icon).color(theme::ACCENT).size(theme::fs(15.0)));
            ui.label(RichText::new(title).strong());
        });
        ui.add_space(6.0);
        ui.label(RichText::new(text).color(theme::TEXT_MUTED).size(theme::fs(12.0)));
    }

    fn viewport(&mut self, ui: &mut Ui) {
        ui.centered_and_justified(|ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(ui.available_height() * 0.35);
                ui.label(RichText::new("RFract").size(theme::fs(32.0)).strong().color(theme::TEXT));
                ui.label(RichText::new("2D fractal generator · development shell").color(theme::TEXT_MUTED));
                ui.add_space(14.0);
                ui.label(RichText::new(format!("{} {}", icons::GPU, self.adapter)).color(theme::TEXT_MUTED).size(theme::fs(12.0)));
                let d = self.denoiser.as_deref().unwrap_or("Looking for the denoiser…");
                ui.label(RichText::new(format!("{} {d}", icons::CPU)).color(theme::TEXT_MUTED).size(theme::fs(12.0)));
            });
        });
    }

    fn prefs_window(&mut self, ctx: &egui::Context) {
        if !self.prefs_open {
            return;
        }
        let mut open = true;
        let before = self.prefs.clone();
        egui::Window::new("Preferences").open(&mut open).resizable(false).collapsible(false).default_width(400.0).show(ctx, |ui| {
            let d = Prefs::default();
            let p = &mut self.prefs;
            if widgets::section(ui, "Appearance", false).open {
                widgets::grid(ui, "prefs-look", |ui| {
                    widgets::slider_u32(
                        ui,
                        "UI scale",
                        "Size of the whole interface in percent: texts, controls and spacing together",
                        &mut p.ui_scale_percent,
                        50..=200,
                        d.ui_scale_percent,
                    );
                    widgets::slider_u32(
                        ui,
                        "Font size",
                        "Size of the texts only, in percent of the built-in size",
                        &mut p.font_percent,
                        50..=200,
                        d.font_percent,
                    );
                });
            }
            ui.add_space(6.0);
            ui.label(
                RichText::new(format!("Settings folder: {}", crate::prefs::config_dir().display())).size(theme::fs(11.0)).color(theme::TEXT_MUTED),
            );
        });
        self.prefs_open = open;
        // Apply size changes once the slider is released (scaling while dragging moves the slider).
        let dragging = ctx.input(|i| i.pointer.any_down());
        if self.prefs != before || (!dragging && self.applied_scale(ctx) != self.prefs.ui_scale_percent) {
            if self.prefs.font_percent != before.font_percent {
                theme::apply(ctx, self.prefs.font_percent);
            }
            if !dragging {
                ctx.set_zoom_factor(self.prefs.ui_scale_percent as f32 / 100.0);
            }
            if self.prefs != before {
                self.save_prefs();
            }
        }
    }

    fn applied_scale(&self, ctx: &egui::Context) -> u32 {
        (ctx.zoom_factor() * 100.0).round() as u32
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if let Some(rx) = &self.denoiser_rx
            && let Ok(s) = rx.try_recv()
        {
            self.denoiser = Some(s);
            self.denoiser_rx = None;
        } else if self.denoiser_rx.is_some() {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
        let closed = widgets::closed_sections();
        if closed != self.prefs.closed_sections {
            self.prefs.closed_sections = closed;
            self.save_prefs();
        }
        self.shortcuts(&ctx);

        egui::Panel::top("topbar").exact_size(46.0).frame(egui::Frame::new().fill(theme::PANEL).inner_margin(egui::Margin::symmetric(12, 6))).show(
            ui,
            |ui| {
                self.top_bar(ui);
            },
        );
        egui::Panel::bottom("status").exact_size(26.0).frame(egui::Frame::new().fill(theme::BG).inner_margin(egui::Margin::symmetric(12, 3))).show(
            ui,
            |ui| {
                self.status_bar(ui);
            },
        );
        egui::Panel::left("layers")
            .resizable(true)
            .default_size(300.0)
            .size_range(240.0..=480.0)
            .frame(egui::Frame::new().fill(theme::PANEL).inner_margin(egui::Margin::symmetric(10, 8)))
            .show(ui, |ui| {
                Self::placeholder(
                    ui,
                    icons::LAYERS,
                    "Layers",
                    "The layer stack: fractal layers with merge modes, opacity and masks, and the formula browser.",
                );
            });
        egui::Panel::right("inspector")
            .resizable(true)
            .default_size(340.0)
            .size_range(300.0..=520.0)
            .frame(egui::Frame::new().fill(theme::PANEL).inner_margin(egui::Margin::symmetric(12, 10)))
            .show(ui, |ui| {
                Self::placeholder(
                    ui,
                    icons::INSPECTOR,
                    "Inspector",
                    "Formula, transformations, outside and inside coloring, gradient, mask, effects and image settings of the selected layer.",
                );
            });
        if self.show_explore {
            egui::Panel::bottom("explore")
                .resizable(false)
                .exact_size(150.0)
                .frame(egui::Frame::new().fill(theme::PANEL).inner_margin(egui::Margin::symmetric(12, 8)))
                .show(ui, |ui| {
                    Self::placeholder(ui, icons::EXPLORE, "Explore", "Presets, random documents and mutations of the current one.");
                });
        }
        if self.show_timeline {
            egui::Panel::bottom("timeline")
                .resizable(true)
                .default_size(200.0)
                .size_range(150.0..=420.0)
                .frame(egui::Frame::new().fill(theme::PANEL).inner_margin(egui::Margin::symmetric(12, 8)))
                .show(ui, |ui| {
                    Self::placeholder(ui, icons::TIMELINE, "Timeline", "Keyframes, camera path, recording and playback.");
                });
        }
        egui::CentralPanel::default().frame(egui::Frame::new().fill(theme::CANVAS)).show(ui, |ui| {
            self.viewport(ui);
        });

        self.help.show(&ctx);
        self.prefs_window(&ctx);
    }
}
