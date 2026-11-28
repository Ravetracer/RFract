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
    // Distance estimation in deep views: double-float with derivatives, perturbation with
    // derivatives (the bundled formulas).
    for e in library().usable().filter(|e| e.id.starts_with("rfract:")) {
        let mut doc = Document::for_formula(&e.id);
        doc.layers[0].outside_method = OutsideMethod::Distance;
        check(&format!("distance {}", e.title), &doc, &[Mode::DualDf64]);
        if codegen::perturbation(&first_layer(&doc)).is_some() {
            check(&format!("distance {}", e.title), &doc, &[Mode::Pert]);
        }
    }
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
        // As a post-transformation of both colorings, chained.
        let mut d = Document::default();
        d.layers[0].outside_method = OutsideMethod::Formula;
        d.layers[0].outside.formula = "rfract-coloring:OrbitTrap".into();
        d.layers[0].inside_method = InsideMethod::Formula;
        d.layers[0].inside.formula = "rfract-coloring:TriangleInequality".into();
        d.layers[0].post_transforms.push(TransformRef { formula: e.id.clone(), ..TransformRef::default() });
        d.layers[0].post_transforms.push(TransformRef { formula: "rfract-transform:Twirl".into(), ..TransformRef::default() });
        check(&format!("post {}", e.title), &d, &all);
        if codegen::perturbation(&first_layer(&d)).is_some() {
            check(&format!("post {}", e.title), &d, &[Mode::Pert]);
        }
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
    // Root finding with derivatives from deriv(): Newton on z³ − 1, Householder on z⁴ + ½sin(z) − 1 (simple roots;
    // multiple roots converge slowly and f32 then stops at other steps).
    cases.push(("root function newton".into(), Document::for_formula("rfract:RootFunction")));
    let mut root = Document::for_formula("rfract:RootFunction");
    {
        use rfr_core::formula::ParamValue as V;
        let p = &mut root.layers[0].params;
        p.insert("method".into(), V::Int(3));
        p.insert("p".into(), V::Complex([4.0, 0.0]));
        p.insert("b".into(), V::Complex([0.5, 0.0]));
    }
    cases.push(("root function householder".into(), root));
    // Lit layers: heights from the smooth iteration count, the coloring value and the luminance.
    let mut lit = Document::for_formula("rfract:Mandelbrot");
    lit.layers[0].light.enabled = true;
    cases.push(("lit smooth".into(), lit.clone()));
    {
        use rfr_core::doc::{HeightSource, Light, SpecularModel};
        let mut v = lit.clone();
        let l = &mut v.layers[0].light;
        l.height = HeightSource::Value;
        l.model = SpecularModel::BlinnPhong;
        l.relief = 0.3;
        l.lights.push(Light { azimuth: 300.0, elevation: 20.0, color: [1.0, 0.4, 0.2], intensity: 0.8 });
        cases.push(("lit value".into(), v));
        let mut m = lit.clone();
        m.layers[0].light.height = HeightSource::Luminance;
        cases.push(("lit luminance".into(), m));
        let mut sh = lit.clone();
        let l = &mut sh.layers[0].light;
        l.shadows = true;
        l.occlusion = 1.0;
        l.lights[0].elevation = 25.0;
        l.lights.push(Light { azimuth: 330.0, elevation: 15.0, color: [0.3, 0.5, 1.0], intensity: 0.6 });
        cases.push(("lit shadows".into(), sh.clone()));
        // Distance heights (derivatives) with the sky reflected, and an environment image.
        let mut de = lit.clone();
        de.layers[0].light.height = HeightSource::Distance;
        de.layers[0].light.reflection = 0.4;
        cases.push(("lit distance sky".into(), de.clone()));
        let env = std::env::temp_dir().join(format!("rfr-test-env-{}.png", std::process::id()));
        let (ew, eh) = (32u32, 16u32);
        let px: Vec<u8> = (0..ew * eh).flat_map(|k| [(k % ew * 8) as u8, (k / ew * 16) as u8, 200, 255]).collect();
        rfr_core::io::png::write_rgba8(&env, ew, eh, &px, None).unwrap();
        let mut im = lit.clone();
        im.layers[0].light.reflection = 0.6;
        im.layers[0].light.environment = env.display().to_string();
        cases.push(("lit environment image".into(), im));
        // A 3D terrain with shadows, seen from the side.
        let mut ter = de.clone();
        let l = &mut ter.layers[0].light;
        l.relief = 0.05;
        l.terrain = true;
        l.shadows = true;
        l.camera.yaw = 30.0;
        cases.push(("lit terrain".into(), ter.clone()));
        // Point lights from the document's light sources, with shadows, flat and as a terrain.
        let light = |x: &str, y: &str, h: f64, color: [f32; 3]| rfr_core::doc::LightSource {
            position: [x.into(), y.into()],
            height: h,
            color,
            visible: false,
            ..Default::default()
        };
        let mut pl = sh.clone();
        pl.layers[0].light.scene_lights = true;
        pl.layers[0].light.ambient = 0.1;
        pl.lights.push(light("-1.3", "0.5", 0.3, [1.0, 0.6, 0.3]));
        pl.lights.push(rfr_core::doc::LightSource { reach: 0.3, ..light("0.4", "-0.4", 0.1, [0.3, 0.6, 1.0]) });
        cases.push(("lit point lights".into(), pl.clone()));
        let mut pt = ter.clone();
        pt.layers[0].light.scene_lights = true;
        pt.lights = pl.lights.clone();
        cases.push(("terrain point lights".into(), pt));
        // The denoiser's normal images of a lit layer and a terrain.
        cases.push(("normals lit".into(), rfr_core::render::post::aux_documents(&lit).unwrap()[1].clone()));
        cases.push(("normals terrain".into(), rfr_core::render::post::aux_documents(&ter).unwrap()[1].clone()));
        // Layer effects: the set alone (outside transparent) with a shadow and glows, and neon
        // edges on a plain Mandelbrot.
        use rfr_core::doc::{LayerEffects, OutsideMethod};
        let mut fx = Document::for_formula("rfract:Mandelbrot");
        fx.image.background = [0.1, 0.1, 0.15, 1.0];
        fx.layers[0].outside_method = OutsideMethod::Solid;
        fx.layers[0].outside.solid_alpha = 0.0;
        fx.layers[0].inside.solid = [0.8, 0.5, 0.2];
        fx.layers[0].effects = LayerEffects {
            shadow: 0.8,
            shadow_distance: 0.03,
            shadow_size: 0.02,
            glow: 0.8,
            glow_size: 0.03,
            inner_glow: 0.7,
            ..LayerEffects::default()
        };
        cases.push(("effects shadow glow".into(), fx));
        let mut neon = Document::for_formula("rfract:Mandelbrot");
        neon.layers[0].effects = LayerEffects { neon: 1.5, ..LayerEffects::default() };
        cases.push(("effects neon".into(), neon));
        // 3D objects: every shape, with shadows and occlusion.
        use rfr_core::doc::{LayerKind, ObjectShape};
        for (shape, dist) in
            [(ObjectShape::Mandelbulb, 2.8), (ObjectShape::Quaternion, 3.0), (ObjectShape::Mandelbox, 24.0), (ObjectShape::Menger, 4.5)]
        {
            let mut o = Document::for_formula("rfract:Mandelbrot");
            o.layers[0].kind = LayerKind::Object;
            o.layers[0].object.shape = shape;
            o.layers[0].object.camera.distance = dist;
            o.layers[0].light.shadows = true;
            o.layers[0].light.occlusion = 1.0;
            cases.push((format!("object {shape:?}"), o));
        }
        // A 3D object lit by the document's lights (point lights in its world).
        let mut ol = Document::for_formula("rfract:Mandelbrot");
        ol.layers[0].kind = LayerKind::Object;
        ol.layers[0].object.camera.distance = 2.8;
        ol.layers[0].light.shadows = true;
        ol.layers[0].light.scene_lights = true;
        let c = ol.view.center.clone();
        let near = |dx: f64, dy: f64| {
            let r = ol.view.radius.to_f64();
            [format!("{}", c[0].parse::<f64>().unwrap() + dx * r), format!("{}", c[1].parse::<f64>().unwrap() + dy * r)]
        };
        ol.lights.push(rfr_core::doc::LightSource {
            position: near(-0.8, -0.6),
            height: 0.5,
            color: [1.0, 0.5, 0.2],
            visible: false,
            ..Default::default()
        });
        ol.lights.push(rfr_core::doc::LightSource {
            position: near(0.7, 0.2),
            height: 0.2,
            reach: 0.5,
            color: [0.2, 0.5, 1.0],
            visible: false,
            ..Default::default()
        });
        cases.push(("object point lights".into(), ol));
    }
    let mut julia = Document::for_formula("rfract:Mandelbrot");
    julia.layers[0].julia = true;
    julia.view.set_center([0.0, 0.0]);
    cases.push(("julia".into(), julia));
    let mut de = Document::for_formula("rfract:Mandelbrot");
    de.layers[0].outside_method = OutsideMethod::Distance;
    cases.push(("distance".into(), de));
    for (label, method) in [("smooth", OutsideMethod::Smooth), ("bands", OutsideMethod::Iterations)] {
        let mut d = Document::for_formula("rfract:Mandelbrot");
        d.view.set_center([-0.745, 0.11]);
        d.view.radius = 0.02.into();
        d.layers[0].outside_method = method;
        d.layers[0].outside.transfer = rfr_core::doc::Transfer::Histogram;
        d.layers[0].outside.density = 1.0;
        d.layers[0].inside_method = InsideMethod::Magnitude;
        d.layers[0].inside.transfer = rfr_core::doc::Transfer::Histogram;
        d.layers[0].inside.density = 1.0;
        cases.push((format!("histogram {label}"), d));
    }
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
        // Post-transformations: the colorings see the transformed z.
        let mut d = Document::for_formula("rfract:Mandelbrot");
        d.layers[0].outside_method = OutsideMethod::Formula;
        d.layers[0].outside.formula = "rfract-coloring:StripeAverage".into();
        d.layers[0].outside.transfer = rfr_core::doc::Transfer::Linear;
        d.layers[0].outside.density = 1.0;
        d.layers[0].inside_method = InsideMethod::Formula;
        d.layers[0].inside.formula = "rfract-coloring:GaussianInteger".into();
        d.layers[0].post_transforms.push(TransformRef { formula: e.id.clone(), ..TransformRef::default() });
        cases.push((format!("post-transform {}", e.title), d));
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
        // Distance estimation there: double-float with derivatives.
        let mut de = d.clone();
        de.layers[0].outside_method = OutsideMethod::Distance;
        assert_eq!(rfr_gpu::renderer::renderer_mode(&Plan::sized(&de, 16, 9).unwrap()), Mode::DualDf64);
        cases.push((format!("{name} distance"), de));
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
    let gpu = rfr_core::render::cpu::rgba8(&rfr_gpu::render_blocking(ctx.clone(), &plan, s, |_| true).unwrap());
    assert!(cpu.chunks(4).any(|p| p[0] > 100) && cpu.chunks(4).any(|p| p[0] < 30), "the view shows structure");
    let diff = block_diff(&cpu, &gpu, w as usize, h as usize);
    assert!(diff < 4.0, "perturbation: block difference {diff:.2} of 255");

    // A coloring formula behind a post-transformation (every iteration, no linear steps).
    let mut post = d.clone();
    post.layers[0].outside_method = OutsideMethod::Formula;
    post.layers[0].outside.formula = "rfract-coloring:StripeAverage".into();
    post.layers[0].post_transforms.push(TransformRef { formula: "rfract-transform:Twirl".into(), ..TransformRef::default() });
    let plan_post = Plan::sized(&post, w, h).unwrap();
    assert!(plan_post.layers()[0].coloring_loops());
    let cpu = rfr_core::render::cpu::render(&post, w, h, s).unwrap().rgba8();
    let gpu = rfr_core::render::cpu::rgba8(&rfr_gpu::render_blocking(ctx.clone(), &plan_post, s, |_| true).unwrap());
    let diff = block_diff(&cpu, &gpu, w as usize, h as usize);
    assert!(diff < 4.0, "perturbation with a post-transformation: block difference {diff:.2} of 255");

    // Distance estimation: derivatives through the deltas (no linear steps).
    let mut de = d.clone();
    de.layers[0].outside_method = OutsideMethod::Distance;
    let plan_de = Plan::sized(&de, w, h).unwrap();
    let cpu = rfr_core::render::cpu::render(&de, w, h, s).unwrap().rgba8();
    let gpu = rfr_core::render::cpu::rgba8(&rfr_gpu::render_blocking(ctx.clone(), &plan_de, s, |_| true).unwrap());
    let smooth = rfr_core::render::cpu::render(&d, w, h, s).unwrap().rgba8();
    assert!(block_diff(&cpu, &smooth, w as usize, h as usize) > 10.0, "distance coloring differs from the smooth fallback");
    let diff = block_diff(&cpu, &gpu, w as usize, h as usize);
    assert!(diff < 4.0, "perturbation with distance estimation: block difference {diff:.2} of 255");

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
    // Metropolis chains in a zoomed view (the CPU and the GPU chains differ, the density not).
    let mut m = b.clone();
    m.view.center = ["-0.15".into(), "0.75".into()];
    m.view.radius = 0.3.into();
    m.layers[0].max_iter = 200;
    m.layers[0].density.orbits = 65536;
    m.layers[0].density.metropolis = true;
    cases.push(("metropolis", m));
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
    let mut koch = rfr_core::io::lsystem::document("Koch", rfr_core::doc::LSystem { width: 0.6, ..Default::default() });
    koch.image.width = 96;
    koch.image.height = 64;
    rfr_core::render::density::fit_view(&mut koch, &[0]);
    koch.layers[0].density.orbits = 2000;
    cases.push(("koch", koch));
    let (w, h) = (96, 64);
    for (name, doc) in cases {
        // Metropolis: the CPU and the GPU run their own chains, so only the noise differs; more
        // passes make it small.
        let s = if name == "metropolis" { 32 } else { 8 };
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

/// A render too large for the GPU reports an error instead of panicking.
#[test]
fn oversized_render_reports_an_error() {
    let Ok(ctx) = rfr_gpu::GpuContext::new_headless() else {
        eprintln!("no GPU: skipped");
        return;
    };
    let doc = Document::for_formula("rfract:Mandelbrot");
    let plan = Plan::sized(&doc, 60000, 60000).unwrap();
    let r = rfr_gpu::render_blocking(ctx, &plan, 1, |_| true);
    assert!(r.is_err(), "a 60000 × 60000 render cannot fit");
    eprintln!("{}", r.err().unwrap());
}

/// The preview's effects (display.wgsl, bloom.wgsl) match the export's (rfr_core post) on the
/// same accumulated image.
#[test]
fn preview_effects_match_the_export() {
    let Ok(ctx) = rfr_gpu::GpuContext::new_headless() else {
        eprintln!("no GPU: skipped");
        return;
    };
    use rfr_core::doc::{Post, ToneMap};
    let doc = Document::for_formula("rfract:Mandelbrot");
    let (w, h) = (160, 90);
    let plan = Plan::sized(&doc, w, h).unwrap();
    let mut r = rfr_gpu::GpuRenderer::new(ctx);
    r.set_plan(&plan);
    r.wait_for_pipeline().unwrap();
    while r.samples() < 4 {
        r.work(std::time::Duration::from_millis(200), 4);
    }
    let lin = r.read_linear().unwrap();
    let base = Post { exposure: 1.5, sharpen: 0.5, bloom: 0.8, bloom_threshold: 0.6, ..Post::default() };
    let cases = [
        ("neutral", Post::default()),
        ("bloom", base.clone()),
        ("streaks aces", Post { streak: 1.0, streak_threshold: 0.5, streak_count: 3, tone_map: ToneMap::Aces, ..base.clone() }),
        (
            "lens and grading",
            Post {
                tone_map: ToneMap::Agx,
                distortion: 0.2,
                chromatic: 0.02,
                temperature: 0.5,
                tint: -0.3,
                saturation: 1.4,
                shadow_amount: 0.6,
                highlight_amount: 0.5,
                contrast: 1.3,
                vignette: 0.8,
                grain: 0.3,
                ..base.clone()
            },
        ),
        ("filmic reinhard", Post { tone_map: ToneMap::Filmic, ..Post::default() }),
        ("god rays", Post { rays: 1.5, rays_threshold: 0.8, exposure: 1.0, ..Post::default() }),
        ("spectral aberration", Post { chromatic: 0.04, spectral: true, ..Post::default() }),
        (
            "levels and curve",
            Post { levels_black: 0.1, levels_white: 0.85, levels_gamma: 1.4, curve: [0.0, 0.15, 0.5, 0.85, 1.0], ..Post::default() },
        ),
        ("reinhard", Post { tone_map: ToneMap::Reinhard, exposure: 2.0, ..Post::default() }),
    ];
    let plain = rfr_core::render::post::finish(&lin, w, h, &Post::default());
    for (name, post) in cases {
        // Every effect case changes the image.
        let change = mean_diff(&rfr_core::render::post::finish(&lin, w, h, &post), &plain);
        assert!(name == "neutral" || change > 2.0, "{name}: changes the image by only {change:.2}");
        r.set_post(&post);
        r.display();
        let gpu = r.read_display().unwrap();
        let cpu = rfr_core::render::post::finish(&lin, w, h, &post);
        let max = gpu.iter().zip(&cpu).enumerate().filter(|(i, _)| i % 4 != 3).map(|(_, (a, b))| a.abs_diff(*b)).max().unwrap();
        let d = mean_diff(&gpu, &cpu);
        eprintln!("{name}: mean {d:.3}, max {max}");
        assert!(d < 0.5 && max <= 8, "{name}: mean {d:.3}, max {max} of 255");
    }
    // Light sources: a visible light with a flare, plain and with effects.
    for post in [Post::default(), base.clone()] {
        let mut lit = doc.clone();
        lit.post = post;
        let flare = rfr_core::flare::LensFlare::presets().into_iter().find(|(n, _)| *n == "Sun").unwrap().1;
        lit.lights.push(rfr_core::doc::LightSource { position: ["-1.2".into(), "0.4".into()], flare, ..Default::default() });
        r.set_post(&lit.post);
        r.set_lights(&lit);
        r.display();
        let gpu = r.read_display().unwrap();
        let cpu = rfr_core::render::post::finish_export(lin.clone(), w, h, &lit, None).0;
        let change = mean_diff(&cpu, &rfr_core::render::post::finish(&lin, w, h, &lit.post));
        let max = gpu.iter().zip(&cpu).enumerate().filter(|(i, _)| i % 4 != 3).map(|(_, (a, b))| a.abs_diff(*b)).max().unwrap();
        let d = mean_diff(&gpu, &cpu);
        eprintln!("lights: changes {change:.2}, mean {d:.3}, max {max}");
        assert!(change > 2.0 && d < 0.5 && max <= 8, "lights: changes {change:.2}, mean {d:.3}, max {max} of 255");
    }
}

/// A frame of a zoom sequence (composed from images at doublings of the zoom) looks like a
/// frame rendered directly at its zoom.
#[test]
fn zoom_sequence_frames_match_direct_renders() {
    let Ok(ctx) = rfr_gpu::GpuContext::new_headless() else {
        eprintln!("no GPU: skipped");
        return;
    };
    use rfr_core::animation::{Animation, Keyframe, Timing, ZoomPath};
    let mut a = Document::for_formula("rfract:Mandelbrot");
    a.view.set_center([-0.6, 0.0]);
    let mut b = a.clone();
    b.view.set_center([-0.743643887037151, 0.131825904205330]);
    b.view.radius = 1.5e-4.into();
    let anim = Animation {
        keyframes: vec![Keyframe { doc: a, ..Keyframe::default() }, Keyframe { doc: b, ..Keyframe::default() }],
        timing: Timing::Steady,
        length_s: 1.0,
        fps: 20.0,
        ease_in_s: 0.0,
        ease_out_s: 0.0,
        zoom_sequence: true,
        ..Animation::default()
    };
    let path = ZoomPath::new(&anim).unwrap();
    let (w, h, s) = (128, 72, 16);
    let mut seq = rfr_gpu::animation::ZoomSequence::new(Some(ctx.clone()), ZoomPath::new(&anim).unwrap(), w, h, s);
    for f in [5usize, 12, 19] {
        let (frame, _) = seq.finished(f).unwrap();
        let mut d = path.doc.clone();
        let l = path.log2_radius_at(f as f64);
        let e = l.floor();
        d.view.radius = rfr_core::math::fx::Fx::new(2f64.powf(l - e), e as i32);
        let direct = rfr_core::render::cpu::rgba8(&rfr_gpu::render_blocking(ctx.clone(), &Plan::sized(&d, w, h).unwrap(), s, |_| true).unwrap());
        let diff = block_diff(&frame, &direct, w as usize, h as usize);
        eprintln!("frame {f}: block difference {diff:.2}");
        assert!(diff < 8.0, "frame {f}: {diff:.2}");
    }
    assert!(seq.rendered <= 16, "{} images for a zoom by 10^4", seq.rendered);
}

/// A poster rendered in tiles (`rfr_gpu::poster`) looks like the image rendered in one piece,
/// effects included; the PNG holds the document.
#[test]
fn tiled_posters_match_whole_renders() {
    let Ok(ctx) = rfr_gpu::GpuContext::new_headless() else {
        eprintln!("no GPU: skipped");
        return;
    };
    let mut doc = Document::for_formula("rfract:Mandelbrot");
    doc.view.set_center([-0.745, 0.11]);
    doc.view.radius = 0.02.into();
    doc.view.rotation = 20.0;
    // Histogram coloring: every tile takes the tables of the whole view.
    doc.layers[0].outside.transfer = rfr_core::doc::Transfer::Histogram;
    doc.layers[0].outside.density = 1.0;
    doc.post.sharpen = 0.3;
    doc.post.vignette = 0.3;
    doc.post.distortion = 0.08;
    doc.post.chromatic = 0.01;
    let (w, h, s) = (2300u32, 900u32, 4);
    let dir = std::env::temp_dir().join(format!("rfr-poster-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("poster.png");
    let never = std::sync::atomic::AtomicBool::new(false);
    rfr_gpu::poster::render_poster(&doc, (w, h, s), Some(ctx.clone()), &path, &never, |_| {}).unwrap();
    let (_, _, tiled) = rfr_core::io::png::read_rgba8(&path).unwrap();
    let (whole, _) = rfr_gpu::animation::FrameRenderer::new(Some(ctx)).still(&doc, w, h, s).unwrap();
    let diff = block_diff(&tiled, &whole, w as usize, h as usize);
    eprintln!("tiled vs whole: block difference {diff:.2}");
    assert!(diff < 3.0, "{diff:.2}");
    assert_eq!(rfr_core::io::open(&path).unwrap().view, doc.view);
    std::fs::remove_dir_all(&dir).ok();
}

/// Adaptive sampling on the GPU gives the image of the CPU renderer (and of full sampling).
#[test]
fn adaptive_sampling_matches_the_cpu() {
    let Ok(ctx) = rfr_gpu::GpuContext::new_headless() else {
        eprintln!("no GPU: skipped");
        return;
    };
    let mut doc = Document::for_formula("rfract:Mandelbrot");
    doc.view.set_center([-0.745, 0.11]);
    doc.view.radius = 0.05.into();
    let (w, h, s) = (160u32, 90u32, 48u32);
    let plan = Plan::sized(&doc, w, h).unwrap().with_adaptive(0.005);
    let gpu = rfr_core::render::cpu::rgba8(&rfr_gpu::render_blocking(ctx.clone(), &plan, s, |_| true).unwrap());
    let mut cpu = rfr_core::render::cpu::CpuRenderer::new(plan);
    for _ in 0..s {
        cpu.pass();
    }
    assert!(cpu.done_share() > 0.2, "{}", cpu.done_share());
    let full =
        rfr_core::render::cpu::rgba8(&rfr_gpu::render_blocking(ctx, &Plan::sized(&doc, w, h).unwrap().with_adaptive(0.0), s, |_| true).unwrap());
    let (d1, d2) = (block_diff(&gpu, &cpu.rgba8(), w as usize, h as usize), block_diff(&gpu, &full, w as usize, h as usize));
    eprintln!("adaptive GPU vs CPU {d1:.2}, vs full {d2:.2}");
    assert!(d1 < 2.0 && d2 < 2.0, "{d1:.2} {d2:.2}");
}
