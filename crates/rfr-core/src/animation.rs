// SPDX-License-Identifier: LGPL-3.0-or-later

//! Keyframe animation: keyframes are whole documents; every number blends between them.
//!
//! The view follows its own path: the radius moves on a monotone cubic through the keyframes
//! in log space (zooming at an even pace feels even), the center so that the next keyframe's
//! center stays put on the screen while zooming toward it (c = c₁ + (c₀ − c₁)·(r − r₁)/(r₀ − r₁),
//! exact on the decimal text at any depth), the rotation on a monotone cubic of the unwrapped
//! angle. Frames are spread over the keyframes either by frame counts per keyframe with an
//! easing each, or at a steady perceived speed (zoom, pan relative to the view and turning)
//! over a length in seconds with smooth starts and stops.
//!
//! Everything else blends with a monotone cubic (no overshoot) through the keyframe values:
//! the documents are blended as JSON, numbers interpolated (integers rounded, distances on a
//! log scale, angles unwrapped), everything else (texts, choices, lists of other lengths) held
//! from the keyframe before. Palette rotation turns all gradients over time. Parts adapted from
//! RMandelbulb3D (`animation.rs`).

use serde::{Deserialize, Serialize};
use serde_json::Value;

use std::path::{Path as FilePath, PathBuf};

use crate::audio::AudioLevels;
use crate::doc::{Camera, Document, Layer, View};
use crate::math::big;
use crate::math::fx::Fx;

/// The animation of a document.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Animation {
    pub fps: f64,
    pub keyframes: Vec<Keyframe>,
    pub timing: Timing,
    /// Steady timing: length in seconds, seconds to speed up at the start and to slow down at
    /// the end.
    pub length_s: f64,
    pub ease_in_s: f64,
    pub ease_out_s: f64,
    /// Palette rotation: gradient turns per second (all colorings).
    pub palette_speed: f64,
    /// Motion blur: how long each frame is exposed, as a share of the frame time (0: off, 0.5:
    /// a film camera's 180° shutter).
    pub shutter: f64,
    /// Zoom sequence: a straight zoom from the first keyframe's size into the last keyframe
    /// (its center and settings), composed from one image per doubling of the zoom (much
    /// faster for long zooms; no palette rotation).
    pub zoom_sequence: bool,
    /// Recorded timing: speed changes are smoothed over this many seconds (0: as flown).
    pub speed_smoothing_s: f64,
    /// Values computed by expressions in every frame (after the keyframes).
    pub drivers: Vec<Driver>,
    /// Sound file put under the movie (relative paths: relative to the document file).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio: Option<PathBuf>,
    /// Position in the sound file (seconds) where frame 0 starts; negative values start the
    /// sound later.
    pub audio_offset_s: f64,
    /// Levels of the sound track for audio-reactive drivers (`audio::analyse`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_levels: Option<AudioLevels>,
}

/// A document value driven by an expression of time (and other values) in an animation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Driver {
    pub enabled: bool,
    /// The value: a path into the document (`view.rotation`, `layers.0.params.power`).
    pub path: String,
    /// The expression (see `expr`): variables `t` (seconds), `frame`, `frames`, `u` (0..1 over
    /// the animation), `key` (keyframe position), `fps`, `value` (the keyframed value),
    /// `audio`, `bass`, `mid`, `treble` (sound levels 0..1).
    pub expression: String,
}

impl Default for Driver {
    fn default() -> Self {
        Driver { enabled: true, path: String::new(), expression: "value".into() }
    }
}

impl Default for Animation {
    fn default() -> Self {
        Animation {
            fps: 30.0,
            keyframes: Vec::new(),
            timing: Timing::Keyframes,
            length_s: 10.0,
            ease_in_s: 1.0,
            ease_out_s: 1.0,
            palette_speed: 0.0,
            shutter: 0.0,
            zoom_sequence: false,
            speed_smoothing_s: 1.0,
            drivers: Vec::new(),
            audio: None,
            audio_offset_s: 0.0,
            audio_levels: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Keyframe {
    /// The document at this keyframe (without its animation).
    pub doc: Document,
    /// Keyframe timing: frames from this keyframe to the next.
    pub frames: u32,
    /// Keyframe timing: how the frames to the next keyframe are spread.
    pub easing: Easing,
    /// Recorded timing: seconds from the start of the flight (keyframes from a recording).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_s: Option<f64>,
}

impl Default for Keyframe {
    fn default() -> Self {
        Keyframe { doc: Document::default(), frames: 60, easing: Easing::Linear, time_s: None }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Easing {
    /// Even steps.
    #[default]
    Linear,
    /// Starts and stops slowly.
    Smooth,
    EaseIn,
    EaseOut,
}

impl Easing {
    pub const ALL: [Easing; 4] = [Easing::Linear, Easing::Smooth, Easing::EaseIn, Easing::EaseOut];

    pub fn label(self) -> &'static str {
        match self {
            Easing::Linear => "Even",
            Easing::Smooth => "Slow in and out",
            Easing::EaseIn => "Slow start",
            Easing::EaseOut => "Slow end",
        }
    }

    pub fn apply(self, t: f64) -> f64 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Easing::Linear => t,
            Easing::Smooth => t * t * (3.0 - 2.0 * t),
            Easing::EaseIn => t * t,
            Easing::EaseOut => 1.0 - (1.0 - t) * (1.0 - t),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Timing {
    /// Frame counts and easing per keyframe.
    #[default]
    Keyframes,
    /// One steady perceived speed over the whole length.
    Steady,
    /// The pace of a recorded flight (keyframe times), speed changes smoothed.
    Recorded,
}

impl Timing {
    pub const ALL: [Timing; 3] = [Timing::Keyframes, Timing::Steady, Timing::Recorded];

    pub fn label(self) -> &'static str {
        match self {
            Timing::Keyframes => "Per keyframe",
            Timing::Steady => "Steady",
            Timing::Recorded => "As flown",
        }
    }
}

/// A straight zoom for the zoom sequence: the last keyframe's document (its center fixed) and
/// the log2 radius of every frame, from the first keyframe's radius to the last one's at a
/// steady pace with the animation's ease in and out (frame count of the animation's timing).
pub struct ZoomPath {
    pub doc: Document,
    pub log2_radius: Vec<f64>,
    pub fps: f64,
    pub shutter: f64,
}

impl ZoomPath {
    pub fn new(anim: &Animation) -> Result<ZoomPath, String> {
        let plan = AnimationPlan::new(anim)?;
        let first = &anim.keyframes[0].doc;
        let mut doc = anim.keyframes[anim.keyframes.len() - 1].doc.clone();
        doc.animation = None;
        let (l0, l1) = (first.view.radius.log2(), doc.view.radius.log2());
        let n = plan.frame_count().max(2);
        let len = (n - 1) as f64 / plan.fps();
        let prof = ease_profile(n, len, anim.ease_in_s, anim.ease_out_s, plan.fps());
        Ok(ZoomPath { doc, log2_radius: prof.iter().map(|p| l0 + (l1 - l0) * p).collect(), fps: plan.fps(), shutter: plan.shutter() })
    }

    /// The log2 radius at frame time `t` (between frames for motion blur).
    pub fn log2_radius_at(&self, t: f64) -> f64 {
        let n = self.log2_radius.len();
        let f = (t.floor().max(0.0) as usize).min(n - 2);
        let a = t - f as f64;
        self.log2_radius[f] + (self.log2_radius[f + 1] - self.log2_radius[f]) * a
    }
}

/// Fields blended on a log scale (path suffixes).
const LOG_FIELDS: [&str; 3] = ["camera.distance", "density.orbits", "max_iter"];
/// Angle fields (degrees, path suffixes), unwrapped between keyframes.
const ANGLE_FIELDS: [&str; 6] = ["camera.yaw", "azimuth", "shadow_angle", "streak_angle", "rays_angle", "rotation"];

/// An animation ready to give the document of any frame.
pub struct AnimationPlan {
    keys: Vec<Document>,
    /// The keyframes as JSON (angles unwrapped).
    values: Vec<Value>,
    path: Path,
    /// Keyframe position (0 at the first keyframe, 1 at the second …) of every frame.
    us: Vec<f64>,
    fps: f64,
    palette_speed: f64,
    shutter: f64,
    drivers: Vec<CompiledDriver>,
    levels: Option<AudioLevels>,
    audio_offset_s: f64,
}

impl AnimationPlan {
    pub fn new(anim: &Animation) -> Result<AnimationPlan, String> {
        if anim.keyframes.is_empty() {
            return Err("the animation has no keyframes".into());
        }
        let keys: Vec<Document> = anim
            .keyframes
            .iter()
            .map(|k| {
                let mut d = k.doc.clone();
                d.animation = None;
                d
            })
            .collect();
        let mut values: Vec<Value> = keys.iter().map(|d| serde_json::to_value(d).unwrap_or(Value::Null)).collect();
        unwrap_angles(&mut values);
        let path = Path::new(keys.iter().map(|d| d.view.clone()).collect(), keys.iter().map(cameras).collect());
        let fps = anim.fps.clamp(1.0, 240.0);
        let mut plan = AnimationPlan {
            keys,
            values,
            path,
            us: Vec::new(),
            fps,
            palette_speed: anim.palette_speed,
            shutter: anim.shutter.clamp(0.0, 1.0),
            drivers: compile_drivers(&anim.drivers).0,
            levels: anim.audio_levels.clone(),
            audio_offset_s: anim.audio_offset_s,
        };
        plan.us = plan.frame_params(anim);
        Ok(plan)
    }

    pub fn frame_count(&self) -> usize {
        self.us.len()
    }

    pub fn fps(&self) -> f64 {
        self.fps
    }

    pub fn shutter(&self) -> f64 {
        self.shutter
    }

    /// The frames at the keyframes (rounded).
    pub fn key_frames(&self) -> Vec<usize> {
        (0..self.keys.len()).map(|k| self.us.partition_point(|u| *u < k as f64 - 1e-9).min(self.us.len().saturating_sub(1))).collect()
    }

    /// Keyframe position of frame time `t` (fractional frames: between the neighbouring
    /// frames, continued straight beyond the ends).
    fn u_at(&self, t: f64) -> f64 {
        let n = self.us.len();
        if n < 2 {
            return 0.0;
        }
        let f = (t.floor().max(0.0) as usize).min(n - 2);
        let a = t - f as f64;
        (self.us[f] + (self.us[f + 1] - self.us[f]) * a).clamp(0.0, (self.keys.len() - 1) as f64)
    }

    /// The document of frame time `t` (in frames; fractional for motion blur).
    pub fn doc_at(&self, t: f64) -> Document {
        let u = self.u_at(t);
        let n = self.keys.len();
        let i = (u.floor().max(0.0) as usize).min(n.saturating_sub(2));
        let a = (u - i as f64).clamp(0.0, 1.0);
        let mut doc = if n == 1 || a <= 1e-12 {
            self.keys[i].clone()
        } else if a >= 1.0 - 1e-12 {
            self.keys[i + 1].clone()
        } else {
            // Missing neighbours at the ends (Null) continue the segment straight.
            let v = |k: isize| if (0..n as isize).contains(&k) { &self.values[k as usize] } else { &Value::Null };
            let ii = i as isize;
            let b = blend(v(ii - 1), v(ii), v(ii + 1), v(ii + 2), a, "");
            serde_json::from_value(b).unwrap_or_else(|_| self.keys[i].clone())
        };
        doc.view = self.view_at(u);
        doc.animation = None;
        if !self.drivers.is_empty() {
            apply_drivers(&mut doc, &self.drivers, self.driver_time(t, u));
        }
        doc
    }

    /// The variables of the drivers at frame time `t` (keyframe position `u`).
    fn driver_time(&self, t: f64, u: f64) -> DriverTime {
        let audio = self.levels.as_ref().map_or([0.0; 4], |l| l.at(self.audio_offset_s + t / self.fps));
        DriverTime { frame: t, frames: self.us.len(), fps: self.fps, key: u, audio }
    }

    /// The values of the drivers at up to `points` frames spread over the animation (NaN
    /// where a driver fails), for curves.
    pub fn driver_curves(&self, drivers: &[Driver], points: usize) -> Vec<Vec<f64>> {
        let n = self.us.len();
        let points = points.clamp(2, n.max(2));
        let docs: Vec<Value> = (0..points)
            .map(|k| serde_json::to_value(self.doc_at((k * n.saturating_sub(1)) as f64 / (points - 1) as f64)).unwrap_or(Value::Null))
            .collect();
        drivers.iter().map(|d| docs.iter().map(|v| value_at(v, d.path.trim()).unwrap_or(f64::NAN)).collect()).collect()
    }

    /// Gradient turns of palette rotation at frame time `t`.
    pub fn palette_phase(&self, t: f64) -> f64 {
        self.palette_speed * t / self.fps
    }

    /// The view at keyframe position `u`.
    pub fn view_at(&self, u: f64) -> View {
        self.path.view_at(u)
    }

    /// Keyframe position of every frame for the animation's timing.
    fn frame_params(&self, anim: &Animation) -> Vec<f64> {
        let n = self.keys.len();
        let fps = self.fps;
        if n == 1 {
            let frames = match anim.timing {
                Timing::Steady => (anim.length_s * fps).round().max(1.0) as usize,
                Timing::Keyframes | Timing::Recorded => anim.keyframes[0].frames.max(1) as usize,
            };
            return vec![0.0; frames];
        }
        match anim.timing {
            Timing::Keyframes => {
                let mut us = Vec::new();
                for (i, k) in anim.keyframes.iter().take(n - 1).enumerate() {
                    let f = k.frames.max(1);
                    for j in 0..f {
                        us.push(i as f64 + k.easing.apply(j as f64 / f as f64));
                    }
                }
                us.push((n - 1) as f64);
                us
            }
            Timing::Steady => {
                let (u, s) = self.perceived_length();
                let total = *s.last().unwrap_or(&0.0);
                let frames = (anim.length_s * fps).round().max(2.0) as usize;
                let len = (frames - 1) as f64 / fps;
                let prof = ease_profile(frames, len, anim.ease_in_s, anim.ease_out_s, fps);
                if total <= 1e-12 {
                    // Nothing moves: spread the frames evenly over the keyframes.
                    return prof.iter().map(|p| p * (n - 1) as f64).collect();
                }
                prof.iter().map(|p| invert(&u, &s, p * total)).collect()
            }
            Timing::Recorded => {
                // Key times: recorded, else from the frame counts.
                let mut times: Vec<f64> = Vec::with_capacity(n);
                let mut acc = 0.0;
                for (i, k) in anim.keyframes.iter().enumerate() {
                    let t = k.time_s.unwrap_or(acc);
                    times.push(if i > 0 { t.max(times[i - 1] + 1e-3) } else { t });
                    acc = times[i] + k.frames.max(1) as f64 / fps;
                }
                let t0 = times[0];
                let frames = ((times[n - 1] - t0) * fps).round().max(1.0) as usize + 1;
                // Keyframe position per frame as flown (even within a segment).
                let flown: Vec<f64> = (0..frames)
                    .map(|f| {
                        let t = t0 + f as f64 / fps;
                        let k = times.iter().rposition(|x| *x <= t).unwrap_or(0).min(n - 2);
                        k as f64 + ((t - times[k]) / (times[k + 1] - times[k])).clamp(0.0, 1.0)
                    })
                    .collect();
                let sigma = anim.speed_smoothing_s.max(0.0) * fps;
                let (u, s) = self.perceived_length();
                let total = *s.last().unwrap_or(&0.0);
                if sigma < 0.3 || total <= 1e-12 {
                    return flown;
                }
                // The perceived speed as flown, smoothed (held at the ends), scaled to the path.
                let pos: Vec<f64> = flown.iter().map(|x| lerp_at(&s, x * PATH_STEPS as f64)).collect();
                let speed = gauss(&pos.windows(2).map(|w| w[1] - w[0]).collect::<Vec<_>>(), sigma);
                let sum: f64 = speed.iter().sum();
                let k = if sum > 0.0 { total / sum } else { 0.0 };
                let mut acc = 0.0;
                let mut us = vec![0.0];
                for v in speed {
                    acc += v * k;
                    us.push(invert(&u, &s, acc));
                }
                us
            }
        }
    }

    /// The perceived distance along the keyframes: zoom (factors of two), movement relative to
    /// the view, turning and 3D camera moves (radians, distance factors), sampled finely;
    /// positions and cumulative lengths.
    fn perceived_length(&self) -> (Vec<f64>, Vec<f64>) {
        let n = self.keys.len();
        let mut u = vec![0.0];
        let mut s = vec![0.0];
        let mut prev = self.path.view_at(0.0);
        let mut prev_cams = self.path.cams_at(0.0);
        for k in 0..n - 1 {
            for j in 1..=PATH_STEPS {
                let uu = k as f64 + j as f64 / PATH_STEPS as f64;
                let v = self.path.view_at(uu);
                let cams = self.path.cams_at(uu);
                let zoom = (v.radius.log2() - prev.radius.log2()).abs();
                let o = rel_offset(&prev, &v);
                let turn = angle_diff(v.rotation, prev.rotation).abs().to_radians();
                let ds = zoom + o[0].hypot(o[1]) + turn + cam_diff(&cams, &prev_cams);
                u.push(uu);
                s.push(s.last().unwrap_or(&0.0) + ds);
                prev = v;
                prev_cams = cams;
            }
        }
        (u, s)
    }

    /// The documents of the sub-frame times of frame `frame` for `samples` samples with
    /// motion blur (the frame time alone without it): (time, samples) pairs.
    pub fn exposure(&self, frame: usize, samples: u32) -> Vec<(f64, u32)> {
        let samples = samples.max(1);
        if self.shutter <= 0.0 || self.us.len() < 2 {
            return vec![(frame as f64, samples)];
        }
        // Up to 16 sub-frames spread over the shutter, the samples shared out between them.
        let subs = samples.clamp(1, 16);
        (0..subs)
            .map(|k| {
                let t = frame as f64 + self.shutter * ((k as f64 + 0.5) / subs as f64 - 0.5);
                (t, samples / subs + u32::from(k < samples % subs))
            })
            .collect()
    }
}

/// Steps per keyframe segment when the path is sampled (perceived length, flight errors).
const PATH_STEPS: usize = 64;

/// The camera path through the keyframes: the views (radius on a monotone cubic in log space,
/// the next keyframe's center kept in place, the unwrapped rotation on a monotone cubic) and
/// the 3D cameras (the same curves as the document blend: yaw unwrapped, distance in log).
struct Path {
    views: Vec<View>,
    /// Unwrapped view rotations.
    rotations: Vec<f64>,
    /// The 3D cameras of every keyframe (none when the keyframes differ in layers): yaw
    /// (unwrapped), pitch, ln distance, field of view.
    cams: Vec<Vec<[f64; 4]>>,
}

/// A camera as path channels (yaw, pitch, ln distance, field of view).
fn cam_channels(c: &Camera) -> [f64; 4] {
    [c.yaw, c.pitch, c.distance.max(1e-300).ln(), c.fov]
}

impl Path {
    fn new(views: Vec<View>, cams: Vec<Vec<Camera>>) -> Path {
        let mut rotations: Vec<f64> = views.iter().map(|v| v.rotation).collect();
        for i in 1..rotations.len() {
            rotations[i] = unwrap(rotations[i - 1], rotations[i]);
        }
        let same = cams.iter().all(|c| c.len() == cams[0].len());
        let mut cams: Vec<Vec<[f64; 4]>> =
            if same { cams.iter().map(|c| c.iter().map(cam_channels).collect()).collect() } else { vec![Vec::new(); views.len()] };
        for i in 1..cams.len() {
            let (a, b) = cams.split_at_mut(i);
            for (p, c) in a[i - 1].iter().zip(b[0].iter_mut()) {
                c[0] = unwrap(p[0], c[0]);
            }
        }
        Path { views, rotations, cams }
    }

    /// Segment and position in it of keyframe position `u`.
    fn segment(&self, u: f64) -> (usize, f64) {
        let n = self.views.len();
        let i = (u.floor().max(0.0) as usize).min(n.saturating_sub(2));
        (i, (u - i as f64).clamp(0.0, 1.0))
    }

    /// Monotone cubic of `f` over keyframes on segment `i` at `a`; missing neighbours at the
    /// ends continue the segment straight.
    fn curve(&self, i: usize, a: f64, f: &dyn Fn(usize) -> f64) -> f64 {
        let n = self.views.len();
        let (x1, x2) = (f(i), f(i + 1));
        let x0 = if i > 0 { f(i - 1) } else { 2.0 * x1 - x2 };
        let x3 = if i + 2 < n { f(i + 2) } else { 2.0 * x2 - x1 };
        monotone(x0, x1, x2, x3, a)
    }

    /// Log2 radius, unwrapped rotation and center factor k (center = c₂ + (c₁ − c₂)·k on
    /// segment i) at keyframe position `u`.
    fn parts(&self, u: f64) -> (usize, f64, f64, f64) {
        let (i, a) = self.segment(u);
        let l = self.curve(i, a, &|k| self.views[k].radius.log2());
        let rot = self.curve(i, a, &|k| self.rotations[k]);
        let (l1, l2) = (self.views[i].radius.log2(), self.views[i + 1].radius.log2());
        // The next keyframe's center stays put while zooming toward it; even when the radius
        // does not change.
        let k = if (l2 - l1).abs() > 1e-9 { (pow2_f(l - l2) - 1.0) / (pow2_f(l1 - l2) - 1.0) } else { 1.0 - a };
        (i, l, rot, k)
    }

    fn view_at(&self, u: f64) -> View {
        if self.views.len() == 1 {
            return self.views[0].clone();
        }
        let (i, l, rot, k) = self.parts(u);
        let radius = Fx::from_f64(1.0) * pow2(l);
        let (v1, v2) = (&self.views[i], &self.views[i + 1]);
        let scale = radius * 1e-3;
        // Enough bits for the finest of the two keyframes and the current view.
        let finest = [scale, v1.radius * 1e-3, v2.radius * 1e-3].into_iter().min_by(|a, b| a.log2().total_cmp(&b.log2())).unwrap_or(scale);
        let bits = big::bits_for(finest) + 64;
        let center = std::array::from_fn(|c| {
            let diff = big::parse(&v1.center[c], bits) - big::parse(&v2.center[c], bits);
            big::add(&v2.center[c], big::to_fx(&diff) * k, scale)
        });
        View { center, radius, rotation: rot.rem_euclid(360.0) }
    }

    /// The camera channels at keyframe position `u`.
    fn cams_at(&self, u: f64) -> Vec<[f64; 4]> {
        if self.views.len() == 1 {
            return self.cams[0].clone();
        }
        let (i, a) = self.segment(u);
        (0..self.cams[0].len()).map(|j| std::array::from_fn(|c| self.curve(i, a, &|k| self.cams[k][j][c]))).collect()
    }
}

/// Offset of `to`'s center from `from`'s in units of `from`'s radius (exact digits at depth).
fn rel_offset(from: &View, to: &View) -> [f64; 2] {
    if from.radius.log2() > -40.0 {
        let r = from.radius.to_f64();
        let (a, b) = (from.center_f64(), to.center_f64());
        return [(b[0] - a[0]) / r, (b[1] - a[1]) / r];
    }
    let bits = big::bits_for(from.radius * 1e-6) + 64;
    let d = |c: usize| (big::to_fx(&(big::parse(&to.center[c], bits) - big::parse(&from.center[c], bits))) / from.radius).to_f64();
    [d(0), d(1)]
}

/// The shortest turn from `b` to `a` (degrees, −180..180).
fn angle_diff(a: f64, b: f64) -> f64 {
    (a - b + 180.0).rem_euclid(360.0) - 180.0
}

/// How far apart two sets of camera channels look: turning and field of view in radians plus
/// distance factors (ln).
fn cam_diff(a: &[[f64; 4]], b: &[[f64; 4]]) -> f64 {
    a.iter().zip(b).map(|(a, b)| (angle_diff(a[0], b[0]).abs() + (a[1] - b[1]).abs() + (a[3] - b[3]).abs()).to_radians() + (a[2] - b[2]).abs()).sum()
}

/// `v` at fractional index `x` (linear between the entries).
fn lerp_at(v: &[f64], x: f64) -> f64 {
    let i = (x.floor().max(0.0) as usize).min(v.len().saturating_sub(2));
    let a = (x - i as f64).clamp(0.0, 1.0);
    match v.len() {
        0 => 0.0,
        1 => v[0],
        _ => v[i] + (v[i + 1] - v[i]) * a,
    }
}

/// Gaussian smoothing over `sigma` entries, mirrored at the ends (values held there).
fn gauss(v: &[f64], sigma: f64) -> Vec<f64> {
    let n = v.len() as isize;
    let r = (3.0 * sigma).ceil() as isize;
    let w: Vec<f64> = (-r..=r).map(|d| (-(d * d) as f64 / (2.0 * sigma * sigma)).exp()).collect();
    (0..n)
        .map(|i| {
            let (mut sum, mut ws) = (0.0, 0.0);
            for d in -r..=r {
                let mut j = i + d;
                while j < 0 || j >= n {
                    j = if j < 0 { -j - 1 } else { 2 * n - j - 1 };
                }
                sum += v[j as usize] * w[(d + r) as usize];
                ws += w[(d + r) as usize];
            }
            sum / ws
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Drivers (adapted from RMandelbulb3D)

impl Animation {
    /// Whether a driver reads the sound levels (`audio`, `bass`, `mid`, `treble`).
    pub fn drivers_use_audio(&self) -> bool {
        self.drivers.iter().any(|d| d.enabled && ["audio", "bass", "mid", "treble"].iter().any(|v| uses_name(&d.expression, v)))
    }

    /// The sound track for the movie from frame `from` (`base`: folder of the document file).
    pub fn audio_track(&self, from: usize, base: Option<&FilePath>) -> Option<crate::video::AudioTrack> {
        let p = self.audio.as_ref().filter(|p| !p.as_os_str().is_empty())?;
        let path = match base {
            Some(b) if p.is_relative() => b.join(p),
            _ => p.clone(),
        };
        Some(crate::video::AudioTrack { path, start_s: self.audio_offset_s + from as f64 / self.fps.max(1e-9) })
    }

    /// Analyses the sound track for the drivers when they need it and the stored levels are
    /// missing or belong to another file. Returns whether it analysed.
    pub fn ensure_audio_levels(&mut self, base: Option<&FilePath>, ffmpeg: &FilePath) -> Result<bool, String> {
        if !self.drivers_use_audio() {
            return Ok(false);
        }
        let track = self.audio_track(0, base).ok_or("the drivers use the sound, but the animation has no sound file")?;
        let source = track.path.to_string_lossy().into_owned();
        if self.audio_levels.as_ref().is_some_and(|l| l.source == source) {
            return Ok(false);
        }
        self.audio_levels = Some(crate::audio::analyse(&track.path, ffmpeg)?);
        Ok(true)
    }
}

/// Whether `name` occurs in `expr` as a whole word.
fn uses_name(expr: &str, name: &str) -> bool {
    let b = expr.as_bytes();
    let word = |c: u8| c.is_ascii_alphanumeric() || c == b'_' || c == b'@' || c == b'.';
    expr.match_indices(name).any(|(i, _)| (i == 0 || !word(b[i - 1])) && b.get(i + name.len()).is_none_or(|c| !word(*c)))
}

/// Variables of a driver expression at one moment of the animation.
#[derive(Clone, Copy, Debug)]
pub struct DriverTime {
    /// Time in frames (fractional between frames).
    pub frame: f64,
    pub frames: usize,
    pub fps: f64,
    /// Keyframe position (0 at the first keyframe, 1 at the second …).
    pub key: f64,
    /// Sound levels at this moment: loudness, bass, mid, treble (0 without a sound).
    pub audio: [f64; 4],
}

/// The JSON value at a path (`a.b.0.c`), or None.
fn node_at<'a>(v: &'a Value, path: &str) -> Option<&'a Value> {
    let mut cur = v;
    for part in path.split('.').filter(|p| !p.is_empty()) {
        cur = match cur {
            Value::Object(m) => m.get(part)?,
            Value::Array(a) => a.get(part.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(cur)
}

/// A number at a path of a document's JSON (`a.b.0.c`; booleans 0 or 1), or None.
pub fn value_at(v: &Value, path: &str) -> Option<f64> {
    match node_at(v, path)? {
        Value::Number(n) => n.as_f64(),
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        _ => None,
    }
}

/// Sets the number at a path (objects get missing keys, `like` gives their type; booleans
/// take values above 0.5 as true; integers stay integers). False when the path does not exist.
fn set_value_at(v: &mut Value, path: &str, x: f64, like: Value) -> bool {
    let parts: Vec<&str> = path.split('.').collect();
    let mut cur = v;
    for (k, part) in parts.iter().enumerate() {
        let last = k + 1 == parts.len();
        cur = match cur {
            Value::Object(m) => {
                if last && !m.contains_key(*part) {
                    m.insert((*part).to_string(), like.clone());
                }
                match m.get_mut(*part) {
                    Some(c) => c,
                    None => return false,
                }
            }
            Value::Array(a) => match part.parse::<usize>().ok().and_then(|i| a.get_mut(i)) {
                Some(c) => c,
                None => return false,
            },
            _ => return false,
        };
    }
    *cur = match cur {
        Value::Bool(_) => Value::Bool(x > 0.5),
        Value::Number(n) if n.is_u64() || n.is_i64() => serde_json::json!(x.round() as i64),
        _ => serde_json::json!(x),
    };
    true
}

/// The default of a formula parameter that the document does not list (it is at its
/// default): `<…>.params.<name>` below an object with a `formula` (layers, colorings,
/// transformations).
fn param_default(v: &Value, path: &str) -> Option<f64> {
    param_default_value(v, path).map(|p| p.as_f64())
}

fn param_default_value(v: &Value, path: &str) -> Option<crate::formula::ast::ParamValue> {
    let (owner, name) = path.rsplit_once(".params.")?;
    let o = node_at(v, owner)?;
    let id = o.get("formula")?.as_str()?;
    let params = o.get("params").cloned().and_then(|p| serde_json::from_value(p).ok()).unwrap_or_default();
    let program = crate::formula::library::library().get(id)?.program_with(&params).ok()?;
    let p = program.params.iter().find(|p| p.name == name)?;
    use crate::formula::ast::ParamValue as P;
    match &p.default {
        P::Float(_) | P::Int(_) | P::Bool(_) => Some(p.default.clone()),
        _ => None,
    }
}

/// A driver with its parsed expression.
#[derive(Clone, Debug)]
pub struct CompiledDriver {
    pub path: String,
    pub expr: crate::expr::Expr,
}

/// Parses the enabled drivers; the errors name the driver (`path: message`).
pub fn compile_drivers(drivers: &[Driver]) -> (Vec<CompiledDriver>, Vec<String>) {
    let mut out = Vec::new();
    let mut errors = Vec::new();
    for d in drivers.iter().filter(|d| d.enabled && !d.path.trim().is_empty()) {
        match crate::expr::Expr::parse(&d.expression) {
            Ok(expr) => out.push(CompiledDriver { path: d.path.trim().to_string(), expr }),
            Err(e) => errors.push(format!("{}: {e}", d.path.trim())),
        }
    }
    (out, errors)
}

/// Applies drivers to a document at time `tm`; returns the errors (`path: message`). Values a
/// driver cannot set (missing path, not finite) stay as they are.
pub fn apply_drivers(doc: &mut Document, drivers: &[CompiledDriver], tm: DriverTime) -> Vec<String> {
    if drivers.is_empty() {
        return Vec::new();
    }
    let Ok(mut v) = serde_json::to_value(&*doc) else { return vec!["the document could not be read".into()] };
    let mut errors = Vec::new();
    let fps = tm.fps.max(1e-9);
    for d in drivers {
        let current = value_at(&v, &d.path).or_else(|| param_default(&v, &d.path));
        let vars = |name: &str| match name {
            "t" => Some(tm.frame / fps),
            "frame" => Some(tm.frame),
            "frames" => Some(tm.frames as f64),
            "u" => Some(if tm.frames > 1 { tm.frame / (tm.frames - 1) as f64 } else { 0.0 }),
            "key" => Some(tm.key),
            "fps" => Some(fps),
            "audio" => Some(tm.audio[0]),
            "bass" => Some(tm.audio[1]),
            "mid" => Some(tm.audio[2]),
            "treble" => Some(tm.audio[3]),
            "value" => current,
            _ => None,
        };
        let paths = |p: &str| value_at(&v, p).or_else(|| param_default(&v, p));
        match d.expr.eval(&vars, &paths) {
            Ok(x) if x.is_finite() => {
                let like = if node_at(&v, &d.path).is_some() {
                    Value::Null
                } else {
                    param_default_value(&v, &d.path).and_then(|p| serde_json::to_value(p).ok()).unwrap_or(Value::Null)
                };
                if !set_value_at(&mut v, &d.path, x, like) {
                    errors.push(format!("{}: no such value in the document", d.path));
                }
            }
            Ok(_) => errors.push(format!("{}: the result is not a number", d.path)),
            Err(e) => errors.push(format!("{}: {e}", d.path)),
        }
    }
    match serde_json::from_value::<Document>(v) {
        Ok(d) => *doc = Document { animation: doc.animation.take(), ..d },
        Err(e) => errors.push(format!("a driven value does not fit: {e}")),
    }
    errors
}

/// Numbers of a document a driver can drive (paths) with their current values; formula
/// parameters at their defaults included.
pub fn drivable_values(doc: &Document) -> Vec<(String, f64)> {
    let d = Document { animation: None, ..doc.clone() };
    let Ok(v) = serde_json::to_value(&d) else { return Vec::new() };
    let mut out = Vec::new();
    fn walk(v: &Value, path: &str, out: &mut Vec<(String, f64)>) {
        match v {
            Value::Number(n) => out.push((path.to_string(), n.as_f64().unwrap_or(0.0))),
            Value::Bool(b) => out.push((path.to_string(), if *b { 1.0 } else { 0.0 })),
            Value::Object(m) => {
                // Formula parameters at their defaults (not in the JSON).
                if let (Some(id), Some(params)) = (m.get("formula").and_then(Value::as_str), m.get("params"))
                    && let Some(program) = crate::formula::library::library()
                        .get(id)
                        .and_then(|e| e.program_with(&serde_json::from_value(params.clone()).unwrap_or_default()).ok())
                {
                    use crate::formula::ast::ParamValue as P;
                    for p in &program.params {
                        if matches!(p.default, P::Float(_) | P::Int(_) | P::Bool(_)) && params.get(&p.name).is_none() {
                            out.push((format!("{path}.params.{}", p.name), p.default.as_f64()));
                        }
                    }
                }
                for (k, c) in m {
                    if path.is_empty() && k == "version" {
                        continue;
                    }
                    walk(c, &if path.is_empty() { k.clone() } else { format!("{path}.{k}") }, out);
                }
            }
            Value::Array(a) => {
                for (i, c) in a.iter().enumerate() {
                    walk(c, &format!("{path}.{i}"), out);
                }
            }
            _ => {}
        }
    }
    walk(&v, "", &mut out);
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

// ---------------------------------------------------------------------------------------------
// Flight recording → keyframes

/// The 3D cameras of all layers (terrain and object camera of every layer, groups included,
/// in tree order).
pub fn cameras(doc: &Document) -> Vec<Camera> {
    fn walk(layers: &[Layer], out: &mut Vec<Camera>) {
        for l in layers {
            out.push(l.light.camera.clone());
            out.push(l.object.camera.clone());
            walk(&l.children, out);
        }
    }
    let mut out = Vec::new();
    walk(&doc.layers, &mut out);
    out
}

/// Sets the 3D cameras of all layers (the order of [`cameras`]).
pub fn set_cameras(doc: &mut Document, cams: &[Camera]) {
    fn walk<'a>(layers: &mut [Layer], it: &mut impl Iterator<Item = &'a Camera>) {
        for l in layers {
            if let Some(c) = it.next() {
                l.light.camera = c.clone();
            }
            if let Some(c) = it.next() {
                l.object.camera = c.clone();
            }
            walk(&mut l.children, it);
        }
    }
    walk(&mut doc.layers, &mut cams.iter());
}

/// One moment of a recorded flight: seconds since the start, the view and the 3D cameras.
#[derive(Clone, Debug, PartialEq)]
pub struct FlightSample {
    pub t: f64,
    pub view: View,
    pub cameras: Vec<Camera>,
}

impl FlightSample {
    pub fn of(doc: &Document, t: f64) -> FlightSample {
        FlightSample { t, view: doc.view.clone(), cameras: cameras(doc) }
    }
}

/// A flight as numbers: centers (binary floats with bits for the finest view), log2 radii,
/// unwrapped rotations and camera channels.
struct Channels {
    centers: Vec<[big::Big; 2]>,
    l: Vec<f64>,
    rot: Vec<f64>,
    cams: Vec<Vec<[f64; 4]>>,
}

impl Channels {
    fn new(samples: &[FlightSample]) -> Channels {
        let finest = samples.iter().map(|s| s.view.radius.log2()).fold(f64::INFINITY, f64::min);
        let bits = big::bits_for(Fx::from_f64(1.0) * pow2(finest) * 1e-6) + 64;
        let centers = samples.iter().map(|s| std::array::from_fn(|c| big::parse(&s.view.center[c], bits))).collect();
        let l = samples.iter().map(|s| s.view.radius.log2()).collect();
        let mut rot: Vec<f64> = samples.iter().map(|s| s.view.rotation).collect();
        for i in 1..rot.len() {
            rot[i] = unwrap(rot[i - 1], rot[i]);
        }
        let same = samples.iter().all(|s| s.cameras.len() == samples[0].cameras.len());
        let mut cams: Vec<Vec<[f64; 4]>> =
            samples.iter().map(|s| if same { s.cameras.iter().map(cam_channels).collect() } else { Vec::new() }).collect();
        for i in 1..cams.len() {
            let (a, b) = cams.split_at_mut(i);
            for (p, c) in a[i - 1].iter().zip(b[0].iter_mut()) {
                c[0] = unwrap(p[0], c[0]);
            }
        }
        Channels { centers, l, rot, cams }
    }
}

/// Cameraman smoothing of a recorded flight: zoom, turning, the 3D cameras and the movement
/// are averaged over a window of about `seconds` (Gaussian), so jerks of the mouse and keys
/// become smooth moves. The window narrows toward the ends, which stay where they were.
/// Centers average relative to each moment's view: views much wider than the current one
/// count less (a deep zoom keeps its target).
#[allow(clippy::needless_range_loop)]
pub fn smooth_flight(samples: &[FlightSample], seconds: f64) -> Vec<FlightSample> {
    let n = samples.len();
    if seconds <= 0.0 || n < 3 {
        return samples.to_vec();
    }
    let ch = Channels::new(samples);
    let sigma = seconds / 2.0;
    let (t0, t1) = (samples[0].t, samples[n - 1].t);
    (0..n)
        .map(|i| {
            let s = &samples[i];
            let h = (3.0 * sigma).min(s.t - t0).min(t1 - s.t);
            if h <= 0.0 {
                return s.clone();
            }
            let (mut ws, mut wc) = (0.0, 0.0);
            let (mut l, mut rot) = (0.0, 0.0);
            let mut cams = vec![[0.0; 4]; ch.cams[i].len()];
            let mut off = [Fx::ZERO, Fx::ZERO];
            let lo = samples.partition_point(|x| x.t < s.t - h);
            let hi = samples.partition_point(|x| x.t <= s.t + h);
            for j in lo..hi {
                let dt = samples[j].t - s.t;
                let w = (-dt * dt / (2.0 * sigma * sigma)).exp();
                ws += w;
                l += w * ch.l[j];
                rot += w * ch.rot[j];
                for (c, v) in cams.iter_mut().zip(&ch.cams[j]) {
                    for k in 0..4 {
                        c[k] += w * v[k];
                    }
                }
                let wcj = w * pow2_f(2.0 * (ch.l[i] - ch.l[j]).min(0.0));
                if j != i && wcj > 1e-9 {
                    for c in 0..2 {
                        off[c] = off[c] + big::to_fx(&(ch.centers[j][c].clone() - ch.centers[i][c].clone())) * wcj;
                    }
                }
                wc += wcj;
            }
            let l = l / ws;
            let radius = Fx::from_f64(1.0) * pow2(l);
            let scale = radius * 1e-3;
            let center = std::array::from_fn(|c| big::add(&s.view.center[c], off[c] / wc.max(1e-300), scale));
            let cameras = cams
                .iter()
                .map(|c| Camera { yaw: angle_diff(c[0] / ws, 0.0), pitch: c[1] / ws, distance: (c[2] / ws).exp(), fov: c[3] / ws })
                .collect();
            FlightSample { t: s.t, view: View { center, radius, rotation: (rot / ws).rem_euclid(360.0) }, cameras }
        })
        .collect()
}

/// Keyframes for a recorded flight: as few as needed so that the path through them stays
/// within `tolerance` of the flight (movement relative to the view, zoom in factors of two,
/// turning in radians, 3D camera moves). The keyframes are `base` with the recorded view and
/// cameras, their times those of the flight (for the recorded timing).
#[allow(clippy::needless_range_loop)]
pub fn keyframes_from_flight(samples: &[FlightSample], tolerance: f64, base: &Document, fps: f64) -> Vec<Keyframe> {
    let n = samples.len();
    if n == 0 {
        return Vec::new();
    }
    let ch = Channels::new(samples);
    let mut keys = vec![0usize];
    if n > 1 {
        keys.push(n - 1);
    }
    let tol = tolerance.max(1e-6);
    // Worst error per segment (error, sample); None: to be measured.
    let mut worst: Vec<Option<(f64, usize)>> = vec![None; keys.len().saturating_sub(1)];
    loop {
        let path = Path::new(keys.iter().map(|&k| samples[k].view.clone()).collect(), keys.iter().map(|&k| samples[k].cameras.clone()).collect());
        for seg in 0..worst.len() {
            if worst[seg].is_none() {
                worst[seg] = Some(segment_error(&path, seg, &keys, &ch, samples));
            }
        }
        let (seg, (e, at)) =
            worst.iter().enumerate().map(|(i, w)| (i, w.unwrap_or((0.0, 0)))).max_by(|a, b| a.1.0.total_cmp(&b.1.0)).unwrap_or((0, (0.0, 0)));
        if e <= tol || keys.len() >= 1000 {
            break;
        }
        keys.insert(seg + 1, at);
        // The new keyframe changes its segments and (through the tangents) their neighbours.
        worst.insert(seg + 1, None);
        let end = (seg + 3).min(worst.len());
        for w in worst.iter_mut().take(end).skip(seg.saturating_sub(1)) {
            *w = None;
        }
    }
    let t0 = samples[0].t;
    keys.iter()
        .enumerate()
        .map(|(j, &k)| {
            let s = &samples[k];
            let mut doc = Document { animation: None, view: s.view.clone(), ..base.clone() };
            set_cameras(&mut doc, &s.cameras);
            let next = keys.get(j + 1).map(|&m| samples[m].t).unwrap_or(s.t);
            Keyframe { doc, frames: ((next - s.t) * fps).round().max(1.0) as u32, easing: Easing::Linear, time_s: Some(s.t - t0) }
        })
        .collect()
}

/// The largest distance of a sample between keyframes `seg` and `seg + 1` from the path there
/// (to the nearest of finely spaced path points), and that sample.
#[allow(clippy::needless_range_loop)]
fn segment_error(path: &Path, seg: usize, keys: &[usize], ch: &Channels, samples: &[FlightSample]) -> (f64, usize) {
    let (a, b) = (keys[seg], keys[seg + 1]);
    if b <= a + 1 {
        return (0.0, a);
    }
    let points: Vec<(f64, f64, f64, Vec<[f64; 4]>)> = (0..=PATH_STEPS)
        .map(|j| {
            let u = seg as f64 + j as f64 / PATH_STEPS as f64;
            let (_, l, rot, k) = path.parts(u);
            (l, rot, k, path.cams_at(u))
        })
        .collect();
    // Centers on the segment: c_b + (c_a − c_b)·k; relative to a sample s: (c_b − c_s) + (c_a − c_b)·k.
    let e: [Fx; 2] = std::array::from_fn(|c| big::to_fx(&(ch.centers[a][c].clone() - ch.centers[b][c].clone())));
    let mut worst = (0.0, a);
    for i in a + 1..b {
        let r = samples[i].view.radius;
        let d: [Fx; 2] = std::array::from_fn(|c| big::to_fx(&(ch.centers[b][c].clone() - ch.centers[i][c].clone())));
        let best = points
            .iter()
            .map(|(l, rot, k, cams)| {
                let off = |c: usize| ((d[c] + e[c] * *k) / r).to_f64();
                off(0).hypot(off(1)) + (l - ch.l[i]).abs() + angle_diff(*rot, ch.rot[i]).abs().to_radians() + cam_diff(cams, &ch.cams[i])
            })
            .fold(f64::INFINITY, f64::min);
        if best > worst.0 {
            worst = (best, i);
        }
    }
    worst
}

fn pow2(l: f64) -> Fx {
    let e = l.floor();
    Fx::new(2f64.powf(l - e), e as i32)
}

fn pow2_f(l: f64) -> f64 {
    2f64.powf(l)
}

/// `b` moved by whole turns to lie within half a turn of `a` (degrees).
fn unwrap(a: f64, b: f64) -> f64 {
    a + ((b - a + 180.0).rem_euclid(360.0) - 180.0)
}

/// Unwraps the angle fields of consecutive keyframes so they turn the short way.
fn unwrap_angles(values: &mut [Value]) {
    fn walk(prev: &Value, cur: &mut Value, path: &str) {
        match cur {
            Value::Number(n) => {
                if ANGLE_FIELDS.iter().any(|f| path.ends_with(f))
                    && let (Some(a), Some(b)) = (prev.as_f64(), n.as_f64())
                    && let Some(v) = serde_json::Number::from_f64(unwrap(a, b))
                {
                    *n = v;
                }
            }
            Value::Array(items) => {
                for (k, item) in items.iter_mut().enumerate() {
                    if let Some(p) = prev.as_array().and_then(|a| a.get(k)) {
                        walk(p, item, path);
                    }
                }
            }
            Value::Object(map) => {
                for (key, item) in map.iter_mut() {
                    if let Some(p) = prev.get(key) {
                        let path = if path.is_empty() { key.clone() } else { format!("{path}.{key}") };
                        walk(p, item, &path);
                    }
                }
            }
            _ => {}
        }
    }
    for i in 1..values.len() {
        let (a, b) = values.split_at_mut(i);
        walk(&a[i - 1], &mut b[0], "");
    }
}

/// Monotone cubic (Fritsch–Carlson tangents) from `v1` to `v2` at `t`, neighbours `v0`, `v3`.
fn monotone(v0: f64, v1: f64, v2: f64, v3: f64, t: f64) -> f64 {
    let d1 = v1 - v0;
    let d = v2 - v1;
    let d3 = v3 - v2;
    let tangent = |a: f64, b: f64| if a * b <= 0.0 { 0.0 } else { 2.0 * a * b / (a + b) };
    let (m1, m2) = (tangent(d1, d), tangent(d, d3));
    let (t2, t3) = (t * t, t * t * t);
    v1 * (2.0 * t3 - 3.0 * t2 + 1.0) + m1 * (t3 - 2.0 * t2 + t) + v2 * (-2.0 * t3 + 3.0 * t2) + m2 * (t3 - t2)
}

fn same_shape(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(_), Value::Number(_)) => true,
        (Value::Array(x), Value::Array(y)) => x.len() == y.len(),
        (Value::Object(_), Value::Object(_)) | (Value::String(_), Value::String(_)) => true,
        _ => false,
    }
}

/// Blends `v1` → `v2` at `t` (neighbours `v0`, `v3`); `path` is the dotted field name.
fn blend(v0: &Value, v1: &Value, v2: &Value, v3: &Value, t: f64, path: &str) -> Value {
    if !same_shape(v1, v2) {
        return v1.clone();
    }
    // A neighbour of another shape counts as missing (Null): the segment continues straight.
    let v0 = if same_shape(v0, v1) { v0 } else { &Value::Null };
    let v3 = if same_shape(v3, v2) { v3 } else { &Value::Null };
    match (v1, v2) {
        (Value::Number(a), Value::Number(b)) => {
            let (x1, x2) = (a.as_f64().unwrap_or(0.0), b.as_f64().unwrap_or(0.0));
            let x0 = v0.as_f64().unwrap_or(2.0 * x1 - x2);
            let x3 = v3.as_f64().unwrap_or(2.0 * x2 - x1);
            let log = LOG_FIELDS.iter().any(|f| path.ends_with(f)) && x1 > 0.0 && x2 > 0.0;
            let r = if log {
                let (l1, l2) = (x1.ln(), x2.ln());
                let l0 = if v0.is_null() || x0 <= 0.0 { 2.0 * l1 - l2 } else { x0.ln() };
                let l3 = if v3.is_null() || x3 <= 0.0 { 2.0 * l2 - l1 } else { x3.ln() };
                monotone(l0, l1, l2, l3, t).exp()
            } else {
                monotone(x0, x1, x2, x3, t)
            };
            if a.is_i64() || a.is_u64() {
                let r = r.round();
                if a.is_u64() && b.is_u64() { Value::from(r.max(0.0) as u64) } else { Value::from(r as i64) }
            } else {
                serde_json::Number::from_f64(r).map(Value::Number).unwrap_or_else(|| v1.clone())
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            let el = |v: &Value, k: usize| v.as_array().and_then(|x| x.get(k)).cloned().unwrap_or(Value::Null);
            Value::Array((0..a.len()).map(|k| blend(&el(v0, k), &a[k], &b[k], &el(v3, k), t, path)).collect())
        }
        // Light positions are decimal strings: blended as numbers.
        (Value::String(a), Value::String(b)) if path.ends_with("lights.position") => {
            let (Ok(x1), Ok(x2)) = (a.trim().parse::<f64>(), b.trim().parse::<f64>()) else { return v1.clone() };
            let x0 = v0.as_str().and_then(|s| s.trim().parse().ok()).unwrap_or(2.0 * x1 - x2);
            let x3 = v3.as_str().and_then(|s| s.trim().parse().ok()).unwrap_or(2.0 * x2 - x1);
            Value::String(monotone(x0, x1, x2, x3, t).to_string())
        }
        (Value::Object(a), Value::Object(b)) => {
            let mut out = a.clone();
            for (key, va) in a {
                // The view has its own path; image size and samples come from the render.
                if path.is_empty() && matches!(key.as_str(), "view" | "animation" | "version" | "bookmarks") {
                    continue;
                }
                let Some(vb) = b.get(key) else { continue };
                let p = if path.is_empty() { key.clone() } else { format!("{path}.{key}") };
                let n0 = v0.get(key).unwrap_or(va);
                let n3 = v3.get(key).unwrap_or(vb);
                out.insert(key.clone(), blend(n0, va, vb, n3, t, &p));
            }
            Value::Object(out)
        }
        _ => v1.clone(),
    }
}

/// Position along `u` where the cumulative length `s` reaches `target`.
fn invert(u: &[f64], s: &[f64], target: f64) -> f64 {
    let i = s.partition_point(|x| *x < target);
    if i == 0 {
        return u[0];
    }
    if i >= s.len() {
        return *u.last().unwrap_or(&0.0);
    }
    let a = (target - s[i - 1]) / (s[i] - s[i - 1]).max(1e-300);
    u[i - 1] + (u[i] - u[i - 1]) * a
}

/// Fraction of the way (0..1) per frame for a speed that ramps up over `ein` seconds and down
/// over `eout` seconds (smooth ramps), constant in between.
fn ease_profile(frames: usize, len: f64, ein: f64, eout: f64, fps: f64) -> Vec<f64> {
    let total = len.max(1e-9);
    let (ein, eout) = (ein.clamp(0.0, total), eout.clamp(0.0, total));
    let (ein, eout) = if ein + eout > total { (ein * total / (ein + eout), eout * total / (ein + eout)) } else { (ein, eout) };
    let speed = |t: f64| {
        let up = if ein > 0.0 && t < ein {
            let x = t / ein;
            x * x * (3.0 - 2.0 * x)
        } else {
            1.0
        };
        let down = if eout > 0.0 && t > total - eout {
            let x = (total - t) / eout;
            x * x * (3.0 - 2.0 * x)
        } else {
            1.0
        };
        up.min(down).max(1e-3)
    };
    let steps = (frames - 1).max(1) * 16;
    let dt = total / steps as f64;
    let mut cum = vec![0.0];
    for i in 0..steps {
        let t = (i as f64 + 0.5) * dt;
        cum.push(cum[i] + speed(t) * dt);
    }
    let all = cum[steps].max(1e-300);
    (0..frames).map(|f| cum[((f as f64 / fps / dt).round() as usize).min(steps)] / all).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(center: [&str; 2], radius: f64, rotation: f64, frames: u32) -> Keyframe {
        let doc =
            Document { view: View { center: [center[0].into(), center[1].into()], radius: Fx::from_f64(radius), rotation }, ..Document::default() };
        Keyframe { doc, frames, easing: Easing::Linear, time_s: None }
    }

    #[test]
    fn light_positions_move_between_keyframes() {
        let mut a = Document::default();
        a.lights.push(crate::doc::LightSource { position: ["0".into(), "1".into()], ..Default::default() });
        let mut b = a.clone();
        b.lights[0].position = ["2".into(), "1".into()];
        let (va, vb) = (serde_json::to_value(&a).unwrap(), serde_json::to_value(&b).unwrap());
        let m = blend(&Value::Null, &va, &vb, &Value::Null, 0.5, "");
        let d: Document = serde_json::from_value(m).unwrap();
        assert_eq!(d.lights[0].position, ["1".to_string(), "1".to_string()]);
    }

    #[test]
    fn angles_unwrap_the_short_way() {
        assert_eq!(unwrap(350.0, 10.0), 370.0);
        assert_eq!(unwrap(10.0, 350.0), -10.0);
        assert_eq!(unwrap(0.0, 90.0), 90.0);
    }

    #[test]
    fn keyframes_and_numbers_blend() {
        let mut a = key(["-0.5", "0"], 1.5, 350.0, 10);
        let mut b = key(["-0.5", "0"], 1.5, 10.0, 10);
        a.doc.layers[0].max_iter = 100;
        b.doc.layers[0].max_iter = 400;
        a.doc.layers[0].outside.offset = 0.0;
        b.doc.layers[0].outside.offset = 0.5;
        let anim = Animation { keyframes: vec![a, b], ..Animation::default() };
        let plan = AnimationPlan::new(&anim).unwrap();
        assert_eq!(plan.frame_count(), 11);
        let mid = plan.doc_at(5.0);
        // Halfway: offset in the middle, iterations on the log scale (200), the rotation
        // through 0° (the short way).
        assert!((mid.layers[0].outside.offset - 0.25).abs() < 1e-9, "{}", mid.layers[0].outside.offset);
        assert_eq!(mid.layers[0].max_iter, 200);
        assert!(mid.view.rotation < 1e-6 || mid.view.rotation > 359.999, "{}", mid.view.rotation);
        assert_eq!(plan.doc_at(10.0).layers[0].max_iter, 400);
    }

    #[test]
    fn deep_zoom_keeps_the_target_in_place() {
        // From the whole set into a point 1e-30 wide: the radius halves at an even pace, the
        // target point stays at the same place on the screen (relative to the radius).
        let target = ["-0.7436438870371587047918", "0.1318259042053119898143"];
        let a = key(["-0.5", "0"], 1.5, 0.0, 100);
        let b = key(target, 1e-30, 0.0, 100);
        let plan = AnimationPlan::new(&Animation { keyframes: vec![a, b], ..Animation::default() }).unwrap();
        // The point that stays put lies within the end view's size of the target, so the
        // target's offset (in radii) is const · (1 − r_end / r): constant until the last frames.
        let mut first: Option<(f64, f64)> = None;
        for f in [10.0, 50.0, 90.0, 99.0] {
            let v = plan.doc_at(f).view;
            // Even zoom: log2 radius linear in the frame.
            let want = (1.5f64).log2() + (1e-30f64.log2() - 1.5f64.log2()) * f / 100.0;
            assert!((v.radius.log2() - want).abs() < 1e-6, "frame {f}: {} vs {want}", v.radius.log2());
            let bits = big::bits_for(v.radius * 1e-6) + 64;
            let off = (big::to_fx(&(big::parse(target[0], bits) - big::parse(&v.center[0], bits))) / v.radius).to_f64();
            let shrink = 1.0 - (Fx::from_f64(1e-30) / v.radius).to_f64();
            match first {
                None => first = Some((off, shrink)),
                Some((o, s)) => assert!((off / shrink - o / s).abs() < 1e-9, "frame {f}: {off} vs {}", o / s * shrink),
            }
        }
        // The end is exactly the target.
        assert_eq!(plan.doc_at(100.0).view.center[0], target[0]);
    }

    #[test]
    fn steady_timing_and_motion_blur() {
        let a = key(["-0.5", "0"], 1.5, 0.0, 10);
        let b = key(["-0.5", "0"], 1.5e-6, 0.0, 10);
        let anim = Animation {
            keyframes: vec![a, b],
            timing: Timing::Steady,
            length_s: 2.0,
            ease_in_s: 0.0,
            ease_out_s: 0.0,
            fps: 25.0,
            shutter: 0.5,
            ..Animation::default()
        };
        let plan = AnimationPlan::new(&anim).unwrap();
        assert_eq!(plan.frame_count(), 50);
        // Constant zoom speed: equal log steps.
        let lr = |f: f64| plan.doc_at(f).view.radius.log2();
        let (d1, d2) = (lr(10.0) - lr(9.0), lr(40.0) - lr(39.0));
        assert!((d1 - d2).abs() < 1e-3 * d1.abs(), "{d1} vs {d2}");
        // Motion blur: 8 samples spread over the shutter around the frame.
        let ex = plan.exposure(20, 8);
        assert_eq!(ex.len(), 8);
        assert_eq!(ex.iter().map(|e| e.1).sum::<u32>(), 8);
        assert!(ex[0].0 < 20.0 && ex[7].0 > 20.0 && ex[7].0 - ex[0].0 < 0.5);
    }

    /// A flight: zooming into a deep point while drifting and turning, with jitter.
    fn flight(seconds: f64, depth: f64, jitter: f64) -> Vec<FlightSample> {
        let target = ["-0.7436438870371587047", "0.1318259042053198403"];
        let mut doc = Document::default();
        let mut out = Vec::new();
        let frames = (seconds * 30.0) as usize;
        for f in 0..=frames {
            let t = f as f64 / 30.0;
            let x = t / seconds;
            // Zoom by 2^(depth·x) toward the target, drifting around it at half the radius.
            let l = 0.5 - depth * x * x * (3.0 - 2.0 * x) + jitter * (f as f64 * 1.7).sin();
            let r = Fx::from_f64(1.0) * pow2(l);
            let a = x * 4.0;
            doc.view.center = std::array::from_fn(|c| {
                let o = if c == 0 { a.cos() } else { a.sin() } * 0.5 * (1.0 - x);
                big::add(target[c], r * o, r * 1e-3)
            });
            doc.view.radius = r;
            doc.view.rotation = (t * 40.0 + jitter * 30.0 * (f as f64 * 2.3).sin()).rem_euclid(360.0);
            doc.layers[0].light.camera.yaw = angle_diff(t * 50.0, 0.0);
            out.push(FlightSample::of(&doc, t));
        }
        out
    }

    #[test]
    fn flights_become_few_keyframes_that_follow_them() {
        for depth in [10.0, 100.0] {
            let samples = flight(12.0, depth, 0.0);
            let keys = keyframes_from_flight(&samples, 0.02, &Document::default(), 30.0);
            assert!(keys.len() >= 3 && keys.len() < 100, "{} keyframes", keys.len());
            let anim = Animation { keyframes: keys, timing: Timing::Recorded, speed_smoothing_s: 0.0, fps: 30.0, ..Animation::default() };
            let plan = AnimationPlan::new(&anim).unwrap();
            assert_eq!(plan.frame_count(), samples.len());
            // As flown: every frame is near the recorded moment.
            let mut worst = 0.0f64;
            for (f, s) in samples.iter().enumerate().step_by(7) {
                let v = plan.view_at(plan.us[f]);
                let o = rel_offset(&s.view, &v);
                let e =
                    o[0].hypot(o[1]) + (v.radius.log2() - s.view.radius.log2()).abs() + angle_diff(v.rotation, s.view.rotation).abs().to_radians();
                worst = worst.max(e);
            }
            // Within a segment the pace is even in keyframe position: a fraction of a zoom
            // step off the recorded moment at most.
            assert!(worst < 0.25, "depth {depth}: {worst}");
            // The end is exact.
            let last = plan.doc_at((plan.frame_count() - 1) as f64);
            assert_eq!(last.view, samples.last().unwrap().view);
            let yaw = |d: &Document| d.layers[0].light.camera.yaw;
            assert!(angle_diff(yaw(&plan.doc_at(100.0)), yaw(&Document { view: View::default(), ..Document::default() })).abs() > 1.0);
        }
    }

    #[test]
    fn cameraman_smooths_jerks_and_keeps_the_ends() {
        let rough = flight(8.0, 30.0, 0.05);
        let smooth = smooth_flight(&rough, 0.6);
        assert_eq!(smooth.first(), rough.first());
        assert_eq!(smooth.last(), rough.last());
        let jerk = |s: &[FlightSample]| -> f64 {
            let ch = Channels::new(s);
            ch.l.windows(3).map(|w| (w[0] - 2.0 * w[1] + w[2]).abs()).sum::<f64>()
                + ch.rot.windows(3).map(|w| (w[0] - 2.0 * w[1] + w[2]).abs()).sum::<f64>() / 30.0
        };
        let (a, b) = (jerk(&rough), jerk(&smooth));
        assert!(b < a * 0.1, "jerk {a} → {b}");
        // The deep end keeps its target: the smoothed views stay near the rough ones.
        for (r, s) in rough.iter().zip(&smooth).skip(rough.len() / 2) {
            let o = rel_offset(&r.view, &s.view);
            assert!(o[0].hypot(o[1]) < 0.5, "{o:?} at {}", r.t);
        }
        // Fewer keyframes for the smoothed flight.
        let doc = Document::default();
        let (kr, ks) = (keyframes_from_flight(&rough, 0.05, &doc, 30.0).len(), keyframes_from_flight(&smooth, 0.05, &doc, 30.0).len());
        assert!(ks < kr, "{kr} → {ks}");
    }

    #[test]
    fn recorded_timing_smooths_the_speed() {
        // Two halves at very different speeds.
        let mut s = flight(6.0, 20.0, 0.0);
        for (i, x) in s.iter_mut().enumerate() {
            x.t = if i < 90 { i as f64 / 30.0 } else { 3.0 + (i - 90) as f64 / 10.0 };
        }
        let keys = keyframes_from_flight(&s, 0.02, &Document::default(), 30.0);
        let mut anim = Animation { keyframes: keys, timing: Timing::Recorded, speed_smoothing_s: 0.0, fps: 30.0, ..Animation::default() };
        let speed = |anim: &Animation| -> Vec<f64> {
            let p = AnimationPlan::new(anim).unwrap();
            let (u, len) = p.perceived_length();
            let pos: Vec<f64> = p.us.iter().map(|x| lerp_at(&len, x * PATH_STEPS as f64)).collect();
            assert!((u.last().unwrap() - (p.keys.len() - 1) as f64).abs() < 1e-9);
            pos.windows(2).map(|w| w[1] - w[0]).collect()
        };
        let raw = speed(&anim);
        anim.speed_smoothing_s = 2.0;
        let smooth = speed(&anim);
        assert_eq!(raw.len(), smooth.len());
        let jump = |v: &[f64]| v.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f64::max);
        assert!(jump(&smooth) < jump(&raw) * 0.2, "{} → {}", jump(&raw), jump(&smooth));
        // Duration as flown.
        assert!(((smooth.len()) as f64 / 30.0 - s.last().unwrap().t).abs() < 0.1);
    }

    #[test]
    fn drivers_follow_time_defaults_and_sound() {
        let mut a = Document::for_formula("rfract:Mandelbrot");
        a.layers[0].params.clear();
        let b = a.clone();
        let mut anim = Animation {
            keyframes: vec![Keyframe { doc: a.clone(), frames: 60, ..Keyframe::default() }, Keyframe { doc: b, ..Keyframe::default() }],
            fps: 30.0,
            drivers: vec![
                Driver { path: "view.rotation".into(), expression: "t * 90".into(), ..Driver::default() },
                Driver { path: "layers.0.params.power".into(), expression: "value + 1 + (u > 0.5)".into(), ..Driver::default() },
                Driver { path: "post.exposure".into(), expression: "bass * 2".into(), ..Driver::default() },
                Driver { path: "layers.0.opacity".into(), expression: "@view.rotation / 1000".into(), ..Driver::default() },
            ],
            ..Animation::default()
        };
        assert!(anim.drivers_use_audio());
        // A sound with bass in its second second.
        let rate = 22050.0;
        let samples: Vec<f32> = (0..(rate as usize * 2))
            .map(|i| if i as f64 > rate { (i as f64 / rate * 60.0 * std::f64::consts::TAU).sin() as f32 } else { 0.0 })
            .collect();
        anim.audio_levels = Some(crate::audio::levels_of(&samples, rate));
        let plan = AnimationPlan::new(&anim).unwrap();
        let d = plan.doc_at(30.0);
        assert!((d.view.rotation - 90.0).abs() < 1e-9, "{}", d.view.rotation);
        use crate::formula::ast::ParamValue;
        assert_eq!(d.layers[0].params.get("power"), Some(&ParamValue::Int(3)));
        assert_eq!(plan.doc_at(50.0).layers[0].params.get("power"), Some(&ParamValue::Int(4)));
        assert!((d.layers[0].opacity - 0.09).abs() < 1e-9, "{}", d.layers[0].opacity);
        assert!(plan.doc_at(15.0).post.exposure < 0.05);
        assert!(plan.doc_at(50.0).post.exposure > 0.3, "{}", plan.doc_at(50.0).post.exposure);
        // Errors name the driver.
        let (_, errors) = compile_drivers(&[Driver { path: "x".into(), expression: "sin(".into(), ..Driver::default() }]);
        assert!(errors[0].starts_with("x:"), "{errors:?}");
        let paths: Vec<String> = drivable_values(&a).into_iter().map(|(p, _)| p).collect();
        assert!(paths.contains(&"layers.0.params.power".to_string()) && paths.contains(&"view.rotation".to_string()), "{paths:?}");
        let curves = plan.driver_curves(&anim.drivers, 1000);
        assert_eq!(curves[0].len(), plan.frame_count());
        assert!((curves[0][30] - 90.0).abs() < 1e-9);
        assert_eq!(plan.driver_curves(&anim.drivers, 7)[0].len(), 7);
    }
}
