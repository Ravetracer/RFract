// SPDX-License-Identifier: LGPL-3.0-or-later

//! The view: progressive GPU preview of the document (in the document's aspect ratio) and the
//! navigation. Drag pans, the wheel zooms around the cursor, Shift-drag or right-drag zooms into
//! a box, Ctrl-drag rotates, double-click zooms in, Alt-click picks a Julia seed. While the user
//! navigates, the preview renders at a lower resolution.

use std::time::{Duration, Instant};

use egui::{Color32, Pos2, Rect, Stroke, Ui, Vec2};
use rfr_core::doc::{Document, View};
use rfr_core::render::{Plan, Precision};
use rfr_gpu::codegen::Mode;
use rfr_gpu::{GpuContext, GpuRenderer};

use crate::prefs::Prefs;
use crate::theme;

/// How long after the last navigation input the preview stays in the fast mode.
const NAVIGATE_HOLD: Duration = Duration::from_millis(250);

pub struct Viewport {
    gpu: Option<GpuRenderer>,
    texture: Option<(egui::TextureId, u64)>,
    /// What the renderer currently has: document, size, fast mode.
    last: Option<(Document, (u32, u32), bool)>,
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
    let s = 2.0 * view.radius / rect.height() as f64;
    let dx = (p.x - rect.center().x) as f64 * s;
    let dy = -((p.y - rect.center().y) as f64) * s;
    let (sin, cos) = view.rotation.to_radians().sin_cos();
    [c[0] + dx * cos - dy * sin, c[1] + dx * sin + dy * cos]
}

impl Viewport {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Viewport {
        let gpu = cc.wgpu_render_state.as_ref().map(|rs| {
            let ctx = GpuContext { device: rs.device.clone(), queue: rs.queue.clone(), adapter_name: rs.adapter.get_info().name };
            GpuRenderer::new(ctx)
        });
        Viewport {
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
            None => "",
        };
        let done = self.converged_after.map(|d| format!(" · done in {:.1} s", d.as_secs_f64())).unwrap_or_default();
        let compiling = if g.compiling() { " · compiling shader…" } else { "" };
        format!("{} × {} · {} spp · {mode}{done}{compiling}", self.render_size.0, self.render_size.1, g.samples())
    }

    /// The view is deeper than double precision reaches.
    pub fn beyond_precision(&self) -> bool {
        self.precision == Some(Precision::Beyond)
    }

    pub fn error(&self) -> Option<String> {
        self.plan_error.clone().or_else(|| self.gpu.as_ref().and_then(|g| g.error().map(str::to_string)))
    }

    fn navigating(&self) -> bool {
        self.navigating_until.is_some_and(|t| Instant::now() < t)
    }

    /// Switches the active layer to Julia mode with `seed` (remembering the view) or back.
    pub fn set_julia(&mut self, doc: &mut Document, seed: Option<[f64; 2]>) -> Option<String> {
        let layer = doc.layers.iter_mut().find(|l| l.visible)?;
        let has_julia = rfr_core::formula::library::library().get(&layer.formula).and_then(|e| e.program.as_ref().ok()).is_some_and(|p| p.has_julia);
        if !has_julia {
            return Some("This formula has no Julia mode".into());
        }
        match seed {
            Some(s) => {
                if !layer.julia {
                    self.mandel_view = Some(doc.view.clone());
                }
                layer.julia = true;
                layer.seed = s;
                doc.view = View { center: ["0".into(), "0".into()], radius: 1.6, rotation: 0.0 };
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

    pub fn show(&mut self, ui: &mut Ui, frame: &eframe::Frame, doc: &mut Document, prefs: &Prefs) -> Option<String> {
        let mut message = None;
        let avail = ui.available_rect_before_wrap();
        let aspect = doc.image.width.max(1) as f32 / doc.image.height.max(1) as f32;
        let rect = fit(avail.shrink(4.0), aspect);
        let resp = ui.allocate_rect(avail, egui::Sense::click_and_drag());
        let (mods, scroll, pointer) = ui.input(|i| (i.modifiers, i.smooth_scroll_delta.y, i.pointer.hover_pos()));
        self.hover = pointer.filter(|p| rect.contains(*p)).map(|p| plane_at(&doc.view, rect, p));
        let mut touched = false;

        // Wheel: zoom around the cursor.
        if resp.hovered()
            && scroll != 0.0
            && let Some(p) = pointer
        {
            let f = (-scroll as f64 * 0.003).exp();
            let at = plane_at(&doc.view, rect, p);
            let c = doc.view.center_f64();
            doc.view.radius *= f;
            doc.view.set_center([at[0] + (c[0] - at[0]) * f, at[1] + (c[1] - at[1]) * f]);
            touched = true;
        }
        let boxing = mods.shift || resp.dragged_by(egui::PointerButton::Secondary);
        if resp.drag_started() {
            self.box_start = if boxing { resp.interact_pointer_pos() } else { None };
            self.rotate_last = None;
        }
        if resp.dragged() && self.box_start.is_none() {
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
                    let a = plane_at(&doc.view, rect, rect.center());
                    let b = plane_at(&doc.view, rect, rect.center() + d);
                    let c = doc.view.center_f64();
                    doc.view.set_center([c[0] - (b[0] - a[0]), c[1] - (b[1] - a[1])]);
                    touched = true;
                }
            }
        }
        if resp.drag_stopped() {
            if let (Some(s), Some(e)) = (self.box_start.take(), resp.interact_pointer_pos()) {
                let b = Rect::from_two_pos(s, e);
                if b.width() > 4.0 && b.height() > 4.0 {
                    let c = plane_at(&doc.view, rect, b.center());
                    let h = (b.width() * rect.height() / rect.width()).max(b.height());
                    doc.view.radius *= (h / rect.height()) as f64;
                    doc.view.set_center(c);
                    touched = true;
                }
            }
            self.rotate_last = None;
        }
        // Clicks: Julia pick (Alt or the pick mode), zoom in (double-click), out (right click).
        if let Some(p) = resp.interact_pointer_pos().filter(|p| rect.contains(*p)) {
            let at = plane_at(&doc.view, rect, p);
            if resp.clicked() && (mods.alt || self.picking_julia) {
                let picking = std::mem::replace(&mut self.picking_julia, false);
                let julia_on = doc.layers.iter().find(|l| l.visible).is_some_and(|l| l.julia);
                // Alt-click in Julia mode goes back; the pick mode always picks a new seed.
                message = if julia_on && !picking { self.set_julia(doc, None) } else { self.set_julia(doc, Some(at)) };
                touched = true;
            } else if resp.double_clicked() {
                doc.view.radius *= 0.5;
                doc.view.set_center(at);
                touched = true;
            } else if resp.secondary_clicked() {
                doc.view.radius *= 2.0;
                touched = true;
            }
        }
        if touched {
            self.navigating_until = Some(Instant::now() + NAVIGATE_HOLD);
        }
        if self.picking_julia {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        }

        // Render.
        let fast = self.navigating();
        let ppp = ui.ctx().pixels_per_point();
        let scale =
            prefs.preview_percent.clamp(10, 100) as f32 / 100.0 * if fast { prefs.navigate_percent.clamp(10, 100) as f32 / 100.0 } else { 1.0 };
        let size = (((rect.width() * ppp * scale).round() as u32).max(16), ((rect.height() * ppp * scale).round() as u32).max(16));
        let tex = self.update_gpu(frame, doc, size, fast, prefs);

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
        let gpu = self.gpu.as_mut()?;
        let changed = self.last.as_ref().is_none_or(|(d, s, f)| d != doc || *s != size || *f != fast);
        if changed {
            match Plan::sized(doc, size.0, size.1) {
                Ok(plan) => {
                    self.precision = Some(plan.precision());
                    gpu.set_plan(&plan);
                    self.plan_error = None;
                }
                Err(e) => self.plan_error = Some(e),
            }
            self.last = Some((doc.clone(), size, fast));
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
