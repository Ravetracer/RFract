// SPDX-License-Identifier: LGPL-3.0-or-later

//! CPU back end: compiles the typed IR into closures over a per-sample [`Frame`]. Generic over
//! the scalar: `f64` for plain renders, [`Dual`](crate::math::scalar::Dual) when the coloring
//! needs the derivative (distance estimation). The GPU shader generator mirrors every rule here.

use crate::math::complex::Cx;
use crate::math::scalar::Scalar;

use super::ast::ParamValue;
use super::ir::*;

/// Per-sample state.
#[derive(Clone, Debug)]
pub struct Frame<S> {
    pub bools: Vec<bool>,
    pub ints: Vec<i64>,
    pub reals: Vec<S>,
    pub cx: Vec<Cx<S>>,
    pub pixel: Cx<S>,
    pub c: Cx<S>,
    pub zprev: Cx<S>,
    pub lastsqr: S,
    pub numiter: i64,
    pub maxit: i64,
    pub ismand: bool,
    pub center: Cx<S>,
    pub magn: S,
    /// The fractal's current `z` (read by colorings as `#z`).
    pub zcur: Cx<S>,
}

type BFn<S> = Box<dyn Fn(&Frame<S>) -> bool + Send + Sync>;
type IFn<S> = Box<dyn Fn(&Frame<S>) -> i64 + Send + Sync>;
type RFn<S> = Box<dyn Fn(&Frame<S>) -> S + Send + Sync>;
type CFn<S> = Box<dyn Fn(&Frame<S>) -> Cx<S> + Send + Sync>;
type SFn<S> = Box<dyn Fn(&mut Frame<S>) + Send + Sync>;

enum E<S> {
    B(BFn<S>),
    I(IFn<S>),
    R(RFn<S>),
    C(CFn<S>),
}

/// How the program's locals map onto the frame's typed vectors.
#[derive(Clone, Debug)]
struct Layout {
    slot: Vec<(Ty, usize)>,
    counts: [usize; 4],
}

/// Outcome of iterating one sample.
#[derive(Clone, Copy, Debug)]
pub struct Orbit<S> {
    pub z: Cx<S>,
    pub zprev: Cx<S>,
    /// `z` two iterations before the end (convergent smoothing).
    pub zprev2: Cx<S>,
    /// Completed iterations.
    pub iter: u32,
    /// The bailout condition ended the orbit (or a value stopped being finite).
    pub escaped: bool,
}

/// Events of [`Compiled::run_with`].
pub enum Hook<S> {
    /// After the fractal's `init`, with `z`.
    Init(Cx<S>),
    /// After an iteration that continues: iteration count, `z`, `z` before the iteration.
    Iter(u32, Cx<S>, Cx<S>),
}

/// A program with its parameter values and function choices baked in.
pub struct Compiled<S> {
    layout: Layout,
    init: Vec<SFn<S>>,
    body: Vec<SFn<S>>,
    bailout: BFn<S>,
    fin: Vec<SFn<S>>,
    index_slot: Option<usize>,
    solid_slot: Option<usize>,
}

/// Real `x^y`: positive bases through exp/log, zero gives zero, negative bases use the real
/// part of the principal complex power.
pub fn real_pow<S: Scalar>(x: S, y: S) -> S {
    if x.v() > 0.0 {
        (y * x.ln()).exp()
    } else if x.v() == 0.0 {
        S::zero()
    } else {
        Cx::real(x).pow(Cx::real(y)).re
    }
}

pub fn real_powi<S: Scalar>(x: S, n: i32) -> S {
    let mut r = S::one();
    let mut b = x;
    let mut e = n.unsigned_abs();
    while e > 0 {
        if e & 1 == 1 {
            r = r * b;
        }
        b = b * b;
        e >>= 1;
    }
    if n < 0 { S::one() / r } else { r }
}

/// Real overload of a real-closed library function.
pub fn real_fn<S: Scalar>(f: Func, x: S) -> S {
    match f {
        Func::Abs => x.abs(),
        Func::Sqr => x * x,
        Func::Exp => x.exp(),
        Func::Recip => S::one() / x,
        Func::Conj | Func::Ident => x,
        Func::Round => x.round(),
        Func::Floor => x.floor(),
        Func::Ceil => x.ceil(),
        Func::Trunc => x.trunc(),
        Func::Sin => x.sin(),
        Func::Cos | Func::Cosxx => x.cos(),
        Func::Tan => x.sin() / x.cos(),
        Func::Cotan => x.cos() / x.sin(),
        Func::Atan => x.atan2(S::one()),
        Func::Sinh => x.sinh(),
        Func::Cosh => x.cosh(),
        Func::Tanh => x.sinh() / x.cosh(),
        Func::Cotanh => x.cosh() / x.sinh(),
        Func::Asinh => (x + (x * x + S::one()).sqrt()).ln(),
        Func::Zero => S::zero(),
        other => complex_fn(other, Cx::real(x)).re,
    }
}

/// A unary library function on a complex value; complex → real functions return (value, 0).
pub fn complex_fn<S: Scalar>(f: Func, z: Cx<S>) -> Cx<S> {
    match f {
        Func::Abs => z.abs(),
        Func::Sqr => z.sqr(),
        Func::Sqrt => z.sqrt(),
        Func::Exp => z.exp(),
        Func::Log => z.log(),
        Func::Recip => z.recip(),
        Func::Conj => z.conj(),
        Func::Flip => z.flip(),
        Func::Round => z.round(),
        Func::Floor => z.floor(),
        Func::Ceil => z.ceil(),
        Func::Trunc => z.trunc(),
        Func::Sin => z.sin(),
        Func::Cos => z.cos(),
        Func::Tan => z.tan(),
        Func::Cotan => z.cotan(),
        Func::Asin => z.asin(),
        Func::Acos => z.acos(),
        Func::Atan => z.atan(),
        Func::Sinh => z.sinh(),
        Func::Cosh => z.cosh(),
        Func::Tanh => z.tanh(),
        Func::Cotanh => z.cotanh(),
        Func::Asinh => z.asinh(),
        Func::Acosh => z.acosh(),
        Func::Atanh => z.atanh(),
        Func::Cosxx => z.cosxx(),
        Func::Ident => z,
        Func::Zero => Cx::zero(),
        Func::Real => Cx::real(z.re),
        Func::Imag => Cx::real(z.im),
        Func::Cabs => Cx::real(z.cabs()),
        Func::Norm => Cx::real(z.norm()),
        Func::Arg => Cx::real(z.arg()),
        Func::Atan2 | Func::Min | Func::Max => Cx::real(z.re),
    }
}

fn fmod<S: Scalar>(a: S, b: S) -> S {
    if b.v() == 0.0 { S::zero() } else { a - b * (a / b).trunc() }
}

/// Parameter values and function choices for one compilation.
pub struct Values<'a> {
    pub params: &'a [ParamValue],
}

struct Cc<'a, S> {
    prog: &'a Program,
    layout: &'a Layout,
    values: &'a [ParamValue],
    _s: std::marker::PhantomData<S>,
}

impl<'a, S: Scalar + Send + Sync + 'static> Cc<'a, S> {
    fn func_choice(&self, i: usize) -> Func {
        match self.values.get(i) {
            Some(ParamValue::Func(name)) => Func::by_name(name),
            _ => None,
        }
        .or_else(|| match &self.prog.params[i].default {
            ParamValue::Func(name) => Func::by_name(name),
            _ => None,
        })
        .unwrap_or(Func::Ident)
    }

    fn value(&self, i: usize) -> ParamValue {
        self.values.get(i).cloned().unwrap_or_else(|| self.prog.params[i].default.clone())
    }

    fn b(&self, e: &Expr) -> BFn<S> {
        match self.expr(e) {
            E::B(f) => f,
            _ => unreachable!("expected bool: {e:?}"),
        }
    }
    fn i(&self, e: &Expr) -> IFn<S> {
        match self.expr(e) {
            E::I(f) => f,
            _ => unreachable!("expected int: {e:?}"),
        }
    }
    fn r(&self, e: &Expr) -> RFn<S> {
        match self.expr(e) {
            E::R(f) => f,
            _ => unreachable!("expected real: {e:?}"),
        }
    }
    fn c(&self, e: &Expr) -> CFn<S> {
        match self.expr(e) {
            E::C(f) => f,
            _ => unreachable!("expected complex: {e:?}"),
        }
    }

    fn expr(&self, e: &Expr) -> E<S> {
        match &e.kind {
            Kind::Bool(b) => {
                let b = *b;
                E::B(Box::new(move |_| b))
            }
            Kind::Int(i) => {
                let i = *i;
                E::I(Box::new(move |_| i))
            }
            Kind::Real(v) => {
                let v = S::from_f64(*v);
                E::R(Box::new(move |_| v))
            }
            Kind::Complex(a, b) => {
                let (a, b) = (self.r(a), self.r(b));
                E::C(Box::new(move |f| Cx::new(a(f), b(f))))
            }
            Kind::Local(slot) => {
                let (ty, k) = self.layout.slot[*slot];
                match ty {
                    Ty::Bool => E::B(Box::new(move |f| f.bools[k])),
                    Ty::Int => E::I(Box::new(move |f| f.ints[k])),
                    Ty::Real => E::R(Box::new(move |f| f.reals[k])),
                    Ty::Complex => E::C(Box::new(move |f| f.cx[k])),
                }
            }
            Kind::Param(i) => {
                let v = self.value(*i);
                match self.prog.params[*i].ty {
                    Ty::Bool => {
                        let b = v.as_f64() != 0.0;
                        E::B(Box::new(move |_| b))
                    }
                    Ty::Int => {
                        let x = v.as_f64() as i64;
                        E::I(Box::new(move |_| x))
                    }
                    Ty::Real => {
                        let x = S::from_f64(v.as_f64());
                        E::R(Box::new(move |_| x))
                    }
                    Ty::Complex => {
                        let c = v.as_complex();
                        let z = Cx::from_f64(c[0], c[1]);
                        E::C(Box::new(move |_| z))
                    }
                }
            }
            Kind::Sys(s) => match s {
                Sys::Pixel => E::C(Box::new(|f| f.pixel)),
                Sys::C => E::C(Box::new(|f| f.c)),
                Sys::ZPrev => E::C(Box::new(|f| f.zprev)),
                Sys::Center => E::C(Box::new(|f| f.center)),
                Sys::LastSqr => E::R(Box::new(|f| f.lastsqr)),
                Sys::Magn => E::R(Box::new(|f| f.magn)),
                Sys::Z => E::C(Box::new(|f| f.zcur)),
                Sys::MaxIt => E::I(Box::new(|f| f.maxit)),
                Sys::NumIter => E::I(Box::new(|f| f.numiter)),
                Sys::IsMand => E::B(Box::new(|f| f.ismand)),
            },
            Kind::Conv(inner) => self.conv(inner, e.ty),
            Kind::Neg(a) => match self.expr(a) {
                E::I(a) => E::I(Box::new(move |f| a(f).wrapping_neg())),
                E::R(a) => E::R(Box::new(move |f| -a(f))),
                E::C(a) => E::C(Box::new(move |f| -a(f))),
                E::B(_) => unreachable!(),
            },
            Kind::Not(a) => {
                let a = self.b(a);
                E::B(Box::new(move |f| !a(f)))
            }
            Kind::Arith(op, a, b) => self.arith(*op, a, b, e.ty),
            Kind::Pow(a, b) => match e.ty {
                Ty::Real => {
                    let (a, b) = (self.r(a), self.r(b));
                    E::R(Box::new(move |f| real_pow(a(f), b(f))))
                }
                _ => {
                    let (a, b) = (self.c(a), self.c(b));
                    E::C(Box::new(move |f| a(f).pow(b(f))))
                }
            },
            Kind::PowI(a, n) => {
                let n = *n;
                match e.ty {
                    Ty::Real => {
                        let a = self.r(a);
                        E::R(Box::new(move |f| real_powi(a(f), n)))
                    }
                    _ => {
                        let a = self.c(a);
                        if n == 2 { E::C(Box::new(move |f| a(f).sqr())) } else { E::C(Box::new(move |f| a(f).powi(n))) }
                    }
                }
            }
            Kind::PowN(a, n) => {
                let n = self.i(n);
                match e.ty {
                    Ty::Real => {
                        let a = self.r(a);
                        E::R(Box::new(move |f| real_powi(a(f), n(f).clamp(-64, 64) as i32)))
                    }
                    _ => {
                        let a = self.c(a);
                        E::C(Box::new(move |f| a(f).powi(n(f).clamp(-64, 64) as i32)))
                    }
                }
            }
            Kind::Cmp(op, a, b) => {
                let op = *op;
                match a.ty {
                    Ty::Int => {
                        let (a, b) = (self.i(a), self.i(b));
                        E::B(Box::new(move |f| cmp_ord(op, a(f).cmp(&b(f)))))
                    }
                    Ty::Real => {
                        let (a, b) = (self.r(a), self.r(b));
                        E::B(Box::new(move |f| cmp_f(op, a(f).v(), b(f).v())))
                    }
                    Ty::Complex => {
                        let (a, b) = (self.c(a), self.c(b));
                        E::B(Box::new(move |f| {
                            let (x, y) = (a(f).v(), b(f).v());
                            let eq = x.re == y.re && x.im == y.im;
                            if op == Cmp::Ne { !eq } else { eq }
                        }))
                    }
                    Ty::Bool => unreachable!(),
                }
            }
            Kind::And(a, b) => {
                let (a, b) = (self.b(a), self.b(b));
                E::B(Box::new(move |f| a(f) && b(f)))
            }
            Kind::Or(a, b) => {
                let (a, b) = (self.b(a), self.b(b));
                E::B(Box::new(move |f| a(f) || b(f)))
            }
            Kind::Call(func, args) => self.call(*func, args, e.ty),
            Kind::CallParam(i, a) => {
                let func = self.func_choice(*i);
                let a = self.c(a);
                E::C(Box::new(move |f| complex_fn(func, a(f))))
            }
        }
    }

    fn conv(&self, inner: &Expr, to: Ty) -> E<S> {
        let from = self.expr(inner);
        match (from, to) {
            (E::B(a), Ty::Int) => E::I(Box::new(move |f| a(f) as i64)),
            (E::B(a), Ty::Real) => E::R(Box::new(move |f| S::from_f64(a(f) as i64 as f64))),
            (E::B(a), Ty::Complex) => E::C(Box::new(move |f| Cx::from_f64(a(f) as i64 as f64, 0.0))),
            (E::I(a), Ty::Bool) => E::B(Box::new(move |f| a(f) != 0)),
            (E::I(a), Ty::Real) => E::R(Box::new(move |f| S::from_f64(a(f) as f64))),
            (E::I(a), Ty::Complex) => E::C(Box::new(move |f| Cx::from_f64(a(f) as f64, 0.0))),
            (E::R(a), Ty::Bool) => E::B(Box::new(move |f| a(f).v() != 0.0)),
            (E::R(a), Ty::Int) => E::I(Box::new(move |f| a(f).v().trunc() as i64)),
            (E::R(a), Ty::Complex) => E::C(Box::new(move |f| Cx::real(a(f)))),
            (E::C(a), Ty::Bool) => E::B(Box::new(move |f| a(f).re.v() != 0.0)),
            (E::C(a), Ty::Int) => E::I(Box::new(move |f| a(f).re.v().trunc() as i64)),
            (E::C(a), Ty::Real) => E::R(Box::new(move |f| a(f).re)),
            (same, _) => same,
        }
    }

    fn arith(&self, op: Arith, a: &Expr, b: &Expr, ty: Ty) -> E<S> {
        match ty {
            Ty::Int => {
                let (a, b) = (self.i(a), self.i(b));
                E::I(match op {
                    Arith::Add => Box::new(move |f| a(f).wrapping_add(b(f))),
                    Arith::Sub => Box::new(move |f| a(f).wrapping_sub(b(f))),
                    Arith::Mul => Box::new(move |f| a(f).wrapping_mul(b(f))),
                    Arith::Div => Box::new(move |f| {
                        let d = b(f);
                        if d == 0 { 0 } else { a(f).wrapping_div(d) }
                    }),
                    Arith::Mod => Box::new(move |f| {
                        let d = b(f);
                        if d == 0 { 0 } else { a(f).wrapping_rem(d) }
                    }),
                })
            }
            Ty::Real => {
                let (a, b) = (self.r(a), self.r(b));
                E::R(match op {
                    Arith::Add => Box::new(move |f| a(f) + b(f)),
                    Arith::Sub => Box::new(move |f| a(f) - b(f)),
                    Arith::Mul => Box::new(move |f| a(f) * b(f)),
                    Arith::Div => Box::new(move |f| a(f) / b(f)),
                    Arith::Mod => Box::new(move |f| fmod(a(f), b(f))),
                })
            }
            _ => {
                let (a, b) = (self.c(a), self.c(b));
                E::C(match op {
                    Arith::Add => Box::new(move |f| a(f) + b(f)),
                    Arith::Sub => Box::new(move |f| a(f) - b(f)),
                    Arith::Mul => Box::new(move |f| a(f) * b(f)),
                    Arith::Div | Arith::Mod => Box::new(move |f| a(f) / b(f)),
                })
            }
        }
    }

    fn call(&self, func: Func, args: &[Expr], ty: Ty) -> E<S> {
        match func {
            Func::Atan2 | Func::Min | Func::Max => {
                let (a, b) = (self.r(&args[0]), self.r(&args[1]));
                E::R(match func {
                    Func::Atan2 => Box::new(move |f| a(f).atan2(b(f))),
                    Func::Min => Box::new(move |f| {
                        let (x, y) = (a(f), b(f));
                        if y.v() < x.v() { y } else { x }
                    }),
                    _ => Box::new(move |f| {
                        let (x, y) = (a(f), b(f));
                        if y.v() > x.v() { y } else { x }
                    }),
                })
            }
            Func::Real | Func::Imag | Func::Cabs | Func::Norm | Func::Arg => {
                let a = self.c(&args[0]);
                E::R(match func {
                    Func::Real => Box::new(move |f| a(f).re),
                    Func::Imag => Box::new(move |f| a(f).im),
                    Func::Cabs => Box::new(move |f| a(f).cabs()),
                    Func::Norm => Box::new(move |f| a(f).norm()),
                    _ => Box::new(move |f| a(f).arg()),
                })
            }
            _ if ty == Ty::Real => {
                let a = self.r(&args[0]);
                E::R(Box::new(move |f| real_fn(func, a(f))))
            }
            _ => {
                let a = self.c(&args[0]);
                E::C(Box::new(move |f| complex_fn(func, a(f))))
            }
        }
    }

    fn stmts(&self, list: &[Stmt]) -> Vec<SFn<S>> {
        list.iter().map(|s| self.stmt(s)).collect()
    }

    fn stmt(&self, s: &Stmt) -> SFn<S> {
        match s {
            Stmt::Assign(slot, e) => {
                let (ty, k) = self.layout.slot[*slot];
                match (ty, self.expr(e)) {
                    (Ty::Bool, E::B(v)) => Box::new(move |f| f.bools[k] = v(f)),
                    (Ty::Int, E::I(v)) => Box::new(move |f| f.ints[k] = v(f)),
                    (Ty::Real, E::R(v)) => Box::new(move |f| f.reals[k] = v(f)),
                    (Ty::Complex, E::C(v)) => Box::new(move |f| f.cx[k] = v(f)),
                    _ => unreachable!("assignment type mismatch"),
                }
            }
            Stmt::SetPart { local, re, value } => {
                let (_, k) = self.layout.slot[*local];
                let v = self.r(value);
                if *re { Box::new(move |f| f.cx[k].re = v(f)) } else { Box::new(move |f| f.cx[k].im = v(f)) }
            }
            Stmt::If(arms, els) => {
                let arms: Vec<(BFn<S>, Vec<SFn<S>>)> = arms.iter().map(|(c, b)| (self.b(c), self.stmts(b))).collect();
                let els = self.stmts(els);
                Box::new(move |f| {
                    for (c, b) in &arms {
                        if c(f) {
                            b.iter().for_each(|s| s(f));
                            return;
                        }
                    }
                    els.iter().for_each(|s| s(f));
                })
            }
            Stmt::While(c, body) => {
                let c = self.b(c);
                let body = self.stmts(body);
                Box::new(move |f| {
                    let mut n = 0;
                    while n < MAX_INNER_LOOP && c(f) {
                        body.iter().for_each(|s| s(f));
                        n += 1;
                    }
                })
            }
            Stmt::Repeat(body, c) => {
                let c = self.b(c);
                let body = self.stmts(body);
                Box::new(move |f| {
                    let mut n = 0;
                    loop {
                        body.iter().for_each(|s| s(f));
                        n += 1;
                        if n >= MAX_INNER_LOOP || c(f) {
                            break;
                        }
                    }
                })
            }
        }
    }
}

fn cmp_ord(op: Cmp, o: std::cmp::Ordering) -> bool {
    use std::cmp::Ordering::*;
    match op {
        Cmp::Lt => o == Less,
        Cmp::Gt => o == Greater,
        Cmp::Le => o != Greater,
        Cmp::Ge => o != Less,
        Cmp::Eq => o == Equal,
        Cmp::Ne => o != Equal,
    }
}

fn cmp_f(op: Cmp, a: f64, b: f64) -> bool {
    match op {
        Cmp::Lt => a < b,
        Cmp::Gt => a > b,
        Cmp::Le => a <= b,
        Cmp::Ge => a >= b,
        Cmp::Eq => a == b,
        Cmp::Ne => a != b,
    }
}

impl<S: Scalar + Send + Sync + 'static> Compiled<S> {
    /// Compiles `prog` with the parameter values `values` (missing ones use the defaults).
    pub fn new(prog: &Program, values: &[ParamValue]) -> Self {
        let mut counts = [0usize; 4];
        let slot = prog
            .locals
            .iter()
            .map(|l| {
                let k = match l.ty {
                    Ty::Bool => 0,
                    Ty::Int => 1,
                    Ty::Real => 2,
                    Ty::Complex => 3,
                };
                counts[k] += 1;
                (l.ty, counts[k] - 1)
            })
            .collect();
        let layout = Layout { slot, counts };
        let cc = Cc::<S> { prog, layout: &layout, values, _s: std::marker::PhantomData };
        let init = cc.stmts(&prog.init);
        let body = cc.stmts(&prog.body);
        let bailout = cc.b(&prog.bailout);
        let fin = cc.stmts(&prog.fin);
        Compiled { layout, init, body, bailout, fin, index_slot: prog.index_slot, solid_slot: prog.solid_slot }
    }

    /// A frame for this program (reuse it for many samples; [`Compiled::run`] resets it).
    pub fn frame(&self) -> Frame<S> {
        let c = &self.layout.counts;
        Frame {
            bools: vec![false; c[0]],
            ints: vec![0; c[1]],
            reals: vec![S::zero(); c[2]],
            cx: vec![Cx::zero(); c[3]],
            pixel: Cx::zero(),
            c: Cx::zero(),
            zprev: Cx::zero(),
            lastsqr: S::zero(),
            numiter: 0,
            maxit: 0,
            ismand: true,
            center: Cx::zero(),
            magn: S::one(),
            zcur: Cx::zero(),
        }
    }

    fn reset(&self, f: &mut Frame<S>, maxit: u32) {
        f.bools.iter_mut().for_each(|v| *v = false);
        f.ints.iter_mut().for_each(|v| *v = 0);
        f.reals.iter_mut().for_each(|v| *v = S::zero());
        f.cx.iter_mut().for_each(|v| *v = Cx::zero());
        f.zprev = Cx::zero();
        f.lastsqr = S::zero();
        f.numiter = 0;
        f.maxit = maxit as i64;
    }

    /// Transformation: runs `global` and `transform` on `pixel`; returns the new pixel and
    /// whether the point is solid (left transparent).
    pub fn transform(&self, f: &mut Frame<S>, pixel: Cx<S>) -> (Cx<S>, bool) {
        self.reset(f, 0);
        let z = self.layout.slot[Z].1;
        f.cx[z] = pixel;
        f.pixel = pixel;
        self.init.iter().for_each(|s| s(f));
        self.body.iter().for_each(|s| s(f));
        (f.cx[z], self.solid(f))
    }

    fn solid(&self, f: &Frame<S>) -> bool {
        self.solid_slot.is_some_and(|s| f.bools[self.layout.slot[s].1])
    }

    /// Coloring: starts a sample (the caller sets `pixel`, `c`, `ismand`, `center`, `magn` and
    /// `zcur` to the fractal's `z` after its `init`).
    pub fn color_init(&self, f: &mut Frame<S>, maxit: u32) {
        self.reset(f, maxit);
        self.init.iter().for_each(|s| s(f));
    }

    /// Coloring: one iteration (after every fractal iteration that continues).
    pub fn color_loop(&self, f: &mut Frame<S>, numiter: u32, zprev: Cx<S>) {
        f.numiter = numiter as i64;
        f.zprev = zprev;
        self.body.iter().for_each(|s| s(f));
    }

    /// Coloring: the end of the orbit; returns `#index` and `#solid`.
    pub fn color_final(&self, f: &mut Frame<S>, numiter: u32, zprev: Cx<S>) -> (S, bool) {
        f.numiter = numiter as i64;
        f.zprev = zprev;
        self.fin.iter().for_each(|s| s(f));
        let index = self.index_slot.map_or(S::zero(), |s| f.reals[self.layout.slot[s].1]);
        (index, self.solid(f))
    }

    /// Iterates one sample. The caller sets `pixel`, `c`, `ismand`, `center` and `magn`.
    pub fn run(&self, f: &mut Frame<S>, maxit: u32) -> Orbit<S> {
        self.run_with(f, maxit, |_| {})
    }

    /// [`Compiled::run`] with a hook called after `init` and after every iteration that
    /// continues (coloring formulas run there).
    pub fn run_with(&self, f: &mut Frame<S>, maxit: u32, mut hook: impl FnMut(Hook<S>)) -> Orbit<S> {
        self.reset(f, maxit);
        let z = self.layout.slot[Z].1;
        self.init.iter().for_each(|s| s(f));
        hook(Hook::Init(f.cx[z]));
        let mut escaped = false;
        let mut iter = 0;
        let mut zprev2 = Cx::zero();
        while iter < maxit {
            zprev2 = f.zprev;
            f.zprev = f.cx[z];
            self.body.iter().for_each(|s| s(f));
            iter += 1;
            f.numiter = iter as i64;
            f.lastsqr = f.cx[z].norm();
            if !f.cx[z].is_finite() || !(self.bailout)(f) {
                escaped = true;
                break;
            }
            hook(Hook::Iter(iter, f.cx[z], f.zprev));
        }
        self.fin.iter().for_each(|s| s(f));
        Orbit { z: f.cx[z], zprev: f.zprev, zprev2, iter, escaped }
    }
}

#[cfg(test)]
mod tests {
    use super::super::check::check;
    use super::super::parse::{parse_ufm_entry, split_entries};
    use super::*;
    use crate::math::complex::C64;
    use crate::math::scalar::Dual;

    fn program(src: &str) -> Program {
        check(&parse_ufm_entry(&split_entries(src)[0]).unwrap()).unwrap()
    }

    #[test]
    fn mandelbrot_orbits() {
        let p = program("M {\ninit:\n  z = 0\nloop:\n  z = z^2 + #pixel\nbailout:\n  |z| <= 4\n}");
        let c = Compiled::<f64>::new(&p, &[]);
        let mut f = c.frame();
        f.pixel = C64::new(-1.0, 0.0);
        assert!(!c.run(&mut f, 100).escaped, "-1 is inside");
        f.pixel = C64::new(1.0, 1.0);
        let o = c.run(&mut f, 100);
        assert!(o.escaped && o.iter == 2, "{o:?}");
    }

    #[test]
    fn dual_frame_tracks_dz_dc() {
        // After two iterations z = c² + c, so dz/dc = 2c + 1.
        let p = program("M {\ninit:\n  z = 0\nloop:\n  z = z*z + #pixel\nbailout:\n  #numiter < 2\n}");
        let c = Compiled::<Dual>::new(&p, &[]);
        let mut f = c.frame();
        f.pixel = Cx::new(Dual::new(0.3, 1.0), Dual::new(0.2, 0.0));
        let o = c.run(&mut f, 10);
        assert_eq!(o.iter, 2);
        assert!((o.z.re.d - 1.6).abs() < 1e-12 && (o.z.im.d - 0.4).abs() < 1e-12, "{o:?}");
    }

    #[test]
    fn control_flow_and_ints() {
        let p = program(
            "W {\ninit:\n  int n = 0\n  float s = 0\n  while n < 5\n    n = n + 1\n    s = s + n\n  endwhile\n  repeat\n    n = n - 2\n  until n <= 0\n  z = (s, n)\nloop:\nbailout:\n  false\n}",
        );
        let c = Compiled::<f64>::new(&p, &[]);
        let mut f = c.frame();
        let o = c.run(&mut f, 1);
        assert_eq!(o.z, C64::new(15.0, -1.0));
    }
}
