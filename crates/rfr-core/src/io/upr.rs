// SPDX-License-Identifier: LGPL-3.0-or-later

//! UFM parameter files (`.upr`): images described by sections (`fractal:`, then per layer
//! `layer:`, `mapping:`, `formula:`, `inside:`, `outside:`, `transform:`, `gradient:`,
//! `opacity:`). Each entry becomes a document. Formulas are looked up in the user's formula
//! folders by file name and entry; entries of the standard formula files, which come with
//! another program, map onto RFract's built-in formulas where one does the same. Whatever is
//! approximated or missing is reported in the notes.

use std::collections::BTreeMap;
use std::path::Path;

use crate::doc::{Coloring, Document, InsideMethod, Layer, MergeMode, OutsideMethod, Transfer, TransformRef};
use crate::formula::ProgramKind;
use crate::formula::ir::Func;
use crate::formula::library::{Library, library};
use crate::formula::{ParamKind, ParamValue};
use crate::gradient::Gradient;

pub use super::ImportedImage as UprImage;

/// Reads every image of a `.upr` file.
pub fn read(path: &Path) -> Result<Vec<UprImage>, String> {
    let text = super::read_text_lossy(path)?;
    let images = parse(&text);
    if images.is_empty() {
        return Err(format!("{}: no images in this parameter file", path.display()));
    }
    Ok(images)
}

/// The images of a parameter file's text.
pub fn parse(text: &str) -> Vec<UprImage> {
    let lib = library();
    crate::formula::parse::split_entries(text)
        .iter()
        .filter_map(|e| {
            let secs = sections(e.body);
            secs.iter().any(|s| s.name == "layer").then(|| convert(&lib, &e.name, &secs))
        })
        .collect()
}

#[derive(Clone, Debug, Default)]
struct Section {
    name: String,
    /// The section's text (lines joined).
    raw: String,
    /// `key=value` items in order, keys lowercase.
    items: Vec<(String, String)>,
}

impl Section {
    fn get(&self, key: &str) -> Option<&str> {
        self.items.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }
    fn num(&self, key: &str) -> Option<f64> {
        self.get(key).and_then(|v| v.split('/').next()?.trim().parse().ok())
    }
    fn yes(&self, key: &str) -> Option<bool> {
        self.get(key).map(|v| matches!(v.to_ascii_lowercase().as_str(), "yes" | "true" | "1"))
    }
}

fn sections(body: &str) -> Vec<Section> {
    let mut out: Vec<Section> = Vec::new();
    for line in body.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with(';') {
            continue;
        }
        if let Some(name) = t.strip_suffix(':')
            && !name.is_empty()
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            out.push(Section { name: name.to_ascii_lowercase(), ..Section::default() });
            continue;
        }
        if let Some(s) = out.last_mut() {
            s.raw.push(' ');
            s.raw.push_str(t);
        }
    }
    for s in &mut out {
        s.items = items(&s.raw);
    }
    out
}

/// `key=value` items; values may be quoted (with spaces).
fn items(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let b: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < b.len() {
        while i < b.len() && b[i].is_whitespace() {
            i += 1;
        }
        let ks = i;
        while i < b.len() && b[i] != '=' && !b[i].is_whitespace() {
            i += 1;
        }
        if i >= b.len() || b[i] != '=' {
            continue;
        }
        let key: String = b[ks..i].iter().collect::<String>().to_ascii_lowercase();
        i += 1;
        let value: String = if i < b.len() && b[i] == '"' {
            i += 1;
            let vs = i;
            while i < b.len() && b[i] != '"' {
                i += 1;
            }
            let v = b[vs..i].iter().collect();
            i += 1;
            v
        } else {
            let vs = i;
            while i < b.len() && !b[i].is_whitespace() {
                i += 1;
            }
            b[vs..i].iter().collect()
        };
        out.push((key, value));
    }
    out
}

/// The sections of one layer.
#[derive(Default)]
struct LayerSecs {
    layer: Section,
    mapping: Option<Section>,
    formula: Option<Section>,
    inside: Option<Section>,
    outside: Option<Section>,
    transforms: Vec<Section>,
    gradient: Option<Section>,
    opacity: Option<Section>,
}

fn convert(lib: &Library, name: &str, secs: &[Section]) -> UprImage {
    let mut notes = Vec::new();
    let mut doc = Document::default();
    let mut groups: Vec<LayerSecs> = Vec::new();
    for s in secs {
        match s.name.as_str() {
            "fractal" => {
                if let (Some(w), Some(h)) = (s.num("width"), s.num("height")) {
                    doc.image.width = (w as u32).clamp(16, 16384);
                    doc.image.height = (h as u32).clamp(16, 16384);
                }
            }
            "layer" => groups.push(LayerSecs { layer: s.clone(), ..LayerSecs::default() }),
            other => {
                let Some(g) = groups.last_mut() else { continue };
                match other {
                    "mapping" => g.mapping = Some(s.clone()),
                    "formula" => g.formula = Some(s.clone()),
                    "inside" => g.inside = Some(s.clone()),
                    "outside" => g.outside = Some(s.clone()),
                    "transform" => g.transforms.push(s.clone()),
                    "gradient" => g.gradient = Some(s.clone()),
                    "opacity" => g.opacity = Some(s.clone()),
                    _ => {}
                }
            }
        }
    }
    // The file lists the top layer first; documents keep the bottom layer first.
    let mut layers: Vec<Layer> = groups.iter().map(|g| layer(lib, g, &mut notes)).collect();
    layers.reverse();
    if let Some(bottom) = groups.last().and_then(|g| g.mapping.as_ref()) {
        let (cx, cy) = match bottom.get("center").map(|c| c.split_once('/').unwrap_or((c, "0"))) {
            Some((x, y)) => (x.trim().to_string(), y.trim().to_string()),
            None => ("0".into(), "0".into()),
        };
        let magn = bottom.num("magn").unwrap_or(1.0).max(1e-300);
        // Magnification 1 shows a 4 × 3 region, fitted into the image.
        let aspect = doc.image.height as f64 / doc.image.width.max(1) as f64;
        doc.view.radius = (1.5f64).max(2.0 * aspect) / magn;
        doc.view.center = [cx, cy];
        doc.view.rotation = bottom.num("angle").unwrap_or(0.0);
        if bottom.num("stretch").is_some_and(|s| (s - 1.0).abs() > 1e-9) || bottom.num("skew").is_some_and(|s| s.abs() > 1e-9) {
            notes.push("Stretched or skewed views are not supported; the view is neither.".into());
        }
        let differs =
            groups.iter().filter_map(|g| g.mapping.as_ref()).any(|m| m.get("center") != bottom.get("center") || m.get("magn") != bottom.get("magn"));
        if differs {
            notes.push("The layers have different views; all use the bottom layer's view.".into());
        }
    }
    // A transparent bottom layer leaves the background transparent.
    if groups.last().is_some_and(|g| g.layer.yes("transparent") == Some(true)) {
        doc.image.background[3] = 0.0;
    }
    if layers.is_empty() {
        layers.push(Layer::default());
    }
    doc.layers = layers;
    UprImage { name: name.to_string(), doc, notes }
}

fn merge_mode(m: &str, notes: &mut Vec<String>) -> MergeMode {
    match m.to_ascii_lowercase().as_str() {
        "normal" => MergeMode::Normal,
        "multiply" => MergeMode::Multiply,
        "screen" => MergeMode::Screen,
        "overlay" => MergeMode::Overlay,
        "softlight" => MergeMode::SoftLight,
        "hardlight" => MergeMode::HardLight,
        "colordodge" => MergeMode::ColorDodge,
        "colorburn" => MergeMode::ColorBurn,
        "darken" => MergeMode::Darken,
        "lighten" => MergeMode::Lighten,
        "difference" => MergeMode::Difference,
        "exclusion" => MergeMode::Exclusion,
        "addition" => MergeMode::Add,
        "subtraction" => MergeMode::Subtract,
        "hue" => MergeMode::Hue,
        "saturation" => MergeMode::Saturation,
        "color" => MergeMode::Color,
        "luminance" => MergeMode::Luminosity,
        other => {
            notes.push(format!("The merge mode '{other}' is not available; the layer uses Normal."));
            MergeMode::Normal
        }
    }
}

/// An ARGB integer (`solid=`) as linear RGB and alpha.
fn argb(v: &str) -> ([f32; 3], f32) {
    let c: u64 = v.trim().parse().unwrap_or(0xff00_0000);
    let ch = |s: u32| crate::gradient::srgb_to_linear(((c >> s) & 255) as u8);
    ([ch(16), ch(8), ch(0)], ((c >> 24) & 255) as f32 / 255.0)
}

/// A formula of the standard files in terms of a built-in one: id and Julia mode.
fn standard(kind: ProgramKind, entry: &str) -> Option<(&'static str, bool)> {
    Some(match (kind, entry.to_ascii_lowercase().as_str()) {
        (ProgramKind::Fractal, "mandelbrot" | "fastmandel") => ("rfract:Mandelbrot", false),
        (ProgramKind::Fractal, "julia" | "fastjulia") => ("rfract:Mandelbrot", true),
        (ProgramKind::Fractal, "pixel") => ("rfract:Pixel", false),
        (ProgramKind::Fractal, "newton") => ("rfract:Newton", false),
        (ProgramKind::Fractal, "nova") => ("rfract:Nova", false),
        (ProgramKind::Fractal, "phoenixjulia") => ("rfract:Phoenix", true),
        (ProgramKind::Coloring, "exponentialsmoothing") => ("rfract-coloring:ExponentialSmoothing", false),
        (ProgramKind::Coloring, "orbittraps") => ("rfract-coloring:OrbitTrap", false),
        (ProgramKind::Coloring, "decomposition") => ("rfract-coloring:Decomposition", false),
        (ProgramKind::Coloring, "gaussianinteger") => ("rfract-coloring:GaussianInteger", false),
        (ProgramKind::Coloring, "triangle") => ("rfract-coloring:TriangleInequality", false),
        (ProgramKind::Coloring, "default" | "basic" | "outside") => ("rfract-coloring:Iterations", false),
        (ProgramKind::Coloring, "decomp") => ("rfract-coloring:Decomposition", false),
        (ProgramKind::Transform, "inverse") => ("rfract-transform:Inversion", false),
        (ProgramKind::Transform, "kaleidoscope") => ("rfract-transform:Kaleidoscope", false),
        (ProgramKind::Transform, "mirror") => ("rfract-transform:Mirror", false),
        (ProgramKind::Transform, "twist") => ("rfract-transform:Twirl", false),
        _ => return None,
    })
}

/// The library id for `filename` / `entry` of `kind`: a user formula with that file name, or
/// the built-in equivalent of a standard formula. Also whether the built-in runs in Julia mode.
fn resolve(lib: &Library, kind: ProgramKind, sec: &Section, notes: &mut Vec<String>) -> Option<(String, bool)> {
    let file = sec.get("filename")?;
    let entry = sec.get("entry")?;
    let base = file.rsplit(['/', '\\']).next().unwrap_or(file);
    let is_standard = base.to_ascii_lowercase().starts_with("standard.");
    if let Some(e) = lib.find_in_file(kind, base, entry) {
        if let Err(err) = e.program() {
            notes.push(format!("{base}:{entry} does not compile ({err})"));
            return None;
        }
        return Some((e.id.clone(), false));
    }
    // The standard files and the FractInt compatibility files of the other program.
    let builtin_file = is_standard || base.to_ascii_lowercase().starts_with("fractint.");
    if builtin_file && let Some((id, julia)) = standard(kind, entry) {
        return Some((id.to_string(), julia));
    }
    notes.push(format!("{base}:{entry} is not in your formula folders{}", if builtin_file { " and has no built-in equivalent" } else { "" }));
    None
}

/// Parameter values (`p_name=…`, `f_name=…`) for the program of `id`.
fn params(id: &str, sec: &Section, notes: &mut Vec<String>) -> BTreeMap<String, ParamValue> {
    let mut out = BTreeMap::new();
    let lib = library();
    let Some(entry) = lib.get(id) else { return out };
    let program = match entry.program() {
        Ok(p) => p,
        Err(e) => {
            notes.push(format!("{} does not compile: {e}", entry.title));
            return out;
        }
    };
    for (k, v) in &sec.items {
        let Some(name) = k.strip_prefix("p_").or_else(|| k.strip_prefix("f_")) else { continue };
        let Some(p) = program.params.iter().find(|p| p.name.eq_ignore_ascii_case(name)) else { continue };
        if let Some(val) = param_value(p.kind, &p.items, v) {
            out.insert(p.name.clone(), val);
        }
    }
    out
}

fn param_value(kind: ParamKind, items: &[String], v: &str) -> Option<ParamValue> {
    let num = |s: &str| s.trim().parse::<f64>().ok();
    let parts: Vec<&str> = v.split('/').collect();
    Some(match kind {
        ParamKind::Float => ParamValue::Float(num(parts[0])?),
        ParamKind::Complex => ParamValue::Complex([num(parts[0])?, parts.get(1).and_then(|s| num(s)).unwrap_or(0.0)]),
        ParamKind::Int => match num(parts[0]) {
            Some(x) => ParamValue::Int(x.round() as i64),
            None => ParamValue::Int(items.iter().position(|i| i.trim().eq_ignore_ascii_case(v.trim()))? as i64),
        },
        ParamKind::Bool => ParamValue::Bool(matches!(v.to_ascii_lowercase().as_str(), "yes" | "true" | "1")),
        ParamKind::Func => {
            let f = v.trim().to_ascii_lowercase();
            Func::by_name(&f)?;
            ParamValue::Func(f)
        }
        ParamKind::Color => {
            if parts.len() >= 3 {
                let c: Vec<f64> = parts.iter().filter_map(|s| num(s)).collect();
                ParamValue::Color([c[0], c[1], c[2], c.get(3).copied().unwrap_or(1.0)])
            } else {
                let c: u64 = v.trim().parse().ok()?;
                let ch = |s: u32| ((c >> s) & 255) as f64 / 255.0;
                ParamValue::Color([ch(16), ch(8), ch(0), ch(24)])
            }
        }
    })
}

fn transfer(t: &str) -> Option<Transfer> {
    Some(match t.to_ascii_lowercase().as_str() {
        "linear" => Transfer::Linear,
        "sqrt" => Transfer::Sqrt,
        "cuberoot" => Transfer::Cbrt,
        "log" => Transfer::Log,
        "sqr" => Transfer::Square,
        _ => return None,
    })
}

/// How a region is colored after the conversion.
enum Method {
    Native(OutsideMethod),
    Formula,
    Solid,
}

fn coloring(lib: &Library, sec: Option<&Section>, gradient: &Gradient, inside: bool, notes: &mut Vec<String>) -> (Method, Coloring) {
    let mut c = Coloring { gradient: gradient.clone(), transfer: Transfer::Linear, density: 1.0, ..Coloring::default() };
    let Some(sec) = sec else { return (Method::Solid, c) };
    if let Some(s) = sec.get("solid") {
        let (rgb, a) = argb(s);
        c.solid = rgb;
        c.solid_alpha = a;
    }
    let t = sec.get("transfer").unwrap_or("linear");
    if t.eq_ignore_ascii_case("none") {
        return (Method::Solid, c);
    }
    c.transfer = transfer(t).unwrap_or(Transfer::Linear);
    c.density = sec.num("density").unwrap_or(1.0);
    c.offset = (sec.num("offset").unwrap_or(0.0) / 400.0).rem_euclid(1.0);
    let entry = sec.get("entry").unwrap_or("").to_ascii_lowercase();
    let standard_file = sec.get("filename").is_some_and(|f| {
        let f = f.to_ascii_lowercase();
        f.contains("standard.") || f.contains("fractint.")
    });
    if standard_file && !inside {
        // The standard gradient colorings that match a built-in method.
        match entry.as_str() {
            "smooth" => {
                c.density *= 0.05;
                return (Method::Native(OutsideMethod::Smooth), c);
            }
            "distanceestimator" => return (Method::Native(OutsideMethod::Distance), c),
            "potential" => return (Method::Native(OutsideMethod::Smooth), c),
            _ => {}
        }
    }
    // FractInt's inside coloring: one color (the gradient's color at the iteration limit there).
    if standard_file && inside && entry == "inside" {
        return (Method::Solid, c);
    }
    match resolve(lib, ProgramKind::Coloring, sec, notes) {
        Some((id, _)) => {
            c.params = params(&id, sec, notes);
            c.formula = id;
            (Method::Formula, c)
        }
        None => (if inside { Method::Solid } else { Method::Native(OutsideMethod::Smooth) }, c),
    }
}

fn layer(lib: &Library, g: &LayerSecs, notes: &mut Vec<String>) -> Layer {
    let mut l = Layer { name: g.layer.get("caption").unwrap_or("Layer").to_string(), ..Layer::default() };
    l.opacity = (g.layer.num("opacity").unwrap_or(100.0) / 100.0).clamp(0.0, 1.0);
    l.visible = g.layer.yes("visible") != Some(false);
    if let Some(m) = g.layer.get("mergemode") {
        l.merge = merge_mode(m, notes);
    }
    if let Some(f) = &g.formula {
        l.max_iter = f.num("maxiter").unwrap_or(250.0).clamp(1.0, 1e9) as u32;
        match resolve(lib, ProgramKind::Fractal, f, notes) {
            Some((id, julia)) => {
                l.params = params(&id, f, notes);
                if julia {
                    l.julia = true;
                    if let Some(ParamValue::Complex(s)) = f.get("p_seed").and_then(|v| param_value(ParamKind::Complex, &[], v)) {
                        l.seed = s;
                    }
                }
                l.formula = id;
            }
            None => notes.push(format!("Layer '{}' shows the Mandelbrot set instead.", l.name)),
        }
    }
    let gradient =
        g.gradient.as_ref().and_then(|s| crate::gradient_io::ugr_gradient(&s.raw, g.opacity.as_ref().map(|o| o.raw.as_str()))).unwrap_or_default();
    let (om, oc) = coloring(lib, g.outside.as_ref(), &gradient, false, notes);
    l.outside_method = match om {
        Method::Native(m) => m,
        Method::Formula => OutsideMethod::Formula,
        Method::Solid => OutsideMethod::Solid,
    };
    l.outside = oc;
    let (im, ic) = coloring(lib, g.inside.as_ref(), &gradient, true, notes);
    l.inside_method = match im {
        Method::Formula => InsideMethod::Formula,
        _ => InsideMethod::Solid,
    };
    l.inside = ic;
    for t in &g.transforms {
        if t.yes("enabled") == Some(false) {
            continue;
        }
        if t.get("entry").is_some_and(|e| e.eq_ignore_ascii_case("viewwindow")) {
            notes.push("The FractInt view window transformation is ignored.".into());
            continue;
        }
        if let Some((id, _)) = resolve(lib, ProgramKind::Transform, t, notes) {
            let params = params(&id, t, notes);
            l.transforms.push(TransformRef { formula: id, params, ..TransformRef::default() });
        }
    }
    l
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
Two_Layers {
fractal:
  title="Two layers" width=800 height=600 layers=2
layer:
  caption="Top" opacity=50 mergemode=multiply
mapping:
  center=-0.75/0.1 magn=4 angle=30
formula:
  maxiter=300 filename="Standard.ufm" entry="Julia" p_seed=-0.8/0.156
  p_power=2/0 p_bailout=128
inside:
  transfer=none solid=4278255360
outside:
  transfer=linear filename="Standard.ucl" entry="Smooth" density=2
gradient:
  smooth=yes rotation=0 index=0 color=255 index=200 color=16711680
opacity:
  smooth=no index=0 opacity=255
layer:
  caption="Background" opacity=100
mapping:
  center=-0.75/0.1 magn=4 angle=30
formula:
  maxiter=100 filename="Standard.ufm" entry="Mandelbrot" p_start=0/0
  p_power=3/0 p_bailout=4
inside:
  transfer=none
outside:
  transfer=sqrt filename="Standard.ucl" entry="Default" density=0.5 offset=100
gradient:
  smooth=no index=0 color=0 index=200 color=16777215
}
"#;

    #[test]
    fn converts_layers_views_colorings_and_gradients() {
        let images = parse(SAMPLE);
        assert_eq!(images.len(), 1);
        let im = &images[0];
        assert_eq!(im.name, "Two_Layers");
        let d = &im.doc;
        assert_eq!((d.image.width, d.image.height), (800, 600));
        assert_eq!(d.view.center, ["-0.75".to_string(), "0.1".to_string()]);
        assert!((d.view.radius - 1.5 / 4.0).abs() < 1e-12);
        assert_eq!(d.view.rotation, 30.0);
        // Bottom layer first.
        assert_eq!(d.layers[0].name, "Background");
        assert_eq!(d.layers[1].name, "Top");
        let bg = &d.layers[0];
        assert_eq!(bg.formula, "rfract:Mandelbrot");
        assert_eq!(bg.max_iter, 100);
        assert_eq!(bg.params.get("power"), Some(&ParamValue::Int(3)));
        assert_eq!(bg.outside_method, OutsideMethod::Formula);
        assert_eq!(bg.outside.formula, "rfract-coloring:Iterations");
        assert_eq!(bg.outside.transfer, Transfer::Sqrt);
        assert!((bg.outside.offset - 0.25).abs() < 1e-12);
        let top = &d.layers[1];
        assert!(top.julia);
        assert_eq!(top.seed, [-0.8, 0.156]);
        assert_eq!(top.merge, MergeMode::Multiply);
        assert!((top.opacity - 0.5).abs() < 1e-12);
        assert_eq!(top.outside_method, OutsideMethod::Smooth);
        assert!((top.outside.density - 0.1).abs() < 1e-12);
        assert_eq!(top.inside_method, InsideMethod::Solid);
        // 4278255360 = 0xFF00FF00: opaque green.
        assert_eq!(top.inside.solid_alpha, 1.0);
        assert!(top.inside.solid[1] > 0.99 && top.inside.solid[0] < 0.01);
        // Red at 0 (BGR 255), blue at 0.5.
        let g = &top.outside.gradient;
        assert_eq!(g.stops.len(), 2);
        assert!(g.stops[0].color[0] > 0.99 && g.stops[1].color[2] > 0.99);
        assert!(im.notes.is_empty(), "{:?}", im.notes);
    }

    #[test]
    fn missing_formulas_are_reported() {
        let text = "X {\nlayer:\n  caption=\"L\"\nformula:\n  maxiter=10 filename=\"nothere.ufm\" entry=\"Nope\"\noutside:\n  transfer=linear\n}\n";
        let images = parse(text);
        assert_eq!(images[0].doc.layers[0].formula, "rfract:Mandelbrot");
        assert!(images[0].notes.iter().any(|n| n.contains("nothere.ufm:Nope")), "{:?}", images[0].notes);
    }
}
