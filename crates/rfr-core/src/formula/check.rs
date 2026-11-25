// SPDX-License-Identifier: LGPL-3.0-or-later

//! Name resolution and type checking: AST → typed IR. Locals take the type of their first
//! assignment (or declaration); when a later assignment needs a wider type (int → real →
//! complex), the formula is checked again with the wider type from the start.

use std::collections::HashMap;

use super::ast::{self, BinOp, DeclTy, Dialect, FormulaAst, ParamKind, ProgramKind, Target, UnOp};
use super::ir::*;
use super::parse::{Error, err};

struct Ck<'a> {
    ast: &'a FormulaAst,
    dialect: Dialect,
    kind: ProgramKind,
    index_slot: Option<usize>,
    color_slot: Option<usize>,
    direct: bool,
    /// Variables with a declared type (UFM `float x`): assignments convert, never widen.
    declared: std::collections::HashSet<String>,
    solid_slot: Option<usize>,
    params: Vec<Param>,
    locals: Vec<Local>,
    /// Local slot by (normalized) name.
    slots: HashMap<String, usize>,
    /// Types fixed by declarations or by an earlier pass (widening).
    preset: HashMap<String, Ty>,
    widen: HashMap<String, Ty>,
    /// FractInt slots (p1…, fn1…) to parameter indexes.
    classic: HashMap<String, usize>,
    has_julia: bool,
    transcendental: bool,
    /// Arrays: element slot and the slots of their dimension sizes.
    arrays: HashMap<String, (usize, Vec<usize>)>,
}

fn conv(e: Expr, ty: Ty) -> Expr {
    if e.ty == ty {
        return e;
    }
    // Fold constant conversions.
    match (&e.kind, ty) {
        (Kind::Int(i), Ty::Real) => return Expr { ty, kind: Kind::Real(*i as f64) },
        (Kind::Bool(b), Ty::Real) => return Expr { ty, kind: Kind::Real(*b as i64 as f64) },
        (Kind::Bool(b), Ty::Int) => return Expr { ty, kind: Kind::Int(*b as i64) },
        _ => {}
    }
    Expr { ty, kind: Kind::Conv(Box::new(e)) }
}

fn real(v: f64) -> Expr {
    Expr { ty: Ty::Real, kind: Kind::Real(v) }
}

/// Constant integer exponent of `e`, if it is one (small enough for repeated multiplication).
fn const_int(e: &ast::Expr) -> Option<i32> {
    match e {
        ast::Expr::Num { v, .. } if v.fract() == 0.0 && v.abs() <= 64.0 => Some(*v as i32),
        ast::Expr::Unary(UnOp::Neg, inner) => const_int(inner).map(|n| -n),
        ast::Expr::Complex(re, im) => match **im {
            ast::Expr::Num { v: 0.0, .. } => const_int(re),
            _ => None,
        },
        _ => None,
    }
}

impl<'a> Ck<'a> {
    fn norm(&self, name: &str) -> String {
        if self.dialect == Dialect::Ufm { name.to_ascii_lowercase() } else { name.to_string() }
    }

    fn param_index(&self, name: &str) -> Option<usize> {
        match self.dialect {
            Dialect::Ufm => self.params.iter().position(|p| p.name.eq_ignore_ascii_case(name)),
            Dialect::Frm => self.params.iter().position(|p| p.name == name),
        }
    }

    /// A parameter used but not declared (allowed in UFM syntax): complex with default 0, a
    /// function parameter defaulting to `ident`, or an enumeration whose items come from the
    /// strings it is compared with.
    fn implicit_param(&mut self, name: &str, kind: ParamKind) -> usize {
        if let Some(i) = self.param_index(name) {
            return i;
        }
        let default = match kind {
            ParamKind::Func => ast::ParamValue::Func("ident".into()),
            ParamKind::Int => ast::ParamValue::Int(0),
            _ => ast::ParamValue::Complex([0.0, 0.0]),
        };
        self.params.push(Param {
            name: name.to_string(),
            kind,
            ty: param_ty(kind),
            default,
            caption: name.to_string(),
            hint: String::new(),
            min: None,
            max: None,
            items: Vec::new(),
        });
        self.params.len() - 1
    }

    /// Value of a constant size expression (numbers, parameter defaults, arithmetic, rounding).
    fn const_eval(&self, e: &ast::Expr) -> Option<f64> {
        Some(match e {
            ast::Expr::Num { v, .. } => *v,
            ast::Expr::Param(n) => self.params[self.param_index(n)?].default.as_f64(),
            ast::Expr::Unary(UnOp::Neg, a) => -self.const_eval(a)?,
            ast::Expr::Binary(op, a, b) => {
                let (x, y) = (self.const_eval(a)?, self.const_eval(b)?);
                match op {
                    BinOp::Add => x + y,
                    BinOp::Sub => x - y,
                    BinOp::Mul => x * y,
                    BinOp::Div if y != 0.0 => x / y,
                    _ => return None,
                }
            }
            ast::Expr::Call { name, param: false, args } if args.len() == 1 => {
                let v = self.const_eval(&args[0])?;
                match name.to_ascii_lowercase().as_str() {
                    "round" => v.round(),
                    "floor" => v.floor(),
                    "ceil" => v.ceil(),
                    "trunc" => v.trunc(),
                    "abs" => v.abs(),
                    "real" => v,
                    _ => return None,
                }
            }
            _ => return None,
        })
    }

    /// Declares an array: its elements and one int local per dimension size.
    fn declare_array(&mut self, ty: DeclTy, name: &str, dims: &[ast::Expr], line: usize, out: &mut Vec<Stmt>) -> Result<(), Error> {
        let key = self.norm(name);
        let t = match ty {
            DeclTy::Bool => Ty::Bool,
            DeclTy::Int => Ty::Int,
            DeclTy::Float => Ty::Real,
            DeclTy::Complex => Ty::Complex,
            DeclTy::Color => Ty::Color,
        };
        fn uses_screen(e: &ast::Expr) -> bool {
            match e {
                ast::Expr::Sys(s) => matches!(s.to_ascii_lowercase().as_str(), "width" | "height" | "screenmax"),
                ast::Expr::Unary(_, a) | ast::Expr::Mag(a) => uses_screen(a),
                ast::Expr::Binary(_, a, b) | ast::Expr::Complex(a, b) => uses_screen(a) || uses_screen(b),
                ast::Expr::Call { args, .. } | ast::Expr::Index(_, args) => args.iter().any(uses_screen),
                _ => false,
            }
        }
        if dims.iter().any(uses_screen) {
            return err(line, format!("the image-sized array '{name}' is not supported (arrays are kept per sample, not per image)"));
        }
        let sizes: Option<Vec<f64>> = dims.iter().map(|d| self.const_eval(d)).collect();
        let cap = match sizes {
            Some(v) if !v.is_empty() => {
                let n = v.iter().map(|x| x.max(1.0)).product::<f64>();
                if n > ARRAY_MAX as f64 {
                    return err(line, format!("the array '{name}' has {n} elements; at most {ARRAY_MAX} are supported"));
                }
                (n as usize).max(1)
            }
            _ => ARRAY_CAP,
        };
        let slot = match self.arrays.get(&key) {
            Some(&(s, _)) => s,
            None => {
                self.locals.push(Local { name: key.clone(), ty: t, cap: Some(cap) });
                self.locals.len() - 1
            }
        };
        let ndim = dims.len().max(1);
        let mut dim_slots = Vec::new();
        for k in 0..ndim {
            let dk = format!("#dim{k}:{key}");
            let i = self.local(&dk, Ty::Int);
            dim_slots.push(i);
            let v = match dims.get(k) {
                Some(d) => {
                    let e = self.expr(d, line)?;
                    conv(self.numeric(e), Ty::Int)
                }
                None => Expr { ty: Ty::Int, kind: Kind::Int(0) },
            };
            out.push(Stmt::Assign(i, v));
        }
        self.arrays.insert(key, (slot, dim_slots));
        Ok(())
    }

    /// Flat element index of `name[idx…]` and the array's slot.
    fn array_index(&mut self, name: &str, idx: &[ast::Expr], line: usize) -> Result<(usize, Expr), Error> {
        let key = self.norm(name);
        let Some((slot, dims)) = self.arrays.get(&key).cloned() else { return err(line, format!("'{name}' is not an array")) };
        if idx.is_empty() || idx.len() > dims.len() {
            return err(line, format!("'{name}' needs {} index(es)", dims.len()));
        }
        let mut acc = {
            let e = self.expr(&idx[0], line)?;
            conv(self.numeric(e), Ty::Int)
        };
        for (k, ix) in idx.iter().enumerate().skip(1) {
            let d = Expr { ty: Ty::Int, kind: Kind::Local(dims[k]) };
            let e = self.expr(ix, line)?;
            let i = conv(self.numeric(e), Ty::Int);
            let mul = Expr { ty: Ty::Int, kind: Kind::Arith(Arith::Mul, Box::new(acc), Box::new(d)) };
            acc = Expr { ty: Ty::Int, kind: Kind::Arith(Arith::Add, Box::new(mul), Box::new(i)) };
        }
        Ok((slot, acc))
    }

    fn sys(&mut self, s: Sys) -> Expr {
        if s == Sys::IsMand {
            self.has_julia = true;
        }
        if s == Sys::C {
            // Formulas reading `c` iterate the parameter plane in Mandelbrot mode and use the
            // seed in Julia mode.
            self.has_julia = true;
        }
        let e = Expr { ty: s.ty(), kind: Kind::Sys(s) };
        // FRM-like v1: maxit is a real value.
        if s == Sys::MaxIt && self.dialect == Dialect::Frm { conv(e, Ty::Real) } else { e }
    }

    fn local(&mut self, name: &str, ty: Ty) -> usize {
        let key = self.norm(name);
        if let Some(&i) = self.slots.get(&key) {
            return i;
        }
        let ty = self.preset.get(&key).copied().map_or(ty, |p| p.max(ty));
        self.locals.push(Local { name: key.clone(), ty, cap: None });
        self.slots.insert(key, self.locals.len() - 1);
        self.locals.len() - 1
    }

    /// Value of a plain identifier.
    fn ident(&mut self, name: &str, line: usize) -> Result<Expr, Error> {
        let key = self.norm(name);
        if self.arrays.contains_key(&key) {
            return err(line, format!("the array '{name}' needs an index"));
        }
        if key == "z" {
            return Ok(Expr { ty: Ty::Complex, kind: Kind::Local(Z) });
        }
        if let Some(&i) = self.slots.get(&key) {
            return Ok(Expr { ty: self.locals[i].ty, kind: Kind::Local(i) });
        }
        if self.dialect == Dialect::Frm {
            if let Some(i) = self.param_index(name) {
                if self.params[i].kind == ParamKind::Func {
                    return err(line, format!("the function parameter '{name}' can only be called"));
                }
                return Ok(Expr { ty: self.params[i].ty, kind: Kind::Param(i) });
            }
            let s = match name {
                "pixel" => Some(Sys::Pixel),
                "c" => Some(Sys::C),
                "zPrev" => Some(Sys::ZPrev),
                "LastSqr" => Some(Sys::LastSqr),
                "maxit" => Some(Sys::MaxIt),
                "ismand" => Some(Sys::IsMand),
                "pi" => return Ok(real(std::f64::consts::PI)),
                "e" => return Ok(real(std::f64::consts::E)),
                "p1" | "p2" | "p3" | "p4" | "p5" => {
                    return Ok(match self.classic.get(name) {
                        Some(&i) => Expr { ty: self.params[i].ty, kind: Kind::Param(i) },
                        None => Expr { ty: Ty::Complex, kind: Kind::Complex(Box::new(real(0.0)), Box::new(real(0.0))) },
                    });
                }
                _ => None,
            };
            if let Some(s) = s {
                return Ok(self.sys(s));
            }
        }
        // A local that is assigned later (in a later section or a later loop iteration).
        if self.assigned_anywhere(&key) {
            let i = self.local(name, Ty::Complex);
            return Ok(Expr { ty: self.locals[i].ty, kind: Kind::Local(i) });
        }
        if self.dialect == Dialect::Ufm {
            // Older UFM style: plain names for the predefined values and parameters.
            match key.as_str() {
                "pixel" => return Ok(self.sys(Sys::Pixel)),
                "pi" => return Ok(real(std::f64::consts::PI)),
                "e" => return Ok(real(std::f64::consts::E)),
                // FractInt's predefined values.
                "whitesq" => {
                    let x = self.sys(Sys::X);
                    let y = self.sys(Sys::Y);
                    let sum = Expr { ty: Ty::Int, kind: Kind::Arith(Arith::Add, Box::new(x), Box::new(y)) };
                    let m = Expr { ty: Ty::Int, kind: Kind::Arith(Arith::Mod, Box::new(sum), Box::new(Expr { ty: Ty::Int, kind: Kind::Int(2) })) };
                    return Ok(Expr { ty: Ty::Bool, kind: Kind::Cmp(Cmp::Eq, Box::new(m), Box::new(Expr { ty: Ty::Int, kind: Kind::Int(1) })) });
                }
                "scrnpix" => return Ok(self.sys(Sys::ScreenPixel)),
                "scrnmax" => return Ok(self.sys(Sys::ScreenMax)),
                "maxit" => return Ok(conv(self.sys(Sys::MaxIt), Ty::Real)),
                "lastsqr" => return Ok(self.sys(Sys::LastSqr)),
                "ismand" => return Ok(self.sys(Sys::IsMand)),
                "rand" => return Ok(self.sys(Sys::Random)),
                "center" => return Ok(self.sys(Sys::Center)),
                // (rotation in degrees, skew) and (magnification, x magnification).
                "rotskew" => {
                    let a = self.sys(Sys::Angle);
                    let deg = Expr { ty: Ty::Real, kind: Kind::Arith(Arith::Mul, Box::new(a), Box::new(real(180.0 / std::f64::consts::PI))) };
                    return Ok(Expr { ty: Ty::Complex, kind: Kind::Complex(Box::new(deg), Box::new(real(0.0))) });
                }
                "magxmag" => {
                    let m = self.sys(Sys::Magn);
                    return Ok(Expr { ty: Ty::Complex, kind: Kind::Complex(Box::new(m), Box::new(real(1.0))) });
                }
                "p1" | "p2" | "p3" | "p4" | "p5" | "p6" => {
                    let i = self.implicit_param(&key, ParamKind::Complex);
                    return Ok(Expr { ty: self.params[i].ty, kind: Kind::Param(i) });
                }
                _ => {}
            }
        }
        if self.ast.fractint {
            // FractInt variables start at zero.
            let i = self.local(name, Ty::Complex);
            return Ok(Expr { ty: self.locals[i].ty, kind: Kind::Local(i) });
        }
        err(line, format!("unknown name '{name}'"))
    }

    /// Parameters assigned somewhere (`@p = …`) become locals that start with the value.
    fn shadow_params(&mut self) -> Vec<Stmt> {
        fn walk(stmts: &[ast::Stmt], out: &mut Vec<String>) {
            for s in stmts {
                match s {
                    ast::Stmt::Assign { target: Target::Var(v), .. } if v.starts_with('@') => out.push(v[1..].to_string()),
                    ast::Stmt::If { arms, els, .. } => {
                        arms.iter().for_each(|(_, b)| walk(b, out));
                        walk(els, out);
                    }
                    ast::Stmt::While { body, .. } | ast::Stmt::Repeat { body, .. } | ast::Stmt::Seq(body) => walk(body, out),
                    _ => {}
                }
            }
        }
        let mut names = Vec::new();
        let a = self.ast;
        for b in [&a.global, &a.init, &a.body, &a.fin, &a.transform] {
            walk(b, &mut names);
        }
        let mut out = Vec::new();
        for n in names {
            let key = format!("@{}", self.norm(&n));
            if self.slots.contains_key(&key) {
                continue;
            }
            let i = match self.param_index(&n) {
                Some(i) => i,
                None => self.implicit_param(&n, ParamKind::Complex),
            };
            if self.params[i].kind == ParamKind::Func {
                continue;
            }
            let ty = self.params[i].ty;
            let slot = self.local(&key, ty);
            out.push(Stmt::Assign(slot, conv(Expr { ty, kind: Kind::Param(i) }, self.locals[slot].ty)));
        }
        out
    }

    fn assigned_anywhere(&self, key: &str) -> bool {
        fn walk(stmts: &[ast::Stmt], key: &str, ufm: bool) -> bool {
            stmts.iter().any(|s| match s {
                ast::Stmt::Assign { target: Target::Var(v) | Target::Comp { var: v, .. }, .. } => {
                    if ufm {
                        v.eq_ignore_ascii_case(key)
                    } else {
                        v == key
                    }
                }
                ast::Stmt::Decl { name, .. } => name.eq_ignore_ascii_case(key),
                ast::Stmt::If { arms, els, .. } => arms.iter().any(|(_, b)| walk(b, key, ufm)) || walk(els, key, ufm),
                ast::Stmt::While { body, .. } | ast::Stmt::Repeat { body, .. } | ast::Stmt::Seq(body) => walk(body, key, ufm),
                _ => false,
            })
        }
        let ufm = self.dialect == Dialect::Ufm;
        let a = self.ast;
        [&a.global, &a.init, &a.body, &a.fin, &a.transform].iter().any(|b| walk(b, key, ufm))
    }

    fn ufm_sys(&mut self, name: &str, line: usize) -> Result<Expr, Error> {
        let lower = name.to_ascii_lowercase();
        match (self.kind, lower.as_str()) {
            (ProgramKind::Transform, "pixel") => return Ok(Expr { ty: Ty::Complex, kind: Kind::Local(Z) }),
            (ProgramKind::Coloring, "index") => return Ok(Expr { ty: Ty::Real, kind: Kind::Local(self.index_slot.unwrap_or(0)) }),
            (ProgramKind::Coloring | ProgramKind::Transform, "solid") => {
                return Ok(Expr { ty: Ty::Bool, kind: Kind::Local(self.solid_slot.unwrap_or(0)) });
            }
            (ProgramKind::Coloring | ProgramKind::Transform, "z") => return Ok(self.sys(Sys::Z)),
            (ProgramKind::Coloring, "color") => return Ok(Expr { ty: Ty::Color, kind: Kind::Local(self.color_slot.unwrap_or(0)) }),
            _ => {}
        }
        let s = match lower.as_str() {
            "pixel" => Sys::Pixel,
            "z" => return Ok(Expr { ty: Ty::Complex, kind: Kind::Local(Z) }),
            "numiter" => Sys::NumIter,
            "maxiter" | "maxit" => Sys::MaxIt,
            "pi" => return Ok(real(std::f64::consts::PI)),
            "e" => return Ok(real(std::f64::consts::E)),
            "center" => Sys::Center,
            "magn" => Sys::Magn,
            "ismand" => Sys::IsMand,
            "c" => Sys::C,
            "zprev" => Sys::ZPrev,
            "x" => Sys::X,
            "y" => Sys::Y,
            "width" => Sys::Width,
            "height" => Sys::Height,
            "screenpixel" | "scrnpix" => Sys::ScreenPixel,
            "screenmax" | "scrnmax" => Sys::ScreenMax,
            "angle" => Sys::Angle,
            "random" | "rand" => Sys::Random,
            "randomrange" => return Ok(real(2147483647.0)),
            other => return err(line, format!("'#{other}' is not supported in formulas yet")),
        };
        Ok(self.sys(s))
    }

    /// Bool → int (UFM) or real (FRM-like) for arithmetic.
    fn numeric(&self, e: Expr) -> Expr {
        if e.ty == Ty::Bool { conv(e, if self.dialect == Dialect::Ufm { Ty::Int } else { Ty::Real }) } else { e }
    }

    fn cond(&self, e: Expr) -> Expr {
        conv(e, Ty::Bool)
    }

    fn expr(&mut self, e: &ast::Expr, line: usize) -> Result<Expr, Error> {
        Ok(match e {
            ast::Expr::Num { v, int } => {
                if *int && v.abs() < 9.0e15 {
                    Expr { ty: Ty::Int, kind: Kind::Int(*v as i64) }
                } else {
                    real(*v)
                }
            }
            ast::Expr::Bool(b) => Expr { ty: Ty::Bool, kind: Kind::Bool(*b) },
            ast::Expr::Complex(a, b) => {
                let a = conv(self.expr(a, line)?, Ty::Real);
                let b = conv(self.expr(b, line)?, Ty::Real);
                Expr { ty: Ty::Complex, kind: Kind::Complex(Box::new(a), Box::new(b)) }
            }
            ast::Expr::Ident(name) => self.ident(name, line)?,
            ast::Expr::Sys(name) => self.ufm_sys(name, line)?,
            ast::Expr::Param(name) if self.slots.contains_key(&format!("@{}", self.norm(name))) => {
                let i = self.slots[&format!("@{}", self.norm(name))];
                Expr { ty: self.locals[i].ty, kind: Kind::Local(i) }
            }
            ast::Expr::Param(name) => match self.param_index(name) {
                Some(i) if self.params[i].kind == ParamKind::Func => {
                    return err(line, format!("the function parameter '@{name}' can only be called"));
                }
                Some(i) => Expr { ty: self.params[i].ty, kind: Kind::Param(i) },
                None if self.dialect == Dialect::Ufm => {
                    let i = self.implicit_param(name, ParamKind::Complex);
                    Expr { ty: self.params[i].ty, kind: Kind::Param(i) }
                }
                None => return err(line, format!("unknown parameter '@{name}'")),
            },
            ast::Expr::Index(name, idx) => {
                let (slot, i) = self.array_index(name, idx, line)?;
                Expr { ty: self.locals[slot].ty, kind: Kind::Index(slot, Box::new(i)) }
            }
            ast::Expr::Str(_) => return err(line, "a text can only be compared with an enumeration parameter (@mode == \"Julia\")"),
            ast::Expr::Mag(inner) => {
                let v = {
                    let v = self.expr(inner, line)?;
                    self.numeric(v)
                };
                match (v.ty, self.dialect) {
                    (Ty::Complex, Dialect::Frm) => Expr { ty: Ty::Real, kind: Kind::Call(Func::Cabs, vec![v]) },
                    (Ty::Complex, Dialect::Ufm) => Expr { ty: Ty::Real, kind: Kind::Call(Func::Norm, vec![v]) },
                    (Ty::Int, _) => Expr { ty: Ty::Real, kind: Kind::Call(Func::Abs, vec![conv(v, Ty::Real)]) },
                    _ => Expr { ty: Ty::Real, kind: Kind::Call(Func::Abs, vec![v]) },
                }
            }
            ast::Expr::Unary(UnOp::Neg, inner) => {
                let v = {
                    let v = self.expr(inner, line)?;
                    self.numeric(v)
                };
                match v.kind {
                    Kind::Real(x) => real(-x),
                    Kind::Int(i) => Expr { ty: Ty::Int, kind: Kind::Int(-i) },
                    _ => Expr { ty: v.ty, kind: Kind::Neg(Box::new(v)) },
                }
            }
            ast::Expr::Unary(UnOp::Not, inner) => {
                let v = {
                    let v = self.expr(inner, line)?;
                    self.cond(v)
                };
                Expr { ty: Ty::Bool, kind: Kind::Not(Box::new(v)) }
            }
            ast::Expr::Binary(op, a, b) => self.binary(*op, a, b, line)?,
            ast::Expr::Call { name, param, args } => self.call(name, *param, args, line)?,
        })
    }

    fn binary(&mut self, op: BinOp, a: &ast::Expr, b: &ast::Expr, line: usize) -> Result<Expr, Error> {
        if op == BinOp::Pow {
            let base = {
                let v = self.expr(a, line)?;
                self.numeric(v)
            };
            if let Some(n) = const_int(b) {
                let ty = base.ty.max(Ty::Real);
                return Ok(Expr { ty, kind: Kind::PowI(Box::new(conv(base, ty)), n) });
            }
            let ex = {
                let v = self.expr(b, line)?;
                self.numeric(v)
            };
            if ex.ty == Ty::Int {
                let ty = base.ty.max(Ty::Real);
                return Ok(Expr { ty, kind: Kind::PowN(Box::new(conv(base, ty)), Box::new(ex)) });
            }
            self.transcendental = true;
            let ty = base.ty.max(ex.ty).max(Ty::Real);
            let ex = if self.dialect == Dialect::Frm && ty == Ty::Complex {
                // FRM-like v1 uses the real part of the exponent.
                let re = conv(ex, Ty::Real);
                Expr { ty: Ty::Complex, kind: Kind::Complex(Box::new(re), Box::new(real(0.0))) }
            } else {
                conv(ex, ty)
            };
            return Ok(Expr { ty, kind: Kind::Pow(Box::new(conv(base, ty)), Box::new(ex)) });
        }
        if matches!(op, BinOp::Eq | BinOp::Ne) {
            let pair = match (a, b) {
                (ast::Expr::Param(p), ast::Expr::Str(s)) | (ast::Expr::Str(s), ast::Expr::Param(p)) => Some((p, s)),
                _ => None,
            };
            if let Some((p, s)) = pair {
                return self.enum_compare(op, p, s, line);
            }
        }
        let l = self.expr(a, line)?;
        let r = self.expr(b, line)?;
        let logic =
            |k: fn(Box<Expr>, Box<Expr>) -> Kind, s: &Self| Expr { ty: Ty::Bool, kind: k(Box::new(s.cond(l.clone())), Box::new(s.cond(r.clone()))) };
        match op {
            BinOp::Or => return Ok(logic(Kind::Or, self)),
            BinOp::And => return Ok(logic(Kind::And, self)),
            _ => {}
        }
        let to_num = |e: Expr| if e.ty == Ty::Bool { conv(e, Ty::Int) } else { e };
        let (l, r) = if matches!(op, BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge | BinOp::Eq | BinOp::Ne) {
            (to_num(l), to_num(r))
        } else {
            (self.numeric(l), self.numeric(r))
        };
        let cmp = |c: Cmp, ty: Ty| Expr { ty: Ty::Bool, kind: Kind::Cmp(c, Box::new(conv(l.clone(), ty)), Box::new(conv(r.clone(), ty))) };
        let both_int = l.ty == Ty::Int && r.ty == Ty::Int;
        let order_ty = if both_int { Ty::Int } else { Ty::Real };
        let eq_ty = l.ty.max(r.ty).max(if both_int { Ty::Int } else { Ty::Real });
        Ok(match op {
            BinOp::Lt => cmp(Cmp::Lt, order_ty),
            BinOp::Gt => cmp(Cmp::Gt, order_ty),
            BinOp::Le => cmp(Cmp::Le, order_ty),
            BinOp::Ge => cmp(Cmp::Ge, order_ty),
            BinOp::Eq => cmp(Cmp::Eq, eq_ty),
            BinOp::Ne => cmp(Cmp::Ne, eq_ty),
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod => {
                let mut ty = l.ty.max(r.ty);
                if op == BinOp::Mod && ty == Ty::Complex {
                    ty = Ty::Real;
                }
                let k = match op {
                    BinOp::Add => Arith::Add,
                    BinOp::Sub => Arith::Sub,
                    BinOp::Mul => Arith::Mul,
                    BinOp::Div => Arith::Div,
                    _ => Arith::Mod,
                };
                Expr { ty, kind: Kind::Arith(k, Box::new(conv(l, ty)), Box::new(conv(r, ty))) }
            }
            BinOp::Or | BinOp::And | BinOp::Pow => unreachable!(),
        })
    }

    /// `@param == "Item"`: compares an enumeration's value with the index of the item.
    fn enum_compare(&mut self, op: BinOp, name: &str, item: &str, line: usize) -> Result<Expr, Error> {
        let i = match self.param_index(name) {
            Some(i) => i,
            None if self.dialect == Dialect::Ufm => self.implicit_param(name, ParamKind::Int),
            None => return err(line, format!("unknown parameter '@{name}'")),
        };
        let wanted = item.trim();
        let p = &mut self.params[i];
        let idx = match p.items.iter().position(|it| it.trim().eq_ignore_ascii_case(wanted)) {
            Some(k) => k as i64,
            None if p.kind == ParamKind::Int && p.default == ast::ParamValue::Int(0) && p.caption == p.name => {
                // An implicit enumeration: its items are the texts it is compared with.
                p.items.push(wanted.to_string());
                (p.items.len() - 1) as i64
            }
            None => -1,
        };
        let ty = self.params[i].ty;
        let lhs = conv(Expr { ty, kind: Kind::Param(i) }, Ty::Int);
        let c = if op == BinOp::Eq { Cmp::Eq } else { Cmp::Ne };
        Ok(Expr { ty: Ty::Bool, kind: Kind::Cmp(c, Box::new(lhs), Box::new(Expr { ty: Ty::Int, kind: Kind::Int(idx) })) })
    }

    fn func_param(&self, name: &str, param: bool) -> Option<usize> {
        let i = if param || self.dialect == Dialect::Frm { self.param_index(name) } else { None };
        let i = i.or_else(|| if self.dialect == Dialect::Frm { self.classic.get(name).copied() } else { None });
        i.filter(|&i| self.params[i].kind == ParamKind::Func)
    }

    fn call(&mut self, name: &str, param: bool, args: &[ast::Expr], line: usize) -> Result<Expr, Error> {
        if let Some(i) = self.func_param(name, param) {
            if args.len() != 1 {
                return err(line, format!("'{name}' takes one argument"));
            }
            let a = conv(
                {
                    let v = self.expr(&args[0], line)?;
                    self.numeric(v)
                },
                Ty::Complex,
            );
            self.transcendental = true;
            return Ok(Expr { ty: Ty::Complex, kind: Kind::CallParam(i, Box::new(a)) });
        }
        let lower_name = name.to_ascii_lowercase();
        if self.dialect == Dialect::Ufm && lower_name == "random" && args.len() == 1 {
            let v = self.expr(&args[0], line)?;
            let a = conv(self.numeric(v), Ty::Int);
            return Ok(Expr { ty: Ty::Int, kind: Kind::RandomInt(Box::new(a)) });
        }
        let old_style = self.dialect == Dialect::Ufm && !param && matches!(lower_name.as_str(), "fn1" | "fn2" | "fn3" | "fn4");
        if (param && self.dialect == Dialect::Ufm && self.param_index(name).is_none()) || old_style {
            if args.len() != 1 {
                return err(line, format!("'{name}' takes one argument"));
            }
            let i = self.implicit_param(&lower_name, ParamKind::Func);
            let v = self.expr(&args[0], line)?;
            let a = conv(self.numeric(v), Ty::Complex);
            self.transcendental = true;
            return Ok(Expr { ty: Ty::Complex, kind: Kind::CallParam(i, Box::new(a)) });
        }
        if param {
            return err(line, format!("unknown function parameter '@{name}'"));
        }
        if self.dialect == Dialect::Frm && matches!(name, "fn1" | "fn2" | "fn3" | "fn4") {
            return err(line, format!("'{name}' is not bound to a function parameter"));
        }
        let lower = name.to_ascii_lowercase();
        if lower == "length"
            && let [ast::Expr::Ident(a)] = args
            && let Some((_, dims)) = self.arrays.get(&self.norm(a))
        {
            return Ok(Expr { ty: Ty::Int, kind: Kind::Local(dims[0]) });
        }
        let lname = if self.dialect == Dialect::Frm { name.to_string() } else { lower.clone() };
        let mut vals = Vec::new();
        for a in args {
            let v = self.expr(a, line)?;
            vals.push(self.numeric(v));
        }
        if self.dialect == Dialect::Ufm
            && let Some(e) = self.color_call(&lname, &mut vals, line)?
        {
            return Ok(e);
        }
        // isNaN / isInf of a real value (complex: the real part); "infinite" is beyond the f32
        // range so the CPU and the GPU agree.
        if self.dialect == Dialect::Ufm && matches!(lname.as_str(), "isnan" | "isinf") && vals.len() == 1 {
            let a = conv(vals.pop().unwrap(), Ty::Real);
            let k = if lname == "isnan" {
                Kind::Cmp(Cmp::Ne, Box::new(a.clone()), Box::new(a))
            } else {
                let abs = Expr { ty: Ty::Real, kind: Kind::Call(Func::Abs, vec![a]) };
                Kind::Cmp(Cmp::Gt, Box::new(abs), Box::new(real(3.4e38)))
            };
            return Ok(Expr { ty: Ty::Bool, kind: k });
        }
        let two_real = |f: Func, vals: Vec<Expr>| -> Result<Expr, Error> {
            if vals.len() != 2 {
                return err(line, format!("'{name}' takes two arguments"));
            }
            let mut it = vals.into_iter();
            let a = conv(it.next().unwrap(), Ty::Real);
            let b = conv(it.next().unwrap(), Ty::Real);
            Ok(Expr { ty: Ty::Real, kind: Kind::Call(f, vec![a, b]) })
        };
        match lname.as_str() {
            "atan2" if vals.len() == 2 => {
                self.transcendental = true;
                return two_real(Func::Atan2, vals);
            }
            "atan2" if vals.len() == 1 && self.dialect == Dialect::Ufm => {
                self.transcendental = true;
                let a = conv(vals.pop().unwrap(), Ty::Complex);
                return Ok(Expr { ty: Ty::Real, kind: Kind::Call(Func::Arg, vec![a]) });
            }
            "min" if self.dialect == Dialect::Ufm => return two_real(Func::Min, vals),
            "max" if self.dialect == Dialect::Ufm => return two_real(Func::Max, vals),
            _ => {}
        }
        let Some(f) = Func::by_name(&lname).filter(|_| self.dialect == Dialect::Ufm || lname == lower) else {
            return err(line, format!("unknown function '{name}'"));
        };
        if vals.len() != 1 {
            return err(line, format!("'{name}' takes one argument"));
        }
        if f.transcendental() {
            self.transcendental = true;
        }
        let a = vals.pop().unwrap();
        if f.returns_real() {
            return Ok(Expr { ty: Ty::Real, kind: Kind::Call(f, vec![conv(a, Ty::Complex)]) });
        }
        if self.dialect == Dialect::Ufm && a.ty != Ty::Complex {
            // Real overloads.
            let a = conv(a, Ty::Real);
            if f.real_closed() {
                return Ok(Expr { ty: Ty::Real, kind: Kind::Call(f, vec![a]) });
            }
            // Functions that leave the real axis (sqrt, log of negative values, flip) give
            // complex results.
            return Ok(Expr { ty: Ty::Complex, kind: Kind::Call(f, vec![conv(a, Ty::Complex)]) });
        }
        Ok(Expr { ty: Ty::Complex, kind: Kind::Call(f, vec![conv(a, Ty::Complex)]) })
    }

    /// Color constructors, channels and mixing functions; `None` for other names.
    fn color_call(&mut self, name: &str, vals: &mut Vec<Expr>, line: usize) -> Result<Option<Expr>, Error> {
        let count = vals.len();
        let first_is_color = vals.first().is_some_and(|v| v.ty == Ty::Color);
        let want = |n: usize| -> Result<(), Error> { if count != n { err(line, format!("'{name}' takes {n} argument(s)")) } else { Ok(()) } };
        let color = |op: ColorOp, args: Vec<Expr>| Some(Expr { ty: Ty::Color, kind: Kind::ColorOp(op, args) });
        let channel = |op: ColorOp, args: Vec<Expr>| Some(Expr { ty: Ty::Real, kind: Kind::ColorOp(op, args) });
        let mut take = std::mem::take(vals).into_iter();
        let mut next = |ty: Ty| conv(take.next().unwrap(), ty);
        Ok(match name {
            "rgb" | "rgba" | "hsl" | "hsla" => {
                let n = if name.ends_with('a') { 4 } else { 3 };
                want(n)?;
                let mut a: Vec<Expr> = (0..n).map(|_| next(Ty::Real)).collect();
                if n == 3 {
                    a.push(real(1.0));
                }
                color(if name.starts_with("hsl") { ColorOp::Hsla } else { ColorOp::Rgba }, a)
            }
            "red" | "green" | "blue" | "alpha" | "hue" | "sat" | "lum" if count == 1 && first_is_color => {
                let op = match name {
                    "red" => ColorOp::Red,
                    "green" => ColorOp::Green,
                    "blue" => ColorOp::Blue,
                    "alpha" => ColorOp::Alpha,
                    "hue" => ColorOp::Hue,
                    "sat" => ColorOp::Sat,
                    _ => ColorOp::Lum,
                };
                channel(op, vec![next(Ty::Color)])
            }
            "blend" | "compose" => {
                want(3)?;
                let a = vec![next(Ty::Color), next(Ty::Color), next(Ty::Real)];
                color(if name == "blend" { ColorOp::Blend } else { ColorOp::Compose }, a)
            }
            "gradient" => {
                want(1)?;
                color(ColorOp::Gradient, vec![next(Ty::Real)])
            }
            _ => match super::color::merge_mode(name) {
                Some(m) => {
                    want(2)?;
                    color(ColorOp::Merge(m), vec![next(Ty::Color), next(Ty::Color)])
                }
                None => {
                    *vals = take.collect();
                    None
                }
            },
        })
    }

    fn assign(&mut self, name: &str, value: Expr, line: usize) -> Result<Stmt, Error> {
        let key = self.norm(name);
        if key == "z" {
            return Ok(Stmt::Assign(Z, conv(value, Ty::Complex)));
        }
        if self.dialect == Dialect::Frm {
            let reserved = ["pixel", "c", "zPrev", "LastSqr", "maxit", "ismand", "pi", "e", "p1", "p2", "p3", "p4", "p5"];
            if reserved.contains(&name) || self.param_index(name).is_some() {
                return err(line, format!("'{name}' cannot be assigned"));
            }
        }
        let i = self.local(name, value.ty);
        let ty = self.locals[i].ty;
        if value.ty > ty && !self.declared.contains(&key) {
            let w = self.widen.entry(key).or_insert(value.ty);
            *w = (*w).max(value.ty);
        }
        Ok(Stmt::Assign(i, conv(value, ty)))
    }

    fn stmts(&mut self, list: &[ast::Stmt]) -> Result<Vec<Stmt>, Error> {
        let mut out = Vec::new();
        for s in list {
            match s {
                ast::Stmt::Assign { target, value, line } => {
                    let v = self.expr(value, *line)?;
                    match target {
                        Target::Var(name) => out.push(self.assign(name, v, *line)?),
                        Target::Sys(name) => {
                            let lower = name.to_ascii_lowercase();
                            match (self.kind, lower.as_str()) {
                                (ProgramKind::Fractal, "z") | (ProgramKind::Transform, "pixel") => out.push(Stmt::Assign(Z, conv(v, Ty::Complex))),
                                (ProgramKind::Coloring, "index") => {
                                    out.push(Stmt::Assign(self.index_slot.unwrap_or(0), conv(self.numeric(v), Ty::Real)))
                                }
                                (ProgramKind::Coloring | ProgramKind::Transform, "solid") => {
                                    out.push(Stmt::Assign(self.solid_slot.unwrap_or(0), conv(v, Ty::Bool)))
                                }
                                (ProgramKind::Coloring, "color") => {
                                    self.direct = true;
                                    out.push(Stmt::Assign(self.color_slot.unwrap_or(0), conv(v, Ty::Color)))
                                }
                                _ => return err(*line, format!("'#{name}' cannot be assigned here")),
                            }
                        }
                        Target::Index(name, idx) => {
                            let (slot, i) = self.array_index(name, idx, *line)?;
                            let t = self.locals[slot].ty;
                            let v = if t == Ty::Bool { v } else { self.numeric(v) };
                            out.push(Stmt::SetIndex(slot, i, conv(v, t)));
                        }
                        Target::Comp { re, var } => {
                            let key = self.norm(var);
                            let i = if key == "z" { Z } else { self.local(var, Ty::Complex) };
                            if self.locals[i].ty != Ty::Complex && self.preset.get(&key) != Some(&Ty::Complex) {
                                self.widen.insert(key, Ty::Complex);
                            }
                            out.push(Stmt::SetPart { local: i, re: *re, value: conv(self.numeric(v), Ty::Real) });
                        }
                    }
                }
                ast::Stmt::Decl { ty, name, value, line } => {
                    let t = match ty {
                        DeclTy::Bool => Ty::Bool,
                        DeclTy::Int => Ty::Int,
                        DeclTy::Float => Ty::Real,
                        DeclTy::Complex => Ty::Complex,
                        DeclTy::Color => Ty::Color,
                    };
                    let key = self.norm(name);
                    if key == "z" {
                        if let Some(v) = value {
                            let v = self.expr(v, *line)?;
                            out.push(Stmt::Assign(Z, conv(v, Ty::Complex)));
                        }
                        continue;
                    }
                    self.preset.entry(key.clone()).or_insert(t);
                    if self.dialect == Dialect::Ufm {
                        self.declared.insert(key);
                    }
                    let i = self.local(name, t);
                    if let Some(v) = value {
                        let v = self.expr(v, *line)?;
                        let lt = self.locals[i].ty;
                        out.push(Stmt::Assign(i, conv(v, lt)));
                    }
                }
                ast::Stmt::If { arms, els, line } => {
                    let mut a = Vec::new();
                    for (c, b) in arms {
                        let c = {
                            let v = self.expr(c, *line)?;
                            self.cond(v)
                        };
                        a.push((c, self.stmts(b)?));
                    }
                    let e = self.stmts(els)?;
                    out.push(Stmt::If(a, e));
                }
                ast::Stmt::While { cond, body, line } => {
                    let c = {
                        let v = self.expr(cond, *line)?;
                        self.cond(v)
                    };
                    out.push(Stmt::While(c, self.stmts(body)?));
                }
                ast::Stmt::Seq(list) => out.extend(self.stmts(list)?),
                ast::Stmt::ArrayDecl { ty, name, dims, line } => self.declare_array(*ty, name, dims, *line, &mut out)?,
                ast::Stmt::Call { name, args, line } => match (name.to_ascii_lowercase().as_str(), args.as_slice()) {
                    ("setlength", [ast::Expr::Ident(a), n]) => {
                        let Some((_, dims)) = self.arrays.get(&self.norm(a)).cloned() else { return err(*line, format!("'{a}' is not an array")) };
                        let e = self.expr(n, *line)?;
                        out.push(Stmt::Assign(dims[0], conv(self.numeric(e), Ty::Int)));
                    }
                    // Debug output of the original program: nothing to show here.
                    ("print", _) => {}
                    _ => return err(*line, format!("unknown procedure '{name}'")),
                },
                ast::Stmt::Repeat { body, cond, line } => {
                    let b = self.stmts(body)?;
                    let c = {
                        let v = self.expr(cond, *line)?;
                        self.cond(v)
                    };
                    out.push(Stmt::Repeat(b, c));
                }
            }
        }
        Ok(out)
    }
}

/// Parameter type of a parameter kind.
pub fn param_ty(kind: ParamKind) -> Ty {
    match kind {
        ParamKind::Bool => Ty::Bool,
        ParamKind::Int => Ty::Int,
        ParamKind::Float => Ty::Real,
        ParamKind::Complex | ParamKind::Func => Ty::Complex,
        ParamKind::Color => Ty::Color,
    }
}

/// A color used as a number somewhere in `stmts` (conversions, arithmetic, numeric functions)?
fn color_misuse(stmts: &[Stmt]) -> Option<usize> {
    for s in stmts {
        let bad = match s {
            Stmt::Assign(_, e) | Stmt::SetPart { value: e, .. } => color_misuse_expr(e),
            Stmt::SetIndex(_, i, e) => color_misuse_expr(i) || color_misuse_expr(e),
            Stmt::If(arms, els) => arms.iter().any(|(c, b)| color_misuse_expr(c) || color_misuse(b).is_some()) || color_misuse(els).is_some(),
            Stmt::While(c, b) | Stmt::Repeat(b, c) => color_misuse_expr(c) || color_misuse(b).is_some(),
        };
        if bad {
            return Some(0);
        }
    }
    None
}

fn color_misuse_expr(e: &Expr) -> bool {
    let colored = |x: &Expr| x.ty == Ty::Color;
    match &e.kind {
        Kind::Conv(a) => e.ty == Ty::Color || a.ty == Ty::Color || color_misuse_expr(a),
        Kind::ColorOp(_, args) => args.iter().any(color_misuse_expr),
        Kind::Index(_, i) => color_misuse_expr(i),
        Kind::Complex(a, b) | Kind::Arith(_, a, b) | Kind::Pow(a, b) | Kind::PowN(a, b) | Kind::Cmp(_, a, b) | Kind::And(a, b) | Kind::Or(a, b) => {
            colored(a) || colored(b) || color_misuse_expr(a) || color_misuse_expr(b)
        }
        Kind::Neg(a) | Kind::Not(a) | Kind::PowI(a, _) | Kind::CallParam(_, a) | Kind::RandomInt(a) => colored(a) || color_misuse_expr(a),
        Kind::Call(_, args) => args.iter().any(|a| colored(a) || color_misuse_expr(a)),
        Kind::Bool(_) | Kind::Int(_) | Kind::Real(_) | Kind::Local(_) | Kind::Param(_) | Kind::Sys(_) => false,
    }
}

/// Checks a parsed fractal formula and lowers it to the typed IR.
pub fn check(ast: &FormulaAst) -> Result<Program, Error> {
    check_kind(ast, ProgramKind::Fractal)
}

/// Checks a parsed formula of the given kind.
pub fn check_kind(ast: &FormulaAst, kind: ProgramKind) -> Result<Program, Error> {
    let mut preset = HashMap::new();
    for _round in 0..16 {
        let mut ck = Ck {
            ast,
            dialect: ast.dialect,
            kind,
            index_slot: None,
            solid_slot: None,
            color_slot: None,
            direct: false,
            declared: Default::default(),
            params: Vec::new(),
            locals: vec![Local { name: "z".into(), ty: Ty::Complex, cap: None }],
            slots: HashMap::new(),
            preset: preset.clone(),
            widen: HashMap::new(),
            classic: HashMap::new(),
            has_julia: false,
            transcendental: false,
            arrays: HashMap::new(),
        };
        ck.slots.insert("z".into(), Z);
        if kind == ProgramKind::Coloring {
            ck.locals.push(Local { name: "#index".into(), ty: Ty::Real, cap: None });
            ck.index_slot = Some(ck.locals.len() - 1);
        }
        if kind != ProgramKind::Fractal {
            ck.locals.push(Local { name: "#solid".into(), ty: Ty::Bool, cap: None });
            ck.solid_slot = Some(ck.locals.len() - 1);
        }
        if kind == ProgramKind::Coloring {
            ck.locals.push(Local { name: "#color".into(), ty: Ty::Color, cap: None });
            ck.color_slot = Some(ck.locals.len() - 1);
        }
        for p in &ast.params {
            if ck.param_index(&p.name).is_some() {
                // UFM files repeat declarations now and then; the first one counts.
                if ast.dialect == Dialect::Ufm {
                    continue;
                }
                return err(0, format!("parameter '{}' is declared twice", p.name));
            }
            if p.kind == ParamKind::Func {
                match &p.default {
                    ast::ParamValue::Func(f) if Func::by_name(f).is_some() => {}
                    other => return err(0, format!("parameter '{}': unknown default function {other:?}", p.name)),
                }
            }
            ck.params.push(Param {
                name: p.name.clone(),
                kind: p.kind,
                ty: param_ty(p.kind),
                default: p.default.clone(),
                caption: p.caption.clone(),
                hint: p.hint.clone(),
                min: p.min,
                max: p.max,
                items: p.items.clone(),
            });
            if let Some(slot) = &p.classic {
                ck.classic.insert(slot.clone(), ck.params.len() - 1);
            }
        }
        let mut init = ck.shadow_params();
        init.extend(ck.stmts(&ast.global)?);
        init.extend(ck.stmts(&ast.init)?);
        let body = if kind == ProgramKind::Transform { ck.stmts(&ast.transform)? } else { ck.stmts(&ast.body)? };
        let bailout = match (&ast.bailout, kind) {
            (Some(b), ProgramKind::Fractal) => {
                let e = ck.expr(b, 0)?;
                ck.cond(e)
            }
            (None, ProgramKind::Fractal) => return err(0, "the formula has no bailout condition"),
            _ => Expr { ty: Ty::Bool, kind: Kind::Bool(true) },
        };
        let fin = ck.stmts(&ast.fin)?;
        if let Some(line) = [&init, &body, &fin].iter().find_map(|b| color_misuse(b)).or(color_misuse_expr(&bailout).then_some(0)) {
            let _ = line;
            return err(0, "colors can only be used with color functions (rgb, blend, compose, red, …), not as numbers");
        }
        if !ck.widen.is_empty() {
            let mut changed = false;
            for (k, t) in ck.widen {
                if preset.get(&k).is_none_or(|&p| p < t) {
                    preset.insert(k, t);
                    changed = true;
                }
            }
            if changed {
                continue;
            }
        }
        return Ok(Program {
            name: ast.name.clone(),
            kind,
            index_slot: ck.index_slot,
            solid_slot: ck.solid_slot,
            color_slot: ck.color_slot,
            direct: ck.direct,
            params: ck.params,
            locals: ck.locals,
            init,
            body,
            bailout,
            fin,
            has_julia: ck.has_julia,
            transcendental: ck.transcendental,
        });
    }
    err(0, "the variable types do not settle (assignments of different types)")
}

#[cfg(test)]
mod tests {
    use super::super::parse::{parse_frm, parse_ufm_entry, split_entries};
    use super::*;

    #[test]
    fn widening_and_types() {
        let src = "T {\ninit:\n  k = 1\n  q = 0\nloop:\n  k = k + 0.5\n  q = z\n  z = z^2 + #pixel\nbailout:\n  |z| < 4\n}";
        let e = split_entries(src);
        let p = check(&parse_ufm_entry(&e[0]).unwrap()).unwrap();
        let ty = |n: &str| p.locals.iter().find(|l| l.name == n).unwrap().ty;
        assert_eq!(ty("k"), Ty::Real);
        assert_eq!(ty("q"), Ty::Complex);
        assert!(matches!(p.body[2], Stmt::Assign(Z, Expr { kind: Kind::Arith(Arith::Add, ref a, _), .. }) if matches!(a.kind, Kind::PowI(_, 2))));
    }

    #[test]
    fn frm_rejects_assigning_inputs() {
        let src = "; @language: frm-like/1\n; @stdlib: 1\n; @numeric-profile: standard32\nX {\n  init:\n    pixel = 0\n  loop:\n    z = z\n  bailout:\n    |z| < 4\n}\n";
        assert!(check(&parse_frm(src).unwrap()).is_err());
    }
}
