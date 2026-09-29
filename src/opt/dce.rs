//! Dead-code elimination. In each block, after its nested blocks:
//!
//! * statements and the tail after a `return` are removed;
//! * `while false { ... }` is removed;
//! * a `let` whose slot is never read anywhere in the function, and whose
//!   value is pure, is removed.

use std::collections::HashSet;

use crate::hir::{Block, Expr, ExprKind, Function, Hir, Stmt};
use crate::opt::{Visit, VisitMut, is_pure, walk_block_mut, walk_expr, walk_stmt};

/// Runs DCE over the whole program; returns the number of removals.
pub fn run(hir: &mut Hir) -> usize {
    let mut count = 0;
    for func in &mut hir.functions {
        let reads = collect_reads(func);
        let mut dce = Dce {
            reads: &reads,
            count: 0,
        };
        dce.visit_block(&mut func.body);
        count += dce.count;
    }
    count
}

struct Dce<'a> {
    /// Slots read anywhere in the function.
    reads: &'a HashSet<u32>,
    count: usize,
}

impl VisitMut for Dce<'_> {
    fn visit_block(&mut self, block: &mut Block) {
        walk_block_mut(self, block);

        if let Some(pos) = block
            .stmts
            .iter()
            .position(|s| matches!(s, Stmt::Return(_)))
        {
            let removed = block.stmts.len() - (pos + 1) + usize::from(block.tail.is_some());
            if removed > 0 {
                block.stmts.truncate(pos + 1);
                block.tail = None;
                self.count += removed;
            }
        }

        let before = block.stmts.len();
        block.stmts.retain(|stmt| keep_stmt(stmt, self.reads));
        self.count += before - block.stmts.len();
    }
}

/// Whether `stmt` survives: `false` for `while false` and unused pure `let`s.
fn keep_stmt(stmt: &Stmt, reads: &HashSet<u32>) -> bool {
    match stmt {
        Stmt::While { cond, .. } => !matches!(cond.kind, ExprKind::Bool(false)),
        Stmt::Let { local, value } => reads.contains(&local.0) || !is_pure(value),
        _ => true,
    }
}

/// The slots read anywhere in `func`. Assigning to a slot is not a read, so a
/// variable that is only ever written counts as unused.
fn collect_reads(func: &Function) -> HashSet<u32> {
    let mut reads = Reads(HashSet::new());
    reads.visit_block(&func.body);
    reads.0
}

struct Reads(HashSet<u32>);

impl Visit for Reads {
    fn visit_stmt(&mut self, stmt: &Stmt) {
        // A `for` loop reads its counter and bound on every iteration.
        if let Stmt::For { var, end_var, .. } = stmt {
            self.0.insert(var.0);
            self.0.insert(end_var.0);
        }
        walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &Expr) {
        if let ExprKind::Local(id) = expr.kind {
            self.0.insert(id.0);
        }
        walk_expr(self, expr);
    }
}
