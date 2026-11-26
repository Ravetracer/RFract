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

const NONE: u32 = 0xffff_ffff;

fn source_index(s: MaskSource) -> u32 {
    match s {
        MaskSource::Luminance => 0,
        MaskSource::Alpha => 1,
    }
}

/// The scalar mode a layer renders with: f32 (with derivatives when its coloring needs them)
/// while that is precise enough, else double-float (distance estimation then falls back to the
/// smooth iteration count).
pub fn layer_mode(plan: &Plan, lp: &LayerPlan) -> Mode {
    match plan.precision() {
        Precision::Single if lp.needs_derivative() => Mode::Dual,
        Precision::Single => Mode::Plain,
        Precision::Beyond if codegen::perturbation(lp).is_some() => Mode::Pert,
        _ => Mode::Df64,
    }
}

/// The mode of the plan's most demanding layer (status display, tests).
pub fn renderer_mode(plan: &Plan) -> Mode {
    let modes: Vec<Mode> = plan.layers().iter().map(|l| layer_mode(plan, l)).collect();
    if modes.contains(&Mode::Pert) {
        Mode::Pert
    } else if modes.contains(&Mode::Df64) {
        Mode::Df64
    } else if modes.contains(&Mode::Dual) {
        Mode::Dual
    } else {
        Mode::Plain
    }
}

fn layer_uni(plan: &Plan, lp: &LayerPlan) -> Uni {
    let f = &plan.frame;
    let (o, i) = (&lp.outside, &lp.inside);
    Uni {
        size: [f.width, f.height, 0, 0],
        view: [f.center[0] as f32, f.center[1] as f32, (f.radius / (f.height as f64 * 0.5)) as f32, plan.magn() as f32],
        rot: [f.cos as f32, f.sin as f32, f.pixel_size() as f32, 1.0],
        iter: [lp.max_iter, lp.julia as u32, o.method_index(), i.method_index()],
        seed: [lp.seed[0] as f32, lp.seed[1] as f32, 0.0, 0.0],
        outc: [o.transfer.index() as f32, o.density as f32, o.offset as f32, 0.0],
        inc: [i.transfer.index() as f32, i.density as f32, i.offset as f32, 0.0],
        out_solid: o.solid,
        in_solid: i.solid,
        band: [0, 0, 0, NONE],
        lo: [(f.center[0] - f.center[0] as f32 as f64) as f32, (f.center[1] - f.center[1] as f32 as f64) as f32, 0.0, 0.0],
        blend: [0; 4],
        opacity: [1.0, 0.0, 0.0, 0.0],
        pert: [0.0; 4],
        perti: [0; 4],
        pref: [0.0; 4],
    }
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
    /// The reference orbit of a deep zoom (binding 5) and what it was computed for.
    orbit: wgpu::Buffer,
    orbit_key: u64,
    orbit_ref: Option<OrbitRef>,
    uni: Uni,
}

/// Where a reference orbit was computed.
#[derive(Clone)]
struct OrbitRef {
    /// The reference point (decimal text).
    point: [String; 2],
    bits: usize,
    len: i32,
    /// The reference point as `f32`.
    center: [f32; 2],
}

/// GPU resources of one operation.
struct OpRes {
    kind: OpKind,
    uniform: wgpu::Buffer,
}

struct Target {
    width: u32,
    height: u32,
    accum: wgpu::Buffer,
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
        GpuRenderer {
            disp_buf: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("display"),
                size: 16,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
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
            ctx,
        }
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
        let device = self.ctx.device.clone();
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
            let prev = old.next();
            // Images: uploaded again only when they change (they can be large).
            let image_src: Vec<_> = codegen::layer_images(lp)
                .into_iter()
                .map(|v| match v {
                    rfr_core::formula::ParamValue::Image { path } => rfr_core::images::get(path),
                    _ => None,
                })
                .collect();
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
            // Deep zooms: the reference orbit, computed again only when the view, the
            // iterations or the formula change.
            let mut uni = layer_uni(plan, lp);
            let pert = if mode == Mode::Pert { codegen::perturbation(lp) } else { None };
            let orbit_key = pert.as_ref().map_or(0, |p| {
                let mut h = std::collections::hash_map::DefaultHasher::new();
                use std::hash::{Hash, Hasher};
                lp.max_iter.hash(&mut h);
                format!("{p:?}").hash(&mut h);
                signature.hash(&mut h);
                h.finish()
            });
            // The reference is kept while it stays near the view and precise enough for its
            // depth (navigation does not wait for a new orbit); samples add the view center's
            // offset from it.
            let need_bits = rfr_core::math::big::bits_for(plan.frame.pixel_size_fx());
            let reusable = |r: &LayerRes| -> Option<[rfr_core::math::fx::Fx; 2]> {
                let o = r.orbit_ref.as_ref()?;
                if r.orbit_key != orbit_key || o.bits < need_bits {
                    return None;
                }
                let bits = need_bits + 64;
                let off = [0, 1].map(|k| {
                    let d = rfr_core::math::big::parse(&plan.center_text[k], bits) - rfr_core::math::big::parse(&o.point[k], bits);
                    rfr_core::math::big::to_fx(&d)
                });
                let reach = plan.frame.radius_fx * 4.0;
                (off[0].abs() <= reach && off[1].abs() <= reach).then_some(off)
            };
            let (orbit, orbit_ref, off) = match (&prev, &pert) {
                (Some(r), Some(_)) if let Some(off) = reusable(r) => (r.orbit.clone(), r.orbit_ref.clone(), off),
                (_, Some(p)) => {
                    let k = rfr_core::formula::cpu::Compiled::<f64>::new(&lp.fractal.program, &lp.fractal.values);
                    // A margin of bits: zooming in reuses the orbit for a while.
                    let bits = need_bits + 128;
                    let o = k.reference_orbit(p, &plan.center_text, bits, lp.max_iter);
                    let data: Vec<[f32; 2]> = o.values.iter().map(|c| [c.re as f32, c.im as f32]).collect();
                    let buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("reference orbit"),
                        contents: bytemuck::cast_slice(&data),
                        usage: wgpu::BufferUsages::STORAGE,
                    });
                    let info =
                        OrbitRef { point: plan.center_text.clone(), bits, len: o.len as i32, center: [o.center.re as f32, o.center.im as f32] };
                    (buf, Some(info), [rfr_core::math::fx::Fx::ZERO; 2])
                }
                _ => (
                    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("reference orbit"),
                        contents: bytemuck::cast_slice(&[0f32; 4]),
                        usage: wgpu::BufferUsages::STORAGE,
                    }),
                    None,
                    [rfr_core::math::fx::Fx::ZERO; 2],
                ),
            };
            if let Some(o) = &orbit_ref {
                let s = plan.frame.radius_fx / (plan.frame.height as f64 * 0.5);
                let off = rfr_core::math::fx::Cfx::from_fx(off[0], off[1]);
                uni.pert = [s.m as f32, off.re as f32, off.im as f32, 0.0];
                uni.perti = [s.e, o.len, off.e, 0];
                uni.pref = [o.center[0], o.center[1], 0.0, 0.0];
            }
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
                        size: (2 * LUT_SIZE * 16) as u64,
                        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    }),
                ),
            };
            self.ctx.queue.write_buffer(&params, 0, bytemuck::cast_slice(&codegen::param_values(lp)));
            let mut l: Vec<[f32; 4]> = lp.outside.lut.clone();
            l.extend_from_slice(&lp.inside.lut);
            self.ctx.queue.write_buffer(&lut, 0, bytemuck::cast_slice(&l));
            self.request_pipeline(signature, lp, mode);
            res.push(LayerRes { signature, params, lut, images, image_src, orbit, orbit_key, orbit_ref, uni });
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
        let (w, h) = (plan.frame.width, plan.frame.height);
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
        Target { width, height, accum, texture, view }
    }

    fn request_pipeline(&mut self, sig: u64, lp: &LayerPlan, mode: Mode) {
        if self.cache.lock().ok().is_some_and(|c| c.contains_key(&sig)) || self.pending.iter().any(|(s, _)| *s == sig) {
            return;
        }
        let source = codegen::kernel(lp, mode);
        let device = self.ctx.device.clone();
        let layout = self.pipeline_layout.clone();
        let cache = self.cache.clone();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let r = compile(&device, &layout, &source);
            if let Ok(mut c) = cache.lock() {
                c.insert(sig, r);
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
            match self.pipeline(l.signature) {
                Some(Ok(_)) => {}
                Some(Err(e)) => {
                    self.error.get_or_insert(e);
                    ready = false;
                }
                None => ready = false,
            }
        }
        self.ready = ready;
    }

    /// Waits until the shaders of the current plan are compiled.
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
                    let Some(Ok(p)) = self.pipeline(l.signature) else { return false };
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
                        ],
                    });
                    steps.push((self.composite_pipeline.clone(), bind));
                }
            }
        }
        let start = Instant::now();
        let mut worked = false;
        while self.samples < max_samples {
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
                            OpKind::Accumulate { src } => u.slots = [2, 0, *src, NONE],
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
        self.ctx.queue.write_buffer(&self.disp_buf, 0, bytemuck::cast_slice(&[t.width, t.height, self.samples, self.row]));
        let bind = self.ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("display"),
            layout: &self.display_pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.disp_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: t.accum.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&t.view) },
            ],
        });
        let mut enc = self.ctx.device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_compute_pass(&Default::default());
            pass.set_pipeline(&self.display_pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(t.width.div_ceil(8), t.height.div_ceil(8), 1);
        }
        self.ctx.queue.submit(Some(enc.finish()));
        self.generation += 1;
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
        let w = t.width as usize;
        let out = px
            .iter()
            .enumerate()
            .map(|(i, a)| {
                let n = self.samples + ((i / w) < self.row as usize) as u32;
                rfr_core::render::cpu::unpremultiply(*a, 1.0 / n.max(1) as f32)
            })
            .collect();
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

fn compile(device: &wgpu::Device, layout: &wgpu::PipelineLayout, source: &str) -> Result<wgpu::ComputePipeline, String> {
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
        entry_point: Some("main"),
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
