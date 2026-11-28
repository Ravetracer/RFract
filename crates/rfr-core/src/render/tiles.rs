// SPDX-License-Identifier: LGPL-3.0-or-later

//! Tiled renders for posters larger than the graphics card holds: every tile is a window of
//! the whole image (`Plan::with_window`), so its pixels are exactly the pixels of the whole
//! image there. This holds for layers that color every pixel on its own (formulas, colorings,
//! transformations, masks, groups) and for 3D objects (rays of the whole image's camera); lit
//! layers, layer effects and density layers depend on more than the window and are not tiled
//! yet.

use crate::doc::{Document, Layer, LayerKind};

/// Images with more pixels than this render in tiles (above about 8K UHD).
pub const TILED_PIXELS: u64 = 7680 * 4320;

/// Side of a tile in pixels.
pub const TILE: u32 = 2048;

/// Whether a `w` × `h` image renders in tiles.
pub fn needs_tiles(w: u32, h: u32) -> bool {
    w as u64 * h as u64 > TILED_PIXELS
}

/// Ok when the document can render in tiles, else why not (the layer that needs the whole
/// image).
pub fn can_tile(doc: &Document) -> Result<(), String> {
    fn check(layers: &[Layer]) -> Result<(), String> {
        for l in layers.iter().filter(|l| l.visible && l.opacity > 0.0) {
            let why = match l.kind {
                LayerKind::Density => Some("is a density layer"),
                LayerKind::Object => None,
                _ if l.light.enabled => Some("is lit"),
                _ if l.effects.active() => Some("has layer effects"),
                _ => None,
            };
            if let Some(why) = why {
                return Err(format!("layer \"{}\" {why}", l.name));
            }
            check(&l.children)?;
            if let Some(m) = l.mask.as_ref().filter(|m| m.enabled) {
                check(std::slice::from_ref(&m.layer))?;
            }
        }
        Ok(())
    }
    check(&doc.layers)
}

/// The tiles of a `w` × `h` image, row by row.
pub fn tiles(w: u32, h: u32) -> Vec<[u32; 4]> {
    let mut out = Vec::new();
    for y in (0..h).step_by(TILE as usize) {
        for x in (0..w).step_by(TILE as usize) {
            out.push([x, y, TILE.min(w - x), TILE.min(h - y)]);
        }
    }
    out
}

/// `tile` grown by `m` pixels on every side, clamped to the `w` × `h` image.
pub fn with_margin(tile: [u32; 4], m: u32, w: u32, h: u32) -> [u32; 4] {
    let (x0, y0) = (tile[0].saturating_sub(m), tile[1].saturating_sub(m));
    let (x1, y1) = ((tile[0] + tile[2] + m).min(w), (tile[1] + tile[3] + m).min(h));
    [x0, y0, x1 - x0, y1 - y0]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::Plan;

    #[test]
    fn tiles_and_margins() {
        assert_eq!(tiles(5000, 2100).len(), 3 * 2);
        assert_eq!(with_margin([0, 2048, 2048, 52], 10, 5000, 2100), [0, 2038, 2058, 62]);
    }

    #[test]
    fn windows_are_the_whole_image_there() {
        // Rotated, with a coloring formula and a transformation: the windows of a render put
        // together are the whole render, pixel for pixel.
        let mut doc = Document::for_formula("rfract:Mandelbrot");
        doc.view.rotation = 30.0;
        doc.layers[0].outside_method = crate::doc::OutsideMethod::Formula;
        doc.layers[0].outside.formula = "rfract-coloring:StripeAverage".into();
        doc.layers[0].transforms.push(crate::doc::TransformRef { formula: "rfract-transform:Twirl".into(), ..Default::default() });
        let (w, h) = (90u32, 50u32);
        let render = |plan: Plan| {
            let mut r = crate::render::cpu::CpuRenderer::new(plan);
            for _ in 0..3 {
                r.pass();
            }
            r.rgba8()
        };
        let whole = render(Plan::sized(&doc, w, h).unwrap());
        for win in [[0u32, 0, 40, 20], [37, 11, 53, 39], [89, 49, 1, 1]] {
            let part = render(Plan::sized(&doc, w, h).unwrap().with_window(win));
            for j in 0..win[3] {
                let a = ((win[1] + j) * w + win[0]) as usize * 4;
                let b = (j * win[2]) as usize * 4;
                let n = win[2] as usize * 4;
                assert_eq!(&whole[a..a + n], &part[b..b + n], "window {win:?}, row {j}");
            }
        }
    }

    #[test]
    fn only_plain_layers_tile() {
        let mut doc = Document::default();
        assert!(can_tile(&doc).is_ok());
        doc.layers[0].light.enabled = true;
        assert!(can_tile(&doc).unwrap_err().contains("is lit"));
    }
}
