// SPDX-License-Identifier: LGPL-3.0-or-later

//! Timeline panel: keyframes, playback, timing, palette rotation and motion blur of the
//! document's animation (`Document::animation`). Adapted from RMandelbulb3D (`timeline.rs`).
//!
//! Editing always happens on the document. Scrubbing and playback show frames in the view
//! without touching the document ("preview"); clicking a keyframe loads it for editing,
//! "Update" stores the current document back into it.

use std::time::Instant;

use egui::{Color32, RichText, Sense, Ui, pos2, vec2};
use egui_phosphor::regular as ph;
use rfr_core::animation::{Animation, AnimationPlan, Easing, Keyframe, Timing};
use rfr_core::doc::Document;

use crate::{theme, widgets};

/// What the panel asks the app to do.
pub enum TimelineAction {
    ExportAnimation,
    Message(String),
}

#[derive(Default)]
pub struct Timeline {
    frame: usize,
    playing: bool,
    play_pos: f64,
    last_tick: Option<Instant>,
    /// A frame is shown in the view instead of the document.
    previewing: bool,
    selected: Option<usize>,
    plan: Option<(Animation, Result<AnimationPlan, String>)>,
}

/// The document without its animation (what a keyframe stores).
fn snapshot(doc: &Document) -> Document {
    Document { animation: None, ..doc.clone() }
}

impl Timeline {
    /// The cached plan of `anim` (rebuilt when it changes).
    fn plan(&mut self, anim: &Animation) -> Option<&AnimationPlan> {
        if self.plan.as_ref().is_none_or(|(a, _)| a != anim) {
            let p = if anim.keyframes.is_empty() { Err("no keyframes".into()) } else { AnimationPlan::new(anim) };
            self.plan = Some((anim.clone(), p));
        }
        self.plan.as_ref().and_then(|(_, p)| p.as_ref().ok())
    }

    /// Back to editing the document.
    pub fn stop_preview(&mut self) {
        self.previewing = false;
        self.playing = false;
    }

    /// The frame the view shows instead of the document (document, palette phase, fast).
    pub fn preview(&mut self, doc: &Document) -> Option<(Document, f64, bool)> {
        if !self.previewing {
            return None;
        }
        let anim = doc.animation.as_deref()?.clone();
        let (frame, playing) = (self.frame, self.playing);
        let p = self.plan(&anim)?;
        let f = frame.min(p.frame_count().saturating_sub(1));
        Some((p.doc_at(f as f64), p.palette_phase(f as f64), playing))
    }

    pub fn show(&mut self, ui: &mut Ui, doc: &mut Document) -> Option<TimelineAction> {
        let mut action = None;
        let anim = doc.animation.as_deref().cloned().unwrap_or_default();
        let fps = anim.fps.clamp(1.0, 240.0);
        let (count, key_frames, plan_err) = match self.plan(&anim) {
            Some(p) => (p.frame_count(), p.key_frames(), None),
            None => (0, Vec::new(), self.plan.as_ref().and_then(|(_, p)| p.as_ref().err().cloned())),
        };
        if self.selected.is_some_and(|s| s >= anim.keyframes.len()) {
            self.selected = None;
        }
        // Playback clock.
        if self.playing && count > 1 {
            let now = Instant::now();
            let dt = self.last_tick.map(|t| now.duration_since(t).as_secs_f64()).unwrap_or(0.0);
            self.last_tick = Some(now);
            self.play_pos = (self.play_pos + dt * fps) % count as f64;
            self.frame = self.play_pos as usize;
            ui.ctx().request_repaint();
        } else {
            self.last_tick = None;
        }
        self.frame = self.frame.min(count.saturating_sub(1));

        // ---- transport row
        ui.horizontal(|ui| {
            ui.add_enabled_ui(count > 1, |ui| {
                if widgets::icon_button(ui, ph::SKIP_BACK, "First frame").clicked() {
                    self.frame = 0;
                    self.previewing = true;
                    self.playing = false;
                }
                let (icon, tip) =
                    if self.playing { (ph::PAUSE, "Pause") } else { (ph::PLAY, "Play the animation in the view (fast preview quality)") };
                if widgets::icon_button(ui, icon, tip).clicked() {
                    self.playing = !self.playing;
                    self.play_pos = self.frame as f64;
                    self.previewing = true;
                }
                if widgets::icon_button(ui, ph::SKIP_FORWARD, "Last frame").clicked() {
                    self.frame = count.saturating_sub(1);
                    self.previewing = true;
                    self.playing = false;
                }
            });
            ui.label(
                RichText::new(format!(
                    "Frame {} / {}  ·  {:.2} s / {:.2} s",
                    self.frame,
                    count.saturating_sub(1),
                    self.frame as f64 / fps,
                    count.saturating_sub(1) as f64 / fps
                ))
                .color(theme::TEXT_MUTED)
                .monospace(),
            );
            if self.previewing
                && ui
                    .button(format!("{}  Edit", ph::PENCIL_SIMPLE))
                    .on_hover_text("Back to the document (the view shows an animation frame now)")
                    .clicked()
            {
                self.stop_preview();
            }
            ui.separator();
            ui.add_enabled_ui(count > 0, |ui| {
                if ui
                    .button(format!("{}  Export animation…", crate::icons::EXPORT))
                    .on_hover_text("Render the animation into a movie or PNG images")
                    .clicked()
                {
                    action = Some(TimelineAction::ExportAnimation);
                }
            });
            if let Some(e) = &plan_err
                && !anim.keyframes.is_empty()
            {
                ui.label(RichText::new(e).color(theme::WARN));
            }
        });

        // ---- scrubber
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 4.0, theme::BG);
        let x_of = |f: usize| rect.left() + 8.0 + (rect.width() - 16.0) * if count > 1 { f as f32 / (count - 1) as f32 } else { 0.0 };
        if count > 1 {
            let secs = (count - 1) as f64 / fps;
            let step = [1.0, 2.0, 5.0, 10.0, 30.0, 60.0].into_iter().find(|s| secs / s <= 20.0).unwrap_or(120.0);
            let mut s = 0.0;
            while s <= secs {
                let x = x_of((s * fps).round() as usize);
                painter.line_segment([pos2(x, rect.bottom() - 6.0), pos2(x, rect.bottom())], egui::Stroke::new(1.0, theme::BORDER));
                s += step;
            }
        }
        for (k, f) in key_frames.iter().enumerate() {
            let c = pos2(x_of(*f), rect.center().y);
            let r = if self.selected == Some(k) { 7.0 } else { 5.5 };
            let fill = if self.selected == Some(k) { theme::ACCENT } else { Color32::from_rgb(255, 200, 80) };
            painter.add(egui::Shape::convex_polygon(
                vec![c + vec2(0.0, -r), c + vec2(r, 0.0), c + vec2(0.0, r), c + vec2(-r, 0.0)],
                fill,
                egui::Stroke::new(1.0, Color32::BLACK),
            ));
        }
        if count > 0 {
            let x = x_of(self.frame);
            painter.line_segment(
                [pos2(x, rect.top()), pos2(x, rect.bottom())],
                egui::Stroke::new(2.0, if self.previewing { Color32::WHITE } else { theme::TEXT_MUTED }),
            );
        }
        if count > 1
            && let Some(p) = resp.interact_pointer_pos()
        {
            let near_key = key_frames.iter().position(|f| (x_of(*f) - p.x).abs() < 7.0);
            if resp.clicked()
                && let Some(k) = near_key
            {
                self.select_key(doc, k, key_frames[k]);
            } else if resp.dragged() || resp.clicked() {
                let t = ((p.x - rect.left() - 8.0) / (rect.width() - 16.0)).clamp(0.0, 1.0);
                self.frame = (t * (count - 1) as f32).round() as usize;
                self.previewing = true;
                self.playing = false;
            }
        }
        resp.on_hover_text("Click or drag to look at a frame; click a diamond to edit that keyframe");

        // ---- settings
        ui.add_space(4.0);
        let mut a = anim.clone();
        egui::ScrollArea::horizontal().show(ui, |ui| {
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(330.0);
                    if let Some(m) = self.keyframe_ui(ui, doc, &mut a, &key_frames) {
                        action = Some(m);
                    }
                });
                ui.separator();
                ui.vertical(|ui| {
                    ui.set_width(300.0);
                    timing_ui(ui, &mut a);
                });
            });
        });
        if a != anim {
            doc.animation = if a.keyframes.is_empty() && doc.animation.is_none() { None } else { Some(Box::new(a)) };
        }
        if self.previewing && doc.animation.as_ref().is_none_or(|a| a.keyframes.is_empty()) {
            self.stop_preview();
        }
        action
    }

    /// Loads keyframe `k` into the document for editing.
    fn select_key(&mut self, doc: &mut Document, k: usize, frame: usize) {
        let Some(anim) = doc.animation.clone() else { return };
        if let Some(key) = anim.keyframes.get(k) {
            *doc = Document { animation: Some(anim.clone()), ..key.doc.clone() };
            self.selected = Some(k);
            self.frame = frame;
            self.stop_preview();
        }
    }

    fn keyframe_ui(&mut self, ui: &mut Ui, doc: &mut Document, a: &mut Animation, key_frames: &[usize]) -> Option<TimelineAction> {
        let mut msg = None;
        ui.horizontal(|ui| {
            ui.label(RichText::new("KEYFRAMES").size(theme::fs(11.0)).color(theme::TEXT_MUTED));
            if ui
                .small_button(format!("{}  Add", ph::PLUS))
                .on_hover_text("Adds the current document as a keyframe (after the selected one, else at the end)")
                .clicked()
            {
                let at = self.selected.map_or(a.keyframes.len(), |s| s + 1);
                let frames = a.keyframes.last().map_or(60, |k| k.frames);
                a.keyframes.insert(at, Keyframe { doc: snapshot(doc), frames, easing: Easing::Linear });
                self.selected = Some(at);
                msg = Some(TimelineAction::Message(format!("Keyframe {} added", at + 1)));
            }
            if let Some(s) = self.selected {
                if ui
                    .small_button(format!("{}  Update", ph::ARROWS_CLOCKWISE))
                    .on_hover_text("Stores the current document in the selected keyframe")
                    .clicked()
                    && let Some(k) = a.keyframes.get_mut(s)
                {
                    k.doc = snapshot(doc);
                    msg = Some(TimelineAction::Message(format!("Keyframe {} updated", s + 1)));
                }
                if ui.small_button(format!("{}  Delete", ph::TRASH)).on_hover_text("Removes the selected keyframe").clicked() && s < a.keyframes.len()
                {
                    a.keyframes.remove(s);
                    self.selected = None;
                    msg = Some(TimelineAction::Message(format!("Keyframe {} removed", s + 1)));
                }
            }
        });
        if a.keyframes.is_empty() {
            ui.label(
                RichText::new("No keyframes: set up the view and the layers, then Add. Each keyframe stores the whole document.")
                    .size(theme::fs(11.5))
                    .color(theme::TEXT_MUTED),
            );
            return msg;
        }
        let per_key = a.timing == Timing::Keyframes;
        egui::ScrollArea::vertical().id_salt("keys").max_height(110.0).show(ui, |ui| {
            let n = a.keyframes.len();
            for (k, key) in a.keyframes.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    let sel = self.selected == Some(k);
                    let label = format!("{} {}", ph::DIAMOND, k + 1);
                    if ui.selectable_label(sel, label).on_hover_text("Load this keyframe into the document to edit it").clicked() {
                        let anim = doc.animation.clone();
                        *doc = Document { animation: anim, ..key.doc.clone() };
                        self.selected = Some(k);
                        self.frame = key_frames.get(k).copied().unwrap_or(0);
                        self.stop_preview();
                    }
                    ui.label(
                        RichText::new(format!("zoom {:.3e}", key.doc.view.magnification().to_f64())).color(theme::TEXT_MUTED).size(theme::fs(11.0)),
                    );
                    if per_key && k + 1 < n {
                        ui.add(egui::DragValue::new(&mut key.frames).range(1..=100_000).suffix(" frames"))
                            .on_hover_text("Frames from this keyframe to the next");
                        egui::ComboBox::from_id_salt(("easing", k)).width(110.0).selected_text(key.easing.label()).show_ui(ui, |ui| {
                            for e in Easing::ALL {
                                ui.selectable_value(&mut key.easing, e, e.label());
                            }
                        });
                    }
                });
            }
        });
        msg
    }
}

fn timing_ui(ui: &mut Ui, a: &mut Animation) {
    ui.label(RichText::new("TIMING").size(theme::fs(11.0)).color(theme::TEXT_MUTED));
    let d = Animation::default();
    widgets::grid(ui, "anim-timing", |ui| {
        widgets::label(ui, "Frames per second", "Frame rate of the movie");
        ui.add(egui::DragValue::new(&mut a.fps).range(1.0..=120.0).speed(0.2)).on_hover_text("Frames per second");
        ui.end_row();
        widgets::label(ui, "Timing", "How the frames are spread over the keyframes");
        ui.horizontal(|ui| {
            for t in Timing::ALL {
                ui.selectable_value(&mut a.timing, t, t.label()).on_hover_text(match t {
                    Timing::Keyframes => "Frames and easing set per keyframe",
                    Timing::Steady => "One steady speed (zoom, movement and turning as seen) over the whole length",
                });
            }
        });
        ui.end_row();
        if a.timing == Timing::Steady {
            widgets::slider_log(ui, "Length", "Length of the animation in seconds", &mut a.length_s, 0.5..=3600.0, d.length_s);
            widgets::slider(ui, "Ease in", "Seconds to speed up at the start", &mut a.ease_in_s, 0.0..=30.0, d.ease_in_s);
            widgets::slider(ui, "Ease out", "Seconds to slow down at the end", &mut a.ease_out_s, 0.0..=30.0, d.ease_out_s);
        }
        widgets::slider(
            ui,
            "Palette rotation",
            "Gradient turns per second for all colorings (0: off)",
            &mut a.palette_speed,
            -2.0..=2.0,
            d.palette_speed,
        );
        widgets::checkbox(
            ui,
            "Zoom sequence",
            "Exports a straight zoom from the first keyframe's size into the last keyframe (its center and settings), composed from one image per doubling of the zoom: much faster for long zooms. Palette rotation does not apply.",
            &mut a.zoom_sequence,
        );
        widgets::slider(
            ui,
            "Motion blur",
            "How long each frame is exposed, as a share of the frame time (0: off, 0.5: a film camera's 180° shutter)",
            &mut a.shutter,
            0.0..=1.0,
            d.shutter,
        );
    });
}
