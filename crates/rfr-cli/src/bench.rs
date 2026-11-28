// SPDX-License-Identifier: LGPL-3.0-or-later

//! `rfr-cli bench`: renders a fixed set of documents and reports the median render times, for
//! comparing builds on the same machine. Shaders compile before the timed runs.

use std::time::{Duration, Instant};

use rfr_core::doc::{Document, HeightSource, Layer, LayerKind};
use rfr_core::render::Plan;

/// A benchmark case: name, document, size and samples.
struct Case {
    name: &'static str,
    doc: Document,
    size: (u32, u32),
    samples: u32,
}

fn example(text: &str) -> Document {
    Document::from_json(text).expect("bundled example")
}

/// The last keyframe of a document's animation (the deepest view of the zoom example).
fn last_keyframe(doc: &Document) -> Document {
    doc.animation.as_ref().and_then(|a| a.keyframes.last()).map(|k| k.doc.clone()).unwrap_or_else(|| doc.clone())
}

/// The period-3 minibrot on the real axis: a large inside the bulb test does not know.
fn minibrot() -> Document {
    let mut d = Document::default();
    d.view.center = ["-1.7548776662466927".into(), "0".into()];
    d.view.radius = 0.02.into();
    d.layers[0].max_iter = 2000;
    d
}

fn cases() -> Vec<Case> {
    let hd = (1920, 1080);
    let mut out = vec![
        Case { name: "mandelbrot", doc: Document::default(), size: hd, samples: 64 },
        Case { name: "seahorse", doc: example(include_str!("../../../docs/manual/examples/seahorse.rfr")), size: hd, samples: 64 },
        Case { name: "julia", doc: example(include_str!("../../../docs/manual/examples/julia.rfr")), size: hd, samples: 64 },
        Case { name: "minibrot", doc: minibrot(), size: hd, samples: 64 },
        Case { name: "burning-ship", doc: example(include_str!("../../../docs/manual/examples/burning-ship.rfr")), size: hd, samples: 64 },
        Case { name: "newton", doc: example(include_str!("../../../docs/manual/examples/newton.rfr")), size: hd, samples: 64 },
        Case { name: "traps", doc: example(include_str!("../../../docs/manual/examples/traps.rfr")), size: hd, samples: 64 },
        Case { name: "distance", doc: example(include_str!("../../../docs/manual/examples/distance.rfr")), size: hd, samples: 64 },
        Case { name: "layers", doc: example(include_str!("../../../docs/manual/examples/layers.rfr")), size: hd, samples: 64 },
        Case { name: "kaleidoscope", doc: example(include_str!("../../../docs/manual/examples/kaleidoscope.rfr")), size: hd, samples: 64 },
        Case { name: "deep-1e40", doc: last_keyframe(&example(include_str!("../../../docs/manual/examples/deep-zoom.rfr"))), size: hd, samples: 16 },
    ];
    // A lit layer with shadows and occlusion.
    let mut lit = Document::default();
    let l = &mut lit.layers[0].light;
    l.enabled = true;
    l.height = HeightSource::Distance;
    l.shadows = true;
    l.occlusion = 1.0;
    out.push(Case { name: "lit-shadows", doc: lit, size: hd, samples: 32 });
    // The terrain of the fly-over, then with path tracing.
    let flyover = last_keyframe(&example(include_str!("../../../docs/manual/examples/flyover.rfr")));
    out.push(Case { name: "terrain", doc: flyover.clone(), size: hd, samples: 16 });
    let mut path = flyover;
    path.layers[0].light.path_tracing = true;
    path.layers[0].light.bounces = 2;
    out.push(Case { name: "terrain-path", doc: path, size: (960, 540), samples: 64 });
    // A 3D Mandelbulb and a Buddhabrot.
    let mut bulb = Document::default();
    bulb.layers[0] = Layer { kind: LayerKind::Object, ..Default::default() };
    out.push(Case { name: "mandelbulb", doc: bulb, size: hd, samples: 32 });
    let mut buddha = Document::for_formula("rfract:Mandelbrot");
    buddha.layers[0].kind = LayerKind::Density;
    out.push(Case { name: "buddhabrot", doc: buddha, size: hd, samples: 32 });
    out
}

/// Machine load (Linux): the one-minute load average.
fn load() -> String {
    std::fs::read_to_string("/proc/loadavg").ok().and_then(|s| s.split_whitespace().next().map(str::to_string)).unwrap_or_else(|| "?".into())
}

/// Time of one GPU render of `plan` with `samples` samples (pipelines and reference orbits
/// ready before the clock starts).
fn gpu_time(r: &mut rfr_gpu::GpuRenderer, plan: &Plan, samples: u32) -> Result<Duration, String> {
    r.set_plan(plan);
    r.wait_for_orbits();
    r.wait_for_pipeline()?;
    let t = Instant::now();
    while r.samples() < samples {
        r.work(Duration::from_millis(200), samples);
        if let Some(e) = r.error() {
            return Err(e.to_string());
        }
    }
    // The image read back: the GPU has finished.
    r.read_linear().ok_or("read back failed")?;
    Ok(t.elapsed())
}

fn cpu_time(plan: &Plan, samples: u32) -> Duration {
    let t = Instant::now();
    let mut r = rfr_core::render::cpu::CpuRenderer::new(plan.clone());
    r.target_samples = samples;
    for _ in 0..samples {
        r.pass();
    }
    let _ = r.linear();
    t.elapsed()
}

/// `rfr-cli bench [documents…] [--cases a,b] [--repeat n] [--scale f] [--cpu] [--list]`.
pub fn run(args: &[String]) -> Result<(), String> {
    let (mut only, mut repeat, mut scale, mut cpu) = (None::<Vec<String>>, 3usize, 1.0f64, false);
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].clone();
        let mut next = |name: &str| {
            i += 1;
            args.get(i).cloned().ok_or_else(|| format!("{name} needs a value"))
        };
        match arg.as_str() {
            "--cases" => only = Some(next("--cases")?.split(',').map(|s| s.trim().to_string()).collect()),
            "--repeat" => repeat = next("--repeat")?.parse().map_err(|_| "bad repeat count")?,
            "--scale" => scale = next("--scale")?.parse().map_err(|_| "bad scale")?,
            "--cpu" => cpu = true,
            "--list" => {
                for c in cases() {
                    println!("{:<14} {} × {}, {} samples", c.name, c.size.0, c.size.1, c.samples);
                }
                return Ok(());
            }
            a if a.starts_with('-') => return Err(format!("unknown option '{a}'")),
            a => files.push(a.into()),
        }
        i += 1;
    }
    // Documents given: those at their own size and samples.
    let list: Vec<Case> = if files.is_empty() {
        cases().into_iter().filter(|c| only.as_ref().is_none_or(|o| o.iter().any(|n| n == c.name))).collect()
    } else {
        let mut l = Vec::new();
        for f in &files {
            let doc = rfr_core::io::open(f)?;
            let name: &'static str = Box::leak(f.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default().into_boxed_str());
            l.push(Case { name, size: (doc.image.width, doc.image.height), samples: doc.image.samples.max(1), doc });
        }
        l
    };
    if list.is_empty() {
        return Err("no such case (--list shows them)".into());
    }
    let ctx = if cpu { None } else { Some(rfr_gpu::GpuContext::new_headless().map_err(|e| format!("no GPU: {e}"))?) };
    let device = ctx.as_ref().map_or("the CPU".to_string(), |c| c.adapter_name.clone());
    println!("{} · {device} · {repeat} runs, median · load {}", rfr_core::name_version(), load());
    let mut total = 0.0;
    for c in list {
        let (w, h) = (((c.size.0 as f64 * scale).round() as u32).max(16), ((c.size.1 as f64 * scale).round() as u32).max(16));
        let plan = Plan::sized(&c.doc, w, h)?;
        let mut times = Vec::new();
        let mut r = ctx.clone().map(rfr_gpu::GpuRenderer::new);
        if let Some(r) = r.as_mut() {
            r.set_band_target(Duration::from_millis(40));
            // Compiles the shaders.
            gpu_time(r, &plan, 1)?;
        }
        for _ in 0..repeat.max(1) {
            times.push(match r.as_mut() {
                Some(r) => gpu_time(r, &plan, c.samples)?,
                None => cpu_time(&plan, c.samples),
            });
        }
        times.sort();
        let med = times[times.len() / 2].as_secs_f64();
        let spread = (times[times.len() - 1].as_secs_f64() - times[0].as_secs_f64()) / med.max(1e-9) * 100.0;
        total += med;
        let mode = ctx.as_ref().map(|_| format!("{:?}", rfr_gpu::renderer::renderer_mode(&plan))).unwrap_or_default();
        println!("{:<14} {:>5} × {:<5} {:>4} spp  {:>8.3} s  ±{:>3.0} %  {mode}", c.name, w, h, c.samples, med, spread / 2.0);
    }
    println!("{:<14} {:>36.3} s  · load {}", "total", total, load());
    Ok(())
}
