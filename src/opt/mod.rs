//! The HIR optimizer: a fixed sequence of passes, repeated until nothing
//! changes or [`OptOptions::max_iterations`] is reached, so one pass can expose
//! work for another (folding a condition to `true` lets DCE drop a branch).
//!
//! * [`inline`]: inlines functions whose body is one expression.
//! * [`fold`]: constant folding, algebraic identities, and constant `if`s.
//! * [`dce`]: removes code after `return`, `while false`, and unused pure
//!   `let`s.
//!
//! No call, assignment, or store is ever removed; `is_pure` alone decides
//! what may go. Every pass is deterministic.

pub mod dce;
pub mod fold;
pub mod inline;

use crate::hir::{Block, Expr, ExprKind, Hir, Stmt};
use crate::sema::types::Type;
use crate::span::Span;

/// Optimizer settings.
#[derive(Debug, Clone, Copy)]
pub struct OptOptions {
    /// Whether to optimize at all; `-O0` turns this off.
    pub enabled: bool,
    /// The most rounds of passes to run.
    pub max_iterations: usize,
}

impl Default for OptOptions {
    fn default() -> OptOptions {
        OptOptions {
            enabled: true,
            max_iterations: 8,
        }
    }
}

/// What an optimizer run did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct OptStats {
    /// Constant folds and algebraic simplifications.
    pub folded: usize,
    /// Dead statements and branches removed.
    pub eliminated: usize,
    /// Calls replaced by the callee's body.
    pub inlined: usize,
    /// Rounds of passes run.
    pub iterations: usize,
}

impl OptStats {
    /// Total rewrites across all passes.
    pub fn total(&self) -> usize {
        self.folded + self.eliminated + self.inlined
    }
}

/// Optimizes `hir` in place.
#[tracing::instrument(level = "debug", skip_all)]
pub fn optimize(hir: &mut Hir, options: OptOptions) -> OptStats {
    let mut stats = OptStats::default();
    if !options.enabled {
        return stats;
    }
    for iteration in 0..options.max_iterations {
        // Inline first, so folding and DCE see across the old call boundary.
        let inlined = inline::run(hir);
        let folded = fold::run(hir);
        let eliminated = dce::run(hir);
        stats.inlined += inlined;
        stats.folded += folded;
        stats.eliminated += eliminated;
        stats.iterations = iteration + 1;
        tracing::debug!(
            iteration,
            inlined,
            folded,
            eliminated,
            "optimizer iteration"
        );
        if inlined + folded + eliminated == 0 {
            break;
        }
    }
    tracing::debug!(?stats, "optimization complete");
    stats
}

// ---- shared helpers ----

/// A side-effect-free `unit` value, to stand in for removed code.
pub(crate) fn unit_expr(span: Span) -> Expr {
    Expr::new(
        ExprKind::Block(Block {
            stmts: Vec::new(),
            tail: None,
            ty: Type::Unit,
        }),
        Type::Unit,
        span,
    )
}

/// Whether evaluating `expr` has no side effect, so it may be removed or
/// duplicated. Anything that calls, assigns, or stores is impure.
pub(crate) fn is_pure(expr: &Expr) -> bool {
    match &expr.kind {
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::Bool(_)
        | ExprKind::Str(_)
        | ExprKind::Local(_) => true,
        ExprKind::Unary { rhs, .. } => is_pure(rhs),
        ExprKind::Binary { lhs, rhs, .. } => is_pure(lhs) && is_pure(rhs),
        ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            is_pure(cond)
                && block_is_pure(then_branch)
                && else_branch.as_ref().is_none_or(|e| is_pure(e))
        }
        ExprKind::Block(block) => block_is_pure(block),
        // Reads count as pure even though an out-of-bounds index traps, so an
        // unused out-of-range read can be removed instead of trapping.
        ExprKind::ArrayLit(elems) => elems.iter().all(is_pure),
        ExprKind::StructLit(fields) => fields.iter().all(is_pure),
        ExprKind::Index { base, index } => is_pure(base) && is_pure(index),
        ExprKind::GetField { base, .. } => is_pure(base),
        ExprKind::Call { .. }
        | ExprKind::Assign { .. }
        | ExprKind::SetIndex { .. }
        | ExprKind::SetField { .. } => false,
    }
}

/// Whether every statement and the tail of `block` are pure. Any control-flow
/// statement (`return`, a loop, `break`, `continue`) makes it impure.
pub(crate) fn block_is_pure(block: &Block) -> bool {
    block.stmts.iter().all(stmt_is_pure) && block.tail.as_ref().is_none_or(|t| is_pure(t))
}

fn stmt_is_pure(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Let { value, .. } => is_pure(value),
        Stmt::Expr(e) => is_pure(e),
        Stmt::Return(_) | Stmt::While { .. } | Stmt::For { .. } | Stmt::Break | Stmt::Continue => {
            false
        }
    }
}

// ---- traversal ----

/// A HIR walk. Each method defaults to visiting the node's children in
/// evaluation order; an override calls the matching `walk_*` to recurse.
pub(crate) trait Visit {
    fn visit_block(&mut self, block: &Block) {
        walk_block(self, block);
    }

    fn visit_stmt(&mut self, stmt: &Stmt) {
        walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &Expr) {
        walk_expr(self, expr);
    }
}

pub(crate) fn walk_block<V: Visit + ?Sized>(v: &mut V, block: &Block) {
    for stmt in &block.stmts {
        v.visit_stmt(stmt);
    }
    if let Some(tail) = &block.tail {
        v.visit_expr(tail);
    }
}

pub(crate) fn walk_stmt<V: Visit + ?Sized>(v: &mut V, stmt: &Stmt) {
    match stmt {
        Stmt::Let { value: e, .. } | Stmt::Expr(e) | Stmt::Return(Some(e)) => v.visit_expr(e),
        Stmt::While { cond, body } => {
            v.visit_expr(cond);
            v.visit_block(body);
        }
        Stmt::For {
            start, end, body, ..
        } => {
            v.visit_expr(start);
            v.visit_expr(end);
            v.visit_block(body);
        }
        Stmt::Return(None) | Stmt::Break | Stmt::Continue => {}
    }
}

pub(crate) fn walk_expr<V: Visit + ?Sized>(v: &mut V, expr: &Expr) {
    match &expr.kind {
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::Bool(_)
        | ExprKind::Str(_)
        | ExprKind::Local(_) => {}
        ExprKind::Unary { rhs: e, .. }
        | ExprKind::Assign { value: e, .. }
        | ExprKind::GetField { base: e, .. } => v.visit_expr(e),
        ExprKind::Binary { lhs: a, rhs: b, .. }
        | ExprKind::Index { base: a, index: b }
        | ExprKind::SetField {
            base: a, value: b, ..
        } => {
            v.visit_expr(a);
            v.visit_expr(b);
        }
        ExprKind::SetIndex { base, index, value } => {
            v.visit_expr(base);
            v.visit_expr(index);
            v.visit_expr(value);
        }
        ExprKind::Call { args: es, .. } | ExprKind::ArrayLit(es) | ExprKind::StructLit(es) => {
            for e in es {
                v.visit_expr(e);
            }
        }
        ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            v.visit_expr(cond);
            v.visit_block(then_branch);
            if let Some(e) = else_branch {
                v.visit_expr(e);
            }
        }
        ExprKind::Block(block) => v.visit_block(block),
    }
}

/// [`Visit`] over mutable HIR.
pub(crate) trait VisitMut {
    fn visit_block(&mut self, block: &mut Block) {
        walk_block_mut(self, block);
    }

    fn visit_stmt(&mut self, stmt: &mut Stmt) {
        walk_stmt_mut(self, stmt);
    }

    fn visit_expr(&mut self, expr: &mut Expr) {
        walk_expr_mut(self, expr);
    }
}

pub(crate) fn walk_block_mut<V: VisitMut + ?Sized>(v: &mut V, block: &mut Block) {
    for stmt in &mut block.stmts {
        v.visit_stmt(stmt);
    }
    if let Some(tail) = &mut block.tail {
        v.visit_expr(tail);
    }
}

pub(crate) fn walk_stmt_mut<V: VisitMut + ?Sized>(v: &mut V, stmt: &mut Stmt) {
    match stmt {
        Stmt::Let { value: e, .. } | Stmt::Expr(e) | Stmt::Return(Some(e)) => v.visit_expr(e),
        Stmt::While { cond, body } => {
            v.visit_expr(cond);
            v.visit_block(body);
        }
        Stmt::For {
            start, end, body, ..
        } => {
            v.visit_expr(start);
            v.visit_expr(end);
            v.visit_block(body);
        }
        Stmt::Return(None) | Stmt::Break | Stmt::Continue => {}
    }
}

pub(crate) fn walk_expr_mut<V: VisitMut + ?Sized>(v: &mut V, expr: &mut Expr) {
    match &mut expr.kind {
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::Bool(_)
        | ExprKind::Str(_)
        | ExprKind::Local(_) => {}
        ExprKind::Unary { rhs: e, .. }
        | ExprKind::Assign { value: e, .. }
        | ExprKind::GetField { base: e, .. } => v.visit_expr(e),
        ExprKind::Binary { lhs: a, rhs: b, .. }
        | ExprKind::Index { base: a, index: b }
        | ExprKind::SetField {
            base: a, value: b, ..
        } => {
            v.visit_expr(a);
            v.visit_expr(b);
        }
        ExprKind::SetIndex { base, index, value } => {
            v.visit_expr(base);
            v.visit_expr(index);
            v.visit_expr(value);
        }
        ExprKind::Call { args: es, .. } | ExprKind::ArrayLit(es) | ExprKind::StructLit(es) => {
            for e in es {
                v.visit_expr(e);
            }
        }
        ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            v.visit_expr(cond);
            v.visit_block(then_branch);
            if let Some(e) = else_branch {
                v.visit_expr(e);
            }
        }
        ExprKind::Block(block) => v.visit_block(block),
    }
}

#[cfg(test)]
mod tests;
