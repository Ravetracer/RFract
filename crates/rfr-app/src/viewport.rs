// SPDX-License-Identifier: LGPL-3.0-or-later

//! The view: progressive GPU preview of the document (in the document's aspect ratio) and the
//! navigation. Drag pans, the wheel zooms around the cursor, Shift-drag or right-drag zooms into
//! a box, Ctrl-drag rotates, double-click zooms in, Alt-click picks a Julia seed. When the
//! selected layer is a 3D terrain, Ctrl-drag orbits its camera and Ctrl-wheel moves it closer
//! or farther; on a 3D object layer dragging and the wheel do that without Ctrl. In fly mode
//! holding the left button flies into the fractal toward the cursor, the right button flies
//! out and the wheel sets the speed, which ramps up and down smoothly. While the user
//! navigates, the preview renders at a lower resolution. A flight recording samples the view
//! and the 3D cameras (timeline).

use std::time::{Duration, Instant};

use egui::{Color32, Pos2, Rect, Stroke, Ui, Vec2};
use rfr_core::animation::FlightSample;
use rfr_core::doc::{Document, View};
use rfr_core::render::{Plan, Precision};
use rfr_gpu::codegen::Mode;
use rfr_gpu::{GpuContext, GpuRenderer};

use crate::layers::{Selection, target, target_mut};
use crate::prefs::Prefs;
use crate::theme;

/// How long after the last navigation input the preview stays in the fast mode.
const NAVIGATE_HOLD: Duration = Duration::from_millis(250);
/// Fly mode: seconds to reach the set speed after pressing a button and to stop after letting
/// go (time constant).
const FLY_RAMP: f64 = 0.45;
/// Flight recording: samples per second.
const RECORD_RATE: f64 = 30.0;

/// A flight being recorded: the view and the 3D cameras over time.
pub struct Recording {
    pub start: Instant,
    pub samples: Vec<FlightSample>,
}

pub struct Viewport {
    /// The selected layer is a 3D terrain (Ctrl-drag orbits its camera) or a 3D object
    /// (dragging orbits its camera).
    pub terrain: bool,
    pub object: bool,
    /// An animation frame shown instead of the document (timeline preview): the document,
    /// its palette phase, fast quality (playback).
    pub preview: Option<(Document, f64, bool)>,
    preview_phase: f64,
    last_phase: f64,
    gpu: Option<GpuRenderer>,
    texture: Option<(egui::TextureId, u64)>,
    /// What the renderer currently has: document, size, fast mode.
    /// Document, size, fast mode and formula library generation of the current plan.
    last: Option<(Document, (u32, u32), bool, u64)>,
    plan_error: Option<String>,
    render_size: (u32, u32),
    pass_start: Instant,
    converged_after: Option<Duration>,
    navigating_until: Option<Instant>,
    box_start: Option<Pos2>,
    rotate_last: Option<f32>,
    /// Plane coordinate under the cursor.
    pub hover: Option<[f64; 2]>,
    /// The pointer is over the view (the status bar shows the navigation hint).
    pub hovered: bool,
    /// Pause rendering (exports run).
    pub paused: bool,
    /// The next click picks the Julia seed.
    pub picking_julia: bool,
    /// The view before switching to Julia mode (restored when switching back).
    mandel_view: Option<View>,
    precision: Option<Precision>,
    /// Deep views: the formulas of layers that cannot be perturbed (and show rounding steps),
    /// with the reason.
    pub unperturbed: Vec<String>,
    /// Color cycling: the gradients turn in the view (the document keeps its offsets).
    pub cycling: bool,
    /// Gradient turns per second.
    pub cycle_speed: f64,
    phase: f64,
    last_tick: Instant,
    /// Fly mode: on, set speed (zoom doublings per second), current speed (positive: in),
    /// the clock and the point the flight heads for.
    pub fly: bool,
    pub fly_speed: f64,
    fly_rate: f64,
    fly_tick: Option<Instant>,
    fly_at: Option<Pos2>,
    /// The flight being recorded.
    pub recording: Option<Recording>,
    /// Gizmos of the light sources (drag to move); the light being dragged.
    pub show_lights: bool,
    light_drag: Option<usize>,
}

/// The largest rectangle with aspect `aspect` (width / height) centered in `avail`.
fn fit(avail: Rect, aspect: f32) -> Rect {
    let mut size = avail.size();
    if size.x / size.y.max(1.0) > aspect {
        size.x = size.y * aspect;
    } else {
        size.y = size.x / aspect;
    }
    Rect::from_center_size(avail.center(), size)
}

/// Plane coordinate of screen position `p` inside the image rectangle `rect`.
fn plane_at(view: &View, rect: Rect, p: Pos2) -> [f64; 2] {
    let c = view.center_f64();
    let r = view.radius.to_f64();
    let o = offset_at(view, rect, p);
    [c[0] + o[0] * r, c[1] + o[1] * r]
}

/// Offset of screen position `p` from the view center in units of the radius (rotation
/// applied): exact at any depth when added to the center with `View::pan`.
fn offset_at(view: &View, rect: Rect, p: Pos2) -> [f64; 2] {
    let s = 2.0 / rect.height() as f64;
    let dx = (p.x - rect.center().x) as f64 * s;
    let dy = -((p.y - rect.center().y) as f64) * s;
    let (sin, cos) = view.rotation.to_radians().sin_cos();
    [dx * cos - dy * sin, dx * sin + dy * cos]
}

impl Viewport {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Viewport {
        let gpu = cc.wgpu_render_state.as_ref().map(|rs| {
            let ctx = GpuContext { device: rs.device.clone(), queue: rs.queue.clone(), adapter_name: rs.adapter.get_info().name };
            GpuRenderer::new(ctx)
        });
        Viewport {
            terrain: false,
            object: false,
            preview: None,
            preview_phase: 0.0,
            last_phase: 0.0,
            gpu,
            texture: None,
            last: None,
            plan_error: None,
            render_size: (0, 0),
            pass_start: Instant::now(),
            converged_after: None,
            navigating_until: None,
            box_start: None,
            rotate_last: None,
            hover: None,
            hovered: false,
            paused: false,
            picking_julia: false,
            mandel_view: None,
            precision: None,
            unperturbed: Vec::new(),
            cycling: false,
            cycle_speed: 0.1,
            phase: 0.0,
            last_tick: Instant::now(),
            fly: false,
            fly_speed: 1.0,
            fly_rate: 0.0,
            fly_tick: None,
            fly_at: None,
            recording: None,
            show_lights: true,
            light_drag: None,
        }
    }

    pub fn gpu_context(&self) -> Option<GpuContext> {
        self.gpu.as_ref().map(|g| g.context().clone())
    }

    pub fn adapter_name(&self) -> String {
        self.gpu.as_ref().map(|g| g.context().adapter_name.clone()).unwrap_or_else(|| "no GPU".into())
    }

    /// Status line facts: samples, size, arithmetic, time to finish.
    pub fn status(&self) -> String {
        let Some(g) = &self.gpu else { return "No GPU: the preview needs a graphics card (Vulkan, DirectX 12 or Metal)".into() };
        let mode = match g.mode() {
            Some(Mode::Plain) => "f32",
            Some(Mode::Dual) => "f32 + derivatives",
            Some(Mode::Df64) => "double-float",
            Some(Mode::Pert) => "perturbation",
            None => "",
        };
        let done = self.converged_after.map(|d| format!(" · done in {:.1} s", d.as_secs_f64())).unwrap_or_default();
        let compiling = if g.compiling() { " · compiling shader…" } else { "" };
        let orbit = if g.orbit_pending() { " · reference orbit…" } else { "" };
        format!("{} × {} · {} spp · {mode}{done}{compiling}{orbit}", self.render_size.0, self.render_size.1, g.samples())
    }

    /// The view is deeper than double precision reaches.
    pub fn beyond_precision(&self) -> bool {
        self.precision == Some(Precision::Beyond)
    }

    pub fn error(&self) -> Option<String> {
        self.plan_error.clone().or_else(|| self.gpu.as_ref().and_then(|g| g.error().map(str::to_string)))
    }

    fn navigating(&self) -> bool {
        self.cycling || self.fly_rate.abs() > 1e-3 || self.navigating_until.is_some_and(|t| Instant::now() < t)
    }

    /// Fly mode: the speed follows the buttons smoothly and the view zooms around the point
    /// the flight heads for. True when the view moved.
    fn fly_step(&mut self, ui: &Ui, resp: &egui::Response, doc: &mut Document, rect: Rect, pointer: Option<Pos2>, scroll: f32) -> bool {
        let now = Instant::now();
        let dt = self.fly_tick.map_or(0.0, |t| now.duration_since(t).as_secs_f64().min(0.1));
        self.fly_tick = Some(now);
        if resp.hovered() && scroll != 0.0 {
            self.fly_speed = (self.fly_speed * (scroll as f64 * 0.002).exp()).clamp(0.05, 20.0);
        }
        let (left, right) = ui.input(|i| (i.pointer.primary_down(), i.pointer.secondary_down()));
        let held = resp.is_pointer_button_down_on();
        let target = if held && left {
            self.fly_speed
        } else if held && right {
            -self.fly_speed
        } else {
            0.0
        };
        if held && let Some(p) = pointer.filter(|p| rect.contains(*p)) {
            self.fly_at = Some(p);
        }
        self.fly_rate += (target - self.fly_rate) * (1.0 - (-dt / FLY_RAMP).exp());
        if target == 0.0 && self.fly_rate.abs() < 1e-3 {
            self.fly_rate = 0.0;
            self.fly_tick = None;
            return false;
        }
        ui.ctx().request_repaint();
        let at = offset_at(&doc.view, rect, self.fly_at.unwrap_or(rect.center()));
        doc.view.zoom_at(at, (-self.fly_rate * dt).exp2());
        dt > 0.0
    }

    /// Switches the active layer to Julia mode with `seed` (remembering the view) or back.
    pub fn set_julia(&mut self, doc: &mut Document, sel: &Selection, seed: Option<[f64; 2]>) -> Option<String> {
        let view = doc.view.clone();
        let layer = target_mut(doc, sel)?;
        let has_julia =
            rfr_core::formula::library::library().get(&layer.formula).and_then(|e| e.program_with(&layer.params).ok()).is_some_and(|p| p.has_julia);
        if !has_julia {
            return Some("This formula has no Julia mode".into());
        }
        match seed {
            Some(s) => {
                if !layer.julia {
                    self.mandel_view = Some(view);
                }
                layer.julia = true;
                layer.seed = s;
                doc.view = View { center: ["0".into(), "0".into()], radius: 1.6.into(), rotation: 0.0 };
                Some(format!("Julia set of ({:.6}, {:.6}) (J or Alt-click again goes back)", s[0], s[1]))
            }
            None => {
                layer.julia = false;
                if let Some(v) = self.mandel_view.take() {
                    doc.view = v;
                }
                Some("Back to the Mandelbrot mode".into())
            }
        }
    }

    /// Shows an animation frame (no navigation): see [`Viewport::preview`].
    fn show_preview(&mut self, ui: &mut Ui, frame: &eframe::Frame, doc: &Document, phase: f64, fast: bool, prefs: &Prefs) {
        let avail = ui.available_rect_before_wrap();
        let aspect = doc.image.width.max(1) as f32 / doc.image.height.max(1) as f32;
        let rect = fit(avail.shrink(4.0), aspect);
        ui.allocate_rect(avail, egui::Sense::hover());
        let ppp = ui.ctx().pixels_per_point();
        let scale =
            prefs.preview_percent.clamp(10, 100) as f32 / 100.0 * if fast { prefs.navigate_percent.clamp(10, 100) as f32 / 100.0 } else { 1.0 };
        let window = ui.ctx().input(|i| i.viewport().inner_rect.map(|r| r.size() * i.viewport().native_pixels_per_point.unwrap_or(1.0)));
        let side = |v: f32, max: f32| ((v * ppp * scale).round().min((max * scale).ceil()) as u32).max(16);
        let Some(w) = window else {
            ui.ctx().request_repaint();
            return;
        };
        let size = (side(rect.width(), w.x), side(rect.height(), w.y));
        self.preview_phase = phase;
        let tex = self.update_gpu(frame, doc, size, fast, prefs);
        let painter = ui.painter_at(avail);
        painter.rect_filled(rect, 0.0, theme::CANVAS);
        if let Some(id) = tex {
            painter.image(id, rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
        }
        if let Some(e) = self.error() {
            painter.text(rect.center(), egui::Align2::CENTER_CENTER, e, egui::FontId::proportional(theme::fs(13.0)), theme::WARN);
        }
        self.hovered = false;
    }

    /// Light gizmos: a click on a light's circle selects it, a drag from it moves the light
    /// (exactly, at any depth). True while the pointer works on a light.
    fn light_gizmos(&mut self, ui: &Ui, resp: &egui::Response, doc: &mut Document, rect: Rect) -> bool {
        let ctx = ui.ctx();
        let hit = |doc: &Document, p: Pos2| {
            doc.lights
                .iter()
                .enumerate()
                .filter_map(|(i, l)| crate::lights::screen_of(doc, rect, l).map(|s| (i, s.distance(p))))
                .filter(|(_, d)| *d <= 11.0)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(i, _)| i)
        };
        let origin = ui.input(|i| i.pointer.press_origin());
        if resp.drag_started_by(egui::PointerButton::Primary)
            && let Some(i) = origin.and_then(|p| hit(doc, p))
        {
            self.light_drag = Some(i);
            crate::lights::select(ctx, i);
        }
        if let Some(i) = self.light_drag {
            if let Some(p) = resp.interact_pointer_pos()
                && i < doc.lights.len()
            {
                let (w, h) = (doc.image.width.max(1), doc.image.height.max(1));
                let x = ((p.x - rect.min.x) / rect.width()) as f64 * w as f64;
                let y = ((p.y - rect.min.y) / rect.height()) as f64 * h as f64;
                if let Some(pos) = rfr_core::flare::position_at(doc, &doc.lights[i], x, y, w, h) {
                    doc.lights[i].position = pos;
                }
            }
            if !resp.dragged() {
                self.light_drag = None;
            }
            return true;
        }
        if (resp.clicked() || resp.double_clicked())
            && let Some(i) = origin.and_then(|p| hit(doc, p))
        {
            crate::lights::select(ctx, i);
            return true;
        }
        false
    }

    pub fn show(&mut self, ui: &mut Ui, frame: &eframe::Frame, doc: &mut Document, sel: &Selection, prefs: &Prefs) -> Option<String> {
        if let Some((pdoc, phase, fast)) = self.preview.clone() {
            self.show_preview(ui, frame, &pdoc, phase, fast, prefs);
            return None;
        }
        self.preview_phase = 0.0;
        let mut message = None;
        let avail = ui.available_rect_before_wrap();
        let aspect = doc.image.width.max(1) as f32 / doc.image.height.max(1) as f32;
        let rect = fit(avail.shrink(4.0), aspect);
        let resp = ui.allocate_rect(avail, egui::Sense::click_and_drag());
        let (mods, scroll, pointer) = ui.input(|i| (i.modifiers, i.smooth_scroll_delta.y, i.pointer.hover_pos()));
        self.hover = pointer.filter(|p| rect.contains(*p)).map(|p| plane_at(&doc.view, rect, p));
        let mut touched = false;
        self.object = target(doc, sel).is_some_and(|l| l.kind == rfr_core::doc::LayerKind::Object);
        self.terrain = !self.object && target(doc, sel).is_some_and(|l| l.light.enabled && l.light.terrain);

        // Light gizmos: a press on a light's circle drags the light instead of the view.
        let gizmo = self.show_lights && self.light_gizmos(ui, &resp, doc, rect);

        // 3D terrain: Ctrl-drag orbits the camera, Ctrl-wheel changes its distance; 3D objects
        // also without Ctrl.
        let orbit = !gizmo && (self.object || (self.terrain && mods.command));
        if orbit && let Some(l) = target_mut(doc, sel) {
            let object = self.object;
            let (cam, (near, far), (low, high)) =
                if object { (&mut l.object.camera, (0.1, 100.0), (-89.0, 89.0)) } else { (&mut l.light.camera, (0.05, 10.0), (1.0, 89.0)) };
            // egui turns Ctrl-wheel into a zoom gesture.
            let zoom = ui.input(|i| i.zoom_delta());
            if resp.hovered() && (zoom != 1.0 || scroll != 0.0) {
                let f = (-scroll as f64 * 0.003).exp() / zoom as f64;
                cam.distance = (cam.distance * f).clamp(near, far);
                touched = true;
            }
            if resp.dragged() {
                let d = resp.drag_delta();
                if d != Vec2::ZERO {
                    cam.yaw = (cam.yaw - d.x as f64 * 0.3 + 180.0).rem_euclid(360.0) - 180.0;
                    cam.pitch = (cam.pitch + d.y as f64 * 0.2).clamp(low, high);
                    touched = true;
                }
            }
        }

        // Fly mode: the buttons fly in and out, the wheel sets the speed.
        let flying = self.fly && !orbit && !gizmo;
        if flying {
            touched |= self.fly_step(ui, &resp, doc, rect, pointer, scroll);
        } else {
            self.fly_rate = 0.0;
            self.fly_tick = None;
        }

        // Wheel: zoom around the cursor.
        if !orbit
            && !flying
            && resp.hovered()
            && scroll != 0.0
            && let Some(p) = pointer
        {
            let f = (-scroll as f64 * 0.003).exp();
            let at = offset_at(&doc.view, rect, p);
            doc.view.zoom_at(at, f);
            touched = true;
        }
        let boxing = !flying && (mods.shift || resp.dragged_by(egui::PointerButton::Secondary));
        if resp.drag_started() {
            self.box_start = if boxing && !gizmo { resp.interact_pointer_pos() } else { None };
            self.rotate_last = None;
        }
        if resp.dragged() && self.box_start.is_none() && !orbit && !flying && !gizmo {
            if mods.command {
                // Rotate around the image center.
                if let Some(p) = resp.interact_pointer_pos() {
                    let a = (p - rect.center()).angle();
                    if let Some(last) = self.rotate_last {
                        let mut d = (a - last).to_degrees() as f64;
                        if d > 180.0 {
                            d -= 360.0;
                        }
                        if d < -180.0 {
                            d += 360.0;
                        }
                        doc.view.rotation = (doc.view.rotation - d).rem_euclid(360.0);
                        touched = true;
                    }
                    self.rotate_last = Some(a);
                }
            } else if resp.dragged_by(egui::PointerButton::Primary) {
                let d = resp.drag_delta();
                if d != Vec2::ZERO {
                    let b = offset_at(&doc.view, rect, rect.center() + d);
                    doc.view.pan([-b[0], -b[1]]);
                    touched = true;
                }
            }
        }
        if resp.drag_stopped() {
            if let (Some(s), Some(e)) = (self.box_start.take(), resp.interact_pointer_pos()) {
                let b = Rect::from_two_pos(s, e);
                if b.width() > 4.0 && b.height() > 4.0 {
                    let c = offset_at(&doc.view, rect, b.center());
                    let h = (b.width() * rect.height() / rect.width()).max(b.height());
                    doc.view.pan(c);
                    doc.view.radius *= (h / rect.height()) as f64;
                    touched = true;
                }
            }
            self.rotate_last = None;
        }
        // Clicks: Julia pick (Alt or the pick mode), zoom in (double-click), out (right click).
        if let Some(p) = resp.interact_pointer_pos().filter(|p| rect.contains(*p) && !gizmo) {
            let at = plane_at(&doc.view, rect, p);
            if resp.clicked() && (mods.alt || self.picking_julia) {
                let picking = std::mem::replace(&mut self.picking_julia, false);
                let julia_on = target(doc, sel).is_some_and(|l| l.julia);
                // Alt-click in Julia mode goes back; the pick mode always picks a new seed.
                message = if julia_on && !picking { self.set_julia(doc, sel, None) } else { self.set_julia(doc, sel, Some(at)) };
                touched = true;
            } else if flying {
            } else if resp.double_clicked() {
                doc.view.pan(offset_at(&doc.view, rect, p));
                doc.view.radius *= 0.5;
                touched = true;
            } else if resp.secondary_clicked() {
                doc.view.radius *= 2.0;
                touched = true;
            }
        }
        if touched {
            self.navigating_until = Some(Instant::now() + NAVIGATE_HOLD);
        }
        if self.picking_julia || flying {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        }
        // Flight recording: the view and the cameras a few dozen times per second.
        if let Some(rec) = &mut self.recording {
            let t = rec.start.elapsed().as_secs_f64();
            if rec.samples.last().is_none_or(|s| t - s.t >= 1.0 / RECORD_RATE) {
                rec.samples.push(FlightSample::of(doc, t));
            }
            ui.ctx().request_repaint();
        }

        // Render.
        let fast = self.navigating();
        let ppp = ui.ctx().pixels_per_point();
        let scale =
            prefs.preview_percent.clamp(10, 100) as f32 / 100.0 * if fast { prefs.navigate_percent.clamp(10, 100) as f32 / 100.0 } else { 1.0 };
        // The preview is never larger than the window. Right after start (or a change of the
        // interface scale) egui may lay out for a huge default screen; the window's own size
        // bounds the render then, and nothing renders before it is known.
        let window = ui.ctx().input(|i| i.viewport().inner_rect.map(|r| r.size() * i.viewport().native_pixels_per_point.unwrap_or(1.0)));
        let side = |v: f32, max: f32| ((v * ppp * scale).round().min((max * scale).ceil()) as u32).max(16);
        let size = window.map_or((16, 16), |w| (side(rect.width(), w.x), side(rect.height(), w.y)));
        let window_known = window.is_some();
        let tex = if window_known {
            self.update_gpu(frame, doc, size, fast, prefs)
        } else {
            ui.ctx().request_repaint();
            None
        };

        let painter = ui.painter_at(avail);
        painter.rect_filled(rect, 0.0, theme::CANVAS);
        if let Some(id) = tex {
            painter.image(id, rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
        }
        if let Some(e) = self.error() {
            painter.text(rect.center(), egui::Align2::CENTER_CENTER, e, egui::FontId::proportional(theme::fs(13.0)), theme::WARN);
        }
        if let (Some(s), Some(p)) = (self.box_start, resp.interact_pointer_pos()) {
            painter.rect_stroke(Rect::from_two_pos(s, p), 0.0, Stroke::new(1.5, theme::ACCENT), egui::StrokeKind::Middle);
        }
        if self.show_lights {
            let sel = crate::lights::selected(ui.ctx());
            for (i, l) in doc.lights.iter().enumerate() {
                let Some(p) = crate::lights::screen_of(doc, rect, l).filter(|p| avail.expand(12.0).contains(*p)) else { continue };
                let c = crate::widgets::to_color32(l.color);
                let (ring, dim) = if l.enabled { (Color32::WHITE, 1.0) } else { (theme::TEXT_MUTED, 0.5) };
                painter.circle_stroke(p, 8.0, Stroke::new(3.0, Color32::from_black_alpha(140)));
                painter.circle_stroke(p, 8.0, Stroke::new(1.5, ring.gamma_multiply(dim)));
                painter.circle_filled(p, 3.5, c.gamma_multiply(dim));
                if i == sel {
                    painter.circle_stroke(p, 12.0, Stroke::new(1.5, theme::ACCENT));
                    painter.text(
                        p + Vec2::new(16.0, -10.0),
                        egui::Align2::LEFT_BOTTOM,
                        &l.name,
                        egui::FontId::proportional(theme::fs(11.5)),
                        theme::ACCENT,
                    );
                }
            }
        }
        let font = egui::FontId::proportional(theme::fs(13.0));
        if let Some(rec) = &self.recording {
            let at = rect.right_top() + Vec2::new(-12.0, 12.0);
            painter.circle_filled(at + Vec2::new(-6.0, 8.0), 6.0, Color32::from_rgb(230, 60, 50));
            painter.text(
                at + Vec2::new(-18.0, 8.0),
                egui::Align2::RIGHT_CENTER,
                format!("REC {:.1} s", rec.start.elapsed().as_secs_f64()),
                font.clone(),
                Color32::from_rgb(255, 120, 110),
            );
        }
        if flying {
            painter.text(
                rect.left_bottom() + Vec2::new(10.0, -10.0),
                egui::Align2::LEFT_BOTTOM,
                format!("Fly: {:.2} zoom steps per second (wheel) · now {:+.2}", self.fly_speed, self.fly_rate),
                font.clone(),
                theme::ACCENT,
            );
            if let Some(p) = self.fly_at.filter(|_| self.fly_rate.abs() > 1e-3) {
                painter.circle_stroke(p, 6.0, Stroke::new(1.5, theme::ACCENT));
            }
        }
        if self.picking_julia {
            painter.text(
                rect.left_top() + Vec2::new(10.0, 10.0),
                egui::Align2::LEFT_TOP,
                "Click a point: its Julia set (Esc cancels)",
                egui::FontId::proportional(theme::fs(13.0)),
                theme::ACCENT,
            );
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.picking_julia = false;
            }
        }
        self.hovered = resp.hovered();
        message
    }

    fn update_gpu(&mut self, frame: &eframe::Frame, doc: &Document, size: (u32, u32), fast: bool, prefs: &Prefs) -> Option<egui::TextureId> {
        let now = Instant::now();
        let dt = now.duration_since(self.last_tick).as_secs_f64().min(0.1);
        self.last_tick = now;
        if self.cycling {
            self.phase = (self.phase + dt * self.cycle_speed).rem_euclid(1.0);
        }
        let gpu = self.gpu.as_mut()?;
        let generation = rfr_core::formula::library::generation();
        let changed = self.cycling
            || self.preview_phase != self.last_phase
            || self.last.as_ref().is_none_or(|(d, s, f, g)| d != doc || *s != size || *f != fast || *g != generation);
        if changed {
            self.last_phase = self.preview_phase;
            match Plan::sized(doc, size.0, size.1) {
                Ok(mut plan) => {
                    if self.cycling || self.preview_phase != 0.0 {
                        plan.shift_colors(if self.cycling { self.phase } else { 0.0 } + self.preview_phase);
                    }
                    self.precision = Some(plan.precision());
                    self.unperturbed = if plan.precision() == Precision::Beyond {
                        plan.layers()
                            .iter()
                            .filter_map(|lp| {
                                if !lp.transforms.is_empty() {
                                    return Some(format!("{}: has transformations", lp.fractal.id));
                                }
                                rfr_core::formula::perturb::Perturbation::new(&lp.fractal.program, &lp.fractal.values, !lp.julia, lp.seed)
                                    .err()
                                    .map(|e| format!("{}: {e}", lp.fractal.id))
                            })
                            .collect()
                    } else {
                        Vec::new()
                    };
                    gpu.set_plan(&plan);
                    self.plan_error = None;
                }
                Err(e) => self.plan_error = Some(e),
            }
            self.last = Some((doc.clone(), size, fast, generation));
            self.render_size = size;
            self.pass_start = Instant::now();
            self.converged_after = None;
        }
        let max = if self.paused {
            0
        } else if fast {
            1
        } else {
            prefs.preview_samples.max(1)
        };
        gpu.work(if fast { Duration::from_millis(35) } else { Duration::from_millis(25) }, max);
        if !fast && gpu.samples() >= max && self.converged_after.is_none() && max > 0 {
            self.converged_after = Some(self.pass_start.elapsed());
        }
        gpu.set_post(&doc.post);
        gpu.set_lights(doc);
        gpu.display();
        let (view, generation) = (gpu.output_view()?.clone(), gpu.generation());
        let rs = frame.wgpu_render_state()?;
        let mut renderer = rs.renderer.write();
        match self.texture {
            Some((_, g)) if g == generation => {}
            Some((id, _)) => {
                renderer.update_egui_texture_from_wgpu_texture(&rs.device, &view, wgpu::FilterMode::Linear, id);
                self.texture = Some((id, generation));
            }
            None => {
                let id = renderer.register_native_texture(&rs.device, &view, wgpu::FilterMode::Linear);
                self.texture = Some((id, generation));
            }
        }
        self.texture.map(|t| t.0)
    }

    /// More frames are needed (refining, compiling or navigating).
    pub fn busy(&self, prefs: &Prefs) -> bool {
        self.gpu.as_ref().is_some_and(|g| !self.paused && (g.compiling() || g.samples() < prefs.preview_samples.max(1))) || self.navigating()
    }
}
