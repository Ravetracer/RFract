// SPDX-License-Identifier: LGPL-3.0-or-later

//! Formula IR → WGSL. Every IR node maps to one expression over the scalar layer (`s_*`) and
//! the complex library (`c_*`, `r_*`) of `cstd.wgsl`, mirroring rfr_core::formula::cpu. Parameter
//! values come from the `P` uniform (editing them never recompiles); only the chosen functions
//! of function parameters and the scalar mode are part of the shader.

use std::fmt::Write;
use std::hash::{Hash, Hasher};

use rfr_core::formula::ParamValue;
use rfr_core::formula::ir::*;

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

/// Parameters the `P` uniform can hold.
pub const MAX_PARAMS: usize = 64;

struct Gen<'a> {
    prog: &'a Program,
    funcs: Vec<Option<Func>>,
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
            Kind::Local(i) => format!("l{i}"),
            Kind::Param(i) => match self.prog.params[*i].ty {
                Ty::Bool => format!("(P.p[{i}].x != 0.0)"),
                Ty::Int => format!("i32(P.p[{i}].x)"),
                Ty::Real => format!("s_c(P.p[{i}].x)"),
                Ty::Complex => format!("c_f(P.p[{i}].x, P.p[{i}].y)"),
            },
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
            Kind::CallParam(i, a) => {
                let f = self.funcs.get(*i).copied().flatten().unwrap_or(Func::Ident);
                self.complex_call(f, &self.expr(a))
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
                    let _ = writeln!(out, "{pad}l{i} = {};", self.expr(e));
                }
                Stmt::SetPart { local, re, value } => {
                    let _ = writeln!(out, "{pad}l{local}.{} = {};", if *re { "re" } else { "im" }, self.expr(value));
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
                    let _ = writeln!(out, "{pad}for (var w{n} = 0u; w{n} < {MAX_INNER_LOOP}u && {}; w{n} = w{n} + 1u) {{", self.expr(c));
                    self.stmts(body, out, depth + 1, counter);
                    let _ = writeln!(out, "{pad}}}");
                }
                Stmt::Repeat(body, c) => {
                    *counter += 1;
                    let n = *counter;
                    let _ = writeln!(out, "{pad}var r{n} = 0u;");
                    let _ = writeln!(out, "{pad}loop {{");
                    self.stmts(body, out, depth + 1, counter);
                    let _ = writeln!(out, "{pad}    r{n} = r{n} + 1u;");
                    let _ = writeln!(out, "{pad}    if r{n} >= {MAX_INNER_LOOP}u || {} {{ break; }}", self.expr(c));
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
    }
}

fn zero(t: Ty) -> &'static str {
    match t {
        Ty::Bool => "false",
        Ty::Int => "0",
        Ty::Real => "s_c(0.0)",
        Ty::Complex => "c_f(0.0, 0.0)",
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

/// The complete kernel shader for `prog`.
pub fn kernel(prog: &Program, values: &[ParamValue], mode: Mode) -> String {
    let g = Gen { prog, funcs: func_choices(prog, values) };
    let mut locals = String::new();
    for (i, l) in prog.locals.iter().enumerate() {
        let _ = writeln!(locals, "    var l{i}: {} = {};", wgsl_ty(l.ty), zero(l.ty));
    }
    let mut counter = 0;
    let mut init = String::new();
    g.stmts(&prog.init, &mut init, 1, &mut counter);
    let mut body = String::new();
    g.stmts(&prog.body, &mut body, 2, &mut counter);
    let mut fin = String::new();
    g.stmts(&prog.fin, &mut fin, 1, &mut counter);
    let pixel = match mode {
        Mode::Plain => "    let pixel = c_f(pr.x, pr.y);".to_string(),
        Mode::Dual => {
            "    let dir = vec2<f32>(cos(turn * TAU), sin(turn * TAU));\n    let pixel = Cx(vec2<f32>(pr.x, dir.x), vec2<f32>(pr.y, dir.y));"
                .to_string()
        }
        Mode::Df64 => "    let pixel = Cx(dd_add(dd(U.view.x, U.lo.x), dd_f(off.x)), dd_add(dd(U.view.y, U.lo.y), dd_f(off.y)));".to_string(),
    };
    let mode_init = if mode == Mode::Df64 { "    dd_one = U.rot.w;" } else { "" };
    let kernel = KERNEL
        .replace("//@MODE_INIT", mode_init)
        .replace("//@PIXEL", &pixel)
        .replace("//@LOCALS", locals.trim_end())
        .replace("//@INIT", init.trim_end())
        .replace("//@BODY", body.trim_end())
        .replace("//@BAILOUT", &g.expr(&prog.bailout))
        .replace("//@FINAL", fin.trim_end());
    let scalar = match mode {
        Mode::Plain => SCALAR_F32,
        Mode::Dual => SCALAR_DUAL,
        Mode::Df64 => SCALAR_DF64,
    };
    format!("{scalar}\n{CSTD}\n{kernel}")
}

/// Shader identity: the program, the chosen functions and the mode. Parameter values are not
/// part of it (they live in a buffer).
pub fn signature(formula_id: &str, prog: &Program, values: &[ParamValue], mode: Mode) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    formula_id.hash(&mut h);
    format!("{:?}", prog.body).hash(&mut h);
    format!("{:?}", prog.init).hash(&mut h);
    format!("{:?}", func_choices(prog, values)).hash(&mut h);
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
