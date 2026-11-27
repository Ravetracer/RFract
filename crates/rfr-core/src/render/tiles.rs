// SPDX-License-Identifier: LGPL-3.0-or-later

//! Tiled renders for posters larger than the graphics card holds: every tile is a render of
//! the document with its view moved and narrowed to the tile, so its pixels are exactly the
//! pixels of the whole image there. This holds for layers that color every pixel on its own
//! (formulas, colorings, transformations, masks, groups); lit layers, layer effects, density
//! layers and 3D objects depend on the whole image and are not tiled.

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
                LayerKind::Object => Some("is a 3D object"),
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

/// The document whose `region` (x, y, width, height in pixels) rendered at the region's size
/// gives the pixels of the `w` × `h` image there.
pub fn region_doc(doc: &Document, w: u32, h: u32, region: [u32; 4]) -> Document {
    let mut d = doc.clone();
    let [x, y, rw, rh] = region.map(|v| v as f64);
    // Offset of the region's center from the image center, in radii (half the height).
    let s = 2.0 / h as f64;
    let (dx, dy) = ((x + rw * 0.5 - w as f64 * 0.5) * s, -(y + rh * 0.5 - h as f64 * 0.5) * s);
    let (sin, cos) = d.view.rotation.to_radians().sin_cos();
    d.view.pan([dx * cos - dy * sin, dx * sin + dy * cos]);
    d.view.radius *= rh / h as f64;
    d.image.width = region[2];
    d.image.height = region[3];
    d.animation = None;
    d
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
    fn region_pixels_are_the_image_pixels() {
        let mut doc = Document::for_formula("rfract:Mandelbrot");
        doc.view.rotation = 30.0;
        let (w, h) = (300u32, 170u32);
        let full = Plan::sized(&doc, w, h).unwrap();
        let region = [123u32, 41, 64, 37];
        let r = region_doc(&doc, w, h, region);
        let part = Plan::sized(&r, region[2], region[3]).unwrap();
        for (i, j) in [(0u32, 0u32), (63, 36), (10, 20)] {
            let a = full.frame.plane(region[0] as f64 + i as f64 + 0.5, region[1] as f64 + j as f64 + 0.5);
            let b = part.frame.plane(i as f64 + 0.5, j as f64 + 0.5);
            assert!((a - b).norm() < 1e-12, "{a:?} {b:?}");
        }
        assert_eq!(tiles(5000, 2100).len(), 3 * 2);
        assert_eq!(with_margin([0, 2048, 2048, 52], 10, 5000, 2100), [0, 2038, 2058, 62]);
    }

    #[test]
    fn only_plain_layers_tile() {
        let mut doc = Document::default();
        assert!(can_tile(&doc).is_ok());
        doc.layers[0].light.enabled = true;
        assert!(can_tile(&doc).unwrap_err().contains("is lit"));
    }
}
