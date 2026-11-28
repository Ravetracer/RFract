// SPDX-License-Identifier: LGPL-3.0-or-later

//! The render client: announces itself, waits for a server to connect, receives the job and
//! its files, renders the pieces the server hands out and sends them back. A finished piece
//! stays in the client's spool folder until the server confirmed it; after a broken
//! connection the server gets the spooled pieces first when it comes back.

use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rfr_core::animation::{AnimationPlan, ZoomPath};
use rfr_core::doc::Document;
use rfr_core::render::Plan;
use rfr_gpu::GpuContext;
use rfr_gpu::animation::{FrameRenderer, ZoomSequence};

use crate::assets::Cache;
use crate::discovery::Announce;
use crate::proto::{JobKind, JobSpec, Msg, Specs, Unit, recv, send, sha256};

#[derive(Clone, Debug, Default)]
pub struct ClientOptions {
    /// Name shown on the server (default: the host name).
    pub name: Option<String>,
    /// TCP port (default `CLIENT_PORT`; another free one if it is taken).
    pub port: Option<u16>,
    /// Shared key the server must know.
    pub key: Option<String>,
    /// Render on the CPU even with a GPU.
    pub cpu: bool,
    /// Cache folder (default: the user's cache folder).
    pub cache: Option<PathBuf>,
    /// Do not announce (tests; the server connects by address).
    pub silent: bool,
    /// Stop after one job (tests).
    pub once: bool,
    /// Runs in the server's process: the files and formulas are already there.
    pub local: bool,
}

/// What the client is doing (for its display).
#[derive(Clone, Debug)]
pub enum ClientEvent {
    Started { name: String, port: u16, specs: Specs },
    Waiting,
    Connected { server: String },
    Files { done: usize, total: usize },
    Rendering { unit: Unit, fraction: f32, done: usize, total: usize },
    Sent { unit: Unit },
    JobFinished,
    Error(String),
}

/// Runs the client until it fails to start (or after one job with `once`).
pub fn run_client(opts: ClientOptions, mut on_event: impl FnMut(ClientEvent)) -> Result<(), String> {
    let ctx = if opts.cpu { None } else { GpuContext::new_headless().ok() };
    let specs = Specs::of_this_machine(ctx.as_ref().map(|c| c.adapter_name.as_str()).unwrap_or(""));
    let name = opts.name.clone().unwrap_or_else(crate::host_name);
    let listener = TcpListener::bind((Ipv4Addr::UNSPECIFIED, opts.port.unwrap_or(crate::CLIENT_PORT)))
        .or_else(|_| TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0)))
        .map_err(|e| format!("cannot open a TCP port: {e}"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let id = crate::proto::random_u64();
    let info = Arc::new(Mutex::new(Announce::new(id, name.clone(), port, opts.key.is_some(), specs.clone())));
    let stop = Arc::new(AtomicBool::new(false));
    if !opts.silent {
        crate::discovery::announce(info.clone(), stop.clone()).map_err(|e| format!("cannot announce on the network: {e}"))?;
    }
    on_event(ClientEvent::Started { name: name.clone(), port, specs: specs.clone() });
    let cache = Cache::new(opts.cache.clone());
    let mut session = Session { ctx, cache, opts: opts.clone(), name, id, specs, job: None };
    let result = loop {
        on_event(ClientEvent::Waiting);
        let (stream, from) = match listener.accept() {
            Ok(s) => s,
            Err(e) => break Err(format!("accept failed: {e}")),
        };
        set_busy(&info, true);
        on_event(ClientEvent::Connected { server: from.ip().to_string() });
        let r = session.serve(stream, &mut on_event);
        set_busy(&info, false);
        match r {
            Ok(finished) => {
                if finished {
                    on_event(ClientEvent::JobFinished);
                    if opts.once {
                        break Ok(());
                    }
                }
            }
            Err(e) => on_event(ClientEvent::Error(e)),
        }
    };
    stop.store(true, Ordering::Relaxed);
    result
}

/// Starts a client in this process (the server's own machine renders too) and returns it as a
/// target. It runs until the program ends.
pub fn start_local(key: Option<String>, cpu: bool) -> Result<crate::server::Target, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let name = format!("{} (this machine)", crate::host_name());
    let opts = ClientOptions { name: Some(name.clone()), port: Some(0), key, silent: true, local: true, cpu, ..Default::default() };
    std::thread::spawn(move || {
        let _ = run_client(opts, |e| {
            if let ClientEvent::Started { port, specs, .. } = e {
                let _ = tx.send((port, specs));
            }
        });
    });
    let (port, specs) = rx.recv_timeout(Duration::from_secs(30)).map_err(|_| "the local client did not start".to_string())?;
    Ok(crate::server::Target { name, addr: ([127, 0, 0, 1], port).into(), specs })
}

fn set_busy(info: &Mutex<Announce>, busy: bool) {
    if let Ok(mut i) = info.lock() {
        i.busy = busy;
    }
}

/// The current job and its renderer (kept over reconnections of the same job).
struct Job {
    spec: JobSpec,
    frames: Option<FramesJob>,
    tiles: Option<TilesJob>,
}

struct FramesJob {
    plan: AnimationPlan,
    renderer: FrameRenderer,
    /// The zoom sequence mode.
    seq: Option<ZoomSequence>,
}

/// A still image in tiles: the whole image's plan and the tiles.
struct TilesJob {
    doc: Document,
    whole: Plan,
    tiles: Vec<[u32; 4]>,
    renderer: FrameRenderer,
}

struct Session {
    ctx: Option<GpuContext>,
    cache: Cache,
    opts: ClientOptions,
    name: String,
    id: u64,
    specs: Specs,
    job: Option<Job>,
}

type Io<T> = Result<T, String>;

fn io<T>(r: std::io::Result<T>) -> Io<T> {
    r.map_err(|e| format!("connection: {e}"))
}

impl Session {
    /// One connection of a server; true when the job finished.
    fn serve(&mut self, mut s: TcpStream, ev: &mut impl FnMut(ClientEvent)) -> Io<bool> {
        let _ = s.set_nodelay(true);
        io(s.set_read_timeout(Some(Duration::from_secs(120))))?;
        let nonce = format!("{:016x}", crate::proto::random_u64());
        let welcome = Msg::Welcome {
            version: crate::version(),
            name: self.name.clone(),
            id: self.id,
            nonce: nonce.clone(),
            key: self.opts.key.is_some(),
            busy: false,
            specs: self.specs.clone(),
        };
        io(send(&mut s, &welcome, None))?;
        let (m, _) = io(recv(&mut s))?;
        let Msg::Hello { version, auth, .. } = m else { return Err("unexpected message".into()) };
        if version != crate::version() {
            let _ = send(&mut s, &Msg::Rejected { reason: format!("version {} (the client has {})", version, crate::version()) }, None);
            return Err(format!("server has version {version}, this client {}", crate::version()));
        }
        if let Some(k) = &self.opts.key
            && auth.as_deref() != Some(crate::proto::auth(&nonce, k).as_str())
        {
            let _ = send(&mut s, &Msg::Rejected { reason: "wrong key".into() }, None);
            return Err("a server with a wrong key tried to connect".into());
        }
        io(send(&mut s, &Msg::Accepted, None))?;
        let (m, _) = io(recv(&mut s))?;
        let Msg::Job(spec) = m else { return Err("expected a job".into()) };
        self.receive_files(&mut s, &spec, ev)?;
        // A new job: set it up (the same job after a reconnection keeps its renderer).
        if self.job.as_ref().is_none_or(|j| j.spec.job_id != spec.job_id) {
            self.job = Some(self.setup(*spec)?);
        }
        let job_id = self.job.as_ref().map(|j| j.spec.job_id).unwrap_or(0);
        self.send_spooled(&mut s, job_id)?;
        loop {
            io(send(&mut s, &Msg::Ready, None))?;
            let (m, _) = io(recv(&mut s))?;
            match m {
                Msg::Work { unit, done, total, density_max } => {
                    let mut counts = (done, total);
                    ev(ClientEvent::Rendering { unit, fraction: 0.0, done, total });
                    match self.render(&mut s, unit, &density_max, &mut counts, ev) {
                        Ok(Some(bytes)) => self.deliver(&mut s, job_id, unit, bytes, ev)?,
                        // Cancelled: another client was faster.
                        Ok(None) => {}
                        Err(e) => {
                            io(send(&mut s, &Msg::Failed { unit, error: e.clone() }, None))?;
                            ev(ClientEvent::Error(e));
                        }
                    }
                }
                Msg::Wait => std::thread::sleep(Duration::from_millis(500)),
                Msg::Done => {
                    let _ = send(&mut s, &Msg::Bye, None);
                    self.finish_job(job_id);
                    return Ok(true);
                }
                other => return Err(format!("unexpected message {other:?}")),
            }
        }
    }

    fn receive_files(&mut self, s: &mut TcpStream, spec: &JobSpec, ev: &mut impl FnMut(ClientEvent)) -> Io<()> {
        // The server's own machine has every file where the document says.
        let mut need: Vec<String> =
            if self.opts.local { Vec::new() } else { spec.files.iter().filter(|f| !self.cache.has(&f.hash)).map(|f| f.hash.clone()).collect() };
        need.sort();
        need.dedup();
        let total = need.len();
        io(send(s, &Msg::Need { hashes: need }, None))?;
        let mut done = 0;
        loop {
            let (m, blob) = io(recv(s))?;
            match m {
                Msg::File { hash, .. } => {
                    self.cache.store(&hash, &blob)?;
                    done += 1;
                    ev(ClientEvent::Files { done, total });
                }
                Msg::FilesDone => break,
                other => return Err(format!("unexpected message {other:?}")),
            }
        }
        if !self.opts.local {
            self.cache.materialize(spec.job_id, &spec.files)?;
        }
        Ok(())
    }

    fn setup(&mut self, spec: JobSpec) -> Io<Job> {
        let mut doc = Document::from_json(&spec.doc)?;
        if !self.opts.local {
            self.cache.rewrite(spec.job_id, &mut doc);
            // The job's formulas are this client's formula folder while it renders.
            let folders = if spec.files.iter().any(|f| f.formula.is_some()) { vec![self.cache.formula_dir(spec.job_id)] } else { Vec::new() };
            rfr_core::formula::library::set_user_folders(&folders);
        }
        let mut job = Job { spec: spec.clone(), frames: None, tiles: None };
        let (w, h, s) = (spec.width, spec.height, spec.samples);
        match spec.kind {
            JobKind::Animation { sequence, .. } => {
                let anim = doc.animation.as_deref().cloned().ok_or("the document has no animation")?;
                let plan = AnimationPlan::new(&anim)?;
                let seq = if sequence { Some(ZoomSequence::new(self.ctx.clone(), ZoomPath::new(&anim)?, w, h, s)) } else { None };
                let renderer = FrameRenderer::new(if seq.is_some() { None } else { self.ctx.clone() });
                job.frames = Some(FramesJob { plan, renderer, seq });
            }
            JobKind::Still => {
                let whole = Plan::sized(&doc, w, h)?;
                let tiles = rfr_core::render::tiles::tiles_sized(w, h, spec.tile);
                job.tiles = Some(TilesJob { doc, whole, tiles, renderer: FrameRenderer::new(self.ctx.clone()) });
            }
        }
        Ok(job)
    }

    /// Renders a piece; None when the server cancelled it.
    fn render(
        &mut self,
        s: &mut TcpStream,
        unit: Unit,
        density_max: &[Vec<[u32; 3]>],
        counts: &mut (usize, usize),
        ev: &mut impl FnMut(ClientEvent),
    ) -> Io<Option<Vec<u8>>> {
        let job = self.job.as_mut().ok_or("no job")?;
        let (w, h, samples) = (job.spec.width, job.spec.height, job.spec.samples.max(1));
        match unit {
            Unit::Frames { f, n } => {
                let fj = job.frames.as_mut().ok_or("not an animation job")?;
                let mut out = Vec::new();
                for (k, frame) in (f..f + n).enumerate() {
                    let (rgba, _) = match fj.seq.as_mut() {
                        Some(z) => z.finished(frame)?,
                        None => fj.renderer.finished(&fj.plan, frame, w, h, samples)?,
                    };
                    let png = png_bytes(w, h, &rgba)?;
                    out.extend((png.len() as u64).to_le_bytes());
                    out.extend(png);
                    // Between the frames of a run: report, and stop when the server says so.
                    if k + 1 < n && !progress(s, unit, (k + 1) as f32 / n as f32, counts, ev)? {
                        return Ok(None);
                    }
                }
                Ok(Some(out))
            }
            Unit::Tile { i } => {
                let tj = job.tiles.as_mut().ok_or("not a still image job")?;
                let tile = *tj.tiles.get(i as usize).ok_or("no such tile")?;
                let region = rfr_gpu::poster::tile_region(tile, w, h, &tj.doc.post);
                let plan = tj.whole.clone().with_density_max(density_max).with_window(region);
                let lin = tj.renderer.render_plan(&plan, samples)?;
                Ok(Some(encode_region(region, &lin)))
            }
            Unit::Maxima { i } => {
                let tj = job.tiles.as_mut().ok_or("not a still image job")?;
                let tile = *tj.tiles.get(i as usize).ok_or("no such tile")?;
                let m = tj.renderer.density_maxima(&tj.whole.clone().with_window(tile), samples)?;
                serde_json::to_vec(&m).map(Some).map_err(|e| e.to_string())
            }
        }
    }

    /// Spools a result, sends it and deletes it once the server confirmed it.
    fn deliver(&mut self, s: &mut TcpStream, job: u64, unit: Unit, bytes: Vec<u8>, ev: &mut impl FnMut(ClientEvent)) -> Io<()> {
        let dir = self.cache.spool_dir(job);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let file = dir.join(spool_name(unit));
        std::fs::write(&file, &bytes).map_err(|e| format!("{}: {e}", file.display()))?;
        send_result(s, unit, &bytes, &file)?;
        ev(ClientEvent::Sent { unit });
        Ok(())
    }

    /// Results of the job that were finished but not confirmed (broken connection).
    fn send_spooled(&mut self, s: &mut TcpStream, job: u64) -> Io<()> {
        let dir = self.cache.spool_dir(job);
        let Ok(rd) = std::fs::read_dir(&dir) else { return Ok(()) };
        for e in rd.flatten() {
            let p = e.path();
            let Some(unit) = p.file_name().and_then(|n| n.to_str()).and_then(parse_spool_name) else { continue };
            let Ok(bytes) = std::fs::read(&p) else { continue };
            send_result(s, unit, &bytes, &p)?;
        }
        Ok(())
    }

    fn finish_job(&mut self, job: u64) {
        self.job = None;
        let _ = std::fs::remove_dir_all(self.cache.job_dir(job));
        let _ = std::fs::remove_dir(self.cache.spool_dir(job));
        if !self.opts.local {
            rfr_core::formula::library::set_user_folders(&[]);
        }
    }
}

/// Reports progress to the server; false when it cancels the piece.
fn progress(s: &mut TcpStream, unit: Unit, fraction: f32, counts: &mut (usize, usize), ev: &mut impl FnMut(ClientEvent)) -> Io<bool> {
    ev(ClientEvent::Rendering { unit, fraction, done: counts.0, total: counts.1 });
    io(send(s, &Msg::Progress { unit, fraction }, None))?;
    match io(recv(s))?.0 {
        Msg::Continue { done, total } => {
            *counts = (done, total);
            Ok(true)
        }
        Msg::Cancel => Ok(false),
        m => Err(format!("unexpected message {m:?}")),
    }
}

fn send_result(s: &mut TcpStream, unit: Unit, bytes: &[u8], file: &std::path::Path) -> Io<()> {
    let hash = sha256(bytes);
    for _ in 0..3 {
        io(send(s, &Msg::Result { unit, hash: hash.clone(), bytes: bytes.len() as u64 }, Some(bytes)))?;
        let (m, _) = io(recv(s))?;
        match m {
            Msg::Ack { ok: true, .. } => {
                // Stored on the server: only now the local copy goes.
                let _ = std::fs::remove_file(file);
                return Ok(());
            }
            Msg::Ack { ok: false, .. } => continue,
            other => return Err(format!("unexpected message {other:?}")),
        }
    }
    Err(format!("{} was not accepted; kept in {}", unit.label(), file.display()))
}

fn spool_name(u: Unit) -> String {
    match u {
        Unit::Frames { f, n } => format!("frames_{f}_{n}.bin"),
        Unit::Tile { i } => format!("tile_{i}.bin"),
        Unit::Maxima { i } => format!("maxima_{i}.json"),
    }
}

fn parse_spool_name(n: &str) -> Option<Unit> {
    if let Some(i) = n.strip_prefix("tile_").and_then(|r| r.strip_suffix(".bin")) {
        return i.parse().ok().map(|i| Unit::Tile { i });
    }
    if let Some(i) = n.strip_prefix("maxima_").and_then(|r| r.strip_suffix(".json")) {
        return i.parse().ok().map(|i| Unit::Maxima { i });
    }
    let r = n.strip_prefix("frames_")?.strip_suffix(".bin")?;
    let (a, b) = r.split_once('_')?;
    Some(Unit::Frames { f: a.parse().ok()?, n: b.parse().ok()? })
}

/// A rendered tile region as bytes: left, top, width, height, then linear RGBA floats.
pub fn encode_region(region: [u32; 4], lin: &[[f32; 4]]) -> Vec<u8> {
    let mut out = Vec::with_capacity(16 + lin.len() * 16);
    for v in region {
        out.extend(v.to_le_bytes());
    }
    for c in lin {
        for v in c {
            out.extend(v.to_le_bytes());
        }
    }
    out
}

/// The inverse of [`encode_region`].
pub fn decode_region(bytes: &[u8]) -> Result<([u32; 4], Vec<[f32; 4]>), String> {
    if bytes.len() < 16 {
        return Err("an empty tile arrived".into());
    }
    let word = |i: usize| [bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]];
    let region = [0, 1, 2, 3].map(|k| u32::from_le_bytes(word(k * 4)));
    let n = region[2] as usize * region[3] as usize;
    if bytes.len() != 16 + n * 16 {
        return Err("a tile of the wrong size arrived".into());
    }
    let lin = (0..n).map(|i| [0, 1, 2, 3].map(|k| f32::from_le_bytes(word(16 + (i * 4 + k) * 4)))).collect();
    Ok((region, lin))
}

/// The frames of a run as PNG files (each with its length in front).
pub fn split_frames(bytes: &[u8]) -> Result<Vec<&[u8]>, String> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let len = bytes.get(at..at + 8).map(|b| u64::from_le_bytes(b.try_into().unwrap_or_default()) as usize).ok_or("frames arrived damaged")?;
        let png = bytes.get(at + 8..at + 8 + len).ok_or("frames arrived damaged")?;
        out.push(png);
        at += 8 + len;
    }
    Ok(out)
}

/// A finished frame as PNG bytes.
fn png_bytes(w: u32, h: u32, rgba: &[u8]) -> Io<Vec<u8>> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut wr = enc.write_header().map_err(|e| e.to_string())?;
        wr.write_image_data(rgba).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spool_names_round_trip() {
        for u in [Unit::Frames { f: 12, n: 3 }, Unit::Tile { i: 3 }, Unit::Maxima { i: 5 }] {
            assert_eq!(parse_spool_name(&spool_name(u)), Some(u));
        }
    }

    #[test]
    fn regions_and_frame_runs_round_trip() {
        let lin = vec![[0.5, 0.25, 1.0, 1.0], [0.0, 0.0, 0.0, 0.0]];
        let (r, back) = decode_region(&encode_region([3, 4, 2, 1], &lin)).unwrap();
        assert_eq!((r, back), ([3, 4, 2, 1], lin));
        let mut run = Vec::new();
        for f in [&b"abc"[..], b"de"] {
            run.extend((f.len() as u64).to_le_bytes());
            run.extend(f);
        }
        assert_eq!(split_frames(&run).unwrap(), vec![&b"abc"[..], b"de"]);
    }
}
