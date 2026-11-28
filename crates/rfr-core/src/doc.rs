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
    /// Effects on the finished image (`render::post`).
    pub post: Post,
    /// Light sources in the plane: glows and lens flares ([`crate::flare`]).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub lights: Vec<LightSource>,
    /// Saved locations: named views to come back to.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub bookmarks: Vec<Bookmark>,
    /// The animation (keyframes are whole documents).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub animation: Option<Box<crate::animation::Animation>>,
}

/// The tone curve that changes nothing.
pub const IDENTITY_CURVE: [f64; 5] = [0.0, 0.25, 0.5, 0.75, 1.0];

/// How bright light is brought into the displayable range.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ToneMap {
    /// Values above 1 are clipped.
    #[default]
    None,
    Reinhard,
    Aces,
    Filmic,
    Agx,
}

impl ToneMap {
    pub const ALL: [ToneMap; 5] = [ToneMap::None, ToneMap::Reinhard, ToneMap::Aces, ToneMap::Filmic, ToneMap::Agx];

    pub fn index(self) -> u32 {
        ToneMap::ALL.iter().position(|t| *t == self).unwrap_or(0) as u32
    }

    pub fn label(self) -> &'static str {
        match self {
            ToneMap::None => "None (clip)",
            ToneMap::Reinhard => "Reinhard",
            ToneMap::Aces => "ACES",
            ToneMap::Filmic => "Filmic",
            ToneMap::Agx => "AgX",
        }
    }
}

/// Effects on the finished image, in this order: sharpening, bloom and streaks (light above a
/// threshold spreads), lens distortion and chromatic aberration, exposure (stops) and white
/// balance (temperature, tint: -1…1), tone mapping, saturation, split toning (dark parts
/// toward one color, bright parts toward another), contrast, vignette, film grain. The
/// defaults change nothing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Post {
    pub sharpen: f64,
    pub bloom: f64,
    pub bloom_threshold: f64,
    pub bloom_size: f64,
    pub streak: f64,
    pub streak_threshold: f64,
    /// Share of the image width.
    pub streak_length: f64,
    /// Degrees, counterclockwise.
    pub streak_angle: f64,
    /// Directions: 1 a line, 2 a cross, 3 and 4 stars.
    pub streak_count: u32,
    pub streak_tint: [f32; 3],
    /// Barrel (above 0) or pincushion (below 0) distortion.
    pub distortion: f64,
    /// Red drawn outward, blue inward, by this share of the distance from the center;
    /// `spectral`: a rainbow of seven wavelengths instead of three channels.
    pub chromatic: f64,
    pub spectral: bool,
    /// God rays: light above the threshold smeared toward a light position (share of the
    /// image width and height from the top left) over a share of the way (length).
    pub rays: f64,
    pub rays_threshold: f64,
    pub rays_position: [f64; 2],
    pub rays_length: f64,
    pub exposure: f64,
    pub temperature: f64,
    pub tint: f64,
    pub tone_map: ToneMap,
    pub saturation: f64,
    pub shadow_color: [f32; 3],
    pub shadow_amount: f64,
    pub highlight_color: [f32; 3],
    pub highlight_amount: f64,
    pub contrast: f64,
    pub vignette: f64,
    pub grain: f64,
    /// Levels (display values): black point, white point, midtones (gamma).
    pub levels_black: f64,
    pub levels_white: f64,
    pub levels_gamma: f64,
    /// Tone curve: the outputs at inputs 0, ¼, ½, ¾ and 1 (display values).
    pub curve: [f64; 5],
    /// Exports only: denoise the image (Intel Open Image Denoise) before the other effects;
    /// `denoise_blend` is the share of the denoised image (1: only denoised).
    pub denoise: bool,
    pub denoise_blend: f64,
}

impl Default for Post {
    fn default() -> Self {
        Post {
            sharpen: 0.0,
            bloom: 0.0,
            bloom_threshold: 1.0,
            bloom_size: 0.6,
            streak: 0.0,
            streak_threshold: 1.0,
            streak_length: 0.3,
            streak_angle: 0.0,
            streak_count: 2,
            streak_tint: [1.0, 1.0, 1.0],
            distortion: 0.0,
            chromatic: 0.0,
            spectral: false,
            rays: 0.0,
            rays_threshold: 1.0,
            rays_position: [0.3, 0.25],
            rays_length: 0.6,
            exposure: 0.0,
            temperature: 0.0,
            tint: 0.0,
            tone_map: ToneMap::None,
            saturation: 1.0,
            shadow_color: [0.2, 0.4, 1.0],
            shadow_amount: 0.0,
            highlight_color: [1.0, 0.7, 0.3],
            highlight_amount: 0.0,
            contrast: 1.0,
            vignette: 0.0,
            grain: 0.0,
            levels_black: 0.0,
            levels_white: 1.0,
            levels_gamma: 1.0,
            curve: IDENTITY_CURVE,
            denoise: false,
            denoise_blend: 1.0,
        }
    }
}

impl Post {
    /// Whether the effects change nothing.
    pub fn is_neutral(&self) -> bool {
        self.sharpen == 0.0
            && self.bloom <= 0.0
            && self.streak <= 0.0
            && self.distortion == 0.0
            && self.chromatic == 0.0
            && self.exposure == 0.0
            && self.temperature == 0.0
            && self.tint == 0.0
            && self.tone_map == ToneMap::None
            && self.saturation == 1.0
            && self.shadow_amount <= 0.0
            && self.highlight_amount <= 0.0
            && self.contrast == 1.0
            && self.vignette == 0.0
            && self.grain == 0.0
            && self.rays <= 0.0
            && self.levels_black == 0.0
            && self.levels_white == 1.0
            && self.levels_gamma == 1.0
            && self.curve == IDENTITY_CURVE
    }
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
    /// Adaptive sampling: a pixel stops taking samples once its noise is below this (0: off;
    /// see `render::adaptive_update`).
    pub adaptive: f64,
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
    /// A raymarched 3D fractal (see [`Object3D`]), lit like a lit layer.
    Object,
}

/// The shape of a 3D object layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ObjectShape {
    /// The Mandelbulb (power n, spherical coordinates).
    #[default]
    Mandelbulb,
    /// The quaternion Julia set of q² + c (a 3D slice at w = 0).
    Quaternion,
    /// The Mandelbox (box and sphere folds, scale).
    Mandelbox,
    /// The Menger sponge.
    Menger,
}

impl ObjectShape {
    pub const ALL: [ObjectShape; 4] = [ObjectShape::Mandelbulb, ObjectShape::Quaternion, ObjectShape::Mandelbox, ObjectShape::Menger];

    pub fn index(self) -> u32 {
        ObjectShape::ALL.iter().position(|s| *s == self).unwrap_or(0) as u32
    }
}

/// A 3D object layer: the shape, its parameters (`power` for the Mandelbulb, `scale` for the
/// Mandelbox), the iterations, Julia mode with its seed (x, y, z, w; the quaternion Julia set
/// always uses it), the camera orbiting the origin (distance in object units) and the detail
/// (smaller: finer surfaces, slower).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Object3D {
    pub shape: ObjectShape,
    pub power: f64,
    pub scale: f64,
    pub iterations: u32,
    pub julia: bool,
    pub seed: [f64; 4],
    pub camera: Camera,
    pub detail: f64,
}

impl Default for Object3D {
    fn default() -> Self {
        Object3D {
            shape: ObjectShape::Mandelbulb,
            power: 8.0,
            scale: 2.0,
            iterations: 10,
            julia: false,
            seed: [-0.291, -0.399, 0.339, 0.437],
            camera: Camera { yaw: 30.0, pitch: 25.0, distance: 3.0, fov: 40.0 },
            detail: 1.0,
        }
    }
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
    /// The chaos game over the layer's affine maps (iterated function systems).
    Ifs,
    /// A random walk over two Möbius generators and their inverses: the limit set of a
    /// Kleinian group (see [`Kleinian`]).
    Kleinian,
    /// The strokes of an L-system's turtle (see [`LSystem`]).
    LSystem,
}

/// An L-system: the axiom is rewritten by the rules `iterations` times; the turtle then draws
/// `F` and `D` (forward with a line), moves on `G`, `M` and `f`, turns by 360°/`angle` on `+` and
/// `-`, turns around on `|`, swaps the turn directions on `!`, saves and restores its state on
/// `[` and `]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LSystem {
    pub axiom: String,
    /// One rule per line: `F=F+F--F+F`.
    pub rules: String,
    /// Turn angle as a division of the full circle (6: 60°).
    pub angle: f64,
    pub iterations: u32,
    /// Stroke width in turtle steps.
    pub width: f64,
}

impl Default for LSystem {
    fn default() -> Self {
        LSystem { axiom: "F--F--F".into(), rules: "F=F+F--F+F".into(), angle: 6.0, iterations: 4, width: 0.15 }
    }
}

/// The most segments an L-system draws.
pub const LSYSTEM_MAX_SEGMENTS: usize = 2_000_000;

impl LSystem {
    /// The rewritten string (limited in length).
    pub fn expand(&self) -> String {
        let rules: Vec<(char, String)> = self
            .rules
            .lines()
            .filter_map(|l| {
                let (k, v) = l.split_once('=')?;
                let c = k.trim().chars().next()?;
                Some((c.to_ascii_uppercase(), v.trim().to_string()))
            })
            .collect();
        let mut s = self.axiom.trim().to_string();
        for _ in 0..self.iterations.min(20) {
            let mut next = String::with_capacity(s.len() * 3);
            for ch in s.chars() {
                match rules.iter().find(|(k, _)| *k == ch.to_ascii_uppercase()) {
                    Some((_, r)) => next.push_str(r),
                    None => next.push(ch),
                }
                if next.len() > 8 * LSYSTEM_MAX_SEGMENTS {
                    return next;
                }
            }
            s = next;
        }
        s
    }

    /// The turtle's line segments (x0, y0, x1, y1), steps of length 1 from the origin.
    pub fn segments(&self) -> Vec<[f64; 4]> {
        let turn = std::f64::consts::TAU / self.angle.max(1.0);
        let (mut x, mut y, mut a, mut sign) = (0.0f64, 0.0f64, std::f64::consts::FRAC_PI_2, 1.0f64);
        let mut stack = Vec::new();
        let mut out = Vec::new();
        for ch in self.expand().chars() {
            match ch {
                'F' | 'D' | 'G' | 'M' | 'f' => {
                    let (nx, ny) = (x + a.cos(), y + a.sin());
                    if matches!(ch, 'F' | 'D') {
                        out.push([x, y, nx, ny]);
                        if out.len() >= LSYSTEM_MAX_SEGMENTS {
                            break;
                        }
                    }
                    (x, y) = (nx, ny);
                }
                '+' => a += sign * turn,
                '-' => a -= sign * turn,
                '|' => a += std::f64::consts::PI,
                '!' => sign = -sign,
                '[' => stack.push((x, y, a, sign)),
                ']' => {
                    if let Some(s) = stack.pop() {
                        (x, y, a, sign) = s;
                    }
                }
                _ => {}
            }
        }
        out
    }
}

/// How a Kleinian group's two generators are made.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Recipe {
    /// Grandma's two-generator recipe from the traces of a and b (ta = tb = 2: the Apollonian
    /// gasket; tb = 2: Maskit's groups).
    #[default]
    Grandma,
    /// Riley's groups: a = [1 2; 0 1], b = [1 0; c 1] with c = ta.
    Riley,
}

/// Settings of a Kleinian density layer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Kleinian {
    pub recipe: Recipe,
    /// Trace of a (Riley: the parameter c), complex.
    pub ta: [f64; 2],
    /// Trace of b, complex.
    pub tb: [f64; 2],
    /// Which root of the recipe's quadratic for the trace of ab.
    pub other_root: bool,
}

impl Default for Kleinian {
    fn default() -> Self {
        Kleinian { recipe: Recipe::Grandma, ta: [1.91, 0.05], tb: [1.91, 0.05], other_root: false }
    }
}

impl Kleinian {
    /// The generators a, b, a⁻¹, b⁻¹ as Möbius matrices [p, q, r, s]: z ↦ (p·z + q) / (r·z + s).
    pub fn generators(&self) -> [[crate::math::complex::C64; 4]; 4] {
        use crate::math::complex::C64;
        let c = |v: [f64; 2]| C64::new(v[0], v[1]);
        let i = C64::new(0.0, 1.0);
        let two = C64::new(2.0, 0.0);
        let (a, b) = match self.recipe {
            Recipe::Riley => {
                let one = C64::new(1.0, 0.0);
                let zero = C64::new(0.0, 0.0);
                ([one, two, zero, one], [one, zero, c(self.ta), one])
            }
            Recipe::Grandma => {
                let (ta, tb) = (c(self.ta), c(self.tb));
                // tab² − ta·tb·tab + ta² + tb² = 0
                let disc = (ta * ta * tb * tb - C64::new(4.0, 0.0) * (ta * ta + tb * tb)).sqrt();
                let tab = if self.other_root { (ta * tb - disc) / two } else { (ta * tb + disc) / two };
                let z0 = (tab - two) * tb / (tb * tab - two * ta + two * i * tab);
                let four = C64::new(4.0, 0.0);
                let a = [
                    ta / two,
                    (ta * tab - two * tb + four * i) / ((two * tab + four) * z0),
                    (ta * tab - two * tb - four * i) * z0 / (two * tab - four),
                    ta / two,
                ];
                let b = [(tb - two * i) / two, tb / two, tb / two, (tb + two * i) / two];
                (a, b)
            }
        };
        // Inverse of [p q; r s] with determinant 1: [s −q; −r p].
        let inv = |m: [C64; 4]| [m[3], -m[1], -m[2], m[0]];
        [a, b, inv(a), inv(b)]
    }
}

/// A nonlinear function applied after an IFS map's affine part (as in fractal flames).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Variation {
    #[default]
    Linear,
    Sinusoidal,
    Spherical,
    Swirl,
    Horseshoe,
    Polar,
    Handkerchief,
    Heart,
    Disc,
    Spiral,
}

impl Variation {
    pub const ALL: [Variation; 10] = [
        Variation::Linear,
        Variation::Sinusoidal,
        Variation::Spherical,
        Variation::Swirl,
        Variation::Horseshoe,
        Variation::Polar,
        Variation::Handkerchief,
        Variation::Heart,
        Variation::Disc,
        Variation::Spiral,
    ];

    pub fn index(self) -> u32 {
        Variation::ALL.iter().position(|v| *v == self).unwrap_or(0) as u32
    }

    pub fn label(self) -> &'static str {
        match self {
            Variation::Linear => "Linear",
            Variation::Sinusoidal => "Sinusoidal",
            Variation::Spherical => "Spherical",
            Variation::Swirl => "Swirl",
            Variation::Horseshoe => "Horseshoe",
            Variation::Polar => "Polar",
            Variation::Handkerchief => "Handkerchief",
            Variation::Heart => "Heart",
            Variation::Disc => "Disc",
            Variation::Spiral => "Spiral",
        }
    }

    /// The variation at (x, y) (`theta = atan2(x, y)` as in fractal flames).
    pub fn apply(self, x: f64, y: f64) -> (f64, f64) {
        let r2 = x * x + y * y;
        let r = r2.sqrt();
        let th = x.atan2(y);
        let pi = std::f64::consts::PI;
        match self {
            Variation::Linear => (x, y),
            Variation::Sinusoidal => (x.sin(), y.sin()),
            Variation::Spherical => {
                let k = 1.0 / (r2 + 1e-12);
                (x * k, y * k)
            }
            Variation::Swirl => {
                let (s, c) = r2.sin_cos();
                (x * s - y * c, x * c + y * s)
            }
            Variation::Horseshoe => {
                let k = 1.0 / (r + 1e-12);
                ((x - y) * (x + y) * k, 2.0 * x * y * k)
            }
            Variation::Polar => (th / pi, r - 1.0),
            Variation::Handkerchief => (r * (th + r).sin(), r * (th - r).cos()),
            Variation::Heart => (r * (th * r).sin(), -r * (th * r).cos()),
            Variation::Disc => (th / pi * (pi * r).sin(), th / pi * (pi * r).cos()),
            Variation::Spiral => {
                let k = 1.0 / (r + 1e-12);
                (k * (th.cos() + r.sin()), k * (th.sin() - r.cos()))
            }
        }
    }
}

/// One map of an iterated function system: `x' = a·x + b·y + e`, `y' = c·x + d·y + f` (the
/// coefficients a…f in that order), then its variation; `weight` is its share of the steps.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct IfsMap {
    pub coefs: [f64; 6],
    pub weight: f64,
    pub variation: Variation,
}

impl Default for IfsMap {
    fn default() -> Self {
        IfsMap { coefs: [0.5, 0.0, 0.0, 0.5, 0.0, 0.0], weight: 1.0, variation: Variation::Linear }
    }
}

impl IfsMap {
    pub fn new(coefs: [f64; 6], weight: f64) -> IfsMap {
        IfsMap { coefs, weight, variation: Variation::Linear }
    }

    pub fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        let c = &self.coefs;
        self.variation.apply(c[0] * x + c[1] * y + c[4], c[2] * x + c[3] * y + c[5])
    }
}

/// Named iterated function systems for the inspector.
pub fn ifs_presets() -> Vec<(&'static str, Vec<IfsMap>)> {
    vec![
        ("Sierpinski triangle", sierpinski()),
        (
            "Barnsley fern",
            vec![
                IfsMap::new([0.0, 0.0, 0.0, 0.16, 0.0, 0.0], 0.01),
                IfsMap::new([0.85, 0.04, -0.04, 0.85, 0.0, 1.6], 0.85),
                IfsMap::new([0.2, -0.26, 0.23, 0.22, 0.0, 1.6], 0.07),
                IfsMap::new([-0.15, 0.28, 0.26, 0.24, 0.0, 0.44], 0.07),
            ],
        ),
        ("Heighway dragon", vec![IfsMap::new([0.5, -0.5, 0.5, 0.5, 0.0, 0.0], 0.5), IfsMap::new([-0.5, -0.5, 0.5, -0.5, 1.0, 0.0], 0.5)]),
        ("Lévy C curve", vec![IfsMap::new([0.5, -0.5, 0.5, 0.5, 0.0, 0.0], 0.5), IfsMap::new([0.5, 0.5, -0.5, 0.5, 0.5, 0.5], 0.5)]),
    ]
}

/// The Sierpinski triangle (the default IFS).
pub fn sierpinski() -> Vec<IfsMap> {
    vec![
        IfsMap::new([0.5, 0.0, 0.0, 0.5, -0.5, -0.5], 1.0),
        IfsMap::new([0.5, 0.0, 0.0, 0.5, 0.5, -0.5], 1.0),
        IfsMap::new([0.5, 0.0, 0.0, 0.5, 0.0, 0.5], 1.0),
    ]
}

impl DensityMode {
    pub fn index(self) -> u32 {
        match self {
            DensityMode::Escaping => 0,
            DensityMode::Captive => 1,
            DensityMode::Attractor => 2,
            DensityMode::Ifs => 3,
            DensityMode::Kleinian => 4,
            DensityMode::LSystem => 5,
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
    /// Escaping and captive orbits: start points from Metropolis chains that favor orbits
    /// through the view (for zoomed views) instead of uniform ones.
    pub metropolis: bool,
    /// Attractors: iterations before points are counted.
    pub warmup: u32,
    pub brightness: f64,
    pub gamma: f64,
    /// The maps of the IFS mode.
    pub ifs: Vec<IfsMap>,
    /// The group of the Kleinian mode.
    pub kleinian: Kleinian,
    /// The L-system of the L-system mode.
    pub lsystem: LSystem,
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
            metropolis: false,
            warmup: 20,
            brightness: 1.0,
            gamma: 1.5,
            ifs: sierpinski(),
            kleinian: Kleinian::default(),
            lsystem: LSystem::default(),
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
    /// By the distribution of the values in the view (see [`crate::render::histogram`]);
    /// [`Transfer::apply`] leaves the value as it is.
    Histogram,
}

impl Transfer {
    pub const ALL: [Transfer; 6] = [Transfer::Linear, Transfer::Sqrt, Transfer::Cbrt, Transfer::Log, Transfer::Square, Transfer::Histogram];
    pub fn apply(self, v: f64) -> f64 {
        match self {
            Transfer::Linear | Transfer::Histogram => v,
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
            Transfer::Histogram => "Histogram",
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
    /// Post-transformations: applied in order to `z` as the coloring formulas see it (at
    /// their init, every iteration and the end); the orbit itself does not change.
    pub post_transforms: Vec<TransformRef>,
    /// Formula id in the library (`rfract:Mandelbrot`, `fractalpark:<id>`).
    pub formula: String,
    /// Parameter values by name; missing ones use the formula's defaults.
    pub params: BTreeMap<String, ParamValue>,
    pub max_iter: u32,
    /// The app raises `max_iter` when the view needs more (see
    /// [`crate::render::iterations::suggest`]); renders use the stored value.
    pub auto_iterations: bool,
    /// Julia mode (for formulas with a Mandelbrot/Julia switch) and its seed.
    pub julia: bool,
    pub seed: [f64; 2],
    pub outside_method: OutsideMethod,
    pub outside: Coloring,
    pub inside_method: InsideMethod,
    pub inside: Coloring,
    /// Density layers: which orbits and how they are tone-mapped.
    pub density: Density,
    /// Lighting of the layer as a height field (not for density layers); 3D object layers
    /// use its lights, highlights, shadows, occlusion and reflections.
    pub light: Lighting,
    /// 3D object layers: the shape and its camera.
    pub object: Object3D,
    /// Effects of the layer: shadow, glows, neon edges.
    pub effects: LayerEffects,
}

/// Effects of a layer, each off at 0: a drop shadow (opacity, distance and blur as shares of
/// the image height, direction in degrees counterclockwise, color), an outer glow and an inner
/// glow (strength, size, color) from the layer's alpha, and neon edges: light along the
/// outside of the set by the distance estimate (strength, width in pixels at 1000 pixels
/// image height, color).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LayerEffects {
    pub shadow: f64,
    pub shadow_distance: f64,
    pub shadow_angle: f64,
    pub shadow_size: f64,
    pub shadow_color: [f32; 3],
    pub glow: f64,
    pub glow_size: f64,
    pub glow_color: [f32; 3],
    pub inner_glow: f64,
    pub inner_size: f64,
    pub inner_color: [f32; 3],
    pub neon: f64,
    pub neon_width: f64,
    pub neon_color: [f32; 3],
}

impl Default for LayerEffects {
    fn default() -> Self {
        LayerEffects {
            shadow: 0.0,
            shadow_distance: 0.01,
            shadow_angle: 315.0,
            shadow_size: 0.01,
            shadow_color: [0.0, 0.0, 0.0],
            glow: 0.0,
            glow_size: 0.02,
            glow_color: [1.0, 0.8, 0.4],
            inner_glow: 0.0,
            inner_size: 0.01,
            inner_color: [1.0, 1.0, 1.0],
            neon: 0.0,
            neon_width: 4.0,
            neon_color: [0.3, 0.8, 1.0],
        }
    }
}

impl LayerEffects {
    /// Whether any effect is on.
    pub fn active(&self) -> bool {
        self.shadow > 0.0 || self.glow > 0.0 || self.inner_glow > 0.0 || self.neon > 0.0
    }
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
            post_transforms: Vec::new(),
            formula: "rfract:Mandelbrot".into(),
            params: BTreeMap::new(),
            max_iter: 500,
            auto_iterations: false,
            julia: false,
            seed: [-0.8, 0.156],
            outside_method: OutsideMethod::Smooth,
            outside: Coloring::default(),
            inside_method: InsideMethod::Solid,
            inside: Coloring { density: 1.0, transfer: Transfer::Linear, ..Coloring::default() },
            density: Density::default(),
            light: Lighting::default(),
            object: Object3D::default(),
            effects: LayerEffects::default(),
        }
    }
}

/// Where a lit layer's heights come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum HeightSource {
    /// The logarithm of the smooth iteration count (points that stay: the iteration limit).
    #[default]
    Smooth,
    /// The coloring value (the gradient position before the offset).
    Value,
    /// The luminance of the layer's color (bump maps from images).
    Luminance,
    /// The estimated distance to the set (logarithmic, falling away from it; needs the
    /// orbit's derivative).
    Distance,
}

impl HeightSource {
    pub const ALL: [HeightSource; 4] = [HeightSource::Smooth, HeightSource::Value, HeightSource::Luminance, HeightSource::Distance];

    pub fn index(self) -> u32 {
        HeightSource::ALL.iter().position(|v| *v == self).unwrap_or(0) as u32
    }
}

/// The highlight model of a lit layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SpecularModel {
    BlinnPhong,
    #[default]
    Ggx,
}

/// A directional light: where it comes from (degrees: azimuth counterclockwise from the
/// right edge of the image, elevation above the image plane), its color (linear RGB) and
/// strength.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Light {
    pub azimuth: f64,
    pub elevation: f64,
    pub color: [f32; 3],
    pub intensity: f64,
}

impl Default for Light {
    fn default() -> Self {
        Light { azimuth: 135.0, elevation: 45.0, color: [1.0, 1.0, 1.0], intensity: 1.0 }
    }
}

/// The most lights of a layer.
pub const MAX_LIGHTS: usize = 4;

/// Lighting of a layer: the layer's values become a height field (`height`, scaled by
/// `relief`), lit by `lights` from above with `ambient` light and highlights.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Lighting {
    pub enabled: bool,
    pub height: HeightSource,
    /// Steepness of the relief (0: flat).
    pub relief: f64,
    pub ambient: f64,
    pub specular: f64,
    /// Roughness of the highlights (0.02 glossy … 1 matte).
    pub roughness: f64,
    pub model: SpecularModel,
    pub lights: Vec<Light>,
    /// Shadows cast over the relief, how far they reach (share of the image height) and how
    /// soft their edges are (the lights' apparent size).
    pub shadows: bool,
    pub shadow_length: f64,
    pub softness: f64,
    /// Ambient occlusion: how much hollows darken (0: off) and how far they look (share of
    /// the image height).
    pub occlusion: f64,
    pub occlusion_radius: f64,
    /// Reflections of the environment (0: none … 1: a mirror): an equirectangular image
    /// (`environment`, a file path) or a sky from `horizon` to `zenith` (linear RGB).
    pub reflection: f64,
    pub environment: String,
    pub horizon: [f32; 3],
    pub zenith: [f32; 3],
    /// The document's light sources ([`LightSource`]) also light the layer, as point lights
    /// at their position and height.
    pub scene_lights: bool,
    /// Show the relief as a 3D terrain seen by `camera` instead of from straight above;
    /// `terrain_height` is the height of its highest part as a share of the image height.
    pub terrain: bool,
    pub terrain_height: f64,
    pub camera: Camera,
    /// 3D terrain: walls down to a base close the terrain into a block; the base lies
    /// `wall_depth` (a share of the terrain height) below the lowest ground of the height
    /// source. The walls show the edge's colors, fading to `wall_color` toward the base.
    pub walls: bool,
    pub wall_depth: f64,
    pub wall_color: [f32; 3],
    /// Renders the surface normals as colors instead of shading (the denoiser's normal
    /// image, `render::post::aux_documents`); never saved.
    #[serde(skip)]
    pub normal_output: bool,
}

/// A camera orbiting the middle of a terrain: the direction it looks from (degrees, 0: from
/// the bottom edge of the image, counterclockwise), its angle above the ground (degrees), its
/// distance (image heights) and its vertical field of view (degrees).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Camera {
    pub yaw: f64,
    pub pitch: f64,
    pub distance: f64,
    pub fov: f64,
}

impl Default for Camera {
    fn default() -> Self {
        Camera { yaw: 0.0, pitch: 35.0, distance: 1.2, fov: 50.0 }
    }
}

impl Default for Lighting {
    fn default() -> Self {
        Lighting {
            enabled: false,
            height: HeightSource::Smooth,
            relief: 1.0,
            ambient: 0.3,
            specular: 0.3,
            roughness: 0.4,
            model: SpecularModel::Ggx,
            lights: vec![Light::default()],
            shadows: false,
            shadow_length: 0.3,
            softness: 0.05,
            occlusion: 0.0,
            occlusion_radius: 0.03,
            reflection: 0.0,
            environment: String::new(),
            horizon: [0.85, 0.8, 0.75],
            zenith: [0.15, 0.3, 0.7],
            scene_lights: false,
            terrain: false,
            terrain_height: 0.15,
            camera: Camera::default(),
            walls: false,
            wall_depth: 0.15,
            wall_color: [0.08, 0.07, 0.06],
            normal_output: false,
        }
    }
}

impl Default for Image {
    fn default() -> Self {
        Image { width: 1920, height: 1080, samples: 64, background: [0.0, 0.0, 0.0, 1.0], adaptive: 0.005 }
    }
}

impl Default for View {
    fn default() -> Self {
        View { center: ["-0.5".into(), "0".into()], radius: 1.4.into(), rotation: 0.0 }
    }
}

impl Default for Document {
    fn default() -> Self {
        Document {
            version: DOC_VERSION,
            image: Image::default(),
            view: View::default(),
            layers: vec![Layer::default()],
            post: Post::default(),
            lights: Vec::new(),
            bookmarks: Vec::new(),
            animation: None,
        }
    }
}

/// A saved location: a name and a view.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Bookmark {
    pub name: String,
    pub view: View,
}

impl Document {
    /// The bookmark called `name` (case does not matter).
    pub fn bookmark(&self, name: &str) -> Option<&Bookmark> {
        self.bookmarks.iter().find(|b| b.name.eq_ignore_ascii_case(name.trim()))
    }
}

/// A light source of the document: a point in the plane (it moves with the fractal), shown as
/// a glow when `visible`, with an optional lens flare.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LightSource {
    pub enabled: bool,
    pub name: String,
    /// Position in the plane (decimal strings, exact at any depth, like the view center).
    pub position: [String; 2],
    /// Height above the plane as a share of the view's height.
    pub height: f64,
    /// Lighting of lit layers and 3D objects: the distance (share of the image height) at
    /// which the light has fallen to half (0: no falloff).
    pub reach: f64,
    /// Linear RGB.
    pub color: [f32; 3],
    pub intensity: f64,
    /// The light itself shows as a glow.
    pub visible: bool,
    pub glow_shape: crate::flare::GlowShape,
    /// Glow radius as a share of the image height.
    pub glow_size: f64,
    pub glow_intensity: f64,
    pub flare: crate::flare::LensFlare,
}

impl Default for LightSource {
    fn default() -> Self {
        LightSource {
            enabled: true,
            name: "Light 1".into(),
            position: ["0".into(), "0".into()],
            height: 0.5,
            reach: 0.0,
            color: [1.0, 0.95, 0.85],
            intensity: 1.0,
            visible: true,
            glow_shape: crate::flare::GlowShape::Soft,
            glow_size: 0.04,
            glow_intensity: 1.0,
            flare: crate::flare::LensFlare::default(),
        }
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
                LayerKind::Fractal | LayerKind::Density | LayerKind::Object => Some(l),
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
    fn bookmarks_are_saved_and_found_by_name() {
        let mut d = Document::default();
        let deep =
            View { center: ["-0.743643887037151".into(), "0.131825904205330".into()], radius: crate::math::fx::Fx::from_f64(1e-40), rotation: 30.0 };
        d.bookmarks.push(Bookmark { name: "Seahorse".into(), view: deep.clone() });
        let back = Document::from_json(&d.to_json()).unwrap();
        assert_eq!(back.bookmark(" seahorse").map(|b| &b.view), Some(&deep));
        assert!(back.bookmark("elephant").is_none());
        // No bookmarks, no field.
        assert!(!Document::default().to_json().contains("bookmarks"));
    }

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
