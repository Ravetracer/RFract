// SPDX-License-Identifier: LGPL-3.0-or-later

//! Every library formula gives valid WGSL in both scalar modes (runs without a GPU), and the
//! GPU renders the same images as the CPU reference renderer (skipped without a GPU).

use rfr_core::doc::{Document, OutsideMethod};
use rfr_core::formula::library::library;
use rfr_core::render::Plan;
use rfr_gpu::codegen::{self, Mode};

fn validate(src: &str) -> Result<(), String> {
    let module = naga::front::wgsl::parse_str(src).map_err(|e| e.emit_to_string(src))?;
    naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty())
        .validate(&module)
        .map_err(|e| e.emit_to_string(src))?;
    Ok(())
}

#[test]
fn every_library_formula_gives_valid_wgsl() {
    let mut failures = Vec::new();
    for e in library().usable() {
        let p = e.program.as_ref().unwrap();
        let values: Vec<_> = p.params.iter().map(|p| p.default.clone()).collect();
        for mode in [Mode::Plain, Mode::Dual, Mode::Df64] {
            if let Err(err) = validate(&codegen::kernel(p, &values, mode)) {
                failures.push(format!("{} ({}, {mode:?}):\n{err}", e.title, e.id));
            }
        }
    }
    assert!(failures.is_empty(), "{} shaders fail:\n{}", failures.len(), failures.iter().take(5).cloned().collect::<Vec<_>>().join("\n\n"));
}

/// Mean absolute difference of two 8-bit images (0..255).
fn mean_diff(a: &[u8], b: &[u8]) -> f64 {
    a.iter().zip(b).map(|(x, y)| (*x as f64 - *y as f64).abs()).sum::<f64>() / a.len() as f64
}

fn rgba8(lin: &[[f32; 3]]) -> Vec<u8> {
    lin.iter()
        .flat_map(|c| {
            [rfr_core::gradient::linear_to_srgb(c[0]), rfr_core::gradient::linear_to_srgb(c[1]), rfr_core::gradient::linear_to_srgb(c[2]), 255]
        })
        .collect()
}

#[test]
fn gpu_matches_the_cpu_renderer() {
    let Ok(ctx) = rfr_gpu::GpuContext::new_headless() else {
        eprintln!("no GPU: skipped");
        return;
    };
    let mut cases: Vec<(String, Document)> = Vec::new();
    for id in ["rfract:Mandelbrot", "rfract:BurningShip", "rfract:Newton", "rfract:Magnet1", "rfract:Phoenix"] {
        cases.push((id.into(), Document::for_formula(id)));
    }
    let mut julia = Document::for_formula("rfract:Mandelbrot");
    julia.layers[0].julia = true;
    julia.view.set_center([0.0, 0.0]);
    cases.push(("julia".into(), julia));
    let mut de = Document::for_formula("rfract:Mandelbrot");
    de.layers[0].outside_method = OutsideMethod::Distance;
    cases.push(("distance".into(), de));
    // A few FractalPark formulas with transcendental functions.
    for e in library().usable().filter(|e| e.set == rfr_core::formula::library::Set::FractalPark).step_by(97) {
        cases.push((e.title.clone(), Document::for_formula(&e.id)));
    }
    // Deep views: double-float on the GPU against f64 on the CPU.
    for (name, c, r) in
        [("deep seahorse 1e-9", [-0.743643887037151, 0.131825904205330], 2e-9), ("deep elephant 1e-11", [0.2925755, -0.0149977], 3e-11)]
    {
        let mut d = Document::for_formula("rfract:Mandelbrot");
        d.view.radius = r;
        d.view.set_center(c);
        d.layers[0].max_iter = 3000;
        assert_eq!(rfr_gpu::renderer::renderer_mode(&Plan::sized(&d, 16, 9).unwrap()), Mode::Df64, "{name} needs double-float");
        cases.push((name.into(), d));
    }
    let (w, h, s) = (160, 90, 4);
    let mut report = Vec::new();
    for (name, doc) in &cases {
        let plan = Plan::sized(doc, w, h).unwrap();
        let cpu = rfr_core::render::cpu::render(doc, w, h, s).unwrap().rgba8();
        let gpu = rgba8(&rfr_gpu::render_blocking(ctx.clone(), &plan, s, |_| true).unwrap());
        let d = mean_diff(&cpu, &gpu);
        if let Ok(dir) = std::env::var("RFR_DUMP") {
            let f = name.replace([' ', ':', '/'], "_");
            rfr_core::io::png::write_rgba8(&std::path::Path::new(&dir).join(format!("{f}-cpu.png")), w, h, &cpu, None).unwrap();
            rfr_core::io::png::write_rgba8(&std::path::Path::new(&dir).join(format!("{f}-gpu.png")), w, h, &gpu, None).unwrap();
        }
        report.push(format!("{name}: {d:.3}"));
        // f32 on the GPU against f64 on the CPU: chaotic orbits near the set diverge, so a
        // few pixels differ; the image as a whole must agree.
        // Deep views are almost all fine detail, where 48 against 53 bits of mantissa change
        // single samples.
        let limit = if name.starts_with("deep") { 4.0 } else { 2.0 };
        assert!(d < limit, "{name}: mean difference {d:.2} of 255\n{}", report.join("\n"));
    }
    eprintln!("{}", report.join("\n"));
}
