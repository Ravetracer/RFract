// SPDX-License-Identifier: LGPL-3.0-or-later

//! Image export: renders the document at its image size and sample count (GPU, or the CPU
//! renderer on a worker thread without a GPU) and writes a PNG that carries the document.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use egui::RichText;
use rfr_core::doc::Document;
use rfr_core::render::Plan;
use rfr_gpu::{GpuContext, GpuRenderer};

use crate::theme;

enum Engine {
    Gpu(Box<GpuRenderer>),
    /// CPU render on a worker thread: progress is unknown, the result arrives at the end.
    Cpu(Receiver<Result<Vec<u8>, String>>),
}

struct Job {
    engine: Engine,
    doc: Document,
    path: PathBuf,
    samples: u32,
    start: Instant,
}

#[derive(Default)]
pub struct Export {
    job: Option<Job>,
}

impl Export {
    pub fn busy(&self) -> bool {
        self.job.is_some()
    }

    /// Starts rendering `doc` into `path`.
    pub fn start(&mut self, doc: &Document, path: PathBuf, gpu: Option<GpuContext>) -> Result<(), String> {
        let plan = Plan::new(doc)?;
        let samples = doc.image.samples.max(1);
        let engine = match gpu {
            Some(ctx) => {
                let mut r = GpuRenderer::new(ctx);
                r.set_band_target(Duration::from_millis(30));
                r.set_plan(&plan);
                Engine::Gpu(Box::new(r))
            }
            None => {
                let (tx, rx) = channel();
                let d = doc.clone();
                std::thread::spawn(move || {
                    let r = rfr_core::render::cpu::render(&d, d.image.width, d.image.height, d.image.samples.max(1)).map(|r| r.rgba8());
                    let _ = tx.send(r);
                });
                Engine::Cpu(rx)
            }
        };
        self.job = Some(Job { engine, doc: doc.clone(), path, samples, start: Instant::now() });
        Ok(())
    }

    /// Advances the export (call every frame) and shows its progress window. Returns a message
    /// when it finished, failed or was cancelled.
    pub fn update(&mut self, ctx: &egui::Context) -> Option<String> {
        let job = self.job.as_mut()?;
        let mut result: Option<Result<Vec<u8>, String>> = None;
        let progress = match &mut job.engine {
            Engine::Gpu(r) => {
                r.work(Duration::from_millis(30), job.samples);
                if let Some(e) = r.error() {
                    result = Some(Err(e.to_string()));
                } else if r.samples() >= job.samples {
                    result = Some(r.read_rgba8().ok_or_else(|| "could not read the image back from the GPU".to_string()));
                }
                r.samples() as f32 / job.samples as f32
            }
            Engine::Cpu(rx) => {
                if let Ok(r) = rx.try_recv() {
                    result = Some(r);
                }
                f32::NAN
            }
        };
        let mut cancel = false;
        egui::Window::new("Export").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
            ui.label(format!("Rendering {} × {}, {} samples per pixel", job.doc.image.width, job.doc.image.height, job.samples));
            ui.label(RichText::new(job.path.display().to_string()).size(theme::fs(11.0)).color(theme::TEXT_MUTED));
            if progress.is_nan() {
                ui.add(egui::ProgressBar::new(0.0).animate(true).text("rendering on the CPU…"));
            } else {
                ui.add(egui::ProgressBar::new(progress).show_percentage());
            }
            ui.label(RichText::new(format!("{:.0} s", job.start.elapsed().as_secs_f64())).size(theme::fs(11.0)).color(theme::TEXT_MUTED));
            if ui.button("Cancel").on_hover_text("Stop the export; nothing is written").clicked() {
                cancel = true;
            }
        });
        ctx.request_repaint();
        if cancel {
            self.job = None;
            return Some("Export cancelled".into());
        }
        let r = result?;
        let job = self.job.take()?;
        Some(match r.and_then(|rgba| rfr_core::io::png::write_rgba8(&job.path, job.doc.image.width, job.doc.image.height, &rgba, Some(&job.doc))) {
            Ok(()) => format!("Exported {} in {:.1} s", job.path.display(), job.start.elapsed().as_secs_f64()),
            Err(e) => format!("Export failed: {e}"),
        })
    }
}
