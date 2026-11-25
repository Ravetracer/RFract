// SPDX-License-Identifier: LGPL-3.0-or-later

//! The document (`.rfr`, JSON): image size, view and the layer stack. Stage 1 renders the
//! first visible layer; the model is already layer-shaped.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::formula::ParamValue;
use crate::gradient::Gradient;

/// File format version; newer files are refused with a message, older ones are upgraded.
pub const DOC_VERSION: u32 = 1;

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
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct View {
    /// Center of the view as decimal strings (exact at any depth).
    pub center: [String; 2],
    /// Half the height of the view in plane units.
    pub radius: f64,
    /// Rotation in degrees (counterclockwise).
    pub rotation: f64,
}

impl View {
    pub fn center_f64(&self) -> [f64; 2] {
        [self.center[0].trim().parse().unwrap_or(0.0), self.center[1].trim().parse().unwrap_or(0.0)]
    }
    pub fn set_center(&mut self, c: [f64; 2]) {
        self.center = [format_coord(c[0], self.radius), format_coord(c[1], self.radius)];
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
    /// Color of the solid method (sRGB, 0..1).
    pub solid: [f32; 3],
}

impl Default for Coloring {
    fn default() -> Self {
        Coloring { gradient: Gradient::default(), transfer: Transfer::Sqrt, density: 0.25, offset: 0.0, solid: [0.0, 0.0, 0.0] }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Layer {
    pub name: String,
    pub visible: bool,
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
}

impl Default for Layer {
    fn default() -> Self {
        Layer {
            name: "Layer 1".into(),
            visible: true,
            formula: "rfract:Mandelbrot".into(),
            params: BTreeMap::new(),
            max_iter: 500,
            julia: false,
            seed: [-0.8, 0.156],
            outside_method: OutsideMethod::Smooth,
            outside: Coloring::default(),
            inside_method: InsideMethod::Solid,
            inside: Coloring { density: 1.0, transfer: Transfer::Linear, ..Coloring::default() },
        }
    }
}

impl Default for Image {
    fn default() -> Self {
        Image { width: 1920, height: 1080, samples: 64 }
    }
}

impl Default for View {
    fn default() -> Self {
        View { center: ["-0.5".into(), "0".into()], radius: 1.4, rotation: 0.0 }
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
        let d: Document = serde_json::from_str(s).map_err(|e| format!("not an RFract document: {e}"))?;
        if d.version > DOC_VERSION {
            return Err(format!("the document needs a newer RFract (file version {}, this program reads up to {DOC_VERSION})", d.version));
        }
        Ok(d)
    }

    /// The layer the renderer shows in stage 1: the first visible one.
    pub fn active_layer(&self) -> Option<&Layer> {
        self.layers.iter().find(|l| l.visible)
    }

    /// A document showing library formula `id` at its suggested view.
    pub fn for_formula(id: &str) -> Document {
        let mut d = Document::default();
        if let Some(e) = crate::formula::library::library().get(id) {
            d.view.radius = e.view.radius;
            d.view.set_center(e.view.center);
            let l = &mut d.layers[0];
            l.formula = id.to_string();
            l.max_iter = e.view.max_iter.max(100);
            if let Some(seed) = e.view.julia {
                l.julia = true;
                l.seed = seed;
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
        let newer = d.to_json().replace("\"version\": 1", "\"version\": 99");
        assert!(Document::from_json(&newer).is_err());
    }

    #[test]
    fn coordinates_keep_their_precision() {
        assert_eq!(format_coord(-0.5, 1.0), "-0.5");
        let v = -0.743643887037151;
        let s = format_coord(v, 1e-12);
        assert!((s.parse::<f64>().unwrap() - v).abs() < 1e-17, "{s}");
    }
}
