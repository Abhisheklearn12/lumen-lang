//! The formatter behind `lumenc fmt`: prints an [`Ast`] as canonical source.
//!
//! Unlike the [AST printer](crate::parser::print), it emits valid Lumen:
//! four-space indents, one statement per line, spaces around binary operators,
//! and only the parentheses precedence requires. Comments are not kept, since
//! the AST has none. Formatting is idempotent; tests check it.

use std::fmt::Write as _;

use crate::parser::ast::*;

/// Formats a whole program.
pub fn format_source(ast: &Ast) -> String {
    let mut f = Formatter {
        out: String::new(),
        depth: 0,
    };
    for (i, item) in ast.items.iter().enumerate() {
        if i > 0 {
            f.out.push('\n');
        }
        f.item(item);
    }
    f.out
}

struct Formatter {
    out: String,
    depth: usize,
}

/// The binding strength of prefix `-` and `!`, above every binary operator.
const PREFIX_PREC: u8 = 7;
/// The binding strength of calls, indexing, and `.`, the tightest.
const POSTFIX_PREC: u8 = 8;

impl Formatter {
    /// The indentation for the current depth.
    fn pad(&self) -> String {
        "    ".repeat(self.depth)
    }

    fn indent(&mut self) {
        let pad = self.pad();
        self.out.push_str(&pad);
    }

    fn line(&mut self, text: &str) {
        self.indent();
        self.out.push_str(text);
        self.out.push('\n');
    }

    // ---- items ----

    fn item(&mut self, item: &Item) {
        match &item.kind {
            ItemKind::Fn(decl) => self.function(decl),
            ItemKind::Const(decl) => {
                let value = self.expr_to_string(&decl.value, 0);
                self.line(&format!(
                    "const {}: {} = {value};",
                    decl.name.name,
                    type_str(&decl.ty)
                ));
            }
            ItemKind::Struct(decl) => {
                self.line(&format!("struct {} {{", decl.name.name));
                self.depth += 1;
                for field in &decl.fields {
                    self.line(&format!("{}: {},", field.name.name, type_str(&field.ty)));
                }
                self.depth -= 1;
                self.line("}");
            }
        }
    }

    fn function(&mut self, decl: &FnDecl) {
        let params = decl
            .params
            .iter()
            .map(|p| format!("{}: {}", p.name.name, type_str(&p.ty)))
            .collect::<Vec<_>>()
            .join(", ");
        let ret = match &decl.ret {
            Some(ty) => format!(" -> {}", type_str(ty)),
            None => String::new(),
        };
        self.indent();
        let _ = write!(self.out, "fn {}({params}){ret} ", decl.name.name);
        self.block(&decl.body);
        self.out.push('\n');
    }

    // ---- blocks & statements ----

    /// Writes a block from the current position; a non-empty block's `}` goes
    /// on its own line.
    fn block(&mut self, block: &Block) {
        if block.stmts.is_empty() && block.tail.is_none() {
            self.out.push_str("{}");
            return;
        }
        self.out.push_str("{\n");
        self.depth += 1;
        for stmt in &block.stmts {
            self.stmt(stmt);
        }
        if let Some(tail) = &block.tail {
            let text = self.expr_to_string(tail, 0);
            self.line(&text);
        }
        self.depth -= 1;
        self.indent();
        self.out.push('}');
    }

    fn stmt(&mut self, stmt: &Stmt) {
        match &stmt.kind {
            StmtKind::Let(l) => {
                let kw = if l.mutable { "let mut" } else { "let" };
                let ty =
                    l.ty.as_ref()
                        .map(|t| format!(": {}", type_str(t)))
                        .unwrap_or_default();
                let value = self.expr_to_string(&l.init, 0);
                self.line(&format!("{kw} {}{ty} = {value};", l.name.name));
            }
            StmtKind::Expr(e) => {
                let text = self.expr_to_string(e, 0);
                self.line(&format!("{text};"));
            }
            StmtKind::Return(Some(e)) => {
                let text = self.expr_to_string(e, 0);
                self.line(&format!("return {text};"));
            }
            StmtKind::Return(None) => self.line("return;"),
            StmtKind::While(w) => {
                let cond = self.expr_to_string(&w.cond, 0);
                self.indent();
                let _ = write!(self.out, "while {cond} ");
                self.block(&w.body);
                self.out.push('\n');
            }
            StmtKind::For(fr) => {
                let start = self.expr_to_string(&fr.start, 0);
                let end = self.expr_to_string(&fr.end, 0);
                self.indent();
                let _ = write!(self.out, "for {} in {start}..{end} ", fr.var.name);
                self.block(&fr.body);
                self.out.push('\n');
            }
            StmtKind::ForEach(fe) => {
                let iter = self.expr_to_string(&fe.iterable, 0);
                self.indent();
                let _ = write!(self.out, "for {} in {iter} ", fe.var.name);
                self.block(&fe.body);
                self.out.push('\n');
            }
            StmtKind::Break => self.line("break;"),
            StmtKind::Continue => self.line("continue;"),
        }
    }

    // ---- expressions ----

    /// A block as text, indented for the current depth.
    fn block_to_string(&mut self, block: &Block) -> String {
        let saved = std::mem::take(&mut self.out);
        self.block(block);
        std::mem::replace(&mut self.out, saved)
    }

    /// `expr` as text, parenthesised if it binds looser than `parent_prec`,
    /// the precedence its context requires (0 for none).
    fn expr_to_string(&mut self, expr: &Expr, parent_prec: u8) -> String {
        match &expr.kind {
            ExprKind::Int(v) => v.to_string(),
            ExprKind::Float(v) => format_float(*v),
            ExprKind::Bool(v) => v.to_string(),
            ExprKind::Str(s) => format!("{s:?}"),
            ExprKind::Name(n) => n.clone(),
            ExprKind::Unary { op, rhs } => {
                format!("{}{}", op.symbol(), self.expr_to_string(rhs, PREFIX_PREC))
            }
            ExprKind::Binary { op, lhs, rhs } => {
                let prec = op.precedence();
                let l = self.expr_to_string(lhs, prec);
                let r = self.expr_to_string(rhs, prec + 1);
                let text = format!("{l} {} {r}", op.symbol());
                if prec < parent_prec {
                    format!("({text})")
                } else {
                    text
                }
            }
            ExprKind::Call { callee, args } => {
                let callee = self.expr_to_string(callee, POSTFIX_PREC);
                let args = args
                    .iter()
                    .map(|a| self.expr_to_string(a, 0))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{callee}({args})")
            }
            ExprKind::Index { base, index } => {
                let base = self.expr_to_string(base, POSTFIX_PREC);
                let index = self.expr_to_string(index, 0);
                format!("{base}[{index}]")
            }
            ExprKind::ArrayLit(elems) => {
                let items = elems
                    .iter()
                    .map(|e| self.expr_to_string(e, 0))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("[{items}]")
            }
            ExprKind::Assign { target, value } => {
                format!(
                    "{} = {}",
                    self.expr_to_string(target, 0),
                    self.expr_to_string(value, 0)
                )
            }
            ExprKind::AssignOp { target, op, value } => {
                format!(
                    "{} {}= {}",
                    self.expr_to_string(target, 0),
                    op.symbol(),
                    self.expr_to_string(value, 0)
                )
            }
            ExprKind::StructLit { name, fields } => {
                let parts = fields
                    .iter()
                    .map(|f| format!("{}: {}", f.name.name, self.expr_to_string(&f.value, 0)))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{} {{ {parts} }}", name.name)
            }
            ExprKind::Field { base, field } => {
                format!("{}.{}", self.expr_to_string(base, POSTFIX_PREC), field.name)
            }
            ExprKind::TupleLit(elems) => {
                let parts = elems
                    .iter()
                    .map(|e| self.expr_to_string(e, 0))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("({parts})")
            }
            ExprKind::TupleIndex { base, index, .. } => {
                format!("{}.{index}", self.expr_to_string(base, POSTFIX_PREC))
            }
            ExprKind::If(if_expr) => self.if_to_string(if_expr),
            ExprKind::Match(m) => self.match_to_string(m),
            ExprKind::Block(block) => self.block_to_string(block),
        }
    }

    fn match_to_string(&mut self, m: &MatchExpr) -> String {
        let scrut = self.expr_to_string(&m.scrutinee, 0);
        let mut text = format!("match {scrut} {{\n");
        self.depth += 1;
        for arm in &m.arms {
            let body = self.expr_to_string(&arm.body, 0);
            let _ = writeln!(text, "{}{} => {body},", self.pad(), arm.pattern);
        }
        self.depth -= 1;
        text.push_str(&self.pad());
        text.push('}');
        text
    }

    fn if_to_string(&mut self, if_expr: &IfExpr) -> String {
        let cond = self.expr_to_string(&if_expr.cond, 0);
        let then = self.block_to_string(&if_expr.then_branch);
        let mut text = format!("if {cond} {then}");
        if let Some(else_branch) = &if_expr.else_branch {
            let else_text = self.expr_to_string(else_branch, 0);
            let _ = write!(text, " else {else_text}");
        }
        text
    }
}

/// A type as written; `?` for a malformed one.
fn type_str(ty: &TypeExpr) -> String {
    match &ty.kind {
        TypeExprKind::Named(name) => name.clone(),
        TypeExprKind::Array(inner) => format!("[{}]", type_str(inner)),
        TypeExprKind::Tuple(elems) => {
            format!(
                "({})",
                elems.iter().map(type_str).collect::<Vec<_>>().join(", ")
            )
        }
        TypeExprKind::Error => "?".to_string(),
    }
}

/// A float literal that reads back as the same value: whole numbers get `.0`,
/// which keeps them from lexing as integers.
fn format_float(v: f64) -> String {
    let s = v.to_string();
    if s.contains('.') || s.contains('e') || s.contains("inf") || s.contains("NaN") {
        s
    } else {
        format!("{s}.0")
    }
}

#[cfg(test)]
mod tests;
