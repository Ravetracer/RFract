// SPDX-License-Identifier: LGPL-3.0-or-later

//! Image export: renders the document at its image size and sample count (GPU, or the CPU
//! renderer on a worker thread without a GPU) and writes a PNG that carries the document.
//! Posters above about 8K UHD render in tiles on a worker thread (`rfr_gpu::poster`).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use egui::RichText;
use rfr_core::doc::Document;
use rfr_core::render::Plan;
use rfr_gpu::{GpuContext, GpuRenderer};

use crate::theme;

/// A finished image (8-bit RGBA and a note on the denoising), or why there is none.
type Finished = Result<(Vec<u8>, Option<String>), String>;

enum Engine {
    Gpu(Box<GpuRenderer>),
    /// CPU render on a worker thread: progress is unknown, the result arrives at the end.
    Cpu(Receiver<Finished>),
    /// The GPU is done; denoising and effects run on a worker thread.
    Finishing(Receiver<Finished>),
    /// A poster in tiles on a worker thread that writes the file itself: done share, cancel
    /// flag, the note when finished.
    Poster(Receiver<Result<Option<String>, String>>, Arc<AtomicU32>, Arc<AtomicBool>),
}

struct Job {
    engine: Engine,
    doc: Document,
    path: PathBuf,
    samples: u32,
    start: Instant,
    /// The GPU of the export (the denoiser's helper images render there too).
    gpu: Option<GpuContext>,
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
        let samples = doc.image.samples.max(1);
        let (w, h) = (doc.image.width.max(1), doc.image.height.max(1));
        if rfr_core::render::tiles::needs_tiles(w, h) {
            rfr_core::render::tiles::can_tile(doc)
                .map_err(|e| format!("{w} × {h} renders in tiles, but {e}; render it smaller or change the layer"))?;
            let (tx, rx) = channel();
            let progress = Arc::new(AtomicU32::new(0));
            let cancel = Arc::new(AtomicBool::new(false));
            let (d, out, pr, ca, g) = (doc.clone(), path.clone(), progress.clone(), cancel.clone(), gpu.clone());
            std::thread::spawn(move || {
                let r = rfr_gpu::poster::render_poster(&d, (w, h, samples), g, &out, &ca, |p| pr.store(p.to_bits(), Ordering::Relaxed));
                if r.is_err() {
                    let _ = std::fs::remove_file(&out);
                }
                let _ = tx.send(r);
            });
            self.job = Some(Job { engine: Engine::Poster(rx, progress, cancel), doc: doc.clone(), path, samples, start: Instant::now(), gpu });
            return Ok(());
        }
        let plan = Plan::new(doc)?;
        let engine = match gpu.clone() {
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
                    let (w, h, s) = (d.image.width, d.image.height, d.image.samples.max(1));
                    let r = rfr_core::render::cpu::render(&d, w, h, s).map(|r| {
                        let aux = if d.post.denoise { rfr_gpu::render_aux(&d, w, h, s, None) } else { None };
                        rfr_core::render::post::finish_export(r.linear(), w, h, &d, aux.as_ref())
                    });
                    let _ = tx.send(r);
                });
                Engine::Cpu(rx)
            }
        };
        self.job = Some(Job { engine, doc: doc.clone(), path, samples, start: Instant::now(), gpu });
        Ok(())
    }

    /// Advances the export (call every frame) and shows its progress window. Returns a message
    /// when it finished, failed or was cancelled.
    pub fn update(&mut self, ctx: &egui::Context) -> Option<String> {
        let job = self.job.as_mut()?;
        let mut result: Option<Finished> = None;
        let mut finishing = None;
        let progress = match &mut job.engine {
            Engine::Gpu(r) => {
                r.work(Duration::from_millis(30), job.samples);
                if let Some(e) = r.error() {
                    result = Some(Err(e.to_string()));
                } else if r.samples() >= job.samples {
                    match r.read_linear() {
                        Some(lin) => {
                            // Denoising can take seconds: off the interface thread.
                            let (tx, rx) = channel();
                            let (w, h, s, doc, gpu) = (job.doc.image.width, job.doc.image.height, job.samples, job.doc.clone(), job.gpu.clone());
                            std::thread::spawn(move || {
                                let aux = if doc.post.denoise { rfr_gpu::render_aux(&doc, w, h, s, gpu.as_ref()) } else { None };
                                let _ = tx.send(Ok(rfr_core::render::post::finish_export(lin, w, h, &doc, aux.as_ref())));
                            });
                            finishing = Some(rx);
                        }
                        None => result = Some(Err("could not read the image back from the GPU".to_string())),
                    }
                }
                r.samples() as f32 / job.samples as f32
            }
            Engine::Cpu(rx) | Engine::Finishing(rx) => {
                if let Ok(r) = rx.try_recv() {
                    result = Some(r);
                }
                f32::NAN
            }
            Engine::Poster(rx, progress, _) => {
                if let Ok(r) = rx.try_recv() {
                    let job = self.job.take()?;
                    return Some(match r {
                        Ok(note) => format!(
                            "Exported {} in tiles in {:.1} s{}",
                            job.path.display(),
                            job.start.elapsed().as_secs_f64(),
                            note.map(|n| format!(", {n}")).unwrap_or_default()
                        ),
                        Err(e) if e == "cancelled" => "Export cancelled (nothing written)".into(),
                        Err(e) => format!("Export failed: {e}"),
                    });
                }
                f32::from_bits(progress.load(Ordering::Relaxed))
            }
        };
        if let Some(rx) = finishing {
            job.engine = Engine::Finishing(rx);
        }
        let finishing_now = matches!(job.engine, Engine::Finishing(_));
        let mut cancel = false;
        egui::Window::new("Export").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
            let tiled = if let Engine::Poster(..) = job.engine {
                format!(" in {} tiles", rfr_gpu::poster::tile_count(job.doc.image.width, job.doc.image.height))
            } else {
                String::new()
            };
            ui.label(format!("Rendering {} × {}, {} samples per pixel{tiled}", job.doc.image.width, job.doc.image.height, job.samples));
            ui.label(RichText::new(job.path.display().to_string()).size(theme::fs(11.0)).color(theme::TEXT_MUTED));
            if finishing_now {
                let text = if job.doc.post.denoise { "denoising and finishing…" } else { "finishing…" };
                ui.add(egui::ProgressBar::new(1.0).animate(true).text(text));
            } else if progress.is_nan() {
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
            // A poster stops after its current tile and removes the partial file.
            if let Engine::Poster(_, _, c) = &job.engine {
                c.store(true, Ordering::Relaxed);
                return None;
            }
            self.job = None;
            return Some("Export cancelled".into());
        }
        let r = result?;
        let job = self.job.take()?;
        Some(
            match r.and_then(|(rgba, note)| {
                rfr_core::io::png::write_rgba8(&job.path, job.doc.image.width, job.doc.image.height, &rgba, Some(&job.doc)).map(|_| note)
            }) {
                Ok(note) => {
                    let note = note.map(|n| format!(", {n}")).unwrap_or_default();
                    format!("Exported {} in {:.1} s{note}", job.path.display(), job.start.elapsed().as_secs_f64())
                }
                Err(e) => format!("Export failed: {e}"),
            },
        )
    }
}
