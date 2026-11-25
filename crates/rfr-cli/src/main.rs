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
  formulas                   List the built-in formulas with their ids
  formulas --check <path>…   Compile formula files (.ufm, .ucl, .uxf, .frm) or folders and report
  info                       Program version, GPU, denoiser and settings folders
  help                       This text

Options:
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

fn render(args: &[String]) -> Result<(), String> {
    let mut input = None;
    let mut output = None;
    let mut cpu = false;
    let (mut width, mut height, mut samples) = (None, None, None);
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-o" | "--output" => output = Some(PathBuf::from(value(args, &mut i, "--output")?)),
            "-W" | "--width" => width = Some(number::<u32>(value(args, &mut i, "--width")?, "--width")?),
            "-H" | "--height" => height = Some(number::<u32>(value(args, &mut i, "--height")?, "--height")?),
            "-s" | "--samples" => samples = Some(number::<u32>(value(args, &mut i, "--samples")?, "--samples")?),
            "--cpu" => cpu = true,
            a if a.starts_with('-') => return Err(format!("unknown option '{a}'")),
            a => input = Some(a.to_string()),
        }
        i += 1;
    }
    let input = input.ok_or("render needs an input (document, PNG or formula id)")?;
    let path = Path::new(&input);
    let doc = if path.exists() {
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
        let stem = if path.exists() {
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
        eprintln!("note: the view is deeper than double precision reaches; expect rounding steps");
    }
    let mut how = "on the CPU".to_string();
    let mut rgba = None;
    if !cpu {
        match rfr_gpu::GpuContext::new_headless() {
            Ok(ctx) => {
                let name = ctx.adapter_name.clone();
                let mode = rfr_gpu::renderer::renderer_mode(&plan);
                match rfr_gpu::render_blocking(ctx, &plan, s, |_| true) {
                    Ok(lin) => {
                        rgba = Some(rfr_core::render::cpu::rgba8(&lin));
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
        None => rfr_core::render::cpu::render(&doc, w, h, s)?.rgba8(),
    };
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
        let state = match &e.program {
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

/// Compiles every formula file below `paths` and prints a summary with the failure reasons.
fn check_formulas(paths: &[String]) -> Result<(), String> {
    let mut files = Vec::new();
    for p in paths {
        collect(Path::new(p), &mut files);
    }
    if files.is_empty() {
        return Err("no .ufm or .frm files found".into());
    }
    files.sort();
    let (mut ok, mut failed) = (0usize, 0usize);
    let mut per_kind: std::collections::BTreeMap<&str, (usize, usize)> = Default::default();
    let mut reasons: std::collections::BTreeMap<String, usize> = Default::default();
    for f in &files {
        let Ok(bytes) = std::fs::read(f) else { continue };
        let text = String::from_utf8_lossy(&bytes);
        let ext = f.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        let (kind, label) = match ext.as_str() {
            "ucl" => (rfr_core::formula::ProgramKind::Coloring, "colorings"),
            "uxf" => (rfr_core::formula::ProgramKind::Transform, "transformations"),
            _ => (rfr_core::formula::ProgramKind::Fractal, "fractal formulas"),
        };
        let results = if text.contains("@language: frm-like/1") {
            vec![(f.display().to_string(), rfr_core::formula::compile_frm(&text))]
        } else {
            rfr_core::formula::compile_ufm_file_kind(&text, kind)
        };
        for (_, r) in results {
            let e = per_kind.entry(label).or_default();
            match r {
                Ok(_) => {
                    ok += 1;
                    e.0 += 1;
                }
                Err(err) => {
                    failed += 1;
                    e.1 += 1;
                    // Group reasons without names and numbers.
                    let key: String = err.msg.split('\'').enumerate().map(|(i, s)| if i % 2 == 1 { "…" } else { s }).collect::<Vec<_>>().join("'");
                    *reasons.entry(key).or_default() += 1;
                }
            }
        }
    }
    let total = ok + failed;
    println!("{} files, {total} entries: {ok} compile ({:.1} %), {failed} fail", files.len(), 100.0 * ok as f64 / total.max(1) as f64);
    for (label, (o, f)) in &per_kind {
        println!("  {label}: {o} of {} compile ({:.1} %)", o + f, 100.0 * *o as f64 / (o + f).max(1) as f64);
    }
    let mut top: Vec<_> = reasons.into_iter().collect();
    top.sort_by_key(|t| std::cmp::Reverse(t.1));
    for (msg, n) in top.iter().take(25) {
        println!("{n:>6}  {msg}");
    }
    Ok(())
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
    let args: Vec<String> = std::env::args().skip(1).collect();
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
