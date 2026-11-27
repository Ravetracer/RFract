// SPDX-License-Identifier: LGPL-3.0-or-later

//! Rendering animation frames: one renderer for all frames (the shaders compile once), motion
//! blur from sub-frames, palette rotation, then the effects of the frame's document.

use rfr_core::animation::AnimationPlan;
use rfr_core::render::Plan;

use crate::renderer::{GpuContext, GpuRenderer};

/// Renders the frames of an animation on the GPU, or on the CPU without one.
pub struct FrameRenderer {
    gpu: Option<GpuRenderer>,
}

impl FrameRenderer {
    pub fn new(ctx: Option<GpuContext>) -> FrameRenderer {
        FrameRenderer {
            gpu: ctx.map(|c| {
                let mut r = GpuRenderer::new(c);
                r.set_band_target(std::time::Duration::from_millis(40));
                r
            }),
        }
    }

    /// The averaged image (linear, straight alpha) of `plan` with `samples` samples.
    fn render_plan(&mut self, plan: &Plan, samples: u32) -> Result<Vec<[f32; 4]>, String> {
        match &mut self.gpu {
            Some(r) => {
                r.set_plan(plan);
                r.wait_for_orbits();
                r.wait_for_pipeline()?;
                while r.samples() < samples {
                    r.work(std::time::Duration::from_millis(200), samples);
                    if let Some(e) = r.error() {
                        return Err(e.to_string());
                    }
                }
                r.read_linear().ok_or_else(|| "could not read the image back from the GPU".into())
            }
            None => {
                let mut r = rfr_core::render::cpu::CpuRenderer::new(plan.clone());
                r.target_samples = samples.max(1);
                for _ in 0..samples.max(1) {
                    r.pass();
                }
                Ok(r.linear())
            }
        }
    }

    /// Frame `frame` of `anim` at `width` × `height` with `samples` samples (spread over the
    /// shutter for motion blur): linear, straight alpha, before the effects; and the document
    /// of the frame (its effects finish the image).
    pub fn render(
        &mut self,
        anim: &AnimationPlan,
        frame: usize,
        width: u32,
        height: u32,
        samples: u32,
    ) -> Result<(Vec<[f32; 4]>, rfr_core::doc::Document), String> {
        let n = (width * height) as usize;
        let mut acc = vec![[0.0f32; 4]; n];
        let mut total = 0.0f32;
        for (t, s) in anim.exposure(frame, samples) {
            if s == 0 {
                continue;
            }
            let doc = anim.doc_at(t);
            let mut plan = Plan::sized(&doc, width, height)?;
            plan.shift_colors(anim.palette_phase(t));
            let lin = self.render_plan(&plan, s)?;
            let w = s as f32;
            for (a, c) in acc.iter_mut().zip(&lin) {
                a[0] += c[0] * c[3] * w;
                a[1] += c[1] * c[3] * w;
                a[2] += c[2] * c[3] * w;
                a[3] += c[3] * w;
            }
            total += w;
        }
        let k = 1.0 / total.max(1.0);
        let lin = acc.into_iter().map(|a| rfr_core::render::cpu::unpremultiply(a, k)).collect();
        Ok((lin, anim.doc_at(frame as f64)))
    }

    /// Frame `frame` finished: the effects of its document (denoised with its helper images
    /// when they ask for it). 8-bit RGBA and a note on the denoising.
    pub fn finished(
        &mut self,
        anim: &AnimationPlan,
        frame: usize,
        width: u32,
        height: u32,
        samples: u32,
    ) -> Result<(Vec<u8>, Option<String>), String> {
        let (lin, doc) = self.render(anim, frame, width, height, samples)?;
        let aux = if doc.post.denoise {
            match rfr_core::render::post::aux_documents(&doc) {
                Some([a, n]) => {
                    let s = samples.clamp(1, 8);
                    let mut pa = Plan::sized(&a, width, height)?;
                    pa.shift_colors(anim.palette_phase(frame as f64));
                    let pn = Plan::sized(&n, width, height)?;
                    Some((self.render_plan(&pa, s)?, self.render_plan(&pn, s)?))
                }
                None => None,
            }
        } else {
            None
        };
        Ok(rfr_core::render::post::finish_export(lin, width, height, &doc.post, aux.as_ref()))
    }
}
