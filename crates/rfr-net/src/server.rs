// SPDX-License-Identifier: LGPL-3.0-or-later

//! The render server: sends a job to the chosen clients and collects the results.
//!
//! Clients pull work: an idle client asks for the next piece, so fast machines take more
//! pieces than slow ones. When nothing is left, an idle client also starts the piece whose
//! client will need the longest (by its measured speed, or its hardware before the first
//! piece); the first result wins and the other client is stopped. Frames are kept as PNG files
//! and encoded after the last one; the tiles of a still image are finished (denoised, effects)
//! as they arrive and written to the PNG a band of rows at a time. With density layers the
//! clients first count the densities of every tile; the tiles follow with the whole image's
//! largest counts.

use std::collections::{HashMap, HashSet, VecDeque};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rfr_core::doc::Document;
use rfr_core::render::post::Glow;
use rfr_gpu::GpuContext;

use crate::proto::{JobKind, JobSpec, Msg, Specs, Unit, recv, send, sha256};

/// Where a job's results go.
#[derive(Clone, Debug)]
pub enum Output {
    /// Animation frames: a movie (`format`) or a folder of PNG files; `ffmpeg` encodes movies
    /// after the last frame.
    Animation { path: PathBuf, format: rfr_core::video::VideoFormat, ffmpeg: PathBuf },
    /// A still image (PNG with the document inside).
    Still { path: PathBuf },
}

#[derive(Clone)]
pub struct JobRequest {
    pub doc: Document,
    /// Folder of the document file (relative sound paths of movies).
    pub doc_dir: Option<PathBuf>,
    pub width: u32,
    pub height: u32,
    pub samples: u32,
    pub kind: JobKind,
    pub output: Output,
    /// Shared key for clients that need one.
    pub key: Option<String>,
    /// The server's graphics card (the glow of a still image renders here; `None`: the CPU).
    pub gpu: Option<GpuContext>,
    /// Still images: pixels per tile side (0: [`crate::TILE`]).
    pub tile: u32,
}

/// A client to render on.
#[derive(Clone, Debug)]
pub struct Target {
    pub name: String,
    pub addr: SocketAddr,
    pub specs: Specs,
}

/// State of one client during a job (for the display).
#[derive(Clone, Debug, Default)]
pub struct ClientStatus {
    pub name: String,
    pub specs: Specs,
    pub state: String,
    pub unit: Option<Unit>,
    pub fraction: f32,
    pub units_done: usize,
    /// Seconds per unit of work weight (see `Unit::weight`), once measured.
    pub speed: Option<f64>,
}

/// State of a job (for the display).
#[derive(Clone, Debug, Default)]
pub struct Status {
    pub clients: Vec<ClientStatus>,
    pub done: usize,
    pub total: usize,
    pub started: Option<Instant>,
    /// Counting densities, rendering, encoding the movie…
    pub phase: String,
    pub finished: bool,
    pub error: Option<String>,
    pub animation: bool,
}

impl ClientStatus {
    /// Measured speed as text: seconds per frame, or per tile of a still image.
    pub fn speed_text(&self, status: &Status) -> String {
        match self.speed {
            Some(v) if status.animation => format!("{v:.1} s/frame"),
            Some(v) => format!("{v:.1} s/tile"),
            None => String::new(),
        }
    }
}

impl Status {
    /// Share of the job done, counting the pieces in progress.
    pub fn fraction(&self) -> f32 {
        if self.total == 0 {
            return 0.0;
        }
        let running: f32 = self.clients.iter().filter(|c| c.unit.is_some()).map(|c| c.fraction).sum();
        ((self.done as f32 + running.min((self.total - self.done) as f32)) / self.total as f32).min(1.0)
    }
}

/// Shared work state.
struct Work {
    queue: VecDeque<Unit>,
    /// Units that wait for others (the tiles of an image with density layers wait for its
    /// density counts).
    held: VecDeque<Unit>,
    /// Units in progress: client index, start time, fraction.
    running: HashMap<Unit, Vec<(usize, Instant, f32)>>,
    done: HashSet<Unit>,
    total: usize,
    /// Seconds per weight unit per client (measured) and a guess from the hardware.
    speed: Vec<Option<f64>>,
    guess: Vec<f64>,
}

impl Work {
    fn finished(&self) -> bool {
        self.done.len() >= self.total
    }

    fn time_per_weight(&self, c: usize) -> f64 {
        self.speed[c].unwrap_or(self.guess[c])
    }

    /// The next unit for client `c`: the queue first, then a duplicate of the unit whose client
    /// needs the longest when `c` would clearly be faster.
    fn next(&mut self, c: usize) -> Option<Unit> {
        if let Some(u) = self.queue.pop_front() {
            self.running.entry(u).or_default().push((c, Instant::now(), 0.0));
            return Some(u);
        }
        let mine = self.time_per_weight(c);
        let mut best: Option<(Unit, f64)> = None;
        for (u, workers) in &self.running {
            if workers.len() >= 2 || workers.iter().any(|w| w.0 == c) {
                continue;
            }
            let (owner, start, frac) = workers[0];
            let weight = u.weight();
            let total = weight * self.time_per_weight(owner);
            let left = (total * (1.0 - frac as f64)).max(total - start.elapsed().as_secs_f64());
            let mine_total = weight * mine;
            if mine_total < 0.7 * left && best.is_none_or(|b| left > b.1) {
                best = Some((*u, left));
            }
        }
        let (u, _) = best?;
        self.running.entry(u).or_default().push((c, Instant::now(), 0.0));
        Some(u)
    }

    /// Unit `u` of client `c` failed or the connection broke: back into the queue unless
    /// someone else works on it.
    fn give_back(&mut self, c: usize, u: Unit) {
        if let Some(w) = self.running.get_mut(&u) {
            w.retain(|x| x.0 != c);
            if w.is_empty() {
                self.running.remove(&u);
                if !self.done.contains(&u) {
                    self.queue.push_front(u);
                }
            }
        }
    }
}

/// A still image: the tiles finished as they arrive and written in bands of rows.
struct Still {
    doc: Document,
    w: u32,
    h: u32,
    tiles: Vec<[u32; 4]>,
    glow: Glow,
    /// Density layers: the largest counts so far, and the tiles still to count.
    maxima: Vec<Vec<[u32; 3]>>,
    counting: usize,
    /// Finished tiles (8-bit RGBA) not written yet, the PNG and the next row to write.
    pending: HashMap<u32, Vec<u8>>,
    png: Option<rfr_core::io::png::RowWriter>,
    next_row: u32,
    note: Option<String>,
}

impl Still {
    fn add_maxima(&mut self, bytes: &[u8]) -> Result<(), String> {
        let m: Vec<Vec<[u32; 3]>> = serde_json::from_slice(bytes).map_err(|e| format!("density counts arrived damaged: {e}"))?;
        self.maxima = if self.maxima.is_empty() { m } else { rfr_core::render::tiles::merge_maxima(&self.maxima, &m) };
        self.counting = self.counting.saturating_sub(1);
        Ok(())
    }

    fn add_tile(&mut self, i: u32, bytes: &[u8]) -> Result<(), String> {
        let tile = *self.tiles.get(i as usize).ok_or("a tile that does not exist arrived")?;
        let (region, lin) = crate::client::decode_region(bytes)?;
        if region != rfr_gpu::poster::tile_region(tile, self.w, self.h, &self.doc.post) {
            return Err("a tile of the wrong size arrived".into());
        }
        let (rgba, note) = rfr_gpu::poster::finish_tile(lin, region, tile, (self.w, self.h), &self.doc, &self.glow);
        self.note = note.or(self.note.take());
        self.pending.insert(i, rgba);
        self.write_bands()
    }

    /// Writes every band of rows whose tiles are all there, from the top.
    fn write_bands(&mut self) -> Result<(), String> {
        loop {
            let band: Vec<u32> = (0..self.tiles.len() as u32).filter(|&k| self.tiles[k as usize][1] == self.next_row).collect();
            if band.is_empty() || !band.iter().all(|k| self.pending.contains_key(k)) {
                return Ok(());
            }
            let bh = self.tiles[band[0] as usize][3];
            let mut rows = vec![0u8; (self.w * bh * 4) as usize];
            for k in band {
                let t = self.tiles[k as usize];
                let src = self.pending.remove(&k).unwrap_or_default();
                for j in 0..t[3] as usize {
                    let line = &src[j * t[2] as usize * 4..(j + 1) * t[2] as usize * 4];
                    let at = (j * self.w as usize + t[0] as usize) * 4;
                    rows[at..at + line.len()].copy_from_slice(line);
                }
            }
            self.png.as_mut().ok_or("the image is already written")?.write_rows(&rows)?;
            self.next_row += bh;
        }
    }
}

/// A rough time per weight unit from the hardware (only until the first piece is measured):
/// graphics cards far ahead of processors.
fn guess_time(s: &Specs) -> f64 {
    if s.gpu.is_empty() { 1.0 / (s.threads.max(1) as f64) * 50.0 } else { 0.5 }
}

struct Shared {
    work: Mutex<Work>,
    status: Arc<Mutex<Status>>,
    still: Option<Mutex<Still>>,
    cancel: Arc<AtomicBool>,
    /// Frames folder (animations) and the first frame.
    frames_dir: Option<PathBuf>,
    from: usize,
}

/// Frames per piece of a zoom sequence (its images carry over from frame to frame).
fn sequence_run(frames: usize, clients: usize) -> usize {
    (frames / (clients.max(1) * 4)).clamp(1, 60)
}

/// Runs a job on `targets` (blocking). `status` is updated for a display; setting `cancel`
/// stops the job. Returns a summary of what was written.
pub fn run_job(req: JobRequest, targets: Vec<Target>, status: Arc<Mutex<Status>>, cancel: Arc<AtomicBool>) -> Result<String, String> {
    if targets.is_empty() {
        return Err("no clients selected".into());
    }
    let set_phase = |p: &str| {
        if let Ok(mut s) = status.lock() {
            s.phase = p.to_string();
        }
    };
    set_phase("Preparing the files");
    // Audio-reactive drivers: the clients get the sound's levels with the document.
    let mut req = req;
    if let Output::Animation { ffmpeg, .. } = &req.output
        && let Some(a) = req.doc.animation.as_mut()
    {
        a.ensure_audio_levels(req.doc_dir.as_deref(), ffmpeg)?;
    }
    let prepared = crate::assets::prepare(&req.doc)?;
    let job_id = crate::proto::random_u64();
    let (w, h) = (req.width.max(1), req.height.max(1));
    let tile = if req.tile == 0 { crate::TILE } else { req.tile.max(16) };
    let mut still = None;
    let (units, held): (Vec<Unit>, Vec<Unit>) = match req.kind {
        JobKind::Animation { from, to, sequence } => {
            if to < from {
                return Err("no frames to render".into());
            }
            let n = if sequence { sequence_run(to + 1 - from, targets.len()) } else { 1 };
            ((from..=to).step_by(n).map(|f| Unit::Frames { f, n: n.min(to + 1 - f) }).collect(), Vec::new())
        }
        JobKind::Still => {
            let Output::Still { path } = &req.output else { return Err("the output does not match the job".into()) };
            let tiles = rfr_core::render::tiles::tiles_sized(w, h, tile);
            let whole = rfr_core::render::Plan::sized(&req.doc, w, h)?;
            let density = whole.layers().iter().any(|l| l.density.is_some());
            set_phase("Rendering the glow");
            let glow = rfr_gpu::poster::whole_glow(&mut rfr_gpu::animation::FrameRenderer::new(req.gpu.clone()), &req.doc, w, h, req.samples.max(1))?;
            let mut saved = req.doc.clone();
            (saved.image.width, saved.image.height, saved.image.samples) = (w, h, req.samples);
            if let Some(d) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
                std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
            }
            let png = rfr_core::io::png::RowWriter::new(path, w, h, Some(&saved))?;
            let n = tiles.len() as u32;
            still = Some(Still {
                doc: req.doc.clone(),
                w,
                h,
                tiles,
                glow,
                maxima: Vec::new(),
                counting: if density { n as usize } else { 0 },
                pending: HashMap::new(),
                png: Some(png),
                next_row: 0,
                note: None,
            });
            let tiles: Vec<Unit> = (0..n).map(|i| Unit::Tile { i }).collect();
            if density { ((0..n).map(|i| Unit::Maxima { i }).collect(), tiles) } else { (tiles, Vec::new()) }
        }
    };
    let (frames_dir, from) = match (&req.output, req.kind) {
        (Output::Animation { path, .. }, JobKind::Animation { from, .. }) => {
            let dir = path.with_extension("frames");
            std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            (Some(dir), from)
        }
        (Output::Still { .. }, JobKind::Still) => (None, 0),
        _ => return Err("the output does not match the job".into()),
    };
    let spec = JobSpec {
        job_id,
        doc: prepared.doc.to_json(),
        width: w,
        height: h,
        samples: req.samples.max(1),
        kind: req.kind,
        files: prepared.files.iter().map(|f| f.1.clone()).collect(),
        tile,
    };
    let total = units.len() + held.len();
    {
        let mut s = status.lock().map_err(|_| "status lock")?;
        *s = Status {
            clients: targets
                .iter()
                .map(|t| ClientStatus { name: t.name.clone(), specs: t.specs.clone(), state: "connecting".into(), ..Default::default() })
                .collect(),
            done: 0,
            total,
            started: Some(Instant::now()),
            phase: if held.is_empty() { "Rendering" } else { "Counting the densities" }.into(),
            finished: false,
            error: None,
            animation: matches!(req.kind, JobKind::Animation { .. }),
        };
    }
    let shared = Arc::new(Shared {
        work: Mutex::new(Work {
            queue: units.into(),
            held: held.into(),
            running: HashMap::new(),
            done: HashSet::new(),
            total,
            speed: vec![None; targets.len()],
            guess: targets.iter().map(|t| guess_time(&t.specs)).collect(),
        }),
        status: status.clone(),
        still: still.map(Mutex::new),
        cancel: cancel.clone(),
        frames_dir: frames_dir.clone(),
        from,
    });
    let files: Arc<HashMap<String, PathBuf>> = Arc::new(prepared.files.iter().map(|(p, e)| (e.hash.clone(), p.clone())).collect());
    let spec = Arc::new(spec);
    let handles: Vec<_> = targets
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let (shared, files, spec, key, t) = (shared.clone(), files.clone(), spec.clone(), req.key.clone(), t.clone());
            std::thread::spawn(move || client_thread(i, t, &shared, &spec, &files, key.as_deref()))
        })
        .collect();
    // Wait until every piece is done, the job is cancelled or no client is left.
    let mut gave_up = false;
    loop {
        std::thread::sleep(Duration::from_millis(200));
        let finished = shared.work.lock().map(|w| w.finished()).unwrap_or(true);
        if finished || cancel.load(Ordering::Relaxed) {
            break;
        }
        if handles.iter().all(|h| h.is_finished()) {
            gave_up = true;
            break;
        }
    }
    let done_all = shared.work.lock().map(|w| w.finished()).unwrap_or(false);
    if !done_all {
        cancel.store(true, Ordering::Relaxed);
    }
    for h in handles {
        let _ = h.join();
    }
    if cancel.load(Ordering::Relaxed) && !done_all {
        let reason = if gave_up { "no client could continue (see their states)" } else { "cancelled" };
        if let Ok(mut s) = status.lock() {
            s.error = Some(reason.into());
            s.finished = true;
        }
        // Nothing half-written stays behind.
        match &req.output {
            Output::Still { path } => {
                drop(shared.still.as_ref().and_then(|s| s.lock().ok().and_then(|mut s| s.png.take())));
                let _ = std::fs::remove_file(path);
            }
            Output::Animation { .. } => {
                if let Some(d) = &frames_dir {
                    let _ = std::fs::remove_dir_all(d);
                }
            }
        }
        return Err(reason.into());
    }
    let summary = finish(&req, &shared, &set_phase);
    if let Ok(mut s) = status.lock() {
        s.finished = true;
        s.phase = "Finished".into();
        if let Err(e) = &summary {
            s.error = Some(e.clone());
        }
    }
    summary
}

/// Writes the movie or closes the still image after the last piece.
fn finish(req: &JobRequest, shared: &Shared, set_phase: &impl Fn(&str)) -> Result<String, String> {
    match (&req.output, req.kind) {
        (Output::Animation { path, format, ffmpeg }, JobKind::Animation { from, to, .. }) => {
            let dir = shared.frames_dir.clone().ok_or("no frames folder")?;
            let n = to + 1 - from;
            set_phase(if format.needs_ffmpeg() { "Encoding the movie" } else { "Writing the frames" });
            let anim = req.doc.animation.as_deref().cloned().ok_or("the document has no animation")?;
            let plan = rfr_core::animation::AnimationPlan::new(&anim)?;
            let audio = anim.audio_track(from, req.doc_dir.as_deref());
            let mut sink = rfr_core::video::VideoSink::new(path, *format, req.width, req.height, plan.fps(), ffmpeg, audio.as_ref())?;
            for i in 0..n {
                let file = dir.join(frame_name(i));
                let (_, _, rgba) = rfr_core::io::png::read_rgba8(&file)?;
                sink.write_frame(i, &rgba)?;
            }
            sink.finish()?;
            let _ = std::fs::remove_dir_all(&dir);
            Ok(format!("{} ({n} frames of {} × {})", path.display(), req.width, req.height))
        }
        (Output::Still { path }, JobKind::Still) => {
            let mut st = shared.still.as_ref().ok_or("no image")?.lock().map_err(|_| "image lock")?;
            st.write_bands()?;
            st.png.take().ok_or("the image is already written")?.finish()?;
            let tiles = st.tiles.len();
            let note = st.note.take().map(|n| format!(", {n}")).unwrap_or_default();
            Ok(format!("{} ({} × {} in {tiles} tiles, {} samples{note})", path.display(), req.width, req.height, req.samples))
        }
        _ => Err("the output does not match the job".into()),
    }
}

fn frame_name(i: usize) -> String {
    format!("frame_{i:05}.png")
}

fn set_client(shared: &Shared, i: usize, f: impl FnOnce(&mut ClientStatus)) {
    if let Ok(mut s) = shared.status.lock()
        && let Some(c) = s.clients.get_mut(i)
    {
        f(c);
    }
}

/// Talks to one client until the job is done; reconnects after errors (a restarted client).
fn client_thread(i: usize, t: Target, shared: &Shared, spec: &JobSpec, files: &HashMap<String, PathBuf>, key: Option<&str>) {
    let mut failures = 0;
    loop {
        if shared.cancel.load(Ordering::Relaxed) || shared.work.lock().map(|w| w.finished()).unwrap_or(true) {
            set_client(shared, i, |c| {
                c.state = "done".into();
                c.unit = None;
            });
            return;
        }
        let mut current: Option<Unit> = None;
        let r = session(i, &t, shared, spec, files, key, &mut current);
        if let Some(u) = current
            && let Ok(mut w) = shared.work.lock()
        {
            w.give_back(i, u);
        }
        match r {
            Ok(()) => {
                set_client(shared, i, |c| {
                    c.state = "done".into();
                    c.unit = None;
                });
                return;
            }
            Err(Fatal(e)) => {
                set_client(shared, i, |c| {
                    c.state = e;
                    c.unit = None;
                });
                return;
            }
            Err(Retry(e)) => {
                failures += 1;
                set_client(shared, i, |c| {
                    c.state = format!("{e}; trying again");
                    c.unit = None;
                });
                // Back off a little more after every failure (up to 20 s).
                let wait = Duration::from_secs((2 * failures).min(20));
                let until = Instant::now() + wait;
                while Instant::now() < until && !shared.cancel.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(200));
                }
            }
        }
    }
}

enum SessionError {
    /// Give up on this client (wrong version or key).
    Fatal(String),
    /// Try again later (connection lost, client busy or restarting).
    Retry(String),
}
use SessionError::{Fatal, Retry};

fn retry<T>(r: std::io::Result<T>) -> Result<T, SessionError> {
    r.map_err(|e| Retry(format!("connection: {e}")))
}

#[allow(clippy::too_many_arguments)]
fn session(
    i: usize,
    t: &Target,
    shared: &Shared,
    spec: &JobSpec,
    files: &HashMap<String, PathBuf>,
    key: Option<&str>,
    current: &mut Option<Unit>,
) -> Result<(), SessionError> {
    set_client(shared, i, |c| c.state = "connecting".into());
    let mut s = retry(TcpStream::connect_timeout(&t.addr, Duration::from_secs(4)))?;
    let _ = s.set_nodelay(true);
    // A client busy with another server does not answer: give up on it quickly.
    retry(s.set_read_timeout(Some(Duration::from_secs(10))))?;
    let (m, _) = recv(&mut s).map_err(|_| Retry("busy or not answering".into()))?;
    // A piece reports only between frames: a deep frame may take long.
    retry(s.set_read_timeout(Some(Duration::from_secs(4 * 3600))))?;
    let Msg::Welcome { version, nonce, key: needs_key, .. } = m else { return Err(Retry("unexpected message".into())) };
    if version != crate::version() {
        return Err(Fatal(format!("version {version} (needs {})", crate::version())));
    }
    let auth = match (needs_key, key) {
        (true, Some(k)) => Some(crate::proto::auth(&nonce, k)),
        (true, None) => return Err(Fatal("needs a key (--key)".into())),
        _ => None,
    };
    let server = crate::host_name();
    retry(send(&mut s, &Msg::Hello { version: crate::version(), server, auth }, None))?;
    match retry(recv(&mut s))?.0 {
        Msg::Accepted => {}
        Msg::Rejected { reason } => return Err(Fatal(format!("rejected: {reason}"))),
        // A client rendering for another server: try again later.
        _ => return Err(Retry("busy".into())),
    }
    set_client(shared, i, |c| c.state = "sending files".into());
    retry(send(&mut s, &Msg::Job(Box::new(spec.clone())), None))?;
    let (m, _) = retry(recv(&mut s))?;
    let Msg::Need { hashes } = m else { return Err(Retry("unexpected message".into())) };
    for (k, h) in hashes.iter().enumerate() {
        let Some(path) = files.get(h) else { continue };
        let bytes = std::fs::read(path).map_err(|e| Fatal(format!("{}: {e}", path.display())))?;
        retry(send(&mut s, &Msg::File { hash: h.clone(), bytes: bytes.len() as u64 }, Some(&bytes)))?;
        set_client(shared, i, |c| c.state = format!("sending files ({}/{})", k + 1, hashes.len()));
    }
    retry(send(&mut s, &Msg::FilesDone, None))?;
    set_client(shared, i, |c| c.state = "ready".into());
    loop {
        let (m, blob) = retry(recv(&mut s))?;
        match m {
            Msg::Ready => {
                if shared.cancel.load(Ordering::Relaxed) {
                    let _ = send(&mut s, &Msg::Done, None);
                    return Ok(());
                }
                let (next, finished, done, total) = {
                    let mut w = shared.work.lock().map_err(|_| Fatal("lock".into()))?;
                    let n = w.next(i);
                    (n, w.finished(), w.done.len(), w.total)
                };
                // Tiles of an image with density layers: the whole image's largest counts.
                let density_max = match (next, &shared.still) {
                    (Some(Unit::Tile { .. }), Some(st)) => st.lock().map(|s| s.maxima.clone()).unwrap_or_default(),
                    _ => Vec::new(),
                };
                match next {
                    Some(u) => {
                        *current = Some(u);
                        set_client(shared, i, |c| {
                            c.state = "rendering".into();
                            c.unit = Some(u);
                            c.fraction = 0.0;
                        });
                        retry(send(&mut s, &Msg::Work { unit: u, done, total, density_max }, None))?;
                    }
                    None if finished => {
                        retry(send(&mut s, &Msg::Done, None))?;
                        let _ = recv(&mut s);
                        return Ok(());
                    }
                    None => {
                        set_client(shared, i, |c| {
                            c.state = "waiting for the others".into();
                            c.unit = None;
                        });
                        retry(send(&mut s, &Msg::Wait, None))?;
                    }
                }
            }
            Msg::Progress { unit, fraction } => {
                let (cancelled, done, total) = {
                    let mut w = shared.work.lock().map_err(|_| Fatal("lock".into()))?;
                    if let Some(ws) = w.running.get_mut(&unit) {
                        for x in ws.iter_mut().filter(|x| x.0 == i) {
                            x.2 = fraction;
                        }
                    }
                    (w.done.contains(&unit) || shared.cancel.load(Ordering::Relaxed), w.done.len(), w.total)
                };
                set_client(shared, i, |c| c.fraction = fraction);
                if cancelled {
                    if let Ok(mut w) = shared.work.lock() {
                        w.give_back(i, unit);
                    }
                    *current = None;
                    retry(send(&mut s, &Msg::Cancel, None))?;
                } else {
                    retry(send(&mut s, &Msg::Continue { done, total }, None))?;
                }
            }
            Msg::Result { unit, hash, .. } => {
                if sha256(&blob) != hash {
                    retry(send(&mut s, &Msg::Ack { unit, ok: false }, None))?;
                    continue;
                }
                store(i, shared, unit, &blob).map_err(Fatal)?;
                retry(send(&mut s, &Msg::Ack { unit, ok: true }, None))?;
                if *current == Some(unit) {
                    *current = None;
                }
            }
            Msg::Failed { unit, error } => {
                if let Ok(mut w) = shared.work.lock() {
                    w.give_back(i, unit);
                }
                *current = None;
                set_client(shared, i, |c| {
                    c.state = format!("failed: {error}");
                    c.unit = None;
                });
                return Err(Fatal(format!("failed: {error}")));
            }
            Msg::Bye => return Ok(()),
            other => return Err(Retry(format!("unexpected message {other:?}"))),
        }
    }
}

/// Stores a result (once per unit; later duplicates are dropped) and updates the counts and
/// the client's speed.
fn store(i: usize, shared: &Shared, unit: Unit, blob: &[u8]) -> Result<(), String> {
    let started = {
        let w = shared.work.lock().map_err(|_| "lock")?;
        if w.done.contains(&unit) {
            return Ok(());
        }
        w.running.get(&unit).and_then(|ws| ws.iter().find(|x| x.0 == i).map(|x| x.1))
    };
    let mut release = false;
    match unit {
        Unit::Frames { f, n } => {
            let dir = shared.frames_dir.as_ref().ok_or("no frames folder")?;
            let pngs = crate::client::split_frames(blob)?;
            if pngs.len() != n {
                return Err(format!("{} arrived with {} frames", unit.label(), pngs.len()));
            }
            for (k, png) in pngs.into_iter().enumerate() {
                let file = dir.join(frame_name(f + k - shared.from));
                let tmp = file.with_extension("part");
                std::fs::write(&tmp, png).and_then(|_| std::fs::rename(&tmp, &file)).map_err(|e| format!("{}: {e}", file.display()))?;
            }
        }
        Unit::Tile { i } => {
            shared.still.as_ref().ok_or("no still image")?.lock().map_err(|_| "image lock")?.add_tile(i, blob)?;
        }
        Unit::Maxima { .. } => {
            let mut st = shared.still.as_ref().ok_or("no still image")?.lock().map_err(|_| "image lock")?;
            st.add_maxima(blob)?;
            release = st.counting == 0;
        }
    }
    let mut w = shared.work.lock().map_err(|_| "lock")?;
    w.done.insert(unit);
    w.running.remove(&unit);
    if release {
        // Every density count is in: the tiles can render.
        let held = std::mem::take(&mut w.held);
        w.queue.extend(held);
    }
    if let Some(t0) = started {
        let per = t0.elapsed().as_secs_f64() / unit.weight();
        // Smoothed: the newest piece counts half.
        w.speed[i] = Some(w.speed[i].map_or(per, |s| 0.5 * s + 0.5 * per));
    }
    let (done, speed) = (w.done.len(), w.speed[i]);
    drop(w);
    if let Ok(mut s) = shared.status.lock() {
        s.done = done;
        if release {
            s.phase = "Rendering".into();
        }
        if let Some(c) = s.clients.get_mut(i) {
            c.units_done += 1;
            c.speed = speed;
            c.unit = None;
            c.fraction = 0.0;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn work(clients: usize, units: usize) -> Work {
        Work {
            queue: (0..units).map(|f| Unit::Frames { f, n: 1 }).collect(),
            held: VecDeque::new(),
            running: HashMap::new(),
            done: HashSet::new(),
            total: units,
            speed: vec![None; clients],
            guess: vec![1.0; clients],
        }
    }

    #[test]
    fn idle_fast_clients_take_over_the_slowest_piece_at_the_end() {
        let mut w = work(2, 1);
        w.speed = vec![Some(10.0), Some(1.0)];
        // The slow client takes the only frame; the fast one gets a duplicate of it.
        assert_eq!(w.next(0), Some(Unit::Frames { f: 0, n: 1 }));
        assert_eq!(w.next(1), Some(Unit::Frames { f: 0, n: 1 }));
        // No third copy, and nothing for a client that already has it.
        assert_eq!(w.next(1), None);
        // Equal speeds: no duplicate.
        let mut w = work(2, 1);
        w.speed = vec![Some(1.0), Some(1.0)];
        w.next(0);
        assert_eq!(w.next(1), None);
    }

    #[test]
    fn failed_pieces_go_back_into_the_queue() {
        let mut w = work(1, 2);
        let u = w.next(0).unwrap();
        w.give_back(0, u);
        assert_eq!(w.queue.front(), Some(&u));
    }

    #[test]
    fn zoom_sequences_go_out_in_runs() {
        assert_eq!(sequence_run(1800, 2), 60);
        assert_eq!(sequence_run(100, 2), 12);
        assert_eq!(sequence_run(3, 4), 1);
    }
}
