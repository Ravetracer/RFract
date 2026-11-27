// SPDX-License-Identifier: LGPL-3.0-or-later

//! rfr-cli: RFract on the command line.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use rfr_core::doc::Document;
use rfr_core::formula::library::library;

const USAGE: &str = "\
Usage: rfr-cli <command> [options]

Commands:
  render <input> [options]   Render a document to PNG. <input> is an .rfr file, a PNG
                             rendered by RFract or a formula id (see 'formulas').
      -o, --output <file>    Output PNG (default: <input name>.png)
      -W, --width <px>       Image width (default: the document's)
      -H, --height <px>      Image height (default: the document's)
      -s, --samples <n>      Samples per pixel (default: the document's)
      --cpu                  Render on the CPU (the reference renderer)
      --entry <name>         The image of a parameter file (.upr, .par; default: the first)
  animate <input.rfr> [options]
                             Render the document's animation: a movie through ffmpeg (.mp4,
                             .mov, .webm, .gif) or numbered PNG files in a folder
      -o, --output <path>    Movie file or folder (default: <input name>.mp4)
      -W, -H, -s             Size and samples per pixel (default: the document's)
      --format <name>        mp4, hevc, av1, prores, webm, gif or png (default: by extension)
      --from <n>, --to <n>   First and last frame (default: all)
      --cpu                  Render on the CPU
      --ffmpeg <program>     The ffmpeg program (default: found on the search path)
  formulas                   List the built-in formulas with their ids
  formulas --check <path>…   Compile formula files (.ufm, .ucl, .uxf, .frm) or folders and report
  params --check <path>… [--thumbs <dir>] [--notes]
                             Convert parameter files (.upr, .par) and report what does not carry over
  info                       Program version, GPU, denoiser and settings folders
  help                       This text

Options:
  --formula-dir <dir>        Also use the formula files in <dir> (repeatable; ids
                             user:<path in dir>:<entry>, as in the app's formula folders)
  -V, --version              Print the version";

fn info() {
    println!("{}", rfr_core::name_version());
    match rfr_gpu::detect_adapter() {
        Ok(a) => println!("GPU:       {a}"),
        Err(e) => println!("GPU:       none ({e}); rendering will use the CPU"),
    }
    match rfr_core::render::oidn::library() {
        Ok(p) => println!("Denoiser:  {}", p.display()),
        Err(e) => println!("Denoiser:  not available ({e})"),
    }
    let lib = library();
    println!("Formulas:  {} built in ({} compile)", lib.entries.len(), lib.usable().count());
    println!("Settings:  {}", rfr_core::paths::config_dir().display());
    println!("Caches:    {}", rfr_core::paths::cache_dir().display());
}

fn value<'a>(args: &'a [String], i: &mut usize, name: &str) -> Result<&'a str, String> {
    *i += 1;
    args.get(*i).map(String::as_str).ok_or_else(|| format!("{name} needs a value"))
}

fn number<T: std::str::FromStr>(s: &str, name: &str) -> Result<T, String> {
    s.parse().map_err(|_| format!("{name}: '{s}' is not a valid number"))
}

fn animate(args: &[String]) -> Result<(), String> {
    use rfr_core::video::{VideoFormat, VideoSink};
    let (mut input, mut output, mut format, mut ffmpeg) = (None, None, None, None);
    let (mut width, mut height, mut samples, mut from, mut to) = (None, None, None, None, None);
    let mut cpu = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-o" | "--output" => output = Some(PathBuf::from(value(args, &mut i, "--output")?)),
            "-W" | "--width" => width = Some(number::<u32>(value(args, &mut i, "--width")?, "--width")?),
            "-H" | "--height" => height = Some(number::<u32>(value(args, &mut i, "--height")?, "--height")?),
            "-s" | "--samples" => samples = Some(number::<u32>(value(args, &mut i, "--samples")?, "--samples")?),
            "--from" => from = Some(number::<usize>(value(args, &mut i, "--from")?, "--from")?),
            "--to" => to = Some(number::<usize>(value(args, &mut i, "--to")?, "--to")?),
            "--ffmpeg" => ffmpeg = Some(PathBuf::from(value(args, &mut i, "--ffmpeg")?)),
            "--format" => {
                let f = value(args, &mut i, "--format")?.to_lowercase();
                format = Some(match f.as_str() {
                    "mp4" | "h264" => VideoFormat::Mp4,
                    "hevc" | "h265" => VideoFormat::Hevc,
                    "av1" => VideoFormat::Av1,
                    "prores" | "mov" => VideoFormat::ProRes,
                    "webm" | "vp9" => VideoFormat::WebM,
                    "gif" => VideoFormat::Gif,
                    "png" => VideoFormat::PngSequence,
                    other => return Err(format!("unknown format '{other}'")),
                });
            }
            "--cpu" => cpu = true,
            a if a.starts_with('-') => return Err(format!("unknown option '{a}'")),
            a => input = Some(PathBuf::from(a)),
        }
        i += 1;
    }
    let input = input.ok_or("animate needs a document with an animation")?;
    let doc = rfr_core::io::open(&input)?;
    let anim = doc.animation.as_deref().ok_or_else(|| format!("{} has no animation", input.display()))?;
    let plan = rfr_core::animation::AnimationPlan::new(anim)?;
    let output = output.unwrap_or_else(|| input.with_extension("mp4"));
    let format = format.unwrap_or_else(|| VideoFormat::from_path(&output));
    let ffmpeg = rfr_core::video::ffmpeg_program(ffmpeg.as_deref());
    if format.needs_ffmpeg() && !rfr_core::video::ffmpeg_available(&ffmpeg) {
        return Err(format!("{} needs ffmpeg, which was not found ({}); use --format png or --ffmpeg <program>", format.label(), ffmpeg.display()));
    }
    let w = width.unwrap_or(doc.image.width).max(1);
    let h = height.unwrap_or(doc.image.height).max(1);
    let s = samples.unwrap_or(doc.image.samples).max(1);
    let last = plan.frame_count().saturating_sub(1);
    let (first, last) = (from.unwrap_or(0).min(last), to.unwrap_or(last).min(last));
    let ctx = if cpu {
        None
    } else {
        match rfr_gpu::GpuContext::new_headless() {
            Ok(c) => Some(c),
            Err(e) => {
                eprintln!("{e}; rendering on the CPU");
                None
            }
        }
    };
    let how = ctx.as_ref().map_or("the CPU".to_string(), |c| c.adapter_name.clone());
    let mut renderer = rfr_gpu::animation::FrameRenderer::new(ctx);
    let mut sink = VideoSink::new(&output, format, w, h, plan.fps(), &ffmpeg, None)?;
    let t = Instant::now();
    let count = last + 1 - first;
    for (k, f) in (first..=last).enumerate() {
        let (rgba, note) = renderer.finished(&plan, f, w, h, s)?;
        sink.write_frame(k, &rgba)?;
        let done = k + 1;
        let left = t.elapsed().as_secs_f64() / done as f64 * (count - done) as f64;
        eprint!("\rframe {f} ({done} of {count}){} · {left:.0} s left   ", note.map(|n| format!(", {n}")).unwrap_or_default());
    }
    eprintln!();
    sink.finish()?;
    println!(
        "rendered {count} frames of {w} × {h}, {s} samples per pixel{} on {how} in {:.1} s → {}",
        if plan.shutter() > 0.0 { format!(", motion blur {:.0}°", plan.shutter() * 360.0) } else { String::new() },
        t.elapsed().as_secs_f64(),
        output.display()
    );
    Ok(())
}

fn render(args: &[String]) -> Result<(), String> {
    let mut input = None;
    let mut output = None;
    let mut cpu = false;
    let mut entry: Option<String> = None;
    let (mut width, mut height, mut samples) = (None, None, None);
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-o" | "--output" => output = Some(PathBuf::from(value(args, &mut i, "--output")?)),
            "-W" | "--width" => width = Some(number::<u32>(value(args, &mut i, "--width")?, "--width")?),
            "-H" | "--height" => height = Some(number::<u32>(value(args, &mut i, "--height")?, "--height")?),
            "-s" | "--samples" => samples = Some(number::<u32>(value(args, &mut i, "--samples")?, "--samples")?),
            "--cpu" => cpu = true,
            "--entry" => entry = Some(value(args, &mut i, "--entry")?.to_string()),
            a if a.starts_with('-') => return Err(format!("unknown option '{a}'")),
            a => input = Some(a.to_string()),
        }
        i += 1;
    }
    let input = input.ok_or("render needs an input (document, PNG, parameter file or formula id)")?;
    let path = Path::new(&input);
    let is_upr = rfr_core::io::is_parameter_file(path);
    let doc = if path.exists() && is_upr {
        let images = rfr_core::io::read_parameter_file(path)?;
        let im = match &entry {
            Some(name) => images.iter().find(|i| i.name.eq_ignore_ascii_case(name)).ok_or_else(|| {
                format!("no image '{name}' in {}; it has: {}", path.display(), images.iter().map(|i| i.name.as_str()).collect::<Vec<_>>().join(", "))
            })?,
            None => &images[0],
        };
        for n in &im.notes {
            eprintln!("note: {n}");
        }
        im.doc.clone()
    } else if path.exists() {
        rfr_core::io::open(path)?
    } else if library().get(&input).is_some() {
        Document::for_formula(&input)
    } else {
        return Err(format!("'{input}' is neither a file nor a formula id (see 'rfr-cli formulas')"));
    };
    let w = width.unwrap_or(doc.image.width).max(1);
    let h = height.unwrap_or(doc.image.height).max(1);
    let s = samples.unwrap_or(doc.image.samples).max(1);
    let output = output.unwrap_or_else(|| {
        let stem = if path.exists() && is_upr {
            let base = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or("render".into());
            format!("{base}-{}", entry.clone().unwrap_or_else(|| "1".into()).replace(['/', '\\', ' '], "_"))
        } else if path.exists() {
            path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or("render".into())
        } else {
            input.replace([':', '/'], "_")
        };
        PathBuf::from(format!("{stem}.png"))
    });
    if output.exists() && output.canonicalize().ok() == path.canonicalize().ok() {
        return Err(format!("refusing to overwrite the input {}", output.display()));
    }
    let t = Instant::now();
    let plan = rfr_core::render::Plan::sized(&doc, w, h)?;
    if plan.precision() == rfr_core::render::Precision::Beyond {
        for lp in plan.layers() {
            if let Err(e) = rfr_core::formula::perturb::Perturbation::new(&lp.fractal.program, &lp.fractal.values, !lp.julia, lp.seed) {
                eprintln!("note: deeper than double precision; {} cannot use perturbation ({e}) and shows rounding steps", lp.fractal.id);
            } else if !lp.transforms.is_empty() {
                eprintln!("note: deeper than double precision; {} has transformations and shows rounding steps", lp.fractal.id);
            }
        }
    }
    let mut how = "on the CPU".to_string();
    let mut rgba = None;
    if !cpu {
        match rfr_gpu::GpuContext::new_headless() {
            Ok(ctx) => {
                let name = ctx.adapter_name.clone();
                let mode = rfr_gpu::renderer::renderer_mode(&plan);
                match rfr_gpu::render_blocking(ctx.clone(), &plan, s, |_| true) {
                    Ok(lin) => {
                        let aux = if doc.post.denoise { rfr_gpu::render_aux(&doc, w, h, s, Some(&ctx)) } else { None };
                        rgba = Some(rfr_core::render::post::finish_export(lin, w, h, &doc.post, aux.as_ref()));
                        how = format!("on {name} ({mode:?})");
                    }
                    Err(e) => eprintln!("GPU render failed ({e}); rendering on the CPU"),
                }
            }
            Err(e) => eprintln!("{e}; rendering on the CPU"),
        }
    }
    let rgba = match rgba {
        Some(v) => v,
        None => {
            let aux = if doc.post.denoise { rfr_gpu::render_aux(&doc, w, h, s, None) } else { None };
            rfr_core::render::post::finish_export(rfr_core::render::cpu::render(&doc, w, h, s)?.linear(), w, h, &doc.post, aux.as_ref())
        }
    };
    let (rgba, note) = rgba;
    if let Some(n) = note {
        how = format!("{how}, {n}");
    }
    let mut saved = doc.clone();
    saved.image.width = w;
    saved.image.height = h;
    saved.image.samples = s;
    rfr_core::io::png::write_rgba8(&output, w, h, &rgba, Some(&saved))?;
    println!("rendered {w} × {h}, {s} samples per pixel {how} in {:.2} s → {}", t.elapsed().as_secs_f64(), output.display());
    Ok(())
}

fn list_formulas() {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    for e in &library().entries {
        let state = match &e.program() {
            Ok(p) if p.has_julia => "  mandelbrot/julia",
            Ok(_) => "",
            Err(_) => "  (does not compile)",
        };
        // Stop quietly when the reader goes away (rfr-cli formulas | head).
        if writeln!(out, "{:<48} {:<28} {}{state}", e.id, e.title, e.family).is_err() {
            return;
        }
    }
}

/// Converts every image of the parameter files (`.upr`, `.par`) below `paths`; reports how many convert without
/// notes and build a render plan, and the notes by frequency. `--thumbs <dir>` renders a
/// small picture of each image there.
fn check_upr(args: &[String]) -> Result<(), String> {
    let mut paths = Vec::new();
    let mut thumbs: Option<PathBuf> = None;
    let mut all_notes = false;
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--notes" {
            all_notes = true;
        } else if args[i] == "--thumbs" {
            thumbs = Some(PathBuf::from(value(args, &mut i, "--thumbs")?));
        } else {
            paths.push(PathBuf::from(&args[i]));
        }
        i += 1;
    }
    let mut files = Vec::new();
    for p in &paths {
        if p.is_dir() {
            if let Ok(rd) = std::fs::read_dir(p) {
                files.extend(rd.flatten().map(|e| e.path()).filter(|f| rfr_core::io::is_parameter_file(f)));
            }
        } else {
            files.push(p.clone());
        }
    }
    files.sort();
    if let Some(d) = &thumbs {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    let gpu = if thumbs.is_some() { rfr_gpu::GpuContext::new_headless().ok() } else { None };
    let (mut total, mut clean, mut planned) = (0, 0, 0);
    let mut reasons: std::collections::BTreeMap<String, usize> = Default::default();
    for f in &files {
        let images = match rfr_core::io::read_parameter_file(f) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("{e}");
                continue;
            }
        };
        for (k, im) in images.iter().enumerate() {
            total += 1;
            if im.notes.is_empty() {
                clean += 1;
            }
            for n in &im.notes {
                if all_notes {
                    println!("{}\t{}\t{n}", f.display(), im.name);
                }
                let key: String = n.split('\'').enumerate().map(|(i, s)| if i % 2 == 1 { "…" } else { s }).collect::<Vec<_>>().join("'");
                let key = key
                    .split(" is not in your formula folders")
                    .next()
                    .map(|s| {
                        if n.contains(" is not in your formula folders") {
                            "a formula is not in the formula folders".to_string()
                        } else {
                            s.to_string()
                        }
                    })
                    .unwrap_or(key);
                *reasons.entry(key).or_default() += 1;
            }
            let aspect = im.doc.image.height as f64 / im.doc.image.width.max(1) as f64;
            let (w, h) = (240, ((240.0 * aspect).round() as u32).max(16));
            match rfr_core::render::Plan::sized(&im.doc, w, h) {
                Ok(plan) => {
                    planned += 1;
                    if let (Some(dir), Some(ctx)) = (&thumbs, &gpu) {
                        let name = format!(
                            "{}-{k:03}-{}.png",
                            f.file_stem().unwrap_or_default().to_string_lossy(),
                            im.name.replace(['/', '\\', ' ', ':'], "_")
                        );
                        let out = dir.join(name);
                        match rfr_gpu::render_blocking(ctx.clone(), &plan, 4, |_| true) {
                            Ok(lin) => {
                                let rgba = rfr_core::render::post::finish(&lin, w, h, &im.doc.post);
                                let _ = rfr_core::io::png::write_rgba8(&out, w, h, &rgba, Some(&im.doc));
                            }
                            Err(e) => eprintln!("{}: {e}", out.display()),
                        }
                    }
                }
                Err(e) => {
                    if all_notes {
                        println!("{}\t{}\tplan: {e}", f.display(), im.name);
                    }
                    *reasons.entry(format!("plan: {}", e.split(':').next().unwrap_or(&e))).or_default() += 1;
                }
            }
        }
    }
    println!(
        "{} files, {total} images: {clean} without notes ({:.1} %), {planned} render ({:.1} %)",
        files.len(),
        100.0 * clean as f64 / total.max(1) as f64,
        100.0 * planned as f64 / total.max(1) as f64
    );
    let mut list: Vec<_> = reasons.into_iter().collect();
    list.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    for (r, n) in list.iter().take(25) {
        println!("  {n:6}  {r}");
    }
    Ok(())
}

/// Compiles every formula file below `paths` and prints a summary with the failure reasons.
/// Entries that use classes (`class`, `import`, `new`) are counted separately from classic ones.
/// `--failures <file>` writes every failure (file, entry, error) to a text file.
fn check_formulas(args: &[String]) -> Result<(), String> {
    let mut paths = Vec::new();
    let mut failures_out = None;
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--failures" {
            failures_out = Some(PathBuf::from(value(args, &mut i, "--failures")?));
        } else {
            paths.push(args[i].clone());
        }
        i += 1;
    }
    let mut files = Vec::new();
    for p in &paths {
        collect(Path::new(p), &mut files);
    }
    if files.is_empty() {
        return Err("no .ufm, .ucl, .uxf or .frm files found".into());
    }
    files.sort();
    // Classes for formulas that use objects: the class libraries below the checked paths.
    let folders: Vec<PathBuf> =
        paths.iter().map(PathBuf::from).map(|p| if p.is_dir() { p } else { p.parent().map(Path::to_path_buf).unwrap_or_default() }).collect();
    let registry = rfr_core::formula::library::ClassRegistry::from_folders(&folders);
    eprintln!("{} classes in the class libraries ({} do not parse)", registry.len(), registry.errors.len());
    // (kind, classic) -> (ok, failed)
    let mut counts: std::collections::BTreeMap<(&str, bool), (usize, usize)> = Default::default();
    // (kind) -> (shader valid and CPU run fine, failed)
    let mut renders: std::collections::BTreeMap<&str, (usize, usize)> = Default::default();
    let mut reasons: std::collections::BTreeMap<String, usize> = Default::default();
    let mut list = String::new();
    for f in &files {
        let Ok(bytes) = std::fs::read(f) else { continue };
        let text = String::from_utf8_lossy(&bytes);
        let ext = f.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        let (kind, label) = match ext.as_str() {
            "ucl" => (rfr_core::formula::ProgramKind::Coloring, "colorings"),
            "uxf" => (rfr_core::formula::ProgramKind::Transform, "transformations"),
            _ => (rfr_core::formula::ProgramKind::Fractal, "fractal formulas"),
        };
        #[allow(clippy::type_complexity)]
        let results: Vec<(String, bool, Result<_, rfr_core::formula::Error>)> = if text.contains("@language: frm-like/1") {
            vec![(String::new(), true, rfr_core::formula::compile_frm(&text))]
        } else {
            let pre = rfr_core::formula::parse::preprocess(&text);
            rfr_core::formula::parse::split_entries(&pre)
                .iter()
                .map(|e| {
                    let lower = e.body.to_ascii_lowercase();
                    let classic = !(lower.contains("import \"") || lower.contains("\nclass ") || lower.contains("= new ") || lower.contains("=new "));
                    (
                        e.name.clone(),
                        classic,
                        rfr_core::formula::parse::parse_ufm_entry(e).and_then(|a| {
                            if a.imports.is_empty() && !a.params.iter().any(|p| p.kind == rfr_core::formula::ParamKind::Class) {
                                return rfr_core::formula::check::check_kind(&a, kind);
                            }
                            let lookup = |n: &str| registry.get(n);
                            let choices = Default::default();
                            let ctx = rfr_core::formula::check::ClassCtx { lookup: &lookup, choices: &choices };
                            rfr_core::formula::check::check_with(&a, kind, Some(&ctx))
                        }),
                    )
                })
                .collect()
        };
        for (name, classic, r) in results {
            let e = counts.entry((label, classic)).or_default();
            match r {
                Ok(prog) => {
                    e.0 += 1;
                    let rr = renders.entry(label).or_default();
                    match render_check(prog, kind) {
                        Ok(()) => rr.0 += 1,
                        Err(why) => {
                            rr.1 += 1;
                            list.push_str(&format!("{}\t{name}\trender\t{why}\n", f.display()));
                        }
                    }
                }
                Err(err) => {
                    e.1 += 1;
                    if classic {
                        // Group reasons without names and numbers.
                        let key: String =
                            err.msg.split('\'').enumerate().map(|(i, s)| if i % 2 == 1 { "…" } else { s }).collect::<Vec<_>>().join("'");
                        *reasons.entry(key).or_default() += 1;
                    }
                    list.push_str(&format!("{}\t{name}\t{}\t{err}\n", f.display(), if classic { "classic" } else { "classes" }));
                }
            }
        }
    }
    let (ok, failed) = counts.values().fold((0, 0), |a, c| (a.0 + c.0, a.1 + c.1));
    println!(
        "{} files, {} entries: {ok} compile ({:.1} %), {failed} fail",
        files.len(),
        ok + failed,
        100.0 * ok as f64 / (ok + failed).max(1) as f64
    );
    for ((label, classic), (o, f)) in &counts {
        println!(
            "  {label} ({}): {o} of {} compile ({:.1} %)",
            if *classic { "classic" } else { "with classes" },
            o + f,
            100.0 * *o as f64 / (o + f).max(1) as f64
        );
    }
    for (label, (o, f)) in &renders {
        println!("  {label} that compile and render (valid shader, CPU run): {o} of {}", o + f);
    }
    println!("Reasons (classic entries):");
    let mut top: Vec<_> = reasons.into_iter().collect();
    top.sort_by_key(|t| std::cmp::Reverse(t.1));
    for (msg, n) in top.iter().take(30) {
        println!("{n:>6}  {msg}");
    }
    if let Some(p) = failures_out {
        std::fs::write(&p, list).map_err(|e| format!("{}: {e}", p.display()))?;
        println!("failures written to {}", p.display());
    }
    Ok(())
}

/// Puts a compiled program into a layer (with the built-in Mandelbrot for colorings and
/// transformations), validates the layer's shader and runs it on the CPU at a few points.
fn render_check(prog: rfr_core::formula::Program, kind: rfr_core::formula::ProgramKind) -> Result<(), String> {
    use rfr_core::formula::ProgramKind;
    use rfr_core::render::{ColorMethod, ColorPlan, LayerPlan, Prog};
    let prog = std::sync::Arc::new(prog);
    let values: Vec<_> = prog.params.iter().map(|p| p.default.clone()).collect();
    let mine = Prog { id: "check".into(), program: prog.clone(), values: values.clone() };
    let mandel = Prog::resolve("rfract:Mandelbrot", &Default::default(), ProgramKind::Fractal)?;
    let color = |method| ColorPlan {
        method,
        lut: vec![[0.5; 4]; 1024],
        transfer: rfr_core::doc::Transfer::Linear,
        density: 1.0,
        offset: 0.0,
        solid: [0.0, 0.0, 0.0, 1.0],
    };
    let lp = match kind {
        ProgramKind::Fractal => LayerPlan {
            fractal: mine,
            max_iter: 50,
            julia: false,
            seed: [0.0; 2],
            transforms: vec![],
            outside: color(ColorMethod::Smooth),
            inside: color(ColorMethod::Solid),
            density: None,
            light: None,
            object: None,
        },
        ProgramKind::Coloring => LayerPlan {
            fractal: mandel,
            max_iter: 50,
            julia: false,
            seed: [0.0; 2],
            transforms: vec![],
            outside: color(ColorMethod::Formula(mine.clone())),
            inside: color(ColorMethod::Formula(mine)),
            density: None,
            light: None,
            object: None,
        },
        ProgramKind::Transform => LayerPlan {
            fractal: mandel,
            max_iter: 50,
            julia: false,
            seed: [0.0; 2],
            transforms: vec![mine],
            outside: color(ColorMethod::Smooth),
            inside: color(ColorMethod::Solid),
            density: None,
            light: None,
            object: None,
        },
    };
    let src = rfr_gpu::codegen::kernel(&lp, rfr_gpu::codegen::Mode::Plain);
    let module = naga::front::wgsl::parse_str(&src).map_err(|e| format!("shader: {}", e.message()))?;
    naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty()).validate(&module).map_err(|e| {
        let mut msg = format!("shader: {}", e.as_inner());
        let mut src: Option<&dyn std::error::Error> = std::error::Error::source(e.as_inner());
        while let Some(s) = src {
            msg.push_str(&format!(": {s}"));
            src = s.source();
        }
        msg
    })?;
    // CPU: a small render of the layer (panics count as failures).
    let doc_free = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let plan = rfr_core::render::Plan {
            frame: rfr_core::render::ViewFrame { width: 4, height: 3, center: [-0.5, 0.0], radius: 1.5, radius_fx: 1.5.into(), cos: 1.0, sin: 0.0 },
            center_text: ["-0.5".into(), "0".into()],
            background: [0.0, 0.0, 0.0, 1.0],
            nodes: vec![rfr_core::render::Node::Layer {
                layer: Box::new(lp),
                blend: rfr_core::render::Blend { mode: Default::default(), opacity: 1.0 },
                mask: None,
            }],
        };
        let mut r = rfr_core::render::cpu::CpuRenderer::new(plan);
        r.pass();
    }));
    doc_free.map_err(|e| {
        format!(
            "CPU run panicked: {}",
            e.downcast_ref::<String>().cloned().or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default()
        )
    })
}

fn collect(p: &Path, out: &mut Vec<PathBuf>) {
    if p.is_dir() {
        if let Ok(rd) = std::fs::read_dir(p) {
            for e in rd.flatten() {
                collect(&e.path(), out);
            }
        }
    } else if p.extension().is_some_and(|e| ["ufm", "ucl", "uxf", "frm"].iter().any(|x| e.eq_ignore_ascii_case(x))) {
        out.push(p.to_path_buf());
    }
}

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    // Formula folders apply to every command.
    let mut dirs = Vec::new();
    while let Some(i) = args.iter().position(|a| a == "--formula-dir") {
        if i + 1 >= args.len() {
            eprintln!("error: --formula-dir needs a folder");
            return ExitCode::FAILURE;
        }
        dirs.push(PathBuf::from(args.remove(i + 1)));
        args.remove(i);
    }
    if !dirs.is_empty() {
        let n = rfr_core::formula::library::set_user_folders(&dirs);
        eprintln!("{n} formulas in the formula folders");
    }
    let rest = args.get(1..).unwrap_or(&[]);
    let result = match args.first().map(String::as_str) {
        None | Some("help" | "-h" | "--help") => {
            println!("{}\n\n{USAGE}", rfr_core::name_version());
            Ok(())
        }
        Some("-V" | "--version" | "version") => {
            println!("{}", rfr_core::name_version());
            Ok(())
        }
        Some("info") => {
            info();
            Ok(())
        }
        Some("render") => render(rest),
        Some("animate") => animate(rest),
        Some("params" | "upr") if rest.first().map(String::as_str) == Some("--check") => check_upr(&rest[1..]),
        Some("formulas") if rest.first().map(String::as_str) == Some("--check") => check_formulas(&rest[1..]),
        Some("formulas") => {
            list_formulas();
            Ok(())
        }
        Some(other) => Err(format!("unknown command '{other}'\n\n{USAGE}")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("rfr-cli: {e}");
            ExitCode::from(2)
        }
    }
}
