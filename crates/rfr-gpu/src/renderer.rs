// SPDX-License-Identifier: LGPL-3.0-or-later

//! Progressive GPU renderer: one sample per pixel per pass, split into row bands (one
//! submission each, adaptive height around a target time). Every band runs the plan's
//! operations: layer kernels blend into band-sized scratch slots (groups and masks get slots of
//! their own), compositing passes merge groups and add the finished sample to the image. Shaders
//! compile on worker threads; parameter, view and coloring changes only rewrite buffers.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytemuck::{Pod, Zeroable};
use rfr_core::doc::MaskSource;
use rfr_core::gradient::LUT_SIZE;
use rfr_core::render::histogram::HIST_ROWS;
use rfr_core::render::{Blend, LayerPlan, Node, Plan, Precision};
use wgpu::util::DeviceExt;

use crate::codegen::{self, MAX_PARAMS, Mode};

/// A wgpu device and queue.
#[derive(Clone)]
pub struct GpuContext {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub adapter_name: String,
}

impl GpuContext {
    /// Limits requested from the device: the adapter's buffer sizes (large accumulation buffers
    /// for poster renders), the defaults for everything else.
    pub fn required_limits(adapter: &wgpu::Adapter) -> wgpu::Limits {
        let al = adapter.limits();
        wgpu::Limits {
            max_storage_buffer_binding_size: al.max_storage_buffer_binding_size,
            max_buffer_size: al.max_buffer_size,
            max_texture_dimension_2d: al.max_texture_dimension_2d.min(16384),
            ..wgpu::Limits::default()
        }
    }

    /// A context on the best adapter, without a window (command line, tests).
    pub fn new_headless() -> Result<GpuContext, String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle().with_env());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
            ..Default::default()
        }))
        .map_err(|e| format!("no GPU adapter: {e}"))?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("rfract"),
            required_limits: Self::required_limits(&adapter),
            ..Default::default()
        }))
        .map_err(|e| format!("no GPU device: {e}"))?;
        Ok(GpuContext { device, queue, adapter_name: adapter.get_info().name })
    }
}

/// Layer kernel uniforms (kernel.wgsl `Uni`).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Default)]
struct Uni {
    size: [u32; 4],
    view: [f32; 4],
    rot: [f32; 4],
    iter: [u32; 4],
    seed: [f32; 4],
    outc: [f32; 4],
    inc: [f32; 4],
    out_solid: [f32; 4],
    in_solid: [f32; 4],
    band: [u32; 4],
    lo: [f32; 4],
    blend: [u32; 4],
    opacity: [f32; 4],
    pert: [f32; 4],
    perti: [i32; 4],
    pref: [f32; 4],
    dens: [f32; 4],
    densi: [u32; 4],
    densn: [u32; 4],
    densx: [u32; 4],
    densb: [f32; 4],
    light: [f32; 4],
    lighti: [u32; 4],
    ldir: [[f32; 4]; 4],
    lrad: [[f32; 4]; 4],
    lshadow: [f32; 4],
    lenv: [f32; 4],
    lsky0: [f32; 4],
    lsky1: [f32; 4],
    lcam: [f32; 4],
    obj: [f32; 4],
    obji: [u32; 4],
    objc: [f32; 4],
    fx0: [f32; 4],
    fx1: [f32; 4],
    fx2: [f32; 4],
    fxc: [[f32; 4]; 4],
    lpos: [[f32; 4]; 4],
    lprad: [[f32; 4]; 4],
    win: [i32; 4],
}

/// The lighting a layer's shaders use: a lit layer's or a 3D object's.
fn light_of(lp: &LayerPlan) -> Option<&rfr_core::render::light::LightPlan> {
    lp.light.as_ref().or(lp.object.as_ref().map(|o| &o.light))
}

/// Compositing uniforms (composite.wgsl `Op`).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Default)]
struct OpUni {
    size: [u32; 4],
    slots: [u32; 4],
    mode: [u32; 4],
    color: [f32; 4],
    blend: [f32; 4],
}

/// Display uniforms (display.wgsl `Disp`).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Default)]
struct DispUni {
    size: [u32; 4],
    a: [f32; 4],
    b: [f32; 4],
    wb: [f32; 4],
    shadow: [f32; 4],
    highlight: [f32; 4],
    bloom: [f32; 4],
    lv: [[u32; 4]; 6],
    lw: [[f32; 4]; 6],
    streak: [f32; 4],
    slv: [u32; 4],
    tone: [f32; 4],
    curve_a: [f32; 4],
    curve_b: [f32; 4],
    rays: [f32; 4],
    rays_lv: [u32; 4],
    flare: [f32; 4],
    flare_lv: [u32; 4],
}

/// The display uniforms of `post` for a `w` × `h` image after `samples` samples and `row`
/// rows of the next.
fn disp_uni(post: &rfr_core::doc::Post, lights: bool, w: u32, h: u32, samples: u32, row: u32) -> DispUni {
    let mut u = DispUni { size: [w, h, samples, row], ..Default::default() };
    if post.is_neutral() && !lights {
        return u;
    }
    let wb = rfr_core::render::post::white_balance(post);
    u.a = [2f32.powf(post.exposure as f32), post.contrast as f32, post.saturation as f32, post.tone_map.index() as f32];
    u.b = [post.vignette as f32, post.grain as f32, post.sharpen as f32, post.distortion as f32];
    u.wb = [wb[0], wb[1], wb[2], post.chromatic as f32];
    u.shadow = [post.shadow_color[0], post.shadow_color[1], post.shadow_color[2], post.shadow_amount as f32];
    u.highlight = [post.highlight_color[0], post.highlight_color[1], post.highlight_color[2], post.highlight_amount as f32];
    let (levels, _) = bloom_levels(w, h);
    u.bloom = [if post.bloom > 0.0 { post.bloom as f32 } else { 0.0 }, levels.len() as f32, 0.0, 1.0];
    for (d, l) in u.lv.iter_mut().zip(&levels) {
        *d = *l;
    }
    for (d, wt) in u.lw.iter_mut().zip(rfr_core::render::post::level_weights(post.bloom_size, levels.len())) {
        *d = [wt, 0.0, 0.0, 0.0];
    }
    if post.streak > 0.0 {
        let k = post.streak as f32;
        u.streak = [post.streak_tint[0] * k, post.streak_tint[1] * k, post.streak_tint[2] * k, 1.0];
        u.slv = streak_regions(w, h).0[3];
    }
    let c = post.curve.map(|v| v as f32);
    u.tone = [post.levels_black as f32, post.levels_white as f32, post.levels_gamma as f32, post.spectral as u32 as f32];
    u.curve_a = [c[0], c[1], c[2], c[3]];
    u.curve_b = [c[4], 0.0, 0.0, 0.0];
    let regions = streak_regions(w, h).0;
    if post.rays > 0.0 {
        u.rays = [post.rays as f32, post.rays_position[0] as f32, post.rays_position[1] as f32, post.rays_length as f32];
        u.rays_lv = regions[4];
    }
    if lights {
        u.flare = [1.0, 0.0, 0.0, 0.0];
        u.flare_lv = regions[5];
    }
    u
}

/// The bloom pyramid of a `w` × `h` image: per level its first texel, width and height, and
/// the texels of all levels.
fn bloom_levels(w: u32, h: u32) -> (Vec<[u32; 4]>, u32) {
    let mut off = 0;
    let mut out = Vec::new();
    for (lw, lh) in rfr_core::render::post::level_sizes(w, h) {
        out.push([off, lw, lh, 0]);
        off += lw * lh;
    }
    (out, off)
}

/// Regions after the bloom pyramid: the streak source, two scratch levels, the streak sum and
/// the god rays' bright light (each the size of the first level), then the light sources'
/// image (`post::light_size`); first texel, width, height each, and the texels of pyramid and
/// regions.
fn streak_regions(w: u32, h: u32) -> ([[u32; 4]; 6], u32) {
    let (levels, total) = bloom_levels(w, h);
    let (lw, lh) = (levels[0][1], levels[0][2]);
    let n = lw * lh;
    let (sw, sh) = rfr_core::render::post::light_size(w, h);
    let regions = std::array::from_fn(|k| if k < 5 { [total + k as u32 * n, lw, lh, 0] } else { [total + 5 * n, sw, sh, 0] });
    (regions, total + 5 * n + sw * sh)
}

/// Uniforms of one bloom pass (bloom.wgsl `BP`).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct BloomPass {
    dst: [u32; 4],
    src: [u32; 4],
    img: [u32; 4],
    prm: [f32; 4],
    prm2: [f32; 4],
}

const NONE: u32 = 0xffff_ffff;

fn source_index(s: MaskSource) -> u32 {
    match s {
        MaskSource::Luminance => 0,
        MaskSource::Alpha => 1,
    }
}

/// The scalar mode a layer renders with: f32 while that is precise enough, then
/// double-float, deeper perturbation (when the formula allows it); derivatives where the
/// coloring needs them (distance estimation).
pub fn layer_mode(plan: &Plan, lp: &LayerPlan) -> Mode {
    // 3D objects do not depend on the plane's precision.
    if lp.object.is_some() {
        return Mode::Plain;
    }
    match plan.precision() {
        Precision::Single if lp.needs_derivative() => Mode::Dual,
        Precision::Single => Mode::Plain,
        Precision::Beyond if codegen::perturbation(lp).is_some() => Mode::Pert,
        _ if lp.needs_derivative() => Mode::DualDf64,
        _ => Mode::Df64,
    }
}

/// The mode of the plan's most demanding layer (status display, tests).
pub fn renderer_mode(plan: &Plan) -> Mode {
    let modes: Vec<Mode> = plan.layers().iter().map(|l| layer_mode(plan, l)).collect();
    if modes.contains(&Mode::Pert) {
        Mode::Pert
    } else if modes.contains(&Mode::DualDf64) {
        Mode::DualDf64
    } else if modes.contains(&Mode::Df64) {
        Mode::Df64
    } else if modes.contains(&Mode::Dual) {
        Mode::Dual
    } else {
        Mode::Plain
    }
}

/// Periodicity tolerance floor of f32 orbits (see `rfr_core::render::period_tolerance`).
const PERIOD_FLOOR_F32: f64 = 2.4e-7;

fn layer_uni(plan: &Plan, lp: &LayerPlan) -> Uni {
    let f = &plan.frame;
    let (o, i) = (&lp.outside, &lp.inside);
    // Early inside in the f32 modes (double-float and perturbation views iterate every orbit).
    let single = matches!(layer_mode(plan, lp), Mode::Plain | Mode::Dual);
    let period = if lp.early.periodicity && single { rfr_core::render::period_tolerance(f.pixel_size(), PERIOD_FLOOR_F32) as f32 } else { 0.0 };
    let bulbs = if lp.early.bulbs && single { 1.0 } else { 0.0 };
    Uni {
        // Buffers cover the window; positions refer to the whole image (`win`).
        size: [plan.window.width, plan.window.height, 0, 0],
        win: [plan.window.x as i32, plan.window.y as i32, f.width as i32, f.height as i32],
        view: [f.center[0] as f32, f.center[1] as f32, (f.radius / (f.height as f64 * 0.5)) as f32, plan.magn() as f32],
        rot: [f.cos as f32, f.sin as f32, f.pixel_size() as f32, 1.0],
        iter: [lp.max_iter, lp.julia as u32, o.method_index(), i.method_index()],
        seed: [lp.seed[0] as f32, lp.seed[1] as f32, period, bulbs],
        outc: [o.transfer.index() as f32, o.density as f32, o.offset as f32, !o.hist.is_empty() as u32 as f32],
        inc: [i.transfer.index() as f32, i.density as f32, i.offset as f32, !i.hist.is_empty() as u32 as f32],
        out_solid: o.solid,
        in_solid: i.solid,
        band: [0, 0, 0, NONE],
        lo: [(f.center[0] - f.center[0] as f32 as f64) as f32, (f.center[1] - f.center[1] as f32 as f64) as f32, 0.0, 0.0],
        blend: [0; 4],
        opacity: [1.0, 0.0, 0.0, 0.0],
        pert: [0.0; 4],
        perti: [0; 4],
        pref: [0.0; 4],
        dens: [0.0; 4],
        densi: [0; 4],
        densn: [0; 4],
        densx: [0; 4],
        densb: [0.0; 4],
        light: light_of(lp).map_or([0.0; 4], |l| [l.ambient, l.slope_scale(f.height), l.specular, l.roughness]),
        lighti: light_of(lp).map_or([0; 4], |l| {
            [l.source.index(), l.lights.len() as u32, (l.model == rfr_core::doc::SpecularModel::Ggx) as u32, l.camera.is_some() as u32]
        }),
        ldir: light_rows(lp, |(d, _)| *d),
        lrad: light_rows(lp, |(_, r)| *r),
        // Lit layers: lengths in pixels; 3D objects: shares of the bounding radius.
        lshadow: light_of(lp).map_or([0.0; 4], |l| {
            let h = if lp.object.is_some() { 1.0 } else { f.height as f32 };
            [l.shadow * h, l.softness, l.occlusion, l.occlusion_radius * h]
        }),
        // The environment image follows the layer's image parameters.
        lenv: light_of(lp).map_or([0.0; 4], |l| {
            let slot = if l.environment.is_some() { codegen::layer_images(lp).len() as f32 } else { -1.0 };
            [l.reflection, slot, l.normal_output as u32 as f32, l.points.len() as f32]
        }),
        lpos: {
            let mut rows = point_rows(lp, |(p, _)| *p);
            for (row, r) in rows.iter_mut().zip(light_of(lp).map_or(&[][..], |l| &l.reach[..])) {
                row[3] = *r;
            }
            rows
        },
        lprad: point_rows(lp, |(_, r)| *r),
        lsky0: light_of(lp).map_or([0.0; 4], |l| [l.horizon[0], l.horizon[1], l.horizon[2], 0.0]),
        lsky1: light_of(lp).map_or([0.0; 4], |l| [l.zenith[0], l.zenith[1], l.zenith[2], 0.0]),
        lcam: match &lp.object {
            Some(o) => o.camera,
            None => lp.light.as_ref().and_then(|l| l.camera).unwrap_or([0.0; 4]),
        },
        obj: lp.object.as_ref().map_or([0.0; 4], |o| [o.power, o.scale, o.detail, o.bound()]),
        obji: lp.object.as_ref().map_or([0; 4], |o| [o.shape.index(), o.iterations, o.julia as u32, 0]),
        objc: lp.object.as_ref().map_or([0.0; 4], |o| o.seed),
        // Layer effects in pixels.
        fx0: lp.light.as_ref().and_then(|l| l.fx.as_ref()).map_or([0.0; 4], |e| {
            let h = f.height as f32;
            [e.shadow, e.shadow_offset[0] * h, e.shadow_offset[1] * h, e.shadow_size * h]
        }),
        fx1: lp.light.as_ref().and_then(|l| l.fx.as_ref()).map_or([0.0; 4], |e| {
            let h = f.height as f32;
            [e.glow, e.glow_size * h, e.inner, e.inner_size * h]
        }),
        fx2: lp.light.as_ref().map_or([0.0; 4], |l| {
            let (neon, width) = l.fx.as_ref().map_or((0.0, 1.0), |e| (e.neon, e.neon_width * f.height as f32 / 1000.0));
            [neon, width, l.shading as u32 as f32, l.fx.is_some() as u32 as f32]
        }),
        fxc: lp.light.as_ref().and_then(|l| l.fx.as_ref()).map_or([[0.0; 4]; 4], |e| {
            let c = |v: [f32; 3]| [v[0], v[1], v[2], 0.0];
            [c(e.shadow_color), c(e.glow_color), c(e.inner_color), c(e.neon_color)]
        }),
    }
}

/// Per light one row of the lit layer's uniforms (direction or color).
fn point_rows(lp: &LayerPlan, f: impl Fn(&([f32; 3], [f32; 3])) -> [f32; 3]) -> [[f32; 4]; 4] {
    let mut out = [[0.0; 4]; 4];
    if let Some(l) = light_of(lp) {
        for (row, li) in out.iter_mut().zip(&l.points) {
            let v = f(li);
            *row = [v[0], v[1], v[2], 0.0];
        }
    }
    out
}

fn light_rows(lp: &LayerPlan, f: impl Fn(&([f32; 3], [f32; 3])) -> [f32; 3]) -> [[f32; 4]; 4] {
    let mut out = [[0.0; 4]; 4];
    if let Some(l) = light_of(lp) {
        for (row, li) in out.iter_mut().zip(&l.lights) {
            let v = f(li);
            *row = [v[0], v[1], v[2], 0.0];
        }
    }
    out
}

/// One step of a band.
enum OpKind {
    /// Layer `layer` (index into the plan's layers) blends into `dst`, or writes a mask there.
    Layer {
        layer: usize,
        dst: u32,
        mask: Option<u32>,
        blend: Blend,
        mask_out: Option<(MaskSource, bool)>,
    },
    Fill {
        dst: u32,
        color: [f32; 4],
    },
    Merge {
        src: u32,
        dst: u32,
        mask: Option<u32>,
        blend: Blend,
    },
    Accumulate {
        src: u32,
    },
}

/// Turns the node tree into operations with scratch slots (in the order of `Plan::layers`);
/// returns the number of slots.
fn schedule(plan: &Plan) -> (Vec<OpKind>, u32) {
    struct S {
        ops: Vec<OpKind>,
        next_layer: usize,
        used: u32,
        free: Vec<u32>,
        max: u32,
    }
    impl S {
        fn alloc(&mut self) -> u32 {
            let s = self.free.pop().unwrap_or_else(|| {
                self.used += 1;
                self.used - 1
            });
            self.max = self.max.max(self.used);
            s
        }
        fn mask(&mut self, m: &Option<rfr_core::render::MaskPlan>) -> Option<u32> {
            let m = m.as_ref()?;
            let slot = self.alloc();
            let layer = self.next_layer;
            self.next_layer += 1;
            self.ops.push(OpKind::Layer {
                layer,
                dst: slot,
                mask: None,
                blend: Blend { mode: Default::default(), opacity: 1.0 },
                mask_out: Some((m.source, m.invert)),
            });
            Some(slot)
        }
        fn nodes(&mut self, nodes: &[Node], dst: u32) {
            for n in nodes {
                match n {
                    Node::Layer { blend, mask, .. } => {
                        let m = self.mask(mask);
                        let layer = self.next_layer;
                        self.next_layer += 1;
                        self.ops.push(OpKind::Layer { layer, dst, mask: m, blend: *blend, mask_out: None });
                        if let Some(m) = m {
                            self.free.push(m);
                        }
                    }
                    Node::Group { children, blend, mask } => {
                        let m = self.mask(mask);
                        let g = self.alloc();
                        self.ops.push(OpKind::Fill { dst: g, color: [0.0; 4] });
                        self.nodes(children, g);
                        self.ops.push(OpKind::Merge { src: g, dst, mask: m, blend: *blend });
                        self.free.push(g);
                        if let Some(m) = m {
                            self.free.push(m);
                        }
                    }
                }
            }
        }
    }
    let mut s = S { ops: Vec::new(), next_layer: 0, used: 1, free: Vec::new(), max: 1 };
    s.ops.push(OpKind::Fill { dst: 0, color: plan.background });
    s.nodes(&plan.nodes, 0);
    s.ops.push(OpKind::Accumulate { src: 0 });
    (s.ops, s.max)
}

/// Compiled pipelines shared by all renderers of a device, by shader signature.
type PipelineCache = Arc<Mutex<HashMap<u64, Result<wgpu::ComputePipeline, String>>>>;

/// GPU resources of one layer of the plan.
struct LayerRes {
    signature: u64,
    params: wgpu::Buffer,
    lut: wgpu::Buffer,
    /// The layer's images (`rfr_core::images::pack`) and what they were made from.
    images: wgpu::Buffer,
    image_src: Vec<Option<Arc<rfr_core::images::ImageData>>>,
    /// The reference orbit of a deep zoom (binding 5).
    orbit: wgpu::Buffer,
    /// Linear steps of the reference orbit (binding 6).
    bla: wgpu::Buffer,
    orbit_state: Option<OrbitState>,
    orbit_job: Option<OrbitJob>,
    /// Density layers: the counts (binding 7) and the uniforms of the scatter passes.
    hist: wgpu::Buffer,
    density: Option<DensityRes>,
    /// Lit layers: the field (binding 7, in `hist`) is written by `main` with this uniform.
    light: Option<wgpu::Buffer>,
    uni: Uni,
}

/// GPU resources of a density layer's scatter passes.
struct DensityRes {
    uniform: wgpu::Buffer,
    orbits: u32,
    pixels: u32,
}

/// Cache keys of a density layer's `scatter` and `dmax` pipelines (derived from its signature).
fn scatter_key(sig: u64) -> u64 {
    sig ^ 0x5CA7_7E50_0000_0001
}
fn dmax_key(sig: u64) -> u64 {
    sig ^ 0xD3A5_0000_0000_0002
}
/// Cache keys of a lit layer's `lit` and `lmip` pipelines.
fn lit_key(sig: u64) -> u64 {
    sig ^ 0x7117_0000_0000_0003
}
fn lmip_key(sig: u64) -> u64 {
    sig ^ 0x3A1B_0000_0000_0004
}

/// A reference orbit on the GPU and where it was computed.
struct OrbitState {
    key: u64,
    orbit: Arc<rfr_core::formula::perturb::RefOrbit>,
    /// The reference point (decimal text), its precision and iteration limit.
    point: [String; 2],
    bits: usize,
    maxit: u32,
    buf: wgpu::Buffer,
    /// Linear steps, the largest |δc| they allow and their level count (0: none).
    bla: wgpu::Buffer,
    bla_dc: rfr_core::math::fx::Fx,
    bla_levels: i32,
}

impl OrbitState {
    fn new(
        device: &wgpu::Device,
        key: u64,
        orbit: Arc<rfr_core::formula::perturb::RefOrbit>,
        point: [String; 2],
        bits: usize,
        maxit: u32,
    ) -> OrbitState {
        let data: Vec<[f32; 2]> = orbit.values.iter().map(|c| [c.re as f32, c.im as f32]).collect();
        let buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("reference orbit"),
            contents: bytemuck::cast_slice(&data),
            usage: wgpu::BufferUsages::STORAGE,
        });
        OrbitState { key, orbit, point, bits, maxit, buf, bla: empty_storage(device), bla_dc: rfr_core::math::fx::Fx::ZERO, bla_levels: 0 }
    }
}

/// A reference orbit being computed in the background.
struct OrbitJob {
    key: u64,
    point: [String; 2],
    bits: usize,
    maxit: u32,
    rx: Receiver<Arc<rfr_core::formula::perturb::RefOrbit>>,
    done: Option<Arc<rfr_core::formula::perturb::RefOrbit>>,
}

impl OrbitJob {
    fn start(key: u64, lp: &LayerPlan, pert: rfr_core::formula::perturb::Perturbation, point: [String; 2], bits: usize) -> OrbitJob {
        let (tx, rx) = channel();
        let (program, values, maxit) = (lp.fractal.program.clone(), lp.fractal.values.clone(), lp.max_iter);
        let at = point.clone();
        std::thread::spawn(move || {
            let k = rfr_core::formula::cpu::Compiled::<f64>::new(&program, &values);
            let _ = tx.send(Arc::new(k.reference_orbit(&pert, &at, bits, maxit)));
        });
        OrbitJob { key, point, bits, maxit, rx, done: None }
    }
}

/// The view center minus `point` (decimal text), as floatexp.
fn offset_fx(plan: &Plan, point: &[String; 2], bits: usize) -> [rfr_core::math::fx::Fx; 2] {
    [0, 1].map(|k| {
        let d = rfr_core::math::big::parse(&plan.center_text[k], bits) - rfr_core::math::big::parse(&point[k], bits);
        rfr_core::math::big::to_fx(&d)
    })
}

/// Whether `point` lies within `reach` of the view center (both axes).
fn offset_within(plan: &Plan, point: &[String; 2], reach: rfr_core::math::fx::Fx) -> bool {
    let bits = rfr_core::math::big::bits_for(plan.frame.pixel_size_fx()) + 64;
    offset_fx(plan, point, bits).iter().all(|o| o.abs() <= reach)
}

/// A small storage buffer for bindings a layer does not use.
fn empty_storage(device: &wgpu::Device) -> wgpu::Buffer {
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("empty"), contents: &[0u8; 16], usage: wgpu::BufferUsages::STORAGE })
}

/// The linear steps as the kernel reads them (kernel.wgsl `bla`).
fn pack_bla(t: &rfr_core::formula::perturb::BlaTable) -> Vec<[f32; 4]> {
    let mut out = vec![[0f32; 4]; 8];
    let mut offset = 0u32;
    for (k, level) in t.levels.iter().enumerate().take(32) {
        out[k / 4][k % 4] = f32::from_bits(offset);
        offset += level.len() as u32;
    }
    for level in t.levels.iter().take(32) {
        for b in level {
            out.push(b.m.a.map(|x| x as f32));
            out.push(b.n.a.map(|x| x as f32));
            out.push([f32::from_bits(b.m.e as u32), f32::from_bits(b.n.e as u32), b.r.m as f32, f32::from_bits(b.r.e as u32)]);
        }
    }
    out
}

/// GPU resources of one operation.
struct OpRes {
    kind: OpKind,
    uniform: wgpu::Buffer,
}

struct Target {
    width: u32,
    height: u32,
    /// Premultiplied light, the running mean per pixel.
    accum: wgpu::Buffer,
    /// Adaptive sampling per pixel: samples, Σ s, Σ s², done.
    stats: wgpu::Buffer,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

pub struct GpuRenderer {
    ctx: GpuContext,
    layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    composite_pipeline: wgpu::ComputePipeline,
    display_pipeline: wgpu::ComputePipeline,
    disp_buf: wgpu::Buffer,
    /// Effects of the preview (display.wgsl, bloom.wgsl): the settings, the bloom passes
    /// (bright, down, blur_h, blur_v, streak), the pyramid and its scratch, pass uniforms.
    post: rfr_core::doc::Post,
    /// The light sources' image (`set_lights`): its key and pixels (RGB, 0).
    lights: Option<(String, Vec<[f32; 4]>)>,
    lights_uploaded: bool,
    bloom_pipelines: [wgpu::ComputePipeline; 5],
    bloom_levels: wgpu::Buffer,
    bloom_tmp: wgpu::Buffer,
    bloom_passes: Vec<wgpu::Buffer>,
    cache: PipelineCache,
    pending: Vec<(u64, Receiver<()>)>,
    error: Option<String>,
    layers: Vec<LayerRes>,
    ops: Vec<OpRes>,
    scratch: Option<(wgpu::Buffer, u32)>,
    target: Option<Target>,
    plan_mode: Option<Mode>,
    samples: u32,
    row: u32,
    band_rows: u32,
    band_target: Duration,
    dirty_display: bool,
    generation: u64,
    ready: bool,
    /// Small storage buffers for bindings a layer does not use (read-only ones, and the
    /// read-write counts of density layers: a buffer cannot be both in one dispatch).
    empty: wgpu::Buffer,
    empty_rw: wgpu::Buffer,
    /// The plan last set (installs reference orbits finished in the background).
    last_plan: Option<Plan>,
    /// Passes of the density layers' orbits since the plan was set.
    density_passes: u32,
    /// Adaptive sampling tolerance of the plan (0: off).
    adaptive: f32,
}

/// Most bytes of scratch slots (the band height is limited by it).
const SCRATCH_BUDGET: u64 = 256 << 20;

impl GpuRenderer {
    pub fn new(ctx: GpuContext) -> GpuRenderer {
        let device = &ctx.device;
        let entry = |binding, ty| wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::COMPUTE, ty, count: None };
        let buffer = |ty| wgpu::BindingType::Buffer { ty, has_dynamic_offset: false, min_binding_size: None };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("kernel"),
            entries: &[
                entry(0, buffer(wgpu::BufferBindingType::Uniform)),
                entry(1, buffer(wgpu::BufferBindingType::Uniform)),
                entry(2, buffer(wgpu::BufferBindingType::Storage { read_only: true })),
                entry(3, buffer(wgpu::BufferBindingType::Storage { read_only: false })),
                entry(4, buffer(wgpu::BufferBindingType::Storage { read_only: true })),
                entry(5, buffer(wgpu::BufferBindingType::Storage { read_only: true })),
                entry(6, buffer(wgpu::BufferBindingType::Storage { read_only: true })),
                entry(7, buffer(wgpu::BufferBindingType::Storage { read_only: false })),
                entry(8, buffer(wgpu::BufferBindingType::Storage { read_only: true })),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("kernel"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let module = |label, src: String| {
            device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some(label), source: wgpu::ShaderSource::Wgsl(src.into()) })
        };
        let pipeline = |label, m: &wgpu::ShaderModule| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(label),
                layout: None,
                module: m,
                entry_point: Some("main"),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let composite_pipeline = pipeline("composite", &module("composite", codegen::composite_shader()));
        let display_pipeline = pipeline("display", &module("display", include_str!("wgsl/display.wgsl").to_string()));
        let bloom_module = module("bloom", include_str!("wgsl/bloom.wgsl").to_string());
        let bloom_pipelines = ["bloom_bright", "bloom_down", "bloom_blur_h", "bloom_blur_v", "bloom_streak"].map(|entry| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: None,
                module: &bloom_module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        });
        let storage = |label| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: 16,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            })
        };
        GpuRenderer {
            disp_buf: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("display"),
                size: std::mem::size_of::<DispUni>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            post: rfr_core::doc::Post::default(),
            lights: None,
            lights_uploaded: false,
            bloom_pipelines,
            bloom_levels: storage("bloom levels"),
            bloom_tmp: storage("bloom tmp"),
            bloom_passes: Vec::new(),
            layout,
            pipeline_layout,
            composite_pipeline,
            display_pipeline,
            cache: Arc::new(Mutex::new(HashMap::new())),
            pending: Vec::new(),
            error: None,
            layers: Vec::new(),
            ops: Vec::new(),
            scratch: None,
            target: None,
            plan_mode: None,
            samples: 0,
            row: 0,
            band_rows: 64,
            band_target: Duration::from_millis(12),
            dirty_display: false,
            generation: 0,
            ready: false,
            empty: empty_storage(device),
            empty_rw: empty_storage(device),
            last_plan: None,
            density_passes: 0,
            adaptive: 0.0,
            ctx,
        }
    }

    /// A reference orbit of a deep zoom is being computed in the background (the view renders
    /// with the previous one meanwhile).
    pub fn orbit_pending(&self) -> bool {
        self.layers.iter().any(|l| l.orbit_job.is_some())
    }

    pub fn context(&self) -> &GpuContext {
        &self.ctx
    }
    pub fn samples(&self) -> u32 {
        self.samples
    }
    /// Shader compile error of the current plan.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
    pub fn compiling(&self) -> bool {
        !self.pending.is_empty()
    }
    /// The most demanding scalar mode of the current plan.
    pub fn mode(&self) -> Option<Mode> {
        self.plan_mode
    }
    pub fn output_view(&self) -> Option<&wgpu::TextureView> {
        self.target.as_ref().map(|t| &t.view)
    }
    pub fn output_size(&self) -> Option<(u32, u32)> {
        self.target.as_ref().map(|t| (t.width, t.height))
    }
    /// Changes whenever the output texture got new content.
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn set_band_target(&mut self, t: Duration) {
        self.band_target = t;
    }

    /// Sets what to render. Restarts the accumulation; compiles shaders (in the background) only
    /// for layers whose programs or modes changed.
    pub fn set_plan(&mut self, plan: &Plan) {
        self.last_plan = Some(plan.clone());
        self.density_passes = 0;
        self.adaptive = plan.adaptive;
        let device = self.ctx.device.clone();
        // Buffers too large for the GPU (memory or limits) become a message instead of a panic.
        let invalid = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let oom = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let layers = plan.layers();
        self.plan_mode = Some(renderer_mode(plan));
        self.error = None;
        let mut old = std::mem::take(&mut self.layers).into_iter();
        let mut res = Vec::with_capacity(layers.len());
        for lp in &layers {
            let mode = layer_mode(plan, lp);
            let signature = codegen::signature(lp, mode);
            if codegen::param_programs(lp).iter().map(|p| p.program.params.len()).sum::<usize>() > MAX_PARAMS {
                self.error = Some(format!("{}: too many parameters for one layer", lp.fractal.id));
            }
            let mut prev = old.next();
            // Images: uploaded again only when they change (they can be large).
            let mut image_src: Vec<_> = codegen::layer_images(lp)
                .into_iter()
                .map(|v| match v {
                    rfr_core::formula::ParamValue::Image { path } => rfr_core::images::get(path),
                    _ => None,
                })
                .collect();
            // A lit layer's environment image takes the slot after them.
            if let Some(env) = light_of(lp).and_then(|l| l.environment.clone()) {
                image_src.push(Some(env));
            }
            let same = |a: &[Option<Arc<rfr_core::images::ImageData>>]| {
                a.len() == image_src.len()
                    && a.iter().zip(&image_src).all(|(x, y)| match (x, y) {
                        (Some(x), Some(y)) => Arc::ptr_eq(x, y),
                        (None, None) => true,
                        _ => false,
                    })
            };
            let images = match &prev {
                Some(r) if same(&r.image_src) => r.images.clone(),
                _ => {
                    let words = rfr_core::images::pack(&image_src);
                    let bytes = (words.len() * 4) as u64;
                    let limit = device.limits().max_storage_buffer_binding_size;
                    let words = if bytes > limit {
                        self.error = Some(format!("{}: the images are too large for the graphics card", lp.fractal.id));
                        rfr_core::images::pack(&vec![None; image_src.len()])
                    } else {
                        words
                    };
                    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("images"),
                        contents: bytemuck::cast_slice(&words),
                        usage: wgpu::BufferUsages::STORAGE,
                    })
                }
            };
            // Deep zooms: the reference orbit. It is kept while it stays near the view and
            // precise enough (samples add the center's offset from it); a new one is computed in
            // the background while the old one keeps rendering.
            let mut uni = layer_uni(plan, lp);
            let pert = if mode == Mode::Pert { codegen::perturbation(lp) } else { None };
            let orbit_key = pert.as_ref().map_or(0, |p| {
                let mut h = std::collections::hash_map::DefaultHasher::new();
                use std::hash::{Hash, Hasher};
                format!("{p:?}").hash(&mut h);
                signature.hash(&mut h);
                h.finish()
            });
            let mut state = prev.as_mut().and_then(|r| r.orbit_state.take()).filter(|s| s.key == orbit_key);
            let mut job = prev.as_mut().and_then(|r| r.orbit_job.take()).filter(|j| j.key == orbit_key);
            if let Some(p) = &pert {
                let need_bits = rfr_core::math::big::bits_for(plan.frame.pixel_size_fx());
                if let Some(o) = job.as_mut().and_then(|j| j.done.take()) {
                    let j = job.take().unwrap();
                    state = Some(OrbitState::new(&device, orbit_key, o, j.point, j.bits, j.maxit));
                }
                let near = |point: &[String; 2], bits: usize| bits >= need_bits && offset_within(plan, point, plan.frame.radius_fx * 4.0);
                let st = match state.take() {
                    Some(s) => {
                        let good = near(&s.point, s.bits) && (s.maxit >= lp.max_iter || s.orbit.len <= s.maxit as usize);
                        if good {
                            job = None;
                        } else if !job.as_ref().is_some_and(|j| near(&j.point, j.bits) && j.maxit >= lp.max_iter) {
                            job = Some(OrbitJob::start(orbit_key, lp, p.clone(), plan.center_text.clone(), need_bits + 128));
                        }
                        s
                    }
                    // The first orbit of a layer: computed right away.
                    None => {
                        let k = rfr_core::formula::cpu::Compiled::<f64>::new(&lp.fractal.program, &lp.fractal.values);
                        let o = k.reference_orbit(p, &plan.center_text, need_bits + 128, lp.max_iter);
                        OrbitState::new(&device, orbit_key, Arc::new(o), plan.center_text.clone(), need_bits + 128, lp.max_iter)
                    }
                };
                let off = offset_fx(plan, &st.point, st.bits.max(need_bits) + 64);
                // Linear steps: built for the largest |δc| the reference can still see (the
                // view and its reach), so navigating near it keeps them.
                let mut st = st;
                if p.bla_possible() && !lp.every_iteration() {
                    let need_dc = plan.frame.max_offset_fx() + off[0].abs() + off[1].abs();
                    if st.bla_levels == 0 || need_dc > st.bla_dc {
                        let dc = need_dc * 1.5 + plan.frame.radius_fx * 6.0;
                        match p.bla_table(&st.orbit, dc, rfr_core::formula::perturb::BLA_EPS_GPU) {
                            Some(t) => {
                                st.bla = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                                    label: Some("bla"),
                                    contents: bytemuck::cast_slice(&pack_bla(&t)),
                                    usage: wgpu::BufferUsages::STORAGE,
                                });
                                st.bla_dc = dc;
                                st.bla_levels = t.levels.len() as i32;
                            }
                            None => st.bla_levels = 0,
                        }
                    }
                } else {
                    st.bla_levels = 0;
                }
                let s = plan.frame.radius_fx / (plan.frame.height as f64 * 0.5);
                let off = rfr_core::math::fx::Cfx::from_fx(off[0], off[1]);
                uni.pert = [s.m as f32, off.re as f32, off.im as f32, 0.0];
                uni.perti = [s.e, st.orbit.len as i32, off.e, st.bla_levels];
                uni.pref = [st.orbit.center.re as f32, st.orbit.center.im as f32, 0.0, 0.0];
                state = Some(st);
            }
            let (orbit, bla) = match &state {
                Some(s) => (s.buf.clone(), s.bla.clone()),
                None => (self.empty.clone(), self.empty.clone()),
            };
            // Density layers: fresh counts for every plan (the view decides where points go).
            let (hist, density) = match &lp.density {
                Some(d) => {
                    let channels = rfr_core::render::density::channels(d) as u32;
                    let pixels = plan.frame.width * plan.frame.height;
                    let hist = device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("density counts"),
                        // Metropolis: four words per chain after the counts and maxima.
                        size: ((channels * pixels + 4 + if rfr_core::render::density::metropolis(d) { 4 * d.orbits.max(1) } else { 0 }) as u64) * 4,
                        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    });
                    uni.dens = d.region.map(|v| v as f32);
                    uni.densi = [d.mode.index(), d.min_iter, d.warmup, d.orbits.max(1)];
                    uni.densn = [d.nebula_iter[0], d.nebula_iter[1], d.nebula_iter[2], 0];
                    uni.densx = [channels, rfr_core::render::density::max_iter(lp, d), (channels == 3) as u32, d.ifs.len() as u32];
                    uni.densb = [d.brightness as f32, d.gamma as f32, d.lsystem.width as f32, rfr_core::render::density::metropolis(d) as u32 as f32];
                    let uniform = device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("density"),
                        size: std::mem::size_of::<Uni>() as u64,
                        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    });
                    (hist, Some(DensityRes { uniform, orbits: d.orbits.max(1), pixels }))
                }
                // Lit layers: the field, 16 bytes per pixel.
                None if lp.light.is_some() => (
                    device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("light field"),
                        // 3D terrain: the height maxima after the field, four per 16 bytes.
                        size: (plan.window.pixels() as u64 * 16
                            + if lp.light.as_ref().is_some_and(|l| l.camera.is_some()) {
                                rfr_core::render::light::mip_sizes(plan.window.width, plan.window.height)
                                    .iter()
                                    .map(|(w, h)| *w as u64 * *h as u64)
                                    .sum::<u64>()
                                    .next_multiple_of(4)
                                    * 4
                            } else {
                                0
                            })
                        .max(16),
                        usage: wgpu::BufferUsages::STORAGE,
                        mapped_at_creation: false,
                    }),
                    None,
                ),
                None => (self.empty_rw.clone(), None),
            };
            let light = lp.light.as_ref().map(|_| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("light"),
                    size: std::mem::size_of::<Uni>() as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            });
            // IFS mode: the maps take binding 6 (density layers have no linear steps).
            let bla = match &lp.density {
                Some(d) if d.mode == rfr_core::doc::DensityMode::Ifs => device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("ifs maps"),
                    contents: bytemuck::cast_slice(&rfr_core::render::density::ifs_words(d)),
                    usage: wgpu::BufferUsages::STORAGE,
                }),
                Some(d) if d.mode == rfr_core::doc::DensityMode::LSystem => {
                    let words = rfr_core::render::density::Strokes::new(d).words();
                    uni.densx[3] = (words.len() / 2) as u32;
                    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("l-system strokes"),
                        contents: bytemuck::cast_slice(&words),
                        usage: wgpu::BufferUsages::STORAGE,
                    })
                }
                Some(d) if d.mode == rfr_core::doc::DensityMode::Kleinian => device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("kleinian generators"),
                    contents: bytemuck::cast_slice(&rfr_core::render::density::kleinian_words(d)),
                    usage: wgpu::BufferUsages::STORAGE,
                }),
                _ => bla,
            };
            let (params, lut) = match prev {
                Some(r) => (r.params, r.lut),
                None => (
                    device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("params"),
                        size: (MAX_PARAMS * 16) as u64,
                        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    }),
                    device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("lut"),
                        size: ((2 * LUT_SIZE + 2 * HIST_ROWS) * 16) as u64,
                        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    }),
                ),
            };
            self.ctx.queue.write_buffer(&params, 0, bytemuck::cast_slice(&codegen::param_values(lp)));
            // Both gradients, then both histogram tables (kernel.wgsl `hist_cdf`).
            let mut l: Vec<[f32; 4]> = lp.outside.lut.clone();
            l.extend_from_slice(&lp.inside.lut);
            for t in [&lp.outside.hist, &lp.inside.hist] {
                let mut rows = vec![[0.0f32; 4]; HIST_ROWS];
                for (k, v) in t.iter().enumerate().take(4 * HIST_ROWS) {
                    rows[k / 4][k % 4] = *v;
                }
                l.extend_from_slice(&rows);
            }
            self.ctx.queue.write_buffer(&lut, 0, bytemuck::cast_slice(&l));
            self.request_pipeline(signature, lp, mode);
            res.push(LayerRes {
                signature,
                params,
                lut,
                images,
                image_src,
                orbit,
                bla,
                orbit_state: state,
                orbit_job: job,
                hist,
                density,
                light,
                uni,
            });
        }
        self.layers = res;
        let (ops, slots) = schedule(plan);
        self.ops = ops
            .into_iter()
            .map(|kind| OpRes {
                kind,
                uniform: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("op"),
                    size: std::mem::size_of::<Uni>() as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
            })
            .collect();
        let (w, h) = (plan.window.width, plan.window.height);
        if self.target.as_ref().is_none_or(|t| t.width != w || t.height != h) {
            self.target = Some(self.create_target(w, h));
        }
        // Scratch: `slots` band-sized slots.
        let fit = (SCRATCH_BUDGET / (w as u64 * 16 * slots as u64)).max(8) as u32;
        let max_rows = fit.min(h.next_multiple_of(8)) / 8 * 8;
        let need = w as u64 * max_rows as u64 * 16 * slots as u64;
        if self.scratch.as_ref().is_none_or(|(b, rows)| b.size() < need || *rows != max_rows) {
            let b = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("scratch"),
                size: need.max(16),
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            });
            self.scratch = Some((b, max_rows));
        }
        self.band_rows = self.band_rows.min(max_rows).max(8);
        let failed = pollster::block_on(oom.pop());
        let failed = failed.or(pollster::block_on(invalid.pop()));
        if let Some(e) = failed {
            self.target = None;
            self.scratch = None;
            self.layers.clear();
            self.ops.clear();
            let why = e.to_string();
            let why = why.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string();
            self.error = Some(format!("The image is too large for the GPU: {w} × {h} pixels ({why})"));
            return;
        }
        self.update_ready();
        self.restart();
    }

    /// Starts the accumulation again (same plan).
    pub fn restart(&mut self) {
        self.samples = 0;
        self.row = 0;
        if let Some(t) = &self.target {
            let mut enc = self.ctx.device.create_command_encoder(&Default::default());
            enc.clear_buffer(&t.accum, 0, None);
            enc.clear_buffer(&t.stats, 0, None);
            self.ctx.queue.submit(Some(enc.finish()));
        }
    }

    fn create_target(&self, width: u32, height: u32) -> Target {
        let device = &self.ctx.device;
        let accum = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("accum"),
            size: (width as u64 * height as u64 * 16).max(16),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let stats = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("stats"),
            size: (width as u64 * height as u64 * 16).max(16),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("output"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        Target { width, height, accum, stats, texture, view }
    }

    /// One pass of every density layer's orbits into its counts, then their largest counts
    /// (density.wgsl `scatter`, `dmax`).
    fn density_pass(&self, scratch: &wgpu::Buffer) {
        let device = &self.ctx.device;
        let Some(stats) = self.target.as_ref().map(|t| &t.stats) else { return };
        let mut enc = device.create_command_encoder(&Default::default());
        for l in &self.layers {
            let Some(d) = &l.density else { continue };
            let (Some(Ok(scatter)), Some(Ok(dmax))) = (self.pipeline(scatter_key(l.signature)), self.pipeline(dmax_key(l.signature))) else {
                continue;
            };
            let mut u = l.uni;
            u.densn[3] = self.density_passes;
            self.ctx.queue.write_buffer(&d.uniform, 0, bytemuck::bytes_of(&u));
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("density"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: d.uniform.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: l.params.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 2, resource: l.lut.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 3, resource: scratch.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 4, resource: l.images.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 5, resource: l.orbit.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 6, resource: l.bla.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 7, resource: l.hist.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 8, resource: stats.as_entire_binding() },
                ],
            });
            let mut pass = enc.begin_compute_pass(&Default::default());
            pass.set_bind_group(0, &bind, &[]);
            pass.set_pipeline(&scatter);
            pass.dispatch_workgroups(d.orbits.div_ceil(64), 1, 1);
            pass.set_pipeline(&dmax);
            pass.dispatch_workgroups(d.pixels.div_ceil(256), 1, 1);
        }
        self.ctx.queue.submit(Some(enc.finish()));
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
    }

    /// Lit layers: their fields for the current sample (kernel `main`, operation 2), in bands
    /// of at most `max_rows` rows.
    fn light_pass(&self, scratch: &wgpu::Buffer, max_rows: u32) {
        let device = &self.ctx.device;
        let Some(t) = self.target.as_ref() else { return };
        let (width, height) = (t.width, t.height);
        let stats = &t.stats;
        for l in &self.layers {
            let Some(uniform) = &l.light else { continue };
            let Some(Ok(main)) = self.pipeline(l.signature) else { continue };
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("light field"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: l.params.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 2, resource: l.lut.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 3, resource: scratch.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 4, resource: l.images.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 5, resource: l.orbit.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 6, resource: l.bla.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 7, resource: l.hist.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 8, resource: stats.as_entire_binding() },
                ],
            });
            let mut row = 0;
            while row < height {
                let rows = self.band_rows.max(8).min(max_rows.max(8)).min(height - row);
                let mut u = l.uni;
                u.size[2] = self.samples;
                u.size[3] = row;
                u.band = [rows, 2, 0, NONE];
                self.ctx.queue.write_buffer(uniform, 0, bytemuck::bytes_of(&u));
                let mut enc = device.create_command_encoder(&Default::default());
                {
                    let mut pass = enc.begin_compute_pass(&Default::default());
                    pass.set_pipeline(&main);
                    pass.set_bind_group(0, &bind, &[]);
                    pass.dispatch_workgroups(width.div_ceil(8), rows.div_ceil(8), 1);
                }
                self.ctx.queue.submit(Some(enc.finish()));
                row += rows;
            }
            // 3D terrain: the height maxima, level by level.
            if l.uni.lighti[3] != 0
                && let Some(Ok(lmip)) = self.pipeline(lmip_key(l.signature))
            {
                for (level, (w, h)) in rfr_core::render::light::mip_sizes(width, height).into_iter().enumerate() {
                    let mut u = l.uni;
                    u.band = [0, 3, level as u32, NONE];
                    self.ctx.queue.write_buffer(uniform, 0, bytemuck::bytes_of(&u));
                    let mut enc = device.create_command_encoder(&Default::default());
                    {
                        let mut pass = enc.begin_compute_pass(&Default::default());
                        pass.set_pipeline(&lmip);
                        pass.set_bind_group(0, &bind, &[]);
                        pass.dispatch_workgroups(w.div_ceil(8), h.div_ceil(8), 1);
                    }
                    self.ctx.queue.submit(Some(enc.finish()));
                }
            }
        }
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
    }

    fn request_pipeline(&mut self, sig: u64, lp: &LayerPlan, mode: Mode) {
        if self.cache.lock().ok().is_some_and(|c| c.contains_key(&sig)) || self.pending.iter().any(|(s, _)| *s == sig) {
            return;
        }
        let source = codegen::kernel(lp, mode);
        let density = lp.density.is_some();
        let lit = lp.light.is_some();
        let device = self.ctx.device.clone();
        let layout = self.pipeline_layout.clone();
        let cache = self.cache.clone();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            // Density layers: the scatter and maximum passes come from the same module.
            let mut built = vec![(sig, compile(&device, &layout, &source, "main"))];
            if density {
                built.push((scatter_key(sig), compile(&device, &layout, &source, "scatter")));
                built.push((dmax_key(sig), compile(&device, &layout, &source, "dmax")));
            }
            if lit {
                built.push((lit_key(sig), compile(&device, &layout, &source, "lit")));
                built.push((lmip_key(sig), compile(&device, &layout, &source, "lmip")));
            }
            if let Ok(mut c) = cache.lock() {
                for (k, r) in built {
                    c.insert(k, r);
                }
            }
            let _ = tx.send(());
        });
        self.pending.push((sig, rx));
    }

    fn pipeline(&self, sig: u64) -> Option<Result<wgpu::ComputePipeline, String>> {
        self.cache.lock().ok().and_then(|c| c.get(&sig).cloned())
    }

    /// Checks whether all pipelines of the plan are compiled (records the first error).
    fn update_ready(&mut self) {
        self.pending.retain(|(_, rx)| matches!(rx.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty)));
        let mut ready = true;
        for l in &self.layers {
            let mut keys = vec![l.signature];
            if l.density.is_some() {
                keys.extend([scatter_key(l.signature), dmax_key(l.signature)]);
            }
            if l.light.is_some() {
                keys.extend([lit_key(l.signature), lmip_key(l.signature)]);
            }
            for k in keys {
                match self.pipeline(k) {
                    Some(Ok(_)) => {}
                    Some(Err(e)) => {
                        self.error.get_or_insert(e);
                        ready = false;
                    }
                    None => ready = false,
                }
            }
        }
        self.ready = ready;
    }

    /// Waits until the shaders of the current plan are compiled.
    /// Waits for the reference orbits computed in the background (deep zooms) and takes them:
    /// a finished render (an export, an animation frame) must not use the old orbit of a
    /// nearby view.
    pub fn wait_for_orbits(&mut self) {
        let mut any = false;
        for l in &mut self.layers {
            if let Some(j) = l.orbit_job.as_mut()
                && j.done.is_none()
                && let Ok(o) = j.rx.recv()
            {
                j.done = Some(o);
                any = true;
            }
        }
        if any && let Some(p) = self.last_plan.clone() {
            self.set_plan(&p);
        }
    }

    pub fn wait_for_pipeline(&mut self) -> Result<(), String> {
        for (_, rx) in std::mem::take(&mut self.pending) {
            let _ = rx.recv();
        }
        self.update_ready();
        if let Some(e) = &self.error {
            return Err(e.clone());
        }
        if !self.ready {
            return Err("no shader for the current formula".into());
        }
        Ok(())
    }

    /// Renders bands until `budget` is used up or `max_samples` passes are complete. Returns
    /// whether anything was rendered.
    pub fn work(&mut self, budget: Duration, max_samples: u32) -> bool {
        // A reference orbit finished in the background: set the plan again to use it.
        let mut finished = false;
        for l in &mut self.layers {
            if let Some(j) = l.orbit_job.as_mut()
                && j.done.is_none()
                && let Ok(o) = j.rx.try_recv()
            {
                j.done = Some(o);
                finished = true;
            }
        }
        if finished && let Some(plan) = self.last_plan.clone() {
            self.set_plan(&plan);
        }
        if !self.ready {
            self.update_ready();
        }
        if !self.ready || self.samples >= max_samples || self.error.is_some() {
            return false;
        }
        let (Some(t), Some((scratch, max_rows))) = (self.target.as_ref(), self.scratch.as_ref()) else { return false };
        let (width, height, max_rows) = (t.width, t.height, *max_rows);
        let device = self.ctx.device.clone();
        let stride = width * max_rows;
        // Pipelines and bind groups per operation (valid for the whole call).
        let mut steps = Vec::with_capacity(self.ops.len());
        for op in &self.ops {
            match &op.kind {
                OpKind::Layer { layer, .. } => {
                    let l = &self.layers[*layer];
                    let key = if l.light.is_some() { lit_key(l.signature) } else { l.signature };
                    let Some(Ok(p)) = self.pipeline(key) else { return false };
                    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("layer"),
                        layout: &self.layout,
                        entries: &[
                            wgpu::BindGroupEntry { binding: 0, resource: op.uniform.as_entire_binding() },
                            wgpu::BindGroupEntry { binding: 1, resource: l.params.as_entire_binding() },
                            wgpu::BindGroupEntry { binding: 2, resource: l.lut.as_entire_binding() },
                            wgpu::BindGroupEntry { binding: 3, resource: scratch.as_entire_binding() },
                            wgpu::BindGroupEntry { binding: 4, resource: l.images.as_entire_binding() },
                            wgpu::BindGroupEntry { binding: 5, resource: l.orbit.as_entire_binding() },
                            wgpu::BindGroupEntry { binding: 6, resource: l.bla.as_entire_binding() },
                            wgpu::BindGroupEntry { binding: 7, resource: l.hist.as_entire_binding() },
                            wgpu::BindGroupEntry { binding: 8, resource: t.stats.as_entire_binding() },
                        ],
                    });
                    steps.push((p, bind));
                }
                _ => {
                    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("composite"),
                        layout: &self.composite_pipeline.get_bind_group_layout(0),
                        entries: &[
                            wgpu::BindGroupEntry { binding: 0, resource: op.uniform.as_entire_binding() },
                            wgpu::BindGroupEntry { binding: 1, resource: scratch.as_entire_binding() },
                            wgpu::BindGroupEntry { binding: 2, resource: t.accum.as_entire_binding() },
                            wgpu::BindGroupEntry { binding: 3, resource: t.stats.as_entire_binding() },
                        ],
                    });
                    steps.push((self.composite_pipeline.clone(), bind));
                }
            }
        }
        let start = Instant::now();
        let mut worked = false;
        while self.samples < max_samples {
            if self.row == 0 && self.layers.iter().any(|l| l.density.is_some()) {
                self.density_pass(scratch);
                self.density_passes += 1;
                if rfr_core::render::density::restart_due(self.density_passes, max_samples) {
                    let mut enc = device.create_command_encoder(&Default::default());
                    enc.clear_buffer(&t.accum, 0, None);
                    enc.clear_buffer(&t.stats, 0, None);
                    self.ctx.queue.submit(Some(enc.finish()));
                    self.samples = 0;
                }
            }
            if self.row == 0 && self.layers.iter().any(|l| l.light.is_some()) {
                self.light_pass(scratch, max_rows);
            }
            let rows = self.band_rows.min(height - self.row).min(max_rows);
            for op in &self.ops {
                match &op.kind {
                    OpKind::Layer { layer, dst, mask, blend, mask_out } => {
                        let mut u = self.layers[*layer].uni;
                        u.size[2] = self.samples;
                        u.size[3] = self.row;
                        u.band = [rows, mask_out.is_some() as u32, *dst, mask.unwrap_or(NONE)];
                        let (source, invert) = mask_out.map_or((0, 0), |(s, i)| (source_index(s), i as u32));
                        u.blend = [blend.mode.index(), stride, source, invert];
                        u.opacity = [blend.opacity, 0.0, 0.0, 0.0];
                        self.ctx.queue.write_buffer(&op.uniform, 0, bytemuck::bytes_of(&u));
                    }
                    other => {
                        let mut u = OpUni { size: [width, rows, self.row, stride], ..Default::default() };
                        match other {
                            OpKind::Fill { dst, color } => {
                                u.slots = [0, *dst, 0, NONE];
                                u.color = *color;
                            }
                            OpKind::Merge { src, dst, mask, blend } => {
                                u.slots = [1, *dst, *src, mask.unwrap_or(NONE)];
                                u.mode = [blend.mode.index(), 0, 0, 0];
                                u.blend = [blend.opacity, 0.0, 0.0, 0.0];
                            }
                            OpKind::Accumulate { src } => {
                                u.slots = [2, 0, *src, NONE];
                                u.mode = [0, 0, height, 0];
                                u.blend = [0.0, self.adaptive, 0.0, 0.0];
                            }
                            OpKind::Layer { .. } => unreachable!(),
                        }
                        self.ctx.queue.write_buffer(&op.uniform, 0, bytemuck::bytes_of(&u));
                    }
                }
            }
            let mut enc = device.create_command_encoder(&Default::default());
            {
                let mut pass = enc.begin_compute_pass(&Default::default());
                for (p, bind) in &steps {
                    pass.set_pipeline(p);
                    pass.set_bind_group(0, bind, &[]);
                    pass.dispatch_workgroups(width.div_ceil(8), rows.div_ceil(8), 1);
                }
            }
            let t0 = Instant::now();
            self.ctx.queue.submit(Some(enc.finish()));
            let _ = device.poll(wgpu::PollType::wait_indefinitely());
            let dt = t0.elapsed();
            worked = true;
            let ratio = self.band_target.as_secs_f64() / dt.as_secs_f64().max(1e-5);
            self.band_rows = ((rows as f64 * ratio.clamp(0.5, 2.0)).round() as u32).clamp(8, max_rows.max(8)).next_multiple_of(8).min(max_rows);
            self.row += rows;
            if self.row >= height {
                self.row = 0;
                self.samples += 1;
            }
            if start.elapsed() >= budget {
                break;
            }
        }
        if worked {
            self.dirty_display = true;
        }
        worked
    }

    /// Updates the output texture from the accumulation (after `work`).
    pub fn display(&mut self) {
        if !self.dirty_display {
            return;
        }
        let Some(t) = self.target.as_ref() else { return };
        self.dirty_display = false;
        let (width, height) = (t.width, t.height);
        let u = disp_uni(&self.post, self.lights.is_some(), width, height, self.samples, self.row);
        self.ctx.queue.write_buffer(&self.disp_buf, 0, bytemuck::bytes_of(&u));
        let mut enc = self.ctx.device.create_command_encoder(&Default::default());
        self.bloom_passes(&mut enc, &u);
        let Some(t) = self.target.as_ref() else { return };
        let bind = self.ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("display"),
            layout: &self.display_pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.disp_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: t.accum.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&t.view) },
                wgpu::BindGroupEntry { binding: 3, resource: self.bloom_levels.as_entire_binding() },
            ],
        });
        {
            let mut pass = enc.begin_compute_pass(&Default::default());
            pass.set_pipeline(&self.display_pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(width.div_ceil(8), height.div_ceil(8), 1);
        }
        self.ctx.queue.submit(Some(enc.finish()));
        self.generation += 1;
    }

    /// The document's light sources (glows, lens flares) over the preview: their image is
    /// computed on the CPU when the lights, the view or the size change (`rfr_core::flare`).
    pub fn set_lights(&mut self, doc: &rfr_core::doc::Document) {
        let Some(t) = self.target.as_ref() else { return };
        let (w, h) = (t.width, t.height);
        if !rfr_core::flare::any(doc) {
            if self.lights.take().is_some() {
                self.dirty_display = true;
            }
            return;
        }
        // The layers count for 3D terrains (the camera places the lights).
        let key = format!("{:?}|{:?}|{:?}|{w}x{h}", doc.lights, doc.view, doc.layers);
        if self.lights.as_ref().is_some_and(|(k, _)| *k == key) {
            return;
        }
        let (lw, lh) = rfr_core::render::post::light_size(w, h);
        let px = rfr_core::flare::image(doc, lw, lh).unwrap_or_else(|| vec![[0.0; 3]; (lw * lh) as usize]);
        self.lights = Some((key, px.iter().map(|c| [c[0], c[1], c[2], 0.0]).collect()));
        self.lights_uploaded = false;
        self.dirty_display = true;
    }

    /// The effects of the preview (`rfr_core::render::post`); a change only shows the image
    /// again, it does not render anew.
    pub fn set_post(&mut self, post: &rfr_core::doc::Post) {
        if self.post != *post {
            self.post = post.clone();
            self.dirty_display = true;
        }
    }

    /// Records the bloom pyramid and streak passes (bloom.wgsl) when the effects have them.
    fn bloom_passes(&mut self, enc: &mut wgpu::CommandEncoder, u: &DispUni) {
        let (bloom_on, streak_on) = (u.bloom[0] > 0.0 && u.bloom[3] > 0.5, u.streak[3] > 0.5);
        let (rays_on, lights_on) = (u.rays[0] > 0.0, u.flare[0] > 0.0);
        if !bloom_on && !streak_on && !rays_on && !lights_on {
            return;
        }
        let Some(t) = self.target.as_ref() else { return };
        let device = &self.ctx.device;
        let (width, height) = (t.width, t.height);
        let (levels, _) = bloom_levels(width, height);
        let (regions, texels) = streak_regions(width, height);
        let bytes = texels as u64 * 16;
        if self.bloom_levels.size() < bytes {
            let make = |label| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(label),
                    size: bytes.next_power_of_two(),
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            };
            self.bloom_levels = make("bloom levels");
            self.bloom_tmp = make("bloom tmp");
            self.lights_uploaded = false;
        }
        // The light sources' image (computed on the CPU, see `set_lights`).
        if let Some((_, px)) = &self.lights
            && !self.lights_uploaded
        {
            self.ctx.queue.write_buffer(&self.bloom_levels, regions[5][0] as u64 * 16, bytemuck::cast_slice(px));
            self.lights_uploaded = true;
        }
        let post = &self.post;
        let img = [width, height, self.samples, self.row];
        let exposure = 2f32.powf(post.exposure as f32);
        let prm = [exposure, post.bloom_threshold as f32, 0.0, 0.0];
        let prm2 = [0.0; 4];
        // Passes: bright into level 0, down into each further level, then blur h + v per level.
        let mut passes: Vec<(usize, BloomPass)> = Vec::new();
        if bloom_on {
            for (k, l) in levels.iter().enumerate() {
                let src = if k == 0 { [0; 4] } else { levels[k - 1] };
                passes.push((if k == 0 { 0 } else { 1 }, BloomPass { dst: *l, src, img, prm, prm2 }));
            }
            for l in &levels {
                passes.push((2, BloomPass { dst: *l, src: *l, img, prm, prm2 }));
                passes.push((3, BloomPass { dst: *l, src: *l, img, prm, prm2 }));
            }
        }
        if streak_on {
            // Bright part into the source region, then per direction three passes through the
            // scratch regions into the sum (the first direction writes, the others add).
            let [src, t1, t2, sum, _, _] = regions;
            passes.push((0, BloomPass { dst: src, src: [0; 4], img, prm: [exposure, post.streak_threshold as f32, 0.0, 0.0], prm2 }));
            let (step0, a) = rfr_core::render::post::streak_params(post.streak_length, src[1]);
            for (i, dir) in rfr_core::render::post::streak_dirs(post.streak_angle, post.streak_count).into_iter().enumerate() {
                for (p, (from, to)) in [(src, t1), (t1, t2), (t2, sum)].into_iter().enumerate() {
                    let add = p == 2 && i > 0;
                    let step = step0 * 4f32.powi(p as i32);
                    passes.push((
                        4,
                        BloomPass {
                            dst: to,
                            src: from,
                            img,
                            prm: [exposure, 0.0, if add { 1.0 } else { 0.0 }, 0.0],
                            prm2: [dir[0], dir[1], step, a],
                        },
                    ));
                }
            }
        }
        // God rays: their bright light with its threshold.
        if rays_on {
            passes.push((0, BloomPass { dst: regions[4], src: [0; 4], img, prm: [exposure, post.rays_threshold as f32, 0.0, 0.0], prm2 }));
        }
        while self.bloom_passes.len() < passes.len() {
            self.bloom_passes.push(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("bloom pass"),
                size: std::mem::size_of::<BloomPass>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        for (i, (kind, bp)) in passes.iter().enumerate() {
            self.ctx.queue.write_buffer(&self.bloom_passes[i], 0, bytemuck::bytes_of(bp));
            let pipe = &self.bloom_pipelines[*kind];
            let mut entries = vec![wgpu::BindGroupEntry { binding: 0, resource: self.bloom_passes[i].as_entire_binding() }];
            if *kind == 0 {
                entries.push(wgpu::BindGroupEntry { binding: 1, resource: t.accum.as_entire_binding() });
            }
            entries.push(wgpu::BindGroupEntry { binding: 3, resource: self.bloom_levels.as_entire_binding() });
            if *kind == 2 || *kind == 3 {
                entries.push(wgpu::BindGroupEntry { binding: 4, resource: self.bloom_tmp.as_entire_binding() });
            }
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("bloom"),
                layout: &pipe.get_bind_group_layout(0),
                entries: &entries,
            });
            let mut pass = enc.begin_compute_pass(&Default::default());
            pass.set_pipeline(pipe);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(bp.dst[1].div_ceil(8), bp.dst[2].div_ceil(8), 1);
        }
    }

    /// The preview as shown (display texture, 8-bit sRGB RGBA, opaque): for tests.
    pub fn read_display(&self) -> Option<Vec<u8>> {
        let t = self.target.as_ref()?;
        let row = (t.width * 4).next_multiple_of(256);
        let staging = self.ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("display readback"),
            size: row as u64 * t.height as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut enc = self.ctx.device.create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: &t.texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo {
                buffer: &staging,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(t.height) },
            },
            wgpu::Extent3d { width: t.width, height: t.height, depth_or_array_layers: 1 },
        );
        self.ctx.queue.submit(Some(enc.finish()));
        let slice = staging.slice(..);
        let (tx, rx) = channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = self.ctx.device.poll(wgpu::PollType::wait_indefinitely());
        rx.recv().ok()?.ok()?;
        let data = slice.get_mapped_range().ok()?;
        let mut out = Vec::with_capacity((t.width * t.height * 4) as usize);
        for y in 0..t.height as usize {
            out.extend_from_slice(&data[y * row as usize..y * row as usize + t.width as usize * 4]);
        }
        drop(data);
        staging.unmap();
        Some(out)
    }

    /// Averaged linear RGB with straight alpha per pixel (reads the accumulation back).
    pub fn read_linear(&self) -> Option<Vec<[f32; 4]>> {
        let t = self.target.as_ref()?;
        let size = t.width as u64 * t.height as u64 * 16;
        let staging = self.ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut enc = self.ctx.device.create_command_encoder(&Default::default());
        enc.copy_buffer_to_buffer(&t.accum, 0, &staging, 0, size);
        self.ctx.queue.submit(Some(enc.finish()));
        let slice = staging.slice(..);
        let (tx, rx) = channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = self.ctx.device.poll(wgpu::PollType::wait_indefinitely());
        rx.recv().ok()?.ok()?;
        let data = slice.get_mapped_range().ok()?;
        let px: &[[f32; 4]] = bytemuck::cast_slice(&data[..]);
        let out = px.iter().map(|a| rfr_core::render::cpu::unpremultiply(*a, 1.0)).collect();
        drop(data);
        staging.unmap();
        Some(out)
    }

    /// The image as 8-bit sRGB RGBA with straight alpha (same encoding as the CPU renderer).
    pub fn read_rgba8(&self) -> Option<Vec<u8>> {
        Some(rfr_core::render::cpu::rgba8(&self.read_linear()?))
    }

    /// The output texture (for copies by the caller).
    pub fn output_texture(&self) -> Option<&wgpu::Texture> {
        self.target.as_ref().map(|t| &t.texture)
    }
}

fn compile(device: &wgpu::Device, layout: &wgpu::PipelineLayout, source: &str, entry: &str) -> Result<wgpu::ComputePipeline, String> {
    // Validate first: naga's message is far more useful than a device error.
    let module = naga::front::wgsl::parse_str(source).map_err(|e| format!("shader: {}", e.emit_to_string(source)))?;
    naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty())
        .validate(&module)
        .map_err(|e| format!("shader: {}", e.emit_to_string(source)))?;
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("kernel"), source: wgpu::ShaderSource::Wgsl(source.into()) });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("kernel"),
        layout: Some(layout),
        module: &module,
        entry_point: Some(entry),
        compilation_options: Default::default(),
        cache: None,
    });
    if let Some(e) = pollster::block_on(scope.pop()) {
        return Err(format!("shader: {e}"));
    }
    Ok(pipeline)
}

/// Renders `samples` passes of `plan` and returns linear RGB with straight alpha per pixel.
pub fn render_blocking(ctx: GpuContext, plan: &Plan, samples: u32, mut progress: impl FnMut(f32) -> bool) -> Result<Vec<[f32; 4]>, String> {
    let mut r = GpuRenderer::new(ctx);
    r.set_band_target(Duration::from_millis(40));
    r.set_plan(plan);
    r.wait_for_pipeline()?;
    while r.samples() < samples {
        r.work(Duration::from_millis(200), samples);
        if let Some(e) = r.error() {
            return Err(e.to_string());
        }
        if !progress(r.samples() as f32 / samples as f32) {
            return Err("cancelled".into());
        }
    }
    r.read_linear().ok_or_else(|| "could not read the image back from the GPU".into())
}

/// The denoiser's helper images of `doc` at `width` × `height` (`rfr_core::render::post::
/// aux_documents`), rendered with at most 8 samples on the GPU of `ctx`, else on the CPU. None
/// when the document has no lit layers or objects, or a render failed.
pub fn render_aux(
    doc: &rfr_core::doc::Document,
    width: u32,
    height: u32,
    samples: u32,
    ctx: Option<&GpuContext>,
) -> Option<rfr_core::render::post::Aux> {
    let docs = rfr_core::render::post::aux_documents(doc)?;
    let s = samples.clamp(1, 8);
    let render = |d: &rfr_core::doc::Document| -> Option<Vec<[f32; 4]>> {
        match ctx {
            Some(ctx) => render_blocking(ctx.clone(), &Plan::sized(d, width, height).ok()?, s, |_| true).ok(),
            None => Some(rfr_core::render::cpu::render(d, width, height, s).ok()?.linear()),
        }
    };
    Some((render(&docs[0])?, render(&docs[1])?))
}
