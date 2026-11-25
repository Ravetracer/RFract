// SPDX-License-Identifier: LGPL-3.0-or-later

//! Progressive GPU renderer: one sample per pixel per pass, split into row bands (one
//! submission each, adaptive height around a target time, so the driver never times out and
//! the UI stays responsive). Shaders compile on a worker thread; parameter, view and coloring
//! changes only rewrite buffers.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytemuck::{Pod, Zeroable};
use rfr_core::doc::{InsideMethod, OutsideMethod};
use rfr_core::gradient::{LUT_SIZE, linear_to_srgb};
use rfr_core::render::Plan;

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
}

fn outside_index(m: OutsideMethod) -> u32 {
    match m {
        OutsideMethod::Iterations => 0,
        OutsideMethod::Smooth => 1,
        OutsideMethod::Distance => 2,
        OutsideMethod::Solid => 3,
    }
}

fn inside_index(m: InsideMethod) -> u32 {
    match m {
        InsideMethod::Solid => 0,
        InsideMethod::Magnitude => 1,
        InsideMethod::Angle => 2,
    }
}

fn uniforms(plan: &Plan) -> Uni {
    let f = &plan.frame;
    let o = &plan.outside;
    let i = &plan.inside;
    Uni {
        size: [f.width, f.height, 0, 0],
        view: [f.center[0] as f32, f.center[1] as f32, (f.radius / (f.height as f64 * 0.5)) as f32, plan.magn() as f32],
        rot: [f.cos as f32, f.sin as f32, f.pixel_size() as f32, 1.0],
        iter: [plan.max_iter, plan.julia as u32, outside_index(plan.outside_method), inside_index(plan.inside_method)],
        seed: [plan.seed[0] as f32, plan.seed[1] as f32, 0.0, 0.0],
        outc: [o.transfer.index() as f32, o.density as f32, o.offset as f32, 0.0],
        inc: [i.transfer.index() as f32, i.density as f32, i.offset as f32, 0.0],
        out_solid: [o.solid[0], o.solid[1], o.solid[2], 1.0],
        in_solid: [i.solid[0], i.solid[1], i.solid[2], 1.0],
        band: [0; 4],
        lo: [(f.center[0] - f.center[0] as f32 as f64) as f32, (f.center[1] - f.center[1] as f32 as f64) as f32, 0.0, 0.0],
    }
}

fn params(plan: &Plan) -> Vec<[f32; 4]> {
    let mut p = vec![[0.0f32; 4]; MAX_PARAMS];
    for (k, v) in plan.values.iter().take(MAX_PARAMS).enumerate() {
        let c = v.as_complex();
        p[k] = [c[0] as f32, c[1] as f32, 0.0, 0.0];
    }
    p
}

/// Compiled pipelines shared by all renderers of a device, by shader signature.
type PipelineCache = Arc<Mutex<HashMap<u64, Result<wgpu::ComputePipeline, String>>>>;

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
    display_pipeline: wgpu::ComputePipeline,
    uniform_buf: wgpu::Buffer,
    param_buf: wgpu::Buffer,
    lut_buf: wgpu::Buffer,
    disp_buf: wgpu::Buffer,
    cache: PipelineCache,
    /// Signature of the wanted shader and the pipeline once it is ready.
    signature: u64,
    mode: Option<Mode>,
    pipeline: Option<wgpu::ComputePipeline>,
    pending: Option<(u64, Receiver<()>)>,
    error: Option<String>,
    target: Option<Target>,
    uni: Option<Uni>,
    samples: u32,
    row: u32,
    band_rows: u32,
    band_target: Duration,
    dirty_display: bool,
    generation: u64,
}

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
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("kernel"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let display_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("display"),
            source: wgpu::ShaderSource::Wgsl(include_str!("wgsl/display.wgsl").into()),
        });
        let display_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("display"),
            layout: None,
            module: &display_module,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let buf =
            |label, size: u64, usage| device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size, usage, mapped_at_creation: false });
        let u = wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST;
        GpuRenderer {
            uniform_buf: buf("uniforms", std::mem::size_of::<Uni>() as u64, u),
            param_buf: buf("params", (MAX_PARAMS * 16) as u64, u),
            lut_buf: buf("lut", (2 * LUT_SIZE * 16) as u64, wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST),
            disp_buf: buf("display", 16, u),
            layout,
            pipeline_layout,
            display_pipeline,
            cache: Arc::new(Mutex::new(HashMap::new())),
            signature: 0,
            mode: None,
            pipeline: None,
            pending: None,
            error: None,
            target: None,
            uni: None,
            samples: 0,
            row: 0,
            band_rows: 64,
            band_target: Duration::from_millis(12),
            dirty_display: false,
            generation: 0,
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
        self.pending.is_some()
    }
    /// The scalar mode of the current shader.
    pub fn mode(&self) -> Option<Mode> {
        self.mode
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

    /// Sets what to render. Restarts the accumulation; compiles a new shader (in the background)
    /// only when the formula, its functions or the scalar mode changed.
    pub fn set_plan(&mut self, plan: &Plan) {
        let mode = renderer_mode(plan);
        let sig = codegen::signature(&plan.formula_id, &plan.program, &plan.values, mode);
        self.mode = Some(mode);
        if sig != self.signature {
            self.signature = sig;
            self.pipeline = None;
            self.error = None;
            self.request_pipeline(sig, plan, mode);
        }
        let (w, h) = (plan.frame.width, plan.frame.height);
        if self.target.as_ref().is_none_or(|t| t.width != w || t.height != h) {
            self.target = Some(self.create_target(w, h));
        }
        let mut lut: Vec<[f32; 4]> = plan.outside.lut.clone();
        lut.extend_from_slice(&plan.inside.lut);
        self.ctx.queue.write_buffer(&self.lut_buf, 0, bytemuck::cast_slice(&lut));
        self.ctx.queue.write_buffer(&self.param_buf, 0, bytemuck::cast_slice(&params(plan)));
        self.uni = Some(uniforms(plan));
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

    fn request_pipeline(&mut self, sig: u64, plan: &Plan, mode: Mode) {
        if let Some(r) = self.cache.lock().ok().and_then(|c| c.get(&sig).cloned()) {
            match r {
                Ok(p) => self.pipeline = Some(p),
                Err(e) => self.error = Some(e),
            }
            self.pending = None;
            return;
        }
        let source = codegen::kernel(&plan.program, &plan.values, mode);
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
        self.pending = Some((sig, rx));
    }

    fn poll_pipeline(&mut self) {
        let ready = match &self.pending {
            Some((_, rx)) => rx.try_recv().is_ok(),
            None => false,
        };
        if ready {
            self.finish_pending();
        }
    }

    /// Takes the result of the finished background compile.
    fn finish_pending(&mut self) {
        let Some((sig, _)) = self.pending.take() else { return };
        if sig != self.signature {
            return;
        }
        match self.cache.lock().ok().and_then(|c| c.get(&sig).cloned()) {
            Some(Ok(p)) => self.pipeline = Some(p),
            Some(Err(e)) => self.error = Some(e),
            None => {}
        }
    }

    /// Waits until the shader of the current plan is compiled.
    pub fn wait_for_pipeline(&mut self) -> Result<(), String> {
        if let Some((_, rx)) = &self.pending {
            // Either the message or a closed channel: the compile thread is done.
            let _ = rx.recv();
            self.finish_pending();
        }
        if let Some(e) = &self.error {
            return Err(e.clone());
        }
        if self.pipeline.is_none() {
            return Err("no shader for the current formula".into());
        }
        Ok(())
    }

    /// Renders bands until `budget` is used up or `max_samples` passes are complete. Returns
    /// whether anything was rendered.
    pub fn work(&mut self, budget: Duration, max_samples: u32) -> bool {
        self.poll_pipeline();
        if self.samples >= max_samples {
            return false;
        }
        let (Some(t), Some(mut un), Some(pipeline)) = (self.target.as_ref(), self.uni, self.pipeline.clone()) else { return false };
        let (width, height) = (t.width, t.height);
        let device = self.ctx.device.clone();
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("kernel"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.uniform_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: self.param_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: self.lut_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: t.accum.as_entire_binding() },
            ],
        });
        let start = Instant::now();
        let mut worked = false;
        while self.samples < max_samples {
            let rows = self.band_rows.min(height - self.row);
            un.size[2] = self.samples;
            un.size[3] = self.row;
            un.band[0] = rows;
            self.ctx.queue.write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(&un));
            let mut enc = device.create_command_encoder(&Default::default());
            {
                let mut pass = enc.begin_compute_pass(&Default::default());
                pass.set_pipeline(&pipeline);
                pass.set_bind_group(0, &bind, &[]);
                pass.dispatch_workgroups(width.div_ceil(8), rows.div_ceil(8), 1);
            }
            let t0 = Instant::now();
            self.ctx.queue.submit(Some(enc.finish()));
            let _ = device.poll(wgpu::PollType::wait_indefinitely());
            let dt = t0.elapsed();
            worked = true;
            let ratio = self.band_target.as_secs_f64() / dt.as_secs_f64().max(1e-5);
            self.band_rows = ((rows as f64 * ratio.clamp(0.5, 2.0)).round() as u32).clamp(8, height.max(8)).next_multiple_of(8);
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
        self.ctx.queue.write_buffer(&self.disp_buf, 0, bytemuck::cast_slice(&[t.width, t.height, self.samples, 0]));
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

    /// Averaged linear RGB per pixel (reads the accumulation back).
    pub fn read_linear(&self) -> Option<Vec<[f32; 3]>> {
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
        let out = px.iter().map(|a| {
            let n = a[3].max(1.0);
            [a[0] / n, a[1] / n, a[2] / n]
        });
        let out = out.collect();
        drop(data);
        staging.unmap();
        Some(out)
    }

    /// The image as 8-bit sRGB RGBA (same encoding as the CPU renderer).
    pub fn read_rgba8(&self) -> Option<Vec<u8>> {
        Some(self.read_linear()?.iter().flat_map(|c| [linear_to_srgb(c[0]), linear_to_srgb(c[1]), linear_to_srgb(c[2]), 255]).collect())
    }

    /// The output texture (for copies by the caller).
    pub fn output_texture(&self) -> Option<&wgpu::Texture> {
        self.target.as_ref().map(|t| &t.texture)
    }
}

/// The scalar mode a plan renders with: f32 (with derivatives when the coloring needs them)
/// while that is precise enough, else double-float (distance estimation then falls back to the
/// smooth iteration count).
pub fn renderer_mode(plan: &Plan) -> Mode {
    match plan.precision() {
        rfr_core::render::Precision::Single if plan.needs_derivative() => Mode::Dual,
        rfr_core::render::Precision::Single => Mode::Plain,
        _ => Mode::Df64,
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

/// Renders `samples` passes of `plan` and returns linear RGB per pixel.
pub fn render_blocking(ctx: GpuContext, plan: &Plan, samples: u32, mut progress: impl FnMut(f32) -> bool) -> Result<Vec<[f32; 3]>, String> {
    let mut r = GpuRenderer::new(ctx);
    r.set_band_target(Duration::from_millis(40));
    r.set_plan(plan);
    r.wait_for_pipeline()?;
    while r.samples() < samples {
        r.work(Duration::from_millis(200), samples);
        if !progress(r.samples() as f32 / samples as f32) {
            return Err("cancelled".into());
        }
    }
    r.read_linear().ok_or_else(|| "could not read the image back from the GPU".into())
}
