// SPDX-License-Identifier: LGPL-3.0-or-later

//! `rfr-cli script`: runs a Lua script on a document.

use std::path::PathBuf;
use std::time::Instant;

/// `rfr-cli script <file.lua> [input] [-o out.rfr] [--param name=value]… [--cpu]`, or
/// `--info <file.lua>`, `--api`, `--examples <dir>`.
pub fn run(args: &[String]) -> Result<(), String> {
    let (mut script, mut input, mut output, mut given, mut info, mut cpu) = (None, None, None, Vec::new(), false, false);
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].clone();
        let mut next = |name: &str| {
            i += 1;
            args.get(i).cloned().ok_or_else(|| format!("{name} needs a value"))
        };
        match arg.as_str() {
            "--api" => {
                print!("{}", rfr_script::API_REFERENCE);
                return Ok(());
            }
            "--examples" => {
                let dir = PathBuf::from(next("--examples")?);
                std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
                for (name, src) in rfr_script::EXAMPLES {
                    let p = dir.join(name);
                    if p.exists() {
                        eprintln!("{} exists, skipped", p.display());
                    } else {
                        std::fs::write(&p, src).map_err(|e| format!("cannot write {}: {e}", p.display()))?;
                        eprintln!("{}", p.display());
                    }
                }
                return Ok(());
            }
            "--info" => info = true,
            "--cpu" => cpu = true,
            "-o" | "--output" => output = Some(PathBuf::from(next("--output")?)),
            "--param" => {
                let v = next("--param")?;
                let (k, val) = v.split_once('=').ok_or_else(|| format!("--param expects name=value, got '{v}'"))?;
                given.push((k.trim().to_string(), val.trim().to_string()));
            }
            a if a.starts_with('-') => return Err(format!("unknown option '{a}'")),
            a if script.is_none() => script = Some(PathBuf::from(a)),
            a => input = Some(PathBuf::from(a)),
        }
        i += 1;
    }
    let script = script.ok_or("script needs a .lua file")?;
    let source = std::fs::read_to_string(&script).map_err(|e| format!("cannot read {}: {e}", script.display()))?;
    let header = rfr_script::Header::parse(&source);
    for w in &header.warnings {
        eprintln!("warning: {}: {w}", script.display());
    }
    if info {
        println!("{}", if header.title.is_empty() { "(no title)" } else { &header.title });
        if !header.description.is_empty() {
            println!("{}", header.description);
        }
        for p in &header.params {
            let range = match &p.kind {
                rfr_script::ParamKind::Number { min, max } => format!("number {min}..{max}"),
                rfr_script::ParamKind::Integer { min, max } => format!("integer {min}..{max}"),
                rfr_script::ParamKind::Bool => "bool".to_string(),
                rfr_script::ParamKind::Choice(o) => format!("choice {}", o.join("|")),
            };
            println!("  {} = {}  ({range})  {}", p.name, p.default, p.help);
        }
        return Ok(());
    }
    let params = header.values(&given)?;
    let doc = match &input {
        Some(p) => rfr_core::io::open(p)?,
        None => rfr_core::doc::Document::default(),
    };
    let name = script.file_name().map_or("script".into(), |n| n.to_string_lossy().to_string());
    let mut run = rfr_script::RunInput::new(doc);
    run.params = params;
    // Paths are relative to the document's folder, else the script's.
    run.base_dir = input.as_ref().and_then(|p| p.parent()).or(script.parent()).map(|p| p.to_path_buf());
    run.on_print = Some(Box::new(|line: &str| println!("{line}")));
    if !cpu {
        run.gpu = rfr_gpu::GpuContext::new_headless().ok();
    }
    let t = Instant::now();
    let out = rfr_script::run(&source, &name, run).map_err(|e| format!("error: {e}"))?;
    eprintln!("finished in {:.2} s", t.elapsed().as_secs_f64());
    if !out.queued.is_empty() {
        let file = rfr_core::paths::config_dir().join("queue.json");
        let mut q = rfr_core::queue::Queue::load(&file).unwrap_or_default();
        let n = out.queued.len();
        q.jobs.extend(out.queued);
        q.save(&file)?;
        eprintln!("{n} job(s) added to the render queue ({}; rfr-cli batch renders it)", file.display());
    }
    match output {
        Some(p) => {
            rfr_core::io::save(&out.doc, &p)?;
            eprintln!("document written to {}", p.display());
        }
        None if out.changed => eprintln!("the document changed; -o <file.rfr> saves it"),
        None => {}
    }
    Ok(())
}
