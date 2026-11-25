// SPDX-License-Identifier: LGPL-3.0-or-later

//! RFract GPU renderer (wgpu compute shaders). Stage 0: device limits and adapter detection;
//! the per-layer shader generation and the progressive renderer follow in stage 1.

/// GPU device setup shared by the application (through eframe) and the command line tool.
pub struct GpuContext;

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
}

/// What the GPU detection found.
#[derive(Clone, Debug)]
pub struct AdapterSummary {
    pub name: String,
    pub backend: String,
    pub device_type: String,
    pub driver: String,
    /// Native 64-bit floats in shaders (not on Metal; deep zoom uses emulation otherwise).
    pub shader_f64: bool,
}

impl std::fmt::Display for AdapterSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({}, {}", self.name, self.backend, self.device_type)?;
        if !self.driver.is_empty() {
            write!(f, ", driver {}", self.driver)?;
        }
        write!(f, ", f64 in shaders: {})", if self.shader_f64 { "yes" } else { "no" })
    }
}

/// The adapter a renderer would use (high performance preference), or why there is none.
pub fn detect_adapter() -> Result<AdapterSummary, String> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle().with_env());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        force_fallback_adapter: false,
        compatible_surface: None,
        ..Default::default()
    }))
    .map_err(|e| format!("no GPU adapter: {e}"))?;
    let info = adapter.get_info();
    Ok(AdapterSummary {
        name: info.name,
        backend: format!("{:?}", info.backend),
        device_type: format!("{:?}", info.device_type),
        driver: [info.driver, info.driver_info].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" "),
        shader_f64: adapter.features().contains(wgpu::Features::SHADER_F64),
    })
}
