// SPDX-License-Identifier: LGPL-3.0-or-later

//! Periodicity checking: whether an orbit that returns to an earlier `z` is caught in a cycle
//! for good. That holds when one loop iteration depends on nothing but `z` and constants: the
//! loop carries no other local from one iteration to the next and reads neither `#numiter`
//! nor `#random`, and the bailout reads no `#numiter` either.

use super::deriv::for_children_ref;
use super::ir::{Expr, Kind, Program, Stmt, Sys, Z};

/// The fractal's loop is a map of `z` alone (see the module notes).
pub fn z_only_state(prog: &Program) -> bool {
    let n = prog.locals.len();
    let mut assigned = vec![false; n];
    assigned_in(&prog.body, &mut assigned);
    let mut defined = vec![false; n];
    defined[Z] = true;
    let check = Check { assigned: &assigned };
    check.stmts(&prog.body, &mut defined) && check.expr(&prog.bailout, &defined)
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

struct Check<'a> {
    /// Locals the loop assigns: reading one before this iteration assigned it carries it over.
    assigned: &'a [bool],
}

impl Check<'_> {
    /// Walks `list`, marking what it assigns for sure in `defined`; false when a carried local
    /// is read or the iteration depends on the iteration count or randomness.
    fn stmts(&self, list: &[Stmt], defined: &mut Vec<bool>) -> bool {
        for s in list {
            let ok = match s {
                Stmt::Assign(i, e) => {
                    let ok = self.expr(e, defined);
                    defined[*i] = true;
                    ok
                }
                // A part of a local keeps the other part: it must be this iteration's already.
                Stmt::SetPart { local, value, .. } => self.expr(value, defined) && (defined[*local] || !self.assigned[*local]),
                // Arrays keep their other elements.
                Stmt::SetIndex(..) => false,
                Stmt::If(arms, els) => {
                    let mut ok = true;
                    let mut joined: Option<Vec<bool>> = None;
                    for (cond, body) in arms {
                        ok &= self.expr(cond, defined);
                        let mut d = defined.clone();
                        ok &= self.stmts(body, &mut d);
                        joined = Some(join(joined, d));
                    }
                    let mut d = defined.clone();
                    ok &= self.stmts(els, &mut d);
                    *defined = join(joined, d);
                    ok
                }
                // A loop body may not run: what it assigns stays undefined after it.
                Stmt::While(cond, body) => self.expr(cond, defined) && self.stmts(body, &mut defined.clone()),
                Stmt::Repeat(body, cond) => self.stmts(body, defined) && self.expr(cond, defined),
            };
            if !ok {
                return false;
            }
        }
        true
    }

    fn expr(&self, e: &Expr, defined: &[bool]) -> bool {
        let ok = match e.kind {
            Kind::Sys(Sys::NumIter | Sys::Random) => false,
            Kind::Local(i) | Kind::Index(i, _) => defined[i] || !self.assigned[i],
            _ => true,
        };
        let mut all = ok;
        for_children_ref(e, &mut |c| all = all && self.expr(c, defined));
        all
    }
}

/// Locals assigned in every branch so far.
fn join(a: Option<Vec<bool>>, b: Vec<bool>) -> Vec<bool> {
    match a {
        None => b,
        Some(a) => a.iter().zip(&b).map(|(x, y)| *x && *y).collect(),
    }
}

/// `c` lies in the main cardioid or the period-2 bulb of the Mandelbrot set z² + c.
pub fn in_main_bulbs(re: f64, im: f64) -> bool {
    let x = re - 0.25;
    let y2 = im * im;
    let q = x * x + y2;
    q * (q + x) <= 0.25 * y2 || (re + 1.0) * (re + 1.0) + y2 <= 0.0625
}

#[cfg(test)]
mod tests {
    use super::super::check::check;
    use super::super::parse::{parse_ufm_entry, split_entries};
    use super::*;

    fn z_only(src: &str) -> bool {
        z_only_state(&check(&parse_ufm_entry(&split_entries(src)[0]).unwrap()).unwrap())
    }

    #[test]
    fn finds_maps_of_z_alone() {
        assert!(z_only("M {\ninit:\n  z = 0\nloop:\n  z = z^2 + #pixel\nbailout:\n  |z| <= 4\n}"));
        // Temporaries assigned before use are fine, so are constants from init.
        assert!(z_only("M {\ninit:\n  z = 0\n  complex k = 2\nloop:\n  complex t = z * z\n  z = t + #pixel * k\nbailout:\n  |z| <= 4\n}"));
        assert!(z_only(
            "M {\ninit:\n  z = 0\nloop:\n  if real(z) > 0\n    complex t = z\n  else\n    complex t = -z\n  endif\n  z = t * t + #pixel\nbailout:\n  |z| <= 4\n}"
        ));
        // Carried state (Phoenix), the iteration count, randomness, a temporary set in one arm.
        assert!(!z_only(
            "P {\ninit:\n  z = 0\n  complex y = 0\nloop:\n  complex n = z * z + #pixel + 0.5 * y\n  y = z\n  z = n\nbailout:\n  |z| <= 4\n}"
        ));
        assert!(!z_only("M {\ninit:\n  z = 0\nloop:\n  z = z^2 + #pixel\nbailout:\n  #numiter < 10\n}"));
        assert!(!z_only("M {\ninit:\n  z = 0\nloop:\n  z = z^2 + #pixel + 0.001 * #random\nbailout:\n  |z| <= 4\n}"));
        assert!(!z_only(
            "M {\ninit:\n  z = 0\n  complex t = 0\nloop:\n  if real(z) > 0\n    t = z\n  endif\n  z = t * t + #pixel\nbailout:\n  |z| <= 4\n}"
        ));
    }

    #[test]
    fn main_bulbs() {
        assert!(in_main_bulbs(0.0, 0.0) && in_main_bulbs(-1.0, 0.0) && in_main_bulbs(0.2, 0.3));
        assert!(!in_main_bulbs(0.3, 0.0) && !in_main_bulbs(-0.75, 0.2) && !in_main_bulbs(-1.3, 0.0));
    }
}
