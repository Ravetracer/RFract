// SPDX-License-Identifier: LGPL-3.0-or-later

//! The application window: top bar, formula browser (left), inspector (right), timeline and
//! explore lane (bottom, later stages), view (center), status bar; file handling, undo and the
//! keyboard shortcuts.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use egui::{RichText, Ui};
use rfr_core::doc::Document;

use crate::browser::{Browser, with_formula};
use crate::doc::Doc;
use crate::export::Export;
use crate::help::{Help, Tab};
use crate::inspector::{self, Action as InspectorAction};
use crate::prefs::Prefs;
use crate::viewport::Viewport;
use crate::{icons, theme, widgets};

#[derive(Clone, Debug)]
enum Action {
    New,
    Open,
    OpenPath(PathBuf),
    Save,
    SaveAs,
    Export,
    Undo,
    Redo,
    ResetView,
    ToggleJulia,
    Quit,
}

/// What to do once unsaved changes are saved or discarded.
#[derive(Clone, Debug)]
enum Pending {
    Close,
    Replace(Box<Action>),
}

pub struct App {
    prefs: Prefs,
    prefs_open: bool,
    help: Help,
    show_timeline: bool,
    show_explore: bool,
    doc: Doc,
    viewport: Viewport,
    browser: Browser,
    export: Export,
    /// Gradient editor window: outside (false) or inside (true) coloring.
    gradient_window: Option<bool>,
    title: String,
    pending: Option<Pending>,
    allow_close: bool,
    denoiser: Option<String>,
    denoiser_rx: Option<Receiver<String>>,
    status: Option<(String, Instant, bool)>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, initial: Option<PathBuf>) -> Self {
        let prefs = Prefs::load();
        widgets::set_closed_sections(&prefs.closed_sections);
        icons::install(&cc.egui_ctx);
        theme::apply(&cc.egui_ctx, prefs.font_percent);
        cc.egui_ctx.set_zoom_factor(prefs.ui_scale_percent as f32 / 100.0);
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let s = match rfr_core::render::oidn::library() {
                Ok(p) => format!("Denoiser: {}", p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()),
                Err(e) => format!("Denoiser not available: {e}"),
            };
            let _ = tx.send(s);
        });
        let mut app = App {
            prefs,
            prefs_open: false,
            help: Help::default(),
            show_timeline: false,
            show_explore: false,
            doc: Doc::new(Document::for_formula("rfract:Mandelbrot")),
            viewport: Viewport::new(cc),
            browser: Browser::default(),
            export: Export::default(),
            gradient_window: None,
            title: String::new(),
            pending: None,
            allow_close: false,
            denoiser: None,
            denoiser_rx: Some(rx),
            status: None,
        };
        if let Some(p) = initial {
            app.open_path(&p);
        }
        app
    }

    fn notify(&mut self, msg: impl Into<String>) {
        self.status = Some((msg.into(), Instant::now(), false));
    }

    fn warn(&mut self, msg: impl Into<String>) {
        self.status = Some((msg.into(), Instant::now(), true));
    }

    fn save_prefs(&mut self) {
        if let Err(e) = self.prefs.save() {
            self.warn(format!("Could not save the preferences: {e}"));
        }
    }

    fn remember_dir(&mut self, p: &Path) {
        if let Some(d) = p.parent() {
            self.prefs.last_dir = Some(d.to_path_buf());
        }
    }

    fn open_path(&mut self, p: &Path) {
        match rfr_core::io::open(p) {
            Ok(d) => {
                let is_png = p.extension().is_some_and(|e| e.eq_ignore_ascii_case("png"));
                // A PNG is a picture, not the document file: saving asks for a new name.
                self.doc.load(d, if is_png { None } else { Some(p.to_path_buf()) });
                self.remember_dir(p);
                self.prefs.add_recent(p.to_path_buf());
                self.save_prefs();
                self.notify(format!("Opened {}", p.display()));
            }
            Err(e) => self.warn(e),
        }
    }

    fn save_to(&mut self, p: PathBuf) {
        let p = if p.extension().is_none() { p.with_extension("rfr") } else { p };
        match rfr_core::io::save(&self.doc.doc, &p) {
            Ok(()) => {
                self.remember_dir(&p);
                self.prefs.add_recent(p.clone());
                self.save_prefs();
                self.notify(format!("Saved {}", p.display()));
                self.doc.mark_saved(p);
            }
            Err(e) => self.warn(format!("Could not save: {e}")),
        }
    }

    /// Runs an action; actions that would drop unsaved changes ask first.
    fn run(&mut self, a: Action, ctx: &egui::Context) {
        let replaces = matches!(a, Action::New | Action::Open | Action::OpenPath(_));
        if replaces && self.doc.modified() && self.pending.is_none() {
            self.pending = Some(Pending::Replace(Box::new(a)));
            return;
        }
        match a {
            Action::New => {
                let mut d = Document::for_formula("rfract:Mandelbrot");
                d.image = self.doc.doc.image.clone();
                self.doc.load(d, None);
                self.notify("New document");
            }
            Action::Open => {
                if let Some(p) = self.prefs.dialog().add_filter("RFract documents and images", &["rfr", "png"]).pick_file() {
                    self.open_path(&p);
                }
            }
            Action::OpenPath(p) => self.open_path(&p),
            Action::Save => match self.doc.path.clone() {
                Some(p) => self.save_to(p),
                None => self.run(Action::SaveAs, ctx),
            },
            Action::SaveAs => {
                let name = self
                    .doc
                    .path
                    .as_ref()
                    .and_then(|p| p.file_name())
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "fractal.rfr".into());
                if let Some(p) = self.prefs.dialog().add_filter("RFract document", &["rfr"]).set_file_name(name).save_file() {
                    self.save_to(p);
                }
            }
            Action::Export => {
                if self.export.busy() {
                    self.notify("An export is already running");
                    return;
                }
                let stem =
                    self.doc.path.as_ref().and_then(|p| p.file_stem()).map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "fractal".into());
                if let Some(p) = self.prefs.dialog().add_filter("PNG image", &["png"]).set_file_name(format!("{stem}.png")).save_file() {
                    let p = if p.extension().is_none() { p.with_extension("png") } else { p };
                    self.remember_dir(&p);
                    match self.export.start(&self.doc.doc, p, self.viewport.gpu_context()) {
                        Ok(()) => self.notify("Exporting…"),
                        Err(e) => self.warn(format!("Cannot export: {e}")),
                    }
                }
            }
            Action::Undo => {
                if !self.doc.undo() {
                    self.notify("Nothing to undo");
                }
            }
            Action::Redo => {
                if !self.doc.redo() {
                    self.notify("Nothing to redo");
                }
            }
            Action::ResetView => {
                let id = self.doc.doc.active_layer().map(|l| l.formula.clone()).unwrap_or_default();
                if let Some(e) = rfr_core::formula::library::library().get(&id) {
                    let julia = self.doc.doc.active_layer().is_some_and(|l| l.julia);
                    let v = &mut self.doc.doc.view;
                    v.rotation = 0.0;
                    if julia {
                        v.radius = 1.6;
                        v.set_center([0.0, 0.0]);
                    } else {
                        v.radius = e.view.radius;
                        v.set_center(e.view.center);
                    }
                    self.notify("View reset to the formula's suggested view");
                }
            }
            Action::ToggleJulia => {
                let on = self.doc.doc.active_layer().is_some_and(|l| l.julia);
                let seed = if on { None } else { self.viewport.hover.or_else(|| self.doc.doc.active_layer().map(|l| l.seed)) };
                if let Some(m) = self.viewport.set_julia(&mut self.doc.doc, seed) {
                    self.notify(m);
                }
            }
            Action::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) -> Option<Action> {
        use egui::{Key, Modifiers as M};
        let typing = ctx.egui_wants_keyboard_input();
        ctx.input_mut(|i| {
            let shift_cmd = M::COMMAND | M::SHIFT;
            if i.consume_key(shift_cmd, Key::S) {
                return Some(Action::SaveAs);
            }
            if i.consume_key(shift_cmd, Key::Z) || i.consume_key(M::COMMAND, Key::Y) {
                return Some(Action::Redo);
            }
            if i.consume_key(M::COMMAND, Key::N) {
                return Some(Action::New);
            }
            if i.consume_key(M::COMMAND, Key::O) {
                return Some(Action::Open);
            }
            if i.consume_key(M::COMMAND, Key::S) {
                return Some(Action::Save);
            }
            if i.consume_key(M::COMMAND, Key::E) {
                return Some(Action::Export);
            }
            if !typing && i.consume_key(M::COMMAND, Key::Z) {
                return Some(Action::Undo);
            }
            if i.consume_key(M::COMMAND, Key::Q) {
                return Some(Action::Quit);
            }
            if i.consume_key(M::NONE, Key::F1) {
                self.help.open = !self.help.open;
            }
            if i.consume_key(M::COMMAND, Key::Comma) {
                self.prefs_open = true;
            }
            if !typing && i.consume_key(M::NONE, Key::J) {
                return Some(Action::ToggleJulia);
            }
            if !typing && i.consume_key(M::NONE, Key::Home) {
                return Some(Action::ResetView);
            }
            None
        })
    }

    fn top_bar(&mut self, ui: &mut Ui) -> Option<Action> {
        let mut action = None;
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.label(RichText::new(icons::PALETTE).color(theme::ACCENT).size(theme::fs(19.0)));
            ui.label(RichText::new("RFract").strong().size(theme::fs(15.0)));
            ui.add_space(10.0);
            ui.menu_button("File", |ui| {
                ui.set_min_width(230.0);
                let item = |ui: &mut Ui, t: &str, k: &str| ui.add(egui::Button::new(t).shortcut_text(k));
                if item(ui, &format!("{}  New", icons::NEW), "Ctrl+N").on_hover_text("A new document with the Mandelbrot set").clicked() {
                    action = Some(Action::New);
                }
                if item(ui, &format!("{}  Open…", icons::OPEN), "Ctrl+O")
                    .on_hover_text("RFract documents (.rfr) and PNG images rendered by RFract")
                    .clicked()
                {
                    action = Some(Action::Open);
                }
                ui.menu_button("Open recent", |ui| {
                    if self.prefs.recent.is_empty() {
                        ui.label(RichText::new("Nothing yet").color(theme::TEXT_MUTED));
                    }
                    for p in self.prefs.recent.clone() {
                        if ui
                            .button(p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default())
                            .on_hover_text(p.display().to_string())
                            .clicked()
                        {
                            action = Some(Action::OpenPath(p));
                        }
                    }
                });
                ui.separator();
                if item(ui, &format!("{}  Save", icons::SAVE), "Ctrl+S").on_hover_text("Save the document (.rfr)").clicked() {
                    action = Some(Action::Save);
                }
                if item(ui, "Save as…", "Ctrl+Shift+S").on_hover_text("Save the document under a new name").clicked() {
                    action = Some(Action::SaveAs);
                }
                if item(ui, &format!("{}  Export image…", icons::EXPORT), "Ctrl+E")
                    .on_hover_text("Render at the image size (Image section) and save a PNG that also holds the document")
                    .clicked()
                {
                    action = Some(Action::Export);
                }
                ui.separator();
                if item(ui, "Preferences…", "Ctrl+,").on_hover_text("Interface size, preview quality").clicked() {
                    self.prefs_open = true;
                }
                if item(ui, "Quit", "Ctrl+Q").on_hover_text("Close RFract").clicked() {
                    action = Some(Action::Quit);
                }
            });
            ui.menu_button("Edit", |ui| {
                ui.set_min_width(200.0);
                if ui
                    .add_enabled(self.doc.can_undo(), egui::Button::new(format!("{}  Undo", icons::UNDO)).shortcut_text("Ctrl+Z"))
                    .on_hover_text("Undo the last change (also view changes)")
                    .clicked()
                {
                    action = Some(Action::Undo);
                }
                if ui
                    .add_enabled(self.doc.can_redo(), egui::Button::new(format!("{}  Redo", icons::REDO)).shortcut_text("Ctrl+Shift+Z"))
                    .on_hover_text("Redo the undone change")
                    .clicked()
                {
                    action = Some(Action::Redo);
                }
            });
            ui.menu_button("View", |ui| {
                ui.set_min_width(220.0);
                if ui
                    .add(egui::Button::new(format!("{}  Reset view", icons::RESET)).shortcut_text("Home"))
                    .on_hover_text("Back to the formula's suggested view")
                    .clicked()
                {
                    action = Some(Action::ResetView);
                }
                if ui
                    .add(egui::Button::new("Julia / Mandelbrot").shortcut_text("J"))
                    .on_hover_text("Switch to the Julia set of the point under the cursor, or back")
                    .clicked()
                {
                    action = Some(Action::ToggleJulia);
                }
                ui.separator();
                ui.checkbox(&mut self.show_timeline, "Timeline").on_hover_text("Show the animation timeline below the view (later stage)");
                ui.checkbox(&mut self.show_explore, "Explore lane").on_hover_text("Show the explore lane below the view (later stage)");
            });
            ui.menu_button("Help", |ui| {
                ui.set_min_width(220.0);
                if ui.add(egui::Button::new("Manual").shortcut_text("F1")).on_hover_text("The user manual inside the program").clicked() {
                    self.help.open = true;
                    self.help.tab = Tab::Manual;
                }
                if ui.button("PDF manual").on_hover_text("Open the PDF version of the manual with the system's viewer").clicked() {
                    match crate::manual::open_pdf() {
                        Ok(p) => self.notify(format!("Opened {}", p.display())),
                        Err(e) => self.warn(e),
                    }
                }
                if ui.button("Shortcuts").on_hover_text("Keyboard and mouse").clicked() {
                    self.help.open = true;
                    self.help.tab = Tab::Shortcuts;
                }
                if ui.button("About RFract").on_hover_text("Version, license and credits").clicked() {
                    self.help.open = true;
                    self.help.tab = Tab::About;
                }
            });
            ui.add_space(12.0);
            if widgets::icon_button(ui, icons::UNDO, "Undo (Ctrl+Z)").clicked() {
                action = Some(Action::Undo);
            }
            if widgets::icon_button(ui, icons::REDO, "Redo (Ctrl+Shift+Z)").clicked() {
                action = Some(Action::Redo);
            }
            if widgets::icon_button(ui, icons::EXPORT, "Export image (Ctrl+E)").clicked() {
                action = Some(Action::Export);
            }
        });
        action
    }

    fn status_bar(&mut self, ui: &mut Ui) {
        ui.horizontal_centered(|ui| {
            if self.status.as_ref().is_some_and(|(_, t, err)| t.elapsed() > Duration::from_secs(if *err { 15 } else { 6 })) {
                self.status = None;
            }
            if let Some((m, _, err)) = &self.status {
                ui.label(RichText::new(m).color(if *err { theme::WARN } else { theme::TEXT }).size(theme::fs(12.0)));
                ui.ctx().request_repaint_after(Duration::from_secs(1));
            } else if self.viewport.hovered {
                ui.label(
                    RichText::new("Drag: move · Wheel: zoom · Shift/right-drag: box zoom · Ctrl-drag: rotate · Double-click: zoom in · Alt-click: Julia")
                        .size(theme::fs(11.5))
                        .color(theme::TEXT_MUTED),
                );
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let small = |t: &str| RichText::new(t).size(theme::fs(11.5)).color(theme::TEXT_MUTED);
                let tip = format!("The graphics card RFract renders with. {}", self.denoiser.as_deref().unwrap_or(""));
                ui.label(small(&format!("{} {}", icons::GPU, self.viewport.adapter_name()))).on_hover_text(tip);
                ui.separator();
                ui.label(small(&self.viewport.status())).on_hover_text("Preview size, samples per pixel and the arithmetic the shader uses");
                ui.separator();
                ui.label(small(&format!("zoom {:.3e}×", 1.5 / self.doc.doc.view.radius))).on_hover_text("Magnification of the view");
                if let Some(h) = self.viewport.hover {
                    ui.separator();
                    ui.label(small(&format!("{:.10}, {:.10}", h[0], h[1]))).on_hover_text("Plane coordinate under the cursor");
                }
                if self.viewport.beyond_precision() {
                    ui.separator();
                    ui.label(RichText::new(format!("{} beyond double precision", icons::WARNING)).size(theme::fs(11.5)).color(theme::WARN))
                        .on_hover_text("This view is deeper than double precision reaches (pixels below about 2e-15 of the coordinates): rounding steps show. Deeper zooms come with the deep zoom stage.");
                }
            });
        });
    }

    fn prefs_window(&mut self, ctx: &egui::Context) {
        if !self.prefs_open {
            return;
        }
        let mut open = true;
        let before = self.prefs.clone();
        egui::Window::new("Preferences").open(&mut open).resizable(false).collapsible(false).default_width(420.0).show(ctx, |ui| {
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
            if widgets::section(ui, "Preview", false).open {
                widgets::grid(ui, "prefs-preview", |ui| {
                    widgets::slider_u32(
                        ui,
                        "Resolution",
                        "Preview resolution in percent of the view size. Lower is faster.",
                        &mut p.preview_percent,
                        25..=100,
                        d.preview_percent,
                    );
                    widgets::slider_u32(
                        ui,
                        "While navigating",
                        "Resolution while you drag or zoom, in percent of the preview resolution",
                        &mut p.navigate_percent,
                        10..=100,
                        d.navigate_percent,
                    );
                    widgets::slider_u32(
                        ui,
                        "Samples",
                        "The preview stops refining after this many samples per pixel",
                        &mut p.preview_samples,
                        1..=1024,
                        d.preview_samples,
                    );
                });
            }
            ui.add_space(6.0);
            ui.label(
                RichText::new(format!("Settings folder: {}", crate::prefs::config_dir().display())).size(theme::fs(11.0)).color(theme::TEXT_MUTED),
            );
        });
        self.prefs_open = open;
        let dragging = ctx.input(|i| i.pointer.any_down());
        if self.prefs.font_percent != before.font_percent {
            theme::apply(ctx, self.prefs.font_percent);
        }
        if !dragging && (ctx.zoom_factor() * 100.0).round() as u32 != self.prefs.ui_scale_percent {
            ctx.set_zoom_factor(self.prefs.ui_scale_percent as f32 / 100.0);
        }
        if self.prefs != before {
            self.save_prefs();
        }
    }

    fn gradient_editor(&mut self, ctx: &egui::Context) {
        let Some(inside) = self.gradient_window else { return };
        let mut open = true;
        let title = if inside { "Gradient – inside" } else { "Gradient – outside" };
        egui::Window::new(title).id(egui::Id::new("gradient-editor")).open(&mut open).default_width(440.0).resizable(true).collapsible(false).show(
            ctx,
            |ui| {
                if let Some(l) = self.doc.doc.layers.iter_mut().find(|l| l.visible) {
                    let g = if inside { &mut l.inside.gradient } else { &mut l.outside.gradient };
                    egui::ScrollArea::vertical().max_height(ctx.content_rect().height() - 160.0).show(ui, |ui| {
                        crate::palette_editor::gradient_editor(ui, g);
                    });
                }
            },
        );
        if !open {
            self.gradient_window = None;
        }
    }

    /// The "save changes?" question before closing or replacing the document.
    fn pending_dialog(&mut self, ctx: &egui::Context) {
        let Some(pending) = self.pending.clone() else { return };
        let mut choice = None;
        egui::Window::new("Unsaved changes").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
            ui.label("The document has changes that are not saved.");
            ui.horizontal(|ui| {
                if ui.button("Save").on_hover_text("Save the document first").clicked() {
                    choice = Some(0);
                }
                if ui.button("Discard").on_hover_text("Continue without saving").clicked() {
                    choice = Some(1);
                }
                if ui.button("Cancel").on_hover_text("Keep working on the document").clicked() {
                    choice = Some(2);
                }
            });
        });
        let Some(c) = choice else { return };
        self.pending = None;
        if c == 2 {
            return;
        }
        if c == 0 {
            self.run(Action::Save, ctx);
            if self.doc.modified() {
                return; // saving was cancelled or failed
            }
        }
        match pending {
            Pending::Close => {
                self.allow_close = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Pending::Replace(a) => {
                // The document is saved or discarded: run without asking again.
                let saved = self.doc.doc.clone();
                self.doc.load(saved, self.doc.path.clone());
                self.run(*a, ctx);
            }
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if let Some(rx) = &self.denoiser_rx
            && let Ok(s) = rx.try_recv()
        {
            self.denoiser = Some(s);
            self.denoiser_rx = None;
        }
        // Closing with unsaved changes asks first.
        if ctx.input(|i| i.viewport().close_requested()) && self.doc.modified() && !self.allow_close {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.pending = Some(Pending::Close);
        }
        let closed = widgets::closed_sections();
        if closed != self.prefs.closed_sections {
            self.prefs.closed_sections = closed;
            self.save_prefs();
        }
        let dropped: Vec<PathBuf> =
            ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect());
        if let Some(p) = dropped.into_iter().next() {
            self.run(Action::OpenPath(p), &ctx);
        }
        if let Some(a) = self.shortcuts(&ctx) {
            self.run(a, &ctx);
        }

        egui::Panel::top("topbar").exact_size(46.0).frame(egui::Frame::new().fill(theme::PANEL).inner_margin(egui::Margin::symmetric(12, 6))).show(
            ui,
            |ui| {
                if let Some(a) = self.top_bar(ui) {
                    self.run(a, &ctx);
                }
            },
        );
        egui::Panel::bottom("status").exact_size(26.0).frame(egui::Frame::new().fill(theme::BG).inner_margin(egui::Margin::symmetric(12, 3))).show(
            ui,
            |ui| {
                self.status_bar(ui);
            },
        );
        let mut picked = None;
        egui::Panel::left("browser")
            .resizable(true)
            .default_size(280.0)
            .size_range(220.0..=480.0)
            .frame(egui::Frame::new().fill(theme::PANEL).inner_margin(egui::Margin::symmetric(10, 8)))
            .show(ui, |ui| {
                picked = self.browser.show(ui, &self.doc.doc);
            });
        if let Some(id) = picked {
            let d = with_formula(&self.doc.doc, &id);
            self.doc.replace(d);
            let title = rfr_core::formula::library::library().get(&id).map(|e| e.title.clone()).unwrap_or(id);
            self.notify(format!("Formula: {title} (Ctrl+Z goes back)"));
        }
        let mut actions = Vec::new();
        egui::Panel::right("inspector")
            .resizable(true)
            .default_size(350.0)
            .size_range(300.0..=540.0)
            .frame(egui::Frame::new().fill(theme::PANEL).inner_margin(egui::Margin::symmetric(12, 10)))
            .show(ui, |ui| {
                actions = inspector::show(ui, &mut self.doc.doc);
            });
        for a in actions {
            match a {
                InspectorAction::Message(m) => self.notify(m),
                InspectorAction::PickJulia => {
                    self.viewport.picking_julia = true;
                    self.notify("Click a point in the view to see its Julia set");
                }
                InspectorAction::Julia(on) => {
                    let seed = if on { self.doc.doc.active_layer().map(|l| l.seed) } else { None };
                    if let Some(m) = self.viewport.set_julia(&mut self.doc.doc, seed) {
                        self.notify(m);
                    }
                }
                InspectorAction::EditGradient(inside) => self.gradient_window = Some(inside),
            }
        }
        if self.show_explore {
            egui::Panel::bottom("explore")
                .resizable(false)
                .exact_size(120.0)
                .frame(egui::Frame::new().fill(theme::PANEL).inner_margin(egui::Margin::symmetric(12, 8)))
                .show(ui, |ui| {
                    ui.label(RichText::new("Explore lane: presets, random documents and mutations (later stage)").color(theme::TEXT_MUTED));
                });
        }
        if self.show_timeline {
            egui::Panel::bottom("timeline")
                .resizable(true)
                .default_size(160.0)
                .size_range(120.0..=420.0)
                .frame(egui::Frame::new().fill(theme::PANEL).inner_margin(egui::Margin::symmetric(12, 8)))
                .show(ui, |ui| {
                    ui.label(RichText::new("Timeline: keyframes, camera path, recording and playback (later stage)").color(theme::TEXT_MUTED));
                });
        }
        self.viewport.paused = self.export.busy();
        let mut msg = None;
        egui::CentralPanel::default().frame(egui::Frame::new().fill(theme::CANVAS)).show(ui, |ui| {
            msg = self.viewport.show(ui, frame, &mut self.doc.doc, &self.prefs);
        });
        if let Some(m) = msg {
            self.notify(m);
        }
        if let Some(m) = self.export.update(&ctx) {
            if m.starts_with("Export failed") { self.warn(m) } else { self.notify(m) }
        }

        self.gradient_editor(&ctx);
        self.help.show(&ctx);
        self.prefs_window(&ctx);
        self.pending_dialog(&ctx);

        // One undo step per gesture.
        if !ctx.input(|i| i.pointer.any_down()) {
            self.doc.commit();
        }
        let title = self.doc.title();
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }
        if self.viewport.busy(&self.prefs) {
            ctx.request_repaint();
        }
    }
}
