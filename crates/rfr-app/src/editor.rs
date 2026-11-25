// SPDX-License-Identifier: LGPL-3.0-or-later

//! The formula editor: shows the source of one formula entry, checks it while you type and
//! saves it back into its file in a formula folder (the library reloads that file, the view
//! follows). Built-in formulas open read-only and can be copied into the first formula folder.

use std::path::{Path, PathBuf};

use egui::{RichText, Ui};
use rfr_core::formula::ProgramKind;
use rfr_core::formula::library::{Entry, Set};

use crate::theme;

/// What the app does after an editor action.
pub enum EditorAction {
    Message(String),
    Warn(String),
    /// Put this formula on the selected layer (a copy that was just made).
    Use(String),
}

pub struct Editor {
    pub open: bool,
    /// The entry's name and kind.
    entry: String,
    kind: Option<ProgramKind>,
    title: String,
    /// The file the entry comes from (user formulas); `None`: built-in, read-only.
    path: Option<PathBuf>,
    /// The entry's text as edited and as last saved.
    text: String,
    saved: String,
    /// Result of the last check of `checked` (the text it was made for).
    checked: String,
    result: Result<String, String>,
}

impl Default for Editor {
    fn default() -> Self {
        Editor {
            open: false,
            entry: String::new(),
            kind: None,
            title: String::new(),
            path: None,
            text: String::new(),
            saved: String::new(),
            checked: String::new(),
            result: Ok(String::new()),
        }
    }
}

/// Byte range of entry `name` in a formula file: from the start of the line with its name to
/// the closing brace.
fn entry_span(src: &str, name: &str) -> Option<(usize, usize)> {
    let entries = rfr_core::formula::parse::split_entries(src);
    let e = entries.iter().find(|e| e.name == name)?;
    let body_start = e.body.as_ptr() as usize - src.as_ptr() as usize;
    let open = src[..body_start].rfind('{')?;
    let start = src[..open].rfind('\n').map_or(0, |i| i + 1);
    let end = body_start + e.body.len();
    let close = end + src[end..].find('}')? + 1;
    Some((start, close))
}

fn extension(kind: ProgramKind) -> &'static str {
    match kind {
        ProgramKind::Fractal => "ufm",
        ProgramKind::Coloring => "ucl",
        ProgramKind::Transform => "uxf",
    }
}

fn template(kind: ProgramKind, name: &str) -> String {
    match kind {
        ProgramKind::Fractal => format!(
            "{name} {{\n; A new fractal formula.\ninit:\n  z = 0\nloop:\n  z = z^@power + #pixel\nbailout:\n  |z| <= @bailout\ndefault:\n  title = \"{name}\"\n  center = (-0.5, 0)\n  magn = 1\n  maxiter = 250\n  float param power\n    caption = \"Power\"\n    default = 2\n  endparam\n  float param bailout\n    caption = \"Bailout\"\n    default = 1e4\n  endparam\n}}"
        ),
        ProgramKind::Coloring => format!(
            "{name} {{\n; A new coloring formula.\ninit:\n  float closest = 1e30\nloop:\n  closest = min(closest, cabs(#z))\nfinal:\n  #index = closest * @scale\ndefault:\n  title = \"{name}\"\n  float param scale\n    caption = \"Scale\"\n    default = 1\n  endparam\n}}"
        ),
        ProgramKind::Transform => format!(
            "{name} {{\n; A new transformation.\ntransform:\n  #pixel = #pixel * @factor\ndefault:\n  title = \"{name}\"\n  complex param factor\n    caption = \"Factor\"\n    default = (1, 0)\n  endparam\n}}"
        ),
    }
}

impl Editor {
    /// Opens the source of `e`.
    pub fn open_entry(&mut self, e: &Entry) -> Result<(), String> {
        let src = e.source().ok_or("the formula's file cannot be read")?;
        let text = if e.entry.is_empty() {
            src
        } else {
            let (a, b) = entry_span(&src, &e.entry).ok_or_else(|| format!("'{}' is no longer in its file", e.entry))?;
            src[a..b].to_string()
        };
        *self = Editor {
            open: true,
            entry: e.entry.clone(),
            kind: Some(e.kind),
            title: e.title.clone(),
            path: if e.set == Set::User { e.path().map(Path::to_path_buf) } else { None },
            saved: text.clone(),
            text,
            checked: String::new(),
            result: Ok(String::new()),
        };
        Ok(())
    }

    /// Writes a new formula of `kind` into `my-formulas.<ext>` in `folder` and opens it.
    pub fn new_formula(&mut self, folder: &Path, kind: ProgramKind) -> Result<String, String> {
        let file = folder.join(format!("my-formulas.{}", extension(kind)));
        let existing = std::fs::read_to_string(&file).unwrap_or_default();
        let names: Vec<String> = rfr_core::formula::parse::split_entries(&existing).into_iter().map(|e| e.name).collect();
        let name = (1..).map(|k| format!("NewFormula{k}")).find(|n| !names.contains(n)).unwrap_or_default();
        self.append(&file, &existing, &template(kind, &name))?;
        Ok(format!("user:{}:{name}", rel_id(&file)?))
    }

    /// Appends `block` to `file` and reloads it.
    fn append(&mut self, file: &Path, existing: &str, block: &str) -> Result<(), String> {
        let mut text = existing.trim_end().to_string();
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        text.push_str(block);
        text.push('\n');
        std::fs::write(file, &text).map_err(|e| format!("{}: {e}", file.display()))?;
        rfr_core::formula::library::reload_file(file)?;
        Ok(())
    }

    /// Checks the edited entry (parse and type check).
    fn check(&mut self) {
        if self.checked == self.text {
            return;
        }
        self.checked = self.text.clone();
        let kind = self.kind.unwrap_or(ProgramKind::Fractal);
        let pre = rfr_core::formula::parse::preprocess(&self.text);
        let entries = rfr_core::formula::parse::split_entries(&pre);
        self.result = match entries.first() {
            None if self.text.contains("@language: frm-like/1") => {
                rfr_core::formula::compile_frm(&self.text).map(|p| format!("Compiles: {} parameter(s)", p.params.len())).map_err(|e| e.to_string())
            }
            None => Err("no formula entry (name { … })".into()),
            Some(e) => rfr_core::formula::parse::parse_ufm_entry(e)
                .and_then(|a| rfr_core::formula::check::check_kind(&a, kind))
                .map(|p| format!("Compiles: {} parameter(s){}", p.params.len(), if p.has_julia { ", Julia mode" } else { "" }))
                .map_err(|e| e.to_string()),
        };
    }

    /// Saves the entry into its file (replacing the entry's old text) and reloads the file.
    fn save(&mut self) -> Result<String, String> {
        let path = self.path.clone().ok_or("built-in formulas cannot be changed")?;
        let src = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let src = String::from_utf8(src).unwrap_or_else(|e| e.into_bytes().iter().map(|&b| b as char).collect());
        let new = if self.entry.is_empty() {
            self.text.clone()
        } else {
            let (a, b) = entry_span(&src, &self.entry).ok_or_else(|| format!("'{}' is no longer in {}", self.entry, path.display()))?;
            format!("{}{}{}", &src[..a], self.text, &src[b..])
        };
        std::fs::write(&path, new).map_err(|e| format!("{}: {e}", path.display()))?;
        // A renamed entry keeps being edited under its new name.
        if !self.entry.is_empty()
            && let Some(e) = rfr_core::formula::parse::split_entries(&self.text).first()
        {
            self.entry = e.name.clone();
        }
        self.saved = self.text.clone();
        let n = rfr_core::formula::library::reload_file(&path)?;
        Ok(format!("Saved {} ({n} formula(s) in the file)", path.display()))
    }

    /// Copies a built-in entry into `my-formulas.<ext>` of the first formula folder.
    fn copy_to_folder(&mut self) -> Result<String, String> {
        let folder = rfr_core::formula::library::user_folders().into_iter().next().ok_or("add a formula folder in File › Preferences first")?;
        let kind = self.kind.unwrap_or(ProgramKind::Fractal);
        let file = folder.join(format!("my-formulas.{}", extension(kind)));
        let existing = std::fs::read_to_string(&file).unwrap_or_default();
        let names: Vec<String> = rfr_core::formula::parse::split_entries(&existing).into_iter().map(|e| e.name).collect();
        let base = if self.entry.is_empty() { "Copy".to_string() } else { self.entry.clone() };
        let name = std::iter::once(base.clone()).chain((2..).map(|k| format!("{base}{k}"))).find(|n| !names.contains(n)).unwrap_or(base);
        let mut block = self.text.clone();
        if !self.entry.is_empty()
            && let Some(rest) = block.strip_prefix(self.entry.as_str())
        {
            block = format!("{name}{rest}");
        }
        self.append(&file, &existing, &block)?;
        Ok(format!("user:{}:{name}", rel_id(&file)?))
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Option<EditorAction> {
        if !self.open {
            return None;
        }
        self.check();
        let mut action = None;
        let mut open = true;
        let read_only = self.path.is_none();
        let dirty = self.text != self.saved;
        let title = format!("Formula – {}{}", self.title, if dirty { " •" } else { "" });
        egui::Window::new(title).id(egui::Id::new("formula-editor")).open(&mut open).default_size([640.0, 560.0]).resizable(true).show(ctx, |ui| {
            ui.horizontal(|ui| {
                match &self.path {
                    Some(p) => ui.label(RichText::new(p.display().to_string()).size(theme::fs(11.0)).color(theme::TEXT_MUTED)),
                    None => ui.label(RichText::new("Built-in formula (read-only)").size(theme::fs(11.0)).color(theme::TEXT_MUTED)),
                };
            });
            ui.horizontal(|ui| {
                if !read_only {
                    let save = ui
                        .add_enabled(dirty, egui::Button::new("Save"))
                        .on_hover_text("Write the formula into its file; the view updates (Ctrl+S in the editor)");
                    let keys = ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::S));
                    if save.clicked() || (keys && dirty) {
                        action = Some(match self.save() {
                            Ok(m) => EditorAction::Message(m),
                            Err(e) => EditorAction::Warn(e),
                        });
                    }
                    if ui.add_enabled(dirty, egui::Button::new("Revert")).on_hover_text("Go back to the saved text").clicked() {
                        self.text = self.saved.clone();
                    }
                } else if ui
                    .button("Copy to my formulas")
                    .on_hover_text("Copy this formula into the first formula folder (my-formulas file) to change it")
                    .clicked()
                {
                    action = Some(match self.copy_to_folder() {
                        Ok(id) => EditorAction::Use(id),
                        Err(e) => EditorAction::Warn(e),
                    });
                }
                match &self.result {
                    Ok(m) if !m.is_empty() => {
                        ui.label(RichText::new(format!("✔ {m}")).color(theme::ACCENT));
                    }
                    Ok(_) => {}
                    Err(e) => {
                        ui.label(RichText::new(e).color(theme::WARN));
                    }
                }
            });
            ui.separator();
            egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui: &mut Ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut self.text)
                        .code_editor()
                        .interactive(!read_only)
                        .desired_width(f32::INFINITY)
                        .desired_rows(30)
                        .lock_focus(true),
                );
            });
        });
        if !open {
            self.open = false;
        }
        action
    }
}

/// The id part of a file in a formula folder (its path relative to the folder).
fn rel_id(file: &Path) -> Result<String, String> {
    let folders = rfr_core::formula::library::user_folders();
    let root = folders.iter().find(|r| file.starts_with(r)).ok_or("the file is not in a formula folder")?;
    Ok(file.strip_prefix(root).unwrap_or(file).components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/"))
}
