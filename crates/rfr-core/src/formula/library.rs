// SPDX-License-Identifier: LGPL-3.0-or-later

//! The formula library: RFract's built-in formulas (`formulas/rfract/*.ufm`), the bundled
//! FractalPark set (`formulas/fractalpark`), both compiled once on first use, and the formulas
//! in the user's folders (indexed in the background, compiled when first used).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, RwLock};

use serde::Deserialize;

use std::collections::BTreeMap;

use super::ast::{Dialect, FormulaAst, ParamValue, ProgramKind};
use super::ir::Program;
use super::parse::Error;

include!(concat!(env!("OUT_DIR"), "/bundled_formulas.rs"));

const RFRACT_UFM: &str = include_str!("../../formulas/rfract/rfract.ufm");
const RFRACT_UCL: &str = include_str!("../../formulas/rfract/rfract.ucl");
const RFRACT_UXF: &str = include_str!("../../formulas/rfract/rfract.uxf");
const FRACTINT_UFM: &str = include_str!("../../formulas/rfract/fractint.ufm");
const ATTRACTORS_UFM: &str = include_str!("../../formulas/rfract/attractors.ufm");
const STANDARD_ULB: &str = include_str!("../../formulas/rfract/standard.ulb");
const FRACTALPARK_INDEX: &str = include_str!("../../formulas/fractalpark/index.json");

/// Where a formula comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Set {
    RFract,
    FractalPark,
    /// A formula file in one of the user's formula folders.
    User,
}

impl Set {
    pub fn label(self) -> &'static str {
        match self {
            Set::RFract => "RFract",
            Set::FractalPark => "FractalPark",
            Set::User => "User formulas",
        }
    }
}

/// The view a formula suggests: center, half the height of the view in plane units, iteration
/// limit and, for Julia-mode formulas, the seed.
#[derive(Clone, Debug, PartialEq)]
pub struct DefaultView {
    pub center: [f64; 2],
    pub radius: f64,
    pub max_iter: u32,
    pub julia: Option<[f64; 2]>,
}

impl Default for DefaultView {
    fn default() -> Self {
        DefaultView { center: [0.0, 0.0], radius: 1.5, max_iter: 250, julia: None }
    }
}

/// The text a formula is compiled from.
#[derive(Clone, Debug)]
enum Origin {
    /// A whole bundled file (FRM-like v1: one formula per file).
    Static(&'static str),
    /// A file in a user folder, read again when the entry is compiled.
    File(PathBuf),
}

#[derive(Debug)]
pub struct Entry {
    /// Stable identifier stored in documents: `rfract:<entry name>`, `fractalpark:<formula id>`,
    /// `user:<path relative to its formula folder>:<entry name>`.
    pub id: String,
    pub title: String,
    pub family: String,
    pub set: Set,
    pub kind: ProgramKind,
    pub dialect: Dialect,
    /// Name of the entry inside a multi-entry UFM file.
    pub entry: String,
    pub view: DefaultView,
    origin: Origin,
    program: OnceLock<Result<Arc<Program>, Error>>,
    /// Whether the formula uses objects (its program depends on the chosen classes).
    classy: OnceLock<bool>,
    /// Programs for other class choices (key: the choices).
    variants: std::sync::Mutex<HashMap<String, Result<Arc<Program>, Error>>>,
}

impl Entry {
    /// The program for stored parameter values: formulas that use objects compile for the
    /// classes chosen in `params` (object parameters, `ParamValue::Class`).
    pub fn program_with(&self, params: &BTreeMap<String, ParamValue>) -> Result<Arc<Program>, Error> {
        let base = self.program();
        if !self.classy.get().copied().unwrap_or(false) {
            return base;
        }
        let choices: BTreeMap<String, ParamValue> =
            params.iter().filter(|(_, v)| matches!(v, ParamValue::Class { .. })).map(|(k, v)| (k.clone(), v.clone())).collect();
        if choices.is_empty() {
            return base;
        }
        let key = choices.iter().map(|(k, v)| format!("{k}={v:?}")).collect::<Vec<_>>().join(";");
        if let Some(r) = self.variants.lock().ok().and_then(|m| m.get(&key).cloned()) {
            return r;
        }
        let r = self.compile_with(&choices).map(Arc::new);
        if let Ok(mut m) = self.variants.lock() {
            m.insert(key, r.clone());
        }
        r
    }

    /// Whether the formula uses objects (known after the first compile).
    pub fn uses_classes(&self) -> bool {
        self.classy.get().copied().unwrap_or(false)
    }
    /// The compiled program (compiled on the first call for user formulas).
    pub fn program(&self) -> Result<Arc<Program>, Error> {
        self.program.get_or_init(|| self.compile_with(&BTreeMap::new()).map(Arc::new)).clone()
    }

    /// Whether the entry compiled, if it was compiled already.
    pub fn status(&self) -> Option<bool> {
        self.program.get().map(|r| r.is_ok())
    }

    /// The formula's source text (the whole file it comes from).
    pub fn source(&self) -> Option<String> {
        match &self.origin {
            Origin::Static(s) => Some(s.to_string()),
            Origin::File(p) => read_text(p),
        }
    }

    /// The file a user formula comes from.
    pub fn path(&self) -> Option<&Path> {
        match &self.origin {
            Origin::File(p) => Some(p),
            Origin::Static(_) => None,
        }
    }

    fn compile_with(&self, choices: &BTreeMap<String, ParamValue>) -> Result<Program, Error> {
        let src = self.source().ok_or_else(|| Error { line: 0, msg: "the formula file cannot be read".into() })?;
        if self.dialect == Dialect::Frm {
            return super::compile_frm(&src);
        }
        let pre = super::parse::preprocess(&src);
        let entries = super::parse::split_entries(&pre);
        let e = entries
            .iter()
            .find(|e| e.name == self.entry)
            .ok_or_else(|| Error { line: 0, msg: format!("the entry '{}' is no longer in the file", self.entry) })?;
        let ast = super::parse::parse_ufm_entry(e)?;
        let classy = !ast.imports.is_empty() || ast.params.iter().any(|p| p.kind == super::ast::ParamKind::Class);
        let _ = self.classy.set(classy);
        if !classy {
            return super::check::check_kind(&ast, self.kind);
        }
        let reg = class_registry();
        let lookup = |n: &str| reg.get(n);
        let ctx = super::check::ClassCtx { lookup: &lookup, choices };
        super::check::check_with(&ast, self.kind, Some(&ctx))
    }

    fn compiled(mut self, program: Result<Program, Error>) -> Entry {
        self.program = OnceLock::from(program.map(Arc::new));
        self
    }
}

pub struct Library {
    pub entries: Vec<Arc<Entry>>,
    index: HashMap<String, usize>,
}

impl Library {
    fn new(entries: Vec<Arc<Entry>>) -> Library {
        let mut index = HashMap::with_capacity(entries.len());
        for (i, e) in entries.iter().enumerate() {
            index.entry(e.id.clone()).or_insert(i);
        }
        Library { entries, index }
    }
    pub fn get(&self, id: &str) -> Option<&Entry> {
        self.index.get(id).map(|&i| &*self.entries[i])
    }
    /// The user formula `entry` of kind `kind` in a file called `file` (any folder; the name's
    /// case is ignored when no entry matches exactly).
    pub fn find_in_file(&self, kind: ProgramKind, file: &str, entry: &str) -> Option<&Entry> {
        let base = file.rsplit(['/', '\\']).next().unwrap_or(file);
        let in_file = |e: &&Entry| {
            e.set == Set::User
                && e.kind == kind
                && e.path().and_then(|p| p.file_name()).is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(base))
        };
        let all = || self.entries.iter().map(|e| &**e).filter(in_file);
        all().find(|e| e.entry == entry).or_else(|| all().find(|e| e.entry.eq_ignore_ascii_case(entry)))
    }

    /// Fractal formulas that compiled (user formulas count until they fail).
    pub fn usable(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().map(|e| &**e).filter(|e| e.status() != Some(false) && e.kind == ProgramKind::Fractal)
    }
    /// Coloring algorithms that compiled (user formulas count until they fail).
    pub fn colorings(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().map(|e| &**e).filter(|e| e.status() != Some(false) && e.kind == ProgramKind::Coloring)
    }
    /// Transformations that compiled (user formulas count until they fail).
    pub fn transforms(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().map(|e| &**e).filter(|e| e.status() != Some(false) && e.kind == ProgramKind::Transform)
    }
}

#[derive(Deserialize)]
struct FpIndex {
    rows: Vec<FpRow>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FpRow {
    formula_id: String,
    display_name: String,
    family: String,
    definition_path: String,
    profile: FpProfile,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FpProfile {
    mode: String,
    center: [f64; 2],
    zoom: f64,
    iterations: u32,
    #[serde(default)]
    julia_c: Option<[f64; 2]>,
}

static USER: RwLock<Option<Arc<Library>>> = RwLock::new(None);
static CLASSES: RwLock<Option<(u64, Arc<ClassRegistry>)>> = RwLock::new(None);

/// The classes of the class libraries (`.ulb`) in the formula folders and RFract's own
/// standard classes; parsed on first use after the folders change.
pub fn class_registry() -> Arc<ClassRegistry> {
    let generation = generation();
    if let Some((g, r)) = CLASSES.read().ok().and_then(|c| c.clone())
        && g == generation
    {
        return r;
    }
    let r = Arc::new(ClassRegistry::from_folders(&user_folders()));
    if let Ok(mut c) = CLASSES.write() {
        *c = Some((generation, r.clone()));
    }
    r
}

/// Classes by (lowercase) name. The first definition of a name counts.
#[derive(Default)]
pub struct ClassRegistry {
    classes: HashMap<String, Arc<super::ast::ClassAst>>,
    files: HashMap<String, String>,
    /// Classes that do not parse: name and why.
    pub errors: Vec<(String, Error)>,
}

impl ClassRegistry {
    /// The classes of every `.ulb` file below `folders` and the bundled standard classes.
    pub fn from_folders(folders: &[PathBuf]) -> ClassRegistry {
        let mut r = ClassRegistry::default();
        r.add_source(STANDARD_ULB, "standard.ulb (RFract)");
        let mut files = Vec::new();
        for f in folders {
            ulb_files(f, &mut files);
        }
        for f in files {
            if let Some(text) = read_text(&f) {
                let name = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                r.add_source(&text, &name);
            }
        }
        r
    }

    fn add_source(&mut self, text: &str, file: &str) {
        for (name, c) in super::parse::parse_classes(text) {
            let key = name.to_ascii_lowercase();
            if self.classes.contains_key(&key) {
                continue;
            }
            match c {
                Ok(c) => {
                    self.files.insert(key.clone(), file.to_string());
                    self.classes.insert(key, Arc::new(c));
                }
                Err(e) => self.errors.push((name, e)),
            }
        }
    }

    pub fn get(&self, name: &str) -> Option<Arc<super::ast::ClassAst>> {
        let n = name.rsplit(':').next().unwrap_or(name).trim();
        self.classes.get(&n.to_ascii_lowercase()).cloned()
    }

    /// The file a class comes from.
    pub fn file_of(&self, name: &str) -> Option<&str> {
        self.files.get(&name.to_ascii_lowercase()).map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.classes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.classes.is_empty()
    }

    /// Classes derived from `base` (including itself), sorted by name.
    pub fn derived(&self, base: &str) -> Vec<String> {
        let base = base.to_ascii_lowercase();
        let mut out: Vec<String> = self
            .classes
            .values()
            .filter(|c| {
                let mut cur = Some((*c).clone());
                let mut steps = 0;
                while let Some(k) = cur {
                    if k.name.eq_ignore_ascii_case(&base) {
                        return true;
                    }
                    steps += 1;
                    if steps > 32 {
                        return false;
                    }
                    cur = k.parent.as_ref().and_then(|p| self.get(p));
                }
                false
            })
            .map(|c| c.name.clone())
            .collect();
        out.sort_by_key(|n| n.to_ascii_lowercase());
        out
    }
}

/// Class library files (`.ulb`) below `dir`.
fn ulb_files(dir: &Path, out: &mut Vec<PathBuf>) {
    if dir.is_dir() {
        if let Ok(rd) = std::fs::read_dir(dir) {
            let mut list: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
            list.sort();
            for p in list {
                ulb_files(&p, out);
            }
        }
    } else if dir.extension().is_some_and(|e| e.eq_ignore_ascii_case("ulb")) {
        out.push(dir.to_path_buf());
    }
}
static FOLDERS: RwLock<Vec<PathBuf>> = RwLock::new(Vec::new());

/// The formula folders of the last [`set_user_folders`].
pub fn user_folders() -> Vec<PathBuf> {
    FOLDERS.read().map(|f| f.clone()).unwrap_or_default()
}
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// The library: the bundled formulas plus the user formulas indexed last.
pub fn library() -> Arc<Library> {
    if let Some(l) = USER.read().ok().and_then(|g| g.clone()) {
        return l;
    }
    bundled().clone()
}

/// Changes whenever the user formulas are replaced (to re-resolve documents).
pub fn generation() -> u64 {
    GENERATION.load(Ordering::Relaxed)
}

fn bundled() -> &'static Arc<Library> {
    static LIB: OnceLock<Arc<Library>> = OnceLock::new();
    LIB.get_or_init(|| Arc::new(Library::new(build())))
}

/// Replaces the user formulas with the entries of `folders` (see [`scan_folders`]).
pub fn set_user_folders(folders: &[PathBuf]) -> usize {
    let user = scan_folders(folders);
    let n = user.len();
    if let Ok(mut f) = FOLDERS.write() {
        *f = folders.to_vec();
    }
    let mut all = bundled().entries.clone();
    all.extend(user);
    if let Ok(mut g) = USER.write() {
        *g = Some(Arc::new(Library::new(all)));
    }
    GENERATION.fetch_add(1, Ordering::Relaxed);
    n
}

/// Formula files a folder holds (recursively): `.ufm`, `.frm` (fractals), `.ucl` (colorings),
/// `.uxf` (transformations).
pub fn formula_files(dir: &Path, out: &mut Vec<PathBuf>) {
    if dir.is_dir() {
        if let Ok(rd) = std::fs::read_dir(dir) {
            let mut list: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
            list.sort();
            for p in list {
                formula_files(&p, out);
            }
        }
    } else if dir.extension().is_some_and(|e| ["ufm", "ucl", "uxf", "frm"].iter().any(|x| e.eq_ignore_ascii_case(x))) {
        out.push(dir.to_path_buf());
    }
}

/// Text of a formula file (Latin-1 when it is not UTF-8, as older formula files are).
fn read_text(p: &Path) -> Option<String> {
    let bytes = std::fs::read(p).ok()?;
    Some(match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => e.into_bytes().iter().map(|&b| b as char).collect(),
    })
}

/// Indexes the formula files of `folders`: every entry with its title, kind and suggested view
/// (parsed, not compiled). Ids are `user:<relative path>:<entry>`; the first folder wins when
/// two hold the same relative path.
pub fn scan_folders(folders: &[PathBuf]) -> Vec<Arc<Entry>> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for root in folders {
        let mut files = Vec::new();
        formula_files(root, &mut files);
        for f in files {
            scan_file(root, &f, &mut seen, &mut out);
        }
    }
    out
}

/// The entries of one formula file below `root` (ids already in `seen` are skipped).
fn scan_file(root: &Path, f: &Path, seen: &mut std::collections::HashSet<String>, out: &mut Vec<Arc<Entry>>) {
    let rel: String =
        f.strip_prefix(root).unwrap_or(f).components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/");
    let Some(text) = read_text(f) else { return };
    let ext = f.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let kind = match ext.as_str() {
        "ucl" => ProgramKind::Coloring,
        "uxf" => ProgramKind::Transform,
        _ => ProgramKind::Fractal,
    };
    let family = rel.clone();
    if text.contains("@language: frm-like/1") {
        let id = format!("user:{rel}");
        if seen.insert(id.clone()) {
            let title = f.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| rel.clone());
            out.push(Arc::new(Entry {
                id,
                title,
                family,
                set: Set::User,
                kind,
                dialect: Dialect::Frm,
                entry: String::new(),
                view: DefaultView::default(),
                origin: Origin::File(f.to_path_buf()),
                program: OnceLock::new(),
                classy: OnceLock::new(),
                variants: Default::default(),
            }));
        }
        return;
    }
    let pre = super::parse::preprocess(&text);
    for e in super::parse::split_entries(&pre) {
        let id = format!("user:{rel}:{}", e.name);
        if !seen.insert(id.clone()) {
            continue;
        }
        let ast = super::parse::parse_ufm_entry(&e);
        let (title, view) = match &ast {
            Ok(a) => (a.title.clone().unwrap_or_else(|| e.name.clone()), view_of(a)),
            Err(_) => (e.name.clone(), DefaultView::default()),
        };
        let entry = Entry {
            id,
            title,
            family: family.clone(),
            set: Set::User,
            kind,
            dialect: Dialect::Ufm,
            entry: e.name.clone(),
            view,
            origin: Origin::File(f.to_path_buf()),
            program: OnceLock::new(),
            classy: OnceLock::new(),
            variants: Default::default(),
        };
        // Entries that do not even parse are known to fail.
        out.push(Arc::new(match ast {
            Err(err) => entry.compiled(Err(err)),
            Ok(_) => entry,
        }));
    }
}

/// Reads one formula file of the formula folders again (after it was edited) and swaps its
/// entries in; returns how many it has. Other entries keep their compiled programs.
pub fn reload_file(path: &Path) -> Result<usize, String> {
    let folders = user_folders();
    let root = folders.iter().find(|r| path.starts_with(r)).ok_or_else(|| format!("{} is not in a formula folder", path.display()))?;
    let mut fresh = Vec::new();
    scan_file(root, path, &mut std::collections::HashSet::new(), &mut fresh);
    let n = fresh.len();
    let current = library();
    let mut all: Vec<Arc<Entry>> = Vec::with_capacity(current.entries.len() + n);
    let mut inserted = false;
    for e in &current.entries {
        if e.path() == Some(path) {
            if !inserted {
                all.append(&mut fresh);
                inserted = true;
            }
            continue;
        }
        all.push(e.clone());
    }
    if !inserted {
        all.append(&mut fresh);
    }
    if let Ok(mut g) = USER.write() {
        *g = Some(Arc::new(Library::new(all)));
    }
    GENERATION.fetch_add(1, Ordering::Relaxed);
    Ok(n)
}

fn view_of(a: &FormulaAst) -> DefaultView {
    DefaultView {
        center: a.center.unwrap_or([0.0, 0.0]),
        radius: 1.5 / a.magn.unwrap_or(1.0).max(1e-300),
        max_iter: a.maxiter.unwrap_or(250),
        julia: None,
    }
}

/// Entries of a built-in UFM syntax file of the given kind (ids `<prefix>:<entry name>`).
fn ufm_entries(src: &'static str, kind: ProgramKind, prefix: &str, family: &str, out: &mut Vec<Arc<Entry>>) {
    for e in super::parse::split_entries(src) {
        let ast = super::parse::parse_ufm_entry(&e);
        let (title, view) = match &ast {
            Ok(a) => {
                (a.title.clone().unwrap_or_else(|| e.name.clone()), if kind == ProgramKind::Fractal { view_of(a) } else { DefaultView::default() })
            }
            Err(_) => (e.name.clone(), DefaultView::default()),
        };
        let program = ast.and_then(|a| super::check::check_kind(&a, kind));
        let entry = Entry {
            id: format!("{prefix}:{}", e.name),
            title,
            family: family.into(),
            set: Set::RFract,
            kind,
            dialect: Dialect::Ufm,
            entry: e.name.clone(),
            view,
            origin: Origin::Static(src),
            program: OnceLock::new(),
            classy: OnceLock::new(),
            variants: Default::default(),
        };
        out.push(Arc::new(entry.compiled(program)));
    }
}

fn build() -> Vec<Arc<Entry>> {
    let mut entries = Vec::new();
    ufm_entries(RFRACT_UCL, ProgramKind::Coloring, "rfract-coloring", "Coloring", &mut entries);
    ufm_entries(RFRACT_UXF, ProgramKind::Transform, "rfract-transform", "Transformation", &mut entries);
    ufm_entries(RFRACT_UFM, ProgramKind::Fractal, "rfract", "RFract", &mut entries);
    ufm_entries(FRACTINT_UFM, ProgramKind::Fractal, "fractint", "FractInt types", &mut entries);
    ufm_entries(ATTRACTORS_UFM, ProgramKind::Fractal, "rfract-attractor", "Attractors", &mut entries);
    let index: FpIndex = serde_json::from_str(FRACTALPARK_INDEX).unwrap_or(FpIndex { rows: Vec::new() });
    let mut seen: HashMap<String, usize> = HashMap::new();
    for row in index.rows {
        let file = row.definition_path.rsplit('/').next().unwrap_or("");
        let Some((_, src)) = FRACTALPARK.iter().find(|(n, _)| *n == file) else { continue };
        let n = seen.entry(row.display_name.clone()).or_insert(0);
        *n += 1;
        let title = if *n > 1 { format!("{} ({n})", row.display_name) } else { row.display_name.clone() };
        let p = &row.profile;
        // FractalPark frames by half the width; RFract by half the height (16:9 assumed).
        let radius = (1.0 / p.zoom.max(1e-12)) * 9.0 / 16.0;
        let entry = Entry {
            id: format!("fractalpark:{}", row.formula_id),
            title,
            family: row.family,
            set: Set::FractalPark,
            kind: ProgramKind::Fractal,
            dialect: Dialect::Frm,
            entry: String::new(),
            view: DefaultView {
                center: p.center,
                radius,
                max_iter: p.iterations.max(1),
                julia: if p.mode == "julia" { Some(p.julia_c.unwrap_or([0.0, 0.0])) } else { None },
            },
            origin: Origin::Static(src),
            program: OnceLock::new(),
            classy: OnceLock::new(),
            variants: Default::default(),
        };
        out_push(&mut entries, entry.compiled(super::compile_frm(src)));
    }
    entries
}

fn out_push(v: &mut Vec<Arc<Entry>>, e: Entry) {
    v.push(Arc::new(e));
}

#[cfg(test)]
mod tests {
    use super::super::cpu::Compiled;
    use super::*;
    use crate::math::complex::C64;
    use crate::math::scalar::Dual;

    #[test]
    fn user_folders_are_indexed_and_compiled_on_use() {
        let dir = std::env::temp_dir().join(format!("rfract-user-formulas-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(
            dir.join("a.ufm"),
            "Good {\ninit:\n  z = 0\nloop:\n  z = z^2 + #pixel\nbailout:\n  |z| < 4\ndefault:\n  title = \"Good one\"\n  center = (-0.5, 0)\n  magn = 2\n}\n\nBad {\ninit:\n  z = 0\nloop:\n  z = nosuchfunction(z)\nbailout:\n  |z| < 4\n}\n",
        )
        .unwrap();
        std::fs::write(dir.join("sub/old.frm"), "Mandel(XAXIS) { ; comment\n  z = pixel:\n  z = sqr(z) + pixel\n  |z| <= 4\n}\n").unwrap();
        std::fs::write(dir.join("sub/c.ucl"), "Plain {\nfinal:\n  #index = 0.5\n}\n").unwrap();
        let entries = scan_folders(std::slice::from_ref(&dir));
        let ids: Vec<&str> = entries.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["user:a.ufm:Good", "user:a.ufm:Bad", "user:sub/c.ucl:Plain", "user:sub/old.frm:Mandel"]);
        let good = &entries[0];
        assert_eq!(good.title, "Good one");
        assert_eq!(good.view.center, [-0.5, 0.0]);
        assert!((good.view.radius - 0.75).abs() < 1e-12);
        // Nothing is compiled while indexing; the first use compiles.
        assert_eq!(good.status(), None);
        assert!(good.program().is_ok());
        assert_eq!(good.status(), Some(true));
        assert!(entries[1].program().is_err());
        assert_eq!(entries[2].kind, ProgramKind::Coloring);
        assert!(entries[2].program().is_ok());
        assert!(entries[3].program().is_ok(), "{:?}", entries[3].program().err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn every_bundled_formula_compiles_and_runs() {
        let lib = library();
        let fp = lib.entries.iter().filter(|e| e.set == Set::FractalPark).count();
        let rf = lib.entries.iter().filter(|e| e.set == Set::RFract && e.kind == ProgramKind::Fractal).count();
        assert!(lib.colorings().count() >= 13, "colorings: {}", lib.colorings().count());
        let broken: Vec<String> = lib
            .entries
            .iter()
            .filter(|e| e.kind != ProgramKind::Fractal)
            .filter_map(|e| e.program().err().map(|err| format!("{}: {err}", e.id)))
            .collect();
        assert!(broken.is_empty(), "{}", broken.join("\n"));
        assert!(lib.transforms().count() >= 11, "transforms: {}", lib.transforms().count());
        assert_eq!(fp, 534, "FractalPark entries");
        assert!(rf >= 17, "RFract entries: {rf}");
        let failed: Vec<String> = lib.entries.iter().filter_map(|e| e.program().err().map(|err| format!("{} ({}): {err}", e.title, e.id))).collect();
        assert!(failed.is_empty(), "{} formulas fail:\n{}", failed.len(), failed.join("\n"));
        // Every formula runs on the CPU, plain and with derivatives, at a few points.
        // Colorings and transformations run on a Mandelbrot orbit.
        let m = lib.get("rfract:Mandelbrot").unwrap().program().unwrap();
        let fractal = Compiled::<f64>::new(&m, &[]);
        for e in lib.colorings().chain(lib.transforms()) {
            let p = e.program().unwrap();
            let k = Compiled::<f64>::new(&p, &[]);
            let (mut ff, mut kf) = (fractal.frame(), k.frame());
            for c in [C64::new(0.3, 0.6), C64::new(-0.1, 0.2)] {
                if e.kind == ProgramKind::Transform {
                    let (q, _) = k.transform(&mut kf, c);
                    assert!(q.is_finite() || q.re.is_nan(), "{}: {q:?}", e.title);
                    continue;
                }
                ff.pixel = c;
                ff.c = c;
                kf.pixel = c;
                kf.c = c;
                let o = fractal.run(&mut ff, 200);
                let o2 = fractal.run_with(&mut ff, 200, |h| match h {
                    super::super::cpu::Hook::Init(z) => {
                        kf.zcur = z;
                        k.color_init(&mut kf, 200);
                    }
                    super::super::cpu::Hook::Iter(it, z, zp) => {
                        kf.zcur = z;
                        k.color_loop(&mut kf, it, zp);
                    }
                });
                assert_eq!(o.iter, o2.iter);
                kf.zcur = o2.z;
                let (index, _) = k.color_final(&mut kf, o2.iter, o2.zprev);
                assert!(index.is_finite(), "{}: index {index}", e.title);
            }
        }
        for e in lib.usable() {
            let p = e.program().unwrap();
            let plain = Compiled::<f64>::new(&p, &[]);
            let dual = Compiled::<Dual>::new(&p, &[]);
            let (mut f, mut g) = (plain.frame(), dual.frame());
            for (k, d) in [(0.0, 0.0), (0.3, -0.2), (-1.2, 0.7)].iter().enumerate() {
                let px = C64::new(e.view.center[0] + d.0 * e.view.radius, e.view.center[1] + d.1 * e.view.radius);
                f.pixel = px;
                f.c = px;
                f.ismand = k != 2;
                let o = plain.run(&mut f, 64);
                g.pixel = crate::math::complex::Cx::new(Dual::new(px.re, 1.0), Dual::new(px.im, 0.0));
                g.c = g.pixel;
                g.ismand = k != 2;
                let od = dual.run(&mut g, 64);
                assert_eq!(o.iter, od.iter, "{}: plain and dual runs differ", e.title);
            }
        }
    }
}
