// SPDX-License-Identifier: LGPL-3.0-or-later

use super::*;

fn run_ok(src: &str) -> RunOutput {
    run(src, "test.lua", RunInput::new(Document::default())).unwrap_or_else(|e| panic!("script failed: {e}"))
}

fn run_err(src: &str) -> String {
    match run(src, "test.lua", RunInput::new(Document::default())) {
        Ok(_) => panic!("script should fail"),
        Err(e) => e.message,
    }
}

#[test]
fn header_params() {
    let h = Header::parse(
        "--- Title\n--- First line\n--- second line.\n--@param n integer 3 1 9 Count of things\n--@param s number 0.5\n--@param on bool true\n--@param c choice a|b|c Pick one\n--@param bad float 1\nprint(1)\n--@param late number 1\n",
    );
    assert_eq!(h.title, "Title");
    assert_eq!(h.description, "First line second line.");
    assert_eq!(h.params.len(), 4);
    assert_eq!(h.params[0].kind, ParamKind::Integer { min: 1, max: 9 });
    assert_eq!(h.params[0].help, "Count of things");
    assert_eq!(h.params[1].default, ParamValue::Number(0.5));
    assert_eq!(h.params[2].default, ParamValue::Bool(true));
    assert_eq!(h.params[3].kind, ParamKind::Choice(vec!["a".into(), "b".into(), "c".into()]));
    assert_eq!(h.warnings.len(), 1, "{:?}", h.warnings);
    let v = h.values(&[("n".into(), "7".into()), ("c".into(), "B".into())]).unwrap();
    assert_eq!(v[0].1, ParamValue::Number(7.0));
    assert_eq!(v[3].1, ParamValue::Text("b".into()));
    assert!(h.values(&[("x".into(), "1".into())]).unwrap_err().contains("known: n, s, on, c"));
}

#[test]
fn edits_the_document() {
    let out = run_ok(
        r#"
        doc.layers[1].formula = "rfract:BurningShip"
        doc.layers[1].max_iter = 2000
        doc.layers[1].outside.offset = 0.25
        doc.image.width = 640
        doc.view.radius = "1e-40"
        local l = rfr.layer("rfract:Tricorn")
        l.merge = "screen"
        l.opacity = 0.5
        table.insert(doc.layers, l)
        print("layers", #doc.layers)
    "#,
    );
    assert!(out.changed);
    let d = &out.doc;
    assert_eq!(d.layers.len(), 2);
    assert_eq!((d.layers[0].formula.as_str(), d.layers[0].max_iter, d.layers[0].outside.offset), ("rfract:BurningShip", 2000, 0.25));
    assert_eq!(d.layers[1].merge, rfr_core::doc::MergeMode::Screen);
    assert_eq!(d.image.width, 640);
    assert!((d.view.radius.log10() + 40.0).abs() < 1e-9);
    assert_eq!(out.log, vec!["layers\t2"]);
    // Reading only changes nothing.
    assert!(!run_ok("local w = doc.image.width").changed);
}

#[test]
fn keyframes_and_exact_zooms() {
    let out = run_ok(
        r#"
        doc.animation = nil
        rfr.add_keyframe(doc, 30)
        for i = 1, 3 do
          rfr.zoom(doc, 1e-10, 0.5, 0)
          rfr.add_keyframe(doc, 30)
        end
        assert(#doc.animation.keyframes == 4)
        rfr.pan(doc, 1, 0)
    "#,
    );
    let a = out.doc.animation.as_ref().unwrap();
    assert_eq!(a.keyframes.len(), 4);
    assert!(a.keyframes.iter().all(|k| k.frames == 30 && k.doc.animation.is_none()));
    let r = a.keyframes[3].doc.view.radius;
    assert!((r.log10() - (Document::default().view.radius.log10() - 30.0)).abs() < 1e-9, "{r}");
    // The center moved beyond double precision toward x = 0.5 view radii.
    assert!(a.keyframes[3].doc.view.center[0].len() > 20, "{}", a.keyframes[3].doc.view.center[0]);
}

#[test]
fn renders_saves_and_queues() {
    let dir = std::env::temp_dir().join(format!("rfr-script-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut input = RunInput::new(Document::default());
    input.base_dir = Some(dir.clone());
    let src = r#"
        local s = rfr.render(doc, "out/a.png", { width = 48, height = 32, samples = 1 })
        print(s)
        rfr.save(doc, "b.rfr")
        local d = rfr.load("b.rfr")
        d.layers[1].julia = true
        rfr.queue(d, "c.png")
        local n = #rfr.formulas()
        assert(n > 50, "formulas: " .. n)
        assert(#rfr.params("rfract:Mandelbrot") > 0)
    "#;
    let out = run(src, "t.lua", input).unwrap_or_else(|e| panic!("{e}"));
    let (w, h, _) = rfr_core::io::png::read_rgba8(&dir.join("out/a.png")).unwrap();
    assert_eq!((w, h), (48, 32));
    assert!(rfr_core::io::png::read_document(&dir.join("out/a.png")).is_ok(), "the PNG holds the document");
    assert!(dir.join("b.rfr").is_file());
    assert_eq!(out.files.len(), 2);
    assert_eq!(out.queued.len(), 1);
    assert!(out.queued[0].doc.as_ref().unwrap().layers[0].julia);
    assert_eq!(out.queued[0].what, rfr_core::queue::JobWhat::Image);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn errors_name_the_line() {
    assert!(run_err("local x = 1\nerror('boom')").starts_with("test.lua:2:"), "{}", run_err("local x = 1\nerror('boom')"));
    let e = run_err("\n\nrfr.document('no:such')");
    assert!(e.contains("test.lua:3") && e.contains("no formula 'no:such'"), "{e}");
    let e = run_err("doc.layers = 5");
    assert!(e.contains("not a valid document"), "{e}");
}

#[test]
fn a_running_script_stops_when_cancelled() {
    let input = RunInput::new(Document::default());
    input.cancel.store(true, Ordering::Relaxed);
    let e = run("while true do end", "loop.lua", input).unwrap_err();
    assert!(e.cancelled, "{}", e.message);
}

#[test]
fn the_examples_parse() {
    for (name, src) in EXAMPLES {
        let h = Header::parse(src);
        assert!(!h.title.is_empty() && h.warnings.is_empty(), "{name}: {:?}", h.warnings);
    }
    // The zoom animation runs on the default document.
    let (_, src) = EXAMPLES.iter().find(|(n, _)| n.starts_with("Zoom")).unwrap();
    let h = Header::parse(src);
    let mut input = RunInput::new(Document::default());
    input.params = h.values(&[]).unwrap();
    let out = run(src, "zoom.lua", input).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(out.doc.animation.unwrap().keyframes.len(), 7);
}
