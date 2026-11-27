// SPDX-License-Identifier: LGPL-3.0-or-later

//! Running render queue jobs (`rfr_core::queue`): a still image or the frames of an animation,
//! on the GPU (or the CPU without one), with progress and cancelling.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use rfr_core::animation::{AnimationPlan, ZoomPath};
use rfr_core::queue::{JobWhat, QueueJob};
use rfr_core::video::VideoSink;

use crate::GpuContext;
use crate::animation::{FrameRenderer, ZoomSequence};

/// Renders `job`; `progress` gets the done share (0..1). Returns what was written.
pub fn run_job(job: &QueueJob, gpu: Option<GpuContext>, ffmpeg: &Path, cancel: &AtomicBool, mut progress: impl FnMut(f32)) -> Result<String, String> {
    let doc = job.load_doc()?;
    let (w, h, s) = job.size(&doc);
    if let Some(d) = job.output.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    match job.what {
        JobWhat::Image => {
            let (rgba, note) = FrameRenderer::new(gpu).still(&doc, w, h, s)?;
            let mut saved = doc.clone();
            (saved.image.width, saved.image.height, saved.image.samples) = (w, h, s);
            rfr_core::io::png::write_rgba8(&job.output, w, h, &rgba, Some(&saved))?;
            progress(1.0);
            Ok(format!("{} ({w} × {h}{})", job.output.display(), note.map(|n| format!(", {n}")).unwrap_or_default()))
        }
        JobWhat::Animation { format, from, to } => {
            let mut anim = doc.animation.as_deref().cloned().ok_or("the document has no animation")?;
            let base = job.base_dir();
            anim.ensure_audio_levels(base.as_deref(), ffmpeg)?;
            let plan = AnimationPlan::new(&anim)?;
            let last = plan.frame_count().saturating_sub(1);
            let (first, last) = (from.min(last), to.min(last));
            if format.needs_ffmpeg() && !rfr_core::video::ffmpeg_available(ffmpeg) {
                return Err(format!("{} needs ffmpeg, which was not found ({})", format.label(), ffmpeg.display()));
            }
            let mut seq = if anim.zoom_sequence { Some(ZoomSequence::new(gpu.clone(), ZoomPath::new(&anim)?, w, h, s)) } else { None };
            let mut renderer = FrameRenderer::new(if seq.is_some() { None } else { gpu });
            let audio = anim.audio_track(first, base.as_deref());
            let mut sink = VideoSink::new(&job.output, format, w, h, plan.fps(), ffmpeg, audio.as_ref())?;
            let count = last + 1 - first;
            for (k, f) in (first..=last).enumerate() {
                if cancel.load(Ordering::Relaxed) {
                    let _ = sink.finish();
                    return Err("stopped".into());
                }
                let (rgba, _) = match seq.as_mut() {
                    Some(z) => z.finished(f)?,
                    None => renderer.finished(&plan, f, w, h, s)?,
                };
                sink.write_frame(k, &rgba)?;
                progress((k + 1) as f32 / count as f32);
            }
            sink.finish()?;
            Ok(format!("{} ({count} frames of {w} × {h})", job.output.display()))
        }
    }
}
