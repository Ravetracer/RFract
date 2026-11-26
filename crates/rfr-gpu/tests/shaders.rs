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
    let mut perturbable = 0;
    for e in library().usable() {
        let doc = Document::for_formula(&e.id);
        check(&e.title, &doc, &all);
        // Deep zooms: the perturbation kernel of every formula that allows it.
        if codegen::perturbation(&first_layer(&doc)).is_some() {
            perturbable += 1;
            check(&e.title, &doc, &[Mode::Pert]);
        }
    }
    assert!(perturbable >= 10, "only {perturbable} formulas can be perturbed");
    // Density layers: the Mandelbrot (Buddhabrot) and every attractor.
    for e in library().usable().filter(|e| e.id == "rfract:Mandelbrot" || e.id.starts_with("rfract-attractor:")) {
        let mut doc = Document::for_formula(&e.id);
        doc.layers[0].kind = rfr_core::doc::LayerKind::Density;
        check(&format!("density {}", e.title), &doc, &[Mode::Plain]);
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

/// Mean absolute difference of the 8 × 8 block averages of two 8-bit images (0..255): the
/// structure, not the noise of single pixels in chaotic regions.
fn block_diff(a: &[u8], b: &[u8], w: usize, h: usize) -> f64 {
    let avg = |img: &[u8], bx: usize, by: usize, c: usize| {
        let mut s = 0.0;
        for y in by * 8..(by * 8 + 8).min(h) {
            for x in bx * 8..(bx * 8 + 8).min(w) {
                s += img[(y * w + x) * 4 + c] as f64;
            }
        }
        s / 64.0
    };
    let (mut total, mut n) = (0.0, 0.0);
    for by in 0..h / 8 {
        for bx in 0..w / 8 {
            for c in 0..3 {
                total += (avg(a, bx, by, c) - avg(b, bx, by, c)).abs();
                n += 1.0;
            }
        }
    }
    total / n
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
    // Image parameters: a small generated image, looked up by the final z and tiled by pixel.
    let img_path = std::env::temp_dir().join(format!("rfr-test-image-{}.png", std::process::id()));
    let (iw, ih) = (12u32, 8u32);
    let pixels: Vec<u8> = (0..iw * ih)
        .flat_map(|k| {
            let (x, y) = (k % iw, k / iw);
            [(x * 21) as u8, (y * 31) as u8, ((x + y) * 13) as u8, if (x + y) % 5 == 0 { 0 } else { 255 }]
        })
        .collect();
    rfr_core::io::png::write_rgba8(&img_path, iw, ih, &pixels, None).unwrap();
    for (label, source, tile) in [("final z", 0, false), ("pixel tiled", 1, true)] {
        let mut d = Document::for_formula("rfract:Mandelbrot");
        d.layers[0].outside_method = OutsideMethod::Formula;
        let o = &mut d.layers[0].outside;
        o.formula = "rfract-coloring:ImageMap".into();
        o.params.insert("image".into(), rfr_core::formula::ParamValue::Image { path: img_path.display().to_string() });
        o.params.insert("source".into(), rfr_core::formula::ParamValue::Int(source));
        o.params.insert("tile".into(), rfr_core::formula::ParamValue::Bool(tile));
        o.params.insert("size".into(), rfr_core::formula::ParamValue::Float(1.3));
        cases.push((format!("image {label}"), d));
    }
    cases.push(("layered".into(), layered()));
    let mut transparent = layered();
    transparent.image.background = [0.0; 4];
    cases.push(("transparent background".into(), transparent));
    for (name, c, r) in
        [("deep seahorse 1e-9", [-0.743643887037151, 0.131825904205330], 2e-9), ("deep elephant 1e-11", [0.2925755, -0.0149977], 3e-11)]
    {
        let mut d = Document::for_formula("rfract:Mandelbrot");
        d.view.radius = r.into();
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

#[test]
fn gpu_perturbation_matches_the_cpu() {
    let Ok(ctx) = rfr_gpu::GpuContext::new_headless() else {
        eprintln!("no GPU: skipped");
        return;
    };
    // 1e-22 on the seahorse spiral: far below double precision; both renderers perturb.
    let mut d = Document::for_formula("rfract:Mandelbrot");
    d.view.center = ["-0.743643887037158704752191506114774".into(), "0.131825904205311970493132056385139".into()];
    d.view.radius = rfr_core::math::fx::Fx::parse("1e-22").unwrap();
    d.layers[0].max_iter = 12000;
    let (w, h, s) = (80, 48, 2);
    let plan = Plan::sized(&d, w, h).unwrap();
    assert_eq!(rfr_gpu::renderer::renderer_mode(&plan), Mode::Pert);
    let cpu = rfr_core::render::cpu::render(&d, w, h, s).unwrap().rgba8();
    let gpu = rfr_core::render::cpu::rgba8(&rfr_gpu::render_blocking(ctx, &plan, s, |_| true).unwrap());
    assert!(cpu.chunks(4).any(|p| p[0] > 100) && cpu.chunks(4).any(|p| p[0] < 30), "the view shows structure");
    let diff = block_diff(&cpu, &gpu, w as usize, h as usize);
    assert!(diff < 4.0, "perturbation: block difference {diff:.2} of 255");

    // Moving the view keeps the reference orbit (the samples add the offset); the image
    // matches a render with a fresh orbit at the new center.
    let mut moved = d.clone();
    moved.view.pan([0.6, -0.4]);
    moved.view.radius *= 0.8;
    let plan2 = Plan::sized(&moved, w, h).unwrap();
    let ctx2 = rfr_gpu::GpuContext::new_headless().unwrap();
    let mut r = rfr_gpu::renderer::GpuRenderer::new(ctx2.clone());
    r.set_band_target(std::time::Duration::from_millis(40));
    for p in [&plan, &plan2] {
        r.set_plan(p);
        r.wait_for_pipeline().unwrap();
        while r.samples() < s {
            r.work(std::time::Duration::from_millis(200), s);
        }
    }
    let reused = rfr_core::render::cpu::rgba8(&r.read_linear().unwrap());
    let fresh = rfr_core::render::cpu::rgba8(&rfr_gpu::render_blocking(ctx2.clone(), &plan2, s, |_| true).unwrap());
    let diff = block_diff(&reused, &fresh, w as usize, h as usize);
    assert!(diff < 3.0, "reused reference orbit: block difference {diff:.2} of 255");

    // Far away: a new orbit computes in the background; once it is there the image matches a
    // fresh render.
    let mut far = moved.clone();
    far.view.pan([10.0, 6.0]);
    let plan3 = Plan::sized(&far, w, h).unwrap();
    r.set_plan(&plan3);
    assert!(r.orbit_pending(), "a new reference orbit is computing");
    let t = std::time::Instant::now();
    while r.orbit_pending() && t.elapsed() < std::time::Duration::from_secs(20) {
        r.work(std::time::Duration::from_millis(50), s);
    }
    assert!(!r.orbit_pending());
    r.wait_for_pipeline().unwrap();
    while r.samples() < s {
        r.work(std::time::Duration::from_millis(200), s);
    }
    let after = rfr_core::render::cpu::rgba8(&r.read_linear().unwrap());
    let fresh = rfr_core::render::cpu::rgba8(&rfr_gpu::render_blocking(ctx2, &plan3, s, |_| true).unwrap());
    let diff = block_diff(&after, &fresh, w as usize, h as usize);
    assert!(diff < 3.0, "background reference orbit: block difference {diff:.2} of 255");
}

#[test]
fn gpu_density_matches_the_cpu() {
    let Ok(ctx) = rfr_gpu::GpuContext::new_headless() else {
        eprintln!("no GPU: skipped");
        return;
    };
    let mut cases = Vec::new();
    let mut b = Document::for_formula("rfract:Mandelbrot");
    b.view.center = ["-0.4".into(), "0".into()];
    b.view.radius = 1.3.into();
    b.layers[0].kind = rfr_core::doc::LayerKind::Density;
    b.layers[0].max_iter = 300;
    b.layers[0].density.orbits = 40000;
    b.layers[0].outside.gradient = rfr_core::gradient::preset("Ember").unwrap();
    cases.push(("buddhabrot", b.clone()));
    let mut n = b.clone();
    n.layers[0].density.nebula = true;
    n.layers[0].density.nebula_iter = [300, 100, 30];
    cases.push(("nebulabrot", n));
    let mut a = Document::for_formula("rfract-attractor:Clifford");
    a.layers[0].kind = rfr_core::doc::LayerKind::Density;
    a.layers[0].density.mode = rfr_core::doc::DensityMode::Attractor;
    a.layers[0].density.orbits = 2000;
    a.layers[0].outside.gradient = rfr_core::gradient::preset("Ember").unwrap();
    cases.push(("clifford", a));
    let mut f = Document::for_formula("rfract:Mandelbrot");
    f.view.center = ["0".into(), "0".into()];
    f.view.radius = 1.2.into();
    f.layers[0].kind = rfr_core::doc::LayerKind::Density;
    f.layers[0].density.mode = rfr_core::doc::DensityMode::Ifs;
    f.layers[0].density.orbits = 2000;
    f.layers[0].max_iter = 200;
    f.layers[0].outside.gradient = rfr_core::gradient::preset("Ember").unwrap();
    cases.push(("sierpinski", f.clone()));
    let mut v = f.clone();
    v.layers[0].density.ifs[0].variation = rfr_core::doc::Variation::Swirl;
    v.layers[0].density.ifs[1].variation = rfr_core::doc::Variation::Spherical;
    cases.push(("ifs variations", v));
    let mut k = f.clone();
    k.view.radius = 1.3.into();
    k.layers[0].density.mode = rfr_core::doc::DensityMode::Kleinian;
    k.layers[0].density.kleinian.ta = [2.0, 0.0];
    k.layers[0].density.kleinian.tb = [2.0, 0.0];
    cases.push(("apollonian", k.clone()));
    let mut q = k.clone();
    q.layers[0].density.kleinian = rfr_core::doc::Kleinian::default();
    cases.push(("quasi-fuchsian", q));
    let (w, h, s) = (96, 64, 8);
    for (name, doc) in cases {
        let plan = Plan::sized(&doc, w, h).unwrap();
        let cpu = rfr_core::render::cpu::render(&doc, w, h, s).unwrap().rgba8();
        let gpu = rfr_core::render::cpu::rgba8(&rfr_gpu::render_blocking(ctx.clone(), &plan, s, |_| true).unwrap());
        if let Ok(dir) = std::env::var("RFR_DUMP") {
            rfr_core::io::png::write_rgba8(&std::path::Path::new(&dir).join(format!("{name}-cpu.png")), w, h, &cpu, None).unwrap();
            rfr_core::io::png::write_rgba8(&std::path::Path::new(&dir).join(format!("{name}-gpu.png")), w, h, &gpu, None).unwrap();
        }
        assert!(cpu.chunks(4).filter(|p| p[0] > 60).count() > 50, "{name}: the CPU image shows the orbits");
        let d = block_diff(&cpu, &gpu, w as usize, h as usize);
        eprintln!("{name}: block difference {d:.2}");
        assert!(d < 6.0, "{name}: block difference {d:.2} of 255");
    }
}
