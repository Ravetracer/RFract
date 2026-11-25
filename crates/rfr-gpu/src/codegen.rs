// SPDX-License-Identifier: LGPL-3.0-or-later

//! Formula IR → WGSL. Every IR node maps to one expression over the scalar layer (`s_*`) and
//! the complex library (`c_*`, `r_*`) of `cstd.wgsl`, mirroring rfr_core::formula::cpu. Parameter
//! values come from the `P` uniform (editing them never recompiles); only the chosen functions
//! of function parameters and the scalar mode are part of the shader.

use std::fmt::Write;
use std::hash::{Hash, Hasher};

use rfr_core::formula::ParamValue;
use rfr_core::formula::ir::*;
use rfr_core::render::{LayerPlan, Prog};

/// Scalar mode of a shader.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mode {
    /// Plain f32.
    Plain,
    /// Dual numbers: derivatives for distance estimation.
    Dual,
    /// Double-float (two f32 per value, about 48 bits): deep views.
    Df64,
}

const SCALAR_F32: &str = include_str!("wgsl/scalar_f32.wgsl");
const SCALAR_DUAL: &str = include_str!("wgsl/scalar_dual.wgsl");
const SCALAR_DF64: &str = include_str!("wgsl/scalar_df64.wgsl");
const CSTD: &str = include_str!("wgsl/cstd.wgsl");
const KERNEL: &str = include_str!("wgsl/kernel.wgsl");
const BLEND: &str = include_str!("wgsl/blend.wgsl");

/// Parameters the `P` uniform can hold (all programs of a layer together).
pub const MAX_PARAMS: usize = 4096;

struct Gen<'a> {
    prog: &'a Program,
    values: &'a [ParamValue],
    funcs: Vec<Option<Func>>,
    /// Name prefix of this program's locals (`l` fractal, `t0_` transformations, `ko` / `ki`
    /// colorings).
    prefix: String,
    /// Index of this program's first parameter in `P`.
    base: usize,
}

/// A WGSL float literal that fits f32 (WGSL rejects literals outside its range).
fn lit(v: f64) -> String {
    let v = if v.is_nan() { 0.0 } else { v.clamp(-3.4e38, 3.4e38) };
    let v = if v != 0.0 && v.abs() < 1e-37 { 0.0 } else { v };
    let s = format!("{v:e}");
    // "1e0" → "1e0" is valid WGSL; make sure there is a mantissa digit before the exponent.
    if s.contains('e') || s.contains('.') { s } else { format!("{s}.0") }
}

/// Name of a unary library function in cstd.wgsl.
fn fname(f: Func) -> &'static str {
    f.name()
}

impl Gen<'_> {
    fn expr(&self, e: &Expr) -> String {
        match &e.kind {
            Kind::Bool(b) => b.to_string(),
            Kind::Int(i) => format!("i32({})", (*i).clamp(i32::MIN as i64, i32::MAX as i64)),
            Kind::Real(v) => {
                // High and low part, so double-float shaders keep the full value.
                let hi = *v as f32;
                let lo = if hi.is_finite() { (*v - hi as f64) as f32 } else { 0.0 };
                format!("s_cc({}, {})", lit(hi as f64), lit(lo as f64))
            }
            Kind::Complex(a, b) => format!("Cx({}, {})", self.expr(a), self.expr(b)),
            Kind::Local(i) => format!("{}{i}", self.prefix),
            Kind::Index(i, idx) => {
                let cap = self.prog.locals[*i].cap.unwrap_or(1);
                format!("{}{i}[clamp({}, 0, {})]", self.prefix, self.expr(idx), cap - 1)
            }
            Kind::Param(i) => {
                let k = self.base + i;
                match self.prog.params[*i].ty {
                    Ty::Bool => format!("(P.p[{k}].x != 0.0)"),
                    Ty::Int => format!("i32(P.p[{k}].x)"),
                    Ty::Real => format!("s_c(P.p[{k}].x)"),
                    Ty::Complex => format!("c_f(P.p[{k}].x, P.p[{k}].y)"),
                    Ty::Color => format!("P.p[{k}]"),
                }
            }
            Kind::Sys(s) => match s {
                Sys::Pixel => "pixel".into(),
                Sys::C => "cval".into(),
                Sys::ZPrev => "zprev".into(),
                Sys::LastSqr => "lastsqr".into(),
                Sys::MaxIt => "i32(maxit)".into(),
                Sys::NumIter => "numiter".into(),
                Sys::IsMand => "ismand".into(),
                Sys::Center => "center".into(),
                Sys::Magn => "magn".into(),
                Sys::Z => "l0".into(),
                Sys::X => "i32(x)".into(),
                Sys::Y => "i32(y)".into(),
                Sys::Width => "i32(U.size.x)".into(),
                Sys::Height => "i32(U.size.y)".into(),
                Sys::ScreenPixel => "c_f(f32(x), f32(y))".into(),
                Sys::ScreenMax => "c_f(f32(U.size.x), f32(U.size.y))".into(),
                Sys::Angle => "s_c(atan2(U.rot.y, U.rot.x))".into(),
                // WGSL evaluates arguments left to right, like the CPU's two draws.
                Sys::Random => format!("c_f(rnd_step(&rng_{p}), rnd_step(&rng_{p}))", p = self.prefix),
            },
            Kind::Conv(inner) => self.conv(inner, e.ty),
            Kind::Neg(a) => match e.ty {
                Ty::Int => format!("(-{})", self.expr(a)),
                Ty::Real => format!("s_neg({})", self.expr(a)),
                _ => format!("c_neg({})", self.expr(a)),
            },
            Kind::Not(a) => format!("(!{})", self.expr(a)),
            Kind::Arith(op, a, b) => {
                let (a, b) = (self.expr(a), self.expr(b));
                match (e.ty, op) {
                    (Ty::Int, Arith::Add) => format!("({a} + {b})"),
                    (Ty::Int, Arith::Sub) => format!("({a} - {b})"),
                    (Ty::Int, Arith::Mul) => format!("({a} * {b})"),
                    (Ty::Int, Arith::Div) => format!("i_div({a}, {b})"),
                    (Ty::Int, Arith::Mod) => format!("i_mod({a}, {b})"),
                    (Ty::Real, Arith::Add) => format!("s_add({a}, {b})"),
                    (Ty::Real, Arith::Sub) => format!("s_sub({a}, {b})"),
                    (Ty::Real, Arith::Mul) => format!("s_mul({a}, {b})"),
                    (Ty::Real, Arith::Div) => format!("s_div({a}, {b})"),
                    (Ty::Real, Arith::Mod) => format!("s_fmod({a}, {b})"),
                    (_, Arith::Add) => format!("c_add({a}, {b})"),
                    (_, Arith::Sub) => format!("c_sub({a}, {b})"),
                    (_, Arith::Mul) => format!("c_mul({a}, {b})"),
                    (_, Arith::Div | Arith::Mod) => format!("c_div({a}, {b})"),
                }
            }
            Kind::Pow(a, b) => match e.ty {
                Ty::Real => format!("s_rpow({}, {})", self.expr(a), self.expr(b)),
                _ => format!("c_pow({}, {})", self.expr(a), self.expr(b)),
            },
            Kind::PowI(a, n) => match (e.ty, n) {
                (Ty::Real, _) => format!("s_powi({}, {n})", self.expr(a)),
                (_, 2) => format!("c_sqr({})", self.expr(a)),
                _ => format!("c_powi({}, {n})", self.expr(a)),
            },
            Kind::PowN(a, n) => match e.ty {
                Ty::Real => format!("s_powi({}, clamp({}, -64, 64))", self.expr(a), self.expr(n)),
                _ => format!("c_powi({}, clamp({}, -64, 64))", self.expr(a), self.expr(n)),
            },
            Kind::Cmp(op, a, b) => {
                let o = match op {
                    Cmp::Lt => "<",
                    Cmp::Gt => ">",
                    Cmp::Le => "<=",
                    Cmp::Ge => ">=",
                    Cmp::Eq => "==",
                    Cmp::Ne => "!=",
                };
                match a.ty {
                    Ty::Int => format!("({} {o} {})", self.expr(a), self.expr(b)),
                    Ty::Real => format!("(s_v({}) {o} s_v({}))", self.expr(a), self.expr(b)),
                    _ if *op == Cmp::Ne => format!("(!c_eq({}, {}))", self.expr(a), self.expr(b)),
                    _ => format!("c_eq({}, {})", self.expr(a), self.expr(b)),
                }
            }
            Kind::And(a, b) => format!("({} && {})", self.expr(a), self.expr(b)),
            Kind::Or(a, b) => format!("({} || {})", self.expr(a), self.expr(b)),
            Kind::Call(f, args) => self.call(*f, args, e.ty),
            Kind::ColorOp(op, a) => self.color_op(*op, a),
            Kind::RandomInt(a) => format!("bitcast<i32>(hash(bitcast<u32>({})))", self.expr(a)),
            Kind::MergeParam(i, a, b) => {
                let mode = merge_choices(self.prog, self.values).get(*i).copied().flatten().unwrap_or_default();
                format!("k_merge({}u, {}, {})", mode.index(), self.expr(a), self.expr(b))
            }
            Kind::CallParam(i, a) => {
                let f = self.funcs.get(*i).copied().flatten().unwrap_or(Func::Ident);
                self.complex_call(f, &self.expr(a))
            }
        }
    }

    fn color_op(&self, op: ColorOp, a: &[Expr]) -> String {
        let r = |i: usize| format!("s_v({})", self.expr(&a[i]));
        let k = |i: usize| self.expr(&a[i]);
        match op {
            ColorOp::Rgba => format!("vec4<f32>({}, {}, {}, {})", r(0), r(1), r(2), r(3)),
            ColorOp::Hsla => format!("k_hsla({}, {}, {}, {})", r(0), r(1), r(2), r(3)),
            ColorOp::Red => format!("s_c(({}).x)", k(0)),
            ColorOp::Green => format!("s_c(({}).y)", k(0)),
            ColorOp::Blue => format!("s_c(({}).z)", k(0)),
            ColorOp::Alpha => format!("s_c(({}).w)", k(0)),
            ColorOp::Hue => format!("s_c(k_hsl({}).x)", k(0)),
            ColorOp::Sat => format!("s_c(k_hsl({}).y)", k(0)),
            ColorOp::Lum => format!("s_c(k_hsl({}).z)", k(0)),
            ColorOp::Blend => format!("mix({}, {}, {})", k(0), k(1), r(2)),
            ColorOp::Compose => format!("k_compose({}, {}, {})", k(0), k(1), r(2)),
            ColorOp::Merge(m) => format!("k_merge({}u, {}, {})", m.index(), k(0), k(1)),
            ColorOp::Gradient => {
                let base = match self.prefix.as_str() {
                    "ko" => "0u",
                    "ki" => "LUT_SIZE",
                    _ => "0xffffffffu",
                };
                format!("k_gradient({base}, {})", r(0))
            }
        }
    }

    /// A unary function on a complex value, complex result (real-valued functions promoted).
    fn complex_call(&self, f: Func, a: &str) -> String {
        match f {
            Func::Real => format!("c_real(({a}).re)"),
            Func::Imag => format!("c_real(({a}).im)"),
            Func::Cabs => format!("c_real(c_cabs({a}))"),
            Func::Norm => format!("c_real(c_norm({a}))"),
            Func::Arg => format!("c_real(c_arg({a}))"),
            Func::Atan2 | Func::Min | Func::Max => format!("c_real(({a}).re)"),
            f => format!("c_{}({a})", fname(f)),
        }
    }

    fn call(&self, f: Func, args: &[Expr], ty: Ty) -> String {
        match f {
            Func::Atan2 => format!("s_atan2({}, {})", self.expr(&args[0]), self.expr(&args[1])),
            Func::Min => format!("s_min({}, {})", self.expr(&args[0]), self.expr(&args[1])),
            Func::Max => format!("s_max({}, {})", self.expr(&args[0]), self.expr(&args[1])),
            Func::Real => format!("({}).re", self.expr(&args[0])),
            Func::Imag => format!("({}).im", self.expr(&args[0])),
            Func::Cabs => format!("c_cabs({})", self.expr(&args[0])),
            Func::Norm => format!("c_norm({})", self.expr(&args[0])),
            Func::Arg => format!("c_arg({})", self.expr(&args[0])),
            _ if ty == Ty::Real => {
                let a = self.expr(&args[0]);
                match f {
                    Func::Conj | Func::Ident => format!("r_ident({a})"),
                    Func::Cosxx => format!("r_cos({a})"),
                    f if f.real_closed() => format!("r_{}({a})", fname(f)),
                    f => format!("c_{}(c_real({a})).re", fname(f)),
                }
            }
            _ => self.complex_call(f, &self.expr(&args[0])),
        }
    }

    fn conv(&self, inner: &Expr, to: Ty) -> String {
        let a = self.expr(inner);
        match (inner.ty, to) {
            (Ty::Bool, Ty::Int) => format!("select(0, 1, {a})"),
            (Ty::Bool, Ty::Real) => format!("s_c(select(0.0, 1.0, {a}))"),
            (Ty::Bool, Ty::Complex) => format!("c_f(select(0.0, 1.0, {a}), 0.0)"),
            (Ty::Int, Ty::Bool) => format!("({a} != 0)"),
            (Ty::Int, Ty::Real) => format!("s_c(f32({a}))"),
            (Ty::Int, Ty::Complex) => format!("c_f(f32({a}), 0.0)"),
            (Ty::Real, Ty::Bool) => format!("(s_v({a}) != 0.0)"),
            (Ty::Real, Ty::Int) => format!("i32(trunc(s_v({a})))"),
            (Ty::Real, Ty::Complex) => format!("c_real({a})"),
            (Ty::Complex, Ty::Bool) => format!("(s_v(({a}).re) != 0.0)"),
            (Ty::Complex, Ty::Int) => format!("i32(trunc(s_v(({a}).re)))"),
            (Ty::Complex, Ty::Real) => format!("({a}).re"),
            _ => a,
        }
    }

    fn stmts(&self, list: &[Stmt], out: &mut String, depth: usize, counter: &mut usize) {
        let pad = "    ".repeat(depth);
        for s in list {
            match s {
                Stmt::Assign(i, e) => {
                    let _ = writeln!(out, "{pad}{}{i} = {};", self.prefix, self.expr(e));
                }
                Stmt::SetIndex(i, idx, e) => {
                    let cap = self.prog.locals[*i].cap.unwrap_or(1);
                    let _ = writeln!(out, "{pad}{}{i}[clamp({}, 0, {})] = {};", self.prefix, self.expr(idx), cap - 1, self.expr(e));
                }
                Stmt::SetPart { local, re, value } => {
                    let _ = writeln!(out, "{pad}{}{local}.{} = {};", self.prefix, if *re { "re" } else { "im" }, self.expr(value));
                }
                Stmt::If(arms, els) => {
                    for (k, (c, b)) in arms.iter().enumerate() {
                        let kw = if k == 0 { "if" } else { "} else if" };
                        let _ = writeln!(out, "{pad}{kw} {} {{", self.expr(c));
                        self.stmts(b, out, depth + 1, counter);
                    }
                    if !els.is_empty() {
                        let _ = writeln!(out, "{pad}}} else {{");
                        self.stmts(els, out, depth + 1, counter);
                    }
                    let _ = writeln!(out, "{pad}}}");
                }
                Stmt::While(c, body) => {
                    *counter += 1;
                    let n = *counter;
                    let _ = writeln!(
                        out,
                        "{pad}for (var w{n}{p} = 0u; w{n}{p} < {MAX_INNER_LOOP}u && {}; w{n}{p} = w{n}{p} + 1u) {{",
                        self.expr(c),
                        p = self.prefix
                    );
                    self.stmts(body, out, depth + 1, counter);
                    let _ = writeln!(out, "{pad}}}");
                }
                Stmt::Repeat(body, c) => {
                    *counter += 1;
                    let n = *counter;
                    let p = &self.prefix;
                    let _ = writeln!(out, "{pad}var r{n}{p} = 0u;");
                    let _ = writeln!(out, "{pad}loop {{");
                    self.stmts(body, out, depth + 1, counter);
                    let _ = writeln!(out, "{pad}    r{n}{p} = r{n}{p} + 1u;");
                    let _ = writeln!(out, "{pad}    if r{n}{p} >= {MAX_INNER_LOOP}u || {} {{ break; }}", self.expr(c));
                    let _ = writeln!(out, "{pad}}}");
                }
            }
        }
    }
}

fn wgsl_ty(t: Ty) -> &'static str {
    match t {
        Ty::Bool => "bool",
        Ty::Int => "i32",
        Ty::Real => "S",
        Ty::Complex => "Cx",
        Ty::Color => "vec4<f32>",
    }
}

fn zero(t: Ty) -> &'static str {
    match t {
        Ty::Bool => "false",
        Ty::Int => "0",
        Ty::Real => "s_c(0.0)",
        Ty::Complex => "c_f(0.0, 0.0)",
        Ty::Color => "vec4<f32>(0.0)",
    }
}

/// The functions chosen for the program's function parameters.
pub fn func_choices(prog: &Program, values: &[ParamValue]) -> Vec<Option<Func>> {
    prog.params
        .iter()
        .enumerate()
        .map(|(i, p)| {
            if p.ty != Ty::Complex || p.kind != rfr_core::formula::ParamKind::Func {
                return None;
            }
            let name = match values.get(i) {
                Some(ParamValue::Func(f)) => Some(f.clone()),
                _ => None,
            }
            .or(match &p.default {
                ParamValue::Func(f) => Some(f.clone()),
                _ => None,
            });
            Some(name.and_then(|n| Func::by_name(&n)).unwrap_or(Func::Ident))
        })
        .collect()
}

/// The programs of a layer in the order their parameters fill `P`: fractal, transformations,
/// outside coloring, inside coloring.
pub fn param_programs(lp: &LayerPlan) -> Vec<&Prog> {
    let mut v = vec![&lp.fractal];
    v.extend(lp.transforms.iter());
    v.extend(lp.outside.formula());
    v.extend(lp.inside.formula());
    v
}

/// Parameter values of a layer as `P` entries.
pub fn param_values(lp: &LayerPlan) -> Vec<[f32; 4]> {
    let mut p = vec![[0.0f32; 4]; MAX_PARAMS];
    let mut k = 0;
    for prog in param_programs(lp) {
        for v in &prog.values {
            if k < MAX_PARAMS {
                p[k] = match v {
                    ParamValue::Color(c) => c.map(|x| x as f32),
                    v => {
                        let c = v.as_complex();
                        [c[0] as f32, c[1] as f32, 0.0, 0.0]
                    }
                };
            }
            k += 1;
        }
    }
    p
}

fn locals(g: &Gen, out: &mut String, pad: &str, first: Option<&str>) {
    for (i, l) in g.prog.locals.iter().enumerate() {
        if let Some(cap) = l.cap {
            // Function-scope variables start zeroed.
            let _ = writeln!(out, "{pad}var {}{i}: array<{}, {cap}>;", g.prefix, wgsl_ty(l.ty));
            continue;
        }
        let init = if i == 0
            && let Some(f) = first
        {
            f.to_string()
        } else {
            zero(l.ty).to_string()
        };
        let _ = writeln!(out, "{pad}var {}{i}: {} = {init};", g.prefix, wgsl_ty(l.ty));
    }
}

/// The complete kernel shader of a layer.
pub fn kernel(lp: &LayerPlan, mode: Mode) -> String {
    fn gen_for<'a>(prog: &'a Prog, prefix: String, base: &mut usize) -> Gen<'a> {
        let g = Gen { prog: &prog.program, values: &prog.values, funcs: func_choices(&prog.program, &prog.values), prefix, base: *base };
        *base += prog.program.params.len();
        g
    }
    let mut base = 0;
    let fractal = gen_for(&lp.fractal, "l".into(), &mut base);
    let transforms: Vec<Gen> = lp.transforms.iter().enumerate().map(|(k, t)| gen_for(t, format!("t{k}_"), &mut base)).collect();
    let outside = lp.outside.formula().map(|p| gen_for(p, "ko".into(), &mut base));
    let inside = lp.inside.formula().map(|p| gen_for(p, "ki".into(), &mut base));
    let mut counter = 0;

    // Transformations: each one runs only when the ones before left the point alive.
    let mut tr = String::new();
    for (k, g) in transforms.iter().enumerate() {
        let pad = "    ".repeat(k + 1);
        if k > 0 {
            let _ = writeln!(tr, "{}if !tsolid {{", "    ".repeat(k));
        }
        locals(g, &mut tr, &pad, Some("pixel"));
        g.stmts(&g.prog.init, &mut tr, k + 1, &mut counter);
        g.stmts(&g.prog.body, &mut tr, k + 1, &mut counter);
        let solid = g.prog.solid_slot.map_or("false".to_string(), |s| format!("{}{s}", g.prefix));
        let _ = writeln!(tr, "{pad}if {solid} {{ tsolid = true; }} else {{ pixel = {}0; }}", g.prefix);
    }
    for k in (1..transforms.len()).rev() {
        let _ = writeln!(tr, "{}}}", "    ".repeat(k));
    }

    let mut loc = String::new();
    locals(&fractal, &mut loc, "        ", None);
    for g in outside.iter().chain(inside.iter()) {
        locals(g, &mut loc, "        ", None);
    }
    let mut init = String::new();
    fractal.stmts(&fractal.prog.init, &mut init, 2, &mut counter);
    let mut color_init = String::new();
    let mut color_loop = String::new();
    for g in outside.iter().chain(inside.iter()) {
        g.stmts(&g.prog.init, &mut color_init, 2, &mut counter);
        g.stmts(&g.prog.body, &mut color_loop, 3, &mut counter);
    }
    let mut body = String::new();
    fractal.stmts(&fractal.prog.body, &mut body, 3, &mut counter);
    let mut fin = String::new();
    fractal.stmts(&fractal.prog.fin, &mut fin, 2, &mut counter);
    let final_of = |g: &Option<Gen>, inside: bool, counter: &mut usize| -> String {
        let mut out = String::new();
        match g {
            Some(g) => {
                g.stmts(&g.prog.fin, &mut out, 4, counter);
                let idx = g.prog.index_slot.map_or("0.0".to_string(), |s| format!("s_v({}{s})", g.prefix));
                let solid = g.prog.solid_slot.map_or("false".to_string(), |s| format!("{}{s}", g.prefix));
                match g.prog.color_slot.filter(|_| g.prog.direct) {
                    Some(k) => {
                        let _ = writeln!(
                            out,
                            "                if {solid} {{\n                    col = shade_formula({inside}, 0.0, true);\n                }} else {{\n                    col = k_direct({}{k});\n                }}",
                            g.prefix
                        );
                    }
                    None => {
                        let _ = writeln!(out, "                col = shade_formula({inside}, {idx}, {solid});");
                    }
                }
            }
            None => {
                let _ = writeln!(out, "                col = shade_native({inside}, zv, zpv, zp2v, iter, dzv);");
            }
        }
        out
    };
    let out_final = final_of(&outside, false, &mut counter);
    let in_final = final_of(&inside, true, &mut counter);
    let pixel = match mode {
        Mode::Plain => "    var pixel = c_f(pr.x, pr.y);".to_string(),
        Mode::Dual => {
            "    let dir = vec2<f32>(cos(turn * TAU), sin(turn * TAU));\n    var pixel = Cx(vec2<f32>(pr.x, dir.x), vec2<f32>(pr.y, dir.y));"
                .to_string()
        }
        Mode::Df64 => "    var pixel = Cx(dd_add(dd(U.view.x, U.lo.x), dd_f(off.x)), dd_add(dd(U.view.y, U.lo.y), dd_f(off.y)));".to_string(),
    };
    let mut mode_init = if mode == Mode::Df64 { "    dd_one = U.rot.w;\n".to_string() } else { String::new() };
    // One random stream per program (render::cpu seeds the same way).
    let mut rng_decl = String::new();
    let streams: Vec<(&str, u32)> = std::iter::once((fractal.prefix.as_str(), 1u32))
        .chain(transforms.iter().enumerate().map(|(k, g)| (g.prefix.as_str(), 2 + k as u32)))
        .chain(outside.iter().map(|g| (g.prefix.as_str(), 100)))
        .chain(inside.iter().map(|g| (g.prefix.as_str(), 101)))
        .collect();
    for (p, tag) in streams {
        let _ = writeln!(rng_decl, "var<private> rng_{p}: u32;");
        let _ = writeln!(mode_init, "    rng_{p} = hash(rng_base ^ {tag}u);");
    }
    let kernel = KERNEL
        .replace("//@MODE_INIT", &mode_init)
        .replace("//@RNG_DECL", &rng_decl)
        .replace("//@PIXEL", &pixel)
        .replace("//@TRANSFORMS", tr.trim_end())
        .replace("//@LOCALS", loc.trim_end())
        .replace("//@INIT", init.trim_end())
        .replace("//@COLOR_INIT", color_init.trim_end())
        .replace("//@BODY", body.trim_end())
        .replace("//@BAILOUT", &fractal.expr(&fractal.prog.bailout))
        .replace("//@COLOR_LOOP", color_loop.trim_end())
        .replace("//@FINAL", fin.trim_end())
        .replace("//@OUT_FINAL", out_final.trim_end())
        .replace("//@IN_FINAL", in_final.trim_end());
    let scalar = match mode {
        Mode::Plain => SCALAR_F32,
        Mode::Dual => SCALAR_DUAL,
        Mode::Df64 => SCALAR_DF64,
    };
    format!("{scalar}\n{CSTD}\n{BLEND}\n{kernel}")
}

/// The compositing shader (fill, group merge, accumulation).
pub fn composite_shader() -> String {
    format!("{BLEND}\n{}", include_str!("wgsl/composite.wgsl"))
}

/// Shader identity of a layer: its programs, the chosen functions, which regions use
/// formulas and the mode. Parameter values are not part of it (they live in a buffer).
pub fn signature(lp: &LayerPlan, mode: Mode) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for (k, p) in param_programs(lp).iter().enumerate() {
        k.hash(&mut h);
        p.id.hash(&mut h);
        format!("{:?}", p.program).hash(&mut h);
        format!("{:?}", func_choices(&p.program, &p.values)).hash(&mut h);
        format!("{:?}", merge_choices(&p.program, &p.values)).hash(&mut h);
    }
    lp.outside.formula().is_some().hash(&mut h);
    lp.inside.formula().is_some().hash(&mut h);
    lp.transforms.len().hash(&mut h);
    mode.hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literals_are_valid_wgsl_numbers() {
        assert_eq!(lit(2.0), "2e0");
        assert_eq!(lit(-0.5), "-5e-1");
        assert_eq!(lit(1e300), "3.4e38");
        assert_eq!(lit(1e-300), "0e0");
    }
}
