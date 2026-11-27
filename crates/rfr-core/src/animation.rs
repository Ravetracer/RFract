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

use crate::doc::{Document, View};
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
}

impl Default for Keyframe {
    fn default() -> Self {
        Keyframe { doc: Document::default(), frames: 60, easing: Easing::Linear }
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
}

impl Timing {
    pub const ALL: [Timing; 2] = [Timing::Keyframes, Timing::Steady];

    pub fn label(self) -> &'static str {
        match self {
            Timing::Keyframes => "Per keyframe",
            Timing::Steady => "Steady",
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
    views: Vec<View>,
    /// Unwrapped view rotations.
    rotations: Vec<f64>,
    /// Keyframe position (0 at the first keyframe, 1 at the second …) of every frame.
    us: Vec<f64>,
    fps: f64,
    palette_speed: f64,
    shutter: f64,
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
        let views: Vec<View> = keys.iter().map(|d| d.view.clone()).collect();
        let mut rotations: Vec<f64> = views.iter().map(|v| v.rotation).collect();
        for i in 1..rotations.len() {
            rotations[i] = unwrap(rotations[i - 1], rotations[i]);
        }
        let fps = anim.fps.clamp(1.0, 240.0);
        let mut plan = AnimationPlan {
            keys,
            values,
            views,
            rotations,
            us: Vec::new(),
            fps,
            palette_speed: anim.palette_speed,
            shutter: anim.shutter.clamp(0.0, 1.0),
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
        doc
    }

    /// Gradient turns of palette rotation at frame time `t`.
    pub fn palette_phase(&self, t: f64) -> f64 {
        self.palette_speed * t / self.fps
    }

    /// The view at keyframe position `u`.
    pub fn view_at(&self, u: f64) -> View {
        let n = self.views.len();
        let i = (u.floor().max(0.0) as usize).min(n.saturating_sub(2));
        let a = (u - i as f64).clamp(0.0, 1.0);
        if n == 1 {
            return self.views[0].clone();
        }
        // Neighbours of the segment; missing ones at the ends continue it straight.
        let four = |f: &dyn Fn(usize) -> f64| {
            let (x1, x2) = (f(i), f(i + 1));
            let x0 = if i > 0 { f(i - 1) } else { 2.0 * x1 - x2 };
            let x3 = if i + 2 < n { f(i + 2) } else { 2.0 * x2 - x1 };
            (x0, x1, x2, x3)
        };
        let (l0, l1, l2, l3) = four(&|k| self.views[k].radius.log2());
        let l = monotone(l0, l1, l2, l3, a);
        let radius = Fx::from_f64(1.0) * pow2(l);
        let (r0, r1, r2, r3) = four(&|k| self.rotations[k]);
        let rotation = monotone(r0, r1, r2, r3, a).rem_euclid(360.0);
        // Center: the next keyframe's center stays put while zooming toward it; even when the
        // radius does not change.
        let k = if (l2 - l1).abs() > 1e-9 { (pow2_f(l - l2) - 1.0) / (pow2_f(l1 - l2) - 1.0) } else { 1.0 - a };
        let (v1, v2) = (&self.views[i], &self.views[i + 1]);
        let scale = radius * 1e-3;
        // Enough bits for the finest of the two keyframes and the current view.
        let finest = [scale, v1.radius * 1e-3, v2.radius * 1e-3].into_iter().min_by(|a, b| a.log2().total_cmp(&b.log2())).unwrap_or(scale);
        let bits = big::bits_for(finest) + 64;
        let center = std::array::from_fn(|c| {
            let diff = big::parse(&v1.center[c], bits) - big::parse(&v2.center[c], bits);
            big::add(&v2.center[c], big::to_fx(&diff) * k, scale)
        });
        View { center, radius, rotation }
    }

    /// Keyframe position of every frame for the animation's timing.
    fn frame_params(&self, anim: &Animation) -> Vec<f64> {
        let n = self.keys.len();
        let fps = self.fps;
        if n == 1 {
            let frames = match anim.timing {
                Timing::Steady => (anim.length_s * fps).round().max(1.0) as usize,
                Timing::Keyframes => anim.keyframes[0].frames.max(1) as usize,
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
        }
    }

    /// The perceived distance along the keyframes: zoom (factors of two), movement relative to
    /// the view and turning (radians), sampled finely; positions and cumulative lengths.
    fn perceived_length(&self) -> (Vec<f64>, Vec<f64>) {
        const STEPS: usize = 64;
        let n = self.keys.len();
        let mut u = vec![0.0];
        let mut s = vec![0.0];
        let mut prev = self.view_at(0.0);
        for k in 0..n - 1 {
            for j in 1..=STEPS {
                let uu = k as f64 + j as f64 / STEPS as f64;
                let v = self.view_at(uu);
                let zoom = (v.radius.log2() - prev.radius.log2()).abs();
                let r = prev.radius.to_f64().max(1e-300);
                let (c0, c1) = (prev.center_f64(), v.center_f64());
                // Deep views: the difference in f64 is not exact, but relative to the radius it
                // only needs a few digits; compute it on the text there.
                let pan = if r < 1e-12 {
                    let bits = big::bits_for(prev.radius * 1e-6) + 64;
                    let d = |c: usize| big::to_fx(&(big::parse(&v.center[c], bits) - big::parse(&prev.center[c], bits))).to_f64();
                    d(0).hypot(d(1)) / r
                } else {
                    (c1[0] - c0[0]).hypot(c1[1] - c0[1]) / r
                };
                let turn = (v.rotation - prev.rotation + 540.0).rem_euclid(360.0) - 180.0;
                let ds = zoom + pan + turn.abs().to_radians();
                u.push(uu);
                s.push(s.last().unwrap_or(&0.0) + ds);
                prev = v;
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
        (Value::Object(_), Value::Object(_)) => true,
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
        (Value::Object(a), Value::Object(b)) => {
            let mut out = a.clone();
            for (key, va) in a {
                // The view has its own path; image size and samples come from the render.
                if path.is_empty() && matches!(key.as_str(), "view" | "animation" | "version") {
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
        Keyframe { doc, frames, easing: Easing::Linear }
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
}
