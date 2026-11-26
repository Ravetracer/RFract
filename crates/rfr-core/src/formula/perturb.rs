// SPDX-License-Identifier: LGPL-3.0-or-later

//! Perturbation for deep zooms, derived from a fractal program's IR.
//!
//! One reference orbit at the view center runs in arbitrary precision; every sample iterates
//! only its difference ("delta") to the reference, in floats with an extended exponent:
//! `f(Z + δ, C + δc) − f(Z, C)` follows from the expression rules below (sums, products,
//! integer powers, conjugates, parts and `abs` through "diffabs"), so no formula is written
//! twice. Rebasing (restart at the orbit's start when the sample's value gets smaller than
//! its delta, or when the reference ends) keeps the deltas valid without glitch detection.
//!
//! Supported: fractal programs whose `init` and `loop` only assign real or complex variables
//! with those operations; conditions must be known before rendering (`#ismand`, parameters).

use super::ast::ParamValue;
use super::ir::*;
use crate::math::big::{self, Big};
use crate::math::complex::C64;
use crate::math::fx::{Cfx, Fx};
use std::ops::{Add, Div, Mul, Neg, Sub};

/// A perturbable expression (values are complex; reals have a zero imaginary part).
#[derive(Clone, Debug)]
pub enum PE {
    Const(C64),
    /// A real or complex local.
    Local(usize),
    /// The sample point (reference: the view center).
    Pixel,
    Add(Box<PE>, Box<PE>),
    Sub(Box<PE>, Box<PE>),
    Mul(Box<PE>, Box<PE>),
    Div(Box<PE>, Box<PE>),
    Neg(Box<PE>),
    PowI(Box<PE>, u32),
    Conj(Box<PE>),
    /// Absolute value of both parts.
    Abs(Box<PE>),
    Re(Box<PE>),
    Im(Box<PE>),
    Flip(Box<PE>),
    /// A complex from the real parts of two values.
    MakeC(Box<PE>, Box<PE>),
}

/// A fractal program lowered for perturbation.
#[derive(Clone, Debug)]
pub struct Perturbation {
    pub init: Vec<(usize, PE)>,
    pub body: Vec<(usize, PE)>,
    /// The locals the orbit carries (stored per iteration of the reference).
    pub state: Vec<usize>,
    pub nlocals: usize,
}

/// The reference orbit: the state locals after `init` (index 0) and after every iteration.
#[derive(Clone, Debug)]
pub struct RefOrbit {
    pub values: Vec<C64>,
    /// Number of stored states.
    pub len: usize,
    pub nstate: usize,
    /// The reference point (the view center) as `f64`.
    pub center: C64,
}

impl RefOrbit {
    pub fn state(&self, n: usize) -> &[C64] {
        &self.values[n * self.nstate..(n + 1) * self.nstate]
    }
}

/// A sample's deltas and its position in the reference orbit.
#[derive(Clone, Debug)]
pub struct DeltaState {
    pub m: usize,
    /// By local slot (only the state locals matter).
    pub delta: Vec<Cfx>,
    refs: Vec<C64>,
}

struct Lower<'a> {
    prog: &'a Program,
    values: &'a [ParamValue],
    ismand: bool,
    seed: C64,
    assigned: Vec<bool>,
}

#[derive(Clone, Copy)]
enum K {
    Num(C64),
    Bool(bool),
}

impl Lower<'_> {
    fn func(&self, f: Func) -> Result<Func, String> {
        Ok(f)
    }

    fn param_func(&self, i: usize) -> Func {
        let name = match self.values.get(i) {
            Some(ParamValue::Func(f)) => Some(f.clone()),
            _ => None,
        }
        .or(match &self.prog.params.get(i).map(|p| &p.default) {
            Some(ParamValue::Func(f)) => Some(f.clone()),
            _ => None,
        });
        name.and_then(|n| Func::by_name(&n)).unwrap_or(Func::Ident)
    }

    /// The value of an expression known before rendering (parameters, `#ismand`).
    fn konst(&self, e: &Expr) -> Option<K> {
        let num = |k: Option<K>| match k {
            Some(K::Num(c)) => Some(c),
            Some(K::Bool(b)) => Some(C64::new(b as i64 as f64, 0.0)),
            None => None,
        };
        Some(match &e.kind {
            Kind::Bool(b) => K::Bool(*b),
            Kind::Int(i) => K::Num(C64::new(*i as f64, 0.0)),
            Kind::Real(v) => K::Num(C64::new(*v, 0.0)),
            Kind::Complex(a, b) => K::Num(C64::new(num(self.konst(a))?.re, num(self.konst(b))?.re)),
            Kind::Param(i) => {
                let v = self.values.get(*i).unwrap_or(&self.prog.params[*i].default);
                match (e.ty, v) {
                    (_, ParamValue::Func(_) | ParamValue::Class { .. } | ParamValue::Image { .. } | ParamValue::Color(_)) => return None,
                    (Ty::Bool, v) => K::Bool(v.as_f64() != 0.0),
                    (_, v) => {
                        let c = v.as_complex();
                        K::Num(C64::new(c[0], if e.ty == Ty::Complex { c[1] } else { 0.0 }))
                    }
                }
            }
            Kind::Sys(Sys::IsMand) => K::Bool(self.ismand),
            Kind::Conv(a) => {
                let v = self.konst(a)?;
                match (e.ty, v) {
                    (Ty::Bool, K::Num(c)) => K::Bool(c.re != 0.0),
                    (Ty::Bool, b) => b,
                    (Ty::Int, k) => K::Num(C64::new(num(Some(k))?.re.trunc(), 0.0)),
                    (Ty::Real, k) => K::Num(C64::new(num(Some(k))?.re, 0.0)),
                    (_, k) => K::Num(num(Some(k))?),
                }
            }
            Kind::Neg(a) => K::Num(-num(self.konst(a))?),
            Kind::Not(a) => match self.konst(a)? {
                K::Bool(b) => K::Bool(!b),
                K::Num(c) => K::Bool(c.re == 0.0),
            },
            Kind::And(a, b) => match (self.konst(a)?, self.konst(b)?) {
                (K::Bool(x), K::Bool(y)) => K::Bool(x && y),
                _ => return None,
            },
            Kind::Or(a, b) => match (self.konst(a)?, self.konst(b)?) {
                (K::Bool(x), K::Bool(y)) => K::Bool(x || y),
                _ => return None,
            },
            Kind::Arith(op, a, b) => {
                let (x, y) = (num(self.konst(a))?, num(self.konst(b))?);
                K::Num(match op {
                    Arith::Add => x + y,
                    Arith::Sub => x - y,
                    Arith::Mul => x * y,
                    Arith::Div if e.ty == Ty::Int => C64::new((x.re as i64).checked_div(y.re as i64)? as f64, 0.0),
                    Arith::Div => x / y,
                    Arith::Mod => return None,
                })
            }
            Kind::Cmp(op, a, b) => {
                let (x, y) = (num(self.konst(a))?, num(self.konst(b))?);
                K::Bool(match op {
                    Cmp::Lt => x.re < y.re,
                    Cmp::Le => x.re <= y.re,
                    Cmp::Gt => x.re > y.re,
                    Cmp::Ge => x.re >= y.re,
                    Cmp::Eq => x == y,
                    Cmp::Ne => x != y,
                })
            }
            _ => return None,
        })
    }

    fn lower(&self, e: &Expr) -> Result<PE, String> {
        if e.ty != Ty::Bool
            && let Some(K::Num(c)) = self.konst(e)
        {
            return Ok(PE::Const(c));
        }
        let b = |x: &Expr| self.lower(x).map(Box::new);
        Ok(match &e.kind {
            Kind::Local(i) => {
                let l = &self.prog.locals[*i];
                if l.cap.is_some() || !matches!(l.ty, Ty::Real | Ty::Complex) {
                    return Err(format!("uses the {} variable '{}'", ty_name(l.ty), l.name));
                }
                if self.assigned[*i] { PE::Local(*i) } else { PE::Const(C64::new(0.0, 0.0)) }
            }
            Kind::Sys(Sys::Pixel) => PE::Pixel,
            Kind::Sys(Sys::C) => {
                if self.ismand {
                    PE::Pixel
                } else {
                    PE::Const(self.seed)
                }
            }
            Kind::Sys(s) => return Err(format!("uses #{s:?}").to_lowercase()),
            Kind::Conv(a) => match (e.ty, a.ty) {
                (Ty::Real, Ty::Complex) => PE::Re(b(a)?),
                (Ty::Real | Ty::Complex, Ty::Real | Ty::Complex) => self.lower(a)?,
                _ => return Err("converts integers or booleans that change while iterating".into()),
            },
            Kind::Complex(x, y) => PE::MakeC(b(x)?, b(y)?),
            Kind::Neg(a) => PE::Neg(b(a)?),
            Kind::Arith(op, x, y) => match op {
                Arith::Add => PE::Add(b(x)?, b(y)?),
                Arith::Sub => PE::Sub(b(x)?, b(y)?),
                Arith::Mul => PE::Mul(b(x)?, b(y)?),
                Arith::Div => PE::Div(b(x)?, b(y)?),
                Arith::Mod => return Err("uses % (modulo)".into()),
            },
            Kind::PowI(x, n) => self.pow(self.lower(x)?, *n as i64)?,
            Kind::PowN(x, n) | Kind::Pow(x, n) => match self.konst(n) {
                Some(K::Num(c)) if c.im == 0.0 && c.re.fract() == 0.0 && c.re.abs() <= 64.0 => self.pow(self.lower(x)?, c.re as i64)?,
                _ => return Err("uses a power that is not a whole number".into()),
            },
            Kind::Call(f, args) if args.len() == 1 => self.call(self.func(*f)?, &args[0])?,
            Kind::CallParam(i, a) => self.call(self.param_func(*i), a)?,
            _ => return Err(format!("uses an operation without perturbation rule ({})", kind_name(&e.kind))),
        })
    }

    fn pow(&self, x: PE, n: i64) -> Result<PE, String> {
        Ok(match n {
            0 => PE::Const(C64::new(1.0, 0.0)),
            1 => x,
            n if n > 0 => PE::PowI(Box::new(x), n as u32),
            n => PE::Div(Box::new(PE::Const(C64::new(1.0, 0.0))), Box::new(PE::PowI(Box::new(x), (-n) as u32))),
        })
    }

    fn call(&self, f: Func, a: &Expr) -> Result<PE, String> {
        let x = Box::new(self.lower(a)?);
        Ok(match f {
            Func::Sqr => PE::PowI(x, 2),
            Func::Conj => PE::Conj(x),
            Func::Abs => PE::Abs(x),
            Func::Real => PE::Re(x),
            Func::Imag => PE::Im(x),
            Func::Flip => PE::Flip(x),
            Func::Ident => *x,
            Func::Recip => PE::Div(Box::new(PE::Const(C64::new(1.0, 0.0))), x),
            Func::Zero => PE::Const(C64::new(0.0, 0.0)),
            f => return Err(format!("uses the function {f:?} (no perturbation rule)").to_lowercase()),
        })
    }

    fn stmts(&self, list: &[Stmt], out: &mut Vec<(usize, PE)>) -> Result<(), String> {
        for s in list {
            match s {
                Stmt::Assign(i, e) => {
                    let l = &self.prog.locals[*i];
                    if l.cap.is_some() || !matches!(l.ty, Ty::Real | Ty::Complex) {
                        return Err(format!("assigns the {} variable '{}'", ty_name(l.ty), l.name));
                    }
                    out.push((*i, self.lower(e)?));
                }
                Stmt::SetPart { local, re, value } => {
                    let v = Box::new(self.lower(value)?);
                    let old = Box::new(PE::Local(*local));
                    let e = if *re { PE::MakeC(v, Box::new(PE::Im(old))) } else { PE::MakeC(Box::new(PE::Re(old)), v) };
                    out.push((*local, e));
                }
                Stmt::If(arms, els) => {
                    let mut taken = None;
                    for (c, body) in arms {
                        match self.konst(c) {
                            Some(K::Bool(true)) => {
                                taken = Some(body);
                                break;
                            }
                            Some(K::Bool(false)) => {}
                            _ => return Err("has a condition that depends on the orbit".into()),
                        }
                    }
                    self.stmts(taken.unwrap_or(els), out)?;
                }
                Stmt::SetIndex(..) => return Err("uses arrays".into()),
                Stmt::While(..) | Stmt::Repeat(..) => return Err("has an inner loop".into()),
            }
        }
        Ok(())
    }
}

fn ty_name(t: Ty) -> &'static str {
    match t {
        Ty::Bool => "boolean",
        Ty::Int => "integer",
        Ty::Real => "real",
        Ty::Complex => "complex",
        Ty::Color => "color",
    }
}

fn kind_name(k: &Kind) -> &'static str {
    match k {
        Kind::Cmp(..) => "comparison",
        Kind::Call(..) => "function",
        Kind::RandomInt(..) => "random",
        Kind::ColorOp(..) | Kind::MergeParam(..) => "color",
        Kind::Index(..) => "array",
        Kind::And(..) | Kind::Or(..) | Kind::Not(..) => "logic",
        _ => "expression",
    }
}

fn assigned_in(list: &[Stmt], out: &mut [bool]) {
    for s in list {
        match s {
            Stmt::Assign(i, _) | Stmt::SetPart { local: i, .. } | Stmt::SetIndex(i, ..) => out[*i] = true,
            Stmt::If(arms, els) => {
                arms.iter().for_each(|(_, b)| assigned_in(b, out));
                assigned_in(els, out);
            }
            Stmt::While(_, b) | Stmt::Repeat(b, _) => assigned_in(b, out),
        }
    }
}

// ---- arithmetic of the three evaluations ----

#[derive(Clone)]
struct BigC {
    re: Big,
    im: Big,
}

impl BigC {
    fn zero(bits: usize) -> BigC {
        BigC { re: big::from_fx(Fx::ZERO, bits), im: big::from_fx(Fx::ZERO, bits) }
    }
    fn from_c64(c: C64, bits: usize) -> BigC {
        BigC { re: big::from_fx(Fx::from_f64(c.re), bits), im: big::from_fx(Fx::from_f64(c.im), bits) }
    }
    fn to_c64(&self) -> C64 {
        C64::new(big::to_fx(&self.re).to_f64(), big::to_fx(&self.im).to_f64())
    }
    fn add(&self, o: &BigC) -> BigC {
        BigC { re: &self.re + &o.re, im: &self.im + &o.im }
    }
    fn sub(&self, o: &BigC) -> BigC {
        BigC { re: &self.re - &o.re, im: &self.im - &o.im }
    }
    fn mul(&self, o: &BigC) -> BigC {
        BigC { re: &self.re * &o.re - &self.im * &o.im, im: &self.re * &o.im + &self.im * &o.re }
    }
    fn div(&self, o: &BigC) -> BigC {
        let d = &o.re * &o.re + &o.im * &o.im;
        BigC { re: (&self.re * &o.re + &self.im * &o.im) / &d, im: (&self.im * &o.re - &self.re * &o.im) / &d }
    }
    fn neg(&self) -> BigC {
        BigC { re: -self.re.clone(), im: -self.im.clone() }
    }
    fn round(self, bits: usize) -> BigC {
        BigC { re: self.re.with_precision(bits).value(), im: self.im.with_precision(bits).value() }
    }
}

fn big_abs(x: &Big) -> Big {
    if x.sign() == dashu_base::Sign::Negative { -x.clone() } else { x.clone() }
}

/// `|R + D| − |R|` without cancellation.
fn diffabs(r: f64, d: Fx) -> Fx {
    let rd = Fx::from_f64(r) + d;
    if r >= 0.0 {
        if rd.m >= 0.0 { d } else { -(Fx::from_f64(2.0 * r) + d) }
    } else if rd.m > 0.0 {
        Fx::from_f64(2.0 * r) + d
    } else {
        -d
    }
}

fn cfx(c: C64) -> Cfx {
    Cfx::from_c64(c.re, c.im)
}

impl Perturbation {
    /// Lowers a fractal program; `Err` says why it cannot be perturbed.
    pub fn new(prog: &Program, values: &[ParamValue], ismand: bool, seed: [f64; 2]) -> Result<Perturbation, String> {
        if prog.kind != super::ast::ProgramKind::Fractal {
            return Err("not a fractal formula".into());
        }
        let mut assigned = vec![false; prog.locals.len()];
        assigned_in(&prog.init, &mut assigned);
        assigned_in(&prog.body, &mut assigned);
        let l = Lower { prog, values, ismand, seed: C64::new(seed[0], seed[1]), assigned };
        let mut init = Vec::new();
        let mut body = Vec::new();
        l.stmts(&prog.init, &mut init)?;
        l.stmts(&prog.body, &mut body)?;
        let state: Vec<usize> = (0..prog.locals.len()).filter(|&i| l.assigned[i]).collect();
        if !state.contains(&Z) {
            return Err("does not iterate z".into());
        }
        Ok(Perturbation { init, body, state, nlocals: prog.locals.len() })
    }

    fn eval_big(&self, e: &PE, locals: &[BigC], pixel: &BigC, bits: usize) -> BigC {
        let ev = |x: &PE| self.eval_big(x, locals, pixel, bits);
        match e {
            PE::Const(c) => BigC::from_c64(*c, bits),
            PE::Local(i) => locals[*i].clone(),
            PE::Pixel => pixel.clone(),
            PE::Add(a, b) => ev(a).add(&ev(b)),
            PE::Sub(a, b) => ev(a).sub(&ev(b)),
            PE::Mul(a, b) => ev(a).mul(&ev(b)).round(bits),
            PE::Div(a, b) => ev(a).div(&ev(b)).round(bits),
            PE::Neg(a) => ev(a).neg(),
            PE::PowI(a, n) => {
                let x = ev(a);
                let mut r = x.clone();
                for _ in 1..*n {
                    r = r.mul(&x).round(bits);
                }
                r
            }
            PE::Conj(a) => {
                let x = ev(a);
                BigC { re: x.re, im: -x.im }
            }
            PE::Abs(a) => {
                let x = ev(a);
                BigC { re: big_abs(&x.re), im: big_abs(&x.im) }
            }
            PE::Re(a) => {
                let x = ev(a);
                BigC { re: x.re, im: big::from_fx(Fx::ZERO, bits) }
            }
            PE::Im(a) => {
                let x = ev(a);
                BigC { re: x.im, im: big::from_fx(Fx::ZERO, bits) }
            }
            PE::Flip(a) => {
                let x = ev(a);
                BigC { re: x.im, im: x.re }
            }
            PE::MakeC(a, b) => BigC { re: ev(a).re, im: ev(b).re },
        }
    }

    /// The reference orbit at `center` (decimal text) with `bits` of precision, until
    /// `maxit` iterations or until `escaped` (called with the full locals as `f64`, by slot)
    /// says the orbit left.
    pub fn reference(&self, center: &[String; 2], bits: usize, maxit: u32, mut escaped: impl FnMut(&[C64]) -> bool) -> RefOrbit {
        let pixel = BigC { re: big::parse(&center[0], bits), im: big::parse(&center[1], bits) };
        let mut locals = vec![BigC::zero(bits); self.nlocals];
        for (i, e) in &self.init {
            locals[*i] = self.eval_big(e, &locals, &pixel, bits);
        }
        let nstate = self.state.len();
        let mut values = Vec::with_capacity(nstate * (maxit as usize + 1).min(1 << 20));
        let mut f64s = vec![C64::new(0.0, 0.0); self.nlocals];
        let mut store = |locals: &[BigC], values: &mut Vec<C64>| {
            for &s in &self.state {
                f64s[s] = locals[s].to_c64();
                values.push(f64s[s]);
            }
            escaped(&f64s)
        };
        let mut len = 1;
        let mut out = store(&locals, &mut values);
        while !out && len <= maxit as usize {
            for (i, e) in &self.body {
                locals[*i] = self.eval_big(e, &locals, &pixel, bits);
            }
            out = store(&locals, &mut values);
            len += 1;
        }
        RefOrbit { values, len, nstate, center: pixel.to_c64() }
    }

    /// Reference value and delta of an expression.
    fn eval_delta(&self, e: &PE, refs: &[C64], delta: &[Cfx], center: C64, dpix: Cfx) -> (C64, Cfx) {
        let ev = |x: &PE| self.eval_delta(x, refs, delta, center, dpix);
        match e {
            PE::Const(c) => (*c, Cfx::ZERO),
            PE::Local(i) => (refs[*i], delta[*i]),
            PE::Pixel => (center, dpix),
            PE::Add(a, b) => {
                let ((ra, da), (rb, db)) = (ev(a), ev(b));
                (ra + rb, da.add(db))
            }
            PE::Sub(a, b) => {
                let ((ra, da), (rb, db)) = (ev(a), ev(b));
                (ra - rb, da.sub(db))
            }
            PE::Mul(a, b) => {
                let ((ra, da), (rb, db)) = (ev(a), ev(b));
                (ra * rb, cfx(ra).mul(db).add(da.mul(cfx(rb))).add(da.mul(db)))
            }
            PE::Div(a, b) => {
                let ((ra, da), (rb, db)) = (ev(a), ev(b));
                // (Da·Rb − Ra·Db) / (Rb·(Rb + Db))
                let num = da.mul(cfx(rb)).sub(cfx(ra).mul(db));
                let den = cfx(rb).mul(cfx(rb).add(db));
                (ra / rb, num.div(den))
            }
            PE::Neg(a) => {
                let (r, d) = ev(a);
                (-r, d.neg())
            }
            PE::PowI(a, n) => {
                let (r, d) = ev(a);
                // Repeated products with the product rule: no cancellation.
                let (mut pr, mut pd) = (r, d);
                for _ in 1..*n {
                    pd = cfx(pr).mul(d).add(pd.mul(cfx(r))).add(pd.mul(d));
                    pr = pr * r;
                }
                (pr, pd)
            }
            PE::Conj(a) => {
                let (r, d) = ev(a);
                (r.conj(), d.conj())
            }
            PE::Abs(a) => {
                let (r, d) = ev(a);
                (C64::new(r.re.abs(), r.im.abs()), Cfx::from_fx(diffabs(r.re, d.re_fx()), diffabs(r.im, d.im_fx())))
            }
            PE::Re(a) => {
                let (r, d) = ev(a);
                (C64::new(r.re, 0.0), Cfx::from_fx(d.re_fx(), Fx::ZERO))
            }
            PE::Im(a) => {
                let (r, d) = ev(a);
                (C64::new(r.im, 0.0), Cfx::from_fx(d.im_fx(), Fx::ZERO))
            }
            PE::Flip(a) => {
                let (r, d) = ev(a);
                (C64::new(r.im, r.re), Cfx { re: d.im, im: d.re, e: d.e })
            }
            PE::MakeC(a, b) => {
                let ((ra, da), (rb, db)) = (ev(a), ev(b));
                (C64::new(ra.re, rb.re), Cfx::from_fx(da.re_fx(), db.re_fx()))
            }
        }
    }

    /// Starts a sample at offset `dpix` from the reference point.
    pub fn start(&self, orbit: &RefOrbit, dpix: Cfx) -> DeltaState {
        let mut st = DeltaState { m: 0, delta: vec![Cfx::ZERO; self.nlocals], refs: vec![C64::new(0.0, 0.0); self.nlocals] };
        for (i, e) in &self.init {
            let (r, d) = self.eval_delta(e, &st.refs, &st.delta, orbit.center, dpix);
            st.refs[*i] = r;
            st.delta[*i] = d;
        }
        st
    }

    /// One iteration of a sample; rebases when its value gets smaller than its delta or the
    /// reference ends.
    pub fn step(&self, orbit: &RefOrbit, st: &mut DeltaState, dpix: Cfx) {
        for (k, &s) in self.state.iter().enumerate() {
            st.refs[s] = orbit.state(st.m)[k];
        }
        for (i, e) in &self.body {
            let (r, d) = self.eval_delta(e, &st.refs, &st.delta, orbit.center, dpix);
            st.refs[*i] = r;
            st.delta[*i] = d;
        }
        st.m += 1;
        let zk = self.state.iter().position(|&s| s == Z).unwrap_or(0);
        let zref = orbit.state(st.m.min(orbit.len - 1))[zk];
        let full_z = cfx(zref).add(st.delta[Z]);
        if st.m + 1 >= orbit.len || full_z.norm() < st.delta[Z].norm() {
            let at = st.m.min(orbit.len - 1);
            for (k, &s) in self.state.iter().enumerate() {
                let full = cfx(orbit.state(at)[k]).add(st.delta[s]);
                st.delta[s] = full.sub(cfx(orbit.state(0)[k]));
            }
            st.m = 0;
        }
    }

    /// The full value of every state local (`f64`), by slot.
    pub fn full(&self, orbit: &RefOrbit, st: &DeltaState, out: &mut [C64]) {
        let at = orbit.state(st.m);
        for (k, &s) in self.state.iter().enumerate() {
            let (re, im) = st.delta[s].to_c64();
            out[s] = C64::new(at[k].re + re, at[k].im + im);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formula::cpu::Compiled;
    use crate::formula::library::library;

    /// Perturbed iteration counts agree with exact (bignum) iteration of the same points.
    fn agrees(id: &str, center: [&str; 2], radius: f64, maxit: u32) {
        let lib = library();
        let e = lib.get(id).unwrap();
        let prog = e.program().unwrap();
        let values: Vec<ParamValue> = prog.params.iter().map(|p| p.default.clone()).collect();
        let k = Compiled::<f64>::new(&prog, &values);
        let pert = Perturbation::new(&prog, &values, true, [0.0, 0.0]).unwrap();
        let center_text = [center[0].to_string(), center[1].to_string()];
        let orbit = k.reference_orbit(&pert, &center_text, 200, maxit);
        let (mut same, mut total) = (0, 0);
        for j in 0..10 {
            for i in 0..10 {
                let d = [(i as f64 - 4.5) / 5.0 * radius, (j as f64 - 4.5) / 5.0 * radius];
                let mut g = k.frame();
                g.ismand = true;
                let p = k.run_perturbed(&mut g, maxit, &pert, &orbit, Cfx::from_c64(d[0], d[1]), |_| {});
                // The exact escape of the exact point (center + d).
                let scale = Fx::from_f64(radius * 1e-6);
                let exact = [big::add(center[0], Fx::from_f64(d[0]), scale), big::add(center[1], Fx::from_f64(d[1]), scale)];
                let o = k.reference_orbit(&pert, &exact, 300, maxit);
                let exact_iter = (o.len - 1) as u32;
                total += 1;
                if p.iter == exact_iter || (exact_iter == maxit && !p.escaped) {
                    same += 1;
                } else if std::env::var("RFR_PERT_DEBUG").is_ok() {
                    eprintln!("{id} {i},{j}: perturbed {} exact {exact_iter}", p.iter);
                }
            }
        }
        assert!(same * 100 >= total * 95, "{id}: {same} of {total} agree");
    }

    #[test]
    fn perturbation_matches_direct_iteration() {
        agrees("rfract:Mandelbrot", ["-0.743643887037151", "0.131825904205330"], 1e-9, 2000);
        agrees("rfract:BurningShip", ["-1.7621", "-0.0281"], 1e-6, 500);
        agrees("rfract:Celtic", ["-0.6", "0.4"], 1e-5, 300);
    }

    #[test]
    fn deep_reference_orbit() {
        // Far below f64: the reference still runs; deltas of 1e-500 iterate.
        let lib = library();
        let e = lib.get("rfract:Mandelbrot").unwrap();
        let prog = e.program().unwrap();
        let values: Vec<ParamValue> = prog.params.iter().map(|p| p.default.clone()).collect();
        let k = Compiled::<f64>::new(&prog, &values);
        let pert = Perturbation::new(&prog, &values, true, [0.0, 0.0]).unwrap();
        let center = ["-1.75".to_string(), "0".to_string()];
        let orbit = k.reference_orbit(&pert, &center, big::bits_for(Fx::parse("1e-500").unwrap()), 1000);
        assert_eq!(orbit.len, 1001, "-1.75 is inside the set");
        let mut f = k.frame();
        let d = Cfx::from_fx(Fx::parse("3e-500").unwrap(), Fx::ZERO);
        let o = k.run_perturbed(&mut f, 1000, &pert, &orbit, d, |_| {});
        assert!(!o.escaped);
    }
}
