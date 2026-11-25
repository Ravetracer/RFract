// SPDX-License-Identifier: LGPL-3.0-or-later

//! Every library formula, coloring and transformation gives valid WGSL in all scalar modes
//! (runs without a GPU), and the GPU renders the same images as the CPU reference renderer,
//! layers, groups, masks and merge modes included (skipped without a GPU).

use rfr_core::doc::{Document, InsideMethod, Layer, LayerKind, Mask, MaskSource, MergeMode, OutsideMethod, TransformRef};
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

fn first_layer(doc: &Document) -> rfr_core::render::LayerPlan {
    Plan::sized(doc, 16, 9).unwrap().layers()[0].clone()
}

#[test]
fn every_library_program_gives_valid_wgsl() {
    let mut failures = Vec::new();
    let mut check = |name: &str, doc: &Document, modes: &[Mode]| {
        let lp = first_layer(doc);
        for mode in modes {
            if let Err(err) = validate(&codegen::kernel(&lp, *mode)) {
                failures.push(format!("{name} ({mode:?}):\n{err}"));
            }
        }
    };
    let all = [Mode::Plain, Mode::Dual, Mode::Df64];
    for e in library().usable() {
        check(&e.title, &Document::for_formula(&e.id), &all);
    }
    for e in library().colorings() {
        let mut d = Document::default();
        d.layers[0].outside_method = OutsideMethod::Formula;
        d.layers[0].outside.formula = e.id.clone();
        d.layers[0].inside_method = InsideMethod::Formula;
        d.layers[0].inside.formula = e.id.clone();
        check(&e.title, &d, &all);
    }
    for e in library().transforms() {
        let mut d = Document::default();
        d.layers[0].transforms.push(TransformRef { formula: e.id.clone(), ..TransformRef::default() });
        d.layers[0].transforms.push(TransformRef { formula: "rfract-transform:Twirl".into(), ..TransformRef::default() });
        check(&e.title, &d, &all);
    }
    assert!(validate(&codegen::composite_shader()).is_ok(), "composite shader");
    assert!(failures.is_empty(), "{} shaders fail:\n{}", failures.len(), failures.iter().take(4).cloned().collect::<Vec<_>>().join("\n\n"));
}

/// Mean absolute difference of two 8-bit images (0..255).
fn mean_diff(a: &[u8], b: &[u8]) -> f64 {
    a.iter().zip(b).map(|(x, y)| (*x as f64 - *y as f64).abs()).sum::<f64>() / a.len() as f64
}

/// A document with every feature of the layer stack.
fn layered() -> Document {
    let mut doc = Document::for_formula("rfract:Mandelbrot");
    doc.image.background = [0.02, 0.02, 0.05, 1.0];
    let base = &mut doc.layers[0];
    base.outside_method = OutsideMethod::Formula;
    base.outside.formula = "rfract-coloring:TriangleInequality".into();
    base.outside.transfer = rfr_core::doc::Transfer::Linear;
    base.outside.density = 1.0;
    base.inside_method = InsideMethod::Formula;
    base.inside.formula = "rfract-coloring:OrbitTrap".into();
    let mut julia = Layer { name: "Julia".into(), julia: true, merge: MergeMode::Overlay, opacity: 0.8, ..Layer::default() };
    julia.transforms.push(TransformRef { formula: "rfract-transform:Kaleidoscope".into(), ..TransformRef::default() });
    julia.outside.gradient.alpha =
        vec![rfr_core::gradient::AlphaStop { pos: 0.0, alpha: 0.2 }, rfr_core::gradient::AlphaStop { pos: 0.5, alpha: 1.0 }];
    let ship = Layer { name: "Ship".into(), formula: "rfract:BurningShip".into(), merge: MergeMode::Hue, ..Layer::default() };
    let mut group = Layer {
        name: "Group".into(),
        kind: LayerKind::Group,
        merge: MergeMode::Screen,
        opacity: 0.7,
        children: vec![julia, ship],
        ..Layer::default()
    };
    let mut mask = Mask::default();
    mask.layer.formula = "rfract:Newton".into();
    mask.invert = true;
    group.mask = Some(mask);
    doc.layers.push(group);
    let mut top = Layer { name: "Masked".into(), formula: "rfract:Phoenix".into(), merge: MergeMode::Multiply, ..Layer::default() };
    let mut m = Mask { source: MaskSource::Alpha, ..Mask::default() };
    m.layer.outside.gradient.alpha =
        vec![rfr_core::gradient::AlphaStop { pos: 0.0, alpha: 0.0 }, rfr_core::gradient::AlphaStop { pos: 0.6, alpha: 1.0 }];
    top.mask = Some(m);
    doc.layers.push(top);
    doc
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
    for e in library().usable().filter(|e| e.set == rfr_core::formula::library::Set::FractalPark).step_by(97) {
        cases.push((e.title.clone(), Document::for_formula(&e.id)));
    }
    for e in library().colorings() {
        let mut d = Document::for_formula("rfract:Mandelbrot");
        d.layers[0].outside_method = OutsideMethod::Formula;
        d.layers[0].outside.formula = e.id.clone();
        d.layers[0].outside.transfer = rfr_core::doc::Transfer::Linear;
        d.layers[0].outside.density = 1.0;
        cases.push((format!("coloring {}", e.title), d));
    }
    for e in library().transforms() {
        let mut d = Document::for_formula("rfract:Mandelbrot");
        d.layers[0].transforms.push(TransformRef { formula: e.id.clone(), ..TransformRef::default() });
        cases.push((format!("transform {}", e.title), d));
    }
    cases.push(("layered".into(), layered()));
    let mut transparent = layered();
    transparent.image.background = [0.0; 4];
    cases.push(("transparent background".into(), transparent));
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
        let gpu = rfr_core::render::cpu::rgba8(&rfr_gpu::render_blocking(ctx.clone(), &plan, s, |_| true).unwrap_or_else(|e| panic!("{name}: {e}")));
        let d = mean_diff(&cpu, &gpu);
        if let Ok(dir) = std::env::var("RFR_DUMP") {
            let f = name.replace([' ', ':', '/'], "_");
            rfr_core::io::png::write_rgba8(&std::path::Path::new(&dir).join(format!("{f}-cpu.png")), w, h, &cpu, None).unwrap();
            rfr_core::io::png::write_rgba8(&std::path::Path::new(&dir).join(format!("{f}-gpu.png")), w, h, &gpu, None).unwrap();
        }
        report.push(format!("{name}: {d:.3}"));
        // f32 on the GPU against f64 on the CPU: chaotic orbits near the set diverge, so a few
        // pixels differ; deep views are almost all fine detail (48 against 53 bits change single
        // samples). The image as a whole must agree.
        let limit = if name.starts_with("deep") { 4.0 } else { 2.0 };
        assert!(d < limit, "{name}: mean difference {d:.2} of 255\n{}", report.join("\n"));
    }
    eprintln!("{}", report.join("\n"));
}
