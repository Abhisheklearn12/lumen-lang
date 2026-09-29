//! The HIR printer behind `lumenc dump hir`: one node per line, indented by
//! depth, with each expression's type after a `:`. Locals print as their
//! [`LocalId`] slot (`_3`), so slot allocation is visible.

use crate::hir::*;
use crate::sema::types::Builtin;

/// Renders a whole program.
pub fn print_hir(hir: &Hir) -> String {
    let mut p = Printer {
        out: String::new(),
        depth: 0,
    };
    for (idx, func) in hir.functions.iter().enumerate() {
        p.function(FnId(idx as u32), func, hir.main);
    }
    p.out
}

struct Printer {
    out: String,
    depth: usize,
}

impl Printer {
    fn line(&mut self, text: &str) {
        for _ in 0..self.depth {
            self.out.push_str("  ");
        }
        self.out.push_str(text);
        self.out.push('\n');
    }

    fn indented(&mut self, f: impl FnOnce(&mut Self)) {
        self.depth += 1;
        f(self);
        self.depth -= 1;
    }

    fn function(&mut self, id: FnId, func: &Function, main: FnId) {
        let params = func
            .params()
            .iter()
            .enumerate()
            .map(|(i, p)| format!("{}:{} {}", i, p.name, p.ty))
            .collect::<Vec<_>>()
            .join(", ");
        let entry = if id == main { " [entry]" } else { "" };
        self.line(&format!(
            "fn {}({}) -> {}{}",
            func.name, params, func.ret, entry
        ));
        self.indented(|p| {
            // Parameters are in the header; list the other slots.
            for (i, local) in func.locals.iter().enumerate().skip(func.param_count) {
                p.line(&format!("local {}: {} {}", i, local.name, local.ty));
            }
            p.block(&func.body);
        });
    }

    fn block(&mut self, block: &Block) {
        self.line(&format!("block: {}", block.ty));
        self.indented(|p| {
            for stmt in &block.stmts {
                p.stmt(stmt);
            }
            if let Some(tail) = &block.tail {
                p.line("tail");
                p.indented(|p| p.expr(tail));
            }
        });
    }

    fn stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Let { local, value } => {
                self.line(&format!("let _{}", local.0));
                self.indented(|p| p.expr(value));
            }
            Stmt::Expr(e) => {
                self.line("expr-stmt");
                self.indented(|p| p.expr(e));
            }
            Stmt::Return(e) => {
                self.line("return");
                if let Some(e) = e {
                    self.indented(|p| p.expr(e));
                }
            }
            Stmt::While { cond, body } => {
                self.line("while");
                self.indented(|p| {
                    p.line("cond");
                    p.indented(|p| p.expr(cond));
                    p.block(body);
                });
            }
            Stmt::For {
                var,
                end_var,
                start,
                end,
                body,
            } => {
                self.line(&format!("for _{} end=_{}", var.0, end_var.0));
                self.indented(|p| {
                    p.line("start");
                    p.indented(|p| p.expr(start));
                    p.line("end");
                    p.indented(|p| p.expr(end));
                    p.block(body);
                });
            }
            Stmt::Break => self.line("break"),
            Stmt::Continue => self.line("continue"),
        }
    }

    fn expr(&mut self, expr: &Expr) {
        let ty = expr.ty;
        match &expr.kind {
            ExprKind::Int(v) => self.line(&format!("int {v}: {ty}")),
            ExprKind::Float(v) => self.line(&format!("float {v}: {ty}")),
            ExprKind::Bool(v) => self.line(&format!("bool {v}: {ty}")),
            ExprKind::Str(s) => self.line(&format!("str {s:?}: {ty}")),
            ExprKind::Local(id) => self.line(&format!("local _{}: {ty}", id.0)),
            ExprKind::Unary { op, rhs } => {
                self.line(&format!("unary {}: {ty}", op.symbol()));
                self.indented(|p| p.expr(rhs));
            }
            ExprKind::Binary { op, lhs, rhs } => {
                self.line(&format!("binary {}: {ty}", op.symbol()));
                self.indented(|p| {
                    p.expr(lhs);
                    p.expr(rhs);
                });
            }
            ExprKind::Call { callee, args } => {
                self.line(&format!("call {}: {ty}", callee_str(*callee)));
                self.indented(|p| {
                    for arg in args {
                        p.expr(arg);
                    }
                });
            }
            ExprKind::Assign { local, value } => {
                self.line(&format!("assign _{}: {ty}", local.0));
                self.indented(|p| p.expr(value));
            }
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.line(&format!("if: {ty}"));
                self.indented(|p| {
                    p.line("cond");
                    p.indented(|p| p.expr(cond));
                    p.block(then_branch);
                    if let Some(else_branch) = else_branch {
                        p.line("else");
                        p.indented(|p| p.expr(else_branch));
                    }
                });
            }
            ExprKind::ArrayLit(elems) => {
                self.line(&format!("array: {ty}"));
                self.indented(|p| {
                    for e in elems {
                        p.expr(e);
                    }
                });
            }
            ExprKind::Index { base, index } => {
                self.line(&format!("index: {ty}"));
                self.indented(|p| {
                    p.expr(base);
                    p.expr(index);
                });
            }
            ExprKind::SetIndex { base, index, value } => {
                self.line(&format!("set-index: {ty}"));
                self.indented(|p| {
                    p.expr(base);
                    p.expr(index);
                    p.expr(value);
                });
            }
            ExprKind::StructLit(fields) => {
                self.line(&format!("struct: {ty}"));
                self.indented(|p| {
                    for (i, e) in fields.iter().enumerate() {
                        p.line(&format!("field {i}"));
                        p.indented(|p| p.expr(e));
                    }
                });
            }
            ExprKind::GetField { base, idx } => {
                self.line(&format!("get-field {idx}: {ty}"));
                self.indented(|p| p.expr(base));
            }
            ExprKind::SetField { base, idx, value } => {
                self.line(&format!("set-field {idx}: {ty}"));
                self.indented(|p| {
                    p.expr(base);
                    p.expr(value);
                });
            }
            ExprKind::Block(block) => self.block(block),
        }
    }
}

fn callee_str(callee: Callee) -> String {
    match callee {
        Callee::Fn(id) => format!("fn#{}", id.0),
        Callee::Builtin(b) => format!("builtin {}", Builtin::name(b)),
    }
}
