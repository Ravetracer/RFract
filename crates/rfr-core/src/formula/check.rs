// SPDX-License-Identifier: LGPL-3.0-or-later

//! Name resolution and type checking: AST → typed IR. Locals take the type of their first
//! assignment (or declaration); when a later assignment needs a wider type (int → real →
//! complex), the formula is checked again with the wider type from the start.

use std::collections::HashMap;

use super::ast::{self, BinOp, DeclTy, Dialect, FormulaAst, ParamKind, Target, UnOp};
use super::ir::*;
use super::parse::{Error, err};

struct Ck<'a> {
    ast: &'a FormulaAst,
    dialect: Dialect,
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
        self.locals.push(Local { name: key.clone(), ty });
        self.slots.insert(key, self.locals.len() - 1);
        self.locals.len() - 1
    }

    /// Value of a plain identifier.
    fn ident(&mut self, name: &str, line: usize) -> Result<Expr, Error> {
        let key = self.norm(name);
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
        err(line, format!("unknown name '{name}'"))
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
                ast::Stmt::While { body, .. } | ast::Stmt::Repeat { body, .. } => walk(body, key, ufm),
                _ => false,
            })
        }
        let ufm = self.dialect == Dialect::Ufm;
        let a = self.ast;
        [&a.global, &a.init, &a.body, &a.fin].iter().any(|b| walk(b, key, ufm))
    }

    fn ufm_sys(&mut self, name: &str, line: usize) -> Result<Expr, Error> {
        let s = match name.to_ascii_lowercase().as_str() {
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
            ast::Expr::Param(name) => match self.param_index(name) {
                Some(i) if self.params[i].kind == ParamKind::Func => {
                    return err(line, format!("the function parameter '@{name}' can only be called"));
                }
                Some(i) => Expr { ty: self.params[i].ty, kind: Kind::Param(i) },
                None => return err(line, format!("unknown parameter '@{name}'")),
            },
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
        if param {
            return err(line, format!("unknown function parameter '@{name}'"));
        }
        if self.dialect == Dialect::Frm && matches!(name, "fn1" | "fn2" | "fn3" | "fn4") {
            return err(line, format!("'{name}' is not bound to a function parameter"));
        }
        let lower = name.to_ascii_lowercase();
        let lname = if self.dialect == Dialect::Frm { name.to_string() } else { lower.clone() };
        let mut vals = Vec::new();
        for a in args {
            let v = self.expr(a, line)?;
            vals.push(self.numeric(v));
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
        if value.ty > ty {
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
                        Target::Sys(name) if name.eq_ignore_ascii_case("z") => out.push(Stmt::Assign(Z, conv(v, Ty::Complex))),
                        Target::Sys(name) => return err(*line, format!("'#{name}' cannot be assigned in a formula")),
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
                    };
                    let key = self.norm(name);
                    if key == "z" {
                        if let Some(v) = value {
                            let v = self.expr(v, *line)?;
                            out.push(Stmt::Assign(Z, conv(v, Ty::Complex)));
                        }
                        continue;
                    }
                    self.preset.entry(key).or_insert(t);
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
    }
}

/// Checks a parsed formula and lowers it to the typed IR.
pub fn check(ast: &FormulaAst) -> Result<Program, Error> {
    let mut preset = HashMap::new();
    for _round in 0..6 {
        let mut ck = Ck {
            ast,
            dialect: ast.dialect,
            params: Vec::new(),
            locals: vec![Local { name: "z".into(), ty: Ty::Complex }],
            slots: HashMap::new(),
            preset: preset.clone(),
            widen: HashMap::new(),
            classic: HashMap::new(),
            has_julia: false,
            transcendental: false,
        };
        ck.slots.insert("z".into(), Z);
        for p in &ast.params {
            if ck.param_index(&p.name).is_some() {
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
        let mut init = ck.stmts(&ast.global)?;
        init.extend(ck.stmts(&ast.init)?);
        let body = ck.stmts(&ast.body)?;
        let bailout = match &ast.bailout {
            Some(b) => {
                let e = ck.expr(b, 0)?;
                ck.cond(e)
            }
            None => return err(0, "the formula has no bailout condition"),
        };
        let fin = ck.stmts(&ast.fin)?;
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
