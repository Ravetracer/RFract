// SPDX-License-Identifier: LGPL-3.0-or-later

//! Images that formulas read through image parameters: loaded once and cached, sampled the
//! same way on the CPU (here) and on the GPU (`kernel.wgsl`, `k_image_*`).
//!
//! The whole image covers the square (-1, -1)…(1, 1) of the complex plane (the top row at
//! imaginary part 1); outside it the color is transparent. Pixels are packed sRGB-encoded
//! RGBA8 (`r | g << 8 | b << 16 | a << 24`, rows from the top).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

/// Larger images are scaled down to at most this many pixels.
pub const MAX_PIXELS: u64 = 4096 * 4096;

/// A decoded image.
#[derive(Debug)]
pub struct ImageData {
    /// Size of the stored pixels.
    pub width: u32,
    pub height: u32,
    /// Size of the file's image (`getWidth`, `getHeight`, `getPixel` coordinates).
    pub orig_width: u32,
    pub orig_height: u32,
    pub pixels: Vec<u32>,
}

type Cache = HashMap<PathBuf, (Option<SystemTime>, Arc<ImageData>)>;
static CACHE: Mutex<Option<Cache>> = Mutex::new(None);

/// The file an image parameter names: the path itself, else a file of that name in one of the
/// formula folders (parameter files from other computers keep their own folders).
pub fn resolve(path: &str) -> Option<PathBuf> {
    let path = path.trim();
    if path.is_empty() {
        return None;
    }
    let p = Path::new(path);
    if p.is_file() {
        return Some(p.to_path_buf());
    }
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    crate::formula::library::user_folders().into_iter().map(|f| f.join(name)).find(|f| f.is_file())
}

/// Loads (or takes from the cache) the image an image parameter names.
pub fn load(path: &str) -> Result<Arc<ImageData>, String> {
    let file = resolve(path).ok_or_else(|| format!("image '{path}' not found"))?;
    let modified = std::fs::metadata(&file).and_then(|m| m.modified()).ok();
    if let Ok(c) = CACHE.lock()
        && let Some((m, img)) = c.as_ref().and_then(|c| c.get(&file))
        && *m == modified
    {
        return Ok(img.clone());
    }
    let img = Arc::new(decode(&file)?);
    if let Ok(mut c) = CACHE.lock() {
        c.get_or_insert_with(HashMap::new).insert(file, (modified, img.clone()));
    }
    Ok(img)
}

/// The image of an image parameter, `None` if there is none or it cannot be read.
pub fn get(path: &str) -> Option<Arc<ImageData>> {
    load(path).ok()
}

fn decode(file: &Path) -> Result<ImageData, String> {
    let img = image::ImageReader::open(file)
        .and_then(|r| r.with_guessed_format())
        .map_err(|e| format!("{}: {e}", file.display()))?
        .decode()
        .map_err(|e| format!("{}: {e}", file.display()))?;
    let (ow, oh) = (img.width(), img.height());
    let img = if u64::from(ow) * u64::from(oh) > MAX_PIXELS {
        let f = (MAX_PIXELS as f64 / (f64::from(ow) * f64::from(oh))).sqrt();
        let (w, h) = (((f64::from(ow) * f).floor() as u32).max(1), ((f64::from(oh) * f).floor() as u32).max(1));
        img.resize_exact(w, h, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();
    let pixels = rgba.pixels().map(|p| u32::from_le_bytes(p.0)).collect();
    Ok(ImageData { width, height, orig_width: ow, orig_height: oh, pixels })
}

/// A packed pixel as a color (sRGB-encoded channels 0…1).
pub fn unpack(p: u32) -> [f64; 4] {
    p.to_le_bytes().map(|b| f64::from(b) / 255.0)
}

/// `getPixel(x, y)`: the pixel at file coordinates (from the top left), transparent outside.
pub fn pixel(img: Option<&ImageData>, x: i64, y: i64) -> [f64; 4] {
    let Some(img) = img.filter(|i| i.width > 0 && i.height > 0) else { return [0.0; 4] };
    if x < 0 || y < 0 || x >= i64::from(img.orig_width) || y >= i64::from(img.orig_height) {
        return [0.0; 4];
    }
    let xs = x * i64::from(img.width) / i64::from(img.orig_width);
    let ys = y * i64::from(img.height) / i64::from(img.orig_height);
    unpack(img.pixels[(ys * i64::from(img.width) + xs) as usize])
}

/// `getColor(z)`: the image at `z`, interpolated between pixels, transparent outside.
pub fn color(img: Option<&ImageData>, re: f64, im: f64) -> [f64; 4] {
    let Some(img) = img.filter(|i| i.width > 0 && i.height > 0) else { return [0.0; 4] };
    let (w, h) = (f64::from(img.width), f64::from(img.height));
    let u = (re + 1.0) * 0.5 * w;
    let v = (1.0 - im) * 0.5 * h;
    if !(u >= 0.0 && u < w && v >= 0.0 && v < h) {
        return [0.0; 4];
    }
    let (x, y) = (u - 0.5, v - 0.5);
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let at = |xi: f64, yi: f64| {
        let xi = xi.clamp(0.0, w - 1.0) as usize;
        let yi = yi.clamp(0.0, h - 1.0) as usize;
        unpack(img.pixels[yi * img.width as usize + xi])
    };
    let (a, b, c, d) = (at(x0, y0), at(x0 + 1.0, y0), at(x0, y0 + 1.0), at(x0 + 1.0, y0 + 1.0));
    std::array::from_fn(|k| {
        let top = a[k] + (b[k] - a[k]) * fx;
        let bottom = c[k] + (d[k] - c[k]) * fx;
        top + (bottom - top) * fy
    })
}

/// The images of a layer packed for the GPU: per image a header of four words (width,
/// height, offset of the pixels in words, original width << 16 | original height), then the
/// pixels. Missing images have width 0.
pub fn pack(images: &[Option<Arc<ImageData>>]) -> Vec<u32> {
    let mut out = vec![0u32; images.len().max(1) * 4];
    for (k, img) in images.iter().enumerate() {
        let Some(img) = img else { continue };
        let offset = out.len() as u32;
        out[k * 4] = img.width;
        out[k * 4 + 1] = img.height;
        out[k * 4 + 2] = offset;
        out[k * 4 + 3] = (img.orig_width.min(0xffff) << 16) | img.orig_height.min(0xffff);
        out.extend_from_slice(&img.pixels);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sampling() {
        // 2 × 1: red, blue.
        let img = ImageData { width: 2, height: 1, orig_width: 2, orig_height: 1, pixels: vec![0xff0000ff, 0xffff0000] };
        assert_eq!(color(Some(&img), -0.5, 0.0), [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(color(Some(&img), 0.5, 0.0), [0.0, 0.0, 1.0, 1.0]);
        let mid = color(Some(&img), 0.0, 0.0);
        assert!((mid[0] - 0.5).abs() < 1e-9 && (mid[2] - 0.5).abs() < 1e-9);
        assert_eq!(color(Some(&img), 1.5, 0.0), [0.0; 4]);
        assert_eq!(pixel(Some(&img), 1, 0), [0.0, 0.0, 1.0, 1.0]);
        assert_eq!(pixel(Some(&img), 2, 0), [0.0; 4]);
        assert_eq!(color(None, 0.0, 0.0), [0.0; 4]);
    }

    #[test]
    fn image_values_survive_documents() {
        use crate::formula::ParamValue;
        for v in [ParamValue::Image { path: "/x/y.png".into() }, ParamValue::Class { class: "Image".into() }, ParamValue::Func("sin".into())] {
            let s = serde_json::to_string(&v).unwrap();
            assert_eq!(serde_json::from_str::<ParamValue>(&s).unwrap(), v, "{s}");
        }
    }
}
