// SPDX-License-Identifier: LGPL-3.0-or-later

//! Untyped syntax tree shared by all front ends (FRM-like v1, UFM syntax). The checker
//! (`check.rs`) resolves names and types and lowers it to the typed IR (`ir.rs`).

/// Source dialect: decides the meaning of `|z|`, the precedence of unary minus and `^`, name
/// lookup (`#pixel`, `@param` versus plain names) and integer literals.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Dialect {
    /// FractalPark FRM-like v1: `|z|` is the magnitude, unary minus binds tighter than `^`,
    /// numbers are real, system values are plain names (`pixel`, `c`, `z`, `ismand`).
    Frm,
    /// UFM syntax (also RFract's own formulas): `|z|` is the squared magnitude,
    /// `-z^2` is `-(z^2)`, integer literals are `int`, `#name` system values, `@name` parameters.
    Ufm,
}

/// What a program computes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ProgramKind {
    /// Fractal formula: `init`, `loop`, `bailout`, `final` on `z`.
    Fractal,
    /// Coloring algorithm: `init`, `loop` (after every iteration that continues), `final`; reads
    /// `#z`, writes `#index` (gradient position) and `#solid`.
    Coloring,
    /// Transformation: `transform` changes `#pixel` before the fractal runs; `#solid = true`
    /// leaves the point transparent.
    Transform,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Or,
    And,
    Lt,
    Gt,
    Le,
    Ge,
    Eq,
    Ne,
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Num {
        v: f64,
        int: bool,
    },
    Bool(bool),
    /// `(re, im)`
    Complex(Box<Expr>, Box<Expr>),
    /// A string (compared with enumeration parameters: `@mode == "Julia"`).
    Str(String),
    Ident(String),
    /// `#name`
    Sys(String),
    /// `@name`
    Param(String),
    /// `name(args)`, or `@name(args)` when `param` is set.
    Call {
        name: String,
        param: bool,
        args: Vec<Expr>,
    },
    Unary(UnOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    /// `|e|`
    Mag(Box<Expr>),
    /// `a[i]`, `a[i, j]`
    Index(String, Vec<Expr>),
    /// `object.field`
    Member(Box<Expr>, String),
    /// `object.field[i, …]`
    MemberIndex(Box<Expr>, String, Vec<Expr>),
    /// `object.Method(args)`, also `Class.Method(args)` (static or base class calls).
    Method(Box<Expr>, String, Vec<Expr>),
    /// `new Class(args)` or `new @param(args)` (the class chosen by an object parameter).
    New {
        class: String,
        param: bool,
        args: Vec<Expr>,
    },
    /// An assignment inside an expression (`(x = sqrt(y)) < 1`): assigns, gives the value.
    Assign(Box<Target>, Box<Expr>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    Var(String),
    Sys(String),
    /// `real(t) = …` (re) or `imag(t) = …`
    Comp {
        re: bool,
        var: String,
    },
    /// `a[i] = …`
    Index(String, Vec<Expr>),
    /// `object.field = …`
    Member(Box<Expr>, String),
    /// `object.field[i] = …`
    MemberIndex(Box<Expr>, String, Vec<Expr>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeclTy {
    Bool,
    Int,
    Float,
    Complex,
    Color,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Stmt {
    Assign {
        target: Target,
        value: Expr,
        line: usize,
    },
    Decl {
        ty: DeclTy,
        name: String,
        value: Option<Expr>,
        line: usize,
    },
    If {
        arms: Vec<(Expr, Vec<Stmt>)>,
        els: Vec<Stmt>,
        line: usize,
    },
    While {
        cond: Expr,
        body: Vec<Stmt>,
        line: usize,
    },
    Repeat {
        body: Vec<Stmt>,
        cond: Expr,
        line: usize,
    },
    /// Several statements from one line (chained assignments `z = c = pixel`).
    Seq(Vec<Stmt>),
    /// `float a[n, m]` (sizes) or `float a[]` (dynamic: no sizes, see `setLength`).
    ArrayDecl {
        ty: DeclTy,
        name: String,
        dims: Vec<Expr>,
        line: usize,
    },
    /// A procedure call statement (`setLength(a, n)`).
    Call {
        name: String,
        args: Vec<Expr>,
        line: usize,
    },
    /// A variable of a class type (`Formula f = new @formulaClass(0)`), or an array of them.
    ObjDecl {
        class: String,
        name: String,
        dims: Option<Vec<Expr>>,
        value: Option<Expr>,
        line: usize,
    },
    /// An expression evaluated for its effect (a method call: `f.Init(#z)`).
    Eval(Expr, usize),
    /// `return` in a class method.
    Return(Option<Expr>, usize),
}

/// A type in class code: a built-in type or a class.
#[derive(Clone, Debug, PartialEq)]
pub enum TypeRef {
    Builtin(DeclTy),
    Class(String),
}

/// A method parameter: type, name, passed by reference (`&name`).
#[derive(Clone, Debug, PartialEq)]
pub struct MethodParam {
    pub ty: TypeRef,
    pub name: String,
    pub by_ref: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MethodAst {
    pub name: String,
    /// `None`: no value (constructors and procedures).
    pub ret: Option<TypeRef>,
    pub params: Vec<MethodParam>,
    pub body: Vec<Stmt>,
    pub is_static: bool,
    pub line: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FieldAst {
    pub ty: TypeRef,
    pub name: String,
    /// Array dimensions (`[]`: dynamic).
    pub dims: Option<Vec<Expr>>,
}

/// A class of a `.ulb` file.
#[derive(Clone, Debug, PartialEq)]
pub struct ClassAst {
    pub name: String,
    pub parent: Option<String>,
    pub fields: Vec<FieldAst>,
    pub methods: Vec<MethodAst>,
    /// Parameters of the `default:` section (every object of the class has them).
    pub params: Vec<ParamDecl>,
    pub title: Option<String>,
    pub line: usize,
}

/// Parameter kinds of formula definitions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ParamKind {
    Bool,
    Int,
    Float,
    Complex,
    /// A function of the standard library, chosen by name (`sin`, `sqr`, …).
    Func,
    /// RGBA color (sRGB-encoded channels 0…1, straight alpha).
    Color,
    /// An object parameter: the user picks a class derived from `ParamDecl::base`.
    Class,
    /// An image file (`Image param`): formulas read it through image objects.
    Image,
}

/// A parameter value as stored in documents and formula defaults.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum ParamValue {
    Bool(bool),
    Int(i64),
    Float(f64),
    Complex([f64; 2]),
    /// RGBA, sRGB-encoded channels 0…1.
    Color([f64; 4]),
    Func(String),
    /// The class chosen for an object parameter.
    Class {
        class: String,
    },
    /// An image file (empty: none).
    Image {
        path: String,
    },
}

impl ParamValue {
    pub fn as_f64(&self) -> f64 {
        match self {
            ParamValue::Bool(b) => *b as i64 as f64,
            ParamValue::Int(i) => *i as f64,
            ParamValue::Float(f) => *f,
            ParamValue::Complex(c) => c[0],
            ParamValue::Color(c) => c[0],
            ParamValue::Func(_) | ParamValue::Class { .. } | ParamValue::Image { .. } => 0.0,
        }
    }
    pub fn as_complex(&self) -> [f64; 2] {
        match self {
            ParamValue::Complex(c) => *c,
            other => [other.as_f64(), 0.0],
        }
    }
    pub fn as_color(&self) -> [f64; 4] {
        match self {
            ParamValue::Color(c) => *c,
            other => {
                let v = other.as_f64();
                [v, v, v, 1.0]
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParamDecl {
    pub name: String,
    pub kind: ParamKind,
    pub default: ParamValue,
    pub caption: String,
    pub hint: String,
    pub min: Option<f64>,
    pub max: Option<f64>,
    /// Labels of an enumeration (`int` parameters).
    pub items: Vec<String>,
    /// FractInt slot this parameter mirrors (`p1`…`p5`, `fn1`…`fn4`).
    pub classic: Option<String>,
    /// Object parameters: the base class of the choices.
    pub base: Option<String>,
}

/// One formula entry as parsed.
#[derive(Clone, Debug, PartialEq)]
pub struct FormulaAst {
    pub name: String,
    pub dialect: Dialect,
    pub params: Vec<ParamDecl>,
    /// `global:` (UFM syntax); runs before `init` here.
    pub global: Vec<Stmt>,
    pub init: Vec<Stmt>,
    pub body: Vec<Stmt>,
    pub bailout: Option<Expr>,
    pub fin: Vec<Stmt>,
    /// `transform:` (transformations).
    pub transform: Vec<Stmt>,
    /// From the `default:` section.
    pub title: Option<String>,
    pub center: Option<[f64; 2]>,
    pub magn: Option<f64>,
    pub maxiter: Option<u32>,
    /// Sections that were present but are not supported yet (reported, skipped).
    pub skipped_sections: Vec<String>,
    /// Written in FractInt syntax (unknown names start at zero).
    pub fractint: bool,
    /// Class libraries named by `import "x.ulb"`.
    pub imports: Vec<String>,
    /// Functions declared in the sections (`complex func f(complex z) … endfunc`).
    pub funcs: Vec<MethodAst>,
}
