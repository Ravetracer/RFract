// SPDX-License-Identifier: LGPL-3.0-or-later

//! Lexer and parser for formula sources: FractalPark FRM-like v1 files (one formula with a
//! directive preamble) and UFM syntax files (any number of `name { … }` entries).

use super::ast::*;

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    Ident(String),
    Sys(String),
    Param(String),
    Num(f64, bool),
    /// Imaginary number literal (`2i`).
    Imag(f64),
    Str(String),
    Op(&'static str),
    Newline,
    Eof,
}

#[derive(Clone, Debug)]
struct Token {
    tok: Tok,
    line: usize,
}

/// A parse or check error with its line (1-based, 0 when unknown).
#[derive(Clone, Debug, PartialEq)]
pub struct Error {
    pub line: usize,
    pub msg: String,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.line > 0 { write!(f, "line {}: {}", self.line, self.msg) } else { f.write_str(&self.msg) }
    }
}

pub fn err<T>(line: usize, msg: impl Into<String>) -> Result<T, Error> {
    Err(Error { line, msg: msg.into() })
}

const OPS: [&str; 27] =
    ["||", "&&", "&", "<=", ">=", "==", "!=", "<", ">", "+", "-", "*", "/", "^", "%", "!", "=", "(", ")", ",", "|", ":", "{", "}", "[", "]", "."];

fn lex(src: &str, first_line: usize) -> Result<Vec<Token>, Error> {
    let mut out = Vec::new();
    let b = src.as_bytes();
    let mut i = 0;
    let mut line = first_line;
    while i < b.len() {
        let c = b[i];
        match c {
            b'\n' => {
                out.push(Token { tok: Tok::Newline, line });
                line += 1;
                i += 1;
            }
            b' ' | b'\t' | b'\r' => i += 1,
            b';' => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            // Line continuation (UFM syntax): a backslash before the end of the line.
            b'\\' => {
                i += 1;
                while i < b.len() && (b[i] == b' ' || b[i] == b'\t' || b[i] == b'\r') {
                    i += 1;
                }
                if i < b.len() && b[i] == b'\n' {
                    i += 1;
                    line += 1;
                }
            }
            b'"' => {
                // Strings end at the closing quote; a backslash at the end of a line continues
                // them on the next line.
                let mut text = String::new();
                let first = line;
                i += 1;
                let mut seg = i;
                while i < b.len() && b[i] != b'"' && b[i] != b'\n' {
                    if b[i] == b'\\' {
                        let mut j = i + 1;
                        while j < b.len() && (b[j] == b' ' || b[j] == b'\t' || b[j] == b'\r') {
                            j += 1;
                        }
                        if j < b.len() && b[j] == b'\n' {
                            text.push_str(&src[seg..i]);
                            line += 1;
                            i = j + 1;
                            while i < b.len() && (b[i] == b' ' || b[i] == b'\t') {
                                i += 1;
                            }
                            seg = i;
                            continue;
                        }
                    }
                    i += 1;
                }
                text.push_str(&src[seg..i]);
                out.push(Token { tok: Tok::Str(text), line: first });
                if i < b.len() && b[i] == b'"' {
                    i += 1;
                }
            }
            b'0'..=b'9' => i = lex_number(src, i, line, &mut out)?,
            b'.' if i + 1 < b.len() && b[i + 1].is_ascii_digit() => i = lex_number(src, i, line, &mut out)?,
            b'#' | b'@' => {
                let start = i + 1;
                i += 1;
                while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                    i += 1;
                }
                let name = src[start..i].to_string();
                if name.is_empty() {
                    return err(line, format!("'{}' without a name", c as char));
                }
                out.push(Token { tok: if c == b'#' { Tok::Sys(name) } else { Tok::Param(name) }, line });
            }
            c if c.is_ascii_alphabetic() || c == b'_' => {
                let start = i;
                while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                    i += 1;
                }
                out.push(Token { tok: Tok::Ident(src[start..i].to_string()), line });
            }
            _ => {
                let rest = &src[i..];
                match OPS.iter().find(|op| rest.starts_with(**op)) {
                    Some(op) => {
                        out.push(Token { tok: Tok::Op(op), line });
                        i += op.len();
                    }
                    None => {
                        let ch = rest.chars().next().unwrap_or('?');
                        return err(line, format!("unexpected character '{ch}'"));
                    }
                }
            }
        }
    }
    out.push(Token { tok: Tok::Newline, line });
    out.push(Token { tok: Tok::Eof, line });
    Ok(out)
}

fn lex_number(src: &str, mut i: usize, line: usize, out: &mut Vec<Token>) -> Result<usize, Error> {
    let b = src.as_bytes();
    let start = i;
    let mut int = true;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i < b.len() && b[i] == b'.' && !(i + 1 < b.len() && b[i + 1] == b'.') {
        int = false;
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        if j < b.len() && b[j].is_ascii_digit() {
            int = false;
            i = j;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
        }
    }
    let text = &src[start..i];
    let v: f64 = text.parse().or_else(|_| format!("{text}0").parse()).map_err(|_| Error { line, msg: format!("bad number '{text}'") })?;
    // Imaginary literal: 2i, 1e-30i.
    if i < b.len() && (b[i] == b'i' || b[i] == b'I') && !(i + 1 < b.len() && (b[i + 1].is_ascii_alphanumeric() || b[i + 1] == b'_')) {
        out.push(Token { tok: Tok::Imag(v), line });
        return Ok(i + 1);
    }
    out.push(Token { tok: Tok::Num(v, int), line });
    Ok(i)
}

/// Value of a constant numeric expression (numbers and arithmetic).
fn fold_const(e: &Expr) -> Option<f64> {
    Some(match e {
        Expr::Num { v, .. } => *v,
        Expr::Unary(UnOp::Neg, a) => -fold_const(a)?,
        Expr::Binary(op, a, b) => {
            let (x, y) = (fold_const(a)?, fold_const(b)?);
            match op {
                BinOp::Add => x + y,
                BinOp::Sub => x - y,
                BinOp::Mul => x * y,
                BinOp::Div if y != 0.0 => x / y,
                _ => return None,
            }
        }
        _ => return None,
    })
}

struct Parser {
    toks: Vec<Token>,
    pos: usize,
    dialect: Dialect,
    /// FractInt syntax (inside UFM files or `.frm`): every number is complex, no integers.
    fractint: bool,
    /// `import "x.ulb"` seen so far.
    imports: Vec<String>,
    /// Open parentheses around the expression being parsed.
    paren_depth: usize,
    /// Functions declared in the sections of the formula being parsed.
    funcs: Vec<MethodAst>,
}

const FRM_SECTIONS: [&str; 4] = ["parameters", "init", "loop", "bailout"];

const SECTIONS: [&str; 17] = [
    "public",
    "protected",
    "private",
    "global",
    "init",
    "loop",
    "bailout",
    "final",
    "default",
    "parameters",
    "switch",
    "transform",
    "perturbinit",
    "perturbloop",
    "builtin",
    "deriv",
    "loop2",
];

impl Parser {
    fn peek(&self) -> &Tok {
        &self.toks[self.pos].tok
    }
    fn peek_at(&self, k: usize) -> &Tok {
        &self.toks[(self.pos + k).min(self.toks.len() - 1)].tok
    }
    fn line(&self) -> usize {
        self.toks[self.pos].line
    }
    fn next(&mut self) -> Tok {
        let t = self.toks[self.pos].tok.clone();
        if self.pos < self.toks.len() - 1 {
            self.pos += 1;
        }
        t
    }
    fn is_op(&self, op: &str) -> bool {
        matches!(self.peek(), Tok::Op(o) if *o == op)
    }
    fn eat_op(&mut self, op: &str) -> bool {
        if self.is_op(op) {
            self.next();
            true
        } else {
            false
        }
    }
    fn expect_op(&mut self, op: &str) -> Result<(), Error> {
        if self.eat_op(op) { Ok(()) } else { err(self.line(), format!("expected '{op}', found {}", show(self.peek()))) }
    }
    fn is_op_at(&self, k: usize, op: &str) -> bool {
        matches!(self.peek_at(k), Tok::Op(o) if *o == op)
    }
    fn is_kw(&self, kw: &str) -> bool {
        matches!(self.peek(), Tok::Ident(s) if s.eq_ignore_ascii_case(kw))
    }
    fn skip_newlines(&mut self) {
        while *self.peek() == Tok::Newline {
            self.next();
        }
    }
    fn end_of_statement(&mut self) -> Result<(), Error> {
        match self.peek() {
            Tok::Newline => {
                self.skip_newlines();
                Ok(())
            }
            Tok::Eof => Ok(()),
            // FractInt syntax: the colon ends the init statements (consumed by the caller).
            Tok::Op(":") if self.fractint => Ok(()),
            // UFM allows several statements on one line separated by commas.
            Tok::Op(",") if self.dialect == Dialect::Ufm => {
                self.next();
                Ok(())
            }
            t => err(self.line(), format!("expected the end of the line, found {}", show(t))),
        }
    }
    /// At `name:` that starts a section?
    fn at_section(&self) -> Option<String> {
        if let Tok::Ident(s) = self.peek() {
            let l = s.to_ascii_lowercase();
            let known = match self.dialect {
                Dialect::Frm => FRM_SECTIONS.contains(&l.as_str()),
                Dialect::Ufm => SECTIONS.contains(&l.as_str()),
            };
            if known && matches!(self.peek_at(1), Tok::Op(":")) {
                return Some(l);
            }
        }
        None
    }

    // ---- expressions ----

    fn expr(&mut self) -> Result<Expr, Error> {
        self.binary(0)
    }

    fn binary(&mut self, min_prec: u8) -> Result<Expr, Error> {
        let mut lhs = self.unary()?;
        loop {
            // `(x = value)`: an assignment inside an expression (the whole of a parenthesized
            // expression or an argument; right-associative: `(a = b = c)`). A text on the
            // right compares (`@mode = "Julia"`).
            if min_prec == 0
                && self.paren_depth > 0
                && self.dialect == Dialect::Ufm
                && self.is_op("=")
                && let Some(target) = assign_target(&lhs)
                && !matches!(self.peek_at(1), Tok::Str(_))
            {
                self.next();
                let value = self.binary(0)?;
                lhs = Expr::Assign(Box::new(target), Box::new(value));
                continue;
            }
            let (op, prec, right) = match self.peek() {
                Tok::Op("||") => (BinOp::Or, 1, false),
                Tok::Op("&&") => (BinOp::And, 2, false),
                Tok::Op("&") if self.dialect == Dialect::Ufm => (BinOp::And, 2, false),
                // `if (a + 1 = b)`: inside parentheses UFM code may compare with a single `=`.
                Tok::Op("=") if self.paren_depth > 0 && self.dialect == Dialect::Ufm => (BinOp::Eq, 3, false),
                Tok::Op("<") => (BinOp::Lt, 3, false),
                Tok::Op(">") => (BinOp::Gt, 3, false),
                Tok::Op("<=") => (BinOp::Le, 3, false),
                Tok::Op(">=") => (BinOp::Ge, 3, false),
                Tok::Op("==") => (BinOp::Eq, 3, false),
                Tok::Op("!=") => (BinOp::Ne, 3, false),
                Tok::Op("+") => (BinOp::Add, 4, false),
                Tok::Op("-") => (BinOp::Sub, 4, false),
                Tok::Op("*") => (BinOp::Mul, 5, false),
                Tok::Op("/") => (BinOp::Div, 5, false),
                Tok::Op("%") => (BinOp::Mod, 5, false),
                Tok::Op("^") => (BinOp::Pow, 6, true),
                _ => break,
            };
            if prec < min_prec {
                break;
            }
            self.next();
            let rhs = if op == BinOp::Pow && self.dialect == Dialect::Ufm {
                // UFM: the exponent may carry its own unary minus (z^-2).
                self.binary(prec)?
            } else {
                self.binary(if right { prec } else { prec + 1 })?
            };
            lhs = Expr::Binary(op, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn unary(&mut self) -> Result<Expr, Error> {
        let op = match self.peek() {
            Tok::Op("-") => Some(UnOp::Neg),
            Tok::Op("!") => Some(UnOp::Not),
            Tok::Op("+") => {
                self.next();
                return self.unary();
            }
            _ => None,
        };
        if let Some(op) = op {
            self.next();
            if self.dialect == Dialect::Ufm {
                // -z^2 = -(z^2): the operand includes a following power.
                let operand = self.binary(6)?;
                return Ok(Expr::Unary(op, Box::new(operand)));
            }
            let operand = self.unary()?;
            return Ok(Expr::Unary(op, Box::new(operand)));
        }
        self.primary()
    }

    fn primary(&mut self) -> Result<Expr, Error> {
        let base = self.primary_base()?;
        self.postfix(base)
    }

    /// `.field` and `.Method(args)` after a value.
    fn postfix(&mut self, mut e: Expr) -> Result<Expr, Error> {
        while self.dialect == Dialect::Ufm && !self.fractint && self.is_op(".") && matches!(self.peek_at(1), Tok::Ident(_)) {
            self.next();
            let Tok::Ident(name) = self.next() else { unreachable!() };
            if self.eat_op("(") {
                let args = self.args()?;
                e = Expr::Method(Box::new(e), name, args);
            } else if self.eat_op("[") {
                let idx = self.index_list()?;
                e = Expr::MemberIndex(Box::new(e), name, idx);
            } else {
                e = Expr::Member(Box::new(e), name);
            }
        }
        Ok(e)
    }

    fn primary_base(&mut self) -> Result<Expr, Error> {
        let line = self.line();
        if self.dialect == Dialect::Ufm && self.is_kw("new") {
            self.next();
            let (class, param) = match self.next() {
                Tok::Param(n) => (n, true),
                Tok::Ident(n) => (self.class_name_rest(n), false),
                t => return err(line, format!("expected a class after 'new', found {}", show(&t))),
            };
            let args = if self.eat_op("(") { self.args()? } else { Vec::new() };
            return Ok(Expr::New { class, param, args });
        }
        match self.next() {
            Tok::Num(v, int) => Ok(Expr::Num { v, int: int && self.dialect == Dialect::Ufm && !self.fractint }),
            Tok::Imag(v) => Ok(Expr::Complex(Box::new(Expr::Num { v: 0.0, int: false }), Box::new(Expr::Num { v, int: false }))),
            Tok::Str(s) => Ok(Expr::Str(s)),
            Tok::Op("(") => {
                self.paren_depth += 1;
                let r = (|| {
                    let a = self.expr()?;
                    if self.eat_op(",") {
                        let b = self.expr()?;
                        self.expect_op(")")?;
                        return Ok(Expr::Complex(Box::new(a), Box::new(b)));
                    }
                    self.expect_op(")")?;
                    Ok(a)
                })();
                self.paren_depth -= 1;
                r
            }
            Tok::Op("|") => {
                let e = self.binary_no_or()?;
                self.expect_op("|")?;
                Ok(Expr::Mag(Box::new(e)))
            }
            Tok::Ident(name) => {
                let lower = name.to_ascii_lowercase();
                if lower == "true" || lower == "false" {
                    return Ok(Expr::Bool(lower == "true"));
                }
                if self.eat_op("(") {
                    let args = self.args()?;
                    return Ok(Expr::Call { name, param: false, args });
                }
                if self.eat_op("[") {
                    let idx = self.index_list()?;
                    return Ok(Expr::Index(name, idx));
                }
                Ok(Expr::Ident(name))
            }
            Tok::Sys(name) => Ok(Expr::Sys(name)),
            Tok::Param(name) => {
                if self.eat_op("(") {
                    let args = self.args()?;
                    return Ok(Expr::Call { name, param: true, args });
                }
                Ok(Expr::Param(name))
            }
            t => err(line, format!("expected a value, found {}", show(&t))),
        }
    }

    /// Inside `|…|`: a `|` ends the magnitude, so `||` cannot start inside it unless spaced.
    fn binary_no_or(&mut self) -> Result<Expr, Error> {
        self.binary(2)
    }

    /// Indexes up to the closing `]` (the `[` is consumed); empty for `[]`.
    fn index_list(&mut self) -> Result<Vec<Expr>, Error> {
        let mut out = Vec::new();
        if self.eat_op("]") {
            return Ok(out);
        }
        loop {
            out.push(self.expr()?);
            if self.eat_op("]") {
                return Ok(out);
            }
            self.expect_op(",")?;
        }
    }

    fn args(&mut self) -> Result<Vec<Expr>, Error> {
        let mut args = Vec::new();
        if self.eat_op(")") {
            return Ok(args);
        }
        loop {
            // Arguments count as parenthesized (assignments inside: `abs(r = random(r))`).
            self.paren_depth += 1;
            let a = self.expr();
            self.paren_depth -= 1;
            args.push(a?);
            if self.eat_op(")") {
                // FractInt functions take one argument: f(a, b) is f((a, b)).
                if self.fractint && args.len() == 2 {
                    let b = args.pop().unwrap();
                    let a = args.pop().unwrap();
                    args.push(Expr::Complex(Box::new(a), Box::new(b)));
                }
                return Ok(args);
            }
            self.expect_op(",")?;
        }
    }

    // ---- statements ----

    /// Statements until one of the `ends` keywords or a section header.
    fn block(&mut self, ends: &[&str]) -> Result<Vec<Stmt>, Error> {
        let mut out = Vec::new();
        loop {
            self.skip_newlines();
            if *self.peek() == Tok::Eof || self.is_op("}") || self.at_section().is_some() || ends.iter().any(|e| self.is_kw(e)) {
                return Ok(out);
            }
            // A function of the formula: `[type] func Name(…) … endfunc`.
            if self.dialect == Dialect::Ufm && !self.fractint {
                let at = |p: &Self, k: usize| matches!(p.peek_at(k), Tok::Ident(s) if s.eq_ignore_ascii_case("func"));
                let typed = matches!(self.peek(), Tok::Ident(_)) && at(self, 1) && matches!(self.peek_at(2), Tok::Ident(_));
                if typed || (at(self, 0) && matches!(self.peek_at(1), Tok::Ident(_)) && self.is_op_at(2, "(")) {
                    let line = self.line();
                    let ret = if typed { Some(self.type_ref()?) } else { None };
                    let m = self.method_decl(ret, false, line)?;
                    self.funcs.push(m);
                    continue;
                }
            }
            out.push(self.statement()?);
        }
    }

    fn statement(&mut self) -> Result<Stmt, Error> {
        let line = self.line();
        if self.is_kw("if") {
            self.next();
            let mut arms = vec![(self.expr()?, Vec::new())];
            self.end_of_statement()?;
            arms[0].1 = self.block(&["elseif", "else", "endif"])?;
            let mut els = Vec::new();
            loop {
                if self.is_kw("elseif") {
                    self.next();
                    let c = self.expr()?;
                    self.end_of_statement()?;
                    let b = self.block(&["elseif", "else", "endif"])?;
                    arms.push((c, b));
                } else if self.is_kw("else") {
                    self.next();
                    // "else if" on one line (FractInt habit).
                    self.end_of_statement()?;
                    els = self.block(&["endif"])?;
                } else if self.is_kw("endif") {
                    self.next();
                    self.end_of_statement()?;
                    return Ok(Stmt::If { arms, els, line });
                } else {
                    return err(self.line(), "'if' without 'endif'");
                }
            }
        }
        if self.is_kw("while") && self.dialect == Dialect::Ufm {
            self.next();
            let cond = self.expr()?;
            self.end_of_statement()?;
            let body = self.block(&["endwhile"])?;
            if !self.is_kw("endwhile") {
                return err(self.line(), "'while' without 'endwhile'");
            }
            self.next();
            self.end_of_statement()?;
            return Ok(Stmt::While { cond, body, line });
        }
        if self.is_kw("repeat") && self.dialect == Dialect::Ufm {
            self.next();
            self.end_of_statement()?;
            let body = self.block(&["until"])?;
            if !self.is_kw("until") {
                return err(self.line(), "'repeat' without 'until'");
            }
            self.next();
            let cond = self.expr()?;
            self.end_of_statement()?;
            return Ok(Stmt::Repeat { body, cond, line });
        }
        if self.dialect == Dialect::Ufm && !self.fractint {
            if self.is_kw("import") {
                self.next();
                if let Tok::Str(f) = self.next() {
                    self.imports.push(f);
                }
                self.end_of_statement()?;
                return Ok(Stmt::Seq(Vec::new()));
            }
            if self.is_kw("return") {
                self.next();
                let value = if matches!(self.peek(), Tok::Newline | Tok::Eof) { None } else { Some(self.expr()?) };
                self.end_of_statement()?;
                return Ok(Stmt::Return(value, line));
            }
            // Class-typed declaration: `Formula f = new @formulaClass(0)`, `Line lines[]`.
            if let (Tok::Ident(t), Tok::Ident(n)) = (self.peek().clone(), self.peek_at(1).clone())
                && builtin_ty(&t).is_none()
                && !is_keyword(&t)
                && !is_keyword(&n)
            {
                self.next();
                self.next();
                let dims = if self.eat_op("[") { Some(self.index_list()?) } else { None };
                let value = if self.eat_op("=") { Some(self.expr()?) } else { None };
                self.end_of_statement()?;
                return Ok(Stmt::ObjDecl { class: t, name: n, dims, value, line });
            }
            // object.field = …, object.Method(…), Class.Method(…), Cast(object).Method(…),
            // array[i].Method(…)
            let cast_call = matches!(self.peek(), Tok::Ident(_)) && (self.is_op_at(1, "(") || self.is_op_at(1, "[")) && {
                let mut depth = 0usize;
                let mut k = 1;
                loop {
                    match self.peek_at(k) {
                        Tok::Op("(" | "[") => depth += 1,
                        Tok::Op(")" | "]") => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        Tok::Newline | Tok::Eof => break,
                        _ => {}
                    }
                    k += 1;
                }
                depth == 0 && self.is_op_at(k + 1, ".")
            };
            if cast_call
                || (matches!(self.peek(), Tok::Ident(_) | Tok::Param(_)) && self.is_op_at(1, ".") && matches!(self.peek_at(2), Tok::Ident(_)))
            {
                let e = self.primary()?;
                if self.eat_op("=") {
                    let value = self.expr()?;
                    self.end_of_statement()?;
                    return match e {
                        Expr::Member(o, f) => Ok(Stmt::Assign { target: Target::Member(o, f), value, line }),
                        Expr::MemberIndex(o, f, i) => Ok(Stmt::Assign { target: Target::MemberIndex(o, f, i), value, line }),
                        _ => err(line, "this cannot be assigned"),
                    };
                }
                self.end_of_statement()?;
                return match e {
                    Expr::Method(..) => Ok(Stmt::Eval(e, line)),
                    _ => err(line, "expected a method call or an assignment"),
                };
            }
        }
        if self.dialect == Dialect::Ufm {
            let ty = match self.peek() {
                Tok::Ident(s) => match s.to_ascii_lowercase().as_str() {
                    "bool" => Some(DeclTy::Bool),
                    "int" => Some(DeclTy::Int),
                    "float" => Some(DeclTy::Float),
                    "complex" => Some(DeclTy::Complex),
                    "color" => Some(DeclTy::Color),
                    _ => None,
                },
                _ => None,
            };
            if let (Some(ty), Tok::Ident(_)) = (ty, self.peek_at(1)) {
                self.next();
                let Tok::Ident(name) = self.next() else { unreachable!() };
                if self.eat_op("[") {
                    let dims = self.index_list()?;
                    // `complex a[0] = v`: an element assignment with a redundant type.
                    if self.eat_op("=") {
                        let value = self.expr()?;
                        self.end_of_statement()?;
                        return Ok(Stmt::Assign { target: Target::Index(name, dims), value, line });
                    }
                    self.end_of_statement()?;
                    return Ok(Stmt::ArrayDecl { ty, name, dims, line });
                }
                if !self.eat_op("=") {
                    self.end_of_statement()?;
                    return Ok(Stmt::Decl { ty, name, value: None, line });
                }
                // complex a = complex b = (0, 0): the inner declarations first.
                let mut chain = vec![(ty, name)];
                loop {
                    let next_ty = match self.peek() {
                        Tok::Ident(s) => match s.to_ascii_lowercase().as_str() {
                            "bool" => Some(DeclTy::Bool),
                            "int" => Some(DeclTy::Int),
                            "float" => Some(DeclTy::Float),
                            "complex" => Some(DeclTy::Complex),
                            "color" => Some(DeclTy::Color),
                            _ => None,
                        },
                        _ => None,
                    };
                    match (next_ty, self.peek().clone(), self.peek_at(1).clone(), self.peek_at(2).clone()) {
                        (Some(t), _, Tok::Ident(n), Tok::Op("=")) => {
                            self.next();
                            self.next();
                            self.next();
                            chain.push((t, n));
                        }
                        // complex z = c1 = c2 = 0: the plain names get the same type.
                        (None, Tok::Ident(n), Tok::Op("="), _) => {
                            self.next();
                            self.next();
                            chain.push((ty, n));
                        }
                        _ => break,
                    }
                }
                let value = self.expr()?;
                self.end_of_statement()?;
                if chain.len() == 1 {
                    let (ty, name) = chain.pop().unwrap();
                    return Ok(Stmt::Decl { ty, name, value: Some(value), line });
                }
                let mut out = Vec::new();
                let mut v = value;
                for (t, n) in chain.into_iter().rev() {
                    out.push(Stmt::Decl { ty: t, name: n.clone(), value: Some(v), line });
                    v = Expr::Ident(n);
                }
                return Ok(Stmt::Seq(out));
            }
        }
        // Assignment target
        let target = match (self.peek().clone(), self.peek_at(1).clone()) {
            (Tok::Ident(f), Tok::Op("(")) if f == "real" || f == "imag" => {
                self.next();
                self.next();
                let var = match self.next() {
                    Tok::Ident(v) => v,
                    Tok::Sys(v) if v.eq_ignore_ascii_case("z") => "z".to_string(),
                    t => return err(line, format!("expected a variable in {f}(…), found {}", show(&t))),
                };
                self.expect_op(")")?;
                Target::Comp { re: f == "real", var }
            }
            (Tok::Ident(v), Tok::Op("=")) => {
                self.next();
                Target::Var(v)
            }
            (Tok::Ident(v), Tok::Op("[")) => {
                self.next();
                self.next();
                let idx = self.index_list()?;
                Target::Index(v, idx)
            }
            (Tok::Ident(f), Tok::Op("(")) if self.dialect == Dialect::Ufm => {
                // A procedure call statement: setLength(a, n).
                self.next();
                self.next();
                let args = self.args()?;
                self.end_of_statement()?;
                return Ok(Stmt::Call { name: f, args, line });
            }
            (Tok::Sys(v), Tok::Op("=")) => {
                self.next();
                Target::Sys(v)
            }
            // `@p = …`: the parameter becomes a variable that starts with its value.
            (Tok::Param(v), Tok::Op("=")) if self.dialect == Dialect::Ufm => {
                self.next();
                Target::Var(format!("@{v}"))
            }
            (t, _) => return err(line, format!("expected a statement, found {}", show(&t))),
        };
        self.expect_op("=")?;
        // z = c = pixel: the inner assignments first, the outer ones read the inner variable.
        let mut chain = vec![target];
        loop {
            match (self.peek().clone(), self.peek_at(1).clone()) {
                (Tok::Ident(v), Tok::Op("=")) => {
                    self.next();
                    self.next();
                    chain.push(Target::Var(v));
                }
                (Tok::Sys(v), Tok::Op("=")) => {
                    self.next();
                    self.next();
                    chain.push(Target::Sys(v));
                }
                (Tok::Ident(v), Tok::Op("[")) => {
                    // `a[i] = …` inside a chain; otherwise the value expression starts here.
                    let save = self.pos;
                    self.next();
                    self.next();
                    match self.index_list() {
                        Ok(idx) if self.eat_op("=") => chain.push(Target::Index(v, idx)),
                        _ => {
                            self.pos = save;
                            break;
                        }
                    }
                }
                _ => break,
            }
        }
        let value = self.expr()?;
        self.end_of_statement()?;
        if chain.len() == 1 {
            return Ok(Stmt::Assign { target: chain.pop().unwrap(), value, line });
        }
        let mut out = Vec::new();
        let mut v = value;
        for t in chain.into_iter().rev() {
            let read = match &t {
                Target::Var(n) => match n.strip_prefix('@') {
                    Some(p) => Expr::Param(p.to_string()),
                    None => Expr::Ident(n.clone()),
                },
                Target::Sys(n) => Expr::Sys(n.clone()),
                Target::Index(n, idx) => Expr::Index(n.clone(), idx.clone()),
                Target::Member(o, f) => Expr::Member(o.clone(), f.clone()),
                Target::MemberIndex(o, f, i) => Expr::MemberIndex(o.clone(), f.clone(), i.clone()),
                Target::Comp { .. } => v.clone(),
            };
            out.push(Stmt::Assign { target: t, value: v, line });
            v = read;
        }
        Ok(Stmt::Seq(out))
    }

    /// Statement position: a bare expression ahead (not an assignment, declaration or block)?
    fn at_expression_statement(&self) -> bool {
        if self.is_kw("if") || self.is_kw("while") || self.is_kw("repeat") {
            return false;
        }
        match (self.peek(), self.peek_at(1)) {
            (Tok::Ident(_) | Tok::Sys(_), Tok::Op("=")) => false,
            (Tok::Ident(_), Tok::Op("[")) => false,
            (Tok::Ident(t), Tok::Ident(_)) if ["bool", "int", "float", "complex"].contains(&t.to_ascii_lowercase().as_str()) => false,
            // real(x) = … assigns a component.
            (Tok::Ident(f), Tok::Op("(")) if f.eq_ignore_ascii_case("real") || f.eq_ignore_ascii_case("imag") => {
                !(matches!(self.peek_at(3), Tok::Op(")")) && matches!(self.peek_at(4), Tok::Op("=")))
            }
            _ => true,
        }
    }

    /// FractInt syntax: `init statements : loop statements, bailout expression` (statements
    /// separated by commas or line ends). Without a colon all statements form a
    /// transformation's `transform` section (UFM transformations may omit the label).
    fn fractint_body(&mut self, f: &mut FormulaAst) -> Result<(), Error> {
        self.fractint = true;
        f.fractint = true;
        let mut first: Vec<Stmt> = Vec::new();
        let mut first_expr: Option<Expr> = None;
        let mut colon = false;
        loop {
            while matches!(self.peek(), Tok::Newline | Tok::Op(",")) {
                self.next();
            }
            if *self.peek() == Tok::Eof || self.is_op("}") || self.at_section().is_some() {
                break;
            }
            if self.eat_op(":") {
                colon = true;
                break;
            }
            if self.at_expression_statement() {
                first_expr = Some(self.expr()?);
                self.end_of_statement()?;
            } else {
                first.push(self.statement()?);
            }
        }
        if !colon {
            f.transform = first;
            f.bailout = first_expr;
            return Ok(());
        }
        f.init = first;
        loop {
            while matches!(self.peek(), Tok::Newline | Tok::Op(",")) {
                self.next();
            }
            if *self.peek() == Tok::Eof || self.is_op("}") || self.at_section().is_some() {
                break;
            }
            if self.at_expression_statement() {
                let e = self.expr()?;
                self.end_of_statement()?;
                // The last bare expression is the bailout condition; earlier ones do nothing.
                f.bailout = Some(e);
            } else {
                f.bailout = None;
                f.body.push(self.statement()?);
            }
        }
        if f.bailout.is_none() {
            // A final assignment: its value is the condition (iterate while it is not zero).
            match f.body.last() {
                Some(Stmt::Assign { target: Target::Var(n), .. }) => f.bailout = Some(Expr::Ident(n.clone())),
                Some(Stmt::Assign { target: Target::Sys(n), .. }) => f.bailout = Some(Expr::Sys(n.clone())),
                _ => return err(self.line(), "the FractInt formula has no bailout condition (its last expression)"),
            }
        }
        Ok(())
    }

    // ---- parameter declarations ----

    /// FRM-like v1 `parameters:` body: `name: real|complex|function = default [domain [a, b]] [classic pN]`.
    fn frm_params(&mut self) -> Result<Vec<ParamDecl>, Error> {
        let mut out = Vec::new();
        loop {
            self.skip_newlines();
            if self.at_section().is_some() || *self.peek() == Tok::Eof || self.is_op("}") {
                return Ok(out);
            }
            let line = self.line();
            let Tok::Ident(name) = self.next() else { return err(line, "expected a parameter name") };
            self.expect_op(":")?;
            let Tok::Ident(ty) = self.next() else { return err(line, "expected a parameter type") };
            self.expect_op("=")?;
            let (kind, default) = match ty.as_str() {
                "real" => (ParamKind::Float, ParamValue::Float(self.signed_number()?)),
                "complex" => {
                    self.expect_op("(")?;
                    let re = self.signed_number()?;
                    self.expect_op(",")?;
                    let im = self.signed_number()?;
                    self.expect_op(")")?;
                    (ParamKind::Complex, ParamValue::Complex([re, im]))
                }
                "function" => match self.next() {
                    Tok::Ident(f) => (ParamKind::Func, ParamValue::Func(f)),
                    t => return err(line, format!("expected a function name, found {}", show(&t))),
                },
                other => return err(line, format!("unknown parameter type '{other}'")),
            };
            let mut p = ParamDecl {
                name: name.clone(),
                kind,
                default,
                caption: name,
                hint: String::new(),
                min: None,
                max: None,
                items: Vec::new(),
                classic: None,
                base: None,
            };
            if self.is_kw("domain") {
                self.next();
                self.expect_op("[")?;
                p.min = Some(self.signed_number()?);
                self.expect_op(",")?;
                p.max = Some(self.signed_number()?);
                self.expect_op("]")?;
            }
            if self.is_kw("classic") {
                self.next();
                match self.next() {
                    Tok::Ident(slot) => p.classic = Some(slot),
                    t => return err(line, format!("expected a classic slot, found {}", show(&t))),
                }
            }
            self.end_of_statement()?;
            out.push(p);
        }
    }

    fn signed_number(&mut self) -> Result<f64, Error> {
        let neg = self.eat_op("-");
        match self.next() {
            Tok::Num(v, _) => Ok(if neg { -v } else { v }),
            t => err(self.line(), format!("expected a number, found {}", show(&t))),
        }
    }

    /// A constant value in the `default:` section: number, complex, bool, string or function.
    fn const_value(&mut self) -> Option<ParamValue> {
        let save = self.pos;
        let neg = self.eat_op("-");
        let v = match self.next() {
            Tok::Num(v, int) => {
                let v = if neg { -v } else { v };
                if int { ParamValue::Int(v as i64) } else { ParamValue::Float(v) }
            }
            Tok::Op("(") => {
                let re = self.signed_number().ok();
                let ok = self.eat_op(",");
                let im = self.signed_number().ok();
                if let (Some(re), true, Some(im), true) = (re, ok, im, self.eat_op(")")) {
                    let s = if neg { -1.0 } else { 1.0 };
                    ParamValue::Complex([s * re, s * im])
                } else {
                    self.pos = save;
                    return None;
                }
            }
            Tok::Ident(s) if s.eq_ignore_ascii_case("true") => ParamValue::Bool(true),
            Tok::Ident(s) if s.eq_ignore_ascii_case("false") => ParamValue::Bool(false),
            Tok::Ident(s) if matches!(s.to_ascii_lowercase().as_str(), "rgb" | "rgba" | "hsl" | "hsla") && self.is_op("(") => {
                self.next();
                let mut v = Vec::new();
                loop {
                    let Some(x) = self.expr().ok().as_ref().and_then(fold_const) else {
                        self.pos = save;
                        return None;
                    };
                    v.push(x);
                    if self.eat_op(")") {
                        break;
                    }
                    if !self.eat_op(",") {
                        self.pos = save;
                        return None;
                    }
                }
                let lower = s.to_ascii_lowercase();
                let n = if lower.ends_with('a') { 4 } else { 3 };
                if v.len() != n {
                    self.pos = save;
                    return None;
                }
                let a = if n == 4 { v[3] } else { 1.0 };
                let [r, g, b] = if lower.starts_with("hsl") { super::color::hsl_to_rgb(v[0], v[1], v[2]) } else { [v[0], v[1], v[2]] };
                ParamValue::Color([r, g, b, a])
            }
            Tok::Ident(s) => {
                // Function default: sin() or sin
                if self.eat_op("(") {
                    let _ = self.eat_op(")");
                }
                ParamValue::Func(s.to_ascii_lowercase())
            }
            _ => {
                self.pos = save;
                return None;
            }
        };
        Some(v)
    }

    /// Skips the rest of one `key = value` item of a parameter block: up to the end of the line
    /// or a comma (one-line blocks: `param x, caption = "X", default = 1, endparam`).
    fn skip_item(&mut self) {
        let mut depth = 0i32;
        loop {
            match self.peek() {
                Tok::Newline | Tok::Eof => break,
                Tok::Op(",") if depth == 0 => break,
                Tok::Op("(") => depth += 1,
                Tok::Op(")") => depth -= 1,
                Tok::Ident(w) if depth == 0 && (w.eq_ignore_ascii_case("endparam") || w.eq_ignore_ascii_case("endfunc")) => return,
                _ => {}
            }
            self.next();
        }
        self.skip_separators();
    }

    fn skip_separators(&mut self) {
        while matches!(self.peek(), Tok::Newline | Tok::Op(",")) {
            self.next();
        }
    }

    fn skip_line(&mut self) {
        while !matches!(self.peek(), Tok::Newline | Tok::Eof) {
            self.next();
        }
        self.skip_newlines();
    }

    /// UFM `default:` section.
    fn ufm_default(&mut self, f: &mut FormulaAst) -> Result<(), Error> {
        loop {
            self.skip_newlines();
            if self.at_section().is_some() || *self.peek() == Tok::Eof || self.is_op("}") {
                return Ok(());
            }
            let Tok::Ident(word) = self.peek().clone() else {
                self.skip_line();
                continue;
            };
            let lower = word.to_ascii_lowercase();
            let typed = match lower.as_str() {
                "bool" => Some(ParamKind::Bool),
                "int" => Some(ParamKind::Int),
                "float" => Some(ParamKind::Float),
                "complex" => Some(ParamKind::Complex),
                "color" => Some(ParamKind::Color),
                _ => None,
            };
            if typed.is_some() && matches!(self.peek_at(1), Tok::Ident(s) if s.eq_ignore_ascii_case("func")) {
                self.next();
                self.next();
                let p = self.ufm_func_block()?;
                f.params.push(p);
                continue;
            }
            // An image parameter: `Image param name … endparam` (the built-in image class).
            if typed.is_none() && word.eq_ignore_ascii_case("image") && matches!(self.peek_at(1), Tok::Ident(s) if s.eq_ignore_ascii_case("param")) {
                self.next();
                self.next();
                if let Some(mut p) = self.ufm_param_block(Some(ParamKind::Image))? {
                    p.base = Some("Image".into());
                    f.params.push(p);
                }
                continue;
            }
            // An object parameter: `Formula param formulaClass … endparam`.
            if typed.is_none() && !is_keyword(&word) && matches!(self.peek_at(1), Tok::Ident(s) if s.eq_ignore_ascii_case("param")) {
                self.next();
                self.next();
                if let Some(mut p) = self.ufm_param_block(Some(ParamKind::Class))? {
                    p.base = Some(word.clone());
                    if let ParamValue::Class { class } = &p.default
                        && class.is_empty()
                    {
                        p.default = ParamValue::Class { class: word.clone() };
                    }
                    f.params.push(p);
                }
                continue;
            }
            if typed.is_some() && matches!(self.peek_at(1), Tok::Ident(s) if s.eq_ignore_ascii_case("param")) {
                self.next();
                self.next();
                let p = self.ufm_param_block(typed)?;
                f.params.extend(p);
                continue;
            }
            match lower.as_str() {
                "param" => {
                    self.next();
                    let p = self.ufm_param_block(None)?;
                    f.params.extend(p);
                }
                "func" => {
                    self.next();
                    let p = self.ufm_func_block()?;
                    f.params.push(p);
                }
                "heading" => {
                    while !(self.is_kw("endheading") || *self.peek() == Tok::Eof) {
                        self.next();
                    }
                    self.next();
                }
                "outsidecoloring" | "insidecoloring" => {
                    self.next();
                    if self.eat_op("=")
                        && let Tok::Str(s) = self.peek().clone()
                    {
                        self.next();
                        if lower == "outsidecoloring" {
                            f.outside_coloring = Some(s);
                        } else {
                            f.inside_coloring = Some(s);
                        }
                    }
                    self.skip_line();
                }
                "distance" => {
                    self.next();
                    if self.eat_op("=")
                        && let Tok::Str(s) = self.peek().clone()
                    {
                        self.next();
                        f.distance = Some(s);
                    }
                    self.skip_line();
                }
                "bound" => {
                    self.next();
                    if self.eat_op("=") {
                        f.bound = self.const_value().map(|v| v.as_f64());
                    }
                    self.skip_line();
                }
                "title" | "center" | "magn" | "maxiter" => {
                    self.next();
                    if self.eat_op("=") {
                        match (lower.as_str(), self.peek().clone()) {
                            ("title", Tok::Str(s)) => {
                                self.next();
                                f.title = Some(s);
                            }
                            ("center", _) => {
                                if let Some(ParamValue::Complex(c)) = self.const_value() {
                                    f.center = Some(c);
                                }
                            }
                            ("magn", _) => f.magn = self.const_value().map(|v| v.as_f64()),
                            ("maxiter", _) => f.maxiter = self.const_value().map(|v| v.as_f64().max(1.0) as u32),
                            _ => {}
                        }
                    }
                    self.skip_line();
                }
                _ => self.skip_line(),
            }
        }
    }

    /// `[type] param name … endparam`.
    fn ufm_param_block(&mut self, kind: Option<ParamKind>) -> Result<Option<ParamDecl>, Error> {
        let line = self.line();
        let name = match self.next() {
            Tok::Ident(n) | Tok::Param(n) => n,
            t => return err(line, format!("expected a parameter name, found {}", show(&t))),
        };
        let mut p = ParamDecl {
            name: name.clone(),
            kind: kind.unwrap_or(ParamKind::Float),
            default: ParamValue::Float(0.0),
            caption: name,
            hint: String::new(),
            min: None,
            max: None,
            items: Vec::new(),
            classic: None,
            base: None,
        };
        let mut default = None;
        self.skip_separators();
        loop {
            if self.is_kw("endparam") {
                self.next();
                break;
            }
            if *self.peek() == Tok::Eof || self.is_op("}") {
                return err(line, "'param' without 'endparam'");
            }
            let Tok::Ident(key) = self.peek().clone() else {
                self.skip_item();
                continue;
            };
            self.next();
            if !self.eat_op("=") {
                self.skip_item();
                continue;
            }
            match key.to_ascii_lowercase().as_str() {
                "caption" => {
                    if let Tok::Str(s) = self.peek().clone() {
                        p.caption = s;
                    }
                }
                "hint" => {
                    if let Tok::Str(s) = self.peek().clone() {
                        p.hint = s;
                    }
                }
                "default" if p.kind == ParamKind::Class => {
                    // A class name, possibly qualified: Name or file.ulb:Name.
                    let mut name = String::new();
                    while let Tok::Ident(t) | Tok::Param(t) = self.peek().clone() {
                        self.next();
                        name = self.class_name_rest(t);
                    }
                    default = Some(ParamValue::Class { class: name });
                }
                "default" => default = self.const_value(),
                "min" => p.min = self.const_value().map(|v| v.as_f64()),
                "max" => p.max = self.const_value().map(|v| v.as_f64()),
                "enum" => {
                    while let Tok::Str(s) = self.peek().clone() {
                        p.items.push(s);
                        self.next();
                    }
                }
                _ => {}
            }
            self.skip_item();
        }
        if kind.is_none() {
            p.kind = match &default {
                Some(ParamValue::Complex(_)) => ParamKind::Complex,
                Some(ParamValue::Color(_)) => ParamKind::Color,
                Some(ParamValue::Bool(_)) => ParamKind::Bool,
                Some(ParamValue::Int(_)) if !p.items.is_empty() => ParamKind::Int,
                _ if !p.items.is_empty() => ParamKind::Int,
                _ => ParamKind::Float,
            };
        }
        p.default = match (p.kind, default) {
            (ParamKind::Bool, Some(v)) => ParamValue::Bool(v.as_f64() != 0.0),
            (ParamKind::Bool, None) => ParamValue::Bool(false),
            (ParamKind::Int, Some(v)) => ParamValue::Int(v.as_f64() as i64),
            (ParamKind::Int, None) => ParamValue::Int(0),
            (ParamKind::Complex, Some(v)) => ParamValue::Complex(v.as_complex()),
            (ParamKind::Complex, None) => ParamValue::Complex([0.0, 0.0]),
            (ParamKind::Class, Some(v @ ParamValue::Class { .. })) => v,
            (ParamKind::Class, _) => ParamValue::Class { class: String::new() },
            (ParamKind::Image, _) => ParamValue::Image { path: String::new() },
            (ParamKind::Color, Some(v)) => ParamValue::Color(v.as_color()),
            (ParamKind::Color, None) => ParamValue::Color([0.0, 0.0, 0.0, 1.0]),
            (_, Some(v)) => ParamValue::Float(v.as_f64()),
            (_, None) => ParamValue::Float(0.0),
        };
        Ok(Some(p))
    }

    fn ufm_func_block(&mut self) -> Result<ParamDecl, Error> {
        let line = self.line();
        let name = match self.next() {
            Tok::Ident(n) | Tok::Param(n) => n,
            t => return err(line, format!("expected a function parameter name, found {}", show(&t))),
        };
        let mut p = ParamDecl {
            name: name.clone(),
            kind: ParamKind::Func,
            default: ParamValue::Func("ident".into()),
            caption: name,
            hint: String::new(),
            min: None,
            max: None,
            items: Vec::new(),
            classic: None,
            base: None,
        };
        self.skip_separators();
        loop {
            if self.is_kw("endfunc") {
                self.next();
                return Ok(p);
            }
            if *self.peek() == Tok::Eof || self.is_op("}") {
                return err(line, "'func' without 'endfunc'");
            }
            let Tok::Ident(key) = self.peek().clone() else {
                self.skip_item();
                continue;
            };
            self.next();
            if self.eat_op("=") {
                match key.to_ascii_lowercase().as_str() {
                    "caption" => {
                        if let Tok::Str(s) = self.peek().clone() {
                            p.caption = s;
                        }
                    }
                    "hint" => {
                        if let Tok::Str(s) = self.peek().clone() {
                            p.hint = s;
                        }
                    }
                    "default" => {
                        if let Some(v @ ParamValue::Func(_)) = self.const_value() {
                            p.default = v;
                        }
                    }
                    _ => {}
                }
            }
            self.skip_item();
        }
    }

    /// The body of one formula up to its closing brace (or the end).
    fn formula_body(&mut self, name: String) -> Result<FormulaAst, Error> {
        let mut f = FormulaAst {
            name,
            dialect: self.dialect,
            params: Vec::new(),
            global: Vec::new(),
            init: Vec::new(),
            body: Vec::new(),
            bailout: None,
            fin: Vec::new(),
            title: None,
            center: None,
            magn: None,
            maxiter: None,
            skipped_sections: Vec::new(),
            transform: Vec::new(),
            fractint: false,
            imports: Vec::new(),
            funcs: Vec::new(),
            outside_coloring: None,
            inside_coloring: None,
            distance: None,
            bound: None,
        };
        let mut seen_section = false;
        loop {
            self.skip_newlines();
            if *self.peek() == Tok::Eof || self.is_op("}") {
                break;
            }
            let Some(section) = self.at_section() else {
                if !seen_section && self.dialect == Dialect::Ufm {
                    // No section label first: FractInt syntax (sections such as default: may follow).
                    self.fractint_body(&mut f)?;
                    seen_section = true;
                    continue;
                }
                if !seen_section {
                    return err(self.line(), "expected a section such as 'init:'");
                }
                return err(self.line(), format!("unexpected {}", show(self.peek())));
            };
            seen_section = true;
            self.next();
            self.next();
            match section.as_str() {
                "parameters" => f.params = self.frm_params()?,
                "default" => self.ufm_default(&mut f)?,
                "global" => f.global = self.block(&[])?,
                "init" => f.init = self.block(&[])?,
                "loop" => f.body = self.block(&[])?,
                "final" => f.fin = self.block(&[])?,
                "transform" => f.transform = self.block(&[])?,
                "bailout" => {
                    self.skip_newlines();
                    if self.at_section().is_none() && !self.is_op("}") {
                        f.bailout = Some(self.expr()?);
                        self.end_of_statement()?;
                    }
                }
                other => {
                    f.skipped_sections.push(other.to_string());
                    // Skip to the next section.
                    loop {
                        self.skip_newlines();
                        if self.at_section().is_some() || *self.peek() == Tok::Eof || self.is_op("}") {
                            break;
                        }
                        self.skip_line();
                    }
                }
            }
        }
        f.imports = std::mem::take(&mut self.imports);
        f.funcs = std::mem::take(&mut self.funcs);
        Ok(f)
    }

    // ---- classes ----

    /// A type name in class code (built-in or class).
    fn type_ref(&mut self) -> Result<TypeRef, Error> {
        let line = self.line();
        match self.next() {
            Tok::Ident(t) => Ok(match builtin_ty(&t) {
                Some(d) => TypeRef::Builtin(d),
                None => TypeRef::Class(self.class_name_rest(t)),
            }),
            t => err(line, format!("expected a type, found {}", show(&t))),
        }
    }

    /// A class name, possibly qualified by its file (`common.ulb:Generic`): the class part.
    fn class_name_rest(&mut self, first: String) -> String {
        let mut name = first;
        // file.ulb:Name
        if self.is_op(".") && matches!(self.peek_at(1), Tok::Ident(_)) && matches!(self.peek_at(2), Tok::Op(":")) {
            self.next();
            self.next();
            self.next();
            if let Tok::Ident(n) = self.next() {
                name = n;
            }
        }
        name
    }

    /// `func Name(params) … endfunc` (at `func`; the return type is read already).
    fn method_decl(&mut self, ret: Option<TypeRef>, is_static: bool, line: usize) -> Result<MethodAst, Error> {
        self.next();
        let name = match self.next() {
            Tok::Ident(n) => n,
            t => return err(line, format!("expected a method name, found {}", show(&t))),
        };
        self.expect_op("(")?;
        let mut params = Vec::new();
        if !self.eat_op(")") {
            loop {
                if self.is_kw("const") {
                    self.next();
                }
                let ty = self.type_ref()?;
                let by_ref = self.eat_op("&");
                let pname = match self.next() {
                    Tok::Ident(n) => n,
                    t => return err(self.line(), format!("expected a parameter name, found {}", show(&t))),
                };
                // Array parameters: name[]
                if self.eat_op("[") {
                    while !self.eat_op("]") {
                        self.next();
                    }
                }
                params.push(MethodParam { ty, name: pname, by_ref });
                if self.eat_op(")") {
                    break;
                }
                self.expect_op(",")?;
            }
        }
        self.end_of_statement()?;
        let body = self.block(&["endfunc"])?;
        if !self.is_kw("endfunc") {
            return err(line, format!("'func {name}' without 'endfunc'"));
        }
        self.next();
        self.end_of_statement()?;
        Ok(MethodAst { name, ret, params, body, is_static, line })
    }

    /// Members of a class body up to its end.
    fn class_body(&mut self, c: &mut ClassAst) -> Result<(), Error> {
        loop {
            self.skip_newlines();
            if *self.peek() == Tok::Eof || self.is_op("}") {
                return Ok(());
            }
            if let Some(section) = self.at_section() {
                self.next();
                self.next();
                if section == "default" {
                    let mut f = FormulaAst {
                        name: c.name.clone(),
                        dialect: Dialect::Ufm,
                        params: Vec::new(),
                        global: Vec::new(),
                        init: Vec::new(),
                        body: Vec::new(),
                        bailout: None,
                        fin: Vec::new(),
                        title: None,
                        center: None,
                        magn: None,
                        maxiter: None,
                        skipped_sections: Vec::new(),
                        transform: Vec::new(),
                        fractint: false,
                        imports: Vec::new(),
                        funcs: Vec::new(),
                        outside_coloring: None,
                        inside_coloring: None,
                        distance: None,
                        bound: None,
                    };
                    self.ufm_default(&mut f)?;
                    c.params.extend(f.params);
                    c.title = f.title;
                }
                continue;
            }
            if self.is_kw("import") {
                self.skip_line();
                continue;
            }
            let line = self.line();
            let is_static = self.is_kw("static");
            if is_static {
                self.next();
            }
            if self.is_kw("const") {
                self.next();
            }
            // [type] func Name(params)
            let ret = if self.is_kw("func") { None } else { Some(self.type_ref()?) };
            if self.is_kw("func") {
                let m = self.method_decl(ret, is_static, line)?;
                c.methods.push(m);
                continue;
            }
            // Fields: type name[, name …] [dims]
            let mut ty = ret.ok_or_else(|| Error { line, msg: "expected a member".into() })?;
            loop {
                // `float h, float v`: a new type in the list.
                if matches!((self.peek(), self.peek_at(1)), (Tok::Ident(_), Tok::Ident(_))) {
                    ty = self.type_ref()?;
                }
                let name = match self.next() {
                    Tok::Ident(n) => n,
                    t => return err(line, format!("expected a field name, found {}", show(&t))),
                };
                let dims = if self.eat_op("[") { Some(self.index_list()?) } else { None };
                c.fields.push(FieldAst { ty: ty.clone(), name, dims });
                if self.eat_op("=") {
                    // Field initializers are rare; the value is ignored (fields start at zero).
                    while !matches!(self.peek(), Tok::Newline | Tok::Eof) && !self.is_op(",") {
                        self.next();
                    }
                }
                if !self.eat_op(",") {
                    break;
                }
            }
            self.end_of_statement()?;
        }
    }
}

/// A built-in type name.
fn builtin_ty(t: &str) -> Option<DeclTy> {
    Some(match t.to_ascii_lowercase().as_str() {
        "bool" => DeclTy::Bool,
        "int" => DeclTy::Int,
        "float" => DeclTy::Float,
        "complex" => DeclTy::Complex,
        "color" => DeclTy::Color,
        _ => return None,
    })
}

/// Words that start statements and cannot be class names.
fn is_keyword(t: &str) -> bool {
    matches!(
        t.to_ascii_lowercase().as_str(),
        "if" | "elseif"
            | "else"
            | "endif"
            | "while"
            | "endwhile"
            | "repeat"
            | "until"
            | "return"
            | "import"
            | "func"
            | "endfunc"
            | "static"
            | "const"
            | "new"
            | "this"
    )
}

/// Tokens of an entry body (with FractInt's `end if` merged).
fn entry_tokens(body: &str, line: usize) -> Result<Vec<Token>, Error> {
    let mut toks = lex(body, line)?;
    let mut k = 0;
    while k + 1 < toks.len() {
        if matches!(&toks[k].tok, Tok::Ident(a) if a.eq_ignore_ascii_case("end"))
            && matches!(&toks[k + 1].tok, Tok::Ident(b) if b.eq_ignore_ascii_case("if"))
        {
            toks[k].tok = Tok::Ident("endif".into());
            toks.remove(k + 1);
        }
        k += 1;
    }
    Ok(toks)
}

/// The classes of a class library (`.ulb`): `class Name(Parent) { … }` entries.
pub fn parse_classes(src: &str) -> Vec<(String, Result<ClassAst, Error>)> {
    let pre = preprocess(src);
    split_entries(&pre)
        .iter()
        .filter_map(|e| {
            let head = e.name.trim();
            let rest = head.strip_prefix("class").or_else(|| head.strip_prefix("Class"))?.trim();
            let (name, parent) = match rest.split_once('(') {
                Some((n, p)) => {
                    let p = p.trim_end_matches(')').trim();
                    let p = p.rsplit(':').next().unwrap_or(p).trim();
                    (n.trim().to_string(), (!p.is_empty()).then(|| p.to_string()))
                }
                None => (rest.to_string(), None),
            };
            let mut c =
                ClassAst { name: name.clone(), parent, fields: Vec::new(), methods: Vec::new(), params: Vec::new(), title: None, line: e.line };
            let r = entry_tokens(e.body, e.line).and_then(|toks| {
                let mut p = Parser { toks, pos: 0, dialect: Dialect::Ufm, fractint: false, imports: Vec::new(), funcs: Vec::new(), paren_depth: 0 };
                p.class_body(&mut c)
            });
            Some((name, r.map(|_| c)))
        })
        .collect()
}

/// The assignment target an expression names (`x`, `@p`, `#z`, `a[i]`, `o.f`, `o.f[i]`).
fn assign_target(e: &Expr) -> Option<Target> {
    Some(match e {
        Expr::Ident(n) if !is_keyword(n) => Target::Var(n.clone()),
        Expr::Param(n) => Target::Var(format!("@{n}")),
        Expr::Sys(n) => Target::Sys(n.clone()),
        Expr::Index(n, i) => Target::Index(n.clone(), i.clone()),
        Expr::Member(o, f) => Target::Member(o.clone(), f.clone()),
        Expr::MemberIndex(o, f, i) => Target::MemberIndex(o.clone(), f.clone(), i.clone()),
        _ => return None,
    })
}

fn show(t: &Tok) -> String {
    match t {
        Tok::Ident(s) => format!("'{s}'"),
        Tok::Sys(s) => format!("'#{s}'"),
        Tok::Param(s) => format!("'@{s}'"),
        Tok::Num(v, _) => format!("'{v}'"),
        Tok::Imag(v) => format!("'{v}i'"),
        Tok::Str(s) => format!("\"{s}\""),
        Tok::Op(o) => format!("'{o}'"),
        Tok::Newline => "the end of the line".into(),
        Tok::Eof => "the end of the file".into(),
    }
}

/// Parses a FractalPark FRM-like v1 definition (directive preamble plus one formula).
pub fn parse_frm(src: &str) -> Result<FormulaAst, Error> {
    let mut header = None;
    let mut offset = 0;
    for (i, raw) in src.split_inclusive('\n').enumerate() {
        let l = raw.trim_end_matches(['\n', '\r']);
        let t = l.trim();
        let line_start = offset;
        offset += raw.len();
        if t.is_empty() || t.starts_with(';') {
            if let Some(rest) = t.strip_prefix(';').map(str::trim).and_then(|r| r.strip_prefix('@')) {
                let (key, value) = rest.split_once(':').map(|(k, v)| (k.trim(), v.trim())).unwrap_or((rest, ""));
                let ok = match key {
                    "language" => value == "frm-like/1",
                    "stdlib" => value == "1",
                    "numeric-profile" => value == "standard32",
                    "classic-guards" => true,
                    _ => false,
                };
                if !ok {
                    return err(i + 1, format!("unsupported directive '@{key}: {value}'"));
                }
            }
            continue;
        }
        header = Some((i, l, line_start));
        break;
    }
    let Some((hi, hl, start)) = header else { return err(0, "no formula in the file") };
    let Some((name, rest)) = hl.split_once('{') else { return err(hi + 1, "expected 'Name {'") };
    let body = &src[start + (hl.len() - rest.len())..];
    let toks = lex(body, hi + 1)?;
    let mut p = Parser { toks, pos: 0, dialect: Dialect::Frm, fractint: false, imports: Vec::new(), funcs: Vec::new(), paren_depth: 0 };
    let f = p.formula_body(name.trim().to_string())?;
    p.expect_op("}")?;
    Ok(f)
}

/// Symbols the preprocessor treats as defined (`$ifdef VER50`): RFract reads formulas written
/// for version 5 of the UFM language.
const PREDEFINED: [&str; 3] = ["VER30", "VER40", "VER50"];

/// Applies `$define`, `$undef`, `$ifdef`, `$ifndef`, `$else` and `$endif` (case-insensitive).
/// Removed lines stay as empty lines, so line numbers keep matching the file.
pub fn preprocess(src: &str) -> String {
    if !src.contains('$') {
        return src.to_string();
    }
    let mut defined: std::collections::HashSet<String> = PREDEFINED.iter().map(|s| s.to_string()).collect();
    // Stack of (this branch active, any branch taken, parent active).
    let mut stack: Vec<(bool, bool)> = Vec::new();
    let active = |stack: &Vec<(bool, bool)>| stack.iter().all(|(a, _)| *a);
    let mut out = String::with_capacity(src.len());
    for line in src.split_inclusive('\n') {
        let t = line.trim_start();
        if let Some(rest) = t.strip_prefix('$') {
            let mut words = rest.split(|c: char| c.is_whitespace() || c == ';');
            let dir = words.next().unwrap_or("").to_ascii_lowercase();
            let name = words.find(|w| !w.is_empty()).unwrap_or("").to_ascii_uppercase();
            match dir.as_str() {
                "define" if active(&stack) => {
                    defined.insert(name);
                }
                "undef" if active(&stack) => {
                    defined.remove(&name);
                }
                "ifdef" => stack.push((defined.contains(&name), false)),
                "ifndef" => stack.push((!defined.contains(&name), false)),
                "else" => {
                    if let Some(top) = stack.last_mut() {
                        top.1 = true;
                        top.0 = !top.0;
                    }
                }
                "endif" => {
                    stack.pop();
                }
                _ => {}
            }
            out.push_str(if line.ends_with('\n') { "\n" } else { "" });
            continue;
        }
        if active(&stack) {
            out.push_str(line);
        } else {
            out.push_str(if line.ends_with('\n') { "\n" } else { "" });
        }
    }
    out
}

/// One entry of a UFM syntax file: its name, the line it starts on and its body.
#[derive(Clone, Debug)]
pub struct Entry<'a> {
    pub name: String,
    pub line: usize,
    pub body: &'a str,
}

/// An entry name without a FractInt symmetry suffix (`Mandelbrot(XAXIS)` → `Mandelbrot`).
fn strip_symmetry(name: &str) -> String {
    if let Some(open) = name.rfind('(')
        && name.ends_with(')')
    {
        let sym = name[open + 1..name.len() - 1].trim().to_ascii_uppercase();
        if [
            "XAXIS",
            "YAXIS",
            "XYAXIS",
            "ORIGIN",
            "PI_SYM",
            "NOSYM",
            "XAXIS_NOPARM",
            "XAXIS_NOREAL",
            "XAXIS_NOIMAG",
            "YAXIS_NOPARM",
            "XYAXIS_NOPARM",
            "ORIGIN_NOPARM",
            "PI_SYM_NOPARM",
        ]
        .contains(&sym.as_str())
        {
            return name[..open].trim().to_string();
        }
    }
    name.to_string()
}

/// Splits a UFM formula file into entries (`name { … }`), skipping `comment { … }`
/// blocks. Braces inside strings and `;` comments are ignored.
pub fn split_entries(src: &str) -> Vec<Entry<'_>> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    let mut line = 1;
    let mut name_start = 0;
    let mut name_line = 1;
    while i < b.len() {
        match b[i] {
            b'\n' => {
                line += 1;
                i += 1;
                if name_start >= i - 1 || src[name_start..i].trim().is_empty() {
                    name_start = i;
                    name_line = line;
                }
            }
            b';' => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'{' => {
                let name = src[name_start..i].trim().to_string();
                let body_start = i + 1;
                let body_line = line;
                // Find the matching brace.
                let mut depth = 1;
                let mut j = body_start;
                let mut in_str = false;
                while j < b.len() && depth > 0 {
                    match b[j] {
                        b'"' => in_str = !in_str,
                        b'\n' => {
                            in_str = false;
                            line += 1;
                        }
                        b';' if !in_str => {
                            while j < b.len() && b[j] != b'\n' {
                                j += 1;
                            }
                            continue;
                        }
                        b'{' if !in_str => depth += 1,
                        b'}' if !in_str => depth -= 1,
                        _ => {}
                    }
                    j += 1;
                }
                let body_end = if depth == 0 { j - 1 } else { j };
                if !name.eq_ignore_ascii_case("comment") && !name.is_empty() {
                    out.push(Entry {
                        name: strip_symmetry(name.lines().last().unwrap_or("").trim()),
                        line: name_line.max(body_line),
                        body: &src[body_start..body_end],
                    });
                }
                i = j;
                name_start = i;
                name_line = line;
            }
            _ => i += 1,
        }
    }
    out
}

/// Parses one entry of a UFM syntax file.
pub fn parse_ufm_entry(e: &Entry) -> Result<FormulaAst, Error> {
    let toks = entry_tokens(e.body, e.line)?;
    let mut p = Parser { toks, pos: 0, dialect: Dialect::Ufm, fractint: false, imports: Vec::new(), funcs: Vec::new(), paren_depth: 0 };
    p.formula_body(e.name.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_frm_definition() {
        let src = "; @language: frm-like/1\n; @stdlib: 1\n; @numeric-profile: standard32\nPowerJulia {\n  parameters:\n    power: real = 2 domain [1, 16] classic p1\n    offset: complex = (-0.75, 0.1) classic p2\n    transform: function = sin classic fn1\n  init:\n    z = pixel\n  loop:\n    z = transform(z ^ power) + offset\n    if real(z) > 0\n      real(z) = -z ^ 2\n    elseif imag(z) < 0\n      z = |z| + c\n    else\n      z = (0, 1)\n    endif\n  bailout:\n    |z| <= 4 && !ismand\n}\n";
        let f = parse_frm(src).unwrap();
        assert_eq!(f.name, "PowerJulia");
        assert_eq!(f.params.len(), 3);
        assert_eq!(f.params[0].max, Some(16.0));
        assert_eq!(f.params[2].default, ParamValue::Func("sin".into()));
        assert_eq!(f.body.len(), 2);
        // -z ^ 2 is (-z) ^ 2 in FRM-like v1
        let Stmt::If { arms, .. } = &f.body[1] else { panic!() };
        let Stmt::Assign { value, .. } = &arms[0].1[0] else { panic!() };
        assert!(matches!(value, Expr::Binary(BinOp::Pow, l, _) if matches!(**l, Expr::Unary(UnOp::Neg, _))));
    }

    #[test]
    fn parses_ufm_entries() {
        let src = "comment {\n a { b } \n}\nMy-Brot {\n; c\ninit:\n  z = 0, float k = 2\nloop:\n  z = -z^@power + #pixel\nbailout:\n  |z| <= @bailout\ndefault:\n  title = \"My Brot\"\n  center = (-0.5, 0)\n  maxiter = 250\n  param power\n    caption = \"Power\"\n    default = (2,0)\n  endparam\n  float param bailout\n    default = 4.0\n    min = 1\n  endparam\n  int param mode\n    enum = \"a\" \"b\"\n    default = 1\n  endparam\n  func fn1\n    default = sin()\n  endfunc\n}\n";
        let e = split_entries(src);
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].name, "My-Brot");
        let f = parse_ufm_entry(&e[0]).unwrap();
        assert_eq!(f.title.as_deref(), Some("My Brot"));
        assert_eq!(f.maxiter, Some(250));
        assert_eq!(f.init.len(), 2);
        assert_eq!(f.params.len(), 4);
        assert_eq!(f.params[0].kind, ParamKind::Complex);
        assert_eq!(f.params[2].items, vec!["a", "b"]);
        assert_eq!(f.params[3].default, ParamValue::Func("sin".into()));
        // -z^p is -(z^p) in UFM syntax
        let Stmt::Assign { value, .. } = &f.body[0] else { panic!() };
        assert!(matches!(value, Expr::Binary(BinOp::Add, l, _) if matches!(**l, Expr::Unary(UnOp::Neg, _))));
    }
}
