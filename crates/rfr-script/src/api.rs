// SPDX-License-Identifier: LGPL-3.0-or-later

//! The Lua API: the `rfr` module, the global `doc` and `params`.
//!
//! Documents are plain Lua tables with the fields of an `.rfr` file (`doc.view.radius`,
//! `doc.layers[1].formula`, …); the functions that take a document read the table as a
//! document and report fields that do not fit.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use mlua::serde::SerializeOptions;
use mlua::{Lua, LuaSerdeExt, Result, Table, Value, Variadic};
use rfr_core::doc::{Document, View};
use rfr_core::formula::ProgramKind;
use rfr_core::queue::{JobWhat, QueueJob};

use crate::ParamValue;

/// Reference of the Lua API: the manual's scripting chapter (help, `rfr-cli script --api`).
pub const API_REFERENCE: &str = include_str!("../../../docs/manual/14-scripting.md");

/// Receiver of `print` lines.
pub type PrintFn = Box<dyn Fn(&str)>;

fn err<T>(msg: impl Into<String>) -> Result<T> {
    Err(mlua::Error::runtime(msg.into()))
}

struct Inner {
    base_dir: Option<PathBuf>,
    on_print: Option<PrintFn>,
    log: Vec<String>,
    queued: Vec<QueueJob>,
    files: Vec<PathBuf>,
    gpu: Option<rfr_gpu::GpuContext>,
    ffmpeg: PathBuf,
    cancel: Arc<AtomicBool>,
}

/// State shared by the API functions of one run.
#[derive(Clone)]
pub struct State(Rc<RefCell<Inner>>);

impl State {
    pub fn new(
        base_dir: Option<PathBuf>,
        on_print: Option<PrintFn>,
        gpu: Option<rfr_gpu::GpuContext>,
        ffmpeg: PathBuf,
        cancel: Arc<AtomicBool>,
    ) -> Self {
        State(Rc::new(RefCell::new(Inner { base_dir, on_print, log: Vec::new(), queued: Vec::new(), files: Vec::new(), gpu, ffmpeg, cancel })))
    }

    pub fn log(&self) -> Vec<String> {
        self.0.borrow().log.clone()
    }

    /// The queued jobs and the written files.
    pub fn take_results(&self) -> (Vec<QueueJob>, Vec<PathBuf>) {
        let mut s = self.0.borrow_mut();
        (std::mem::take(&mut s.queued), std::mem::take(&mut s.files))
    }

    fn print(&self, line: String) {
        let mut s = self.0.borrow_mut();
        if let Some(f) = &s.on_print {
            f(&line);
        }
        s.log.push(line);
    }

    fn path(&self, p: &str) -> PathBuf {
        let p = Path::new(p);
        match &self.0.borrow().base_dir {
            Some(dir) if p.is_relative() => dir.join(p),
            _ => p.to_path_buf(),
        }
    }
}

fn options() -> SerializeOptions {
    SerializeOptions::new().serialize_none_to_null(false).serialize_unit_to_null(false)
}

/// A value as a Lua table (lists with the array marker, so empty lists stay lists).
fn to_lua<T: serde::Serialize>(lua: &Lua, v: &T) -> Result<Value> {
    lua.to_value_with(v, options())
}

/// A Lua table read as a document.
pub fn to_doc(lua: &Lua, v: Value, what: &str) -> Result<Document> {
    lua.from_value::<Document>(v).map_err(|e| mlua::Error::runtime(format!("{what} is not a valid document: {e}")))
}

/// Writes the fields of `doc` into the table `t` (the script's table changes in place).
fn write_back(lua: &Lua, t: &Table, doc: &Document) -> Result<()> {
    let Value::Table(fresh) = to_lua(lua, doc)? else { return err("internal: a document is not a table") };
    for pair in fresh.pairs::<Value, Value>() {
        let (k, v) = pair?;
        t.set(k, v)?;
    }
    // Fields that are gone now (an animation removed).
    let keys: Vec<Value> = t.pairs::<Value, Value>().filter_map(|p| p.ok().map(|p| p.0)).collect();
    for k in keys {
        if fresh.get::<Value>(k.clone())?.is_nil() {
            t.set(k, Value::Nil)?;
        }
    }
    Ok(())
}

/// Options of `render`, `animate` and `queue`: size, samples, frames, format.
struct JobOpts {
    width: u32,
    height: u32,
    samples: u32,
    from: usize,
    to: usize,
    format: Option<String>,
    animation: Option<bool>,
}

fn job_opts(t: Option<Table>) -> Result<JobOpts> {
    let mut o = JobOpts { width: 0, height: 0, samples: 0, from: 0, to: usize::MAX, format: None, animation: None };
    if let Some(t) = t {
        o.width = t.get::<Option<u32>>("width")?.unwrap_or(0);
        o.height = t.get::<Option<u32>>("height")?.unwrap_or(0);
        o.samples = t.get::<Option<u32>>("samples")?.unwrap_or(0);
        o.from = t.get::<Option<usize>>("from")?.unwrap_or(0);
        o.to = t.get::<Option<usize>>("to")?.unwrap_or(usize::MAX);
        o.format = t.get::<Option<String>>("format")?;
        o.animation = t.get::<Option<bool>>("animation")?;
    }
    Ok(o)
}

fn video_format(name: Option<&str>, path: &Path) -> Result<rfr_core::video::VideoFormat> {
    use rfr_core::video::VideoFormat as F;
    Ok(match name.map(str::to_lowercase).as_deref() {
        None => F::from_path(path),
        Some("mp4" | "h264") => F::Mp4,
        Some("hevc" | "h265") => F::Hevc,
        Some("av1") => F::Av1,
        Some("prores" | "mov") => F::ProRes,
        Some("webm" | "vp9") => F::WebM,
        Some("gif") => F::Gif,
        Some("png") => F::PngSequence,
        Some(f) => return err(format!("unknown movie format '{f}' (mp4, hevc, av1, prores, webm, gif, png)")),
    })
}

fn job(doc: Document, output: PathBuf, o: &JobOpts, animation: bool) -> Result<QueueJob> {
    let what =
        if animation { JobWhat::Animation { format: video_format(o.format.as_deref(), &output)?, from: o.from, to: o.to } } else { JobWhat::Image };
    let name = output.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "script".into());
    Ok(QueueJob {
        id: rfr_core::queue::new_id(),
        name,
        doc: Some(Box::new(doc)),
        what,
        width: o.width,
        height: o.height,
        samples: o.samples,
        output,
        ..Default::default()
    })
}

fn kind_name(k: ProgramKind) -> &'static str {
    match k {
        ProgramKind::Fractal => "fractal",
        ProgramKind::Coloring => "coloring",
        ProgramKind::Transform => "transform",
        ProgramKind::Object => "object",
    }
}

/// Sets up the globals `doc`, `params`, `print` and the module `rfr`.
pub fn install(lua: &Lua, state: &State, doc: &Document, params: &[(String, ParamValue)]) -> Result<()> {
    let g = lua.globals();
    g.set("doc", to_lua(lua, doc)?)?;
    let p = lua.create_table()?;
    for (name, v) in params {
        match v {
            ParamValue::Number(n) if n.fract() == 0.0 && n.abs() < 9e15 => p.set(name.as_str(), *n as i64)?,
            ParamValue::Number(n) => p.set(name.as_str(), *n)?,
            ParamValue::Bool(b) => p.set(name.as_str(), *b)?,
            ParamValue::Text(s) => p.set(name.as_str(), s.as_str())?,
        }
    }
    g.set("params", p)?;
    let st = state.clone();
    g.set(
        "print",
        lua.create_function(move |_, args: Variadic<Value>| {
            let parts: Vec<String> = args.iter().map(|v| v.to_string().unwrap_or_else(|_| format!("<{}>", v.type_name()))).collect();
            st.print(parts.join("\t"));
            Ok(())
        })?,
    )?;

    let rfr = lua.create_table()?;
    rfr.set("version", rfr_core::name_version())?;
    rfr.set(
        "document",
        lua.create_function(|lua, formula: Option<String>| {
            let doc = match formula {
                Some(id) => {
                    if rfr_core::formula::library::library().get(&id).is_none() {
                        return err(format!("no formula '{id}' (rfr.formulas() lists them)"));
                    }
                    Document::for_formula(&id)
                }
                None => Document::default(),
            };
            to_lua(lua, &doc)
        })?,
    )?;
    rfr.set(
        "layer",
        lua.create_function(|lua, formula: Option<String>| {
            let id = formula.unwrap_or_else(|| Document::default().layers[0].formula.clone());
            if rfr_core::formula::library::library().get(&id).is_none() {
                return err(format!("no formula '{id}' (rfr.formulas() lists them)"));
            }
            to_lua(lua, &Document::for_formula(&id).layers[0])
        })?,
    )?;
    let st = state.clone();
    rfr.set(
        "load",
        lua.create_function(move |lua, path: String| {
            let p = st.path(&path);
            let doc = rfr_core::io::open(&p).map_err(mlua::Error::runtime)?;
            to_lua(lua, &doc)
        })?,
    )?;
    let st = state.clone();
    rfr.set(
        "save",
        lua.create_function(move |lua, (d, path): (Value, String)| {
            let doc = to_doc(lua, d, "the document to save")?;
            let p = st.path(&path);
            rfr_core::io::save(&doc, &p).map_err(mlua::Error::runtime)?;
            st.0.borrow_mut().files.push(p.clone());
            Ok(p.to_string_lossy().into_owned())
        })?,
    )?;
    // Renders run like render queue jobs (posters in tiles, movies through ffmpeg).
    let render = |state: &State, animation: bool| {
        let st = state.clone();
        move |lua: &Lua, (d, path, opts): (Value, String, Option<Table>)| -> Result<String> {
            let doc = to_doc(lua, d, "the document to render")?;
            let o = job_opts(opts)?;
            let output = st.path(&path);
            if animation && doc.animation.as_ref().is_none_or(|a| a.keyframes.is_empty()) {
                return err("the document has no keyframes to animate (rfr.add_keyframe adds them)");
            }
            let j = job(doc, output.clone(), &o, animation)?;
            let (gpu, ffmpeg, cancel) = {
                let s = st.0.borrow();
                (s.gpu.clone(), s.ffmpeg.clone(), s.cancel.clone())
            };
            let summary = rfr_gpu::batch::run_job(&j, gpu, &ffmpeg, &cancel, |_| {})
                .map_err(|e| if e == "cancelled" || e == "stopped" { mlua::Error::runtime(crate::CANCELLED) } else { mlua::Error::runtime(e) })?;
            st.0.borrow_mut().files.push(output);
            Ok(summary)
        }
    };
    rfr.set("render", lua.create_function(render(state, false))?)?;
    rfr.set("animate", lua.create_function(render(state, true))?)?;
    let st = state.clone();
    rfr.set(
        "queue",
        lua.create_function(move |lua, (d, path, opts): (Value, String, Option<Table>)| {
            let doc = to_doc(lua, d, "the document to queue")?;
            let o = job_opts(opts)?;
            let output = st.path(&path);
            let keyframes = doc.animation.as_ref().is_some_and(|a| !a.keyframes.is_empty());
            let still = output.extension().is_some_and(|e| e.eq_ignore_ascii_case("png"));
            let animation = o.animation.unwrap_or(keyframes && !still);
            if animation && !keyframes {
                return err("the document has no keyframes to animate");
            }
            let j = job(doc, output, &o, animation)?;
            st.0.borrow_mut().queued.push(j);
            Ok(())
        })?,
    )?;
    rfr.set(
        "formulas",
        lua.create_function(|lua, kind: Option<String>| {
            let lib = rfr_core::formula::library::library();
            let want = kind.as_deref().unwrap_or("fractal").to_lowercase();
            let list: Vec<&rfr_core::formula::library::Entry> = match want.as_str() {
                "fractal" | "fractals" => lib.usable().collect(),
                "coloring" | "colorings" => lib.colorings().collect(),
                "transform" | "transforms" | "transformation" => lib.transforms().collect(),
                "object" | "objects" | "3d" => lib.objects().collect(),
                other => return err(format!("unknown formula kind '{other}' (fractal, coloring, transform, object)")),
            };
            let out = lua.create_table()?;
            for (i, e) in list.into_iter().enumerate() {
                let t = lua.create_table()?;
                t.set("id", e.id.as_str())?;
                t.set("title", e.title.as_str())?;
                t.set("family", e.family.as_str())?;
                t.set("kind", kind_name(e.kind))?;
                out.set(i + 1, t)?;
            }
            Ok(out)
        })?,
    )?;
    rfr.set(
        "params",
        lua.create_function(|lua, id: String| {
            let lib = rfr_core::formula::library::library();
            let Some(e) = lib.get(&id) else { return err(format!("no formula '{id}'")) };
            let prog = e.program().map_err(|e| mlua::Error::runtime(format!("{id} does not compile: {e}")))?;
            let out = lua.create_table()?;
            for (i, p) in prog.params.iter().enumerate() {
                let t = lua.create_table()?;
                t.set("name", p.name.as_str())?;
                t.set("caption", p.caption.as_str())?;
                t.set("kind", format!("{:?}", p.kind).to_lowercase())?;
                t.set("default", to_lua(lua, &p.default)?)?;
                t.set("min", p.min)?;
                t.set("max", p.max)?;
                t.set("hint", p.hint.as_str())?;
                out.set(i + 1, t)?;
            }
            Ok(out)
        })?,
    )?;
    // View changes exact at any depth (the center has as many digits as it needs).
    let view_op = |f: fn(&mut View, f64, f64, f64)| {
        move |lua: &Lua, (d, a, b, c): (Table, f64, Option<f64>, Option<f64>)| -> Result<()> {
            let mut view: View = lua.from_value(d.get::<Value>("view")?).map_err(|e| mlua::Error::runtime(format!("doc.view is not a view: {e}")))?;
            f(&mut view, a, b.unwrap_or(0.0), c.unwrap_or(0.0));
            d.set("view", to_lua(lua, &view)?)
        }
    };
    rfr.set("zoom", lua.create_function(view_op(|v, factor, x, y| v.zoom_at([x, y], factor)))?)?;
    rfr.set("pan", lua.create_function(view_op(|v, dx, dy, _| v.pan([dx, dy])))?)?;
    rfr.set(
        "add_keyframe",
        lua.create_function(|lua, (d, frames): (Table, Option<u32>)| {
            let mut doc = to_doc(lua, Value::Table(d.clone()), "the document")?;
            let mut key = doc.clone();
            key.animation = None;
            let anim = doc.animation.get_or_insert_with(Default::default);
            anim.keyframes.push(rfr_core::animation::Keyframe { doc: key, frames: frames.unwrap_or(60).max(1), ..Default::default() });
            let n = anim.keyframes.len();
            write_back(lua, &d, &doc)?;
            Ok(n)
        })?,
    )?;
    rfr.set(
        "dump",
        lua.create_function(|lua, v: Value| {
            let json: serde_json::Value = lua.from_value(v)?;
            serde_json::to_string_pretty(&json).map_err(mlua::Error::external)
        })?,
    )?;
    rfr.set(
        "copy",
        lua.create_function(|lua, v: Value| {
            let json: serde_json::Value = lua.from_value(v)?;
            to_lua(lua, &json)
        })?,
    )?;
    g.set("rfr", rfr)?;
    Ok(())
}
