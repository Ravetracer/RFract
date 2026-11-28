// SPDX-License-Identifier: LGPL-3.0-or-later

//! Name resolution and type checking: AST → typed IR. Locals take the type of their first
//! assignment (or declaration); when a later assignment needs a wider type (int → real →
//! complex), the formula is checked again with the wider type from the start.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

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
    /// Classes for formulas that use objects.
    classes: Option<&'a ClassCtx<'a>>,
    /// Objects made so far (each `new` is one object; its fields are locals `o<n>.<field>`).
    instances: Vec<Inst>,
    /// Inlined method calls, innermost last.
    scopes: Vec<Scope>,
    /// Object variables (by key) and the object each one refers to (`None`: null).
    objects: HashMap<String, OVal>,
    obj_vars: HashSet<String>,
    /// Arrays of objects (by key) and their lengths; element `k` is the object variable
    /// `<key>[k]` in `objects`.
    obj_arrays: HashMap<String, usize>,
    /// Counters of unrolled loops and their values in the iteration being checked.
    unroll: HashMap<String, i64>,
    /// Upper bounds of int locals whose values are known only when rendering (method
    /// arguments from parameters with a maximum).
    bounds: HashMap<String, i64>,
    /// Object arrays sized when rendering: the local holding their length (the array has
    /// room for the bound).
    obj_len: HashMap<String, usize>,
    /// Statements an expression needs before it (inlined method bodies).
    pending: Vec<Stmt>,
    /// Counter of inlined calls (unique local names).
    site: usize,
    /// The formula's own functions as the methods of a class without objects.
    formula_class: Option<Arc<ast::ClassAst>>,
}

/// Classes for checking formulas that use objects: the class lookup (case-insensitive name)
/// and the classes chosen for object parameters (`<param path>` → `ParamValue::Class`).
pub struct ClassCtx<'a> {
    pub lookup: &'a dyn Fn(&str) -> Option<Arc<ast::ClassAst>>,
    pub choices: &'a BTreeMap<String, ast::ParamValue>,
}

/// An object: its class and the prefix of its parameters (`formulaClass.`).
struct Inst {
    class: Arc<ast::ClassAst>,
    prefix: String,
    /// Made from an object parameter: its parameters are user parameters; otherwise they keep
    /// their defaults.
    plugin: bool,
    /// A built-in image object: the image parameter it reads (`Some(None)`: no image).
    image: Option<Option<usize>>,
}

/// An inlined method call.
struct Scope {
    /// Lowercase names of parameters, locals and fields → keys.
    names: HashMap<String, String>,
    this: Option<usize>,
    /// The class whose method runs (for `Parent.Method` calls) and the object's parameters.
    class: Option<Arc<ast::ClassAst>>,
    prefix: String,
    plugin: bool,
    ret: Option<(usize, Ty)>,
    /// The object variable that receives returned objects.
    ret_obj: Option<String>,
    done: Option<usize>,
}

const MAX_INLINE_DEPTH: usize = 40;
/// Iterations a loop over objects may be unrolled to.
const MAX_UNROLL: usize = 256;

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
        let n = if self.dialect == Dialect::Ufm { name.to_ascii_lowercase() } else { name.to_string() };
        match self.scopes.last().and_then(|s| s.names.get(&n)) {
            Some(k) => k.clone(),
            None => n,
        }
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
            base: None,
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

    /// `target = src` copies an array: `src` is one, and the target is either an array or a
    /// new one with sizes (`complex b[n] = a`).
    fn is_array_copy(&self, target: &Target, src: &str) -> bool {
        if !self.arrays.contains_key(&self.norm(src)) {
            return false;
        }
        match target {
            Target::Var(n) => self.arrays.contains_key(&self.norm(n)),
            Target::Index(n, _) => !self.arrays.contains_key(&self.norm(n)),
            _ => false,
        }
    }

    /// Copies the elements of array `src` into array `dst` (keys), element by element.
    fn copy_array(&mut self, dst: &str, src: &str, out: &mut Vec<Stmt>) {
        let (ds, _) = self.arrays[dst].clone();
        let (ss, sdims) = self.arrays[src].clone();
        let int = |kind| Expr { ty: Ty::Int, kind };
        let n = sdims
            .iter()
            .map(|&d| int(Kind::Local(d)))
            .reduce(|a, b| int(Kind::Arith(Arith::Mul, Box::new(a), Box::new(b))))
            .unwrap_or(int(Kind::Int(0)));
        self.site += 1;
        let k = format!("~{}:~copy", self.site);
        self.preset.insert(k.clone(), Ty::Int);
        let k = self.key_local(&k, Ty::Int);
        let kv = || int(Kind::Local(k));
        let elem = Expr { ty: self.locals[ss].ty, kind: Kind::Index(ss, Box::new(kv())) };
        let t = self.locals[ds].ty;
        let elem = if t == Ty::Bool { elem } else { self.numeric(elem) };
        out.push(Stmt::Assign(k, int(Kind::Int(0))));
        out.push(Stmt::While(
            Expr { ty: Ty::Bool, kind: Kind::Cmp(Cmp::Lt, Box::new(kv()), Box::new(n)) },
            vec![Stmt::SetIndex(ds, kv(), conv(elem, t)), Stmt::Assign(k, int(Kind::Arith(Arith::Add, Box::new(kv()), Box::new(int(Kind::Int(1))))))],
        ));
    }

    /// Flat element index of `name[idx…]` and the array's slot.
    fn array_index(&mut self, name: &str, idx: &[ast::Expr], line: usize) -> Result<(usize, Expr), Error> {
        let key = self.norm(name);
        self.array_index_key(&key, name, idx, line)
    }

    fn array_index_key(&mut self, key: &str, name: &str, idx: &[ast::Expr], line: usize) -> Result<(usize, Expr), Error> {
        let key = key.to_string();
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

    /// Whether a name is the complex `z` of 2D formulas (3D objects have real `x`, `y`, `z`).
    fn is_z(&self, key: &str) -> bool {
        key == "z" && self.kind != ProgramKind::Object
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
        if let Some(&v) = self.unroll.get(&key) {
            return Ok(Expr { ty: Ty::Int, kind: Kind::Int(v) });
        }
        // An object as a value: whether it is not null.
        if self.obj_vars.contains(&key) {
            return Ok(truth(&self.oval(&key).alts()));
        }
        if self.is_z(&key) {
            return Ok(Expr { ty: Ty::Complex, kind: Kind::Local(Z) });
        }
        if let Some(&i) = self.slots.get(&key) {
            return Ok(Expr { ty: self.locals[i].ty, kind: Kind::Local(i) });
        }
        // Fields and method locals read before they are set start at zero.
        if self.scopes.last().is_some_and(|s| s.names.values().any(|v| *v == key)) {
            if self.obj_vars.contains(&key) {
                return err(line, format!("the object '{name}' cannot be used as a value"));
            }
            let i = self.key_local(&key, Ty::Complex);
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
                out.extend(inner_assigned(s).into_iter().filter_map(|v| v.strip_prefix('@').map(str::to_string)));
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
            } || inner_assigned(s).iter().any(|v| if ufm { v.eq_ignore_ascii_case(key) } else { v == key }))
        }
        let ufm = self.dialect == Dialect::Ufm;
        let a = self.ast;
        [&a.global, &a.init, &a.body, &a.fin, &a.transform].iter().any(|b| walk(b, key, ufm))
    }

    // ---- objects ----

    fn class(&self, name: &str, line: usize) -> Result<Arc<ast::ClassAst>, Error> {
        let Some(ctx) = self.classes else { return err(line, format!("the class '{name}' is not available (no class libraries)")) };
        (ctx.lookup)(name).ok_or_else(|| Error { line, msg: format!("unknown class '{name}' (is its .ulb file in a formula folder?)") })
    }

    /// The class and its ancestors, the root first.
    fn chain(&self, c: &Arc<ast::ClassAst>, line: usize) -> Result<Vec<Arc<ast::ClassAst>>, Error> {
        let mut out = vec![c.clone()];
        let mut cur = c.clone();
        while let Some(p) = cur.parent.clone() {
            if out.len() > 32 || out.iter().any(|x| x.name.eq_ignore_ascii_case(&p)) {
                return err(line, format!("the classes above '{}' form a loop", c.name));
            }
            cur = self.class(&p, line)?;
            out.push(cur.clone());
        }
        out.reverse();
        Ok(out)
    }

    /// The implementation of method `name` for class `c` (searched upwards).
    fn find_method(&self, c: &Arc<ast::ClassAst>, name: &str, line: usize) -> Result<Option<(Arc<ast::ClassAst>, ast::MethodAst)>, Error> {
        for k in self.chain(c, line)?.into_iter().rev() {
            if let Some(m) = k.methods.iter().find(|m| m.name.eq_ignore_ascii_case(name)) {
                return Ok(Some((k.clone(), m.clone())));
            }
        }
        Ok(None)
    }

    /// A local by key (bypassing name mapping).
    fn key_local(&mut self, key: &str, ty: Ty) -> usize {
        if let Some(&i) = self.slots.get(key) {
            return i;
        }
        let ty = self.preset.get(key).copied().map_or(ty, |p| p.max(ty));
        self.locals.push(Local { name: key.to_string(), ty, cap: None });
        self.slots.insert(key.to_string(), self.locals.len() - 1);
        self.locals.len() - 1
    }

    /// Makes an object of class `name` (or of the class chosen by object parameter `@name`)
    /// and runs its constructor.
    fn instantiate(&mut self, name: &str, param: bool, args: &[ast::Expr], line: usize) -> Result<Option<usize>, Error> {
        let (prefix, plugin) = match self.scopes.last() {
            Some(s) => (s.prefix.clone(), s.plugin),
            None => (String::new(), true),
        };
        if param && let Some(image) = self.image_param(name) {
            // `new @image`: the built-in image class; its methods are image operations.
            let class = Arc::new(ast::ClassAst {
                name: "Image".into(),
                parent: None,
                fields: Vec::new(),
                methods: Vec::new(),
                params: Vec::new(),
                title: None,
                line: 0,
            });
            let id = self.instances.len();
            self.instances.push(Inst { class, prefix: format!("{prefix}~{id}."), plugin: false, image: Some(image) });
            return Ok(Some(id));
        }
        let (class, new_prefix, new_plugin) = if param {
            let full = format!("{prefix}{name}");
            let i = self.param_index(&full).ok_or_else(|| Error { line, msg: format!("unknown object parameter '@{name}'") })?;
            let chosen = match self.classes.and_then(|c| c.choices.get(&full)) {
                Some(ast::ParamValue::Class { class }) if !class.is_empty() && plugin => class.clone(),
                _ => match &self.params[i].default {
                    ast::ParamValue::Class { class } => class.clone(),
                    _ => return err(line, format!("'@{name}' is not an object parameter")),
                },
            };
            (self.class(&chosen, line)?, format!("{full}."), plugin)
        } else {
            let c = self.class(name, line)?;
            let n = self.instances.len();
            (c, format!("{prefix}~{n}."), false)
        };
        let id = self.instances.len();
        self.instances.push(Inst { class: class.clone(), prefix: new_prefix.clone(), plugin: new_plugin, image: None });
        let chain = self.chain(&class, line)?;
        // Fields start at zero; object fields are null.
        let mut array_fields = Vec::new();
        for c in &chain {
            for f in &c.fields {
                let key = field_key(id, &f.name);
                match &f.ty {
                    ast::TypeRef::Class(_) => {
                        if let Some(d) = &f.dims {
                            let n = self.obj_array_size(&f.name, d, line)?;
                            self.declare_obj_array(key, n);
                            continue;
                        }
                        self.obj_vars.insert(key.clone());
                        self.objects.insert(key, OVal::One(None));
                    }
                    ast::TypeRef::Builtin(t) => {
                        let ty = match t {
                            DeclTy::Bool => Ty::Bool,
                            DeclTy::Int => Ty::Int,
                            DeclTy::Float => Ty::Real,
                            DeclTy::Complex => Ty::Complex,
                            DeclTy::Color => Ty::Color,
                        };
                        if let Some(d) = &f.dims {
                            array_fields.push((*t, f.name.clone(), d.clone()));
                        } else {
                            self.preset.entry(key.clone()).or_insert(ty);
                            self.declared.insert(key);
                        }
                    }
                }
            }
        }
        // Parameters of a plugin object are user parameters under its prefix.
        if new_plugin {
            for c in &chain {
                for p in &c.params {
                    let full = format!("{new_prefix}{}", p.name);
                    if self.param_index(&full).is_some() {
                        continue;
                    }
                    self.params.push(Param {
                        name: full,
                        kind: p.kind,
                        ty: param_ty(p.kind),
                        default: p.default.clone(),
                        caption: p.caption.clone(),
                        hint: p.hint.clone(),
                        min: p.min,
                        max: p.max,
                        items: p.items.clone(),
                        base: p.base.clone(),
                    });
                }
            }
        }
        // Array fields, declared in the object's scope.
        if !array_fields.is_empty() {
            let names = array_fields.iter().map(|(_, n, _)| (n.to_ascii_lowercase(), field_key(id, n))).collect();
            self.scopes.push(Scope {
                names,
                this: Some(id),
                class: Some(class.clone()),
                prefix: new_prefix.clone(),
                plugin: new_plugin,
                ret: None,
                ret_obj: None,
                done: None,
            });
            let mut out = Vec::new();
            let r = array_fields.iter().try_for_each(|(t, n, d)| self.declare_array(*t, n, d, line, &mut out));
            self.scopes.pop();
            r?;
            self.pending.extend(out);
        }
        // The constructor: the nearest class (upwards) with a method named like itself.
        let ctor = chain.iter().rev().find_map(|c| c.methods.iter().find(|m| m.name.eq_ignore_ascii_case(&c.name)).map(|m| (c.clone(), m.clone())));
        if let Some((c, m)) = ctor {
            self.inline(Some(id), &c, &m, args, line)?;
        }
        Ok(Some(id))
    }

    /// Value of an integer known when checking: numbers, counters of unrolled loops, lengths
    /// of object arrays, and arithmetic on them.
    fn static_int(&self, e: &ast::Expr) -> Option<i64> {
        Some(match e {
            ast::Expr::Num { v, .. } if v.fract() == 0.0 && v.abs() < 9.0e15 => *v as i64,
            ast::Expr::Ident(n) => *self.unroll.get(&self.norm(n))?,
            ast::Expr::Unary(UnOp::Neg, a) => -self.static_int(a)?,
            ast::Expr::Binary(op, a, b) => {
                let (x, y) = (self.static_int(a)?, self.static_int(b)?);
                match op {
                    BinOp::Add => x.checked_add(y)?,
                    BinOp::Sub => x.checked_sub(y)?,
                    BinOp::Mul => x.checked_mul(y)?,
                    BinOp::Div if y != 0 => x / y,
                    BinOp::Mod if y != 0 => x % y,
                    _ => return None,
                }
            }
            ast::Expr::Call { name, param: false, args } if name.eq_ignore_ascii_case("length") && args.len() == 1 => match &args[0] {
                ast::Expr::Ident(a) if !self.obj_len.contains_key(&self.norm(a)) => *self.obj_arrays.get(&self.norm(a))? as i64,
                _ => return None,
            },
            _ => return None,
        })
    }

    /// An upper bound of an int value known only when rendering: parameters with a maximum,
    /// locals with a recorded bound, lengths of object arrays sized when rendering, sums.
    fn bound_int(&self, e: &ast::Expr) -> Option<i64> {
        if let Some(v) = self.static_int(e) {
            return Some(v);
        }
        Some(match e {
            ast::Expr::Param(n) => {
                let prefix = self.scopes.last().map_or(String::new(), |s| s.prefix.clone());
                let i = self.param_index(&format!("{prefix}{n}")).or_else(|| self.param_index(n))?;
                let p = &self.params[i];
                if p.kind != ParamKind::Int {
                    return None;
                }
                p.max?.floor() as i64
            }
            ast::Expr::Ident(n) => *self.bounds.get(&self.norm(n))?,
            ast::Expr::Binary(BinOp::Add, a, b) => self.bound_int(a)?.checked_add(self.bound_int(b)?)?,
            ast::Expr::Binary(BinOp::Sub, a, b) => self.bound_int(a)?.checked_sub(self.static_int(b)?)?,
            ast::Expr::Call { name, param: false, args } if name.eq_ignore_ascii_case("length") && args.len() == 1 => match &args[0] {
                ast::Expr::Ident(a) => *self.obj_arrays.get(&self.norm(a))? as i64,
                _ => return None,
            },
            _ => return None,
        })
    }

    /// Value of a condition on integers known when checking (see `static_int`).
    fn static_cond(&self, e: &ast::Expr) -> Option<bool> {
        Some(match e {
            ast::Expr::Bool(b) => *b,
            ast::Expr::Unary(UnOp::Not, a) => !self.static_cond(a)?,
            ast::Expr::Binary(BinOp::And, a, b) => self.static_cond(a)? && self.static_cond(b)?,
            ast::Expr::Binary(BinOp::Or, a, b) => self.static_cond(a)? || self.static_cond(b)?,
            ast::Expr::Binary(op, a, b) => {
                let (x, y) = (self.static_int(a)?, self.static_int(b)?);
                match op {
                    BinOp::Lt => x < y,
                    BinOp::Le => x <= y,
                    BinOp::Gt => x > y,
                    BinOp::Ge => x >= y,
                    BinOp::Eq => x == y,
                    BinOp::Ne => x != y,
                    _ => return None,
                }
            }
            _ => return None,
        })
    }

    /// Length of an object array declared with sizes `dims` (`[]`: empty until `setLength`).
    fn obj_array_size(&self, name: &str, dims: &[ast::Expr], line: usize) -> Result<usize, Error> {
        match dims {
            [] => Ok(0),
            [d] => match self.static_int(d).map(|v| v as f64).or_else(|| self.const_eval(d)) {
                Some(n) if (0.0..=ARRAY_MAX as f64).contains(&n) => Ok(n as usize),
                Some(n) => err(line, format!("the object array '{name}' has {n} elements; at most {ARRAY_MAX} are supported")),
                None => err(line, format!("the size of the object array '{name}' must be a number")),
            },
            _ => err(line, format!("arrays of objects have one dimension ('{name}')")),
        }
    }

    /// Declares (or resizes) an object array; new elements are null.
    fn declare_obj_array(&mut self, key: String, n: usize) {
        let old = self.obj_arrays.insert(key.clone(), n).unwrap_or(0);
        for k in n..old {
            self.objects.remove(&elem_key(&key, k));
        }
        for k in old..n {
            self.objects.insert(elem_key(&key, k), OVal::One(None));
        }
    }

    /// The object variable `key` (null when it was never set).
    fn oval(&self, key: &str) -> OVal {
        self.objects.get(key).cloned().unwrap_or(OVal::One(None))
    }

    /// A new tag: an int local that tells which candidate an object variable holds.
    fn new_tag(&mut self) -> usize {
        self.site += 1;
        let k = format!("~{}:~tag", self.site);
        self.preset.insert(k.clone(), Ty::Int);
        self.key_local(&k, Ty::Int)
    }

    /// An int value used several times: a local (computed once) unless it is one already.
    fn once(&mut self, v: Expr) -> Expr {
        if matches!(v.kind, Kind::Local(_) | Kind::Int(_)) {
            return v;
        }
        self.site += 1;
        let k = format!("~{}:~index", self.site);
        self.preset.insert(k.clone(), Ty::Int);
        let i = self.key_local(&k, Ty::Int);
        self.pending.push(Stmt::Assign(i, conv(v, Ty::Int)));
        Expr { ty: Ty::Int, kind: Kind::Local(i) }
    }

    /// A value used several times: stored in a local first (statements go to `out`).
    fn value_once(&mut self, v: Expr, out: &mut Vec<Stmt>) -> Expr {
        if matches!(v.kind, Kind::Local(_) | Kind::Int(_) | Kind::Real(_) | Kind::Bool(_)) {
            return v;
        }
        self.site += 1;
        let k = format!("~{}:~value", self.site);
        self.preset.insert(k.clone(), v.ty);
        let i = self.key_local(&k, v.ty);
        out.push(Stmt::Assign(i, v));
        Expr { ty: self.locals[i].ty, kind: Kind::Local(i) }
    }

    /// An object value from alternatives: one object, or a new tag set at run time.
    fn materialize(&mut self, alts: Alts) -> OVal {
        let mut objs: Vec<Option<usize>> = Vec::new();
        for (_, o) in &alts {
            if !objs.contains(o) {
                objs.push(*o);
            }
        }
        if objs.len() <= 1 {
            return OVal::One(objs.first().copied().flatten());
        }
        let t = self.new_tag();
        let mut cands = vec![None];
        cands.extend(objs.into_iter().filter(Option::is_some));
        let pos = |o: Option<usize>| cands.iter().position(|x| *x == o).unwrap_or(0) as i64;
        let arms: Vec<(Expr, Vec<Stmt>)> =
            alts.iter().filter(|(_, o)| o.is_some()).map(|(c, o)| (c.clone(), vec![Stmt::Assign(t, int_e(pos(*o)))])).collect();
        self.pending.push(Stmt::Assign(t, int_e(0)));
        self.pending.push(Stmt::If(arms, Vec::new()));
        OVal::Tag(t, cands)
    }

    /// Statements that set tag `t` (candidates `cands`) to the object `v` holds.
    fn remap(t: usize, cands: &[Option<usize>], v: &OVal) -> Vec<Stmt> {
        let pos = |o: Option<usize>| cands.iter().position(|x| *x == o).unwrap_or(0) as i64;
        match v {
            OVal::One(o) => vec![Stmt::Assign(t, int_e(pos(*o)))],
            OVal::Tag(t2, c2) if *t2 == t && c2 == cands => Vec::new(),
            OVal::Tag(t2, c2) => {
                let arms = c2
                    .iter()
                    .enumerate()
                    .filter(|(_, o)| pos(**o) != 0)
                    .map(|(j, o)| (eq_int(Expr { ty: Ty::Int, kind: Kind::Local(*t2) }, j as i64), vec![Stmt::Assign(t, int_e(pos(*o)))]))
                    .collect();
                vec![Stmt::Assign(t, int_e(0)), Stmt::If(arms, Vec::new())]
            }
        }
    }

    /// Joins the object variables after branches (`ends[k]` after `bodies[k]`; exactly one
    /// branch runs). Variables that differ get a tag, set at the end of every branch.
    fn merge(&mut self, before: &HashMap<String, OVal>, ends: Vec<HashMap<String, OVal>>, mut bodies: Vec<&mut Vec<Stmt>>) {
        let mut keys: Vec<String> = ends.iter().flat_map(|e| e.keys().cloned()).collect();
        keys.sort();
        keys.dedup();
        let mut out = HashMap::new();
        for key in keys {
            let present: Vec<&OVal> = ends.iter().filter_map(|e| e.get(&key)).collect();
            // Made in one branch only (objects and locals of that branch).
            if !before.contains_key(&key) && present.windows(2).all(|w| w[0] == w[1]) {
                out.insert(key, present[0].clone());
                continue;
            }
            let vals: Vec<OVal> = ends.iter().map(|e| e.get(&key).cloned().unwrap_or(OVal::One(None))).collect();
            if vals.windows(2).all(|w| w[0] == w[1]) {
                out.insert(key, vals[0].clone());
                continue;
            }
            let t = self.new_tag();
            let mut cands = vec![None];
            for v in &vals {
                for o in v.cands() {
                    if !cands.contains(&o) {
                        cands.push(o);
                    }
                }
            }
            for (v, b) in vals.iter().zip(bodies.iter_mut()) {
                b.extend(Self::remap(t, &cands, v));
            }
            out.insert(key, OVal::Tag(t, cands));
        }
        self.objects = out;
    }

    /// After a loop body: object variables that held an object before the loop must not
    /// change (the body was checked with the objects before the first iteration).
    fn loop_objects(&self, start: &HashMap<String, OVal>, line: usize) -> Result<(), Error> {
        for (k, v) in start {
            if *v != OVal::One(None) && self.objects.get(k).is_some_and(|now| now != v) {
                let name = k.rsplit(['.', ':']).next().unwrap_or(k);
                return err(line, format!("'{name}' refers to other objects after an iteration of a loop (not supported)"));
            }
        }
        Ok(())
    }

    /// Variable `key` holds `v` when `cond` holds at run time, else what it held.
    fn store_if(&mut self, key: &str, cond: Expr, v: OVal) {
        let old = self.oval(key);
        if const_bool(&cond) == Some(true) {
            self.objects.insert(key.to_string(), v);
            return;
        }
        if old == v || const_bool(&cond) == Some(false) {
            return;
        }
        let t = self.new_tag();
        let mut cands = vec![None];
        for o in old.cands().into_iter().chain(v.cands()) {
            if !cands.contains(&o) {
                cands.push(o);
            }
        }
        let set_old = Self::remap(t, &cands, &old);
        let set_new = Self::remap(t, &cands, &v);
        self.pending.extend(set_old);
        self.pending.push(Stmt::If(vec![(cond, set_new)], Vec::new()));
        self.objects.insert(key.to_string(), OVal::Tag(t, cands));
    }

    /// `name[i]` or `object.field[i]` of an object array: the objects it can refer to and
    /// when. `None` if `e` is no object array.
    fn obj_array_alts(&mut self, e: &ast::Expr, line: usize) -> Result<Option<Alts>, Error> {
        let (bases, name, idx): (Vec<(Expr, String)>, &str, &[ast::Expr]) = match e {
            ast::Expr::Index(name, idx) => (vec![(bool_e(true), self.norm(name))], name, idx),
            ast::Expr::MemberIndex(o, f, idx) => {
                let alts = self.obj_alts(o, line)?;
                let keys: Vec<(Expr, String)> = alts.into_iter().filter_map(|(c, o)| o.map(|i| (c, field_key(i, f)))).collect();
                if keys.is_empty() {
                    return err(line, format!("'.{f}' of a null object"));
                }
                (keys, f, idx)
            }
            _ => return Ok(None),
        };
        if !bases.iter().all(|(_, k)| self.obj_arrays.contains_key(k)) {
            return Ok(None);
        }
        let [ix] = idx else { return err(line, format!("arrays of objects have one dimension ('{name}')")) };
        let mut alts = Vec::new();
        match self.static_int(ix) {
            Some(i) => {
                for (cb, key) in bases {
                    let n = self.obj_arrays[&key];
                    let Some(k) = usize::try_from(i).ok().filter(|&k| k < n) else {
                        return err(line, format!("index {i} is outside of the object array '{name}' ({n} elements)"));
                    };
                    alts.extend(self.oval(&elem_key(&key, k)).alts().into_iter().map(|(c, o)| (and_e(cb.clone(), c), o)));
                }
            }
            None => {
                // The index is computed once; the element is chosen at run time.
                let v = self.expr(ix, line)?;
                let v = conv(self.numeric(v), Ty::Int);
                let sel = self.once(v);
                for (cb, key) in bases {
                    for k in 0..self.obj_arrays[&key] {
                        let ck = and_e(cb.clone(), eq_int(sel.clone(), k as i64));
                        alts.extend(self.oval(&elem_key(&key, k)).alts().into_iter().map(|(c, o)| (and_e(ck.clone(), c), o)));
                    }
                }
            }
        }
        Ok(Some(alts))
    }

    /// The objects an expression can refer to, each with the condition when.
    fn obj_alts(&mut self, e: &ast::Expr, line: usize) -> Result<Alts, Error> {
        match self.obj_array_alts(e, line)? {
            Some(a) => Ok(a),
            None => Ok(self.object_of(e, line)?.alts()),
        }
    }

    /// Runs `each` for every object the alternatives can give, inside `if condition`.
    fn dispatch(&mut self, alts: Alts, line: usize, mut each: impl FnMut(&mut Self, usize) -> Result<Called, Error>) -> Result<Called, Error> {
        let _ = line;
        // One object: no choice.
        if let Some((_, Some(inst))) = alts.first()
            && alts.iter().all(|(_, o)| *o == Some(*inst))
        {
            return each(self, *inst);
        }
        self.site += 1;
        let key = format!("~{}:~pick", self.site);
        let okey = format!("~{}:~pickobj", self.site);
        self.objects.insert(okey.clone(), OVal::One(None));
        let before = self.objects.clone();
        let (mut arms, mut ends, mut res, mut obj) = (Vec::new(), Vec::new(), None, false);
        for (c, o) in alts {
            let Some(inst) = o else { continue };
            self.objects = before.clone();
            let saved = std::mem::take(&mut self.pending);
            let called = each(self, inst);
            let mut body = std::mem::replace(&mut self.pending, saved);
            match called? {
                Called::Value(e) => {
                    let i = *res.get_or_insert_with(|| {
                        self.preset.entry(key.clone()).or_insert(e.ty);
                        self.key_local(&key, e.ty)
                    });
                    let t = self.locals[i].ty;
                    if e.ty > t && t != Ty::Bool {
                        let w = self.widen.entry(key.clone()).or_insert(e.ty);
                        *w = (*w).max(e.ty);
                    }
                    let e = if t == Ty::Bool { e } else { self.numeric(e) };
                    body.push(Stmt::Assign(i, conv(e, t)));
                }
                Called::Object(v) => {
                    obj = true;
                    self.objects.insert(okey.clone(), v);
                }
                Called::Nothing => {}
            }
            ends.push(std::mem::take(&mut self.objects));
            arms.push((c, body));
        }
        let mut els = Vec::new();
        ends.push(before.clone());
        let bodies: Vec<&mut Vec<Stmt>> = arms.iter_mut().map(|(_, b)| b).chain(std::iter::once(&mut els)).collect();
        self.merge(&before, ends, bodies);
        if !arms.is_empty() {
            self.pending.push(Stmt::If(arms, els));
        }
        let o = self.objects.remove(&okey).unwrap_or(OVal::One(None));
        Ok(match res {
            Some(i) => Called::Value(Expr { ty: self.locals[i].ty, kind: Kind::Local(i) }),
            None if obj => Called::Object(o),
            None => Called::Nothing,
        })
    }

    /// An object in a condition: true when it is not null.
    fn obj_truth(&mut self, e: &ast::Expr, line: usize) -> Result<Expr, Error> {
        let alts = self.obj_alts(e, line)?;
        Ok(truth(&alts))
    }

    /// `array[index] = object` for an object array.
    fn assign_element(&mut self, key: &str, name: &str, idx: &[ast::Expr], value: &ast::Expr, line: usize) -> Result<(), Error> {
        let [ix] = idx else { return err(line, format!("arrays of objects have one dimension ('{name}')")) };
        let o = self.object_of(value, line)?;
        let n = self.obj_arrays[key];
        match self.static_int(ix) {
            Some(i) if usize::try_from(i).is_ok_and(|i| i < n) => {
                self.objects.insert(elem_key(key, i as usize), o);
                Ok(())
            }
            Some(i) => err(line, format!("index {i} is outside of the object array '{name}' ({n} elements)")),
            None => {
                // Known only when rendering: every element may change.
                let v = self.expr(ix, line)?;
                let v = conv(self.numeric(v), Ty::Int);
                let sel = self.once(v);
                for k in 0..n {
                    self.store_if(&elem_key(key, k), eq_int(sel.clone(), k as i64), o.clone());
                }
                Ok(())
            }
        }
    }

    /// Whether a loop body makes objects or uses object arrays (then it is unrolled).
    fn wants_unroll(&self, body: &[ast::Stmt]) -> bool {
        let field_arrays: Vec<String> = self.obj_arrays.keys().filter_map(|k| k.rsplit_once('.').map(|(_, f)| f.to_string())).collect();
        let is_arr = |name: &str, member: bool| {
            if member { field_arrays.iter().any(|f| f.eq_ignore_ascii_case(name)) } else { self.obj_arrays.contains_key(&self.norm(name)) }
        };
        let mut hit = |e: &ast::Expr| match e {
            ast::Expr::New { .. } => true,
            ast::Expr::Index(n, _) => is_arr(n, false),
            ast::Expr::MemberIndex(_, f, _) => is_arr(f, true),
            _ => false,
        };
        let mut hit_target = |t: &Target| match t {
            Target::Index(n, _) => is_arr(n, false),
            Target::MemberIndex(_, f, _) => is_arr(f, true),
            _ => false,
        };
        body.iter().any(|s| stmt_any(s, &mut hit, &mut hit_target))
    }

    /// Whether `stmts` assign the variable with key `key` anywhere.
    fn assigns(&self, stmts: &[ast::Stmt], key: &str) -> bool {
        let mut no = |_: &ast::Expr| false;
        let mut target = |t: &Target| matches!(t, Target::Var(v) | Target::Comp { var: v, .. } if self.norm(v) == key);
        stmts.iter().any(|s| stmt_any(s, &mut no, &mut target))
    }

    /// Unrolls a counting `while` loop that makes objects or indexes object arrays, so the
    /// indexes are known when checking: the counter starts with a known value (its last
    /// assignment before the loop), changes only in plain statements of the body by known
    /// amounts, and the condition depends on the counter only. `None`: the loop stays a loop.
    fn try_unroll(&mut self, before: &[ast::Stmt], cond: &ast::Expr, body: &[ast::Stmt], line: usize) -> Result<Option<Vec<Stmt>>, Error> {
        if !self.wants_unroll(body) || body.iter().any(contains_return) {
            return Ok(None);
        }
        let ast::Expr::Binary(BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge | BinOp::Ne, a, _) = cond else { return Ok(None) };
        let ast::Expr::Ident(var) = &**a else { return Ok(None) };
        let key = self.norm(var);
        if self.unroll.contains_key(&key) || self.obj_vars.contains(&key) {
            return Ok(None);
        }
        let is_counter = |s: &ast::Stmt, ck: &Self| matches!(s, ast::Stmt::Assign { target: Target::Var(n), .. } if ck.norm(n) == key);
        // The start value.
        let mut start = None;
        for s in before.iter().rev() {
            match s {
                ast::Stmt::Decl { name, value: Some(v), .. } | ast::Stmt::Assign { target: Target::Var(name), value: v, .. }
                    if self.norm(name) == key =>
                {
                    start = self.static_int(v);
                    break;
                }
                s if self.assigns(std::slice::from_ref(s), &key) => break,
                _ => {}
            }
        }
        let Some(start) = start else { return Ok(None) };
        if body.iter().any(|s| !is_counter(s, self) && self.assigns(std::slice::from_ref(s), &key)) {
            return Ok(None);
        }
        // A limit known only when rendering but bounded (`i < @count` with a maximum): the
        // loop unrolls up to the bound; every iteration runs only while the condition holds.
        let bounded = match cond {
            ast::Expr::Binary(op @ (BinOp::Lt | BinOp::Le), _, x) if self.static_int(x).is_none() => self.bound_int(x).map(|b| (*op, b)),
            _ => None,
        };
        let holds = |ck: &Self| match bounded {
            Some((op, b)) => ck.unroll.get(&key).map(|&i| if op == BinOp::Lt { i < b } else { i <= b }),
            None => ck.static_cond(cond),
        };
        // Count the iterations first (the body's other statements cannot change the counter).
        self.unroll.insert(key.clone(), start);
        let mut n = 0;
        let finite = loop {
            match holds(self) {
                Some(true) if n < MAX_UNROLL => n += 1,
                Some(false) => break true,
                _ => break false,
            }
            let mut ok = true;
            for s in body {
                if let ast::Stmt::Assign { value, .. } = s
                    && is_counter(s, self)
                {
                    match self.static_int(value) {
                        Some(v) => {
                            self.unroll.insert(key.clone(), v);
                        }
                        None => ok = false,
                    }
                }
            }
            if !ok {
                break false;
            }
        };
        self.unroll.insert(key.clone(), start);
        if !finite {
            self.unroll.remove(&key);
            return Ok(None);
        }
        let slot = self.local(var, Ty::Int);
        let mut out = Vec::new();
        let r = (|| {
            for _ in 0..n {
                // Bounded loops: the condition with this iteration's counter, checked when
                // rendering.
                let guard = if bounded.is_some() { Some(self.cexpr(cond, line)?) } else { None };
                let start_objects = self.objects.clone();
                let mut iter = Vec::new();
                for s in body {
                    match s {
                        ast::Stmt::Assign { value, .. } if is_counter(s, self) => {
                            let v = self.static_int(value).unwrap_or(0);
                            self.unroll.insert(key.clone(), v);
                            if bounded.is_some() {
                                // Guarded iterations keep the counter at run time.
                                let t = self.locals[slot].ty;
                                iter.push(Stmt::Assign(slot, conv(Expr { ty: Ty::Int, kind: Kind::Int(v) }, t)));
                            }
                        }
                        _ => iter.extend(self.stmts(std::slice::from_ref(s))?),
                    }
                }
                match guard {
                    None => out.extend(iter),
                    Some((pre, c)) => {
                        let end = std::mem::replace(&mut self.objects, start_objects.clone());
                        let mut skipped = Vec::new();
                        self.merge(&start_objects, vec![end, start_objects.clone()], vec![&mut iter, &mut skipped]);
                        out.extend(pre);
                        out.push(Stmt::If(vec![(self.cond(c), iter)], skipped));
                    }
                }
            }
            Ok(())
        })();
        let last = self.unroll.remove(&key).unwrap_or(start);
        r?;
        if bounded.is_none() {
            // The counter keeps its final value after the loop.
            out.push(self.assign(var, Expr { ty: Ty::Int, kind: Kind::Int(last) }, line)?);
        }
        Ok(Some(out))
    }

    /// The object (or the run-time choice of objects) an expression refers to.
    fn object_of(&mut self, e: &ast::Expr, line: usize) -> Result<OVal, Error> {
        match e {
            ast::Expr::Index(..) | ast::Expr::MemberIndex(..) => match self.obj_array_alts(e, line)? {
                Some(alts) => Ok(self.materialize(alts)),
                None => err(line, "expected an object"),
            },
            ast::Expr::Assign(t, v) => {
                let read = self.assign_in_expr(t, v, line)?;
                self.object_of(&read, line)
            }
            ast::Expr::Num { v, .. } if *v == 0.0 => Ok(OVal::One(None)),
            ast::Expr::Ident(n) if n.eq_ignore_ascii_case("this") => Ok(OVal::One(self.scopes.last().and_then(|s| s.this))),
            ast::Expr::Ident(n) => {
                let key = self.norm(n);
                match self.objects.get(&key) {
                    Some(o) => Ok(o.clone()),
                    None => err(line, format!("'{n}' is not an object")),
                }
            }
            ast::Expr::Member(o, f) => {
                let base = self.obj_alts(o, line)?;
                if base.iter().all(|(_, o)| o.is_none()) {
                    return err(line, format!("'.{f}' of a null object"));
                }
                let mut alts = Vec::new();
                for (c, inst) in base {
                    let Some(inst) = inst else { continue };
                    let Some(v) = self.objects.get(&field_key(inst, f)).cloned() else { return err(line, format!("'{f}' is not an object field")) };
                    if const_bool(&c) == Some(true) {
                        return Ok(v);
                    }
                    alts.extend(v.alts().into_iter().map(|(c2, o)| (and_e(c.clone(), c2), o)));
                }
                Ok(self.materialize(alts))
            }
            ast::Expr::New { class, param, args } => Ok(OVal::One(self.instantiate(class, *param, args, line)?)),
            // A cast: Class(object) is the object itself.
            ast::Expr::Call { name, param: false, args } if args.len() == 1 && self.classes.is_some_and(|c| (c.lookup)(name).is_some()) => {
                self.object_of(&args[0], line)
            }
            ast::Expr::Method(o, name, args) => match self.method_call(o, name, args, line)? {
                Called::Object(o) => Ok(o),
                _ => err(line, format!("'{name}' does not return an object")),
            },
            _ => err(line, "expected an object"),
        }
    }

    /// `(target = value)` inside an expression: the assignment goes to the pending statements;
    /// returns the expression that reads the target.
    fn assign_in_expr(&mut self, t: &Target, v: &ast::Expr, line: usize) -> Result<ast::Expr, Error> {
        let mut out = Vec::new();
        self.stmt(&ast::Stmt::Assign { target: t.clone(), value: v.clone(), line }, &mut out)?;
        self.pending.extend(out);
        Ok(match t {
            Target::Var(n) => match n.strip_prefix('@') {
                Some(p) => ast::Expr::Param(p.to_string()),
                None => ast::Expr::Ident(n.clone()),
            },
            Target::Sys(n) => ast::Expr::Sys(n.clone()),
            Target::Index(n, i) => ast::Expr::Index(n.clone(), i.clone()),
            Target::Member(o, f) => ast::Expr::Member(o.clone(), f.clone()),
            Target::MemberIndex(o, f, i) => ast::Expr::MemberIndex(o.clone(), f.clone(), i.clone()),
            Target::Comp { re, var } => {
                ast::Expr::Call { name: if *re { "real" } else { "imag" }.into(), param: false, args: vec![ast::Expr::Ident(var.clone())] }
            }
        })
    }

    /// Whether an expression denotes an object (for comparisons with 0).
    fn is_object_expr(&self, e: &ast::Expr) -> bool {
        match e {
            ast::Expr::Ident(n) => n.eq_ignore_ascii_case("this") || self.obj_vars.contains(&self.norm(n)),
            ast::Expr::Index(n, _) => self.obj_arrays.contains_key(&self.norm(n)),
            ast::Expr::Member(..) | ast::Expr::New { .. } => true,
            _ => false,
        }
    }

    /// The image parameter `@name` names in the current scope: `Some(Some(index))` for a user
    /// parameter, `Some(None)` for one of an object whose parameters keep their defaults (no
    /// image), `None` if it is no image parameter.
    fn image_param(&self, name: &str) -> Option<Option<usize>> {
        let (prefix, plugin, class) = match self.scopes.last() {
            Some(s) => (s.prefix.clone(), s.plugin, s.this.map(|t| self.instances[t].class.clone()).or(s.class.clone())),
            None => (String::new(), true, None),
        };
        if plugin && let Some(i) = self.param_index(&format!("{prefix}{name}")) {
            return (self.params[i].kind == ParamKind::Image).then_some(Some(i));
        }
        let chain = self.chain(&class?, 0).ok()?;
        chain
            .iter()
            .rev()
            .find_map(|c| c.params.iter().find(|p| p.name.eq_ignore_ascii_case(name)))
            .filter(|p| p.kind == ParamKind::Image)
            .map(|_| None)
    }

    /// A method of a built-in image object.
    fn image_method(&mut self, image: Option<usize>, name: &str, args: &[ast::Expr], line: usize) -> Result<Called, Error> {
        let want = |n: usize| if args.len() == n { Ok(()) } else { err(line, format!("'{name}' takes {n} argument(s)")) };
        let arg = |ck: &mut Self, k: usize, ty: Ty| -> Result<Expr, Error> {
            let v = ck.expr(&args[k], line)?;
            Ok(conv(ck.numeric(v), ty))
        };
        let op = |o: fn(usize) -> ColorOp, ty: Ty, a: Vec<Expr>| match image {
            Some(p) => Expr { ty, kind: Kind::ColorOp(o(p), a) },
            // No image: transparent, zero size, empty.
            None => match ty {
                Ty::Color => Expr { ty, kind: Kind::ColorOp(ColorOp::Rgba, vec![real(0.0), real(0.0), real(0.0), real(0.0)]) },
                Ty::Bool => Expr { ty, kind: Kind::Bool(true) },
                _ => Expr { ty, kind: Kind::Int(0) },
            },
        };
        Ok(Called::Value(match name.to_ascii_lowercase().as_str() {
            "getcolor" => {
                want(1)?;
                let z = arg(self, 0, Ty::Complex)?;
                op(ColorOp::ImageColor, Ty::Color, vec![z])
            }
            "getpixel" => {
                want(2)?;
                let (x, y) = (arg(self, 0, Ty::Int)?, arg(self, 1, Ty::Int)?);
                op(ColorOp::ImagePixel, Ty::Color, vec![x, y])
            }
            "getwidth" => {
                want(0)?;
                op(ColorOp::ImageWidth, Ty::Int, Vec::new())
            }
            "getheight" => {
                want(0)?;
                op(ColorOp::ImageHeight, Ty::Int, Vec::new())
            }
            "getempty" => {
                want(0)?;
                op(ColorOp::ImageEmpty, Ty::Bool, Vec::new())
            }
            _ => return err(line, format!("images have no method '{name}' (getColor, getPixel, getWidth, getHeight, getEmpty)")),
        }))
    }

    /// `object.Method(args)`, `Class.Method(args)` (static, or a base class's implementation
    /// for `this`).
    fn method_call(&mut self, obj: &ast::Expr, name: &str, args: &[ast::Expr], line: usize) -> Result<Called, Error> {
        // Class.Method: a base class call on `this` or a static method.
        if let ast::Expr::Ident(cn) = obj
            && !cn.eq_ignore_ascii_case("this")
            && !self.obj_vars.contains(&self.norm(cn))
        {
            let class = self.class(cn, line)?;
            let this = self.scopes.last().and_then(|s| s.this);
            let (impl_class, m) =
                self.find_method(&class, name, line)?.ok_or_else(|| Error { line, msg: format!("'{cn}' has no method '{name}'") })?;
            let this = if m.is_static { None } else { this };
            return self.inline(this, &impl_class, &m, args, line);
        }
        let call = |ck: &mut Self, inst: usize| {
            if let Some(image) = ck.instances[inst].image {
                return ck.image_method(image, name, args, line);
            }
            let class = ck.instances[inst].class.clone();
            let (impl_class, m) =
                ck.find_method(&class, name, line)?.ok_or_else(|| Error { line, msg: format!("'{}' has no method '{name}'", class.name) })?;
            ck.inline(Some(inst), &impl_class, &m, args, line)
        };
        let alts = self.obj_alts(obj, line)?;
        if alts.iter().all(|(_, o)| o.is_none()) {
            return err(line, format!("'{name}' called on a null object"));
        }
        self.dispatch(alts, line, call)
    }

    /// A method called without an object inside class code (`Init(pz)`, `Formula(pparent)`):
    /// a method of `this` (or of the class, for static code). `None` if there is none.
    fn implicit_method(&mut self, name: &str, args: &[ast::Expr], line: usize) -> Result<Option<Called>, Error> {
        if let Some(s) = self.scopes.last() {
            let (this, class) = (s.this, s.class.clone());
            let class = match (this, class) {
                (Some(t), _) => Some(self.instances[t].class.clone()),
                (None, c) => c,
            };
            if let Some(class) = class
                && let Some((impl_class, m)) = self.find_method(&class, name, line)?
            {
                let this = if m.is_static { None } else { this };
                return self.inline(this, &impl_class, &m, args, line).map(Some);
            }
        }
        // A function of the formula (`complex func f(…)` in a section).
        if let Some(fc) = self.formula_class.clone()
            && let Some(m) = fc.methods.iter().find(|m| m.name.eq_ignore_ascii_case(name)).cloned()
        {
            return self.inline(None, &fc, &m, args, line).map(Some);
        }
        Ok(None)
    }

    /// The class an object parameter (`@name`) currently chooses.
    fn chosen_class(&self, name: &str) -> Option<String> {
        let prefix = self.scopes.last().map_or(String::new(), |s| s.prefix.clone());
        let full = format!("{prefix}{name}");
        let i = self.param_index(&full)?;
        if self.params[i].kind != ParamKind::Class {
            return None;
        }
        match self.classes.and_then(|c| c.choices.get(&full)) {
            Some(ast::ParamValue::Class { class }) if !class.is_empty() => Some(class.clone()),
            _ => match &self.params[i].default {
                ast::ParamValue::Class { class } => Some(class.clone()),
                _ => None,
            },
        }
    }

    /// Inlines method `m` of class `c` for object `this`: its statements go to `pending`, the
    /// result is a local (values) or an object.
    fn inline(&mut self, this: Option<usize>, c: &Arc<ast::ClassAst>, m: &ast::MethodAst, args: &[ast::Expr], line: usize) -> Result<Called, Error> {
        if self.scopes.len() >= MAX_INLINE_DEPTH {
            return err(line, format!("calls nested too deeply at '{}' (recursion is not supported)", m.name));
        }
        if args.len() != m.params.len() {
            return err(line, format!("'{}' takes {} argument(s)", m.name, m.params.len()));
        }
        self.site += 1;
        let site = self.site;
        let local_key = |n: &str| format!("~{site}:{}", n.to_ascii_lowercase());
        // Arguments in the caller's scope.
        enum Arg {
            Value(Expr, Option<i64>),
            Alias(String),
            Object(OVal),
        }
        let mut bound = Vec::new();
        for (p, a) in m.params.iter().zip(args) {
            bound.push(match &p.ty {
                ast::TypeRef::Class(_) => Arg::Object(self.object_of(a, line)?),
                ast::TypeRef::Builtin(_) => match a {
                    // Arrays are passed by reference.
                    ast::Expr::Ident(n) if self.arrays.contains_key(&self.norm(n)) => Arg::Alias(self.norm(n)),
                    ast::Expr::Ident(n) if p.by_ref && self.slots.contains_key(&self.norm(n)) && !self.unroll.contains_key(&self.norm(n)) => {
                        Arg::Alias(self.norm(n))
                    }
                    _ => {
                        // Values known only when rendering keep their bound (array sizes).
                        let bound = if self.static_int(a).is_none() { self.bound_int(a) } else { None };
                        Arg::Value(self.expr(a, line)?, bound)
                    }
                },
            });
        }
        // The method's names: fields of the object, then parameters and locals.
        let mut names = HashMap::new();
        let mut fields = HashSet::new();
        if let Some(t) = this {
            let class = self.instances[t].class.clone();
            for k in self.chain(&class, line)? {
                for f in &k.fields {
                    let l = f.name.to_ascii_lowercase();
                    names.insert(l.clone(), field_key(t, &l));
                    fields.insert(l);
                }
            }
        }
        let mut locals = HashSet::new();
        method_locals(&m.body, &fields, &mut locals);
        for l in locals {
            names.insert(l.clone(), local_key(&l));
        }
        let mut pre = Vec::new();
        for (p, a) in m.params.iter().zip(bound) {
            let l = p.name.to_ascii_lowercase();
            match a {
                Arg::Alias(k) => {
                    names.insert(l, k);
                }
                Arg::Object(o) => {
                    let k = local_key(&l);
                    names.insert(l, k.clone());
                    self.obj_vars.insert(k.clone());
                    self.objects.insert(k, o);
                }
                Arg::Value(v, bound) => {
                    let k = local_key(&l);
                    names.insert(l, k.clone());
                    let ty = match &p.ty {
                        ast::TypeRef::Builtin(DeclTy::Bool) => Ty::Bool,
                        ast::TypeRef::Builtin(DeclTy::Int) => Ty::Int,
                        ast::TypeRef::Builtin(DeclTy::Float) => Ty::Real,
                        ast::TypeRef::Builtin(DeclTy::Color) => Ty::Color,
                        _ => Ty::Complex,
                    };
                    self.preset.entry(k.clone()).or_insert(ty);
                    self.declared.insert(k.clone());
                    let i = self.key_local(&k, ty);
                    let t = self.locals[i].ty;
                    let v = if t == Ty::Bool { v } else { self.numeric(v) };
                    pre.push(Stmt::Assign(i, conv(v, t)));
                    if let Some(b) = bound {
                        self.bounds.insert(k.clone(), b);
                    }
                }
            }
        }
        let (prefix, plugin) = match this {
            Some(t) => (self.instances[t].prefix.clone(), self.instances[t].plugin),
            None => self.scopes.last().map_or((String::new(), true), |s| (s.prefix.clone(), s.plugin)),
        };
        let ret = match &m.ret {
            Some(ast::TypeRef::Builtin(d)) => {
                let ty = match d {
                    DeclTy::Bool => Ty::Bool,
                    DeclTy::Int => Ty::Int,
                    DeclTy::Float => Ty::Real,
                    DeclTy::Complex => Ty::Complex,
                    DeclTy::Color => Ty::Color,
                };
                let k = local_key("~return");
                self.preset.entry(k.clone()).or_insert(ty);
                self.declared.insert(k.clone());
                let i = self.key_local(&k, ty);
                Some((i, self.locals[i].ty))
            }
            _ => None,
        };
        // Returned objects go to an object variable, so returns in branches merge like other
        // assignments; for methods without a declared type only when something is returned.
        let returns_obj = matches!(m.ret, Some(ast::TypeRef::Class(_)));
        let ret_obj = (!matches!(m.ret, Some(ast::TypeRef::Builtin(_)))).then(|| local_key("~retobj"));
        if returns_obj && let Some(k) = &ret_obj {
            self.objects.insert(k.clone(), OVal::One(None));
        }
        // A done flag when a return can come before the end.
        let early = m.body.iter().enumerate().any(|(k, s)| contains_return(s) && !(k + 1 == m.body.len() && matches!(s, ast::Stmt::Return(..))));
        let done = early.then(|| {
            let k = local_key("~done");
            self.preset.entry(k.clone()).or_insert(Ty::Bool);
            self.key_local(&k, Ty::Bool)
        });
        if let Some(d) = done {
            pre.push(Stmt::Assign(d, Expr { ty: Ty::Bool, kind: Kind::Bool(false) }));
        }
        self.scopes.push(Scope { names, this, class: Some(c.clone()), prefix, plugin, ret, ret_obj, done });
        let body = self.stmts(&m.body);
        let scope = self.scopes.pop().unwrap();
        pre.extend(body?);
        self.pending.extend(pre);
        Ok(match (scope.ret, scope.ret_obj.and_then(|k| self.objects.remove(&k))) {
            (Some((i, t)), _) => Called::Value(Expr { ty: t, kind: Kind::Local(i) }),
            (None, Some(o)) => Called::Object(o),
            _ => Called::Nothing,
        })
    }

    /// `@name` inside a method: the object's own parameter (user parameter for plugin
    /// objects, the default value otherwise).
    fn scoped_param(&mut self, name: &str, line: usize) -> Option<Result<Expr, Error>> {
        let s = self.scopes.last()?;
        let (prefix, plugin, class, this) = (s.prefix.clone(), s.plugin, s.class.clone()?, s.this);
        let full = format!("{prefix}{name}");
        if plugin && let Some(i) = self.param_index(&full) {
            return Some(Ok(Expr { ty: self.params[i].ty, kind: Kind::Param(i) }));
        }
        // Defaults of a non-plugin object (search the object's class, else the method's).
        let class = this.map_or(class, |t| self.instances[t].class.clone());
        let chain = match self.chain(&class, line) {
            Ok(c) => c,
            Err(e) => return Some(Err(e)),
        };
        let p = chain.iter().rev().find_map(|c| c.params.iter().find(|p| p.name.eq_ignore_ascii_case(name)))?;
        Some(Ok(match &p.default {
            ast::ParamValue::Bool(b) => Expr { ty: Ty::Bool, kind: Kind::Bool(*b) },
            ast::ParamValue::Int(i) => Expr { ty: Ty::Int, kind: Kind::Int(*i) },
            ast::ParamValue::Float(f) => real(*f),
            ast::ParamValue::Complex(c) => Expr { ty: Ty::Complex, kind: Kind::Complex(Box::new(real(c[0])), Box::new(real(c[1]))) },
            ast::ParamValue::Color(c) => {
                Expr { ty: Ty::Color, kind: Kind::ColorOp(ColorOp::Rgba, vec![real(c[0]), real(c[1]), real(c[2]), real(c[3])]) }
            }
            _ => return Some(err(line, format!("'@{name}' cannot be used as a value"))),
        }))
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
            (ProgramKind::Object, "px" | "py" | "pz" | "cx" | "cy" | "cz") => {
                let axis = lower.as_bytes()[1] - b'x';
                return Ok(self.sys(if lower.starts_with('p') { Sys::P3(axis) } else { Sys::C3(axis) }));
            }
            (ProgramKind::Object, "pixel" | "z" | "c" | "zprev" | "center" | "magn" | "x" | "y" | "screenpixel" | "screenmax" | "angle") => {
                return err(line, format!("'#{name}' is not available in 3D formulas (they have #px, #py, #pz, #cx, #cy, #cz)"));
            }
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
            ast::Expr::Assign(t, v) => {
                // The assignment runs before the using statement; the value is the target's.
                let read = self.assign_in_expr(t, v, line)?;
                self.expr(&read, line)?
            }
            ast::Expr::Complex(a, b) => {
                let a = conv(self.expr(a, line)?, Ty::Real);
                let b = conv(self.expr(b, line)?, Ty::Real);
                Expr { ty: Ty::Complex, kind: Kind::Complex(Box::new(a), Box::new(b)) }
            }
            ast::Expr::Ident(name) => self.ident(name, line)?,
            ast::Expr::Sys(name) => self.ufm_sys(name, line)?,
            ast::Expr::Param(name) if !self.scopes.is_empty() && self.scopes.last().is_some_and(|s| s.class.is_some()) => {
                match self.scoped_param(name, line) {
                    Some(r) => r?,
                    None => return err(line, format!("unknown parameter '@{name}' in a class")),
                }
            }
            ast::Expr::Member(o, f) => {
                let field = |ck: &mut Self, inst: usize| {
                    let key = field_key(inst, f);
                    if ck.obj_vars.contains(&key) {
                        return Ok(Called::Value(truth(&ck.oval(&key).alts())));
                    }
                    let i = ck.key_local(&key, Ty::Complex);
                    Ok(Called::Value(Expr { ty: ck.locals[i].ty, kind: Kind::Local(i) }))
                };
                let alts = self.obj_alts(o, line)?;
                if alts.iter().all(|(_, o)| o.is_none()) {
                    return err(line, format!("'.{f}' of a null object"));
                }
                let called = self.dispatch(alts, line, field)?;
                match called {
                    Called::Value(e) => e,
                    _ => return err(line, format!("'.{f}' of a null object")),
                }
            }
            ast::Expr::MemberIndex(o, f, idx) => {
                let alts = self.obj_alts(o, line)?;
                let Some(first) = alts.iter().find_map(|(_, o)| *o) else { return err(line, format!("'.{f}' of a null object")) };
                if self.obj_arrays.contains_key(&field_key(first, f)) {
                    return self.obj_truth(e, line);
                }
                let element = |ck: &mut Self, inst: usize| {
                    let (slot, i) = ck.array_index_key(&field_key(inst, f), f, idx, line)?;
                    Ok(Called::Value(Expr { ty: ck.locals[slot].ty, kind: Kind::Index(slot, Box::new(i)) }))
                };
                match self.dispatch(alts, line, element)? {
                    Called::Value(e) => e,
                    _ => return err(line, format!("'.{f}' of a null object")),
                }
            }
            ast::Expr::Method(o, name, args) => match self.method_call(o, name, args, line)? {
                Called::Value(e) => e,
                _ => return err(line, format!("'{name}' gives no value")),
            },
            ast::Expr::New { .. } => return err(line, "'new' makes an object; assign it to an object variable"),
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
            ast::Expr::Index(name, _) if self.obj_arrays.contains_key(&self.norm(name)) => self.obj_truth(e, line)?,
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
                if v.ty == Ty::Color {
                    let zero = self.as_color(real(0.0), line)?;
                    return Ok(Expr { ty: Ty::Color, kind: Kind::ColorOp(ColorOp::Arith(Arith::Sub), vec![zero, v]) });
                }
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
        // An object parameter compared with a class name: known when checking.
        if matches!(op, BinOp::Eq | BinOp::Ne)
            && let (ast::Expr::Param(p), ast::Expr::Ident(c)) | (ast::Expr::Ident(c), ast::Expr::Param(p)) = (a, b)
            && let Some(chosen) = self.chosen_class(p)
        {
            // True when the chosen class is that class or derives from it.
            let same = match self.class(&chosen, line) {
                Ok(k) => self.chain(&k, line)?.iter().any(|x| x.name.eq_ignore_ascii_case(c)),
                Err(_) => chosen.rsplit(':').next().unwrap_or(&chosen).eq_ignore_ascii_case(c),
            };
            return Ok(Expr { ty: Ty::Bool, kind: Kind::Bool(same == (op == BinOp::Eq)) });
        }
        // Objects compared with 0 (null) or with each other: known when checking.
        if matches!(op, BinOp::Eq | BinOp::Ne) && (self.is_object_expr(a) || self.is_object_expr(b)) {
            // Compared with null: also for elements chosen at run time.
            if let (o, ast::Expr::Num { v: 0.0, .. }) | (ast::Expr::Num { v: 0.0, .. }, o) = (a, b) {
                let set = self.obj_truth(o, line)?;
                return Ok(if op == BinOp::Ne { set } else { Expr { ty: Ty::Bool, kind: Kind::Not(Box::new(set)) } });
            }
            let (x, y) = (self.obj_alts(a, line)?, self.obj_alts(b, line)?);
            let same = obj_eq(&x, &y);
            return Ok(if op == BinOp::Eq { same } else { Expr { ty: Ty::Bool, kind: Kind::Not(Box::new(same)) } });
        }
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
        // Arithmetic on colors: channel by channel; a number applies to all four channels.
        if (l.ty == Ty::Color || r.ty == Ty::Color) && matches!(op, BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div) {
            let k = match op {
                BinOp::Add => Arith::Add,
                BinOp::Sub => Arith::Sub,
                BinOp::Mul => Arith::Mul,
                _ => Arith::Div,
            };
            let (l, r) = (self.as_color(l, line)?, self.as_color(r, line)?);
            return Ok(Expr { ty: Ty::Color, kind: Kind::ColorOp(ColorOp::Arith(k), vec![l, r]) });
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

    /// A color operand of color arithmetic: colors as they are, numbers in all four channels.
    fn as_color(&self, e: Expr, line: usize) -> Result<Expr, Error> {
        match e.ty {
            Ty::Color => Ok(e),
            Ty::Complex => err(line, "a color cannot be combined with a complex number"),
            _ => Ok(Expr { ty: Ty::Color, kind: Kind::ColorOp(ColorOp::Splat, vec![conv(self.numeric(e), Ty::Real)]) }),
        }
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
        if param
            && let Some(s) = self.scopes.last()
            && s.class.is_some()
        {
            return self.param_index(&format!("{}{name}", s.prefix)).filter(|&i| self.params[i].kind == ParamKind::Func);
        }
        let i = if param || self.dialect == Dialect::Frm { self.param_index(name) } else { None };
        let i = i.or_else(|| if self.dialect == Dialect::Frm { self.classic.get(name).copied() } else { None });
        i.filter(|&i| self.params[i].kind == ParamKind::Func)
    }

    fn call(&mut self, name: &str, param: bool, args: &[ast::Expr], line: usize) -> Result<Expr, Error> {
        if let Some(i) = self.func_param(name, param) {
            // A color function parameter (merge mode): two colors.
            if args.len() == 2 {
                let a = self.expr(&args[0], line)?;
                let b = self.expr(&args[1], line)?;
                return Ok(Expr { ty: Ty::Color, kind: Kind::MergeParam(i, Box::new(conv(a, Ty::Color)), Box::new(conv(b, Ty::Color))) });
            }
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
        {
            if let Some((_, dims)) = self.arrays.get(&self.norm(a)) {
                return Ok(Expr { ty: Ty::Int, kind: Kind::Local(dims[0]) });
            }
            if let Some(&slot) = self.obj_len.get(&self.norm(a)) {
                return Ok(Expr { ty: Ty::Int, kind: Kind::Local(slot) });
            }
            if let Some(&n) = self.obj_arrays.get(&self.norm(a)) {
                return Ok(Expr { ty: Ty::Int, kind: Kind::Int(n as i64) });
            }
        }
        if lower == "deriv" && args.len() == 2 {
            return self.deriv(args, line);
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
            if !param && let Some(called) = self.implicit_method(name, args, line)? {
                return match called {
                    Called::Value(e) => Ok(e),
                    _ => err(line, format!("'{name}' gives no value")),
                };
            }
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

    /// `deriv(e, x)`: the derivative of a real or complex expression with respect to a real or
    /// complex variable (`super::deriv`).
    fn deriv(&mut self, args: &[ast::Expr], line: usize) -> Result<Expr, Error> {
        let x = self.expr(&args[1], line)?;
        let slot = match x.kind {
            Kind::Local(slot) if matches!(x.ty, Ty::Real | Ty::Complex) && self.locals[slot].cap.is_none() => slot,
            _ => return err(line, "deriv(e, x): x must be a real or complex variable"),
        };
        let v = self.expr(&args[0], line)?;
        let e = match self.numeric(v) {
            e if e.ty == Ty::Int => conv(e, Ty::Real),
            e if matches!(e.ty, Ty::Real | Ty::Complex) => e,
            _ => return err(line, "deriv(e, x): e must be a number"),
        };
        Ok(Expr { ty: e.ty, kind: Kind::Deriv(Box::new(e), slot) })
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
        if self.is_z(&key) {
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
        let done = self.scopes.last().and_then(|s| s.done);
        for (k, s) in list.iter().enumerate() {
            let before = std::mem::take(&mut self.pending);
            let mut produced = Vec::new();
            match s {
                ast::Stmt::While { cond, body, line } => match self.try_unroll(&list[..k], cond, body, *line)? {
                    Some(u) => produced = u,
                    None => self.stmt(s, &mut produced)?,
                },
                _ => self.stmt(s, &mut produced)?,
            }
            let pre = std::mem::replace(&mut self.pending, before);
            out.extend(pre);
            out.extend(produced);
            // After a statement that may return, the rest runs only if it did not.
            if let Some(d) = done
                && contains_return(s)
                && k + 1 < list.len()
            {
                let start = self.objects.clone();
                let mut rest = self.stmts(&list[k + 1..])?;
                let end = std::mem::replace(&mut self.objects, start.clone());
                let mut skipped = Vec::new();
                self.merge(&start, vec![end, start.clone()], vec![&mut rest, &mut skipped]);
                if !rest.is_empty() || !skipped.is_empty() {
                    let not_done = Expr { ty: Ty::Bool, kind: Kind::Not(Box::new(Expr { ty: Ty::Bool, kind: Kind::Local(d) })) };
                    out.push(Stmt::If(vec![(not_done, rest)], skipped));
                }
                break;
            }
        }
        Ok(out)
    }

    /// An expression with the statements it needs (inlined calls) captured separately.
    fn cexpr(&mut self, e: &ast::Expr, line: usize) -> Result<(Vec<Stmt>, Expr), Error> {
        let before = std::mem::take(&mut self.pending);
        let r = self.expr(e, line);
        let pre = std::mem::replace(&mut self.pending, before);
        Ok((pre, r?))
    }

    fn stmt(&mut self, s: &ast::Stmt, out: &mut Vec<Stmt>) -> Result<(), Error> {
        {
            match s {
                ast::Stmt::Assign { target: Target::Var(name), value, line } if self.obj_vars.contains(&self.norm(name)) => {
                    let o = self.object_of(value, *line)?;
                    let key = self.norm(name);
                    self.objects.insert(key, o);
                }
                // `complex b[n] = a` (declares b) and `b = a`: copies an array.
                ast::Stmt::Assign { target, value: ast::Expr::Ident(src), line } if self.is_array_copy(target, src) => {
                    let src_key = self.norm(src);
                    if let Target::Index(name, dims) = target {
                        let ty = match self.locals[self.arrays[&src_key].0].ty {
                            Ty::Bool => DeclTy::Bool,
                            Ty::Int => DeclTy::Int,
                            Ty::Real => DeclTy::Float,
                            Ty::Complex => DeclTy::Complex,
                            Ty::Color => DeclTy::Color,
                        };
                        self.declare_array(ty, name, dims, *line, out)?;
                    }
                    let dst_key = match target {
                        Target::Index(n, _) | Target::Var(n) => self.norm(n),
                        _ => unreachable!(),
                    };
                    self.copy_array(&dst_key, &src_key, out);
                }
                ast::Stmt::Assign { target: Target::Index(name, idx), value, line } if self.obj_arrays.contains_key(&self.norm(name)) => {
                    let key = self.norm(name);
                    self.assign_element(&key, name, idx, value, *line)?;
                }
                ast::Stmt::Assign { target: Target::Member(obj, field), value, line } => {
                    let alts: Vec<(Expr, usize)> = self.obj_alts(obj, *line)?.into_iter().filter_map(|(c, o)| o.map(|i| (c, i))).collect();
                    let Some(&(_, first)) = alts.first() else { return err(*line, format!("'.{field}' of a null object")) };
                    if self.obj_vars.contains(&field_key(first, field)) {
                        let o = self.object_of(value, *line)?;
                        for (c, inst) in alts {
                            self.store_if(&field_key(inst, field), c, o.clone());
                        }
                    } else {
                        let v = self.expr(value, *line)?;
                        let v = if v.ty == Ty::Bool { v } else { self.numeric(v) };
                        // Several objects: the value is computed once, stored where it applies.
                        let v = if alts.len() > 1 { self.value_once(v, out) } else { v };
                        for (c, inst) in alts {
                            let i = self.key_local(&field_key(inst, field), v.ty);
                            let t = self.locals[i].ty;
                            let set = Stmt::Assign(i, conv(v.clone(), t));
                            out.push(if const_bool(&c) == Some(true) { set } else { Stmt::If(vec![(c, vec![set])], Vec::new()) });
                        }
                    }
                }
                ast::Stmt::Assign { target: Target::MemberIndex(obj, field, idx), value, line } => {
                    let alts: Vec<(Expr, usize)> = self.obj_alts(obj, *line)?.into_iter().filter_map(|(c, o)| o.map(|i| (c, i))).collect();
                    let Some(&(_, first)) = alts.first() else { return err(*line, format!("'.{field}' of a null object")) };
                    let key = field_key(first, field);
                    if self.obj_arrays.contains_key(&key) {
                        if alts.len() > 1 {
                            return err(*line, format!("objects are stored in '{field}' of an object chosen only when rendering (not supported)"));
                        }
                        return self.assign_element(&key, field, idx, value, *line);
                    }
                    let v = self.expr(value, *line)?;
                    let v = if v.ty == Ty::Bool { v } else { self.numeric(v) };
                    let v = if alts.len() > 1 { self.value_once(v, out) } else { v };
                    for (c, inst) in alts {
                        let (slot, i) = self.array_index_key(&field_key(inst, field), field, idx, *line)?;
                        let t = self.locals[slot].ty;
                        let set = Stmt::SetIndex(slot, i, conv(v.clone(), t));
                        out.push(if const_bool(&c) == Some(true) { set } else { Stmt::If(vec![(c, vec![set])], Vec::new()) });
                    }
                }
                ast::Stmt::Assign { target, value, line } => {
                    let v = self.expr(value, *line)?;
                    match target {
                        Target::Member(..) | Target::MemberIndex(..) => unreachable!(),
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
                            let i = if self.is_z(&key) { Z } else { self.local(var, Ty::Complex) };
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
                    if self.is_z(&key) {
                        if let Some(v) = value {
                            let v = self.expr(v, *line)?;
                            out.push(Stmt::Assign(Z, conv(v, Ty::Complex)));
                        }
                        return Ok(());
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
                    // Conditions that need statements (inlined calls) split the chain: later
                    // conditions run only when the earlier ones failed.
                    // Object variables are tracked per branch and joined afterwards.
                    let start = self.objects.clone();
                    let (mut parts, mut ends, mut always) = (Vec::new(), Vec::new(), false);
                    for (c, b) in arms {
                        let (pre, v) = self.cexpr(c, *line)?;
                        let c = self.cond(v);
                        // A branch known to be false (object checks): its code is not compiled,
                        // it may call methods the chosen class does not have.
                        if pre.is_empty() && const_bool(&c) == Some(false) {
                            continue;
                        }
                        let at_cond = self.objects.clone();
                        let body = self.stmts(b)?;
                        ends.push(std::mem::replace(&mut self.objects, at_cond));
                        let known = pre.is_empty() && const_bool(&c) == Some(true);
                        parts.push((pre, c, body));
                        // A branch known to be taken: the later ones never run.
                        if known {
                            always = true;
                            break;
                        }
                    }
                    let mut tail = Vec::new();
                    if !always {
                        tail = self.stmts(els)?;
                        ends.push(std::mem::take(&mut self.objects));
                    }
                    let mut bodies: Vec<&mut Vec<Stmt>> = parts.iter_mut().map(|(_, _, b)| b).collect();
                    if !always {
                        bodies.push(&mut tail);
                    }
                    self.merge(&start, ends, bodies);
                    let mut arms_ir: Vec<(Expr, Vec<Stmt>)> = Vec::new();
                    for (k, (pre, c, body)) in parts.into_iter().enumerate().rev() {
                        arms_ir.insert(0, (c, body));
                        if !pre.is_empty() || k == 0 {
                            let iff = Stmt::If(std::mem::take(&mut arms_ir), tail);
                            tail = pre;
                            tail.push(iff);
                        }
                    }
                    out.extend(tail);
                }
                ast::Stmt::While { cond, body, line } => {
                    let start = self.objects.clone();
                    let (pre, v) = self.cexpr(cond, *line)?;
                    let mut c = self.cond(v);
                    let mut b = self.stmts(body)?;
                    self.loop_objects(&start, *line)?;
                    b.extend(pre.iter().cloned());
                    if let Some(d) = self.scopes.last().and_then(|s| s.done)
                        && body.iter().any(contains_return)
                    {
                        let not_done = Expr { ty: Ty::Bool, kind: Kind::Not(Box::new(Expr { ty: Ty::Bool, kind: Kind::Local(d) })) };
                        c = Expr { ty: Ty::Bool, kind: Kind::And(Box::new(c), Box::new(not_done)) };
                    }
                    out.extend(pre);
                    out.push(Stmt::While(c, b));
                }
                ast::Stmt::ObjDecl { class, name, dims, value, line } => {
                    let key = self.norm(name);
                    if let Some(d) = dims {
                        let _ = class;
                        let n = self.obj_array_size(name, d, *line)?;
                        self.declare_obj_array(key, n);
                        return Ok(());
                    }
                    self.obj_vars.insert(key.clone());
                    let o = match value {
                        Some(v) => self.object_of(v, *line)?,
                        None => OVal::One(None),
                    };
                    self.objects.insert(key, o);
                }
                ast::Stmt::Eval(e, line) => match e {
                    ast::Expr::Method(obj, name, args) => {
                        let _ = self.method_call(obj, name, args, *line)?;
                    }
                    _ => {
                        let _ = self.expr(e, *line)?;
                    }
                },
                ast::Stmt::Return(value, line) => {
                    let Some(scope) = self.scopes.last() else { return err(*line, "'return' outside of a method") };
                    let (ret, done, ret_obj) = (scope.ret, scope.done, scope.ret_obj.clone());
                    match (value, ret) {
                        (Some(v), Some((slot, t))) => {
                            let e = self.expr(v, *line)?;
                            let e = if t == Ty::Bool { e } else { self.numeric(e) };
                            out.push(Stmt::Assign(slot, conv(e, t)));
                        }
                        (Some(v), None) => {
                            if let Some(k) = ret_obj {
                                let o = self.object_of(v, *line)?;
                                self.objects.insert(k, o);
                            }
                        }
                        _ => {}
                    }
                    if let Some(d) = done {
                        out.push(Stmt::Assign(d, Expr { ty: Ty::Bool, kind: Kind::Bool(true) }));
                    }
                }
                ast::Stmt::Seq(list) => out.extend(self.stmts(list)?),
                ast::Stmt::ArrayDecl { ty, name, dims, line } => self.declare_array(*ty, name, dims, *line, out)?,
                ast::Stmt::Call { name, args, line } => match (name.to_ascii_lowercase().as_str(), args.as_slice()) {
                    ("setlength", [ast::Expr::Ident(a), n]) if self.obj_arrays.contains_key(&self.norm(a)) => {
                        let key = self.norm(a);
                        if let Some(len) = self.static_int(n).and_then(|v| usize::try_from(v).ok()).filter(|&v| v <= ARRAY_MAX) {
                            self.obj_len.remove(&key);
                            self.declare_obj_array(key, len);
                        } else if let Some(cap) = self.bound_int(n).and_then(|v| usize::try_from(v).ok()).filter(|&v| v <= ARRAY_MAX) {
                            // Sized when rendering: room for the bound, the length in a local.
                            self.declare_obj_array(key.clone(), cap);
                            let slot = self.key_local(&format!("#len:{key}"), Ty::Int);
                            let v = self.expr(n, *line)?;
                            out.push(Stmt::Assign(slot, conv(self.numeric(v), Ty::Int)));
                            self.obj_len.insert(key, slot);
                        } else {
                            return err(
                                *line,
                                format!(
                                    "the length of the object array '{a}' must be known when checking (a number, a counting loop or an int parameter with a maximum)"
                                ),
                            );
                        }
                    }
                    ("setlength", [ast::Expr::Ident(a), n]) => {
                        let Some((_, dims)) = self.arrays.get(&self.norm(a)).cloned() else { return err(*line, format!("'{a}' is not an array")) };
                        let e = self.expr(n, *line)?;
                        out.push(Stmt::Assign(dims[0], conv(self.numeric(e), Ty::Int)));
                    }
                    // Debug output of the original program: nothing to show here.
                    ("print", _) => {}
                    _ => match self.implicit_method(name, args, *line)? {
                        Some(_) => {}
                        None => return err(*line, format!("unknown procedure '{name}'")),
                    },
                },
                ast::Stmt::Repeat { body, cond, line } => {
                    let start = self.objects.clone();
                    let mut b = self.stmts(body)?;
                    self.loop_objects(&start, *line)?;
                    let (pre, v) = self.cexpr(cond, *line)?;
                    let mut c = self.cond(v);
                    b.extend(pre);
                    if let Some(d) = self.scopes.last().and_then(|s| s.done)
                        && body.iter().any(contains_return)
                    {
                        c = Expr { ty: Ty::Bool, kind: Kind::Or(Box::new(c), Box::new(Expr { ty: Ty::Bool, kind: Kind::Local(d) })) };
                    }
                    out.push(Stmt::Repeat(b, c));
                }
            }
        }
        Ok(())
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
        // Object and image parameters have no value in the program (they choose code or name
        // a file); a placeholder.
        ParamKind::Class | ParamKind::Image => Ty::Int,
    }
}

/// What an inlined call gives.
enum Called {
    Value(Expr),
    Object(OVal),
    Nothing,
}

/// What an object variable holds: one object (or null), or one of several candidates chosen
/// at run time by a tag (an int local holding the candidate's position; candidate 0 is null).
#[derive(Clone, Debug, PartialEq)]
enum OVal {
    One(Option<usize>),
    Tag(usize, Vec<Option<usize>>),
}

/// The objects an expression can refer to, each with the run-time condition when.
type Alts = Vec<(Expr, Option<usize>)>;

impl OVal {
    fn alts(&self) -> Alts {
        match self {
            OVal::One(o) => vec![(bool_e(true), *o)],
            OVal::Tag(t, c) => c.iter().enumerate().map(|(j, o)| (eq_int(Expr { ty: Ty::Int, kind: Kind::Local(*t) }, j as i64), *o)).collect(),
        }
    }

    fn cands(&self) -> Vec<Option<usize>> {
        match self {
            OVal::One(o) => vec![*o],
            OVal::Tag(_, c) => c.clone(),
        }
    }
}

fn bool_e(b: bool) -> Expr {
    Expr { ty: Ty::Bool, kind: Kind::Bool(b) }
}

fn int_e(i: i64) -> Expr {
    Expr { ty: Ty::Int, kind: Kind::Int(i) }
}

fn eq_int(a: Expr, k: i64) -> Expr {
    Expr { ty: Ty::Bool, kind: Kind::Cmp(Cmp::Eq, Box::new(a), Box::new(int_e(k))) }
}

fn and_e(a: Expr, b: Expr) -> Expr {
    match (const_bool(&a), const_bool(&b)) {
        (Some(true), _) => b,
        (_, Some(true)) => a,
        (Some(false), _) | (_, Some(false)) => bool_e(false),
        _ => Expr { ty: Ty::Bool, kind: Kind::And(Box::new(a), Box::new(b)) },
    }
}

fn or_e(a: Expr, b: Expr) -> Expr {
    match (const_bool(&a), const_bool(&b)) {
        (Some(false), _) => b,
        (_, Some(false)) => a,
        (Some(true), _) | (_, Some(true)) => bool_e(true),
        _ => Expr { ty: Ty::Bool, kind: Kind::Or(Box::new(a), Box::new(b)) },
    }
}

/// Whether the object is not null.
fn truth(alts: &Alts) -> Expr {
    alts.iter().filter(|(_, o)| o.is_some()).fold(bool_e(false), |acc, (c, _)| or_e(acc, c.clone()))
}

/// Whether two objects are the same.
fn obj_eq(a: &Alts, b: &Alts) -> Expr {
    let mut acc = bool_e(false);
    for (ca, oa) in a {
        for (cb, ob) in b {
            if oa == ob {
                acc = or_e(acc, and_e(ca.clone(), cb.clone()));
            }
        }
    }
    acc
}

/// The object variable holding element `k` of the object array `key`.
fn elem_key(key: &str, k: usize) -> String {
    format!("{key}[{k}]")
}

/// Whether `s` (at any depth) contains an expression for which `pe` holds or an assignment
/// target for which `pt` holds (declarations count as assignments to `Target::Var`).
fn stmt_any(s: &ast::Stmt, pe: &mut dyn FnMut(&ast::Expr) -> bool, pt: &mut dyn FnMut(&Target) -> bool) -> bool {
    fn expr_any(e: &ast::Expr, pe: &mut dyn FnMut(&ast::Expr) -> bool, pt: &mut dyn FnMut(&Target) -> bool) -> bool {
        if pe(e) {
            return true;
        }
        match e {
            ast::Expr::Complex(a, b) | ast::Expr::Binary(_, a, b) => expr_any(a, pe, pt) || expr_any(b, pe, pt),
            ast::Expr::Unary(_, a) | ast::Expr::Mag(a) | ast::Expr::Member(a, _) => expr_any(a, pe, pt),
            ast::Expr::Call { args, .. } | ast::Expr::Index(_, args) | ast::Expr::New { args, .. } => args.iter().any(|a| expr_any(a, pe, pt)),
            ast::Expr::MemberIndex(o, _, args) | ast::Expr::Method(o, _, args) => expr_any(o, pe, pt) || args.iter().any(|a| expr_any(a, pe, pt)),
            ast::Expr::Assign(t, v) => pt(t) || target_any(t, pe, pt) || expr_any(v, pe, pt),
            _ => false,
        }
    }
    fn target_any(t: &Target, pe: &mut dyn FnMut(&ast::Expr) -> bool, pt: &mut dyn FnMut(&Target) -> bool) -> bool {
        match t {
            Target::Index(_, idx) => idx.iter().any(|a| expr_any(a, pe, pt)),
            Target::Member(o, _) => expr_any(o, pe, pt),
            Target::MemberIndex(o, _, idx) => expr_any(o, pe, pt) || idx.iter().any(|a| expr_any(a, pe, pt)),
            _ => false,
        }
    }
    let list = |b: &[ast::Stmt], pe: &mut dyn FnMut(&ast::Expr) -> bool, pt: &mut dyn FnMut(&Target) -> bool| b.iter().any(|s| stmt_any(s, pe, pt));
    match s {
        ast::Stmt::Assign { target, value, .. } => pt(target) || expr_any(value, pe, pt) || target_any(target, pe, pt),
        ast::Stmt::Decl { name, value, .. } => pt(&Target::Var(name.clone())) || value.as_ref().is_some_and(|v| expr_any(v, pe, pt)),
        ast::Stmt::ObjDecl { dims, value, .. } => {
            dims.iter().flatten().any(|d| expr_any(d, pe, pt)) || value.as_ref().is_some_and(|v| expr_any(v, pe, pt))
        }
        ast::Stmt::ArrayDecl { dims, .. } => dims.iter().any(|d| expr_any(d, pe, pt)),
        ast::Stmt::If { arms, els, .. } => arms.iter().any(|(c, b)| expr_any(c, pe, pt) || list(b, pe, pt)) || list(els, pe, pt),
        ast::Stmt::While { cond, body, .. } | ast::Stmt::Repeat { body, cond, .. } => expr_any(cond, pe, pt) || list(body, pe, pt),
        ast::Stmt::Seq(body) => list(body, pe, pt),
        ast::Stmt::Call { args, .. } => args.iter().any(|a| expr_any(a, pe, pt)),
        ast::Stmt::Eval(e, _) => expr_any(e, pe, pt),
        ast::Stmt::Return(v, _) => v.as_ref().is_some_and(|v| expr_any(v, pe, pt)),
    }
}

/// Variables assigned inside expressions of `s` (at any depth): `(x = …)`.
fn inner_assigned(s: &ast::Stmt) -> Vec<String> {
    let mut names = Vec::new();
    let mut pe = |e: &ast::Expr| {
        if let ast::Expr::Assign(t, _) = e
            && let Target::Var(v) | Target::Comp { var: v, .. } = &**t
        {
            names.push(v.clone());
        }
        false
    };
    stmt_any(s, &mut pe, &mut |_| false);
    names
}

/// The key of field `name` of object `inst`.
fn field_key(inst: usize, name: &str) -> String {
    format!("o{inst}.{}", name.to_ascii_lowercase())
}

/// The value of a condition known when checking.
fn const_bool(e: &Expr) -> Option<bool> {
    match &e.kind {
        Kind::Bool(b) => Some(*b),
        Kind::Not(a) => const_bool(a).map(|b| !b),
        Kind::And(a, b) => match (const_bool(a), const_bool(b)) {
            (Some(false), _) | (_, Some(false)) => Some(false),
            (Some(true), Some(true)) => Some(true),
            _ => None,
        },
        Kind::Or(a, b) => match (const_bool(a), const_bool(b)) {
            (Some(true), _) | (_, Some(true)) => Some(true),
            (Some(false), Some(false)) => Some(false),
            _ => None,
        },
        _ => None,
    }
}

/// Whether a statement contains a `return` (at any depth).
fn contains_return(s: &ast::Stmt) -> bool {
    match s {
        ast::Stmt::Return(..) => true,
        ast::Stmt::If { arms, els, .. } => arms.iter().any(|(_, b)| b.iter().any(contains_return)) || els.iter().any(contains_return),
        ast::Stmt::While { body, .. } | ast::Stmt::Repeat { body, .. } | ast::Stmt::Seq(body) => body.iter().any(contains_return),
        _ => false,
    }
}

/// Names a method body introduces: declared ones, and assigned ones that are not fields.
fn method_locals(body: &[ast::Stmt], fields: &HashSet<String>, out: &mut HashSet<String>) {
    for s in body {
        for n in inner_assigned(s) {
            let l = n.to_ascii_lowercase();
            if !l.starts_with('@') && !fields.contains(&l) && l != "z" {
                out.insert(l);
            }
        }
        match s {
            ast::Stmt::Decl { name, .. } | ast::Stmt::ArrayDecl { name, .. } | ast::Stmt::ObjDecl { name, .. } => {
                out.insert(name.to_ascii_lowercase());
            }
            ast::Stmt::Assign { target: Target::Var(n) | Target::Comp { var: n, .. }, .. } if !n.starts_with('@') => {
                let l = n.to_ascii_lowercase();
                if !fields.contains(&l) && l != "z" {
                    out.insert(l);
                }
            }
            ast::Stmt::If { arms, els, .. } => {
                for (_, b) in arms {
                    method_locals(b, fields, out);
                }
                method_locals(els, fields, out);
            }
            ast::Stmt::While { body, .. } | ast::Stmt::Repeat { body, .. } | ast::Stmt::Seq(body) => method_locals(body, fields, out),
            _ => {}
        }
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
        Kind::MergeParam(_, a, b) => color_misuse_expr(a) || color_misuse_expr(b),
        Kind::Index(_, i) => color_misuse_expr(i),
        Kind::Deriv(a, _) => color_misuse_expr(a),
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
    check_with(ast, kind, None)
}

/// Checks a parsed formula that may use objects (`classes`: the class libraries and the
/// classes chosen for object parameters).
pub fn check_with(ast: &FormulaAst, kind: ProgramKind, classes: Option<&ClassCtx>) -> Result<Program, Error> {
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
            // 3D objects: the point's real coordinates; 2D formulas: the complex z.
            locals: if kind == ProgramKind::Object {
                ["x", "y", "z"].map(|n| Local { name: n.into(), ty: Ty::Real, cap: None }).to_vec()
            } else {
                vec![Local { name: "z".into(), ty: Ty::Complex, cap: None }]
            },
            slots: HashMap::new(),
            preset: preset.clone(),
            widen: HashMap::new(),
            classic: HashMap::new(),
            has_julia: false,
            transcendental: false,
            arrays: HashMap::new(),
            classes,
            instances: Vec::new(),
            scopes: Vec::new(),
            objects: HashMap::new(),
            obj_vars: HashSet::new(),
            obj_arrays: HashMap::new(),
            unroll: HashMap::new(),
            bounds: HashMap::new(),
            obj_len: HashMap::new(),
            pending: Vec::new(),
            site: 0,
            formula_class: (!ast.funcs.is_empty()).then(|| {
                Arc::new(ast::ClassAst {
                    name: "~formula".into(),
                    parent: None,
                    fields: Vec::new(),
                    methods: ast.funcs.clone(),
                    params: Vec::new(),
                    title: None,
                    line: 0,
                })
            }),
        };
        if kind == ProgramKind::Object {
            for (i, n) in ["x", "y", "z"].iter().enumerate() {
                ck.slots.insert((*n).into(), i);
                ck.declared.insert((*n).into());
            }
        } else {
            ck.slots.insert("z".into(), Z);
        }
        if kind == ProgramKind::Coloring {
            ck.locals.push(Local { name: "#index".into(), ty: Ty::Real, cap: None });
            ck.index_slot = Some(ck.locals.len() - 1);
        }
        if matches!(kind, ProgramKind::Coloring | ProgramKind::Transform) {
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
                    ast::ParamValue::Func(f) if Func::by_name(f).is_some() || super::color::merge_mode(f).is_some() => {}
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
                base: p.base.clone(),
            });
            if let Some(slot) = &p.classic {
                ck.classic.insert(slot.clone(), ck.params.len() - 1);
            }
        }
        let mut init = ck.shadow_params();
        init.extend(ck.stmts(&ast.global)?);
        init.extend(ck.stmts(&ast.init)?);
        let mut body = if kind == ProgramKind::Transform { ck.stmts(&ast.transform)? } else { ck.stmts(&ast.body)? };
        let bailout = match (&ast.bailout, kind) {
            (Some(b), ProgramKind::Fractal | ProgramKind::Object) => {
                // Statements the bailout needs (inlined calls) run at the end of the loop.
                let (pre, e) = ck.cexpr(b, 0)?;
                body.extend(pre);
                ck.cond(e)
            }
            (None, ProgramKind::Fractal | ProgramKind::Object) => return err(0, "the formula has no bailout condition"),
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
        let mut prog = Program {
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
            linear_de: ast.distance.as_deref().is_some_and(|d| d.eq_ignore_ascii_case("linear")),
            bound: ast.bound.unwrap_or(4.0).max(0.1),
        };
        super::deriv::resolve(&mut prog);
        return Ok(prog);
    }
    err(0, "the variable types do not settle (assignments of different types)")
}

#[cfg(test)]
mod tests {
    use super::super::parse::{parse_frm, parse_ufm_entry, split_entries};
    use super::*;

    #[test]
    fn classes_inline_into_plain_code() {
        use super::super::cpu::Compiled;
        use crate::math::complex::C64;
        let lib = "class Base {\npublic:\n  func Base(Base pparent)\n  endfunc\n  complex func Step(complex pz)\n    return pz\n  endfunc\nprotected:\n  int m_n\ndefault:\n  float param scale\n    default = 1\n  endparam\n}\n\nclass Square(Base) {\npublic:\n  func Square(Base pparent)\n    Base.Base(pparent)\n  endfunc\n  complex func Step(complex pz)\n    m_n = m_n + 1\n    if m_n > 100000\n      return 0\n    endif\n    return Base.Step(pz)^2 * @scale\n  endfunc\n}\n";
        let classes: Vec<_> = crate::formula::parse::parse_classes(lib).into_iter().map(|(_, c)| std::sync::Arc::new(c.unwrap())).collect();
        let lookup = |n: &str| classes.iter().find(|c| c.name.eq_ignore_ascii_case(n)).cloned();
        let src = "Plug {\nglobal:\n  import \"x.ulb\"\n  Base f = new @plug(0)\ninit:\n  z = 0\nloop:\n  z = f.Step(z) + #pixel\nbailout:\n  |z| < 4\ndefault:\n  Base param plug\n    default = Square\n  endparam\n}\n";
        let e = split_entries(src);
        let ast = parse_ufm_entry(&e[0]).unwrap();
        let choices = BTreeMap::new();
        let ctx = ClassCtx { lookup: &lookup, choices: &choices };
        let p = check_with(&ast, ProgramKind::Fractal, Some(&ctx)).unwrap();
        let names: Vec<&str> = p.params.iter().map(|q| q.name.as_str()).collect();
        assert_eq!(names, ["plug", "plug.scale"]);
        // The default plugin squares: the Mandelbrot set.
        let k = Compiled::<f64>::new(&p, &[]);
        let mut f = k.frame();
        for (c, inside) in [(C64::new(-0.5, 0.0), true), (C64::new(0.5, 0.5), false), (C64::new(-1.0, 0.0), true)] {
            f.pixel = c;
            let o = k.run(&mut f, 200);
            assert_eq!(!o.escaped, inside, "{c:?}");
        }
        // The base class adds the point only: everything escapes slowly or not at all.
        let choices = BTreeMap::from([("plug".to_string(), ast::ParamValue::Class { class: "Base".into() })]);
        let ctx = ClassCtx { lookup: &lookup, choices: &choices };
        let p = check_with(&ast, ProgramKind::Fractal, Some(&ctx)).unwrap();
        let k = Compiled::<f64>::new(&p, &[]);
        let mut f = k.frame();
        f.pixel = C64::new(1.0, 0.0);
        assert_eq!(k.run(&mut f, 200).iter, 2);
    }

    #[test]
    fn object_arrays_unroll_and_dispatch() {
        use super::super::cpu::Compiled;
        use crate::math::complex::C64;
        let lib = "class Step {\npublic:\n  func Step(float pk)\n    m_k = pk\n  endfunc\n  complex func Apply(complex pz)\n    return pz * m_k\n  endfunc\nprotected:\n  float m_k\n}\n\nclass Merge {\npublic:\n  func Merge()\n    int j = 0\n    while j < length(m_s)\n      m_s[j] = new Step(j + 1)\n      j = j + 1\n    endwhile\n  endfunc\n  complex func Apply(complex pz)\n    int j = 0\n    while j < 3\n      if m_s[j]\n        pz = m_s[j].Apply(pz)\n      endif\n      j = j + 1\n    endwhile\n    return pz\n  endfunc\n  complex func Pick(complex pz, int n)\n    if m_s[n] != 0\n      return m_s[n].Apply(pz)\n    endif\n    return pz\n  endfunc\nprotected:\n  Step m_s[3]\n}\n";
        let classes: Vec<_> = crate::formula::parse::parse_classes(lib).into_iter().map(|(_, c)| std::sync::Arc::new(c.unwrap())).collect();
        let lookup = |n: &str| classes.iter().find(|c| c.name.eq_ignore_ascii_case(n)).cloned();
        let src = "T {\nglobal:\n  Merge m = new Merge()\n  Step extra[2]\ninit:\n  z = m.Apply(#pixel)\n  if extra[@which - 1]\n    z = 0\n  endif\nloop:\n  z = m.Pick(z, @which)\nbailout:\n  |z| < 1e20\ndefault:\n  int param which\n    default = 2\n  endparam\n}\n";
        let e = split_entries(src);
        let ast = parse_ufm_entry(&e[0]).unwrap();
        let choices = BTreeMap::new();
        let ctx = ClassCtx { lookup: &lookup, choices: &choices };
        let p = check_with(&ast, ProgramKind::Fractal, Some(&ctx)).unwrap();
        // Three objects with factors 1, 2, 3: the init multiplies by 6, every iteration by
        // the factor the parameter picks at run time.
        for (which, factor) in [(0, 1.0), (1, 2.0), (2, 3.0)] {
            let k = Compiled::<f64>::new(&p, &[ast::ParamValue::Int(which)]);
            let mut f = k.frame();
            f.pixel = C64::new(1.0, 0.0);
            let o = k.run(&mut f, 3);
            assert!((o.z.re - 6.0 * factor * factor * factor).abs() < 1e-9, "which {which}: {:?}", o.z);
        }
    }

    #[test]
    fn objects_chosen_at_run_time() {
        use super::super::cpu::Compiled;
        use crate::math::complex::C64;
        let lib = "class Step {\npublic:\n  func Step(float pk)\n    m_k = pk\n  endfunc\n  complex func Apply(complex pz)\n    return pz * m_k\n  endfunc\n  Step func Bigger(Step po)\n    if po.m_k > m_k\n      return po\n    endif\n    return this\n  endfunc\nprotected:\n  float m_k\n}\n";
        let classes: Vec<_> = crate::formula::parse::parse_classes(lib).into_iter().map(|(_, c)| std::sync::Arc::new(c.unwrap())).collect();
        let lookup = |n: &str| classes.iter().find(|c| c.name.eq_ignore_ascii_case(n)).cloned();
        let choices = BTreeMap::new();
        let ctx = ClassCtx { lookup: &lookup, choices: &choices };
        // Stored at an index from a parameter, chosen by a condition, returned from branches.
        let src = "T {\nglobal:\n  Step s[2]\n  s[0] = new Step(2)\n  s[1] = new Step(3)\n  s[@which] = new Step(5)\n  Step t\n  if @which == 0\n    t = new Step(7)\n  else\n    t = s[0]\n  endif\n  Step u = s[0].Bigger(s[1])\ninit:\n  z = #pixel\nloop:\n  z = t.Apply(u.Apply(s[1].Apply(z)))\nbailout:\n  |z| < 1e30\ndefault:\n  int param which\n    default = 0\n  endparam\n}\n";
        let ast = parse_ufm_entry(&split_entries(src)[0]).unwrap();
        let p = check_with(&ast, ProgramKind::Fractal, Some(&ctx)).unwrap();
        // which = 0: s = [5, 3], t = 7, u = 5 → 105; which = 1: s = [2, 5], t = s[0] = 2, u = 5 → 50.
        for (which, factor) in [(0, 105.0), (1, 50.0)] {
            let k = Compiled::<f64>::new(&p, &[ast::ParamValue::Int(which)]);
            let mut f = k.frame();
            f.pixel = C64::new(1.0, 0.0);
            let o = k.run(&mut f, 1);
            assert!((o.z.re - factor).abs() < 1e-9, "which {which}: {:?}", o.z);
        }
        // A loop that changes which object a variable refers to is refused.
        let src = "T {\nglobal:\n  Step a = new Step(2)\n  int i = 0\n  while i < @which\n    a.Apply(1)\n    a = new Step(3)\n    i = i + 1\n  endwhile\ninit:\n  z = 0\nloop:\n  z = z\nbailout:\n  |z| < 4\ndefault:\n  int param which\n    default = 0\n  endparam\n}\n";
        let ast = parse_ufm_entry(&split_entries(src)[0]).unwrap();
        assert!(check_with(&ast, ProgramKind::Fractal, Some(&ctx)).is_err());
    }

    #[test]
    fn color_arithmetic() {
        use super::super::cpu::Compiled;
        let src = "C {\nfinal:\n  color a = rgba(1, 0.5, 0, 1)\n  color b = rgba(0, 0.5, 1, 0.5)\n  float u = 0.25\n  #color = a * u + b * (1 - u) - -rgba(0, 0, 0, 0.1) / 2\n}\n";
        let p = check_kind(&parse_ufm_entry(&split_entries(src)[0]).unwrap(), ProgramKind::Coloring).unwrap();
        let k = Compiled::<f64>::new(&p, &[]);
        let mut f = k.frame();
        k.color_init(&mut f, 10);
        k.color_final(&mut f, 0, crate::math::complex::C64::new(0.0, 0.0));
        let c = k.direct_color(&f).unwrap();
        let want = [0.25, 0.5, 0.75, 0.25 + 0.375 + 0.05];
        assert!(c.iter().zip(want).all(|(x, y)| (x - y).abs() < 1e-12), "{c:?}");
        // Colors and complex numbers do not mix.
        let src = "C {\nfinal:\n  #color = rgb(1, 0, 0) * (1, 1)\n}\n";
        assert!(check_kind(&parse_ufm_entry(&split_entries(src)[0]).unwrap(), ProgramKind::Coloring).is_err());
    }

    #[test]
    fn assignments_inside_expressions() {
        use super::super::cpu::Compiled;
        // a = b = 3, r = 4 (inside an argument), n counts to 10; z = 3 + 3 + 4 + 10 = 20.
        let src = "T {\ninit:\n  int n = 0\n  float a = 0\n  float b = 0\n  float r = 0\n  if (a = b = 3) == 3\n    z = a + b + abs(r = r + 4) + 0 * r\n  endif\n  repeat\n  until (n = n + 1) >= 10\n  z = z + n\nloop:\n  z = z\nbailout:\n  |z| < 1e9\ndefault:\n  int param mode\n    enum = \"A\" \"B\"\n    default = 1\n  endparam\n}\n";
        let p = check(&parse_ufm_entry(&split_entries(src)[0]).unwrap()).unwrap();
        let k = Compiled::<f64>::new(&p, &[ast::ParamValue::Int(1)]);
        let mut f = k.frame();
        let o = k.run(&mut f, 1);
        assert!((o.z.re - 20.0).abs() < 1e-12, "{:?}", o.z);
        // A text on the right compares: (@mode = "B").
        let src = "T {\ninit:\n  z = 0\n  if (@mode = \"B\")\n    z = 1\n  endif\nloop:\n  z = z\nbailout:\n  |z| < 4\ndefault:\n  int param mode\n    enum = \"A\" \"B\"\n    default = 1\n  endparam\n}\n";
        let p = check(&parse_ufm_entry(&split_entries(src)[0]).unwrap()).unwrap();
        let k = Compiled::<f64>::new(&p, &[ast::ParamValue::Int(1)]);
        let mut f = k.frame();
        assert_eq!(k.run(&mut f, 1).z.re, 1.0);
    }

    #[test]
    fn object_arrays_sized_by_a_parameter() {
        use super::super::cpu::Compiled;
        use crate::math::complex::C64;
        let lib = "class Step {\npublic:\n  func Step(float pk)\n    m_k = pk\n  endfunc\n  complex func Apply(complex pz)\n    return pz * m_k\n  endfunc\nprotected:\n  float m_k\n}\n\nclass Holder {\npublic:\n  func Holder(int plength)\n    setLength(m_e, plength)\n  endfunc\n  Step m_e[]\n}\n";
        let classes: Vec<_> = crate::formula::parse::parse_classes(lib).into_iter().map(|(_, c)| std::sync::Arc::new(c.unwrap())).collect();
        let lookup = |n: &str| classes.iter().find(|c| c.name.eq_ignore_ascii_case(n)).cloned();
        let choices = BTreeMap::new();
        let ctx = ClassCtx { lookup: &lookup, choices: &choices };
        // count objects with factors 1, 2, …; z = #pixel * count!.
        let src = "T {\nglobal:\n  Holder h = new Holder(@count)\n  int i = 0\n  while i < @count\n    h.m_e[i] = new Step(i + 1)\n    i = i + 1\n  endwhile\ninit:\n  z = #pixel\n  int j = 0\n  while j < @count\n    z = h.m_e[j].Apply(z)\n    j = j + 1\n  endwhile\nloop:\n  z = z\nbailout:\n  |z| < 1e9\ndefault:\n  int param count\n    default = 2\n    min = 1\n    max = 4\n  endparam\n}\n";
        let ast = parse_ufm_entry(&split_entries(src)[0]).unwrap();
        let p = check_with(&ast, ProgramKind::Fractal, Some(&ctx)).unwrap();
        for (count, want) in [(1, 1.0), (2, 2.0), (3, 6.0), (4, 24.0)] {
            let k = Compiled::<f64>::new(&p, &[ast::ParamValue::Int(count)]);
            let mut f = k.frame();
            f.pixel = C64::new(1.0, 0.0);
            assert_eq!(k.run(&mut f, 1).z.re, want, "count {count}");
        }
    }

    #[test]
    fn array_copies() {
        use super::super::cpu::Compiled;
        let src = "T {\ninit:\n  float a[3]\n  a[0] = 1\n  a[1] = 2\n  a[2] = 3\n  float b[3] = a\n  a[0] = 100\n  float c[3]\n  c = b\n  z = c[0] + c[1] + c[2]\nloop:\n  z = z\nbailout:\n  |z| < 1e9\n}\n";
        let p = check(&parse_ufm_entry(&split_entries(src)[0]).unwrap()).unwrap();
        let k = Compiled::<f64>::new(&p, &[]);
        let mut f = k.frame();
        assert_eq!(k.run(&mut f, 1).z.re, 6.0);
    }

    #[test]
    fn formula_functions() {
        use super::super::cpu::Compiled;
        use crate::math::complex::C64;
        // step(z) = twice(z)^2 / 2 + c = z^2 + c (the Mandelbrot set); count() counts calls.
        let src = "T {\ninit:\n  int calls = 0\n  complex func twice(complex w)\n    return w * 2\n  endfunc\n  complex func step(complex w, complex k)\n    complex t = twice(w)\n    return t * t / 4 + k\n  endfunc\n  func count()\n    calls = calls + 1\n  endfunc\n  z = 0\nloop:\n  z = step(z, #pixel)\n  count()\nbailout:\n  |z| < 4 && calls < 1000\n}\n";
        let p = check(&parse_ufm_entry(&split_entries(src)[0]).unwrap()).unwrap();
        let k = Compiled::<f64>::new(&p, &[]);
        let mut f = k.frame();
        for (c, inside) in [(C64::new(-0.5, 0.0), true), (C64::new(0.5, 0.5), false), (C64::new(-1.0, 0.0), true)] {
            f.pixel = c;
            let o = k.run(&mut f, 200);
            assert_eq!(!o.escaped, inside, "{c:?}");
        }
    }

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
