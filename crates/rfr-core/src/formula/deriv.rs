// SPDX-License-Identifier: LGPL-3.0-or-later

//! Derivatives of expressions: `deriv(e, x)` ([`Kind::Deriv`]).
//!
//! The derivative is built from the IR by the chain rule, so the CPU, the GPU and the
//! perturbation lowering share one set of rules and evaluate plain IR. The derivative is the
//! one along the real direction of the variable: for holomorphic expressions in a complex
//! variable it is the complex derivative; `conj`, `abs`, `real`, `imag`, `flip`, … follow as
//! real-linear or piecewise maps. Rounding functions, integers, parameters and other
//! variables count as constants.
//!
//! Two steps: [`resolve`] (after checking) replaces variables inside `deriv` by the values
//! they were assigned earlier in the same section, as long as nothing reassigned them in
//! between, so `f = z^3 - 1` followed by `deriv(f, z)` works. [`derivative`] (in the back
//! ends, which know the chosen function parameters) builds the derivative expression.

use super::ir::*;
use std::collections::HashMap;

// ---- Resolution of earlier assignments ----

/// Replaces variables inside every `deriv` of the program by their straight-line definitions.
pub fn resolve(prog: &mut Program) {
    let mut env = Env::default();
    resolve_stmts(&mut prog.init, &mut env, &prog.locals);
    let mut env = Env::default();
    resolve_stmts(&mut prog.body, &mut env, &prog.locals);
    resolve_expr(&mut prog.bailout, &env);
    let mut env = Env::default();
    resolve_stmts(&mut prog.fin, &mut env, &prog.locals);
}

/// Known values of real and complex variables (no variable of a definition was reassigned since).
#[derive(Clone, Default)]
struct Env {
    defs: HashMap<usize, Expr>,
}

impl Env {
    fn invalidate(&mut self, slot: usize) {
        self.defs.remove(&slot);
        self.defs.retain(|_, e| !uses_local(e, slot));
    }

    fn assign(&mut self, slot: usize, value: &Expr, locals: &[Local]) {
        self.invalidate(slot);
        let l = &locals[slot];
        if l.cap.is_none() && matches!(l.ty, Ty::Real | Ty::Complex) && !uses_local(value, slot) && !has_random(value) {
            self.defs.insert(slot, value.clone());
        }
    }
}

fn resolve_stmts(list: &mut [Stmt], env: &mut Env, locals: &[Local]) {
    for s in list {
        match s {
            Stmt::Assign(slot, e) => {
                resolve_expr(e, env);
                env.assign(*slot, e, locals);
            }
            Stmt::SetPart { local, value, .. } => {
                resolve_expr(value, env);
                env.invalidate(*local);
            }
            Stmt::SetIndex(_, i, v) => {
                resolve_expr(i, env);
                resolve_expr(v, env);
            }
            Stmt::If(arms, els) => {
                for (c, b) in arms.iter_mut() {
                    resolve_expr(c, env);
                    resolve_stmts(b, &mut env.clone(), locals);
                }
                resolve_stmts(els, &mut env.clone(), locals);
                for slot in assigned(std::slice::from_ref(s)) {
                    env.invalidate(slot);
                }
            }
            Stmt::While(c, b) | Stmt::Repeat(b, c) => {
                // Values from before the loop hold in every round only when the loop keeps them.
                for slot in assigned(b) {
                    env.invalidate(slot);
                }
                resolve_expr(c, env);
                resolve_stmts(b, &mut env.clone(), locals);
            }
        }
    }
}

/// Locals that the statements assign (at any depth).
fn assigned(list: &[Stmt]) -> Vec<usize> {
    let mut out = Vec::new();
    fn walk(list: &[Stmt], out: &mut Vec<usize>) {
        for s in list {
            match s {
                Stmt::Assign(slot, _) | Stmt::SetPart { local: slot, .. } | Stmt::SetIndex(slot, _, _) => out.push(*slot),
                Stmt::If(arms, els) => {
                    for (_, b) in arms {
                        walk(b, out);
                    }
                    walk(els, out);
                }
                Stmt::While(_, b) | Stmt::Repeat(b, _) => walk(b, out),
            }
        }
    }
    walk(list, &mut out);
    out
}

/// Resolves the `deriv` nodes in `e` (inner ones first).
fn resolve_expr(e: &mut Expr, env: &Env) {
    for_children(e, &mut |c| resolve_expr(c, env));
    if let Kind::Deriv(inner, v) = &mut e.kind {
        let v = *v;
        substitute(inner, v, env, &mut Vec::new());
    }
}

/// Replaces variables (other than `v`) whose known value depends on `v` by that value.
fn substitute(e: &mut Expr, v: usize, env: &Env, stack: &mut Vec<usize>) {
    if let Kind::Local(i) = e.kind
        && i != v
        && !stack.contains(&i)
        && let Some(def) = env.defs.get(&i)
        && depends(def, v, env, &mut vec![i])
    {
        let mut def = def.clone();
        stack.push(i);
        substitute(&mut def, v, env, stack);
        stack.pop();
        *e = def;
        return;
    }
    for_children(e, &mut |c| substitute(c, v, env, stack));
}

/// Whether `e` reads `v`, directly or through known values.
fn depends(e: &Expr, v: usize, env: &Env, stack: &mut Vec<usize>) -> bool {
    if let Kind::Local(i) = e.kind {
        if i == v {
            return true;
        }
        if stack.contains(&i) {
            return false;
        }
        return match env.defs.get(&i) {
            Some(def) => {
                stack.push(i);
                let d = depends(def, v, env, stack);
                stack.pop();
                d
            }
            None => false,
        };
    }
    let mut found = false;
    for_children_ref(e, &mut |c| found = found || depends(c, v, env, stack));
    found
}

fn uses_local(e: &Expr, slot: usize) -> bool {
    if let Kind::Local(i) | Kind::Index(i, _) = e.kind
        && i == slot
    {
        return true;
    }
    let mut found = false;
    for_children_ref(e, &mut |c| found = found || uses_local(c, slot));
    found
}

/// Expressions with a new value at every use cannot be copied.
fn has_random(e: &Expr) -> bool {
    if matches!(e.kind, Kind::Sys(Sys::Random)) {
        return true;
    }
    let mut found = false;
    for_children_ref(e, &mut |c| found = found || has_random(c));
    found
}

fn for_children(e: &mut Expr, f: &mut dyn FnMut(&mut Expr)) {
    match &mut e.kind {
        Kind::Bool(_) | Kind::Int(_) | Kind::Real(_) | Kind::Local(_) | Kind::Param(_) | Kind::Sys(_) => {}
        Kind::Complex(a, b)
        | Kind::Arith(_, a, b)
        | Kind::Pow(a, b)
        | Kind::PowN(a, b)
        | Kind::Cmp(_, a, b)
        | Kind::And(a, b)
        | Kind::Or(a, b)
        | Kind::MergeParam(_, a, b) => {
            f(a);
            f(b);
        }
        Kind::Conv(a)
        | Kind::Neg(a)
        | Kind::Not(a)
        | Kind::PowI(a, _)
        | Kind::CallParam(_, a)
        | Kind::RandomInt(a)
        | Kind::Index(_, a)
        | Kind::Deriv(a, _) => f(a),
        Kind::Call(_, args) | Kind::ColorOp(_, args) => args.iter_mut().for_each(f),
    }
}

fn for_children_ref(e: &Expr, f: &mut dyn FnMut(&Expr)) {
    match &e.kind {
        Kind::Bool(_) | Kind::Int(_) | Kind::Real(_) | Kind::Local(_) | Kind::Param(_) | Kind::Sys(_) => {}
        Kind::Complex(a, b)
        | Kind::Arith(_, a, b)
        | Kind::Pow(a, b)
        | Kind::PowN(a, b)
        | Kind::Cmp(_, a, b)
        | Kind::And(a, b)
        | Kind::Or(a, b)
        | Kind::MergeParam(_, a, b) => {
            f(a);
            f(b);
        }
        Kind::Conv(a)
        | Kind::Neg(a)
        | Kind::Not(a)
        | Kind::PowI(a, _)
        | Kind::CallParam(_, a)
        | Kind::RandomInt(a)
        | Kind::Index(_, a)
        | Kind::Deriv(a, _) => f(a),
        Kind::Call(_, args) | Kind::ColorOp(_, args) => args.iter().for_each(f),
    }
}

// ---- Differentiation ----

/// The derivative of `e` with respect to local `v` (same type as `e`), without `deriv` nodes.
/// `funcs` gives the function each function parameter chooses.
pub fn derivative(e: &Expr, v: usize, funcs: &dyn Fn(usize) -> Func) -> Expr {
    let e = expand(e, funcs);
    D { v, funcs }.d(&e)
}

/// Replaces every `deriv` node in `e` by its derivative.
pub fn expand(e: &Expr, funcs: &dyn Fn(usize) -> Func) -> Expr {
    let mut e = e.clone();
    expand_in(&mut e, funcs);
    e
}

fn expand_in(e: &mut Expr, funcs: &dyn Fn(usize) -> Func) {
    for_children(e, &mut |c| expand_in(c, funcs));
    if let Kind::Deriv(inner, v) = &e.kind {
        *e = D { v: *v, funcs }.d(inner);
    }
}

struct D<'a> {
    v: usize,
    funcs: &'a dyn Fn(usize) -> Func,
}

fn ex(ty: Ty, kind: Kind) -> Expr {
    Expr { ty, kind }
}

fn bx(e: Expr) -> Box<Expr> {
    Box::new(e)
}

/// The constant `x` of a numeric type.
fn num(ty: Ty, x: f64) -> Expr {
    match ty {
        Ty::Int => ex(ty, Kind::Int(x as i64)),
        Ty::Complex => ex(ty, Kind::Complex(bx(num(Ty::Real, x)), bx(num(Ty::Real, 0.0)))),
        _ => ex(Ty::Real, Kind::Real(x)),
    }
}

fn zero(ty: Ty) -> Expr {
    num(ty, 0.0)
}

fn const_value(e: &Expr) -> Option<f64> {
    match &e.kind {
        Kind::Int(i) => Some(*i as f64),
        Kind::Real(x) => Some(*x),
        Kind::Complex(a, b) if const_value(b) == Some(0.0) => const_value(a),
        Kind::Conv(a) if matches!(a.ty, Ty::Int | Ty::Real) => const_value(a),
        _ => None,
    }
}

fn is_zero(e: &Expr) -> bool {
    const_value(e) == Some(0.0)
}

fn is_one(e: &Expr) -> bool {
    const_value(e) == Some(1.0)
}

fn add(a: Expr, b: Expr) -> Expr {
    if is_zero(&a) {
        return b;
    }
    if is_zero(&b) {
        return a;
    }
    ex(a.ty, Kind::Arith(Arith::Add, bx(a), bx(b)))
}

fn sub(a: Expr, b: Expr) -> Expr {
    if is_zero(&b) {
        return a;
    }
    if is_zero(&a) {
        return neg(b);
    }
    ex(a.ty, Kind::Arith(Arith::Sub, bx(a), bx(b)))
}

fn mul(a: Expr, b: Expr) -> Expr {
    if is_zero(&a) || is_zero(&b) {
        return zero(a.ty);
    }
    if is_one(&a) {
        return b;
    }
    if is_one(&b) {
        return a;
    }
    ex(a.ty, Kind::Arith(Arith::Mul, bx(a), bx(b)))
}

fn div(a: Expr, b: Expr) -> Expr {
    if is_zero(&a) {
        return zero(a.ty);
    }
    if is_one(&b) {
        return a;
    }
    ex(a.ty, Kind::Arith(Arith::Div, bx(a), bx(b)))
}

fn neg(a: Expr) -> Expr {
    if is_zero(&a) {
        return a;
    }
    if let Kind::Neg(inner) = a.kind {
        return *inner;
    }
    ex(a.ty, Kind::Neg(bx(a)))
}

/// `k · a` for a numeric constant.
fn scale(k: f64, a: Expr) -> Expr {
    let ty = a.ty;
    mul(num(ty, k), a)
}

fn conv(a: Expr, ty: Ty) -> Expr {
    if a.ty == ty {
        return a;
    }
    if is_zero(&a) {
        return zero(ty);
    }
    ex(ty, Kind::Conv(bx(a)))
}

/// A unary library function on a value of the argument's type (real overloads stay real).
fn call(f: Func, a: Expr) -> Expr {
    ex(a.ty, Kind::Call(f, vec![a]))
}

/// A function on a complex argument; for a real argument, the real part of the complex
/// function (sqrt and log leave the real axis).
fn call_c(f: Func, a: Expr) -> Expr {
    let ty = a.ty;
    conv(call(f, conv(a, Ty::Complex)), ty)
}

fn re(a: Expr) -> Expr {
    match a.ty {
        Ty::Complex => match a.kind {
            Kind::Complex(x, _) => *x,
            Kind::Conv(x) if x.ty == Ty::Real => *x,
            kind => ex(Ty::Real, Kind::Call(Func::Real, vec![ex(Ty::Complex, kind)])),
        },
        _ => a,
    }
}

fn im(a: Expr) -> Expr {
    match a.ty {
        Ty::Complex => match a.kind {
            Kind::Complex(_, y) => *y,
            Kind::Conv(x) if x.ty == Ty::Real => zero(Ty::Real),
            kind => ex(Ty::Real, Kind::Call(Func::Imag, vec![ex(Ty::Complex, kind)])),
        },
        _ => zero(Ty::Real),
    }
}

/// 1 where `a < b`, else 0 (real).
fn less(a: &Expr, b: &Expr) -> Expr {
    conv(ex(Ty::Bool, Kind::Cmp(Cmp::Lt, bx(a.clone()), bx(b.clone()))), Ty::Real)
}

/// The sign of a real value (0 at 0).
fn sign(a: &Expr) -> Expr {
    let z = zero(Ty::Real);
    sub(less(&z, a), less(a, &z))
}

/// `re(a)·re(da) + im(a)·im(da)` (real).
fn dot(a: &Expr, da: &Expr) -> Expr {
    add(mul(re(a.clone()), re(da.clone())), mul(im(a.clone()), im(da.clone())))
}

impl D<'_> {
    fn d(&self, e: &Expr) -> Expr {
        let ty = e.ty;
        if !matches!(ty, Ty::Real | Ty::Complex) {
            return zero(if ty == Ty::Int { Ty::Int } else { Ty::Real });
        }
        match &e.kind {
            Kind::Local(i) if *i == self.v => num(ty, 1.0),
            Kind::Bool(_) | Kind::Int(_) | Kind::Real(_) | Kind::Local(_) | Kind::Param(_) | Kind::Sys(_) => zero(ty),
            Kind::RandomInt(_) | Kind::ColorOp(..) | Kind::MergeParam(..) | Kind::Index(..) => zero(ty),
            Kind::Cmp(..) | Kind::And(..) | Kind::Or(..) | Kind::Not(_) => zero(ty),
            Kind::Complex(a, b) => {
                let (da, db) = (self.d(a), self.d(b));
                if is_zero(&da) && is_zero(&db) { zero(ty) } else { ex(ty, Kind::Complex(bx(da), bx(db))) }
            }
            Kind::Conv(a) => match a.ty {
                Ty::Real | Ty::Complex => conv(self.d(a), ty),
                _ => zero(ty),
            },
            Kind::Neg(a) => neg(self.d(a)),
            Kind::Arith(op, a, b) => {
                let (da, db) = (self.d(a), self.d(b));
                let (a, b) = ((**a).clone(), (**b).clone());
                match op {
                    Arith::Add => add(da, db),
                    Arith::Sub => sub(da, db),
                    Arith::Mul => add(mul(da, b), mul(a, db)),
                    Arith::Div => {
                        if is_zero(&db) {
                            div(da, b)
                        } else {
                            div(sub(mul(da, b.clone()), mul(a, db)), mul(b.clone(), b))
                        }
                    }
                    // Piecewise: a − b·trunc(a/b).
                    Arith::Mod => {
                        if is_zero(&db) || ty != Ty::Real {
                            da
                        } else {
                            sub(da, mul(db, call(Func::Trunc, div(a, b))))
                        }
                    }
                }
            }
            Kind::Pow(a, b) => {
                let (da, db) = (self.d(a), self.d(b));
                let (a, b) = ((**a).clone(), (**b).clone());
                let lower = ex(ty, Kind::Pow(bx(a.clone()), bx(sub(b.clone(), num(ty, 1.0)))));
                let base = mul(mul(b.clone(), lower), da);
                if is_zero(&db) {
                    base
                } else {
                    // d(a^b) = b·a^(b−1)·da + a^b·log(a)·db
                    let p = ex(ty, Kind::Pow(bx(a.clone()), bx(b)));
                    add(base, mul(mul(p, call_c(Func::Log, a)), db))
                }
            }
            Kind::PowI(a, n) => {
                let da = self.d(a);
                let lower = match n - 1 {
                    0 => num(ty, 1.0),
                    1 => (**a).clone(),
                    m => ex(ty, Kind::PowI(a.clone(), m)),
                };
                mul(scale(*n as f64, lower), da)
            }
            Kind::PowN(a, n) => {
                let da = self.d(a);
                let m = ex(Ty::Int, Kind::Arith(Arith::Sub, n.clone(), bx(num(Ty::Int, 1.0))));
                let lower = ex(ty, Kind::PowN(a.clone(), bx(m)));
                mul(mul(conv((**n).clone(), ty), lower), da)
            }
            Kind::Call(f, args) => self.call(*f, args, ty),
            Kind::CallParam(i, a) => {
                let f = (self.funcs)(*i);
                if f.returns_real() {
                    conv(self.call(f, std::slice::from_ref(a), Ty::Real), Ty::Complex)
                } else {
                    self.call(f, std::slice::from_ref(a), Ty::Complex)
                }
            }
            Kind::Deriv(inner, v) => self.d(&D { v: *v, funcs: self.funcs }.d(inner)),
        }
    }

    fn call(&self, f: Func, args: &[Expr], ty: Ty) -> Expr {
        match f {
            Func::Atan2 | Func::Min | Func::Max => {
                let (a, b) = (&args[0], &args[1]);
                let (da, db) = (self.d(a), self.d(b));
                if is_zero(&da) && is_zero(&db) {
                    return zero(Ty::Real);
                }
                match f {
                    // atan2(y, x)
                    Func::Atan2 => {
                        let n = add(mul(a.clone(), a.clone()), mul(b.clone(), b.clone()));
                        div(sub(mul(b.clone(), da), mul(a.clone(), db)), n)
                    }
                    // The back ends take b when b < a (min) or b > a (max).
                    _ => {
                        let t = if f == Func::Min { less(b, a) } else { less(a, b) };
                        add(mul(t.clone(), db), mul(sub(num(Ty::Real, 1.0), t), da))
                    }
                }
            }
            // Noise counts as constant (its slopes are not part of the language).
            Func::Noise | Func::Perlin | Func::Voronoi => zero(Ty::Real),
            Func::Real | Func::Imag | Func::Cabs | Func::Norm | Func::Arg => {
                let a = &args[0];
                let da = self.d(a);
                if is_zero(&da) {
                    return zero(Ty::Real);
                }
                match f {
                    Func::Real => re(da),
                    Func::Imag => im(da),
                    Func::Cabs => div(dot(a, &da), ex(Ty::Real, Kind::Call(Func::Cabs, vec![a.clone()]))),
                    Func::Norm => scale(2.0, dot(a, &da)),
                    _ => {
                        let cross = sub(mul(re(a.clone()), im(da.clone())), mul(im(a.clone()), re(da)));
                        div(cross, ex(Ty::Real, Kind::Call(Func::Norm, vec![a.clone()])))
                    }
                }
            }
            _ => {
                let a = conv(args[0].clone(), ty);
                let da = self.d(&a);
                if is_zero(&da) {
                    return zero(ty);
                }
                let one = || num(ty, 1.0);
                let sq = |x: Expr| mul(x.clone(), x);
                match f {
                    Func::Round | Func::Floor | Func::Ceil | Func::Trunc | Func::Zero => zero(ty),
                    Func::Ident => da,
                    Func::Conj | Func::Flip => call(f, da),
                    Func::Abs => match ty {
                        Ty::Complex => {
                            let x = mul(sign(&re(a.clone())), re(da.clone()));
                            let y = mul(sign(&im(a)), im(da));
                            ex(ty, Kind::Complex(bx(x), bx(y)))
                        }
                        _ => mul(sign(&a), da),
                    },
                    Func::Sqr => mul(scale(2.0, a), da),
                    Func::Sqrt => div(da, scale(2.0, call_c(Func::Sqrt, a))),
                    Func::Exp => mul(call(Func::Exp, a), da),
                    Func::Log => div(da, a),
                    Func::Recip => neg(div(da, sq(a))),
                    Func::Sin => mul(call(Func::Cos, a), da),
                    Func::Cos => neg(mul(call(Func::Sin, a), da)),
                    Func::Tan => div(da, sq(call(Func::Cos, a))),
                    Func::Cotan => neg(div(da, sq(call(Func::Sin, a)))),
                    Func::Asin => div(da, call_c(Func::Sqrt, sub(one(), sq(a)))),
                    Func::Acos => neg(div(da, call_c(Func::Sqrt, sub(one(), sq(a))))),
                    Func::Atan => div(da, add(one(), sq(a))),
                    Func::Sinh => mul(call(Func::Cosh, a), da),
                    Func::Cosh => mul(call(Func::Sinh, a), da),
                    Func::Tanh => div(da, sq(call(Func::Cosh, a))),
                    Func::Cotanh => neg(div(da, sq(call(Func::Sinh, a)))),
                    Func::Asinh => div(da, call_c(Func::Sqrt, add(sq(a), one()))),
                    Func::Acosh => {
                        let s = mul(call_c(Func::Sqrt, sub(a.clone(), one())), call_c(Func::Sqrt, add(a, one())));
                        div(da, s)
                    }
                    Func::Atanh => div(da, sub(one(), sq(a))),
                    // cosxx(z) = conj(cos(z))
                    Func::Cosxx => call(Func::Conj, neg(mul(call(Func::Sin, a), call(Func::Conj, da)))),
                    Func::Real
                    | Func::Imag
                    | Func::Cabs
                    | Func::Norm
                    | Func::Arg
                    | Func::Atan2
                    | Func::Min
                    | Func::Max
                    | Func::Noise
                    | Func::Perlin
                    | Func::Voronoi => unreachable!(),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formula::cpu;

    use crate::math::complex::C64;

    fn program(src: &str) -> Program {
        match &crate::formula::compile_ufm_file(src)[0].1 {
            Ok(p) => p.clone(),
            Err(e) => panic!("{src}: {e:?}"),
        }
    }

    /// `z` after `init` with `#pixel` = `x`.
    fn eval_init_z(prog: &Program, x: C64) -> C64 {
        let c = cpu::Compiled::<f64>::new(prog, &[]);
        let mut f = c.frame();
        f.pixel = x;
        c.run(&mut f, 0).z
    }

    /// Checks `deriv(f, x)` against a central difference of `f` at a few points.
    fn check_against_difference(f: &str, real: bool) {
        let (ty, x) = if real { ("float", "real(#pixel)") } else { ("complex", "#pixel") };
        let src = |d: &str| format!("T {{\ninit:\n  {ty} x = {x}\n  {ty} f = {f}\n  z = {d}\nloop:\n  z = z\nbailout:\n  false\n}}\n");
        let prog = program(&src("deriv(f, x)"));
        let fprog = program(&src("f"));
        for &(re, imv) in &[(0.31, 0.47), (-0.8, 0.25), (1.3, -0.6), (0.6, 0.2)] {
            let x = C64::new(re, if real { 0.0 } else { imv });
            let h = 1e-6;
            let hp = C64::new(h, 0.0);
            let num = (eval_init_z(&fprog, x + hp) - eval_init_z(&fprog, x - hp)) / C64::new(2.0 * h, 0.0);
            let got = eval_init_z(&prog, x);
            let err = (got - num).cabs() / (1.0 + num.cabs());
            assert!(err < 1e-5, "{f} at {x:?}: deriv {got:?}, difference {num:?}");
        }
    }

    #[test]
    fn complex_functions() {
        for f in [
            "x^3 - 2*x + 1",
            "(x + 1) / (x - 2)",
            "x^2.5",
            "x^(x/3)",
            "sqr(x) * exp(x)",
            "sqrt(x) + log(x)",
            "recip(x) + sin(x) * cos(x)",
            "tan(x) + cotan(x)",
            "asin(x / 2) + acos(x / 3) + atan(x)",
            "sinh(x) + cosh(x) * tanh(x) + cotanh(x)",
            "asinh(x) + acosh(x + 2) + atanh(x / 2)",
            "conj(x) * 2 + flip(x)",
            "abs(x) + cosxx(x)",
            "(real(x) + imag(x), cabs(x) + |x|)",
            "(atan2(x), 0)",
            "x^-3",
            "1 / x^4",
        ] {
            check_against_difference(f, false);
        }
    }

    #[test]
    fn real_functions() {
        for f in [
            "x^3 - 2*x + 1",
            "sin(x) * exp(x)",
            "abs(x - 1) + atan(x)",
            "min(x, 0.5) + max(x*x, 0.3)",
            "atan2(x, 2)",
            "asinh(x) + cosh(x)",
            "x^1.5",
            "x % 0.35",
            "1.7 % (x + 2)",
        ] {
            check_against_difference(f, true);
        }
    }

    #[test]
    fn follows_earlier_assignments_and_second_derivatives() {
        // f'' of x^4 + x through two derivs and a helper variable.
        let src = "T {\ninit:\n  complex x = #pixel\n  complex g = x * x\n  complex f = g * g + x\n  complex d = deriv(f, x)\n  complex dd = deriv(d, x)\n  z = dd\nloop:\n  z = z\nbailout:\n  false\n}\n";
        let x = C64::new(0.7, -0.3);
        let got = eval_init_z(&program(src), x);
        let want = x * x * C64::new(12.0, 0.0);
        assert!((got - want).cabs() < 1e-9, "{got:?} vs {want:?}");
    }

    #[test]
    fn reassigned_variables_count_as_constants() {
        // g changes under a condition: deriv(g, x) cannot follow it.
        let src = "T {\ninit:\n  complex x = #pixel\n  complex g = x * x\n  if real(x) > 100\n    g = 1\n  endif\n  z = deriv(g, x)\nloop:\n  z = z\nbailout:\n  false\n}\n";
        let got = eval_init_z(&program(src), C64::new(0.7, -0.3));
        assert_eq!(got.cabs(), 0.0);
    }
}
