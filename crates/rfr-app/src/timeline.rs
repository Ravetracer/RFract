// SPDX-License-Identifier: LGPL-3.0-or-later

//! Timeline panel: keyframes, playback, timing, palette rotation and motion blur of the
//! document's animation (`Document::animation`). Adapted from RMandelbulb3D (`timeline.rs`).
//!
//! Editing always happens on the document. Scrubbing and playback show frames in the view
//! without touching the document ("preview"); clicking a keyframe loads it for editing,
//! "Update" stores the current document back into it. A recorded flight (the view and the 3D
//! cameras while navigating, fly mode included) becomes keyframes with the recorded timing,
//! smoothed by the cameraman.

use std::time::Instant;

use egui::{Color32, RichText, Sense, Ui, pos2, vec2};
use egui_phosphor::regular as ph;
use rfr_core::animation::{Animation, AnimationPlan, Easing, FlightSample, Keyframe, Timing, keyframes_from_flight, smooth_flight};
use rfr_core::doc::Document;

use crate::viewport::{Recording, Viewport};
use crate::{theme, widgets};

/// What the panel asks the app to do.
pub enum TimelineAction {
    ExportAnimation,
    Message(String),
}

/// The last recorded flight and how it becomes keyframes.
struct Flight {
    samples: Vec<FlightSample>,
    /// Cameraman smoothing in seconds.
    smooth_s: f64,
    /// Keyframe detail: how closely the keyframes follow the flight.
    tolerance: f64,
}

impl Default for Flight {
    fn default() -> Self {
        Flight { samples: Vec::new(), smooth_s: 0.5, tolerance: 0.05 }
    }
}

#[derive(Default)]
pub struct Timeline {
    flight: Flight,
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

    pub fn show(&mut self, ui: &mut Ui, doc: &mut Document, view: &mut Viewport) -> Option<TimelineAction> {
        let mut action = None;
        let recording = view.recording.is_some();
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
            ui.add_enabled_ui(count > 1 && !recording, |ui| {
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
            let rec_text = if recording {
                RichText::new(format!("{}  Stop recording", ph::STOP)).color(Color32::from_rgb(255, 110, 100))
            } else {
                RichText::new(format!("{}  Record flight", ph::RECORD))
            };
            if ui
                .button(rec_text)
                .on_hover_text(
                    "Records the view and the 3D cameras while you navigate (fly mode: F). When you stop, keyframes follow the flight with its timing; smoothing and detail are set under FLIGHT",
                )
                .clicked()
            {
                if let Some(rec) = view.recording.take() {
                    action = Some(self.finish_recording(doc, rec.samples));
                } else {
                    self.stop_preview();
                    view.recording = Some(Recording { start: Instant::now(), samples: Vec::new() });
                    action = Some(TimelineAction::Message("Recording: navigate or fly (F), then Stop recording".into()));
                }
            }
            ui.add_enabled_ui(count > 0 && !recording, |ui| {
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
                ui.separator();
                ui.vertical(|ui| {
                    ui.set_width(300.0);
                    if let Some(m) = self.flight_ui(ui, doc, &mut a, view) {
                        action = Some(m);
                    }
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

    /// Keeps a recorded flight (without the still time before and after it) and makes
    /// keyframes of it.
    fn finish_recording(&mut self, doc: &mut Document, samples: Vec<FlightSample>) -> TimelineAction {
        let moved = |a: &FlightSample, b: &FlightSample| a.view != b.view || a.cameras != b.cameras;
        let first = samples.iter().position(|s| moved(s, &samples[0]));
        let last = samples.iter().rposition(|s| samples.last().is_some_and(|e| moved(s, e)));
        let (Some(first), Some(last)) = (first, last) else {
            return TimelineAction::Message("Nothing recorded: the view did not move".into());
        };
        let (a, b) = (first - 1, (last + 1).min(samples.len() - 1));
        let t0 = samples[a].t;
        self.flight.samples = samples[a..=b].iter().map(|s| FlightSample { t: s.t - t0, ..s.clone() }).collect();
        let n = self.make_keyframes(doc);
        TimelineAction::Message(format!("Recorded {:.1} s: {n} keyframes", self.flight.samples.last().map_or(0.0, |s| s.t)))
    }

    /// Replaces the keyframes by keyframes along the recorded flight (recorded timing).
    fn make_keyframes(&mut self, doc: &mut Document) -> usize {
        let mut a = doc.animation.as_deref().cloned().unwrap_or_default();
        let smooth = smooth_flight(&self.flight.samples, self.flight.smooth_s);
        a.keyframes = keyframes_from_flight(&smooth, self.flight.tolerance, &snapshot(doc), a.fps.clamp(1.0, 240.0));
        a.timing = Timing::Recorded;
        let n = a.keyframes.len();
        doc.animation = Some(Box::new(a));
        self.selected = None;
        self.frame = 0;
        n
    }

    fn flight_ui(&mut self, ui: &mut Ui, doc: &mut Document, a: &mut Animation, view: &mut Viewport) -> Option<TimelineAction> {
        let mut msg = None;
        ui.label(RichText::new("FLIGHT").size(theme::fs(11.0)).color(theme::TEXT_MUTED));
        ui.checkbox(&mut view.fly, "Fly mode (F)").on_hover_text(
            "Hold the left mouse button to fly into the fractal toward the cursor, the right button to fly out; the wheel sets the speed",
        );
        if self.flight.samples.is_empty() {
            ui.label(
                RichText::new("Record flight, navigate or fly, stop: keyframes follow the flight.").size(theme::fs(11.5)).color(theme::TEXT_MUTED),
            );
            return None;
        }
        let secs = self.flight.samples.last().map_or(0.0, |s| s.t);
        ui.label(RichText::new(format!("Recorded: {secs:.1} s, {} samples", self.flight.samples.len())).color(theme::TEXT_MUTED));
        let d = Flight::default();
        widgets::grid(ui, "flight", |ui| {
            widgets::slider(
                ui,
                "Cameraman",
                "Smooths the flight like a camera operator: seconds over which zooming, moving and turning are evened out (0: as flown)",
                &mut self.flight.smooth_s,
                0.0..=5.0,
                d.smooth_s,
            );
            widgets::slider_log(
                ui,
                "Tolerance",
                "How far the keyframe path may stray from the flight (share of the view; smaller: more keyframes, closer to the flight)",
                &mut self.flight.tolerance,
                0.005..=0.5,
                d.tolerance,
            );
        });
        ui.horizontal(|ui| {
            if ui
                .button(format!("{}  Keyframes again", ph::ARROWS_CLOCKWISE))
                .on_hover_text("Makes the keyframes from the recorded flight again with these settings (replaces the keyframes; the other settings of the document come from the current document)")
                .clicked()
            {
                let n = self.make_keyframes(doc);
                *a = doc.animation.as_deref().cloned().unwrap_or_default();
                msg = Some(TimelineAction::Message(format!("{n} keyframes from the recorded flight")));
            }
            if ui.button("Forget").on_hover_text("Removes the recorded flight (the keyframes stay)").clicked() {
                self.flight.samples.clear();
                msg = Some(TimelineAction::Message("Recorded flight removed".into()));
            }
        });
        msg
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
                a.keyframes.insert(at, Keyframe { doc: snapshot(doc), frames, easing: Easing::Linear, time_s: None });
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
                    Timing::Recorded => "The pace of the recorded flight (keyframe times), with its speed changes smoothed",
                });
            }
        });
        ui.end_row();
        if a.timing == Timing::Steady {
            widgets::slider_log(ui, "Length", "Length of the animation in seconds", &mut a.length_s, 0.5..=3600.0, d.length_s);
            widgets::slider(ui, "Ease in", "Seconds to speed up at the start", &mut a.ease_in_s, 0.0..=30.0, d.ease_in_s);
            widgets::slider(ui, "Ease out", "Seconds to slow down at the end", &mut a.ease_out_s, 0.0..=30.0, d.ease_out_s);
        }
        if a.timing == Timing::Recorded {
            widgets::slider(
                ui,
                "Smooth speed",
                "Seconds over which speed changes of the flight are smoothed (0: exactly as flown)",
                &mut a.speed_smoothing_s,
                0.0..=10.0,
                d.speed_smoothing_s,
            );
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
