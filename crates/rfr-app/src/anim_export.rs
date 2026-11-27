// SPDX-License-Identifier: LGPL-3.0-or-later

//! Animation export: renders the frames of the document's animation on a worker thread (one
//! renderer for all frames) into a movie through ffmpeg or into PNG images, with progress and
//! cancelling.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::Instant;

use egui::RichText;
use rfr_core::animation::AnimationPlan;
use rfr_core::doc::Document;
use rfr_core::video::{VideoFormat, VideoSink};
use rfr_gpu::GpuContext;

use crate::theme;

enum Progress {
    Frame(usize),
    Done(Result<String, String>),
}

struct Job {
    rx: Receiver<Progress>,
    cancel: Arc<AtomicBool>,
    frames: usize,
    done: usize,
    path: PathBuf,
    start: Instant,
}

#[derive(Default)]
pub struct AnimExport {
    job: Option<Job>,
}

impl AnimExport {
    pub fn busy(&self) -> bool {
        self.job.is_some()
    }

    /// Starts rendering the animation of `doc` (its image size and samples) into `path`: a
    /// movie by the extension, else a folder of PNG images.
    /// `base`: the folder of the document file (relative sound paths).
    pub fn start(&mut self, doc: &Document, path: PathBuf, gpu: Option<GpuContext>, ffmpeg: PathBuf, base: Option<PathBuf>) -> Result<(), String> {
        let mut anim = doc.animation.as_deref().cloned().ok_or("the document has no animation")?;
        anim.ensure_audio_levels(base.as_deref(), &ffmpeg)?;
        let anim = &anim;
        let plan = AnimationPlan::new(anim)?;
        let audio = anim.audio_track(0, base.as_deref());
        let format = VideoFormat::from_path(&path);
        if format.needs_ffmpeg() && !rfr_core::video::ffmpeg_available(&ffmpeg) {
            return Err(format!(
                "{} needs ffmpeg, which was not found ({}); choose a name without extension for PNG images",
                format.label(),
                ffmpeg.display()
            ));
        }
        let (w, h, s) = (doc.image.width.max(1), doc.image.height.max(1), doc.image.samples.max(1));
        let frames = plan.frame_count();
        let zoom = anim.zoom_sequence.then(|| anim.clone());
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let stop = cancel.clone();
        let out = path.clone();
        std::thread::spawn(move || {
            let run = || -> Result<String, String> {
                let t = Instant::now();
                let mut seq = match &zoom {
                    Some(z) => Some(rfr_gpu::animation::ZoomSequence::new(gpu.clone(), rfr_core::animation::ZoomPath::new(z)?, w, h, s)),
                    None => None,
                };
                let mut renderer = rfr_gpu::animation::FrameRenderer::new(if seq.is_some() { None } else { gpu });
                let mut sink = VideoSink::new(&out, format, w, h, plan.fps(), &ffmpeg, audio.as_ref())?;
                let mut note = None;
                for f in 0..frames {
                    if stop.load(Ordering::Relaxed) {
                        let _ = sink.finish();
                        return Err("cancelled".into());
                    }
                    let (rgba, n) = match seq.as_mut() {
                        Some(z) => z.finished(f)?,
                        None => renderer.finished(&plan, f, w, h, s)?,
                    };
                    note = note.or(n);
                    sink.write_frame(f, &rgba)?;
                    let _ = tx.send(Progress::Frame(f + 1));
                }
                sink.finish()?;
                Ok(format!(
                    "Animation exported: {frames} frames in {:.1} s{} → {}",
                    t.elapsed().as_secs_f64(),
                    note.map(|n| format!(", {n}")).unwrap_or_default(),
                    out.display()
                ))
            };
            let r = run();
            let _ = tx.send(Progress::Done(r));
        });
        self.job = Some(Job { rx, cancel, frames, done: 0, path, start: Instant::now() });
        Ok(())
    }

    /// Advances the export (call every frame) and shows its progress window. Returns a message
    /// when it finished, failed or was cancelled.
    pub fn update(&mut self, ctx: &egui::Context) -> Option<String> {
        let job = self.job.as_mut()?;
        let mut result = None;
        while let Ok(p) = job.rx.try_recv() {
            match p {
                Progress::Frame(n) => job.done = n,
                Progress::Done(r) => result = Some(r),
            }
        }
        if let Some(r) = result {
            self.job = None;
            return Some(match r {
                Ok(m) => m,
                Err(e) if e == "cancelled" => "Animation export cancelled".into(),
                Err(e) => format!("Animation export failed: {e}"),
            });
        }
        let mut cancel = false;
        egui::Window::new("Export animation").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
            ui.label(format!("Rendering frame {} of {}", (job.done + 1).min(job.frames), job.frames));
            ui.label(RichText::new(job.path.display().to_string()).size(theme::fs(11.0)).color(theme::TEXT_MUTED));
            ui.add(egui::ProgressBar::new(job.done as f32 / job.frames.max(1) as f32).show_percentage());
            let el = job.start.elapsed().as_secs_f64();
            let left = if job.done > 0 { el / job.done as f64 * (job.frames - job.done) as f64 } else { 0.0 };
            ui.label(RichText::new(format!("{el:.0} s, about {left:.0} s left")).size(theme::fs(11.0)).color(theme::TEXT_MUTED));
            if ui.button("Cancel").on_hover_text("Stop after the current frame; the frames so far stay").clicked() {
                cancel = true;
            }
        });
        if cancel {
            job.cancel.store(true, Ordering::Relaxed);
        }
        ctx.request_repaint();
        None
    }
}
