// SPDX-License-Identifier: LGPL-3.0-or-later

//! Tiled poster renders (`rfr_core::render::tiles`): the image renders tile by tile, each tile
//! with a margin for the effects and the denoiser, and goes into the PNG a band of rows at a
//! time, so neither the graphics card nor the memory holds the whole image. The light that
//! spreads over the image (bloom, streaks, god rays, lens flares) comes from a smaller render
//! of the whole view.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use rfr_core::doc::Document;
use rfr_core::render::Plan;
use rfr_core::render::post::{self, Glow};
use rfr_core::render::tiles::{self, TILE};

use crate::GpuContext;
use crate::animation::FrameRenderer;

/// Longest side of the render the glow is taken from.
const GLOW_SIDE: u32 = 3840;
/// Extra margin around every tile for the denoiser (it looks at neighbouring pixels).
const DENOISE_MARGIN: u32 = 32;

/// Renders `doc` at `w` × `h` with `samples` samples (the size) in tiles into the PNG `path` (with the
/// document inside). `progress` gets the done share (0..1); `cancel` stops after a tile.
/// Returns notes (the denoiser).
pub fn render_poster(
    doc: &Document,
    (w, h, samples): (u32, u32, u32),
    gpu: Option<GpuContext>,
    path: &Path,
    cancel: &AtomicBool,
    mut progress: impl FnMut(f32),
) -> Result<Option<String>, String> {
    let post = &doc.post;
    let mut renderer = FrameRenderer::new(gpu);
    let all = tiles::tiles(w, h);
    let whole = Plan::sized(doc, w, h)?;
    // Density layers: a first sweep finds the largest counts of the whole image per pass.
    let density = whole.layers().iter().any(|l| l.density.is_some());
    let steps = all.len() * if density { 2 } else { 1 };
    let mut done = 0;
    let mut maxima: Vec<Vec<[u32; 3]>> = Vec::new();
    if density {
        for tile in &all {
            if cancel.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            let m = renderer.density_maxima(&whole.clone().with_window(*tile), samples)?;
            maxima = if maxima.is_empty() { m } else { tiles::merge_maxima(&maxima, &m) };
            done += 1;
            progress(done as f32 / steps as f32);
        }
    }
    let whole = whole.with_density_max(&maxima);
    // The glow from a smaller render of the whole view, the light sources at that size.
    let lights = rfr_core::flare::any(doc);
    let k = (GLOW_SIDE as f64 / w.max(h) as f64).min(1.0);
    let (gw, gh) = (((w as f64 * k).round() as u32).max(1), ((h as f64 * k).round() as u32).max(1));
    let glow = if Glow::none(post) {
        Glow::empty()
    } else {
        let lin = renderer.render_plan(&Plan::sized(doc, gw, gh)?, samples.min(16))?;
        Glow::new(&lin, gw, gh, post)
    };
    let glow = if lights { glow.with_lights(doc, gw, gh) } else { glow };
    let mut saved = doc.clone();
    (saved.image.width, saved.image.height, saved.image.samples) = (w, h, samples);
    let mut png = rfr_core::io::png::RowWriter::new(path, w, h, Some(&saved))?;
    let mut note = None;
    let mut band: Vec<u8> = Vec::new();
    let mut band_y = 0;
    for tile in &all {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        if tile[1] != band_y || band.is_empty() {
            band_y = tile[1];
            band = vec![0u8; (w * tile[3] * 4) as usize];
        }
        let m = post::margin(*tile, w, h, post) + if post.denoise { DENOISE_MARGIN } else { 0 };
        let region = tiles::with_margin(*tile, m, w, h);
        let mut lin = renderer.render_plan(&whole.clone().with_window(region), samples)?;
        if post.denoise {
            note = Some(match post::denoise(&mut lin, region[2], region[3], post.denoise_blend, None) {
                Ok(dev) => format!("denoised in tiles ({})", dev.label()),
                Err(e) => format!("not denoised: {e}"),
            });
        }
        let rgba: Vec<u8> = if post.is_neutral() && !lights {
            // As the whole image would be saved: no effects at all.
            let cut: Vec<[f32; 4]> = (0..tile[3])
                .flat_map(|j| {
                    let row = ((tile[1] - region[1] + j) * region[2] + tile[0] - region[0]) as usize;
                    lin[row..row + tile[2] as usize].to_vec()
                })
                .collect();
            rfr_core::render::cpu::rgba8(&cut)
        } else {
            let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            post::apply_region(&lin, region, *tile, w, h, &glow, post).iter().flat_map(|c| [q(c[0]), q(c[1]), q(c[2]), q(c[3])]).collect()
        };
        for j in 0..tile[3] as usize {
            let src = &rgba[j * tile[2] as usize * 4..(j + 1) * tile[2] as usize * 4];
            let at = (j * w as usize + tile[0] as usize) * 4;
            band[at..at + src.len()].copy_from_slice(src);
        }
        // The band is complete after its last tile.
        if tile[0] + tile[2] == w {
            png.write_rows(&band)?;
        }
        done += 1;
        progress(done as f32 / steps as f32);
    }
    png.finish()?;
    Ok(note)
}

/// The tiles a poster of `w` × `h` renders (for progress texts).
pub fn tile_count(w: u32, h: u32) -> usize {
    (w.div_ceil(TILE) * h.div_ceil(TILE)) as usize
}
