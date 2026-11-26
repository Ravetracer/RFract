// SPDX-License-Identifier: LGPL-3.0-or-later

//! FractInt parameter files (`.par`): `name { key=value … }` entries with the fractal type,
//! its parameters and functions, the view (`corners=` or `center-mag=`), the iteration limit,
//! inside and outside coloring and the 256-color palette (`colors=`). Built-in escape-time
//! types use RFract's FractInt types (`formulas/rfract/fractint.ufm`) or its own formulas;
//! `type=formula` looks the formula up in the user's formula folders. Whatever is approximated
//! or missing is reported in the notes.

use std::collections::BTreeMap;
use std::path::Path;

use super::ImportedImage;
use crate::doc::{Coloring, Document, InsideMethod, Layer, OutsideMethod, Transfer};
use crate::formula::ProgramKind;
use crate::formula::ir::Func;
use crate::formula::library::{Library, library};
use crate::formula::{ParamKind, ParamValue};
use crate::gradient::{Blend, Gradient, Stop};

/// Reads every image of a `.par` file. A palette given as `colors=@file.map` is looked up next
/// to the parameter file.
pub fn read(path: &Path) -> Result<Vec<ImportedImage>, String> {
    let text = super::read_text_lossy(path)?;
    let images = parse(&text, path.parent());
    if images.is_empty() {
        return Err(format!("{}: no images in this parameter file", path.display()));
    }
    Ok(images)
}

/// The images of a parameter file's text (`dir`: where `colors=@file.map` palettes are).
pub fn parse(text: &str, dir: Option<&Path>) -> Vec<ImportedImage> {
    // A line ending in a backslash continues on the next one (long palettes).
    let mut joined = String::with_capacity(text.len());
    let mut lines = text.lines().peekable();
    while let Some(l) = lines.next() {
        let t = l.trim_end();
        if let Some(head) = t.strip_suffix('\\') {
            joined.push_str(head);
            if let Some(next) = lines.peek_mut() {
                *next = next.trim_start();
            }
        } else {
            joined.push_str(t);
            joined.push('\n');
        }
    }
    let lib = library();
    crate::formula::parse::split_entries(&joined)
        .iter()
        .filter_map(|e| {
            let items = items(e.body);
            items.iter().any(|(k, _)| k == "type").then(|| convert(&lib, &e.name, &items, dir))
        })
        .collect()
}

/// `key=value` items of an entry (comments after `;` dropped), keys lowercase.
fn items(body: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in body.lines() {
        let line = line.split(';').next().unwrap_or("");
        for tok in line.split_whitespace() {
            if let Some((k, v)) = tok.split_once('=') {
                out.push((k.to_ascii_lowercase(), v.to_string()));
            }
        }
    }
    out
}

fn get<'a>(items: &'a [(String, String)], key: &str) -> Option<&'a str> {
    items.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

fn nums(v: &str) -> Vec<f64> {
    v.split('/').map(|s| s.trim().parse().unwrap_or(0.0)).collect()
}

/// The 256 colors of a `colors=` value (6-bit channels, `<n>` for n blended colors).
pub fn decode_colors(v: &str) -> Option<Vec<[u8; 3]>> {
    let mut out: Vec<[u8; 3]> = Vec::new();
    let b = v.as_bytes();
    let mut i = 0;
    let mut pending: usize = 0;
    let digit = |c: u8| -> Option<u8> {
        match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'A'..=b'Z' => Some(c - b'A' + 10),
            b'_'..=b'z' => Some(c - b'_' + 36),
            _ => None,
        }
    };
    while i < b.len() {
        if b[i] == b'<' {
            let end = v[i..].find('>')? + i;
            pending = v[i + 1..end].trim().parse().ok()?;
            i = end + 1;
            continue;
        }
        if i + 3 > b.len() {
            return None;
        }
        let c = [digit(b[i])?, digit(b[i + 1])?, digit(b[i + 2])?];
        i += 3;
        if pending > 0 {
            let start = *out.last()?;
            let spread = pending + 1;
            for k in 1..spread {
                out.push(std::array::from_fn(|j| ((k * c[j] as usize + (spread - k) * start[j] as usize + spread / 2) / spread) as u8));
            }
            pending = 0;
        }
        out.push(c);
    }
    if out.is_empty() {
        return None;
    }
    out.truncate(256);
    while out.len() < 256 {
        out.push([40, 40, 40]);
    }
    Some(out.into_iter().map(|c| c.map(|x| ((x as u32 * 255 + 31) / 63) as u8)).collect())
}

/// A gradient with one stop per palette entry, sampled exactly at the entries by the
/// iteration coloring (one entry per iteration).
fn palette_gradient(colors: &[[u8; 3]]) -> Gradient {
    let n = colors.len().max(1) as f64;
    Gradient {
        stops: colors.iter().enumerate().map(|(i, c)| Stop::srgb(i as f64 / n, c[0], c[1], c[2])).collect(),
        blend: Blend::Linear,
        alpha: Vec::new(),
    }
}

/// The standard VGA palette, FractInt's palette when a parameter file names none: the 16 EGA
/// colors, 16 grays, nine 24-step hue circles (three intensities × three saturations) and
/// black.
pub fn vga_palette() -> Vec<[u8; 3]> {
    let six = |v: u32| ((v * 255 + 31) / 63) as u8;
    let mut out: Vec<[u8; 3]> = Vec::with_capacity(256);
    for i in 0..16u32 {
        let hi = if i & 8 != 0 { 21 } else { 0 };
        let mut c = [if i & 4 != 0 { 42 } else { 0 }, if i & 2 != 0 { 42 } else { 0 }, if i & 1 != 0 { 42 } else { 0 }];
        c = c.map(|x| x + hi);
        if i == 6 {
            c = [42, 21, 0];
        }
        out.push(c.map(six));
    }
    for g in [0, 5, 8, 11, 14, 17, 20, 24, 28, 32, 36, 40, 45, 50, 56, 63] {
        out.push([six(g); 3]);
    }
    let levels: [[[u32; 5]; 3]; 3] = [
        [[0, 16, 31, 47, 63], [31, 39, 47, 55, 63], [45, 49, 54, 58, 63]],
        [[0, 7, 14, 21, 28], [14, 17, 21, 24, 28], [20, 22, 24, 26, 28]],
        [[0, 4, 8, 12, 16], [8, 10, 12, 14, 16], [11, 12, 13, 15, 16]],
    ];
    // Blue, magenta, red, yellow, green, cyan, back to blue: (r, g, b) level indices.
    let mut circle: Vec<[usize; 3]> = Vec::with_capacity(24);
    for k in 0..4 {
        circle.push([k, 0, 4]);
    }
    for k in 0..4 {
        circle.push([4, 0, 4 - k]);
    }
    for k in 0..4 {
        circle.push([4, k, 0]);
    }
    for k in 0..4 {
        circle.push([4 - k, 4, 0]);
    }
    for k in 0..4 {
        circle.push([0, 4, k]);
    }
    for k in 0..4 {
        circle.push([0, 4 - k, 4]);
    }
    for intensity in &levels {
        for l in intensity {
            for c in &circle {
                out.push([six(l[c[0]]), six(l[c[1]]), six(l[c[2]])]);
            }
        }
    }
    out.resize(256, [0, 0, 0]);
    out
}

fn default_gradient() -> Gradient {
    palette_gradient(&vga_palette())
}

/// Library formula of a FractInt type: (id, Julia mode with seed p1).
fn type_formula(lib: &Library, t: &str) -> Option<(String, bool)> {
    let t = t.trim_start_matches('*').to_ascii_lowercase();
    // Floating-point variants compute the same set.
    let base = t.strip_suffix("fp").filter(|b| lib.get(&format!("fractint:{b}")).is_some() || ["mandel", "julia"].contains(b)).unwrap_or(&t);
    Some(match base {
        "mandel" => ("rfract:Mandelbrot".into(), false),
        "julia" => ("rfract:Mandelbrot".into(), true),
        "magnet1m" => ("rfract:Magnet1".into(), false),
        "magnet1j" => ("rfract:Magnet1".into(), true),
        "magnet2m" => ("rfract:Magnet2".into(), false),
        "magnet2j" => ("rfract:Magnet2".into(), true),
        "newton" | "newtbasin" => ("rfract:Newton".into(), false),
        other if lib.get(&format!("fractint:{other}")).is_some() => (format!("fractint:{other}"), false),
        _ => return None,
    })
}

/// A palette file: next to the parameter file, in a `maps` folder beside it or in the formula
/// folders (and their `maps` subfolders).
fn find_map(file: &str, dir: Option<&Path>) -> Option<std::path::PathBuf> {
    let mut dirs: Vec<std::path::PathBuf> = Vec::new();
    if let Some(d) = dir {
        dirs.push(d.to_path_buf());
        dirs.push(d.join("maps"));
        if let Some(up) = d.parent() {
            dirs.push(up.join("maps"));
        }
    }
    for f in crate::formula::library::user_folders() {
        dirs.push(f.join("maps"));
        dirs.push(f);
    }
    dirs.into_iter().map(|d| d.join(file)).find(|p| p.is_file())
}

fn convert(lib: &Library, name: &str, items: &[(String, String)], dir: Option<&Path>) -> ImportedImage {
    let mut notes = Vec::new();
    let mut doc = Document::default();
    let mut l = Layer { name: name.to_string(), ..Layer::default() };
    let t = get(items, "type").unwrap_or("mandel");
    let p = get(items, "params").map(nums).unwrap_or_default();
    let pc = |k: usize| [p.get(2 * k).copied().unwrap_or(0.0), p.get(2 * k + 1).copied().unwrap_or(0.0)];
    let functions: Vec<String> = get(items, "function").map(|f| f.split('/').map(|s| s.trim().to_ascii_lowercase()).collect()).unwrap_or_default();
    l.max_iter = get(items, "maxiter").and_then(|v| v.parse::<f64>().ok()).unwrap_or(150.0).clamp(1.0, 1e9) as u32;

    // The formula and its parameters.
    let resolved = if t.eq_ignore_ascii_case("formula") {
        let file = get(items, "formulafile").unwrap_or("");
        let entry = get(items, "formulaname").unwrap_or("");
        match lib.find_in_file(ProgramKind::Fractal, file, entry) {
            Some(e) if e.program().is_ok() => Some((e.id.clone(), false)),
            Some(e) => {
                notes.push(format!("{file}:{entry} does not compile ({})", e.program().err().map(|x| x.to_string()).unwrap_or_default()));
                None
            }
            None => {
                notes.push(format!("{file}:{entry} is not in your formula folders"));
                None
            }
        }
    } else {
        let r = type_formula(lib, t);
        if r.is_none() {
            notes.push(format!("The FractInt type '{t}' is not available."));
        }
        r
    };
    match resolved {
        Some((id, julia)) => {
            let program = lib.get(&id).and_then(|e| e.program().ok());
            let mut params: BTreeMap<String, ParamValue> = BTreeMap::new();
            if let Some(prog) = &program {
                let has = |n: &str| prog.params.iter().find(|q| q.name.eq_ignore_ascii_case(n)).map(|q| (q.name.clone(), q.kind));
                for k in 0..3 {
                    if let Some((n, ParamKind::Complex)) = has(&format!("p{}", k + 1)) {
                        params.insert(n, ParamValue::Complex(pc(k)));
                    }
                }
                for (k, f) in functions.iter().enumerate() {
                    if let Some((n, ParamKind::Func)) = has(&format!("fn{}", k + 1)) {
                        if Func::by_name(f).is_some() {
                            params.insert(n, ParamValue::Func(f.clone()));
                        } else {
                            notes.push(format!("The function '{f}' is not available; {n} keeps its default."));
                        }
                    }
                }
                if let (Some(b), Some((n, ParamKind::Float))) = (get(items, "bailout").and_then(|v| v.parse::<f64>().ok()), has("bailout")) {
                    params.insert(n, ParamValue::Float(b));
                }
                // RFract's own formulas: the perturbation is the start value, the degree a power.
                if id == "rfract:Mandelbrot" && !julia {
                    params.insert("start".into(), ParamValue::Complex(pc(0)));
                }
                if id == "rfract:Newton" {
                    params.insert("power".into(), ParamValue::Int(pc(0)[0].round().max(2.0) as i64));
                }
            }
            if julia {
                l.julia = true;
                l.seed = pc(0);
            }
            l.formula = id;
            l.params = params;
        }
        None => notes.push("The image shows the Mandelbrot set instead.".into()),
    }

    // The view.
    let mut aspect = 0.75;
    if let Some(c) = get(items, "corners").map(nums).filter(|c| c.len() >= 4) {
        // Top left (xmin, ymax), bottom right (xmax, ymin), bottom left (x3rd, y3rd).
        let bl = if c.len() >= 6 { [c[4], c[5]] } else { [c[0], c[2]] };
        let (tl, br) = ([c[0], c[3]], [c[1], c[2]]);
        let wv = [br[0] - bl[0], br[1] - bl[1]];
        let hv = [tl[0] - bl[0], tl[1] - bl[1]];
        let (w, h) = (wv[0].hypot(wv[1]), hv[0].hypot(hv[1]));
        doc.view.radius = (h / 2.0).max(1e-300).into();
        doc.view.set_center([(tl[0] + br[0]) / 2.0, (tl[1] + br[1]) / 2.0]);
        doc.view.rotation = wv[1].atan2(wv[0]).to_degrees();
        if w > 0.0 {
            aspect = h / w;
        }
        if w > 0.0 && h > 0.0 && ((wv[0] * hv[0] + wv[1] * hv[1]) / (w * h)).abs() > 1e-6 {
            notes.push("Skewed views are not supported; the view is rotated only.".into());
        }
    } else if let Some(c) = get(items, "center-mag").map(nums).filter(|c| c.len() >= 3) {
        doc.view.radius = (1.0 / c[2].max(1e-300)).into();
        doc.view.set_center([c[0], c[1]]);
        if let Some(&x) = c.get(3)
            && (x - 1.0).abs() > 1e-9
        {
            aspect = 0.75 * x;
        }
        doc.view.rotation = c.get(4).copied().unwrap_or(0.0);
        if c.get(5).is_some_and(|s| s.abs() > 1e-9) {
            notes.push("Skewed views are not supported.".into());
        }
    }
    doc.image.width = 1024;
    doc.image.height = ((1024.0 * aspect).round() as u32).clamp(16, 16384);

    // Colors: FractInt colors by the iteration count, one palette entry per iteration.
    let gradient = match get(items, "colors") {
        Some(v) if v.starts_with('@') => {
            let file = &v[1..];
            let file = if file.contains('.') { file.to_string() } else { format!("{file}.map") };
            let is_default = file.eq_ignore_ascii_case("default.map");
            match find_map(&file, dir).as_deref().map(crate::gradient_io::import) {
                Some(Ok(mut g)) if !g.is_empty() => g.remove(0).1,
                _ if is_default => default_gradient(),
                _ => {
                    notes.push(format!("The palette file {file} was not found (next to the parameter file, in ../maps or in the formula folders); the default palette stands in."));
                    default_gradient()
                }
            }
        }
        Some(v) => match decode_colors(v) {
            Some(c) => palette_gradient(&c),
            None => {
                notes.push("The palette (colors=) could not be read; a default palette stands in.".into());
                default_gradient()
            }
        },
        None => default_gradient(),
    };
    let base = Coloring { gradient: gradient.clone(), transfer: Transfer::Linear, density: 1.0 / 256.0, offset: 0.0, ..Coloring::default() };
    l.outside = base.clone();
    l.outside_method = OutsideMethod::Iterations;
    let outside = get(items, "outside").unwrap_or("iter").to_ascii_lowercase();
    if outside != "iter" && outside.parse::<i64>().is_err() {
        notes.push(format!("outside={outside} is shown as the iteration count."));
    } else if let Ok(k) = outside.parse::<i64>() {
        l.outside_method = OutsideMethod::Solid;
        let c = crate::gradient::lut_sample(&gradient.lut(), (k.rem_euclid(256)) as f64 / 256.0);
        l.outside.solid = [c[0], c[1], c[2]];
    }
    if get(items, "potential").is_some() {
        l.outside_method = OutsideMethod::Smooth;
        notes.push("Continuous potential is shown with smooth coloring.".into());
    }
    if let Some(d) = get(items, "decomp").and_then(|v| v.split('/').next()?.parse::<i64>().ok())
        && d > 0
    {
        l.outside_method = OutsideMethod::Formula;
        l.outside.formula = if d == 2 { "rfract-coloring:BinaryDecomposition".into() } else { "rfract-coloring:Decomposition".into() };
        l.outside.density = 1.0;
    }
    if get(items, "biomorph").is_some_and(|v| v != "-1") {
        notes.push("Biomorph coloring is not supported.".into());
    }
    // Lyapunov images carry their palette position in z.
    if l.formula == "fractint:lyapunov" {
        l.outside_method = OutsideMethod::Formula;
        l.outside.formula = "rfract-coloring:ZValue".into();
        l.outside.density = 1.0;
        let k = get(items, "inside").and_then(|v| v.parse::<f64>().ok()).filter(|k| *k > 0.0).unwrap_or(0.0);
        let c = crate::gradient::lut_sample(&gradient.lut(), k.rem_euclid(256.0) / 256.0);
        l.outside.solid = [c[0], c[1], c[2]];
    }
    let inside = get(items, "inside").unwrap_or("maxiter").to_ascii_lowercase();
    l.inside = Coloring { density: 1.0, ..base };
    l.inside_method = InsideMethod::Solid;
    let solid_at = |k: f64| {
        let c = crate::gradient::lut_sample(&gradient.lut(), k.rem_euclid(256.0) / 256.0);
        [c[0], c[1], c[2]]
    };
    match inside.as_str() {
        "maxiter" | "maxit" => l.inside.solid = solid_at(l.max_iter as f64),
        "bof60" => (l.inside_method, l.inside.formula) = (InsideMethod::Formula, "rfract-coloring:Bof60".into()),
        "bof61" => (l.inside_method, l.inside.formula) = (InsideMethod::Formula, "rfract-coloring:Bof61".into()),
        "zmag" => l.inside_method = InsideMethod::Magnitude,
        "atan" => l.inside_method = InsideMethod::Angle,
        "epscross" => (l.inside_method, l.inside.formula) = (InsideMethod::Formula, "rfract-coloring:EpsilonCross".into()),
        other => match other.parse::<f64>() {
            Ok(k) => l.inside.solid = solid_at(k),
            Err(_) => {
                l.inside.solid = solid_at(0.0);
                notes.push(format!("inside={other} is shown as one color."));
            }
        },
    }
    doc.layers = vec![l];
    ImportedImage { name: name.to_string(), doc, notes }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_palettes_with_blends() {
        let c = decode_colors("000<3>zzz").unwrap();
        assert_eq!(c.len(), 256);
        assert_eq!(c[0], [0, 0, 0]);
        assert_eq!(c[4], [255, 255, 255]);
        // Three blended entries between black and white.
        assert!(c[2][0] > 120 && c[2][0] < 135, "{:?}", c[2]);
        assert_eq!(c[5], [162, 162, 162]);
        assert!(decode_colors("00").is_none());
    }

    const SAMPLE: &str = "daisy { ; comment\n  reset type=lambda(fn||fn) function=recip/sqr\n  corners=-0.6/0.6/-0.45/0.45 params=1/0.9/1 maxiter=500\n  inside=0 colors=000zzz<3>z00\\\n  0z0\n  }\n\nclassic { reset type=julia params=-0.8/0.156 center-mag=0/0/0.5 maxiter=99 }\n";

    #[test]
    fn converts_types_views_and_palettes() {
        let images = parse(SAMPLE, None);
        assert_eq!(images.len(), 2);
        let d = &images[0].doc;
        let l = &d.layers[0];
        assert_eq!(l.formula, "fractint:lambda(fn||fn)");
        assert_eq!(l.max_iter, 500);
        assert_eq!(l.params.get("p1"), Some(&ParamValue::Complex([1.0, 0.9])));
        assert_eq!(l.params.get("p2"), Some(&ParamValue::Complex([1.0, 0.0])));
        assert_eq!(l.params.get("fn1"), Some(&ParamValue::Func("recip".into())));
        assert_eq!(l.params.get("fn2"), Some(&ParamValue::Func("sqr".into())));
        assert!((d.view.radius.to_f64() - 0.45).abs() < 1e-12);
        assert_eq!((d.image.width, d.image.height), (1024, 768));
        assert_eq!(l.outside_method, OutsideMethod::Iterations);
        // The continued palette line: black, white, 3 blends, red, green.
        assert_eq!(l.outside.gradient.stops.len(), 256);
        assert!(images[0].notes.is_empty(), "{:?}", images[0].notes);
        let j = &images[1].doc.layers[0];
        assert_eq!(j.formula, "rfract:Mandelbrot");
        assert!(j.julia);
        assert_eq!(j.seed, [-0.8, 0.156]);
        assert!((images[1].doc.view.radius.to_f64() - 2.0).abs() < 1e-12);
    }
}
