//! Constant folding, algebraic identities, and constant `if`s, in one
//! post-order walk, so each node sees its operands already folded.
//!
//! * Constants: operators on literals become a literal (`1 + 2` → `3`).
//!   Integers wrap as in the VM; a division or remainder that would trap (by
//!   zero, or `i64::MIN / -1`) is left for the runtime to report.
//! * Identities: `x + 0`, `0 + x`, `x - 0`, `x * 1`, `1 * x` → `x`, and
//!   `x * 0`, `0 * x` → `0` if `x` is pure. They also fire on floats, where
//!   they are inexact: `-0.0 + 0.0` is `0.0`, and `x * 0.0` is `-0.0` or NaN
//!   for negative or non-finite `x`.
//! * Short-circuits with a literal left side: `true && x` → `x`,
//!   `false && x` → `false`, `true || x` → `true`, `false || x` → `x`.
//! * Constant conditions: `if true { a } else { b }` → `a`, and likewise for
//!   `false`.

use crate::hir::{BinOp, Block, Expr, ExprKind, Hir, UnOp};
use crate::opt::{VisitMut, is_pure, unit_expr, walk_block_mut, walk_expr_mut};
use crate::sema::types::Type;
use crate::span::Span;

/// Runs the fold pass over the whole program; returns the number of rewrites.
pub fn run(hir: &mut Hir) -> usize {
    let mut folder = Folder { count: 0 };
    for func in &mut hir.functions {
        folder.visit_block(&mut func.body);
    }
    folder.count
}

struct Folder {
    count: usize,
}

impl VisitMut for Folder {
    fn visit_block(&mut self, block: &mut Block) {
        walk_block_mut(self, block);
        if let Some(tail) = &block.tail {
            block.ty = tail.ty;
        }
    }

    fn visit_expr(&mut self, expr: &mut Expr) {
        walk_expr_mut(self, expr);
        // Take the kind so its children can be moved into a replacement; put
        // it back if nothing applies.
        let kind = std::mem::replace(&mut expr.kind, ExprKind::Bool(false));
        match rewrite(kind, expr.ty, expr.span) {
            Ok(replacement) => {
                *expr = replacement;
                self.count += 1;
            }
            Err(kind) => expr.kind = kind,
        }
    }
}

/// Rewrites one node: `Ok` holds the replacement, `Err` the unchanged kind.
fn rewrite(kind: ExprKind, ty: Type, span: Span) -> Result<Expr, ExprKind> {
    match kind {
        ExprKind::Unary { op, rhs } => fold_unary(op, rhs, ty, span),
        ExprKind::Binary { op, lhs, rhs } => fold_binary(op, lhs, rhs, ty, span),
        ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => fold_if(*cond, then_branch, else_branch, ty, span),
        other => Err(other),
    }
}

fn fold_unary(op: UnOp, rhs: Box<Expr>, ty: Type, span: Span) -> Result<Expr, ExprKind> {
    let folded = match (op, &rhs.kind) {
        (UnOp::Neg, ExprKind::Int(v)) => Some(ExprKind::Int(v.wrapping_neg())),
        (UnOp::Neg, ExprKind::Float(v)) => Some(ExprKind::Float(-v)),
        (UnOp::Not, ExprKind::Bool(v)) => Some(ExprKind::Bool(!v)),
        _ => None,
    };
    match folded {
        Some(kind) => Ok(Expr::new(kind, ty, span)),
        None => Err(ExprKind::Unary { op, rhs }),
    }
}

fn fold_binary(
    op: BinOp,
    lhs: Box<Expr>,
    rhs: Box<Expr>,
    ty: Type,
    span: Span,
) -> Result<Expr, ExprKind> {
    if let Some(kind) = const_binary(op, &lhs.kind, &rhs.kind) {
        return Ok(Expr::new(kind, ty, span));
    }

    // Identities. Test the operands first, so they can then be moved.
    let l_zero = is_zero(&lhs.kind);
    let r_zero = is_zero(&rhs.kind);
    let l_one = is_one(&lhs.kind);
    let r_one = is_one(&rhs.kind);
    let l_bool = as_bool(&lhs.kind);

    match op {
        BinOp::Add => {
            if r_zero {
                return Ok(*lhs);
            }
            if l_zero {
                return Ok(*rhs);
            }
        }
        BinOp::Sub => {
            if r_zero {
                return Ok(*lhs);
            }
        }
        BinOp::Mul => {
            if r_one {
                return Ok(*lhs);
            }
            if l_one {
                return Ok(*rhs);
            }
            // `x * 0` → `0` drops `x`, so only if it is pure.
            if r_zero && is_pure(&lhs) {
                return Ok(*rhs);
            }
            if l_zero && is_pure(&rhs) {
                return Ok(*lhs);
            }
        }
        BinOp::And => match l_bool {
            Some(true) => return Ok(*rhs),
            Some(false) => return Ok(*lhs),
            None => {}
        },
        BinOp::Or => match l_bool {
            Some(true) => return Ok(*lhs),
            Some(false) => return Ok(*rhs),
            None => {}
        },
        _ => {}
    }
    Err(ExprKind::Binary { op, lhs, rhs })
}

/// Collapses an `if` whose condition is a literal.
fn fold_if(
    cond: Expr,
    then_branch: Block,
    else_branch: Option<Box<Expr>>,
    ty: Type,
    span: Span,
) -> Result<Expr, ExprKind> {
    let cond_span = cond.span;
    match cond.kind {
        ExprKind::Bool(true) => Ok(Expr::new(ExprKind::Block(then_branch), ty, span)),
        ExprKind::Bool(false) => match else_branch {
            Some(else_expr) => Ok(*else_expr),
            // `if false { ... }` without `else` is `unit`.
            None => Ok(unit_expr(span)),
        },
        // Not a literal: rebuild the `if` unchanged.
        other => {
            let cond = Box::new(Expr::new(other, Type::Bool, cond_span));
            Err(ExprKind::If {
                cond,
                then_branch,
                else_branch,
            })
        }
    }
}

/// `lhs op rhs` for two literals, if it can be computed at compile time.
fn const_binary(op: BinOp, lhs: &ExprKind, rhs: &ExprKind) -> Option<ExprKind> {
    use ExprKind::{Bool, Float, Int, Str};
    match (lhs, rhs) {
        (Int(a), Int(b)) => op
            .fold_int(*a, *b)
            .map(Int)
            .or_else(|| op.compare(a, b).map(Bool)),
        (Float(a), Float(b)) => op
            .fold_float(*a, *b)
            .map(Float)
            .or_else(|| op.compare(a, b).map(Bool)),
        (Bool(a), Bool(b)) => match op {
            BinOp::Eq => Some(Bool(a == b)),
            BinOp::Ne => Some(Bool(a != b)),
            BinOp::And => Some(Bool(*a && *b)),
            BinOp::Or => Some(Bool(*a || *b)),
            _ => None,
        },
        (Str(a), Str(b)) => match op {
            BinOp::Eq => Some(Bool(a == b)),
            BinOp::Ne => Some(Bool(a != b)),
            _ => None,
        },
        _ => None,
    }
}

// ---- literal predicates ----

fn is_zero(kind: &ExprKind) -> bool {
    matches!(kind, ExprKind::Int(0)) || matches!(kind, ExprKind::Float(f) if *f == 0.0)
}

fn is_one(kind: &ExprKind) -> bool {
    matches!(kind, ExprKind::Int(1)) || matches!(kind, ExprKind::Float(f) if *f == 1.0)
}

fn as_bool(kind: &ExprKind) -> Option<bool> {
    match kind {
        ExprKind::Bool(b) => Some(*b),
        _ => None,
    }
}
