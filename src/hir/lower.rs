//! Lowering: AST, [`Resolution`], and [`Typeck`] to [`Hir`].
//!
//! It runs only on programs that type-checked, so it reports nothing. Where
//! malformed input could still reach a case (a name that is not a local, say),
//! it substitutes a harmless placeholder instead of panicking.
//!
//! Each function's parameters and bindings get dense [`LocalId`]s, parameters
//! first. Names become local reads or inlined constants, calls name their
//! [`Callee`], and every node takes the type the checker gave it.

use std::collections::HashMap;

use crate::hir::*;
use crate::parser::ast;
use crate::parser::ast::NodeId;
use crate::sema::resolve::{Res, Resolution, StructId};
use crate::sema::typeck::{ConstValue, Typeck};
use crate::sema::types::Type;

/// The literal an inlined constant becomes.
fn const_value_kind(value: &ConstValue) -> ExprKind {
    match value {
        ConstValue::Int(v) => ExprKind::Int(*v),
        ConstValue::Float(v) => ExprKind::Float(*v),
        ConstValue::Bool(v) => ExprKind::Bool(*v),
        ConstValue::Str(s) => ExprKind::Str(s.clone()),
    }
}

/// Lowers a type-checked program to [`Hir`].
#[tracing::instrument(level = "debug", skip_all)]
pub fn lower(ast: &ast::Ast, res: &Resolution, tc: &Typeck) -> Hir {
    let mut lowerer = Lowerer {
        res,
        tc,
        locals: Vec::new(),
        local_map: HashMap::new(),
    };
    let mut functions = Vec::with_capacity(res.functions.len());
    for (fn_id, item_idx) in res.functions.iter() {
        let ast::ItemKind::Fn(decl) = &ast.items[item_idx].kind else {
            continue;
        };
        functions.push(lowerer.lower_fn(fn_id, decl));
    }
    // A well-typed program has a `main`.
    let main = tc.main.unwrap_or(FnId(0));
    tracing::debug!(functions = functions.len(), "lowering complete");
    Hir { functions, main }
}

struct Lowerer<'a> {
    res: &'a Resolution,
    tc: &'a Typeck,
    /// The current function's locals.
    locals: Vec<LocalDecl>,
    /// Definition → slot, for the current function.
    local_map: HashMap<NodeId, LocalId>,
}

impl Lowerer<'_> {
    fn lower_fn(&mut self, fn_id: FnId, decl: &ast::FnDecl) -> Function {
        self.locals.clear();
        self.local_map.clear();

        for param in &decl.params {
            self.alloc_local(param.id, &param.name.name, self.tc.local_type(param.id));
        }
        let param_count = self.locals.len();
        let ret = self.tc.signature(fn_id).ret;
        let body = self.lower_block(&decl.body);

        Function {
            name: decl.name.name.clone(),
            locals: std::mem::take(&mut self.locals),
            param_count,
            ret,
            body,
        }
    }

    fn lower_block(&mut self, block: &ast::Block) -> Block {
        let stmts = block.stmts.iter().map(|s| self.lower_stmt(s)).collect();
        let tail = block.tail.as_ref().map(|t| Box::new(self.lower_expr(t)));
        let ty = tail.as_ref().map(|t| t.ty).unwrap_or(Type::Unit);
        Block { stmts, tail, ty }
    }

    fn lower_stmt(&mut self, stmt: &ast::Stmt) -> Stmt {
        match &stmt.kind {
            ast::StmtKind::Let(l) => {
                let value = self.lower_expr(&l.init);
                let local = self.alloc_local(l.id, &l.name.name, self.tc.local_type(l.id));
                Stmt::Let { local, value }
            }
            ast::StmtKind::Expr(e) => Stmt::Expr(self.lower_expr(e)),
            ast::StmtKind::Return(e) => Stmt::Return(e.as_ref().map(|e| self.lower_expr(e))),
            ast::StmtKind::While(w) => Stmt::While {
                cond: self.lower_expr(&w.cond),
                body: self.lower_block(&w.body),
            },
            ast::StmtKind::For(f) => {
                let start = self.lower_expr(&f.start);
                let end = self.lower_expr(&f.end);
                let var = self.alloc_local(f.id, &f.var.name, Type::Int);
                let end_var = self.alloc_synthetic("<for-end>", Type::Int);
                let body = self.lower_block(&f.body);
                Stmt::For {
                    var,
                    end_var,
                    start,
                    end,
                    body,
                }
            }
            ast::StmtKind::ForEach(f) => self.lower_for_each(f, stmt.span),
            ast::StmtKind::Break => Stmt::Break,
            ast::StmtKind::Continue => Stmt::Continue,
        }
    }

    /// Desugars `for v in arr { body }` to an index loop over the array,
    /// evaluated once:
    ///
    /// ```text
    /// {
    ///     let <arr> = arr;
    ///     for <i> in 0..len(<arr>) {
    ///         let v = <arr>[<i>];
    ///         body
    ///     }
    /// }
    /// ```
    fn lower_for_each(&mut self, f: &ast::ForEachStmt, span: Span) -> Stmt {
        let iterable = self.lower_expr(&f.iterable);
        let elem_ty = match iterable.ty {
            Type::Array(elem) => elem.ty(),
            _ => Type::Error,
        };
        // Hidden slots: the array, the index, and the length.
        let arr_local = self.alloc_synthetic("<foreach-arr>", iterable.ty);
        let idx = self.alloc_synthetic("<foreach-idx>", Type::Int);
        let end_var = self.alloc_synthetic("<for-end>", Type::Int);
        // The body refers to `v`, so allocate it first.
        let var = self.alloc_local(f.id, &f.var.name, elem_ty);

        let mut body = self.lower_block(&f.body);
        let read = Stmt::Let {
            local: var,
            value: Expr::new(
                ExprKind::Index {
                    base: Box::new(Expr::new(ExprKind::Local(arr_local), iterable.ty, span)),
                    index: Box::new(Expr::new(ExprKind::Local(idx), Type::Int, span)),
                },
                elem_ty,
                span,
            ),
        };
        body.stmts.insert(0, read);

        let len_call = Expr::new(
            ExprKind::Call {
                callee: Callee::Builtin(Builtin::Len),
                args: vec![Expr::new(ExprKind::Local(arr_local), iterable.ty, span)],
            },
            Type::Int,
            span,
        );
        let for_stmt = Stmt::For {
            var: idx,
            end_var,
            start: Expr::new(ExprKind::Int(0), Type::Int, span),
            end: len_call,
            body,
        };
        let let_arr = Stmt::Let {
            local: arr_local,
            value: iterable,
        };
        Stmt::Expr(Expr::new(
            ExprKind::Block(Block {
                stmts: vec![let_arr, for_stmt],
                tail: None,
                ty: Type::Unit,
            }),
            Type::Unit,
            span,
        ))
    }

    fn lower_expr(&mut self, expr: &ast::Expr) -> Expr {
        let ty = self.tc.type_of(expr.id);
        let kind = match &expr.kind {
            ast::ExprKind::Int(v) => ExprKind::Int(*v),
            ast::ExprKind::Float(v) => ExprKind::Float(*v),
            ast::ExprKind::Bool(v) => ExprKind::Bool(*v),
            ast::ExprKind::Str(s) => ExprKind::Str(s.clone()),
            ast::ExprKind::Name(_) => self.lower_name(expr.id),
            ast::ExprKind::Unary { op, rhs } => ExprKind::Unary {
                op: *op,
                rhs: Box::new(self.lower_expr(rhs)),
            },
            ast::ExprKind::Binary { op, lhs, rhs } => ExprKind::Binary {
                op: *op,
                lhs: Box::new(self.lower_expr(lhs)),
                rhs: Box::new(self.lower_expr(rhs)),
            },
            ast::ExprKind::Call { callee, args } => self.lower_call(callee, args),
            ast::ExprKind::ArrayLit(elems) => {
                ExprKind::ArrayLit(elems.iter().map(|e| self.lower_expr(e)).collect())
            }
            ast::ExprKind::Index { base, index } => ExprKind::Index {
                base: Box::new(self.lower_expr(base)),
                index: Box::new(self.lower_expr(index)),
            },
            ast::ExprKind::StructLit { name, fields } => self.lower_struct_lit(name, fields),
            ast::ExprKind::Field { base, field } => {
                let idx = self.field_index(base, &field.name);
                ExprKind::GetField {
                    base: Box::new(self.lower_expr(base)),
                    idx,
                }
            }
            // A tuple is a struct with numbered fields.
            ast::ExprKind::TupleLit(elems) => {
                ExprKind::StructLit(elems.iter().map(|e| self.lower_expr(e)).collect())
            }
            ast::ExprKind::TupleIndex { base, index, .. } => ExprKind::GetField {
                base: Box::new(self.lower_expr(base)),
                idx: *index as u32,
            },
            ast::ExprKind::Assign { target, value } => self.lower_assign(target, value),
            ast::ExprKind::AssignOp { target, op, value } => {
                self.lower_assign_op(*op, target, value)
            }
            ast::ExprKind::If(if_expr) => ExprKind::If {
                cond: Box::new(self.lower_expr(&if_expr.cond)),
                then_branch: self.lower_block(&if_expr.then_branch),
                else_branch: if_expr
                    .else_branch
                    .as_ref()
                    .map(|e| Box::new(self.lower_expr(e))),
            },
            ast::ExprKind::Match(m) => self.lower_match(m, ty, expr.span),
            ast::ExprKind::Block(block) => ExprKind::Block(self.lower_block(block)),
        };
        Expr::new(kind, ty, expr.span)
    }

    /// Desugars `match` to an `if` chain on the scrutinee, evaluated once:
    ///
    /// ```text
    /// { let <m> = scrutinee;
    ///   if <m> == p0 { b0 } else if <m> == p1 { b1 } else { default } }
    /// ```
    ///
    /// `default` is the first `_` arm's body; arms after it are unreachable
    /// and dropped. Without `_`, the match is a `bool` match covering both
    /// values, and the last arm becomes the default.
    fn lower_match(&mut self, m: &ast::MatchExpr, ty: Type, span: Span) -> ExprKind {
        let scrut = self.lower_expr(&m.scrutinee);
        let scrut_ty = scrut.ty;
        let tmp = self.alloc_synthetic("<match>", scrut_ty);

        let wild = m
            .arms
            .iter()
            .position(|a| matches!(a.pattern, ast::Pattern::Wild));
        let (cond_arms, default): (&[ast::MatchArm], Option<&ast::Expr>) = match wild {
            Some(i) => (&m.arms[..i], Some(&m.arms[i].body)),
            None => match m.arms.split_last() {
                Some((last, rest)) => (rest, Some(&last.body)),
                None => (&[], None),
            },
        };

        // Build from the last arm back, nesting each `if` in the next one's
        // `else`.
        let mut else_expr: Option<Box<Expr>> = default.map(|b| Box::new(self.lower_expr(b)));
        for arm in cond_arms.iter().rev() {
            let body = self.lower_expr(&arm.body);
            let body_ty = body.ty;
            let then_branch = Block {
                stmts: Vec::new(),
                tail: Some(Box::new(body)),
                ty: body_ty,
            };
            let cond = Expr::new(
                ExprKind::Binary {
                    op: BinOp::Eq,
                    lhs: Box::new(Expr::new(ExprKind::Local(tmp), scrut_ty, arm.span)),
                    rhs: Box::new(self.lower_pattern_literal(&arm.pattern, scrut_ty, arm.span)),
                },
                Type::Bool,
                arm.span,
            );
            let if_expr = Expr::new(
                ExprKind::If {
                    cond: Box::new(cond),
                    then_branch,
                    else_branch: else_expr.take(),
                },
                ty,
                arm.span,
            );
            else_expr = Some(Box::new(if_expr));
        }

        // A match with no arms fails type checking; lower it to `unit`.
        let tail = else_expr.unwrap_or_else(|| {
            Box::new(Expr::new(
                ExprKind::Block(Block {
                    stmts: Vec::new(),
                    tail: None,
                    ty: Type::Unit,
                }),
                Type::Unit,
                span,
            ))
        });
        ExprKind::Block(Block {
            stmts: vec![Stmt::Let {
                local: tmp,
                value: scrut,
            }],
            tail: Some(tail),
            ty,
        })
    }

    /// The literal a pattern compares against.
    fn lower_pattern_literal(&self, pattern: &ast::Pattern, ty: Type, span: Span) -> Expr {
        let kind = match pattern {
            ast::Pattern::Int(v) => ExprKind::Int(*v),
            ast::Pattern::Bool(b) => ExprKind::Bool(*b),
            // Unreachable: `_` becomes the default, never a test.
            ast::Pattern::Wild => ExprKind::Bool(true),
        };
        Expr::new(kind, ty, span)
    }

    /// Lowers a name used as a value: a local read or an inlined constant.
    fn lower_name(&self, use_id: NodeId) -> ExprKind {
        match self.res.use_of(use_id) {
            Some(Res::Local(def)) => ExprKind::Local(self.local_of(def)),
            Some(Res::Const(id)) => const_value_kind(self.tc.const_value(id)),
            // Unreachable: functions are not values.
            _ => ExprKind::Int(0),
        }
    }

    fn lower_call(&mut self, callee: &ast::Expr, args: &[ast::Expr]) -> ExprKind {
        let target = match self.res.use_of(callee.id) {
            Some(Res::Fn(id)) => Callee::Fn(id),
            Some(Res::Builtin(b)) => Callee::Builtin(b),
            // Unreachable: only names are callable.
            _ => Callee::Fn(FnId(0)),
        };
        let args = args.iter().map(|a| self.lower_expr(a)).collect();
        ExprKind::Call {
            callee: target,
            args,
        }
    }

    /// Lowers a struct literal with its values in declaration order, which is
    /// also the order they are evaluated in, whatever order they were written.
    fn lower_struct_lit(&mut self, name: &ast::Ident, fields: &[ast::FieldInit]) -> ExprKind {
        let Some(id) = self.res.structs.lookup(&name.name) else {
            return ExprKind::Int(0); // unreachable: not a struct
        };
        let tc = self.tc;
        let values = tc
            .struct_info(id)
            .fields
            .iter()
            .map(
                |(field_name, _)| match fields.iter().find(|f| &f.name.name == field_name) {
                    Some(fi) => self.lower_expr(&fi.value),
                    None => Expr::new(ExprKind::Int(0), Type::Error, name.span),
                },
            )
            .collect();
        ExprKind::StructLit(values)
    }

    /// The index of `field` in the struct type of `base`.
    fn field_index(&self, base: &ast::Expr, field: &str) -> u32 {
        match self.tc.type_of(base.id) {
            Type::Struct(sid) => self
                .tc
                .struct_info(StructId(sid))
                .field(field)
                .map(|(i, _)| i as u32)
                .unwrap_or(0),
            _ => 0,
        }
    }

    fn lower_assign(&mut self, target: &ast::Expr, value: &ast::Expr) -> ExprKind {
        match &target.kind {
            ast::ExprKind::Index { base, index } => ExprKind::SetIndex {
                base: Box::new(self.lower_expr(base)),
                index: Box::new(self.lower_expr(index)),
                value: Box::new(self.lower_expr(value)),
            },
            ast::ExprKind::Field { base, field } => {
                let idx = self.field_index(base, &field.name);
                self.lower_set_field(base, idx, value)
            }
            ast::ExprKind::TupleIndex { base, index, .. } => {
                self.lower_set_field(base, *index as u32, value)
            }
            _ => ExprKind::Assign {
                local: self.target_local(target),
                value: Box::new(self.lower_expr(value)),
            },
        }
    }

    fn lower_set_field(&mut self, base: &ast::Expr, idx: u32, value: &ast::Expr) -> ExprKind {
        ExprKind::SetField {
            base: Box::new(self.lower_expr(base)),
            idx,
            value: Box::new(self.lower_expr(value)),
        }
    }

    /// Desugars `target op= value` to `target = target op value`.
    fn lower_assign_op(
        &mut self,
        op: ast::BinOp,
        target: &ast::Expr,
        value: &ast::Expr,
    ) -> ExprKind {
        let local = self.target_local(target);
        let ty = self.tc.type_of(target.id);
        let lhs = Expr::new(ExprKind::Local(local), ty, target.span);
        let rhs = self.lower_expr(value);
        let combined = Expr::new(
            ExprKind::Binary {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            },
            ty,
            target.span.to(value.span),
        );
        ExprKind::Assign {
            local,
            value: Box::new(combined),
        }
    }

    // ---- local slots ----

    /// Allocates the next slot for the binding defined by `def`.
    fn alloc_local(&mut self, def: NodeId, name: &str, ty: Type) -> LocalId {
        let id = self.alloc_synthetic(name, ty);
        self.local_map.insert(def, id);
        id
    }

    /// Allocates the next slot for a compiler temporary.
    fn alloc_synthetic(&mut self, name: &str, ty: Type) -> LocalId {
        let id = LocalId(self.locals.len() as u32);
        self.locals.push(LocalDecl {
            name: name.to_string(),
            ty,
        });
        id
    }

    /// The slot of the binding defined by `def`.
    fn local_of(&self, def: NodeId) -> LocalId {
        self.local_map.get(&def).copied().unwrap_or(LocalId(0))
    }

    /// The slot an assignment to `target`, a local's name, writes.
    fn target_local(&self, target: &ast::Expr) -> LocalId {
        match self.res.use_of(target.id) {
            Some(Res::Local(def)) => self.local_of(def),
            _ => LocalId(0),
        }
    }
}
