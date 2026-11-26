//! Compiles one entry of a formula file with the class libraries of its folder:
//! `check_entry <file> <entry> [<param path>=<class> …]` (classes chosen for object parameters).
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let path = std::path::Path::new(&a[0]);
    let reg = rfr_core::formula::library::ClassRegistry::from_folders(&[path.parent().unwrap().to_path_buf()]);
    let bytes = std::fs::read(path).unwrap();
    let text: String = String::from_utf8(bytes.clone()).unwrap_or_else(|_| bytes.iter().map(|&b| b as char).collect());
    let pre = rfr_core::formula::parse::preprocess(&text);
    let e = rfr_core::formula::parse::split_entries(&pre).into_iter().find(|e| e.name == a[1]).expect("entry");
    let ast = rfr_core::formula::parse::parse_ufm_entry(&e).unwrap();
    let lookup = |n: &str| reg.get(n);
    let choices: std::collections::BTreeMap<String, rfr_core::formula::ast::ParamValue> = a[2..]
        .iter()
        .filter_map(|c| c.split_once('='))
        .map(|(k, v)| (k.to_string(), rfr_core::formula::ast::ParamValue::Class { class: v.to_string() }))
        .collect();
    let ctx = rfr_core::formula::check::ClassCtx { lookup: &lookup, choices: &choices };
    let kind = if a[0].ends_with(".ucl") {
        rfr_core::formula::ProgramKind::Coloring
    } else if a[0].ends_with(".uxf") {
        rfr_core::formula::ProgramKind::Transform
    } else {
        rfr_core::formula::ProgramKind::Fractal
    };
    match rfr_core::formula::check::check_with(&ast, kind, Some(&ctx)) {
        Ok(p) => {
            println!("ok: {} params, {} locals", p.params.len(), p.locals.len());
            for q in &p.params {
                println!("  param {} = {:?}", q.name, q.default);
            }
            if kind == rfr_core::formula::ProgramKind::Fractal {
                let k = rfr_core::formula::cpu::Compiled::<f64>::new(&p, &[]);
                let mut f = k.frame();
                for c in [(-0.5, 0.0), (0.4, 0.4), (2.0, 1.0)] {
                    let px = rfr_core::math::complex::C64::new(c.0, c.1);
                    f.pixel = px;
                    f.c = px;
                    let o = k.run(&mut f, 100);
                    println!("  {c:?}: iter {} escaped {} z {:?}", o.iter, o.escaped, o.z);
                }
            }
        }
        Err(e) => println!("error: {e}"),
    }
}
