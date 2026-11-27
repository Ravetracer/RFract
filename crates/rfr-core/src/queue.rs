// SPDX-License-Identifier: LGPL-3.0-or-later

//! Render queue (batch rendering): images and animations rendered one after the other, for
//! example overnight. The queue is a JSON file; the app keeps one in its settings folder and
//! `rfr-cli batch` runs any queue file (or a list of documents). Adapted from RMandelbulb3D.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::doc::Document;
use crate::video::VideoFormat;

/// What a job renders.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "k")]
pub enum JobWhat {
    /// The document's view as a PNG image (with the document inside).
    Image,
    /// Frames `from..=to` of the document's animation (`to` beyond the end: to the last frame).
    Animation { format: VideoFormat, from: usize, to: usize },
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "k")]
pub enum JobState {
    #[default]
    Waiting,
    Running,
    Done {
        seconds: f64,
    },
    Failed {
        error: String,
    },
    /// Skipped until it is set to waiting again.
    Paused,
}

impl JobState {
    pub fn label(&self) -> String {
        match self {
            JobState::Waiting => "waiting".into(),
            JobState::Running => "rendering".into(),
            JobState::Done { seconds } => format!("done ({})", duration(*seconds)),
            JobState::Failed { error } => format!("failed: {error}"),
            JobState::Paused => "paused".into(),
        }
    }
}

/// Seconds as h:mm:ss or m:ss.
pub fn duration(s: f64) -> String {
    let s = s.max(0.0).round() as u64;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct QueueJob {
    pub id: u64,
    pub name: String,
    /// The document file to render (read when the job starts).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doc_file: Option<PathBuf>,
    /// Or the document itself (a copy taken when the job was added) and the folder of its
    /// file (relative sound paths).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doc: Option<Box<Document>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doc_dir: Option<PathBuf>,
    pub what: JobWhat,
    /// 0: the document's size and samples.
    pub width: u32,
    pub height: u32,
    pub samples: u32,
    /// Image file, movie file or folder of PNG frames.
    pub output: PathBuf,
    pub state: JobState,
}

impl Default for QueueJob {
    fn default() -> Self {
        QueueJob {
            id: 0,
            name: String::new(),
            doc_file: None,
            doc: None,
            doc_dir: None,
            what: JobWhat::Image,
            width: 0,
            height: 0,
            samples: 0,
            output: PathBuf::new(),
            state: JobState::Waiting,
        }
    }
}

impl QueueJob {
    /// The job's document (its copy or its file).
    pub fn load_doc(&self) -> Result<Document, String> {
        match (&self.doc, &self.doc_file) {
            (Some(d), _) => Ok((**d).clone()),
            (None, Some(f)) => crate::io::open(f),
            _ => Err("the job has no document".into()),
        }
    }

    /// The folder relative paths of the document refer to.
    pub fn base_dir(&self) -> Option<PathBuf> {
        self.doc_dir.clone().or_else(|| self.doc_file.as_ref().and_then(|f| f.parent()).map(Path::to_path_buf))
    }

    /// Image size and samples: the job's, or the document's (one side given: the other by the
    /// aspect).
    pub fn size(&self, doc: &Document) -> (u32, u32, u32) {
        let aspect = doc.image.width.max(1) as f64 / doc.image.height.max(1) as f64;
        let (w, h) = match (self.width, self.height) {
            (0, 0) => (doc.image.width, doc.image.height),
            (w, 0) => (w, ((w as f64) / aspect).round().max(1.0) as u32),
            (0, h) => (((h as f64) * aspect).round().max(1.0) as u32, h),
            (w, h) => (w, h),
        };
        let s = if self.samples == 0 { doc.image.samples } else { self.samples };
        (w.max(1), h.max(1), s.max(1))
    }

    /// One line describing the job: "1920 × 1080, 128 samples, MP4, frames 0-240, film.mp4".
    pub fn summary(&self) -> String {
        let size = match (self.width, self.height) {
            (0, 0) => "document size".to_string(),
            (w, 0) => format!("{w} wide"),
            (0, h) => format!("{h} high"),
            (w, h) => format!("{w} × {h}"),
        };
        let samples = if self.samples == 0 { "document samples".to_string() } else { format!("{} samples", self.samples) };
        let what = match self.what {
            JobWhat::Image => "PNG".to_string(),
            JobWhat::Animation { format, from, to } => {
                let to = if to == usize::MAX { "end".to_string() } else { to.to_string() };
                format!("{}, frames {from}-{to}", format.label())
            }
        };
        let out = self.output.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        format!("{size}, {samples}, {what}, {out}")
    }

    /// A job for a document file: its animation when it has keyframes, else an image, saved
    /// next to the file (or in `out_dir`).
    pub fn for_file(path: &Path, out_dir: Option<&Path>) -> Result<QueueJob, String> {
        let doc = crate::io::open(path)?;
        let animated = doc.animation.as_ref().is_some_and(|a| !a.keyframes.is_empty());
        let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "render".into());
        let dir = out_dir.map(Path::to_path_buf).or_else(|| path.parent().map(Path::to_path_buf)).unwrap_or_default();
        let (what, output) = if animated {
            (JobWhat::Animation { format: VideoFormat::Mp4, from: 0, to: usize::MAX }, dir.join(format!("{stem}.mp4")))
        } else {
            (JobWhat::Image, dir.join(format!("{stem}.png")))
        };
        Ok(QueueJob { id: new_id(), name: stem, doc_file: Some(path.to_path_buf()), what, output, ..Default::default() })
    }
}

/// A new job id (time based, unique within a queue in practice).
pub fn new_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static LAST: AtomicU64 = AtomicU64::new(0);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_micros() as u64).unwrap_or(1);
    let mut prev = LAST.load(Ordering::Relaxed);
    loop {
        let next = now.max(prev + 1);
        match LAST.compare_exchange(prev, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return next,
            Err(p) => prev = p,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Queue {
    pub jobs: Vec<QueueJob>,
}

impl Queue {
    pub fn load(path: &Path) -> Result<Queue, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut q: Queue = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        // A job that was running when the program stopped starts again.
        for j in &mut q.jobs {
            if j.state == JobState::Running {
                j.state = JobState::Waiting;
            }
        }
        Ok(q)
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self).unwrap_or_default())
            .and_then(|_| std::fs::rename(&tmp, path))
            .map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Index of the next waiting job.
    pub fn next_waiting(&self) -> Option<usize> {
        self.jobs.iter().position(|j| j.state == JobState::Waiting)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_round_trips_and_restarts_running_jobs() {
        let dir = std::env::temp_dir().join(format!("rfr-queue-{}", std::process::id()));
        let path = dir.join("queue.json");
        let mut q = Queue::default();
        let doc = Document::default();
        q.jobs.push(QueueJob { id: new_id(), name: "a".into(), doc: Some(Box::new(doc.clone())), state: JobState::Running, ..Default::default() });
        q.jobs.push(QueueJob {
            id: new_id(),
            name: "b".into(),
            what: JobWhat::Animation { format: VideoFormat::PngSequence, from: 0, to: usize::MAX },
            width: 640,
            ..Default::default()
        });
        assert_ne!(q.jobs[0].id, q.jobs[1].id);
        q.save(&path).unwrap();
        let back = Queue::load(&path).unwrap();
        assert_eq!(back.jobs[0].state, JobState::Waiting);
        assert_eq!(back.jobs[1].what, q.jobs[1].what);
        assert_eq!(back.next_waiting(), Some(0));
        assert_eq!(back.jobs[0].size(&doc), (doc.image.width, doc.image.height, doc.image.samples));
        let (w, h, _) = back.jobs[1].size(&doc);
        assert_eq!((w, h), (640, (640.0 * doc.image.height as f64 / doc.image.width as f64).round() as u32));
        assert_eq!(back.jobs[0].load_doc().unwrap(), doc);
        assert!(back.jobs[1].load_doc().is_err());
        std::fs::remove_dir_all(&dir).ok();
    }
}
