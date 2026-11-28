// SPDX-License-Identifier: LGPL-3.0-or-later

//! Lua scripting: scripts build and change documents, render images and animations and fill
//! the render queue.
//!
//! A script works on the global `doc` (the current document as a Lua table, the same fields as
//! an `.rfr` file) and the `rfr` module; see [`API_REFERENCE`]. Its leading comment block names
//! it and declares parameters:
//!
//! ```lua
//! --- Zoom animation
//! --- Keyframes zooming into the middle of the view.
//! --@param steps integer 8 1 40     Number of keyframes after the first
//! --@param factor number 0.1 0.001 0.9  Zoom of each step
//! ```
//!
//! Their values are in the global table `params` (`params.steps`, …).

mod api;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use mlua::{HookTriggers, Lua, LuaOptions, StdLib, Value, VmState};
use rfr_core::doc::Document;
use rfr_core::queue::QueueJob;

pub use api::{API_REFERENCE, PrintFn};

/// File extension of scripts.
pub const EXTENSION: &str = "lua";

/// Example scripts shipped with RFract (file name, source).
pub const EXAMPLES: &[(&str, &str)] = &[
    ("Zoom animation.lua", include_str!("../scripts/Zoom animation.lua")),
    ("Julia grid.lua", include_str!("../scripts/Julia grid.lua")),
    ("Formula gallery.lua", include_str!("../scripts/Formula gallery.lua")),
    ("Layer stack.lua", include_str!("../scripts/Layer stack.lua")),
    ("Color variations.lua", include_str!("../scripts/Color variations.lua")),
];

/// Type and range of a script parameter.
#[derive(Clone, Debug, PartialEq)]
pub enum ParamKind {
    Number { min: f64, max: f64 },
    Integer { min: i64, max: i64 },
    Bool,
    Choice(Vec<String>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum ParamValue {
    Number(f64),
    Bool(bool),
    Text(String),
}

impl std::fmt::Display for ParamValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParamValue::Number(v) => write!(f, "{v}"),
            ParamValue::Bool(b) => write!(f, "{b}"),
            ParamValue::Text(s) => f.write_str(s),
        }
    }
}

/// A parameter declared with `--@param`.
#[derive(Clone, Debug, PartialEq)]
pub struct ParamDecl {
    pub name: String,
    pub kind: ParamKind,
    pub default: ParamValue,
    pub help: String,
}

impl ParamDecl {
    /// Parses a value given as text (command line).
    pub fn parse_value(&self, s: &str) -> Result<ParamValue, String> {
        let bad = || format!("parameter '{}': '{s}' is not a valid value", self.name);
        Ok(match &self.kind {
            ParamKind::Number { .. } => ParamValue::Number(s.parse().map_err(|_| bad())?),
            ParamKind::Integer { .. } => ParamValue::Number(s.parse::<i64>().map_err(|_| bad())? as f64),
            ParamKind::Bool => ParamValue::Bool(match s {
                "true" | "1" | "yes" | "on" => true,
                "false" | "0" | "no" | "off" => false,
                _ => return Err(bad()),
            }),
            ParamKind::Choice(options) => {
                let o = options
                    .iter()
                    .find(|o| o.eq_ignore_ascii_case(s))
                    .ok_or_else(|| format!("parameter '{}': '{s}' is not one of {}", self.name, options.join(", ")))?;
                ParamValue::Text(o.clone())
            }
        })
    }
}

/// Name, description and parameters from the leading comment block of a script.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Header {
    pub title: String,
    pub description: String,
    pub params: Vec<ParamDecl>,
    /// Lines of the header that could not be read (`--@param` with a wrong format).
    pub warnings: Vec<String>,
}

impl Header {
    pub fn parse(source: &str) -> Header {
        let mut h = Header::default();
        for (n, line) in source.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Some(rest) = line.strip_prefix("--@param") {
                match parse_param(rest) {
                    Ok(p) => h.params.push(p),
                    Err(e) => h.warnings.push(format!("line {}: {e}", n + 1)),
                }
            } else if let Some(text) = line.strip_prefix("---") {
                let text = text.trim();
                if h.title.is_empty() {
                    h.title = text.to_string();
                } else if !text.is_empty() {
                    if !h.description.is_empty() {
                        h.description.push(' ');
                    }
                    h.description.push_str(text);
                }
            } else if !line.starts_with("--") {
                break;
            }
        }
        h
    }

    /// Parameter values: the defaults, overridden by `given` (name, text) pairs.
    pub fn values(&self, given: &[(String, String)]) -> Result<Vec<(String, ParamValue)>, String> {
        let mut out: Vec<(String, ParamValue)> = self.params.iter().map(|p| (p.name.clone(), p.default.clone())).collect();
        for (name, text) in given {
            let Some(i) = self.params.iter().position(|p| &p.name == name) else {
                let known: Vec<&str> = self.params.iter().map(|p| p.name.as_str()).collect();
                return Err(if known.is_empty() {
                    format!("the script has no parameter '{name}' (it declares none)")
                } else {
                    format!("the script has no parameter '{name}' (known: {})", known.join(", "))
                });
            };
            out[i].1 = self.params[i].parse_value(text)?;
        }
        Ok(out)
    }
}

fn parse_param(rest: &str) -> Result<ParamDecl, String> {
    const FORMAT: &str = "expected `--@param <name> number|integer|bool|choice <default or a|b|c> [min max] [help]`";
    let mut words = rest.split_whitespace();
    let name = words.next().ok_or(FORMAT)?.to_string();
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') || name.starts_with(|c: char| c.is_ascii_digit()) {
        return Err(format!("'{name}' is not a valid parameter name (letters, digits, _)"));
    }
    let kind = words.next().ok_or(FORMAT)?;
    let default = words.next().ok_or(FORMAT)?;
    let mut rest: Vec<&str> = words.collect();
    let num = |s: &str| s.parse::<f64>().map_err(|_| format!("'{s}' is not a number ({FORMAT})"));
    // Optional numeric range after the default.
    let mut range = |lo: f64, hi: f64| -> Result<(f64, f64), String> {
        if rest.len() >= 2 && rest[0].parse::<f64>().is_ok() {
            let r = (num(rest[0])?, num(rest[1])?);
            rest.drain(..2);
            Ok(r)
        } else {
            Ok((lo, hi))
        }
    };
    let (kind, default) = match kind {
        "number" => {
            let d = num(default)?;
            let (min, max) = range(d.min(0.0), if d == 0.0 { 1.0 } else { d.abs() * 2.0 })?;
            (ParamKind::Number { min, max }, ParamValue::Number(d))
        }
        "integer" | "int" => {
            let d = num(default)?.round();
            let (min, max) = range(d.min(0.0), (d.abs() * 2.0).max(10.0))?;
            (ParamKind::Integer { min: min as i64, max: max as i64 }, ParamValue::Number(d))
        }
        "bool" | "boolean" => {
            let d = match default {
                "true" => true,
                "false" => false,
                _ => return Err(format!("the default of a bool must be true or false, not '{default}'")),
            };
            (ParamKind::Bool, ParamValue::Bool(d))
        }
        "choice" => {
            let options: Vec<String> = default.split('|').filter(|s| !s.is_empty()).map(str::to_string).collect();
            if options.is_empty() {
                return Err("a choice needs options like a|b|c".into());
            }
            (ParamKind::Choice(options.clone()), ParamValue::Text(options[0].clone()))
        }
        other => return Err(format!("unknown parameter type '{other}' ({FORMAT})")),
    };
    Ok(ParamDecl { name, kind, default, help: rest.join(" ") })
}

/// What a script runs on.
pub struct RunInput {
    /// The current document (the global `doc`).
    pub doc: Document,
    pub params: Vec<(String, ParamValue)>,
    /// Set to stop the script (checked every few thousand Lua instructions and between renders).
    pub cancel: Arc<AtomicBool>,
    /// Folder that relative paths refer to.
    pub base_dir: Option<PathBuf>,
    /// Receives every `print` line while the script runs.
    pub on_print: Option<api::PrintFn>,
    /// The graphics card renders run on (`None`: the CPU).
    pub gpu: Option<rfr_gpu::GpuContext>,
    /// The ffmpeg program for movies.
    pub ffmpeg: PathBuf,
}

impl RunInput {
    pub fn new(doc: Document) -> Self {
        Self {
            doc,
            params: Vec::new(),
            cancel: Arc::new(AtomicBool::new(false)),
            base_dir: None,
            on_print: None,
            gpu: None,
            ffmpeg: rfr_core::video::ffmpeg_program(None),
        }
    }
}

#[derive(Debug)]
pub struct RunOutput {
    /// The global `doc` after the run.
    pub doc: Document,
    /// `doc` differs from the input document.
    pub changed: bool,
    /// Jobs added with `rfr.queue`.
    pub queued: Vec<QueueJob>,
    /// Files written by `rfr.render`, `rfr.animate` and `rfr.save`.
    pub files: Vec<PathBuf>,
    /// `print` lines.
    pub log: Vec<String>,
}

#[derive(Debug)]
pub struct ScriptError {
    pub message: String,
    pub cancelled: bool,
    /// `print` lines up to the error.
    pub log: Vec<String>,
}

impl std::fmt::Display for ScriptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

const CANCELLED: &str = "script stopped";
/// Memory a script may use (protects the app against runaway tables).
const MEMORY_LIMIT: usize = 2 << 30;

/// Runs a script. `name` (e.g. the file name) appears in error messages as `name:line: message`.
pub fn run(source: &str, name: &str, input: RunInput) -> Result<RunOutput, ScriptError> {
    let state = api::State::new(input.base_dir.clone(), input.on_print, input.gpu.clone(), input.ffmpeg.clone(), input.cancel.clone());
    let fail = |e: mlua::Error, state: &api::State| {
        let message = error_text(&e, name);
        ScriptError { cancelled: message.ends_with(CANCELLED), message, log: state.log() }
    };
    let lua = Lua::new_with(StdLib::ALL_SAFE, LuaOptions::default()).map_err(|e| fail(e, &state))?;
    let _ = lua.set_memory_limit(MEMORY_LIMIT);
    let cancel = input.cancel.clone();
    lua.set_hook(HookTriggers::new().every_nth_instruction(4096), move |_, _| {
        if cancel.load(Ordering::Relaxed) { Err(mlua::Error::runtime(CANCELLED)) } else { Ok(VmState::Continue) }
    })
    .map_err(|e| fail(e, &state))?;
    let original = input.doc.clone();
    let result = (|| -> mlua::Result<Document> {
        api::install(&lua, &state, &input.doc, &input.params)?;
        lua.load(source).set_name(format!("@{name}")).exec()?;
        match lua.globals().get::<Value>("doc")? {
            Value::Nil => Ok(original.clone()),
            v => api::to_doc(&lua, v, "the global 'doc'"),
        }
    })();
    match result {
        Ok(doc) => {
            let (queued, files) = state.take_results();
            Ok(RunOutput { changed: doc != original, doc, queued, files, log: state.log() })
        }
        Err(e) => Err(fail(e, &state)),
    }
}

/// The message of an error without mlua's wrapping, with the script line of a failing call
/// into RFract.
fn error_text(e: &mlua::Error, name: &str) -> String {
    match e {
        mlua::Error::CallbackError { traceback, cause } => {
            let inner = error_text(cause, name);
            let prefix = format!("{name}:");
            let at = traceback
                .lines()
                .map(str::trim)
                .find(|l| l.starts_with(&prefix))
                .and_then(|l| l.split(": in ").next())
                .map(|l| l.trim_end_matches(':'));
            match at {
                Some(at) if !inner.starts_with(&prefix) => format!("{at}: {inner}"),
                _ => inner,
            }
        }
        mlua::Error::RuntimeError(m) | mlua::Error::SyntaxError { message: m, .. } => m.clone(),
        mlua::Error::MemoryError(m) => format!("out of memory: {m}"),
        mlua::Error::ExternalError(e) => e.to_string(),
        mlua::Error::WithContext { context, cause } => format!("{context}: {}", error_text(cause, name)),
        mlua::Error::FromLuaConversionError { message: Some(m), .. } => m.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests;
