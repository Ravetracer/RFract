// SPDX-License-Identifier: LGPL-3.0-or-later

//! The files of a job: the server lists them with their hashes and makes the document's image
//! paths absolute; a client keeps received files by hash in its cache, recreates the server's
//! image paths below a job folder and the user formula files in the job's formula folder.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use rfr_core::doc::Document;

use crate::proto::{FileEntry, sha256};

/// A document with absolute image paths and the files it needs (server side).
pub struct Prepared {
    pub doc: Document,
    /// Local path and entry of every file.
    pub files: Vec<(PathBuf, FileEntry)>,
}

fn absolute(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| if p.is_absolute() { p.to_path_buf() } else { std::env::current_dir().unwrap_or_default().join(p) })
}

/// Hashes of files by path, size and modification time (large images are hashed once).
fn hash_file(p: &Path) -> Result<(u64, String), String> {
    type Hashes = HashMap<(PathBuf, u64, SystemTime), String>;
    static CACHE: Mutex<Option<Hashes>> = Mutex::new(None);
    let meta = std::fs::metadata(p).map_err(|e| format!("{}: {e}", p.display()))?;
    let key = (p.to_path_buf(), meta.len(), meta.modified().unwrap_or(SystemTime::UNIX_EPOCH));
    if let Some(h) = CACHE.lock().ok().and_then(|c| c.as_ref().and_then(|m| m.get(&key).cloned())) {
        return Ok((meta.len(), h));
    }
    let bytes = std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?;
    let h = sha256(&bytes);
    if let Ok(mut c) = CACHE.lock() {
        c.get_or_insert_with(HashMap::new).insert(key, h.clone());
    }
    Ok((meta.len(), h))
}

/// The path of `file` inside one of the formula folders (`/`-separated).
fn formula_path(file: &Path, folders: &[PathBuf]) -> Option<String> {
    folders.iter().find_map(|root| {
        let rel = file.strip_prefix(root).ok()?;
        Some(rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/"))
    })
}

/// Class library files (`.ulb`) below `dir`.
fn class_files(dir: &Path, out: &mut Vec<PathBuf>) {
    if dir.is_dir() {
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                class_files(&e.path(), out);
            }
        }
    } else if dir.extension().is_some_and(|e| e.eq_ignore_ascii_case("ulb")) {
        out.push(dir.to_path_buf());
    }
}

/// Server: makes the document's image paths absolute and lists the files with their hashes
/// (images, the user formula files the document uses and, when one of them uses classes, the
/// class libraries of the formula folders).
pub fn prepare(doc: &Document) -> Result<Prepared, String> {
    let mut doc = doc.clone();
    let mut images: Vec<PathBuf> = Vec::new();
    rfr_core::files::visit_images(&mut doc, &mut |name| {
        if let Some(f) = rfr_core::images::resolve(name) {
            let f = absolute(&f);
            *name = f.to_string_lossy().into_owned();
            images.push(f);
        }
    });
    let folders: Vec<PathBuf> = rfr_core::formula::library::user_folders().iter().map(|f| absolute(f)).collect();
    let lib = rfr_core::formula::library::library();
    let mut formulas: Vec<PathBuf> = Vec::new();
    let mut classes = false;
    for id in rfr_core::files::formula_ids(&doc).iter().filter(|id| id.starts_with("user:")) {
        let Some(e) = lib.get(id) else { return Err(format!("formula {id} not found in the formula folders")) };
        if let Some(p) = e.path() {
            formulas.push(absolute(p));
        }
        classes |= e.uses_classes();
    }
    if classes {
        for root in &folders {
            class_files(root, &mut formulas);
        }
    }
    let mut paths: Vec<(PathBuf, Option<String>)> = images.into_iter().map(|p| (p, None)).collect();
    for f in formulas {
        let rel = formula_path(&f, &folders).ok_or_else(|| format!("{} is not in a formula folder", f.display()))?;
        paths.push((f, Some(rel)));
    }
    paths.sort();
    paths.dedup();
    let mut files = Vec::new();
    for (p, formula) in paths {
        if !p.is_file() {
            continue;
        }
        let (size, hash) = hash_file(&p)?;
        files.push((p.clone(), FileEntry { path: p.to_string_lossy().into_owned(), size, hash, formula }));
    }
    Ok(Prepared { doc, files })
}

/// A server path below `root`: drive letters and separators of any system, without `..`.
pub fn mirror(root: &Path, server_path: &str) -> PathBuf {
    let mut out = root.to_path_buf();
    let unified = server_path.replace('\\', "/");
    for part in Path::new(&unified).components() {
        match part {
            Component::Normal(s) => out.push(s.to_string_lossy().replace(':', "")),
            Component::Prefix(p) => out.push(p.as_os_str().to_string_lossy().replace([':', '\\', '/', '?', '.'], "")),
            _ => {}
        }
    }
    out
}

/// Client: the cache of received files, by hash.
pub struct Cache {
    pub root: PathBuf,
}

impl Cache {
    /// The cache below `dir`, or the user's cache folder.
    pub fn new(dir: Option<PathBuf>) -> Cache {
        let root = dir.unwrap_or_else(|| rfr_core::paths::cache_dir().join("net"));
        Cache { root }
    }

    fn blob(&self, hash: &str) -> PathBuf {
        self.root.join("files").join(hash.chars().filter(|c| c.is_ascii_hexdigit()).collect::<String>())
    }

    pub fn has(&self, hash: &str) -> bool {
        self.blob(hash).is_file()
    }

    /// Stores received bytes after checking their hash.
    pub fn store(&self, hash: &str, bytes: &[u8]) -> Result<(), String> {
        if sha256(bytes) != hash {
            return Err("a file arrived damaged".into());
        }
        let p = self.blob(hash);
        std::fs::create_dir_all(p.parent().unwrap_or(&self.root)).map_err(|e| e.to_string())?;
        let tmp = p.with_extension("part");
        std::fs::write(&tmp, bytes).and_then(|_| std::fs::rename(&tmp, &p)).map_err(|e| format!("{}: {e}", p.display()))
    }

    /// Folder of a job (recreated server paths, formulas).
    pub fn job_dir(&self, job: u64) -> PathBuf {
        self.root.join("jobs").join(format!("{job:016x}"))
    }

    /// The job's formula folder.
    pub fn formula_dir(&self, job: u64) -> PathBuf {
        self.job_dir(job).join("formulas")
    }

    /// Folder of finished results not yet confirmed by the server.
    pub fn spool_dir(&self, job: u64) -> PathBuf {
        self.root.join("spool").join(format!("{job:016x}"))
    }

    /// Puts the job's files in place (hard links to the cache, copies where links fail): images
    /// at their server paths below `root/`, formula files at their paths in the formula folder.
    pub fn materialize(&self, job: u64, files: &[FileEntry]) -> Result<(), String> {
        let dir = self.job_dir(job);
        std::fs::create_dir_all(self.formula_dir(job)).map_err(|e| e.to_string())?;
        for f in files {
            let target = match &f.formula {
                Some(rel) => mirror(&self.formula_dir(job), rel),
                None => mirror(&dir.join("root"), &f.path),
            };
            if target.is_file() {
                continue;
            }
            std::fs::create_dir_all(target.parent().unwrap_or(&dir)).map_err(|e| e.to_string())?;
            let blob = self.blob(&f.hash);
            if std::fs::hard_link(&blob, &target).is_err() {
                std::fs::copy(&blob, &target).map_err(|e| format!("{}: {e}", target.display()))?;
            }
        }
        Ok(())
    }

    /// Rewrites the document's (absolute, server) image paths to the recreated ones.
    pub fn rewrite(&self, job: u64, doc: &mut Document) {
        let root = self.job_dir(job).join("root");
        rfr_core::files::visit_images(doc, &mut |name| {
            let absolute = name.starts_with('/') || name.starts_with('\\') || name.as_bytes().get(1) == Some(&b':');
            if absolute {
                *name = mirror(&root, name).to_string_lossy().into_owned();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mirrored_paths_stay_below_the_root() {
        let root = Path::new("/cache/job");
        assert_eq!(mirror(root, "/home/a/sky.png"), Path::new("/cache/job/home/a/sky.png"));
        assert_eq!(mirror(root, "C:\\Images\\sky.png"), Path::new("/cache/job/C/Images/sky.png"));
        assert_eq!(mirror(root, "/a/../../etc/passwd"), Path::new("/cache/job/a/etc/passwd"));
    }

    #[test]
    fn files_travel_by_hash_and_documents_point_at_them() {
        let dir = std::env::temp_dir().join(format!("rfr-net-assets-{}", std::process::id()));
        let src = dir.join("src");
        std::fs::create_dir_all(&src).unwrap();
        let img = src.join("sky.png");
        rfr_core::io::png::write_rgba8(&img, 2, 1, &[255; 8], None).unwrap();
        let mut doc = Document::default();
        doc.layers[0].light.environment = img.to_string_lossy().into_owned();
        let p = prepare(&doc).unwrap();
        assert_eq!(p.files.len(), 1);
        let cache = Cache::new(Some(dir.join("cache")));
        let (path, entry) = &p.files[0];
        assert!(!cache.has(&entry.hash));
        cache.store(&entry.hash, &std::fs::read(path).unwrap()).unwrap();
        assert!(cache.store(&entry.hash, b"other").is_err());
        cache.materialize(7, std::slice::from_ref(entry)).unwrap();
        let mut d = p.doc.clone();
        cache.rewrite(7, &mut d);
        let env = &d.layers[0].light.environment;
        assert!(Path::new(env).is_file(), "{env}");
        assert!(env.starts_with(&*cache.root.to_string_lossy()));
        std::fs::remove_dir_all(&dir).ok();
    }
}
