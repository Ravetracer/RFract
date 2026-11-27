// SPDX-License-Identifier: LGPL-3.0-or-later

//! Typed intermediate representation of a formula. Every node carries its type and every
//! conversion is explicit, so the back ends (CPU closures in `cpu.rs`, WGSL in rfr-gpu) only
//! translate node by node and agree on the semantics.

use super::ast::{ParamKind, ParamValue};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Ty {
    Bool,
    Int,
    Real,
    Complex,
    /// RGBA (sRGB-encoded channels, straight alpha); never converted to or from numbers.
    Color,
}

/// Values the host provides or maintains.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Sys {
    /// Plane coordinate of the sample.
    Pixel,
    /// The constant: the pixel in Mandelbrot mode, the seed in Julia mode.
    C,
    /// `z` before the current loop iteration.
    ZPrev,
    /// `|z|²` after the last completed loop iteration.
    LastSqr,
    /// Iteration limit (real in FRM-like, int in UFM).
    MaxIt,
    /// Completed iterations (int).
    NumIter,
    /// Mandelbrot (parameter plane) or Julia mode.
    IsMand,
    /// Center of the view (complex) and magnification (real).
    Center,
    Magn,
    /// The fractal's current `z` (colorings: `#z`).
    Z,
    /// Image position of the pixel (int) and image size (int).
    X,
    Y,
    Width,
    Height,
    /// Image position of the pixel and image size as complex numbers.
    ScreenPixel,
    ScreenMax,
    /// Rotation of the view in radians.
    Angle,
    /// A new random complex number with both parts in -1..1 at every use.
    Random,
}

impl Sys {
    pub fn ty(self) -> Ty {
        match self {
            Sys::Pixel | Sys::C | Sys::ZPrev | Sys::Center | Sys::Z | Sys::ScreenPixel | Sys::ScreenMax | Sys::Random => Ty::Complex,
            Sys::LastSqr | Sys::Magn | Sys::Angle => Ty::Real,
            Sys::MaxIt | Sys::NumIter | Sys::X | Sys::Y | Sys::Width | Sys::Height => Ty::Int,
            Sys::IsMand => Ty::Bool,
        }
    }
}

/// Standard library functions. Unary ones take and return complex values unless noted; with a
/// real argument and a real result type they compute the real function (UFM
/// overloads), which equals the real part of the complex one on the real axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Func {
    Abs,
    Sqr,
    Sqrt,
    Exp,
    Log,
    Recip,
    Conj,
    Flip,
    Round,
    Floor,
    Ceil,
    Trunc,
    Sin,
    Cos,
    Tan,
    Cotan,
    Asin,
    Acos,
    Atan,
    Sinh,
    Cosh,
    Tanh,
    Cotanh,
    Asinh,
    Acosh,
    Atanh,
    Cosxx,
    Ident,
    Zero,
    /// Complex → real.
    Real,
    Imag,
    Cabs,
    /// UFM `|z|`: squared magnitude (complex → real).
    Norm,
    /// UFM `atan2(z)`: argument of z (complex → real).
    Arg,
    /// FRM-like `atan2(y, x)` (real, real → real).
    Atan2,
    /// (real, real) → real
    Min,
    Max,
}

impl Func {
    /// Functions that can fill a function parameter (unary, complex in and out).
    pub const UNARY: [(&'static str, Func); 33] = [
        ("abs", Func::Abs),
        ("sqr", Func::Sqr),
        ("sqrt", Func::Sqrt),
        ("exp", Func::Exp),
        ("log", Func::Log),
        ("recip", Func::Recip),
        ("conj", Func::Conj),
        ("flip", Func::Flip),
        ("round", Func::Round),
        ("floor", Func::Floor),
        ("ceil", Func::Ceil),
        ("trunc", Func::Trunc),
        ("sin", Func::Sin),
        ("cos", Func::Cos),
        ("tan", Func::Tan),
        ("cotan", Func::Cotan),
        ("asin", Func::Asin),
        ("acos", Func::Acos),
        ("atan", Func::Atan),
        ("sinh", Func::Sinh),
        ("cosh", Func::Cosh),
        ("tanh", Func::Tanh),
        ("cotanh", Func::Cotanh),
        ("asinh", Func::Asinh),
        ("acosh", Func::Acosh),
        ("atanh", Func::Atanh),
        ("cosxx", Func::Cosxx),
        ("ident", Func::Ident),
        ("identity", Func::Ident),
        ("zero", Func::Zero),
        ("real", Func::Real),
        ("imag", Func::Imag),
        ("cabs", Func::Cabs),
    ];

    pub fn by_name(name: &str) -> Option<Func> {
        let l = name.to_ascii_lowercase();
        let l = match l.as_str() {
            "cot" => "cotan",
            "cotanh" | "coth" => "cotanh",
            other => other,
        };
        Func::UNARY.iter().find(|(n, _)| *n == l).map(|(_, f)| *f)
    }

    pub fn name(self) -> &'static str {
        match self {
            Func::Norm => "norm",
            Func::Arg => "arg",
            Func::Atan2 => "atan2",
            Func::Min => "min",
            Func::Max => "max",
            f => Func::UNARY.iter().find(|(_, g)| *g == f).map(|(n, _)| *n).unwrap_or("?"),
        }
    }

    /// Complex → real functions.
    pub fn returns_real(self) -> bool {
        matches!(self, Func::Real | Func::Imag | Func::Cabs | Func::Norm | Func::Arg | Func::Atan2 | Func::Min | Func::Max)
    }

    /// Functions whose result on a real argument is real (so the UFM real overload is
    /// the same as the complex function's real part, without an imaginary part to drop).
    pub fn real_closed(self) -> bool {
        matches!(
            self,
            Func::Abs
                | Func::Sqr
                | Func::Exp
                | Func::Recip
                | Func::Conj
                | Func::Round
                | Func::Floor
                | Func::Ceil
                | Func::Trunc
                | Func::Sin
                | Func::Cos
                | Func::Tan
                | Func::Cotan
                | Func::Atan
                | Func::Sinh
                | Func::Cosh
                | Func::Tanh
                | Func::Cotanh
                | Func::Asinh
                | Func::Cosxx
                | Func::Ident
                | Func::Zero
        )
    }

    /// Uses transcendental math (exp, log, trigonometry): not exact in double-float mode.
    pub fn transcendental(self) -> bool {
        !matches!(
            self,
            Func::Abs
                | Func::Sqr
                | Func::Sqrt
                | Func::Recip
                | Func::Conj
                | Func::Flip
                | Func::Round
                | Func::Floor
                | Func::Ceil
                | Func::Trunc
                | Func::Ident
                | Func::Zero
                | Func::Real
                | Func::Imag
                | Func::Cabs
                | Func::Norm
                | Func::Min
                | Func::Max
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arith {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cmp {
    Lt,
    Gt,
    Le,
    Ge,
    Eq,
    Ne,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Expr {
    pub ty: Ty,
    pub kind: Kind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    Bool(bool),
    Int(i64),
    Real(f64),
    /// Complex from two reals.
    Complex(Box<Expr>, Box<Expr>),
    Local(usize),
    Param(usize),
    Sys(Sys),
    /// Converts the operand to `ty`: bool → 0/1, int → real, real → complex (im 0), complex →
    /// real (real part), real → int (truncation), numeric → bool (real part ≠ 0).
    Conv(Box<Expr>),
    Neg(Box<Expr>),
    Not(Box<Expr>),
    /// Both operands have the expression's type (int, real or complex).
    Arith(Arith, Box<Expr>, Box<Expr>),
    /// Principal power; both operands have the expression's type (real or complex).
    Pow(Box<Expr>, Box<Expr>),
    /// Power with a constant integer exponent (repeated multiplication).
    PowI(Box<Expr>, i32),
    /// Power with an integer exponent computed at run time (repeated multiplication, the
    /// exponent clamped to ±64).
    PowN(Box<Expr>, Box<Expr>),
    /// Both operands have the same type: int, real (ordering) or complex (== and != only).
    Cmp(Cmp, Box<Expr>, Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    /// Library function; argument types follow from the function and `ty`.
    Call(Func, Vec<Expr>),
    /// Call through a function parameter (complex → complex).
    CallParam(usize, Box<Expr>),
    /// UFM `random(seed)`: the next pseudo-random int after `seed` (32-bit hash, sign kept).
    RandomInt(Box<Expr>),
    /// Color constructor, channel or mixing function (see [`ColorOp`] for the operands).
    ColorOp(ColorOp, Vec<Expr>),
    /// `@fn(a, b)` through a color function parameter: the merge mode it chooses (colors).
    MergeParam(usize, Box<Expr>, Box<Expr>),
    /// Element of the array local `slot` at the (flat, int) index, clamped to its capacity.
    Index(usize, Box<Expr>),
    /// `deriv(e, x)`: the derivative of `e` (real or complex) with respect to the real or
    /// complex local `x`. The back ends expand it with [`super::deriv::derivative`].
    Deriv(Box<Expr>, usize),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Stmt {
    Assign(usize, Expr),
    /// `real(local) = e` (`re`) or `imag(local) = e`; the value is real.
    SetPart {
        local: usize,
        re: bool,
        value: Expr,
    },
    If(Vec<(Expr, Vec<Stmt>)>, Vec<Stmt>),
    /// `array[index] = value` (flat int index, clamped).
    SetIndex(usize, Expr, Expr),
    While(Expr, Vec<Stmt>),
    Repeat(Vec<Stmt>, Expr),
}

/// Inner loops (while, repeat) stop after this many rounds, so a formula cannot hang a frame.
pub const MAX_INNER_LOOP: u32 = 65536;

#[derive(Clone, Debug, PartialEq)]
pub struct Local {
    pub name: String,
    pub ty: Ty,
    /// Arrays: the number of elements the local holds (indexes are clamped to it).
    pub cap: Option<usize>,
}

/// Elements an array holds when its size is not a constant (the GPU keeps arrays per sample).
pub const ARRAY_CAP: usize = 2048;
/// The most elements a constant-size array may have.
pub const ARRAY_MAX: usize = 4096;

#[derive(Clone, Debug, PartialEq)]
pub struct Param {
    pub name: String,
    pub kind: ParamKind,
    pub ty: Ty,
    pub default: ParamValue,
    pub caption: String,
    pub hint: String,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub items: Vec<String>,
    /// Object parameters: the base class of the choices.
    pub base: Option<String>,
}

/// The local slot of `z`.
pub const Z: usize = 0;

#[derive(Clone, Debug, PartialEq)]
pub struct Program {
    pub name: String,
    pub kind: super::ast::ProgramKind,
    pub params: Vec<Param>,
    /// Local variables; slot [`Z`] is `z` (complex).
    pub locals: Vec<Local>,
    pub init: Vec<Stmt>,
    pub body: Vec<Stmt>,
    /// Continue while true.
    pub bailout: Expr,
    pub fin: Vec<Stmt>,
    /// Reads `ismand` / `#ismand`: the formula has a Julia mode.
    pub has_julia: bool,
    /// Uses transcendental functions or general powers (double-float mode is not exact).
    pub transcendental: bool,
    /// Local slots of `#index` (colorings) and `#solid` (colorings, transformations).
    pub index_slot: Option<usize>,
    pub solid_slot: Option<usize>,
    /// Local slot of `#color` (colorings); `direct` when the program assigns it (the color
    /// then replaces the gradient lookup of `#index`).
    pub color_slot: Option<usize>,
    pub direct: bool,
}

/// Color operations of formulas (`super::color`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ColorOp {
    /// `rgba(r, g, b, a)` (real × 4 → color; `rgb` passes alpha 1).
    Rgba,
    /// `hsla(h, s, l, a)` (real × 4 → color; hue 0…6).
    Hsla,
    /// Channels (color → real).
    Red,
    Green,
    Blue,
    Alpha,
    Hue,
    Sat,
    Lum,
    /// `blend(a, b, t)` (color, color, real → color).
    Blend,
    /// `compose(bottom, top, amount)` (color, color, real → color).
    Compose,
    /// `merge…(bottom, top)` (color, color → color).
    Merge(crate::doc::MergeMode),
    /// `gradient(t)`: the coloring's gradient at `t` (real → color).
    Gradient,
    /// Arithmetic on colors, channel by channel (`Add`, `Sub`, `Mul`, `Div`; color × 2).
    Arith(Arith),
    /// A number as a color with all four channels set to it (real → color).
    Splat,
    /// Image objects of image parameter `p` (`crate::images`): `getColor(z)` (complex →
    /// color), `getPixel(x, y)` (int, int → color), `getWidth()`, `getHeight()` (→ int),
    /// `getEmpty()` (→ bool).
    ImageColor(usize),
    ImagePixel(usize),
    ImageWidth(usize),
    ImageHeight(usize),
    ImageEmpty(usize),
}

/// The merge mode each color function parameter chooses (`None`: not one).
pub fn merge_choices(prog: &Program, values: &[ParamValue]) -> Vec<Option<crate::doc::MergeMode>> {
    prog.params
        .iter()
        .enumerate()
        .map(|(i, p)| {
            if p.kind != ParamKind::Func {
                return None;
            }
            let name = match (values.get(i), &p.default) {
                (Some(ParamValue::Func(f)), _) if super::color::merge_mode(f).is_some() => f.clone(),
                (_, ParamValue::Func(f)) => f.clone(),
                _ => return None,
            };
            super::color::merge_mode(&name)
        })
        .collect()
}

impl Program {
    /// Index of the parameter called `name`.
    pub fn param(&self, name: &str) -> Option<usize> {
        self.params.iter().position(|p| p.name == name)
    }
}
