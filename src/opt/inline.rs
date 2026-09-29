//! Function inlining: a call to a function whose body is a single expression
//! becomes a copy of that body, so folding and DCE can see through it.
//!
//! A candidate is any function but `main` whose body reads only its parameters,
//! has no statements, assignments, or stores, and does not call itself; it may
//! call other functions. `f(a, b)` becomes `{ let p0 = a; let p1 = b; <body> }`
//! with the parameters remapped to fresh caller slots, so each argument is
//! still evaluated once, in order.

use std::collections::HashMap;

use crate::hir::{Block, Callee, Expr, ExprKind, FnId, Hir, LocalDecl, LocalId, Stmt};
use crate::opt::{VisitMut, walk_expr_mut};
use crate::sema::types::Type;

/// An inlinable function: its parameter count and body.
struct Candidate {
    param_count: usize,
    body: Expr,
}

/// Inlines eligible calls throughout the program; returns how many.
pub fn run(hir: &mut Hir) -> usize {
    let candidates = collect_candidates(hir);
    if candidates.is_empty() {
        return 0;
    }
    let mut count = 0;
    for (i, func) in hir.functions.iter_mut().enumerate() {
        let mut inliner = Inliner {
            candidates: &candidates,
            self_id: FnId(i as u32),
            locals: &mut func.locals,
            count: 0,
        };
        inliner.visit_block(&mut func.body);
        count += inliner.count;
    }
    count
}

/// The candidates for inlining, by id.
fn collect_candidates(hir: &Hir) -> HashMap<FnId, Candidate> {
    let mut out = HashMap::new();
    for (i, func) in hir.functions.iter().enumerate() {
        let id = FnId(i as u32);
        if id == hir.main {
            continue;
        }
        if !func.body.stmts.is_empty() {
            continue;
        }
        let Some(tail) = &func.body.tail else {
            continue;
        };
        if is_simple(tail, func.param_count) && !calls(tail, id) {
            out.insert(
                id,
                Candidate {
                    param_count: func.param_count,
                    body: (**tail).clone(),
                },
            );
        }
    }
    out
}

/// Whether `expr` reads only the first `params` slots and contains no
/// statements, assignments, or stores.
fn is_simple(expr: &Expr, params: usize) -> bool {
    match &expr.kind {
        ExprKind::Int(_) | ExprKind::Float(_) | ExprKind::Bool(_) | ExprKind::Str(_) => true,
        ExprKind::Local(id) => (id.0 as usize) < params,
        ExprKind::Unary { rhs, .. } => is_simple(rhs, params),
        ExprKind::Binary { lhs, rhs, .. } => is_simple(lhs, params) && is_simple(rhs, params),
        ExprKind::Call { args, .. } => args.iter().all(|a| is_simple(a, params)),
        ExprKind::ArrayLit(es) | ExprKind::StructLit(es) => es.iter().all(|e| is_simple(e, params)),
        ExprKind::Index { base, index } => is_simple(base, params) && is_simple(index, params),
        ExprKind::GetField { base, .. } => is_simple(base, params),
        ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            is_simple(cond, params)
                && simple_block(then_branch, params)
                && else_branch.as_ref().is_none_or(|e| is_simple(e, params))
        }
        ExprKind::Block(b) => simple_block(b, params),
        ExprKind::Assign { .. } | ExprKind::SetIndex { .. } | ExprKind::SetField { .. } => false,
    }
}

/// Whether `block` has no statements and a simple tail.
fn simple_block(block: &Block, params: usize) -> bool {
    block.stmts.is_empty() && block.tail.as_deref().is_none_or(|t| is_simple(t, params))
}

/// Whether `expr`, a simple expression, calls function `id`.
fn calls(expr: &Expr, id: FnId) -> bool {
    match &expr.kind {
        ExprKind::Call { callee, args } => {
            matches!(callee, Callee::Fn(f) if *f == id) || args.iter().any(|a| calls(a, id))
        }
        ExprKind::Unary { rhs, .. } => calls(rhs, id),
        ExprKind::Binary { lhs, rhs, .. } => calls(lhs, id) || calls(rhs, id),
        ExprKind::ArrayLit(es) | ExprKind::StructLit(es) => es.iter().any(|e| calls(e, id)),
        ExprKind::Index { base, index } => calls(base, id) || calls(index, id),
        ExprKind::GetField { base, .. } => calls(base, id),
        ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            calls(cond, id)
                || then_branch.tail.as_deref().is_some_and(|t| calls(t, id))
                || else_branch.as_deref().is_some_and(|e| calls(e, id))
        }
        ExprKind::Block(b) => b.tail.as_deref().is_some_and(|t| calls(t, id)),
        _ => false,
    }
}

struct Inliner<'a> {
    candidates: &'a HashMap<FnId, Candidate>,
    /// The function being rewritten, which never inlines into itself.
    self_id: FnId,
    locals: &'a mut Vec<LocalDecl>,
    count: usize,
}

impl VisitMut for Inliner<'_> {
    fn visit_expr(&mut self, expr: &mut Expr) {
        // Children first, so inlined arguments are already rewritten.
        walk_expr_mut(self, expr);
        let candidates = self.candidates;
        let cand = match &expr.kind {
            ExprKind::Call {
                callee: Callee::Fn(id),
                ..
            } if *id != self.self_id => candidates.get(id),
            _ => None,
        };
        if let Some(cand) = cand {
            let ExprKind::Call { args, .. } = std::mem::replace(&mut expr.kind, ExprKind::Int(0))
            else {
                unreachable!()
            };
            expr.kind = build_inlined(cand, args, expr.ty, self.locals);
            self.count += 1;
        }
    }
}

/// The block that replaces a call to `cand`.
fn build_inlined(
    cand: &Candidate,
    args: Vec<Expr>,
    result_ty: Type,
    locals: &mut Vec<LocalDecl>,
) -> ExprKind {
    // Bind each argument to a fresh caller slot.
    let mut params = Vec::with_capacity(cand.param_count);
    let mut stmts = Vec::with_capacity(cand.param_count);
    for arg in args.into_iter().take(cand.param_count) {
        let new_local = LocalId(locals.len() as u32);
        locals.push(LocalDecl {
            name: format!("<inl{}>", new_local.0),
            ty: arg.ty,
        });
        params.push(new_local);
        stmts.push(Stmt::Let {
            local: new_local,
            value: arg,
        });
    }
    let mut body = cand.body.clone();
    RemapParams(&params).visit_expr(&mut body);
    ExprKind::Block(Block {
        stmts,
        tail: Some(Box::new(body)),
        ty: result_ty,
    })
}

/// Rewrites reads of parameter `i` to read slot `self.0[i]`.
struct RemapParams<'a>(&'a [LocalId]);

impl VisitMut for RemapParams<'_> {
    fn visit_expr(&mut self, expr: &mut Expr) {
        if let ExprKind::Local(id) = &mut expr.kind
            && let Some(&new) = self.0.get(id.0 as usize)
        {
            *id = new;
        }
        walk_expr_mut(self, expr);
    }
}
