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
    /// Colors (sRGB-encoded RGBA).
    pub colors: Vec<[f64; 4]>,
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
    /// Image position of the pixel and image size.
    pub px: i64,
    pub py: i64,
    pub width: i64,
    pub height: i64,
    /// View rotation in radians.
    pub angle: f64,
    /// State of `#random` (the same generator as the GPU's).
    pub rng: RngState,
    /// Periodicity checking (fractals whose loop is a map of z alone, see
    /// [`super::period`]): an orbit that comes back within this distance (per part) of the
    /// `z` saved at the last power-of-two iteration ends as inside at the iteration limit;
    /// 0: off.
    pub period: f64,
}

/// Generator state of `#random`; an atomic so frames can be shared with worker threads (each
/// thread uses its own frame).
#[derive(Debug, Default)]
pub struct RngState(pub std::sync::atomic::AtomicU32);

impl Clone for RngState {
    fn clone(&self) -> Self {
        RngState(std::sync::atomic::AtomicU32::new(self.get()))
    }
}

impl RngState {
    pub fn get(&self) -> u32 {
        self.0.load(std::sync::atomic::Ordering::Relaxed)
    }
    pub fn set(&self, v: u32) {
        self.0.store(v, std::sync::atomic::Ordering::Relaxed)
    }
}

/// One step of the random generator behind `#random`: returns a value in -1..1 (mirrored by
/// `rnd_next` in kernel.wgsl).
pub fn rnd_next(state: &RngState) -> f64 {
    let s = state.get().wrapping_mul(747_796_405).wrapping_add(2_891_336_453);
    state.set(s);
    let word = ((s >> ((s >> 28) + 4)) ^ s).wrapping_mul(277_803_737);
    let v = (word >> 22) ^ word;
    (v >> 8) as f64 / 8_388_608.0 - 1.0
}

type BFn<S> = Box<dyn Fn(&Frame<S>) -> bool + Send + Sync>;
type IFn<S> = Box<dyn Fn(&Frame<S>) -> i64 + Send + Sync>;
type RFn<S> = Box<dyn Fn(&Frame<S>) -> S + Send + Sync>;
type CFn<S> = Box<dyn Fn(&Frame<S>) -> Cx<S> + Send + Sync>;
type KFn<S> = Box<dyn Fn(&Frame<S>) -> [f64; 4] + Send + Sync>;
type SFn<S> = Box<dyn Fn(&mut Frame<S>) + Send + Sync>;

enum E<S> {
    B(BFn<S>),
    I(IFn<S>),
    R(RFn<S>),
    C(CFn<S>),
    K(KFn<S>),
}

/// How the program's locals map onto the frame's typed vectors.
#[derive(Clone, Debug)]
struct Layout {
    slot: Vec<(Ty, usize)>,
    /// Element count per local (1 for scalars).
    cap: Vec<usize>,
    counts: [usize; 5],
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
    /// `#color` when the program assigns it.
    color_slot: Option<usize>,
}

/// Real `x^y`: positive bases through exp/log, zero gives zero, negative bases use the real
/// part of the principal complex power.
pub fn real_pow<S: Scalar>(x: S, y: S) -> S {
    if x.v() > 0.0 {
        (y * x.ln()).exp()
    } else if x.v() == 0.0 {
        if y.v() == 0.0 { S::one() } else { S::zero() }
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
        Func::Noise | Func::Perlin | Func::Voronoi => Cx::real(S::from_f64(noise_fn(f, z.re.v(), z.im.v()))),
        Func::Norm => Cx::real(z.norm()),
        Func::Arg => Cx::real(z.arg()),
        Func::Atan2 | Func::Min | Func::Max => Cx::real(z.re),
    }
}

/// The noise functions of the formula language at (x, y).
pub fn noise_fn(f: Func, x: f64, y: f64) -> f64 {
    match f {
        Func::Perlin => crate::math::noise::perlin(x, y),
        Func::Voronoi => crate::math::noise::voronoi(x, y),
        _ => crate::math::noise::value(x, y),
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
    /// The coloring's gradient for `gradient(t)` (linear RGB, alpha).
    lut: std::sync::Arc<Vec<[f32; 4]>>,
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
    fn k(&self, e: &Expr) -> KFn<S> {
        match self.expr(e) {
            E::K(f) => f,
            _ => unreachable!("expected color: {e:?}"),
        }
    }

    fn color_op(&self, op: ColorOp, a: &[Expr]) -> E<S> {
        use super::color;
        match op {
            ColorOp::Rgba | ColorOp::Hsla => {
                let (x, y, z, w) = (self.r(&a[0]), self.r(&a[1]), self.r(&a[2]), self.r(&a[3]));
                if op == ColorOp::Rgba {
                    E::K(Box::new(move |f| [x(f).v(), y(f).v(), z(f).v(), w(f).v()]))
                } else {
                    E::K(Box::new(move |f| {
                        let [r, g, b] = color::hsl_to_rgb(x(f).v(), y(f).v(), z(f).v());
                        [r, g, b, w(f).v()]
                    }))
                }
            }
            ColorOp::Red | ColorOp::Green | ColorOp::Blue | ColorOp::Alpha => {
                let c = self.k(&a[0]);
                let i = match op {
                    ColorOp::Red => 0,
                    ColorOp::Green => 1,
                    ColorOp::Blue => 2,
                    _ => 3,
                };
                E::R(Box::new(move |f| S::from_f64(c(f)[i])))
            }
            ColorOp::Hue | ColorOp::Sat | ColorOp::Lum => {
                let c = self.k(&a[0]);
                let i = match op {
                    ColorOp::Hue => 0,
                    ColorOp::Sat => 1,
                    _ => 2,
                };
                E::R(Box::new(move |f| S::from_f64(color::rgb_to_hsl(c(f))[i])))
            }
            ColorOp::Blend | ColorOp::Compose => {
                let (x, y, t) = (self.k(&a[0]), self.k(&a[1]), self.r(&a[2]));
                if op == ColorOp::Blend {
                    E::K(Box::new(move |f| color::blend(x(f), y(f), t(f).v())))
                } else {
                    E::K(Box::new(move |f| color::compose(x(f), y(f), t(f).v())))
                }
            }
            ColorOp::Merge(m) => {
                let (x, y) = (self.k(&a[0]), self.k(&a[1]));
                E::K(Box::new(move |f| color::merge(m, x(f), y(f))))
            }
            ColorOp::Gradient => {
                let t = self.r(&a[0]);
                let lut = self.lut.clone();
                E::K(Box::new(move |f| {
                    let t = t(f).v();
                    if lut.is_empty() {
                        let g = t.rem_euclid(1.0);
                        return [g, g, g, 1.0];
                    }
                    let c = crate::gradient::lut_sample(&lut, t);
                    let e = crate::render::composite::srgb_encode;
                    [e(c[0]) as f64, e(c[1]) as f64, e(c[2]) as f64, c[3] as f64]
                }))
            }
            ColorOp::Arith(k) => {
                let (x, y) = (self.k(&a[0]), self.k(&a[1]));
                E::K(Box::new(move |f| {
                    let (p, q) = (x(f), y(f));
                    std::array::from_fn(|i| match k {
                        Arith::Add => p[i] + q[i],
                        Arith::Sub => p[i] - q[i],
                        Arith::Mul => p[i] * q[i],
                        _ => p[i] / q[i],
                    })
                }))
            }
            ColorOp::Splat => {
                let x = self.r(&a[0]);
                E::K(Box::new(move |f| [x(f).v(); 4]))
            }
            ColorOp::ImageColor(p) | ColorOp::ImagePixel(p) | ColorOp::ImageWidth(p) | ColorOp::ImageHeight(p) | ColorOp::ImageEmpty(p) => {
                let img = match self.values.get(p) {
                    Some(ParamValue::Image { path }) => crate::images::get(path),
                    _ => None,
                };
                match op {
                    ColorOp::ImageColor(_) => {
                        let z = self.c(&a[0]);
                        E::K(Box::new(move |f| {
                            let z = z(f);
                            crate::images::color(img.as_deref(), z.re.v(), z.im.v())
                        }))
                    }
                    ColorOp::ImagePixel(_) => {
                        let (x, y) = (self.i(&a[0]), self.i(&a[1]));
                        E::K(Box::new(move |f| crate::images::pixel(img.as_deref(), x(f), y(f))))
                    }
                    ColorOp::ImageEmpty(_) => {
                        let empty = img.is_none();
                        E::B(Box::new(move |_| empty))
                    }
                    _ => {
                        let n = img.map_or(0, |i| i64::from(if op == ColorOp::ImageWidth(p) { i.orig_width } else { i.orig_height }));
                        E::I(Box::new(move |_| n))
                    }
                }
            }
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
                    Ty::Color => E::K(Box::new(move |f| f.colors[k])),
                }
            }
            Kind::Index(slot, idx) => {
                let (ty, k) = self.layout.slot[*slot];
                let last = self.layout.cap[*slot] as i64 - 1;
                let i = self.i(idx);
                match ty {
                    Ty::Bool => E::B(Box::new(move |f| f.bools[k + i(f).clamp(0, last) as usize])),
                    Ty::Int => E::I(Box::new(move |f| f.ints[k + i(f).clamp(0, last) as usize])),
                    Ty::Real => E::R(Box::new(move |f| f.reals[k + i(f).clamp(0, last) as usize])),
                    Ty::Complex => E::C(Box::new(move |f| f.cx[k + i(f).clamp(0, last) as usize])),
                    Ty::Color => E::K(Box::new(move |f| f.colors[k + i(f).clamp(0, last) as usize])),
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
                    Ty::Color => {
                        let c = v.as_color();
                        E::K(Box::new(move |_| c))
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
                Sys::X => E::I(Box::new(|f| f.px)),
                Sys::Y => E::I(Box::new(|f| f.py)),
                Sys::Width => E::I(Box::new(|f| f.width)),
                Sys::Height => E::I(Box::new(|f| f.height)),
                Sys::ScreenPixel => E::C(Box::new(|f| Cx::from_f64(f.px as f64, f.py as f64))),
                Sys::ScreenMax => E::C(Box::new(|f| Cx::from_f64(f.width as f64, f.height as f64))),
                Sys::Angle => E::R(Box::new(|f| S::from_f64(f.angle))),
                Sys::Random => E::C(Box::new(|f| {
                    let re = rnd_next(&f.rng);
                    let im = rnd_next(&f.rng);
                    Cx::from_f64(re, im)
                })),
                Sys::MaxIt => E::I(Box::new(|f| f.maxit)),
                Sys::NumIter => E::I(Box::new(|f| f.numiter)),
                Sys::IsMand => E::B(Box::new(|f| f.ismand)),
            },
            Kind::Conv(inner) => self.conv(inner, e.ty),
            Kind::Neg(a) => match self.expr(a) {
                E::I(a) => E::I(Box::new(move |f| a(f).wrapping_neg())),
                E::R(a) => E::R(Box::new(move |f| -a(f))),
                E::C(a) => E::C(Box::new(move |f| -a(f))),
                E::B(_) | E::K(_) => unreachable!(),
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
                    Ty::Bool | Ty::Color => unreachable!(),
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
            Kind::ColorOp(op, args) => self.color_op(*op, args),
            Kind::RandomInt(a) => {
                let a = self.i(a);
                E::I(Box::new(move |f| crate::render::hash(a(f) as u32) as i32 as i64))
            }
            Kind::MergeParam(i, a, b) => {
                let mode = merge_choices(self.prog, self.values).get(*i).copied().flatten().unwrap_or_default();
                let (a, b) = (self.k(a), self.k(b));
                E::K(Box::new(move |f| super::color::merge(mode, a(f), b(f))))
            }
            Kind::Deriv(a, v) => self.expr(&super::deriv::derivative(a, *v, &|i| self.func_choice(i))),
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
            Func::Noise | Func::Perlin | Func::Voronoi => {
                let a = self.c(&args[0]);
                E::R(Box::new(move |f| {
                    let z = a(f);
                    S::from_f64(noise_fn(func, z.re.v(), z.im.v()))
                }))
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
                    (Ty::Color, E::K(v)) => Box::new(move |f| f.colors[k] = v(f)),
                    _ => unreachable!("assignment type mismatch"),
                }
            }
            Stmt::SetIndex(slot, idx, e) => {
                let (ty, k) = self.layout.slot[*slot];
                let last = self.layout.cap[*slot] as i64 - 1;
                let i = self.i(idx);
                match (ty, self.expr(e)) {
                    (Ty::Bool, E::B(v)) => Box::new(move |f| {
                        let j = k + i(f).clamp(0, last) as usize;
                        f.bools[j] = v(f)
                    }),
                    (Ty::Int, E::I(v)) => Box::new(move |f| {
                        let j = k + i(f).clamp(0, last) as usize;
                        f.ints[j] = v(f)
                    }),
                    (Ty::Real, E::R(v)) => Box::new(move |f| {
                        let j = k + i(f).clamp(0, last) as usize;
                        f.reals[j] = v(f)
                    }),
                    (Ty::Complex, E::C(v)) => Box::new(move |f| {
                        let j = k + i(f).clamp(0, last) as usize;
                        f.cx[j] = v(f)
                    }),
                    (Ty::Color, E::K(v)) => Box::new(move |f| {
                        let j = k + i(f).clamp(0, last) as usize;
                        f.colors[j] = v(f)
                    }),
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
        Self::with_gradient(prog, values, Vec::new())
    }

    /// [`Compiled::new`] for colorings: `lut` is the gradient `gradient(t)` reads.
    pub fn with_gradient(prog: &Program, values: &[ParamValue], lut: Vec<[f32; 4]>) -> Self {
        let mut counts = [0usize; 5];
        let slot = prog
            .locals
            .iter()
            .map(|l| {
                let k = match l.ty {
                    Ty::Bool => 0,
                    Ty::Int => 1,
                    Ty::Real => 2,
                    Ty::Complex => 3,
                    Ty::Color => 4,
                };
                let n = l.cap.unwrap_or(1);
                counts[k] += n;
                (l.ty, counts[k] - n)
            })
            .collect();
        let cap = prog.locals.iter().map(|l| l.cap.unwrap_or(1)).collect();
        let layout = Layout { slot, cap, counts };
        let cc = Cc::<S> { prog, layout: &layout, values, lut: std::sync::Arc::new(lut), _s: std::marker::PhantomData };
        let init = cc.stmts(&prog.init);
        let body = cc.stmts(&prog.body);
        let bailout = cc.b(&prog.bailout);
        let fin = cc.stmts(&prog.fin);
        let color_slot = prog.color_slot.filter(|_| prog.direct);
        Compiled { layout, init, body, bailout, fin, index_slot: prog.index_slot, solid_slot: prog.solid_slot, color_slot }
    }

    /// A frame for this program (reuse it for many samples; [`Compiled::run`] resets it).
    pub fn frame(&self) -> Frame<S> {
        let c = &self.layout.counts;
        Frame {
            bools: vec![false; c[0]],
            ints: vec![0; c[1]],
            reals: vec![S::zero(); c[2]],
            cx: vec![Cx::zero(); c[3]],
            colors: vec![[0.0; 4]; c[4]],
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
            px: 0,
            py: 0,
            width: 1,
            height: 1,
            angle: 0.0,
            rng: RngState::default(),
            period: 0.0,
        }
    }

    fn reset(&self, f: &mut Frame<S>, maxit: u32) {
        f.bools.iter_mut().for_each(|v| *v = false);
        f.ints.iter_mut().for_each(|v| *v = 0);
        f.reals.iter_mut().for_each(|v| *v = S::zero());
        f.cx.iter_mut().for_each(|v| *v = Cx::zero());
        f.colors.iter_mut().for_each(|v| *v = [0.0; 4]);
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

    /// Coloring: `#color` after [`Compiled::color_final`] when the program sets it (direct
    /// coloring), sRGB-encoded RGBA.
    pub fn direct_color(&self, f: &Frame<S>) -> Option<[f64; 4]> {
        self.color_slot.map(|s| f.colors[self.layout.slot[s].1])
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

    /// Writes the real or complex local `slot` (`f64` value).
    fn set_local(&self, f: &mut Frame<S>, slot: usize, v: crate::math::complex::C64) {
        self.set_local_d(f, slot, v, crate::math::complex::C64::new(0.0, 0.0));
    }

    /// Writes the real or complex local `slot` with derivative `d` (kept by dual numbers).
    fn set_local_d(&self, f: &mut Frame<S>, slot: usize, v: crate::math::complex::C64, d: crate::math::complex::C64) {
        let (ty, k) = self.layout.slot[slot];
        match ty {
            Ty::Complex => f.cx[k] = Cx::new(S::with_d(v.re, d.re), S::with_d(v.im, d.im)),
            Ty::Real => f.reals[k] = S::with_d(v.re, d.re),
            _ => {}
        }
    }

    /// The reference orbit of a deep zoom at `center`: the formula's own bailout decides when
    /// it ends.
    pub fn reference_orbit(&self, pert: &super::perturb::Perturbation, center: &[String; 2], bits: usize, maxit: u32) -> super::perturb::RefOrbit {
        let mut f = self.frame();
        self.reset(&mut f, maxit);
        pert.reference(center, bits, maxit, |full| {
            for &s in &pert.state {
                self.set_local(&mut f, s, full[s]);
            }
            let z = f.cx[self.layout.slot[Z].1];
            f.lastsqr = z.norm();
            !z.is_finite() || !(self.bailout)(&f)
        })
    }

    /// [`Compiled::run_with`] by perturbation: the deltas of the sample at offset `dpix` from
    /// the reference iterate; the frame holds the full values for the bailout, the hooks and
    /// `final`. With `dseed` (the derivative of the pixel) the locals also get their
    /// derivatives (dual numbers; no linear steps should be used then).
    pub fn run_perturbed(
        &self,
        f: &mut Frame<S>,
        maxit: u32,
        deep: super::perturb::Deep,
        dpix: crate::math::fx::Cfx,
        dseed: Option<crate::math::fx::Cfx>,
        mut hook: impl FnMut(Hook<S>),
    ) -> Orbit<S> {
        use crate::math::complex::C64;
        let super::perturb::Deep { pert, orbit, bla } = deep;
        self.reset(f, maxit);
        let z = self.layout.slot[Z].1;
        let mut full = vec![C64::new(0.0, 0.0); pert.nlocals];
        let mut st = pert.start_with(orbit, dpix, dseed);
        let mut write = |f: &mut Frame<S>, st: &super::perturb::DeltaState| {
            pert.full(orbit, st, &mut full);
            for &s in &pert.state {
                // Derivatives beyond f64 (far below the pixel scale) stay at the largest value.
                let d = st.deriv.get(s).map_or(C64::new(0.0, 0.0), |d| {
                    let (re, im) = d.to_c64();
                    if re.is_finite() && im.is_finite() { C64::new(re, im) } else { C64::new(1e300, 0.0) }
                });
                self.set_local_d(f, s, full[s], d);
            }
        };
        write(f, &st);
        hook(Hook::Init(f.cx[z]));
        let mut escaped = false;
        let mut iter = 0;
        let mut zprev2 = Cx::zero();
        while iter < maxit {
            zprev2 = f.zprev;
            f.zprev = f.cx[z];
            // Linear steps skip iterations while they are valid (no hooks run for them).
            let skipped = bla.map_or(0, |t| pert.bla_skip(t, &mut st, dpix, maxit - iter));
            if skipped > 0 {
                pert.rebase_if_needed(orbit, &mut st);
                write(f, &st);
                iter += skipped;
                f.numiter = iter as i64;
                f.lastsqr = f.cx[z].norm();
                continue;
            }
            pert.step(orbit, &mut st, dpix);
            write(f, &st);
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
        let (mut saved, mut save_at) = (f.cx[z], 1);
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
            if f.period > 0.0 {
                let zc = f.cx[z];
                if (zc.re.v() - saved.re.v()).abs() < f.period && (zc.im.v() - saved.im.v()).abs() < f.period {
                    iter = maxit;
                    break;
                }
                if iter == save_at {
                    saved = zc;
                    save_at *= 2;
                }
            }
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
