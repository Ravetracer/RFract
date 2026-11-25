// SPDX-License-Identifier: LGPL-3.0-or-later

//! RFract GPU renderer: WGSL generated per formula (`codegen.rs`) and a progressive compute
//! renderer (`renderer.rs`) that mirrors the CPU reference renderer of rfr-core.

pub mod codegen;
pub mod renderer;

pub use renderer::{GpuContext, GpuRenderer, render_blocking};

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
