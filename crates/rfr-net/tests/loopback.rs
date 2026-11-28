// SPDX-License-Identifier: LGPL-3.0-or-later

//! A server and two clients on this machine render an animation and still images; the
//! results equal renders on one machine.

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, mpsc};

use rfr_core::doc::Document;
use rfr_net::client::{ClientEvent, ClientOptions, run_client};
use rfr_net::proto::JobKind;
use rfr_net::server::{JobRequest, Output, Status, Target, run_job};

/// Starts a client on a free port (CPU, no announcements).
fn start_client(dir: &std::path::Path, name: &str) -> Target {
    let (tx, rx) = mpsc::channel();
    let opts = ClientOptions { name: Some(name.into()), port: Some(0), cpu: true, cache: Some(dir.join(name)), silent: true, ..Default::default() };
    std::thread::spawn(move || {
        let _ = run_client(opts, |e| {
            if let ClientEvent::Started { port, specs, .. } = e {
                let _ = tx.send((port, specs));
            }
        });
    });
    let (port, specs) = rx.recv().unwrap();
    Target { name: name.into(), addr: ([127, 0, 0, 1], port).into(), specs }
}

fn request(doc: &Document, w: u32, h: u32, samples: u32, kind: JobKind, output: Output) -> JobRequest {
    JobRequest { doc: doc.clone(), doc_dir: None, width: w, height: h, samples, kind, output, key: None, gpu: None, tile: 32 }
}

fn worst(a: &[u8], b: &[u8]) -> u8 {
    a.iter().zip(b).map(|(x, y)| x.abs_diff(*y)).max().unwrap_or(0)
}

#[test]
fn clients_render_an_animation_and_stills() {
    let dir = std::env::temp_dir().join(format!("rfr-net-loop-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let clients = vec![start_client(&dir, "a"), start_client(&dir, "b")];

    // Animation: three keyframes zooming in, as PNG frames.
    let mut doc = Document::default();
    let mut anim = rfr_core::animation::Animation::default();
    for k in 0..3 {
        let mut d = Document::default();
        d.view.radius *= 0.5f64.powi(k);
        anim.keyframes.push(rfr_core::animation::Keyframe { doc: d, frames: 3, ..Default::default() });
    }
    doc.animation = Some(Box::new(anim.clone()));
    let plan = rfr_core::animation::AnimationPlan::new(&anim).unwrap();
    let last = plan.frame_count() - 1;
    let out = dir.join("frames");
    let kind = JobKind::Animation { from: 0, to: last, sequence: false };
    let output = Output::Animation { path: out.clone(), format: rfr_core::video::VideoFormat::PngSequence, ffmpeg: "ffmpeg".into() };
    let status = Arc::new(Mutex::new(Status::default()));
    let summary = run_job(request(&doc, 40, 24, 2, kind, output), clients.clone(), status.clone(), Arc::new(AtomicBool::new(false))).unwrap();
    assert!(summary.contains("frames"), "{summary}");
    let mut local = rfr_gpu::animation::FrameRenderer::new(None);
    for f in 0..=last {
        let file = out.join(format!("frame_{f:05}.png"));
        let (_, _, rgba) = rfr_core::io::png::read_rgba8(&file).unwrap_or_else(|e| panic!("frame {f}: {e}"));
        let (want, _) = local.finished(&plan, f, 40, 24, 2).unwrap();
        assert!(worst(&rgba, &want) <= 1, "frame {f} differs");
    }
    let st = status.lock().unwrap().clone();
    assert_eq!(st.done, last + 1);
    assert_eq!(st.clients.iter().map(|c| c.units_done).sum::<usize>(), last + 1);
    // The clients deleted their copies after the server stored them.
    for c in ["a", "b"] {
        let spool = dir.join(c).join("spool");
        let left =
            std::fs::read_dir(&spool).map(|d| d.flatten().flat_map(|e| std::fs::read_dir(e.path()).into_iter().flatten()).count()).unwrap_or(0);
        assert_eq!(left, 0, "client {c} kept results");
    }

    // Stills in 32-pixel tiles: a plain one and one with a density layer (counted first) and
    // a glow; both equal one tiled render on this machine.
    let mut density = Document::for_formula("rfract:Mandelbrot");
    density.layers[0].kind = rfr_core::doc::LayerKind::Density;
    density.post.bloom = 0.5;
    for (name, d) in [("plain", Document::default()), ("density", density)] {
        let path = dir.join(format!("{name}.png"));
        let status = Arc::new(Mutex::new(Status::default()));
        let summary = run_job(
            request(&d, 90, 50, 2, JobKind::Still, Output::Still { path: path.clone() }),
            clients.clone(),
            status.clone(),
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
        assert!(summary.contains("in 6 tiles"), "{summary}");
        let (w, h, got) = rfr_core::io::png::read_rgba8(&path).unwrap();
        assert_eq!((w, h), (90, 50));
        let want_path = dir.join(format!("{name}-local.png"));
        rfr_gpu::poster::render_poster_sized(&d, (90, 50, 2), 32, None, &want_path, &AtomicBool::new(false), |_| {}).unwrap();
        let (_, _, want) = rfr_core::io::png::read_rgba8(&want_path).unwrap();
        assert!(worst(&got, &want) <= 1, "{name}: the network render differs");
        let units = if name == "density" { 12 } else { 6 };
        assert_eq!(status.lock().unwrap().total, units, "{name}");
    }

    // A formula from a formula folder travels with the job: the clients render it from their
    // copy (the same image as here).
    let formulas = dir.join("formulas");
    std::fs::create_dir_all(&formulas).unwrap();
    let src = "Cubic {\ninit:\n  z = 0\nloop:\n  z = z * z * z + #pixel\nbailout:\n  |z| < 1e8\n}\n";
    std::fs::write(formulas.join("net.ufm"), src).unwrap();
    rfr_core::formula::library::set_user_folders(std::slice::from_ref(&formulas));
    let mut cubic = Document::default();
    cubic.layers[0].formula = "user:net.ufm:Cubic".into();
    let want_path = dir.join("cubic-local.png");
    rfr_gpu::poster::render_poster_sized(&cubic, (64, 40, 1), 32, None, &want_path, &AtomicBool::new(false), |_| {}).unwrap();
    let (_, _, want) = rfr_core::io::png::read_rgba8(&want_path).unwrap();
    let path = dir.join("cubic.png");
    run_job(
        request(&cubic, 64, 40, 1, JobKind::Still, Output::Still { path: path.clone() }),
        clients.clone(),
        Arc::new(Mutex::new(Status::default())),
        Arc::new(AtomicBool::new(false)),
    )
    .unwrap();
    let (_, _, got) = rfr_core::io::png::read_rgba8(&path).unwrap();
    assert!(worst(&got, &want) <= 1, "the formula from the formula folder renders differently");
    let hash = rfr_net::proto::sha256(src.as_bytes());
    assert!(["a", "b"].iter().any(|c| dir.join(c).join("files").join(&hash).is_file()), "the formula file reached a client");
    rfr_core::formula::library::set_user_folders(&[]);
    std::fs::remove_dir_all(&dir).ok();
}
