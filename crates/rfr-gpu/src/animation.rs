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
    pub(crate) fn render_plan(&mut self, plan: &Plan, samples: u32) -> Result<Vec<[f32; 4]>, String> {
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

impl FrameRenderer {
    /// A still image of `doc` finished with its effects (denoised with its helper images when
    /// they ask for it): 8-bit RGBA and a note on the denoising.
    pub fn still(&mut self, doc: &rfr_core::doc::Document, width: u32, height: u32, samples: u32) -> Result<(Vec<u8>, Option<String>), String> {
        let lin = self.render_plan(&Plan::sized(doc, width, height)?, samples)?;
        let aux = match rfr_core::render::post::aux_documents(doc).filter(|_| doc.post.denoise) {
            Some([a, n]) => {
                let s = samples.clamp(1, 8);
                Some((self.render_plan(&Plan::sized(&a, width, height)?, s)?, self.render_plan(&Plan::sized(&n, width, height)?, s)?))
            }
            None => None,
        };
        Ok(rfr_core::render::post::finish_export(lin, width, height, &doc.post, aux.as_ref()))
    }
}

/// One image of a zoom sequence: its radius (log2) and premultiplied linear light at twice the
/// frame size.
struct SeqImage {
    k: i64,
    log2_radius: f64,
    px: Vec<[f32; 4]>,
}

/// The deep-zoom sequence mode: frames composed from one image per doubling of the zoom
/// (`rfr_core::animation::ZoomPath`), each rendered once at twice the frame size, so a frame
/// only ever shrinks an image (by up to 2×). The detailed image fills the middle, the next
/// coarser one the rest; a narrow band at the seam blends them. Only the images the current
/// frames need are kept.
pub struct ZoomSequence {
    renderer: FrameRenderer,
    path: rfr_core::animation::ZoomPath,
    width: u32,
    height: u32,
    samples: u32,
    images: Vec<SeqImage>,
    /// Images rendered so far.
    pub rendered: usize,
}

impl ZoomSequence {
    pub fn new(ctx: Option<GpuContext>, path: rfr_core::animation::ZoomPath, width: u32, height: u32, samples: u32) -> ZoomSequence {
        ZoomSequence { renderer: FrameRenderer::new(ctx), path, width, height, samples, images: Vec::new(), rendered: 0 }
    }

    pub fn frame_count(&self) -> usize {
        self.path.log2_radius.len()
    }

    /// Renders image `k` (radius = end radius · 2^k) when it is missing.
    fn ensure(&mut self, k: i64) -> Result<(), String> {
        if self.images.iter().any(|i| i.k == k) {
            return Ok(());
        }
        let l = self.path.doc.view.radius.log2() + k as f64;
        let mut doc = self.path.doc.clone();
        let e = l.floor();
        doc.view.radius = rfr_core::math::fx::Fx::new(2f64.powf(l - e), e as i32);
        let plan = Plan::sized(&doc, self.width * 2, self.height * 2)?;
        // Four image pixels per frame pixel: a quarter of the samples each.
        let lin = self.renderer.render_plan(&plan, (self.samples / 4).max(1))?;
        let px = lin.into_iter().map(|c| [c[0] * c[3], c[1] * c[3], c[2] * c[3], c[3]]).collect();
        self.rendered += 1;
        // Keep the newest few (frames move steadily through the zoom).
        if self.images.len() >= 3 {
            self.images.remove(0);
        }
        self.images.push(SeqImage { k, log2_radius: l, px });
        Ok(())
    }

    /// The composed light (premultiplied) at log2 radius `l`.
    fn compose(&mut self, l: f64) -> Result<Vec<[f32; 4]>, String> {
        let end = self.path.doc.view.radius.log2();
        // Image k (radius end · 2^k ≤ r) holds the detail in the middle, k + 1 the rest.
        let k = (l - end).floor().max(0.0) as i64;
        self.ensure(k + 1)?;
        self.ensure(k)?;
        let fi = self.images.iter().find(|i| i.k == k).ok_or("zoom sequence image missing")?;
        let ci = self.images.iter().find(|i| i.k == k + 1).ok_or("zoom sequence image missing")?;
        let (w, h) = (self.width as usize, self.height as usize);
        let (iw, ih) = (w * 2, h * 2);
        let sample = |img: &SeqImage, ux: f64, uy: f64| -> Option<[f32; 4]> {
            // ux, uy: offset from the center in units of the image's radius (y down).
            let fx = ux * (ih as f64 * 0.5) + iw as f64 * 0.5 - 0.5;
            let fy = uy * (ih as f64 * 0.5) + ih as f64 * 0.5 - 0.5;
            if fx < -0.5 || fy < -0.5 || fx > iw as f64 - 0.5 || fy > ih as f64 - 0.5 {
                return None;
            }
            let (fx, fy) = (fx.clamp(0.0, iw as f64 - 1.0), fy.clamp(0.0, ih as f64 - 1.0));
            let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
            let (x1, y1) = ((x0 + 1).min(iw - 1), (y0 + 1).min(ih - 1));
            let (tx, ty) = ((fx - x0 as f64) as f32, (fy - y0 as f64) as f32);
            let p = |x: usize, y: usize| img.px[y * iw + x];
            let (a, b, c, d) = (p(x0, y0), p(x1, y0), p(x0, y1), p(x1, y1));
            Some(std::array::from_fn(|i| {
                let top = a[i] + (b[i] - a[i]) * tx;
                let bottom = c[i] + (d[i] - c[i]) * tx;
                top + (bottom - top) * ty
            }))
        };
        let (sf, sc) = (2f64.powf(l - fi.log2_radius), 2f64.powf(l - ci.log2_radius));
        let aspect = iw as f64 / ih as f64;
        let mut out = vec![[0.0f32; 4]; w * h];
        for (i, o) in out.iter_mut().enumerate() {
            let (x, y) = (i % w, i / w);
            // Offset in units of the frame's radius.
            let ux = (x as f64 + 0.5 - w as f64 * 0.5) / (h as f64 * 0.5);
            let uy = (y as f64 + 0.5 - h as f64 * 0.5) / (h as f64 * 0.5);
            let c = sample(ci, ux * sc, uy * sc).unwrap_or([0.0; 4]);
            *o = match sample(fi, ux * sf, uy * sf) {
                Some(f) => {
                    // Toward the coarse image over the outer 6 % of the fine one (no seam).
                    let edge = ((ux * sf).abs() / aspect).max((uy * sf).abs());
                    let t = ((edge - 0.94) / 0.06).clamp(0.0, 1.0) as f32;
                    std::array::from_fn(|k| f[k] + (c[k] - f[k]) * t)
                }
                None => c,
            };
        }
        Ok(out)
    }

    /// Frame `frame` finished with the last keyframe's effects (motion blur: compositions at
    /// several radii within the shutter, averaged).
    pub fn finished(&mut self, frame: usize) -> Result<(Vec<u8>, Option<String>), String> {
        let subs = if self.path.shutter > 0.0 { 8 } else { 1 };
        let mut acc: Option<Vec<[f32; 4]>> = None;
        for s in 0..subs {
            let t = if subs > 1 { frame as f64 + self.path.shutter * ((s as f64 + 0.5) / subs as f64 - 0.5) } else { frame as f64 };
            let c = self.compose(self.path.log2_radius_at(t))?;
            match acc.as_mut() {
                None => acc = Some(c),
                Some(a) => {
                    for (x, y) in a.iter_mut().zip(&c) {
                        for k in 0..4 {
                            x[k] += y[k];
                        }
                    }
                }
            }
        }
        let k = 1.0 / subs as f32;
        let lin: Vec<[f32; 4]> = acc.unwrap_or_default().into_iter().map(|a| rfr_core::render::cpu::unpremultiply(a, k)).collect();
        Ok(rfr_core::render::post::finish_export(lin, self.width, self.height, &self.path.doc.post, None))
    }
}
