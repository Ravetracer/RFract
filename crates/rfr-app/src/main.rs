// SPDX-License-Identifier: LGPL-3.0-or-later

//! RFract: 2D fractal generator with layers, GPU rendering and animation.

// Release builds on Windows start without a console window.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod anim_export;
mod app;
mod auto_iter;
mod browser;
mod doc;
mod editor;
mod explorer;
mod export;
mod help;
mod icons;
mod inspector;
mod layers;
mod lights;
mod manual;
mod palette_editor;
mod prefs;
mod queue_window;
mod theme;
mod timeline;
mod viewport;
mod widgets;

use std::sync::Arc;

fn main() -> eframe::Result {
    let initial = std::env::args().nth(1).map(std::path::PathBuf::from);
    let mut wgpu_options = egui_wgpu::WgpuConfiguration::default();
    if let egui_wgpu::WgpuSetup::CreateNew(setup) = &mut wgpu_options.wgpu_setup {
        setup.device_descriptor = Arc::new(|adapter: &wgpu::Adapter| wgpu::DeviceDescriptor {
            label: Some("rfract device"),
            required_limits: rfr_gpu::GpuContext::required_limits(adapter),
            ..Default::default()
        });
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("RFract")
            .with_inner_size([1480.0, 900.0])
            .with_min_inner_size([960.0, 600.0])
            .with_drag_and_drop(true)
            .with_icon(eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")).unwrap_or_default()),
        renderer: eframe::Renderer::Wgpu,
        wgpu_options,
        ..Default::default()
    };
    eframe::run_native("RFract", options, Box::new(move |cc| Ok(Box::new(app::App::new(cc, initial)))))
}
