// SPDX-License-Identifier: LGPL-3.0-or-later

//! Gradient files: UFM gradients (`.ugr`, several per file, with opacity), FractInt palettes
//! (`.map`, 256 colors), GIMP gradients (`.ggr`) and palettes (`.gpl`), cpt-city / GMT color
//! tables (`.cpt`). Import reads all of them; export writes `.ugr`, `.map` and `.ggr`.

use std::path::Path;

use crate::gradient::{AlphaStop, Blend, Gradient, Stop, linear_to_srgb, srgb_to_linear};

fn srgb_stop(pos: f64, r: f64, g: f64, b: f64) -> Stop {
    let c = |v: f64| srgb_to_linear((v.clamp(0.0, 255.0)).round() as u8);
    Stop::new(pos, [c(r), c(g), c(b)])
}

fn to_srgb8(c: [f32; 3]) -> [u8; 3] {
    [linear_to_srgb(c[0]), linear_to_srgb(c[1]), linear_to_srgb(c[2])]
}

/// Drops stops that linear blending of their neighbors reproduces within one 8-bit step
/// (palettes with hundreds of entries become a few stops).
fn simplify(stops: Vec<Stop>) -> Vec<Stop> {
    if stops.len() <= 2 {
        return stops;
    }
    let srgb = |s: &Stop| to_srgb8(s.color).map(|v| v as f64);
    let mut keep = vec![stops[0].clone()];
    let mut i = 0;
    while i < stops.len() - 1 {
        // Extend the segment from stop i as far as the colors in between stay on the line.
        let mut j = i + 1;
        while j + 1 < stops.len() {
            // The renderer blends in OKLab: compare with exactly that blend.
            let pair = [Stop::new(0.0, stops[i].color), Stop::new(1.0, stops[j + 1].color)];
            let span = stops[j + 1].pos - stops[i].pos;
            let ok = (i + 1..=j).all(|k| {
                let u = if span > 0.0 { ((stops[k].pos - stops[i].pos) / span).min(0.999_999) } else { 0.0 };
                let blended = to_srgb8(crate::gradient::sample(&pair, u)).map(|v| v as f64);
                let c = srgb(&stops[k]);
                (0..3).all(|ch| (blended[ch] - c[ch]).abs() <= 1.0)
            });
            if !ok {
                break;
            }
            j += 1;
        }
        keep.push(stops[j].clone());
        i = j;
    }
    keep
}

/// Reads every gradient of a file; names come from the file (entry names, `Name:` lines) or
/// the file name.
pub fn import(path: &Path) -> Result<Vec<(String, Gradient)>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let text = String::from_utf8_lossy(&bytes);
    let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "Imported".into());
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let out = match ext.as_str() {
        "ugr" => parse_ugr(&text),
        "map" => parse_map(&text).map(|g| vec![(stem, g)]),
        "ggr" => parse_ggr(&text).map(|(n, g)| vec![(n.unwrap_or(stem), g)]),
        "gpl" => parse_gpl(&text).map(|(n, g)| vec![(n.unwrap_or(stem), g)]),
        "cpt" => parse_cpt(&text).map(|g| vec![(stem, g)]),
        other => Err(format!("unknown gradient format '.{other}' (supported: .ugr, .map, .ggr, .gpl, .cpt)")),
    }
    .map_err(|e| format!("{}: {e}", path.display()))?;
    if out.is_empty() {
        return Err(format!("{}: no gradient found", path.display()));
    }
    Ok(out)
}

/// Writes `g` as `.ugr`, `.map` or `.ggr` (by the extension of `path`).
pub fn export(path: &Path, name: &str, g: &Gradient) -> Result<(), String> {
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let text = match ext.as_str() {
        "ugr" => write_ugr(name, g),
        "map" => write_map(g),
        "ggr" => write_ggr(name, g),
        other => return Err(format!("cannot write '.{other}' gradients (supported: .ugr, .map, .ggr)")),
    };
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

/// `key=value` pairs of a UFM gradient section in order.
fn pairs(section: &str) -> Vec<(String, String)> {
    section.split_whitespace().filter_map(|t| t.split_once('=')).map(|(k, v)| (k.to_ascii_lowercase(), v.trim_matches('"').to_string())).collect()
}

fn parse_ugr(text: &str) -> Result<Vec<(String, Gradient)>, String> {
    let mut out = Vec::new();
    for e in crate::formula::parse::split_entries(text) {
        let body = e.body;
        let lower = body.to_ascii_lowercase();
        let Some(gi) = lower.find("gradient:") else { continue };
        let oi = lower.find("opacity:");
        let gsec = &body[gi + 9..oi.filter(|&o| o > gi).unwrap_or(body.len())];
        if let Some(g) = ugr_gradient(gsec, oi.map(|o| &body[o + 8..])) {
            let title = pairs(gsec).into_iter().find(|(k, _)| k == "title").map(|(_, v)| v).unwrap_or(e.name.clone());
            out.push((title, g));
        }
    }
    Ok(out)
}

/// A gradient from the text of a UFM `gradient:` section (`index=… color=…` pairs, 400
/// positions, BGR colors, `rotation`, `smooth`) and its `opacity:` section.
pub(crate) fn ugr_gradient(gsec: &str, osec: Option<&str>) -> Option<Gradient> {
    let mut stops = Vec::new();
    let mut smooth = true;
    let mut rotation = 0.0;
    let mut index: Option<f64> = None;
    let list = pairs(gsec);
    for (k, v) in &list {
        if k == "rotation" {
            rotation = v.parse().unwrap_or(0.0);
        }
    }
    for (k, v) in list {
        match k.as_str() {
            "smooth" => smooth = v.eq_ignore_ascii_case("yes"),
            "index" => index = v.parse().ok(),
            "color" => {
                if let (Some(i), Ok(c)) = (index.take(), v.parse::<u32>()) {
                    let (r, g, b) = (c & 255, (c >> 8) & 255, (c >> 16) & 255);
                    stops.push(srgb_stop(((i + rotation) / 400.0).rem_euclid(1.0), r as f64, g as f64, b as f64));
                }
            }
            _ => {}
        }
    }
    let mut alpha = Vec::new();
    if let Some(o) = osec {
        let mut index: Option<f64> = None;
        for (k, v) in pairs(o) {
            match k.as_str() {
                "index" => index = v.parse().ok(),
                "opacity" => {
                    if let (Some(i), Ok(a)) = (index.take(), v.parse::<f32>()) {
                        alpha.push(AlphaStop { pos: ((i + rotation) / 400.0).rem_euclid(1.0), alpha: (a / 255.0).clamp(0.0, 1.0) });
                    }
                }
                _ => {}
            }
        }
        alpha.sort_by(|a, b| a.pos.total_cmp(&b.pos));
        if alpha.iter().all(|a| a.alpha >= 1.0) {
            alpha.clear();
        }
    }
    if stops.is_empty() {
        return None;
    }
    stops.sort_by(|a, b| a.pos.total_cmp(&b.pos));
    Some(Gradient { stops, blend: if smooth { Blend::Smooth } else { Blend::Linear }, alpha })
}

fn write_ugr(name: &str, g: &Gradient) -> String {
    let entry: String = name.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    let mut s = format!("{entry} {{\ngradient:\n  title=\"{name}\" smooth={}", if g.blend == Blend::Smooth { "yes" } else { "no" });
    let mut stops = g.stops.clone();
    stops.sort_by(|a, b| a.pos.total_cmp(&b.pos));
    let has_zero = stops.iter().any(|st| (st.pos * 400.0).round() as i64 <= 0);
    for st in &stops {
        let [r, gg, b] = to_srgb8(st.color);
        let mut idx = (st.pos * 400.0).round() as i64;
        // Positions wrap at 400: a last stop at the end repeats the first one.
        if idx >= 400 && has_zero {
            continue;
        }
        idx = idx.clamp(0, 399);
        s.push_str(&format!(" index={idx} color={}", r as u32 + ((gg as u32) << 8) + ((b as u32) << 16)));
    }
    s.push_str("\nopacity:\n  smooth=no");
    if g.alpha.is_empty() {
        s.push_str(" index=0 opacity=255");
    }
    for a in &g.alpha {
        s.push_str(&format!(
            " index={} opacity={}",
            ((a.pos * 400.0).round() as i64).clamp(0, 399),
            (a.alpha.clamp(0.0, 1.0) * 255.0).round() as u32
        ));
    }
    s.push_str("\n}\n");
    s
}

fn parse_map(text: &str) -> Result<Gradient, String> {
    let colors: Vec<[f64; 3]> = text
        .lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace().map(|t| t.parse::<f64>());
            match (it.next(), it.next(), it.next()) {
                (Some(Ok(r)), Some(Ok(g)), Some(Ok(b))) => Some([r, g, b]),
                _ => None,
            }
        })
        .collect();
    if colors.len() < 2 {
        return Err("no colors (lines of 'red green blue')".into());
    }
    let n = colors.len() as f64;
    let stops = colors.iter().enumerate().map(|(i, c)| srgb_stop(i as f64 / n, c[0], c[1], c[2])).collect();
    Ok(Gradient { stops: simplify(stops), blend: Blend::Linear, alpha: Vec::new() })
}

fn write_map(g: &Gradient) -> String {
    (0..256)
        .map(|i| {
            let [r, gg, b] = to_srgb8(g.sample(i as f64 / 256.0));
            format!("{r} {gg} {b}\n")
        })
        .collect()
}

fn parse_ggr(text: &str) -> Result<(Option<String>, Gradient), String> {
    let mut lines = text.lines();
    if !lines.next().is_some_and(|l| l.trim().starts_with("GIMP Gradient")) {
        return Err("not a GIMP gradient (first line 'GIMP Gradient')".into());
    }
    let mut name = None;
    let mut stops = Vec::new();
    let mut alpha = Vec::new();
    for l in lines {
        if let Some(n) = l.strip_prefix("Name:") {
            name = Some(n.trim().to_string());
            continue;
        }
        let v: Vec<f64> = l.split_whitespace().filter_map(|t| t.parse().ok()).collect();
        if v.len() < 11 {
            continue;
        }
        let c = |k: usize| v[k] * 255.0;
        stops.push(srgb_stop(v[0], c(3), c(4), c(5)));
        stops.push(srgb_stop(v[2], c(7), c(8), c(9)));
        alpha.push(AlphaStop { pos: v[0], alpha: v[6] as f32 });
        alpha.push(AlphaStop { pos: v[2], alpha: v[10] as f32 });
    }
    if stops.is_empty() {
        return Err("no segments".into());
    }
    stops.dedup_by(|b, a| a.pos == b.pos && a.color == b.color);
    alpha.dedup_by(|b, a| a.pos == b.pos && a.alpha == b.alpha);
    if alpha.iter().all(|a| a.alpha >= 1.0) {
        alpha.clear();
    }
    Ok((name, Gradient { stops, blend: Blend::Linear, alpha }))
}

fn write_ggr(name: &str, g: &Gradient) -> String {
    let mut stops = g.stops.clone();
    stops.sort_by(|a, b| a.pos.total_cmp(&b.pos));
    if stops.first().is_some_and(|s| s.pos > 0.0) {
        stops.insert(0, Stop::new(0.0, g.sample(0.0)));
    }
    if stops.last().is_some_and(|s| s.pos < 1.0) {
        stops.push(Stop::new(1.0, g.sample(1.0)));
    }
    let f = |c: [f32; 3]| to_srgb8(c).map(|v| v as f64 / 255.0);
    let segs: Vec<String> = stops
        .windows(2)
        .map(|w| {
            let (a, b) = (&w[0], &w[1]);
            let (ca, cb) = (f(a.color), f(b.color));
            let (aa, ab) = (crate::gradient::sample_alpha(&g.alpha, a.pos), crate::gradient::sample_alpha(&g.alpha, b.pos));
            format!(
                "{:.6} {:.6} {:.6} {:.6} {:.6} {:.6} {:.6} {:.6} {:.6} {:.6} {:.6} 0 0",
                a.pos,
                (a.pos + b.pos) * 0.5,
                b.pos,
                ca[0],
                ca[1],
                ca[2],
                aa,
                cb[0],
                cb[1],
                cb[2],
                ab
            )
        })
        .collect();
    format!("GIMP Gradient\nName: {name}\n{}\n{}\n", segs.len(), segs.join("\n"))
}

fn parse_gpl(text: &str) -> Result<(Option<String>, Gradient), String> {
    let mut name = None;
    let mut colors = Vec::new();
    for l in text.lines() {
        let t = l.trim();
        if let Some(n) = t.strip_prefix("Name:") {
            name = Some(n.trim().to_string());
            continue;
        }
        if t.starts_with('#') || t.starts_with("GIMP") || t.starts_with("Columns") {
            continue;
        }
        let v: Vec<f64> = t.split_whitespace().take(3).filter_map(|x| x.parse().ok()).collect();
        if v.len() == 3 {
            colors.push(v);
        }
    }
    if colors.len() < 2 {
        return Err("fewer than two colors".into());
    }
    let n = colors.len() as f64;
    let stops = colors.iter().enumerate().map(|(i, c)| srgb_stop(i as f64 / n, c[0], c[1], c[2])).collect();
    Ok((name, Gradient { stops, blend: Blend::Smooth, alpha: Vec::new() }))
}

fn parse_cpt(text: &str) -> Result<Gradient, String> {
    if text
        .lines()
        .any(|l| l.trim_start().starts_with('#') && l.to_ascii_uppercase().contains("COLOR_MODEL") && l.to_ascii_uppercase().contains("HSV"))
    {
        return Err("HSV color tables are not supported yet".into());
    }
    let mut raw: Vec<(f64, [f64; 3])> = Vec::new();
    for l in text.lines() {
        let t = l.trim();
        if t.is_empty() || t.starts_with('#') || t.starts_with(['B', 'F', 'N']) {
            continue;
        }
        let v: Vec<f64> = t.replace('/', " ").split_whitespace().filter_map(|x| x.parse().ok()).collect();
        if v.len() >= 8 {
            raw.push((v[0], [v[1], v[2], v[3]]));
            raw.push((v[4], [v[5], v[6], v[7]]));
        }
    }
    if raw.len() < 2 {
        return Err("no color segments ('z0 r g b z1 r g b')".into());
    }
    let (lo, hi) = (raw.first().map(|r| r.0).unwrap_or(0.0), raw.last().map(|r| r.0).unwrap_or(1.0));
    let span = if hi > lo { hi - lo } else { 1.0 };
    let mut stops: Vec<Stop> = raw.iter().map(|(z, c)| srgb_stop((z - lo) / span, c[0], c[1], c[2])).collect();
    stops.dedup_by(|b, a| a.pos == b.pos && a.color == b.color);
    Ok(Gradient { stops: simplify(stops), blend: Blend::Linear, alpha: Vec::new() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f32; 3], b: [f32; 3], tol: i32) -> bool {
        to_srgb8(a).iter().zip(to_srgb8(b)).all(|(x, y)| (*x as i32 - y as i32).abs() <= tol)
    }

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("rfract-gradient-io-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d.join(name)
    }

    #[test]
    fn ugr_map_and_ggr_round_trip() {
        let mut g = crate::gradient::preset("Neon").unwrap();
        g.blend = Blend::Linear;
        g.alpha = vec![AlphaStop { pos: 0.0, alpha: 0.5 }, AlphaStop { pos: 0.5, alpha: 1.0 }];
        for ext in ["ugr", "map", "ggr"] {
            let p = tmp(&format!("t.{ext}"));
            export(&p, "Test", &g).unwrap();
            let back = import(&p).unwrap();
            assert_eq!(back.len(), 1, "{ext}");
            for k in 0..20 {
                let t = k as f64 / 20.0;
                // A .map holds 256 samples: steep peaks between them come back lower.
                let tol = if ext == "map" { 16 } else { 2 };
                assert!(close(back[0].1.sample(t), g.sample(t), tol), "{ext} at {t}: {:?} vs {:?}", back[0].1.sample(t), g.sample(t));
            }
            if ext != "map" {
                assert!((crate::gradient::sample_alpha(&back[0].1.alpha, 0.25) - 0.75).abs() < 0.02, "{ext} alpha");
            }
        }
    }

    #[test]
    fn reads_gimp_palettes_and_cpt_and_simplifies() {
        let gpl = "GIMP Palette\nName: Three\nColumns: 3\n#\n255 0 0 Red\n0 255 0 Green\n0 0 255 Blue\n";
        let (n, g) = parse_gpl(gpl).unwrap();
        assert_eq!(n.as_deref(), Some("Three"));
        assert_eq!(g.stops.len(), 3);
        let cpt = "# test\n0 0 0 0 50 255 255 255\n50 255 0 0 100 0 0 255\nB 0 0 0\nF 255 255 255\n";
        let g = parse_cpt(cpt).unwrap();
        // Halfway between black and white: a neutral gray (OKLab's middle, not RGB's).
        let mid = to_srgb8(g.sample(0.25));
        assert!(mid[0] == mid[1] && mid[1] == mid[2] && (80..150).contains(&mid[0]), "{mid:?}");
        // A 256-line ramp becomes a handful of stops (an sRGB ramp is not straight in OKLab).
        let ramp: String = (0..256).map(|i| format!("{i} {i} {i}\n")).collect();
        let n = parse_map(&ramp).unwrap().stops.len();
        assert!(n <= 32, "{n} stops");
    }
}
