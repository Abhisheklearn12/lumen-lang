//! The AST printer behind `lumenc dump ast`: one node per line, indented by
//! depth. Unlike `Debug`, it omits spans and ids, so output is stable for tests.

use super::ast::*;

/// Renders a whole program.
pub fn print_ast(ast: &Ast) -> String {
    let mut p = Printer {
        out: String::new(),
        depth: 0,
    };
    for item in &ast.items {
        p.item(item);
    }
    p.out
}

struct Printer {
    out: String,
    depth: usize,
}

impl Printer {
    /// Writes `text` as one line at the current depth.
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

    fn item(&mut self, item: &Item) {
        match &item.kind {
            ItemKind::Fn(decl) => {
                let params = decl
                    .params
                    .iter()
                    .map(|p| format!("{}: {}", p.name.name, type_str(&p.ty)))
                    .collect::<Vec<_>>()
                    .join(", ");
                let ret = decl
                    .ret
                    .as_ref()
                    .map(type_str)
                    .unwrap_or_else(|| "unit".to_string());
                self.line(&format!("fn {}({}) -> {}", decl.name.name, params, ret));
                self.indented(|p| p.block(&decl.body));
            }
            ItemKind::Const(decl) => {
                self.line(&format!("const {}: {}", decl.name.name, type_str(&decl.ty)));
                self.indented(|p| p.expr(&decl.value));
            }
            ItemKind::Struct(decl) => {
                self.line(&format!("struct {}", decl.name.name));
                self.indented(|p| {
                    for field in &decl.fields {
                        p.line(&format!(
                            "field {}: {}",
                            field.name.name,
                            type_str(&field.ty)
                        ));
                    }
                });
            }
        }
    }

    fn block(&mut self, block: &Block) {
        self.line("block");
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
        match &stmt.kind {
            StmtKind::Let(l) => {
                let kw = if l.mutable { "let mut" } else { "let" };
                let ty =
                    l.ty.as_ref()
                        .map(|t| format!(": {}", type_str(t)))
                        .unwrap_or_default();
                self.line(&format!("{} {}{}", kw, l.name.name, ty));
                self.indented(|p| p.expr(&l.init));
            }
            StmtKind::Expr(e) => {
                self.line("expr-stmt");
                self.indented(|p| p.expr(e));
            }
            StmtKind::Return(e) => {
                self.line("return");
                if let Some(e) = e {
                    self.indented(|p| p.expr(e));
                }
            }
            StmtKind::While(w) => {
                self.line("while");
                self.indented(|p| {
                    p.line("cond");
                    p.indented(|p| p.expr(&w.cond));
                    p.block(&w.body);
                });
            }
            StmtKind::For(f) => {
                self.line(&format!("for {}", f.var.name));
                self.indented(|p| {
                    p.line("start");
                    p.indented(|p| p.expr(&f.start));
                    p.line("end");
                    p.indented(|p| p.expr(&f.end));
                    p.block(&f.body);
                });
            }
            StmtKind::ForEach(f) => {
                self.line(&format!("foreach {}", f.var.name));
                self.indented(|p| {
                    p.line("in");
                    p.indented(|p| p.expr(&f.iterable));
                    p.block(&f.body);
                });
            }
            StmtKind::Break => self.line("break"),
            StmtKind::Continue => self.line("continue"),
        }
    }

    fn expr(&mut self, expr: &Expr) {
        match &expr.kind {
            ExprKind::Int(v) => self.line(&format!("int {v}")),
            ExprKind::Float(v) => self.line(&format!("float {v}")),
            ExprKind::Bool(v) => self.line(&format!("bool {v}")),
            ExprKind::Str(s) => self.line(&format!("str {s:?}")),
            ExprKind::Name(n) => self.line(&format!("name {n}")),
            ExprKind::Unary { op, rhs } => {
                self.line(&format!("unary {}", op.symbol()));
                self.indented(|p| p.expr(rhs));
            }
            ExprKind::Binary { op, lhs, rhs } => {
                self.line(&format!("binary {}", op.symbol()));
                self.indented(|p| {
                    p.expr(lhs);
                    p.expr(rhs);
                });
            }
            ExprKind::Call { callee, args } => {
                self.line("call");
                self.indented(|p| {
                    p.line("callee");
                    p.indented(|p| p.expr(callee));
                    for arg in args {
                        p.line("arg");
                        p.indented(|p| p.expr(arg));
                    }
                });
            }
            ExprKind::Assign { target, value } => {
                self.line("assign");
                self.indented(|p| {
                    p.expr(target);
                    p.expr(value);
                });
            }
            ExprKind::AssignOp { target, op, value } => {
                self.line(&format!("assign-op {}", op.symbol()));
                self.indented(|p| {
                    p.expr(target);
                    p.expr(value);
                });
            }
            ExprKind::If(if_expr) => {
                self.line("if");
                self.indented(|p| {
                    p.line("cond");
                    p.indented(|p| p.expr(&if_expr.cond));
                    p.block(&if_expr.then_branch);
                    if let Some(else_branch) = &if_expr.else_branch {
                        p.line("else");
                        p.indented(|p| p.expr(else_branch));
                    }
                });
            }
            ExprKind::ArrayLit(elems) => {
                self.line("array");
                self.indented(|p| {
                    for e in elems {
                        p.expr(e);
                    }
                });
            }
            ExprKind::Index { base, index } => {
                self.line("index");
                self.indented(|p| {
                    p.expr(base);
                    p.expr(index);
                });
            }
            ExprKind::StructLit { name, fields } => {
                self.line(&format!("struct-lit {}", name.name));
                self.indented(|p| {
                    for field in fields {
                        p.line(&format!("field {}", field.name.name));
                        p.indented(|p| p.expr(&field.value));
                    }
                });
            }
            ExprKind::Field { base, field } => {
                self.line(&format!("field-access .{}", field.name));
                self.indented(|p| p.expr(base));
            }
            ExprKind::TupleLit(elems) => {
                self.line("tuple");
                self.indented(|p| {
                    for e in elems {
                        p.expr(e);
                    }
                });
            }
            ExprKind::TupleIndex { base, index, .. } => {
                self.line(&format!("tuple-index .{index}"));
                self.indented(|p| p.expr(base));
            }
            ExprKind::Match(m) => {
                self.line("match");
                self.indented(|p| {
                    p.line("scrutinee");
                    p.indented(|p| p.expr(&m.scrutinee));
                    for arm in &m.arms {
                        p.line(&format!("arm {}", arm.pattern));
                        p.indented(|p| p.expr(&arm.body));
                    }
                });
            }
            ExprKind::Block(block) => self.block(block),
        }
    }
}

/// Renders a type as written, with `<error>` for a malformed one.
fn type_str(ty: &TypeExpr) -> String {
    match &ty.kind {
        TypeExprKind::Named(name) => name.clone(),
        TypeExprKind::Array(inner) => format!("[{}]", type_str(inner)),
        TypeExprKind::Tuple(elems) => {
            let parts = elems.iter().map(type_str).collect::<Vec<_>>().join(", ");
            format!("({parts})")
        }
        TypeExprKind::Error => "<error>".to_string(),
    }
}
