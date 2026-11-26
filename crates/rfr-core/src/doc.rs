// SPDX-License-Identifier: LGPL-3.0-or-later

//! The document (`.rfr`, JSON): image size and background, view and the layer tree (fractal
//! layers and groups with merge modes, opacity and masks).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::formula::ParamValue;
use crate::gradient::Gradient;

/// File format version; newer files are refused with a message, older ones are upgraded.
/// 2: layer tree (groups, merge modes, masks), transformations, coloring formulas, background.
pub const DOC_VERSION: u32 = 2;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Document {
    pub version: u32,
    pub image: Image,
    pub view: View,
    pub layers: Vec<Layer>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    /// Samples per pixel of final renders (export, command line).
    pub samples: u32,
    /// Below all layers (linear RGB and alpha; alpha 0 exports transparent PNGs).
    pub background: [f32; 4],
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct View {
    /// Center of the view as decimal strings (exact at any depth).
    pub center: [String; 2],
    /// Half the height of the view in plane units (any depth: floatexp).
    pub radius: crate::math::fx::Fx,
    /// Rotation in degrees (counterclockwise).
    pub rotation: f64,
}

impl View {
    pub fn center_f64(&self) -> [f64; 2] {
        [self.center[0].trim().parse().unwrap_or(0.0), self.center[1].trim().parse().unwrap_or(0.0)]
    }
    pub fn set_center(&mut self, c: [f64; 2]) {
        self.center = [format_coord(c[0], self.radius.to_f64()), format_coord(c[1], self.radius.to_f64())];
    }

    /// Moves the center by `d` (in units of the radius), exactly at any depth.
    pub fn pan(&mut self, d: [f64; 2]) {
        let scale = self.radius * 1e-3;
        for (c, d) in self.center.iter_mut().zip(d) {
            if d != 0.0 {
                *c = crate::math::big::add(c, self.radius * d, scale);
            }
        }
    }

    /// Zooms by `factor` (< 1: in) keeping the point at offset `at` from the center (in units
    /// of the radius) in place.
    pub fn zoom_at(&mut self, at: [f64; 2], factor: f64) {
        self.pan([at[0] * (1.0 - factor), at[1] * (1.0 - factor)]);
        self.radius *= factor;
    }

    /// Magnification relative to the default view (radius 1.5).
    pub fn magnification(&self) -> crate::math::fx::Fx {
        crate::math::fx::Fx::from_f64(1.5) / self.radius
    }
}

/// A coordinate with enough digits for the view's scale (17 significant digits relative to the
/// pixel size, so nothing is lost by the round trip through text).
pub fn format_coord(v: f64, radius: f64) -> String {
    let digits = ((-radius.abs().max(1e-300).log10()).ceil() as i64 + 6).clamp(6, 40) as usize;
    let s = format!("{v:.digits$}");
    let s = if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s };
    if s == "-0" { "0".into() } else { s }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum OutsideMethod {
    /// Iteration count (bands).
    Iterations,
    /// Continuous iteration count: divergent orbits by their escape radius, convergent ones by
    /// their last step.
    #[default]
    Smooth,
    /// Distance estimate to the set, relative to the pixel size (log scale).
    Distance,
    Solid,
    /// A coloring formula (`Coloring::formula`).
    Formula,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum InsideMethod {
    #[default]
    Solid,
    /// |z| after the last iteration.
    Magnitude,
    /// Angle of z after the last iteration.
    Angle,
    /// A coloring formula (`Coloring::formula`).
    Formula,
}

/// How a layer combines with what is below it (the separable modes work per channel, Hue to
/// Luminosity on hue, saturation and luminosity).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default, Hash)]
#[serde(rename_all = "lowercase")]
pub enum MergeMode {
    #[default]
    Normal,
    Multiply,
    Screen,
    Overlay,
    SoftLight,
    HardLight,
    ColorDodge,
    ColorBurn,
    Darken,
    Lighten,
    Difference,
    Exclusion,
    Add,
    Subtract,
    Hue,
    Saturation,
    Color,
    Luminosity,
}

impl MergeMode {
    pub const ALL: [MergeMode; 18] = [
        MergeMode::Normal,
        MergeMode::Multiply,
        MergeMode::Screen,
        MergeMode::Overlay,
        MergeMode::SoftLight,
        MergeMode::HardLight,
        MergeMode::ColorDodge,
        MergeMode::ColorBurn,
        MergeMode::Darken,
        MergeMode::Lighten,
        MergeMode::Difference,
        MergeMode::Exclusion,
        MergeMode::Add,
        MergeMode::Subtract,
        MergeMode::Hue,
        MergeMode::Saturation,
        MergeMode::Color,
        MergeMode::Luminosity,
    ];
    pub fn label(self) -> &'static str {
        match self {
            MergeMode::Normal => "Normal",
            MergeMode::Multiply => "Multiply",
            MergeMode::Screen => "Screen",
            MergeMode::Overlay => "Overlay",
            MergeMode::SoftLight => "Soft light",
            MergeMode::HardLight => "Hard light",
            MergeMode::ColorDodge => "Color dodge",
            MergeMode::ColorBurn => "Color burn",
            MergeMode::Darken => "Darken",
            MergeMode::Lighten => "Lighten",
            MergeMode::Difference => "Difference",
            MergeMode::Exclusion => "Exclusion",
            MergeMode::Add => "Add",
            MergeMode::Subtract => "Subtract",
            MergeMode::Hue => "Hue",
            MergeMode::Saturation => "Saturation",
            MergeMode::Color => "Color",
            MergeMode::Luminosity => "Luminosity",
        }
    }
    pub fn tip(self) -> &'static str {
        match self {
            MergeMode::Normal => "The layer covers what is below (by its alpha and opacity)",
            MergeMode::Multiply => "Darkens: multiplies the colors (white keeps the color below)",
            MergeMode::Screen => "Lightens: the inverse of multiplying the inverted colors (black keeps the color below)",
            MergeMode::Overlay => "Multiply in dark, screen in light parts of the image below: more contrast",
            MergeMode::SoftLight => "Gentle darkening or lightening by the layer's lightness",
            MergeMode::HardLight => "Multiply or screen by the layer's lightness: strong contrast",
            MergeMode::ColorDodge => "Brightens the image below by the layer's color",
            MergeMode::ColorBurn => "Darkens the image below by the layer's color",
            MergeMode::Darken => "The darker of both per channel",
            MergeMode::Lighten => "The lighter of both per channel",
            MergeMode::Difference => "The absolute difference of both",
            MergeMode::Exclusion => "Like difference with less contrast",
            MergeMode::Add => "Adds the colors (clipped)",
            MergeMode::Subtract => "Subtracts the layer from the image below (clipped)",
            MergeMode::Hue => "The layer's hue with the saturation and luminosity below",
            MergeMode::Saturation => "The layer's saturation with the hue and luminosity below",
            MergeMode::Color => "The layer's hue and saturation with the luminosity below",
            MergeMode::Luminosity => "The layer's luminosity with the hue and saturation below",
        }
    }
    pub fn index(self) -> u32 {
        MergeMode::ALL.iter().position(|m| *m == self).unwrap_or(0) as u32
    }
}

/// What a layer is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum LayerKind {
    #[default]
    Fractal,
    /// Combines its children first, then merges the result like one layer.
    Group,
    /// Where the orbits of the layer's formula go: Buddhabrot, attractors (see [`Density`]).
    Density,
}

/// Which orbits a density layer draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum DensityMode {
    /// Orbits of points that escape (Buddhabrot): random c in the region.
    #[default]
    Escaping,
    /// Orbits of points that stay (anti-Buddhabrot).
    Captive,
    /// Every orbit from random starts in the region after a warm-up (strange attractors).
    Attractor,
}

impl DensityMode {
    pub fn index(self) -> u32 {
        match self {
            DensityMode::Escaping => 0,
            DensityMode::Captive => 1,
            DensityMode::Attractor => 2,
        }
    }
}

/// Settings of a density layer: the orbits of its formula are counted per pixel and the
/// counts are tone-mapped (log density) to the outside gradient, or to RGB (Nebulabrot).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Density {
    pub mode: DensityMode,
    /// Escaping orbits shorter than this are not drawn.
    pub min_iter: u32,
    /// Nebulabrot: red, green and blue count orbits up to their own iteration limits.
    pub nebula: bool,
    pub nebula_iter: [u32; 3],
    /// Where the start points are drawn from: x0, y0, x1, y1 (plane units).
    pub region: [f64; 4],
    /// Orbits per pass.
    pub orbits: u32,
    /// Attractors: iterations before points are counted.
    pub warmup: u32,
    pub brightness: f64,
    pub gamma: f64,
}

impl Default for Density {
    fn default() -> Self {
        Density {
            mode: DensityMode::Escaping,
            min_iter: 20,
            nebula: false,
            nebula_iter: [5000, 500, 50],
            region: [-2.0, -1.5, 1.0, 1.5],
            orbits: 65536,
            warmup: 20,
            brightness: 1.0,
            gamma: 1.5,
        }
    }
}

/// Where a mask's value comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum MaskSource {
    /// Luminosity of the mask's color times its alpha.
    #[default]
    Luminance,
    Alpha,
}

/// A fractal whose brightness (or alpha) limits where its layer shows.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Mask {
    pub enabled: bool,
    pub layer: Box<Layer>,
    pub invert: bool,
    pub source: MaskSource,
}

impl Default for Mask {
    fn default() -> Self {
        let mut layer = Layer { name: "Mask".into(), ..Layer::default() };
        layer.outside.gradient = crate::gradient::preset("Grayscale").unwrap_or_default();
        layer.inside.solid = [1.0, 1.0, 1.0];
        Mask { enabled: true, layer: Box::new(layer), invert: false, source: MaskSource::Luminance }
    }
}

/// A transformation (formula from the library) applied to the points before the fractal.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TransformRef {
    pub formula: String,
    pub params: BTreeMap<String, ParamValue>,
    pub enabled: bool,
}

impl Default for TransformRef {
    fn default() -> Self {
        TransformRef { formula: "rfract-transform:Affine".into(), params: BTreeMap::new(), enabled: true }
    }
}

/// Maps the coloring value before density and offset.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Transfer {
    #[default]
    Linear,
    Sqrt,
    Cbrt,
    Log,
    Square,
}

impl Transfer {
    pub const ALL: [Transfer; 5] = [Transfer::Linear, Transfer::Sqrt, Transfer::Cbrt, Transfer::Log, Transfer::Square];
    pub fn apply(self, v: f64) -> f64 {
        match self {
            Transfer::Linear => v,
            Transfer::Sqrt => v.max(0.0).sqrt(),
            Transfer::Cbrt => v.cbrt(),
            Transfer::Log => (1.0 + v.max(0.0)).ln(),
            Transfer::Square => v * v,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Transfer::Linear => "Linear",
            Transfer::Sqrt => "Square root",
            Transfer::Cbrt => "Cube root",
            Transfer::Log => "Logarithm",
            Transfer::Square => "Square",
        }
    }
    pub fn index(self) -> u32 {
        Transfer::ALL.iter().position(|t| *t == self).unwrap_or(0) as u32
    }
}

/// How one region (outside or inside) is colored.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Coloring {
    pub gradient: Gradient,
    pub transfer: Transfer,
    /// Gradient cycles per unit of the transferred value.
    pub density: f64,
    /// Shift along the gradient (0..1, one full cycle).
    pub offset: f64,
    /// Color of the solid method (linear RGB) and its alpha (0 leaves the region transparent).
    pub solid: [f32; 3],
    pub solid_alpha: f32,
    /// Coloring formula of the Formula method (library id) and its parameters.
    pub formula: String,
    pub params: BTreeMap<String, ParamValue>,
}

impl Default for Coloring {
    fn default() -> Self {
        Coloring {
            gradient: Gradient::default(),
            transfer: Transfer::Sqrt,
            density: 0.25,
            offset: 0.0,
            solid: [0.0, 0.0, 0.0],
            solid_alpha: 1.0,
            formula: "rfract-coloring:OrbitTrap".into(),
            params: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Layer {
    pub name: String,
    pub visible: bool,
    pub kind: LayerKind,
    /// Layers of a group, bottom first.
    pub children: Vec<Layer>,
    pub merge: MergeMode,
    /// 0..1
    pub opacity: f64,
    pub mask: Option<Mask>,
    /// Transformations of the points before the fractal, applied in order.
    pub transforms: Vec<TransformRef>,
    /// Formula id in the library (`rfract:Mandelbrot`, `fractalpark:<id>`).
    pub formula: String,
    /// Parameter values by name; missing ones use the formula's defaults.
    pub params: BTreeMap<String, ParamValue>,
    pub max_iter: u32,
    /// Julia mode (for formulas with a Mandelbrot/Julia switch) and its seed.
    pub julia: bool,
    pub seed: [f64; 2],
    pub outside_method: OutsideMethod,
    pub outside: Coloring,
    pub inside_method: InsideMethod,
    pub inside: Coloring,
    /// Density layers: which orbits and how they are tone-mapped.
    pub density: Density,
}

impl Default for Layer {
    fn default() -> Self {
        Layer {
            name: "Layer 1".into(),
            visible: true,
            kind: LayerKind::Fractal,
            children: Vec::new(),
            merge: MergeMode::Normal,
            opacity: 1.0,
            mask: None,
            transforms: Vec::new(),
            formula: "rfract:Mandelbrot".into(),
            params: BTreeMap::new(),
            max_iter: 500,
            julia: false,
            seed: [-0.8, 0.156],
            outside_method: OutsideMethod::Smooth,
            outside: Coloring::default(),
            inside_method: InsideMethod::Solid,
            inside: Coloring { density: 1.0, transfer: Transfer::Linear, ..Coloring::default() },
            density: Density::default(),
        }
    }
}

impl Default for Image {
    fn default() -> Self {
        Image { width: 1920, height: 1080, samples: 64, background: [0.0, 0.0, 0.0, 1.0] }
    }
}

impl Default for View {
    fn default() -> Self {
        View { center: ["-0.5".into(), "0".into()], radius: 1.4.into(), rotation: 0.0 }
    }
}

impl Default for Document {
    fn default() -> Self {
        Document { version: DOC_VERSION, image: Image::default(), view: View::default(), layers: vec![Layer::default()] }
    }
}

impl Document {
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    pub fn from_json(s: &str) -> Result<Document, String> {
        let mut d: Document = serde_json::from_str(s).map_err(|e| format!("not an RFract document: {e}"))?;
        if d.version > DOC_VERSION {
            return Err(format!("the document needs a newer RFract (file version {}, this program reads up to {DOC_VERSION})", d.version));
        }
        // Version 1 had one layer and no new fields: the defaults are the old behavior.
        d.version = DOC_VERSION;
        Ok(d)
    }

    /// The first visible fractal layer (depth first, top of the stack first).
    pub fn active_layer(&self) -> Option<&Layer> {
        fn find(list: &[Layer]) -> Option<&Layer> {
            list.iter().rev().filter(|l| l.visible).find_map(|l| match l.kind {
                LayerKind::Fractal | LayerKind::Density => Some(l),
                LayerKind::Group => find(&l.children),
            })
        }
        find(&self.layers)
    }

    /// The layer at `path` (indexes from the top level down through groups).
    pub fn layer(&self, path: &[usize]) -> Option<&Layer> {
        let (first, rest) = path.split_first()?;
        let mut l = self.layers.get(*first)?;
        for i in rest {
            l = l.children.get(*i)?;
        }
        Some(l)
    }

    pub fn layer_mut(&mut self, path: &[usize]) -> Option<&mut Layer> {
        let (first, rest) = path.split_first()?;
        let mut l = self.layers.get_mut(*first)?;
        for i in rest {
            l = l.children.get_mut(*i)?;
        }
        Some(l)
    }

    /// The list that holds the layer at `path` (the top level or a group's children).
    pub fn siblings_mut(&mut self, path: &[usize]) -> Option<&mut Vec<Layer>> {
        match path.split_last() {
            Some((_, [])) => Some(&mut self.layers),
            Some((_, parent)) => self.layer_mut(parent).map(|l| &mut l.children),
            None => None,
        }
    }

    /// A document showing library formula `id` at its suggested view.
    pub fn for_formula(id: &str) -> Document {
        let mut d = Document::default();
        if let Some(e) = crate::formula::library::library().get(id) {
            d.view.radius = e.view.radius.into();
            d.view.set_center(e.view.center);
            let l = &mut d.layers[0];
            l.formula = id.to_string();
            l.max_iter = e.view.max_iter.max(100);
            if let Some(seed) = e.view.julia {
                l.julia = true;
                l.seed = seed;
            }
            if let Some(c) = &e.view.outside_coloring {
                l.outside_method = OutsideMethod::Formula;
                l.outside.formula = c.clone();
            }
            if let Some(c) = &e.view.inside_coloring {
                l.inside_method = InsideMethod::Formula;
                l.inside.formula = c.clone();
            }
            l.name = e.title.clone();
        }
        d
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_round_trip_and_version_check() {
        let mut d = Document::default();
        d.layers[0].params.insert("power".into(), ParamValue::Int(3));
        d.layers[0].params.insert("start".into(), ParamValue::Complex([0.1, -0.2]));
        let back = Document::from_json(&d.to_json()).unwrap();
        assert_eq!(back, d);
        let newer = d.to_json().replace("\"version\": 2", "\"version\": 99");
        assert!(Document::from_json(&newer).is_err());
    }

    #[test]
    fn version_1_documents_upgrade() {
        let v1 = r#"{"version":1,"image":{"width":640,"height":360,"samples":16},"view":{"center":["-0.5","0"],"radius":1.4,"rotation":0},"layers":[{"formula":"rfract:Mandelbrot","max_iter":300}]}"#;
        let d = Document::from_json(v1).unwrap();
        assert_eq!(d.version, DOC_VERSION);
        assert_eq!(d.layers[0].merge, MergeMode::Normal);
        assert_eq!(d.layers[0].opacity, 1.0);
        assert_eq!(d.image.background, [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(d.active_layer().unwrap().max_iter, 300);
    }

    #[test]
    fn coordinates_keep_their_precision() {
        assert_eq!(format_coord(-0.5, 1.0), "-0.5");
        let v = -0.743643887037151;
        let s = format_coord(v, 1e-12);
        assert!((s.parse::<f64>().unwrap() - v).abs() < 1e-17, "{s}");
    }
}
