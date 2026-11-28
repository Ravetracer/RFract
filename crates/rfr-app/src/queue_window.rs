// SPDX-License-Identifier: LGPL-3.0-or-later

//! Render queue window: images and animations rendered one after the other (batch rendering,
//! for example overnight). Jobs are copies of the current document (image or animation) or
//! document files; the queue is kept in the settings folder. Adapted from RMandelbulb3D.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Instant;

use egui::RichText;
use egui_phosphor::regular as ph;
use rfr_core::doc::Document;
use rfr_core::queue::{JobState, JobWhat, Queue, QueueJob, duration, new_id};
use rfr_core::video::VideoFormat;
use rfr_gpu::GpuContext;

use crate::prefs::Prefs;
use crate::{icons, theme};

struct Running {
    id: u64,
    progress: Arc<AtomicU32>,
    cancel: Arc<AtomicBool>,
    rx: mpsc::Receiver<Result<String, String>>,
    started: Instant,
}

pub struct QueueWindow {
    pub open: bool,
    queue: Queue,
    /// Working through the waiting jobs.
    active: bool,
    running: Option<Running>,
    message: Option<String>,
    /// The job whose settings are open.
    editing: Option<u64>,
}

fn queue_file() -> PathBuf {
    rfr_core::paths::config_dir().join("queue.json")
}

impl QueueWindow {
    pub fn new() -> Self {
        let queue = Queue::load(&queue_file()).unwrap_or_default();
        QueueWindow { open: false, queue, active: false, running: None, message: None, editing: None }
    }

    /// A job renders (the preview pauses).
    pub fn busy(&self) -> bool {
        self.running.is_some()
    }

    fn waiting(&self) -> usize {
        self.queue.jobs.iter().filter(|j| j.state == JobState::Waiting).count()
    }

    fn save(&mut self) {
        if let Err(e) = self.queue.save(&queue_file()) {
            self.message = Some(format!("Queue not saved: {e}"));
        }
    }

    /// Adds jobs (from a script) at the end of the queue.
    pub fn add_jobs(&mut self, jobs: Vec<QueueJob>) {
        self.queue.jobs.extend(jobs);
        self.save();
    }

    /// Updates the running job and starts the next one; returns a message when a job ended.
    pub fn poll(&mut self, gpu: Option<GpuContext>) -> Option<String> {
        let mut msg = None;
        if let Some(r) = &self.running
            && let Ok(res) = r.rx.try_recv()
        {
            let (id, secs) = (r.id, r.started.elapsed().as_secs_f64());
            let cancelled = r.cancel.load(Ordering::Relaxed);
            self.running = None;
            if let Some(j) = self.queue.jobs.iter_mut().find(|j| j.id == id) {
                j.state = match &res {
                    Ok(_) => JobState::Done { seconds: secs },
                    Err(_) if cancelled => JobState::Paused,
                    Err(e) => JobState::Failed { error: e.clone() },
                };
                msg = Some(match &res {
                    Ok(s) => format!("Queue: {} saved {s}", j.name),
                    Err(_) if cancelled => format!("Queue: {} stopped (paused)", j.name),
                    Err(e) => format!("Queue: {} failed: {e}", j.name),
                });
            }
            self.save();
        }
        if self.active && self.running.is_none() {
            match self.queue.next_waiting() {
                Some(i) => self.start(i, gpu),
                None => {
                    self.active = false;
                    msg = Some(match msg {
                        Some(m) => format!("{m} · queue finished"),
                        None => "Queue finished".into(),
                    });
                }
            }
        }
        msg
    }

    fn start(&mut self, i: usize, gpu: Option<GpuContext>) {
        let job = self.queue.jobs[i].clone();
        self.queue.jobs[i].state = JobState::Running;
        self.save();
        let progress = Arc::new(AtomicU32::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let ffmpeg = rfr_core::video::ffmpeg_program(None);
        let (pr, ca) = (progress.clone(), cancel.clone());
        std::thread::spawn(move || {
            let r = rfr_gpu::batch::run_job(&job, gpu, &ffmpeg, &ca, |p| pr.store(p.to_bits(), Ordering::Relaxed));
            let _ = tx.send(r);
        });
        self.running = Some(Running { id: self.queue.jobs[i].id, progress, cancel, rx, started: Instant::now() });
    }

    /// Adds a copy of `doc` as an image or animation job; asks for the output file.
    fn add_current(&mut self, doc: &Document, doc_path: Option<&Path>, animation: bool, prefs: &Prefs) {
        let stem = doc_path.and_then(|p| p.file_stem()).map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "render".into());
        let dlg = prefs.dialog();
        let out = if animation {
            dlg.add_filter("Movie", &["mp4", "mov", "webm", "gif"])
                .set_file_name(format!("{stem}.mp4"))
                .set_title("Queue the animation as (.mp4, .mov, .webm, .gif; a name without extension: a folder of PNG images)")
                .save_file()
        } else {
            dlg.add_filter("PNG image", &["png"]).set_file_name(format!("{stem}.png")).set_title("Queue the image as").save_file()
        };
        let Some(output) = out else { return };
        let what = if animation { JobWhat::Animation { format: VideoFormat::from_path(&output), from: 0, to: usize::MAX } } else { JobWhat::Image };
        let job = QueueJob {
            id: new_id(),
            name: stem,
            doc: Some(Box::new(doc.clone())),
            doc_dir: doc_path.and_then(|p| p.parent()).map(Path::to_path_buf),
            what,
            output,
            ..Default::default()
        };
        self.message = Some(format!("Added {} ({})", job.name, job.summary()));
        self.queue.jobs.push(job);
        self.save();
    }

    pub fn show(&mut self, ctx: &egui::Context, doc: &Document, doc_path: Option<&Path>, prefs: &Prefs) {
        if self.running.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
        if !self.open {
            return;
        }
        let mut open = self.open;
        let mut changed = false;
        egui::Window::new("Render queue").id(egui::Id::new("render-queue")).open(&mut open).resizable(true).collapsible(false).default_width(600.0).show(
            ctx,
            |ui| {
                ui.label(
                    RichText::new(
                        "Renders the waiting jobs one after the other. A job holds a copy of the document as it was when added (or a document file); the queue stays when RFract closes.",
                    )
                    .size(theme::fs(11.5))
                    .color(theme::TEXT_MUTED),
                );
                ui.horizontal_wrapped(|ui| {
                    let waiting = self.waiting();
                    if self.active {
                        if ui.button(format!("{}  Stop", ph::STOP)).on_hover_text("Stops the current job (it is paused) and the queue").clicked() {
                            self.active = false;
                            if let Some(r) = &self.running {
                                r.cancel.store(true, Ordering::Relaxed);
                            }
                        }
                    } else if ui
                        .add_enabled(waiting > 0, egui::Button::new(RichText::new(format!("{}  Start ({waiting} waiting)", ph::PLAY)).strong()).fill(theme::ACCENT_DIM))
                        .on_hover_text("Renders the waiting jobs from the top; the preview pauses meanwhile")
                        .clicked()
                    {
                        self.active = true;
                    }
                    if ui.button("Add this image…").on_hover_text("Queues the current document as a PNG image at its image size and samples").clicked() {
                        self.add_current(doc, doc_path, false, prefs);
                    }
                    let animated = doc.animation.as_ref().is_some_and(|a| !a.keyframes.is_empty());
                    if ui
                        .add_enabled(animated, egui::Button::new("Add this animation…"))
                        .on_hover_text("Queues the current document's animation as a movie or PNG frames (needs keyframes)")
                        .clicked()
                    {
                        self.add_current(doc, doc_path, true, prefs);
                    }
                    if ui
                        .button("Add document files…")
                        .on_hover_text("One job per file: its animation (MP4) when it has keyframes, else a PNG image, saved next to the file; read when the job starts")
                        .clicked()
                        && let Some(files) = prefs.dialog().add_filter("RFract documents", &["rfr", "png"]).pick_files()
                    {
                        for f in files {
                            match QueueJob::for_file(&f, None) {
                                Ok(j) => self.queue.jobs.push(j),
                                Err(e) => self.message = Some(e),
                            }
                        }
                        changed = true;
                    }
                    if ui.button("Remove finished").on_hover_text("Removes the jobs that are done").clicked() {
                        self.queue.jobs.retain(|j| !matches!(j.state, JobState::Done { .. }));
                        changed = true;
                    }
                });
                if let Some(m) = &self.message {
                    ui.label(RichText::new(m).size(theme::fs(11.5)).color(theme::TEXT_MUTED));
                }
                ui.separator();
                if self.queue.jobs.is_empty() {
                    ui.label(RichText::new("The queue is empty.").color(theme::TEXT_MUTED));
                }
                let running_id = self.running.as_ref().map(|r| r.id);
                let mut action: Option<(usize, &str)> = None;
                egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                    egui::Grid::new("queue-jobs").num_columns(3).spacing([10.0, 6.0]).striped(true).show(ui, |ui| {
                        let n = self.queue.jobs.len();
                        for (i, j) in self.queue.jobs.iter().enumerate() {
                            ui.vertical(|ui| {
                                ui.set_min_width(300.0);
                                ui.label(RichText::new(&j.name).strong());
                                ui.label(RichText::new(j.summary()).size(theme::fs(11.0)).color(theme::TEXT_MUTED))
                                    .on_hover_text(j.output.display().to_string());
                            });
                            if Some(j.id) == running_id
                                && let Some(r) = &self.running
                            {
                                let p = f32::from_bits(r.progress.load(Ordering::Relaxed));
                                let el = r.started.elapsed().as_secs_f64();
                                let eta = if p > 0.02 { format!(", about {} left", duration(el / p as f64 - el)) } else { String::new() };
                                ui.vertical(|ui| {
                                    ui.add(egui::ProgressBar::new(p).desired_width(160.0).show_percentage());
                                    ui.label(RichText::new(format!("{}{eta}", duration(el))).size(theme::fs(11.0)).color(theme::TEXT_MUTED));
                                });
                            } else {
                                let color = if matches!(j.state, JobState::Failed { .. }) { theme::WARN } else { theme::TEXT_MUTED };
                                ui.label(RichText::new(j.state.label()).size(theme::fs(11.5)).color(color));
                            }
                            ui.horizontal(|ui| {
                                let idle = Some(j.id) != running_id;
                                if ui.add_enabled(idle && i > 0, egui::Button::new(ph::CARET_UP).small()).on_hover_text("Earlier").clicked() {
                                    action = Some((i, "up"));
                                }
                                if ui.add_enabled(idle && i + 1 < n, egui::Button::new(ph::CARET_DOWN).small()).on_hover_text("Later").clicked() {
                                    action = Some((i, "down"));
                                }
                                let (label, tip) = if j.state == JobState::Waiting {
                                    ("Pause", "Skip this job until it is set to waiting again")
                                } else {
                                    ("Again", "Render this job again (waiting)")
                                };
                                if ui.add_enabled(idle, egui::Button::new(label).small()).on_hover_text(tip).clicked() {
                                    action = Some((i, "toggle"));
                                }
                                let open = self.editing == Some(j.id);
                                if ui
                                    .add_enabled(idle, egui::Button::new(ph::GEAR).small().selected(open))
                                    .on_hover_text("Settings of the job: size, samples, frames, output file")
                                    .clicked()
                                {
                                    action = Some((i, "edit"));
                                }
                                if ui.add_enabled(idle, egui::Button::new(icons::TRASH).small()).on_hover_text("Remove the job").clicked() {
                                    action = Some((i, "remove"));
                                }
                            });
                            ui.end_row();
                        }
                    });
                });
                if let Some((i, a)) = action {
                    match a {
                        "up" => self.queue.jobs.swap(i, i - 1),
                        "down" => self.queue.jobs.swap(i, i + 1),
                        "edit" => {
                            let id = self.queue.jobs[i].id;
                            self.editing = if self.editing == Some(id) { None } else { Some(id) };
                        }
                        "toggle" => {
                            let j = &mut self.queue.jobs[i];
                            j.state = if j.state == JobState::Waiting { JobState::Paused } else { JobState::Waiting };
                        }
                        _ => {
                            self.queue.jobs.remove(i);
                        }
                    }
                    changed = true;
                }
                if let Some(j) = self.editing.filter(|id| Some(*id) != running_id).and_then(|id| self.queue.jobs.iter_mut().find(|j| j.id == id)) {
                    ui.separator();
                    changed |= job_settings(ui, j, prefs);
                }
            },
        );
        if changed {
            self.save();
        }
        self.open = open;
    }
}

/// The settings of a job; true when something changed.
fn job_settings(ui: &mut egui::Ui, j: &mut QueueJob, prefs: &Prefs) -> bool {
    let before = j.clone();
    ui.label(RichText::new(format!("SETTINGS OF {}", j.name.to_uppercase())).size(theme::fs(11.0)).color(theme::TEXT_MUTED));
    crate::widgets::grid(ui, "queue-job", |ui| {
        crate::widgets::label(ui, "Name", "Name of the job in the list");
        ui.text_edit_singleline(&mut j.name);
        ui.end_row();
        crate::widgets::label(
            ui,
            "Size",
            "Width and height in pixels; 0 takes the document's (one side 0: from the other by the aspect). Above 8K UHD the image renders in tiles",
        );
        ui.horizontal(|ui| {
            ui.add(egui::DragValue::new(&mut j.width).range(0..=100_000).suffix(" px")).on_hover_text("Width (0: the document's)");
            ui.label("×");
            ui.add(egui::DragValue::new(&mut j.height).range(0..=100_000).suffix(" px")).on_hover_text("Height (0: the document's)");
        });
        ui.end_row();
        crate::widgets::label(ui, "Samples", "Samples per pixel; 0 takes the document's");
        ui.add(egui::DragValue::new(&mut j.samples).range(0..=65_536)).on_hover_text("Samples per pixel (0: the document's)");
        ui.end_row();
        if let JobWhat::Animation { from, to, .. } = &mut j.what {
            crate::widgets::label(ui, "Frames", "First and last frame to render");
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(from).range(0..=1_000_000)).on_hover_text("First frame");
                ui.label("to");
                let mut end = *to == usize::MAX;
                if ui.checkbox(&mut end, "the end").on_hover_text("Up to the last frame of the animation").changed() {
                    *to = if end { usize::MAX } else { (*from).max(100) };
                }
                if !end {
                    ui.add(egui::DragValue::new(to).range(*from..=1_000_000)).on_hover_text("Last frame");
                }
            });
            ui.end_row();
        }
        crate::widgets::label(ui, "Output", "The file (or folder of PNG frames) the job writes");
        ui.horizontal(|ui| {
            let mut text = j.output.display().to_string();
            if ui.add(egui::TextEdit::singleline(&mut text).desired_width(320.0)).on_hover_text("Output file").changed() {
                j.output = PathBuf::from(text);
            }
            if ui.button("…").on_hover_text("Choose the output file").clicked() {
                let mut dlg = prefs.dialog().set_title("Output of the job");
                if let Some(n) = j.output.file_name() {
                    dlg = dlg.set_file_name(n.to_string_lossy());
                }
                if let Some(d) = j.output.parent().filter(|d| d.is_dir()) {
                    dlg = dlg.set_directory(d);
                }
                if let Some(p) = dlg.save_file() {
                    if let JobWhat::Animation { format, .. } = &mut j.what {
                        *format = VideoFormat::from_path(&p);
                    }
                    j.output = p;
                }
            }
        });
        ui.end_row();
    });
    if let JobWhat::Animation { format, .. } = &mut j.what {
        *format = VideoFormat::from_path(&j.output);
    }
    if *j != before && j.state != JobState::Waiting {
        j.state = JobState::Waiting;
    }
    *j != before
}
