// SPDX-License-Identifier: LGPL-3.0-or-later

//! The files a document needs besides itself: the images its image parameters and
//! environments name, and the formulas it uses (also in its animation's keyframes). Network
//! rendering sends them along.

use crate::doc::{Document, Layer};
use crate::formula::ParamValue;

/// Calls `f` with every image file name of `doc` (image parameters, environment images of lit
/// layers), keyframes included; `f` may change the name.
pub fn visit_images(doc: &mut Document, f: &mut impl FnMut(&mut String)) {
    fn params(p: &mut std::collections::BTreeMap<String, ParamValue>, f: &mut impl FnMut(&mut String)) {
        for v in p.values_mut() {
            if let ParamValue::Image { path } = v
                && !path.trim().is_empty()
            {
                f(path);
            }
        }
    }
    fn layer(l: &mut Layer, f: &mut impl FnMut(&mut String)) {
        params(&mut l.params, f);
        params(&mut l.outside.params, f);
        params(&mut l.inside.params, f);
        params(&mut l.object.params, f);
        for t in l.transforms.iter_mut().chain(l.post_transforms.iter_mut()) {
            params(&mut t.params, f);
        }
        if !l.light.environment.trim().is_empty() {
            f(&mut l.light.environment);
        }
        if let Some(m) = l.mask.as_mut() {
            layer(&mut m.layer, f);
        }
        for c in &mut l.children {
            layer(c, f);
        }
    }
    for l in &mut doc.layers {
        layer(l, f);
    }
    if let Some(a) = doc.animation.as_mut() {
        for k in &mut a.keyframes {
            visit_images(&mut k.doc, f);
        }
    }
}

/// The ids of every formula `doc` uses (fractals, colorings, transformations, 3D formulas),
/// keyframes included, sorted and without repeats.
pub fn formula_ids(doc: &Document) -> Vec<String> {
    fn layer(l: &Layer, out: &mut Vec<String>) {
        out.push(l.formula.clone());
        for c in [&l.outside, &l.inside] {
            if !c.formula.is_empty() {
                out.push(c.formula.clone());
            }
        }
        out.extend(l.transforms.iter().chain(&l.post_transforms).map(|t| t.formula.clone()));
        if l.kind == crate::doc::LayerKind::Object {
            out.push(l.object.formula.clone());
        }
        if let Some(m) = &l.mask {
            layer(&m.layer, out);
        }
        for c in &l.children {
            layer(c, out);
        }
    }
    fn walk(doc: &Document, out: &mut Vec<String>) {
        for l in &doc.layers {
            layer(l, out);
        }
        if let Some(a) = &doc.animation {
            for k in &a.keyframes {
                walk(&k.doc, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(doc, &mut out);
    out.retain(|s| !s.is_empty());
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::TransformRef;

    #[test]
    fn images_and_formulas_of_a_document() {
        let mut doc = Document::default();
        doc.layers[0].params.insert("img".into(), ParamValue::Image { path: "/a/b.png".into() });
        doc.layers[0].light.environment = "sky.jpg".into();
        doc.layers[0].transforms.push(TransformRef { formula: "rfract-transform:Mirror".into(), ..Default::default() });
        let mut key = doc.clone();
        key.layers[0].formula = "user:mine.ufm:Thing".into();
        doc.animation = Some(Box::new(crate::animation::Animation {
            keyframes: vec![crate::animation::Keyframe { doc: key, ..Default::default() }],
            ..Default::default()
        }));
        let mut seen = Vec::new();
        visit_images(&mut doc, &mut |p| {
            seen.push(p.clone());
            *p = format!("/moved/{p}");
        });
        assert_eq!(seen, ["/a/b.png", "sky.jpg", "/a/b.png", "sky.jpg"]);
        assert_eq!(doc.layers[0].light.environment, "/moved/sky.jpg");
        let ids = formula_ids(&doc);
        assert!(ids.contains(&"user:mine.ufm:Thing".to_string()) && ids.contains(&"rfract-transform:Mirror".to_string()), "{ids:?}");
    }
}
