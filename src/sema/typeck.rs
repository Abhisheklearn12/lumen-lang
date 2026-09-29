//! Type checking: gives every expression and binding a [`Type`] and reports
//! ill-typed code.
//!
//! It runs after [resolution](mod@super::resolve), so every name already points
//! at its definition. Types are inferred bottom-up; annotations, arguments,
//! conditions, and `return` values are then checked against what they must be.
//! There are no implicit conversions: `i64` and `f64` never mix.
//!
//! An ill-typed expression gets [`Type::Error`], which is compatible with
//! everything, so each mistake is reported once. The result is a [`Typeck`] of
//! side tables that HIR lowering reads.

use std::collections::{HashMap, HashSet};

use crate::diagnostics::{Diagnostic, Diagnostics};
use crate::errors::DiagCode;
use crate::parser::ast::*;
use crate::sema::resolve::{ConstId, FnId, Res, Resolution, StructId};
use crate::sema::types::{Builtin, Elem, Type};
use crate::span::Span;

/// A struct's name and fields.
#[derive(Clone, Debug)]
pub struct StructInfo {
    pub name: String,
    /// Fields and their types, in declaration order.
    pub fields: Vec<(String, Type)>,
}

impl StructInfo {
    /// The index and type of field `name`, if it exists.
    pub fn field(&self, name: &str) -> Option<(usize, Type)> {
        self.fields
            .iter()
            .position(|(n, _)| n == name)
            .map(|i| (i, self.fields[i].1))
    }
}

/// A user function's signature.
#[derive(Clone, Debug)]
pub struct FnSig {
    pub name: String,
    pub params: Vec<Type>,
    pub ret: Type,
}

/// A constant's compile-time value.
#[derive(Clone, Debug, PartialEq)]
pub enum ConstValue {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
}

impl ConstValue {
    /// The value's type.
    pub fn ty(&self) -> Type {
        match self {
            ConstValue::Int(_) => Type::Int,
            ConstValue::Float(_) => Type::Float,
            ConstValue::Bool(_) => Type::Bool,
            ConstValue::Str(_) => Type::Str,
        }
    }
}

/// Type checking's output: side tables keyed by [`NodeId`] or by item id.
#[derive(Debug, Default)]
pub struct Typeck {
    /// Each expression's type.
    pub expr_types: HashMap<NodeId, Type>,
    /// Each binding's type, keyed by its definition.
    pub local_types: HashMap<NodeId, Type>,
    /// Indexed by [`FnId`].
    pub signatures: Vec<FnSig>,
    /// Declared types, indexed by [`ConstId`].
    pub const_types: Vec<Type>,
    /// Evaluated values, indexed by [`ConstId`].
    pub const_values: Vec<ConstValue>,
    /// Indexed by [`StructId`].
    pub structs: Vec<StructInfo>,
    /// Element types of each tuple type, indexed by the `u32` in
    /// [`Type::Tuple`]. Equal element lists share one index.
    pub tuple_types: Vec<Vec<Type>>,
    /// The entry point, if the program has a valid `main`.
    pub main: Option<FnId>,
}

impl Typeck {
    /// The type of expression `id`, or [`Type::Error`] if it was never typed
    /// (possible only in a program with errors).
    pub fn type_of(&self, id: NodeId) -> Type {
        self.expr_types.get(&id).copied().unwrap_or(Type::Error)
    }

    /// The type of the binding defined by `id`, or [`Type::Error`].
    pub fn local_type(&self, id: NodeId) -> Type {
        self.local_types.get(&id).copied().unwrap_or(Type::Error)
    }

    /// The signature of function `id`.
    pub fn signature(&self, id: FnId) -> &FnSig {
        &self.signatures[id.0 as usize]
    }

    /// The declared type of constant `id`, or [`Type::Error`].
    pub fn const_type(&self, id: ConstId) -> Type {
        self.const_types
            .get(id.0 as usize)
            .copied()
            .unwrap_or(Type::Error)
    }

    /// The value of constant `id`.
    pub fn const_value(&self, id: ConstId) -> &ConstValue {
        &self.const_values[id.0 as usize]
    }

    /// The fields of struct `id`.
    pub fn struct_info(&self, id: StructId) -> &StructInfo {
        &self.structs[id.0 as usize]
    }

    /// The element types of tuple type `id`.
    pub fn tuple_elems(&self, id: u32) -> &[Type] {
        &self.tuple_types[id as usize]
    }

    /// The index of the tuple type with these elements, added if new, so that
    /// equal tuple types compare equal.
    fn intern_tuple(&mut self, elems: Vec<Type>) -> u32 {
        if let Some(pos) = self.tuple_types.iter().position(|t| *t == elems) {
            return pos as u32;
        }
        self.tuple_types.push(elems);
        (self.tuple_types.len() - 1) as u32
    }
}

/// Type-checks `ast`, reporting errors to `diags`.
#[tracing::instrument(level = "debug", skip_all)]
pub fn check(ast: &Ast, res: &Resolution, diags: &mut Diagnostics) -> Typeck {
    let mut checker = Checker {
        ast,
        res,
        diags,
        tc: Typeck::default(),
        cur_ret: Type::Unit,
        loop_depth: 0,
    };
    checker.collect_structs();
    checker.collect_signatures();
    checker.check_consts();
    checker.check_main();
    for (fn_id, item_idx) in res.functions.iter() {
        if let ItemKind::Fn(decl) = &ast.items[item_idx].kind {
            checker.check_fn(fn_id, decl);
        }
    }
    tracing::debug!(
        typed_exprs = checker.tc.expr_types.len(),
        "type checking complete"
    );
    checker.tc
}

struct Checker<'a> {
    ast: &'a Ast,
    res: &'a Resolution,
    diags: &'a mut Diagnostics,
    tc: Typeck,
    /// The return type of the function being checked.
    cur_ret: Type,
    /// How many loops enclose the current code, for `break`/`continue`.
    loop_depth: u32,
}

impl Checker<'_> {
    // ---- signatures ----

    /// Records every function's signature before any body is checked, so calls
    /// may precede declarations.
    fn collect_signatures(&mut self) {
        for (_fn_id, item_idx) in self.res.functions.iter() {
            let ItemKind::Fn(decl) = &self.ast.items[item_idx].kind else {
                continue;
            };
            let params: Vec<Type> = decl
                .params
                .iter()
                .map(|p| {
                    let ty = self.resolve_type(&p.ty);
                    self.tc.local_types.insert(p.id, ty);
                    ty
                })
                .collect();
            let ret = decl
                .ret
                .as_ref()
                .map(|t| self.resolve_type(t))
                .unwrap_or(Type::Unit);
            self.tc.signatures.push(FnSig {
                name: decl.name.name.clone(),
                params,
                ret,
            });
        }
    }

    /// Records every struct's fields up front, so struct types may be named
    /// before their declaration.
    fn collect_structs(&mut self) {
        for (_id, item_idx) in self.res.structs.iter() {
            let ItemKind::Struct(decl) = &self.ast.items[item_idx].kind else {
                continue;
            };
            let mut fields = Vec::with_capacity(decl.fields.len());
            let mut seen: HashSet<&str> = HashSet::new();
            for field in &decl.fields {
                if !seen.insert(&field.name.name) {
                    self.diags.emit(
                        Diagnostic::error(
                            DiagCode::BadStructLiteral,
                            format!("field `{}` is declared more than once", field.name.name),
                        )
                        .with_primary(field.name.span, "duplicate field"),
                    );
                }
                let ty = self.resolve_type(&field.ty);
                fields.push((field.name.name.clone(), ty));
            }
            self.tc.structs.push(StructInfo {
                name: decl.name.name.clone(),
                fields,
            });
        }
    }

    /// Checks and evaluates each constant in declaration order; a constant may
    /// use only constants declared before it.
    fn check_consts(&mut self) {
        for (_const_id, item_idx) in self.res.consts.iter() {
            let ItemKind::Const(decl) = &self.ast.items[item_idx].kind else {
                continue;
            };
            let declared = self.resolve_type(&decl.ty);
            let value_ty = self.check_expr(&decl.value);
            if !value_ty.compatible(declared) {
                self.diags.emit(
                    mismatch(
                        format!("mismatched types in `const {}`", decl.name.name),
                        decl.value.span,
                        declared,
                        value_ty,
                    )
                    .with_label(decl.ty.span, "expected due to this annotation"),
                );
            }
            let value = self.eval_const(&decl.value).unwrap_or_else(|| {
                // An ill-typed value has already been reported.
                if !value_ty.is_error() {
                    self.diags.emit(
                        Diagnostic::error(
                            DiagCode::NotConstant,
                            format!("`const {}` is not a constant expression", decl.name.name),
                        )
                        .with_primary(decl.value.span, "must be evaluable at compile time")
                        .with_note("constants may use literals, operators, and earlier constants"),
                    );
                }
                default_const_value(declared)
            });
            self.tc.const_types.push(declared);
            self.tc.const_values.push(value);
        }
    }

    /// The value of `expr`, or `None` if it is not a compile-time constant.
    fn eval_const(&self, expr: &Expr) -> Option<ConstValue> {
        match &expr.kind {
            ExprKind::Int(v) => Some(ConstValue::Int(*v)),
            ExprKind::Float(v) => Some(ConstValue::Float(*v)),
            ExprKind::Bool(v) => Some(ConstValue::Bool(*v)),
            ExprKind::Str(s) => Some(ConstValue::Str(s.clone())),
            ExprKind::Name(_) => match self.res.use_of(expr.id) {
                // Only constants already evaluated, i.e. declared earlier.
                Some(Res::Const(id)) if (id.0 as usize) < self.tc.const_values.len() => {
                    Some(self.tc.const_values[id.0 as usize].clone())
                }
                _ => None,
            },
            ExprKind::Unary { op, rhs } => eval_const_unary(*op, self.eval_const(rhs)?),
            ExprKind::Binary { op, lhs, rhs } => {
                eval_const_binary(*op, self.eval_const(lhs)?, self.eval_const(rhs)?)
            }
            _ => None,
        }
    }

    /// The [`Type`] a [`TypeExpr`] denotes, reporting unknown names and invalid
    /// array element types.
    fn resolve_type(&mut self, ty: &TypeExpr) -> Type {
        match &ty.kind {
            TypeExprKind::Named(name) => {
                if let Some(ty) = Type::from_name(name) {
                    return ty;
                }
                if let Some(id) = self.res.structs.lookup(name) {
                    return Type::Struct(id.0);
                }
                let mut diag =
                    Diagnostic::error(DiagCode::UnknownType, format!("unknown type `{name}`"))
                        .with_primary(ty.span, "not a known type")
                        .with_help("the built-in types are i64, f64, bool, str, unit");
                const PRIMS: [&str; 5] = ["i64", "f64", "bool", "str", "unit"];
                if let Some(hint) = crate::suggest::closest(name, PRIMS) {
                    diag = diag.with_help(format!("did you mean `{hint}`?"));
                }
                self.diags.emit(diag);
                Type::Error
            }
            TypeExprKind::Array(inner) => {
                let elem_ty = self.resolve_type(inner);
                if elem_ty.is_error() {
                    return Type::Error;
                }
                match Elem::of(elem_ty) {
                    Some(elem) => Type::Array(elem),
                    None => {
                        self.diags.emit(
                            Diagnostic::error(
                                DiagCode::BadArrayType,
                                format!("`{elem_ty}` is not a valid array element type"),
                            )
                            .with_primary(
                                inner.span,
                                "array elements must be i64, f64, bool, or str",
                            ),
                        );
                        Type::Error
                    }
                }
            }
            TypeExprKind::Tuple(elem_exprs) => {
                let elems: Vec<Type> = elem_exprs.iter().map(|e| self.resolve_type(e)).collect();
                if elems.iter().any(|t| t.is_error()) {
                    return Type::Error;
                }
                Type::Tuple(self.tc.intern_tuple(elems))
            }
            // The parser already reported it.
            TypeExprKind::Error => Type::Error,
        }
    }

    /// Checks that the program has a `fn main()` with no parameters returning
    /// `unit`.
    fn check_main(&mut self) {
        match self.res.functions.lookup("main") {
            Some(id) => {
                let sig = self.tc.signature(id);
                let ok = sig.params.is_empty() && matches!(sig.ret, Type::Unit);
                if ok {
                    self.tc.main = Some(id);
                } else {
                    let item = &self.ast.items[self.res.functions.item_index(id)];
                    self.diags.emit(
                        Diagnostic::error(DiagCode::BadMain, "`main` has an invalid signature")
                            .with_primary(
                                item.span,
                                "must be `fn main()` taking no arguments and returning unit",
                            ),
                    );
                }
            }
            None => {
                self.diags.emit(
                    Diagnostic::error(DiagCode::BadMain, "program has no `main` function")
                        .with_help("add an entry point: `fn main() { ... }`"),
                );
            }
        }
    }

    // ---- functions / blocks / statements ----

    fn check_fn(&mut self, fn_id: FnId, decl: &FnDecl) {
        self.cur_ret = self.tc.signature(fn_id).ret;
        let body_ty = self.check_block(&decl.body);

        // If the body always hits a `return`, each was checked where it stands.
        // Otherwise the body's value is the return value.
        if block_diverges(&decl.body) || body_ty.compatible(self.cur_ret) {
            return;
        }
        if matches!(body_ty, Type::Unit) && !matches!(self.cur_ret, Type::Unit) {
            self.diags.emit(
                Diagnostic::error(
                    DiagCode::MissingReturn,
                    format!(
                        "function `{}` may reach its end without returning `{}`",
                        decl.name.name, self.cur_ret
                    ),
                )
                .with_primary(decl.body.span, format!("expected `{}` value", self.cur_ret))
                .with_help("add a trailing expression or a `return`"),
            );
        } else {
            let tail_span = decl
                .body
                .tail
                .as_ref()
                .map(|t| t.span)
                .unwrap_or(decl.body.span);
            self.diags.emit(
                Diagnostic::error(
                    DiagCode::ReturnTypeMismatch,
                    format!(
                        "function `{}` should return `{}` but its body has type `{}`",
                        decl.name.name, self.cur_ret, body_ty
                    ),
                )
                .with_primary(
                    tail_span,
                    format!("expected `{}`, found `{}`", self.cur_ret, body_ty),
                ),
            );
        }
    }

    /// Checks a block, returning its tail's type (`unit` if none).
    fn check_block(&mut self, block: &Block) -> Type {
        for stmt in &block.stmts {
            self.check_stmt(stmt);
        }
        match &block.tail {
            Some(tail) => self.check_expr(tail),
            None => Type::Unit,
        }
    }

    fn check_stmt(&mut self, stmt: &Stmt) {
        match &stmt.kind {
            StmtKind::Let(l) => self.check_let(l),
            StmtKind::Expr(e) => {
                self.check_expr(e);
            }
            StmtKind::Return(value) => self.check_return(value.as_ref(), stmt.span),
            StmtKind::While(w) => {
                let cond = self.check_expr(&w.cond);
                self.expect(
                    cond,
                    Type::Bool,
                    w.cond.span,
                    DiagCode::NonBoolCondition,
                    "`while` condition",
                );
                self.check_loop_body(&w.body);
            }
            StmtKind::For(f) => self.check_for(f),
            StmtKind::ForEach(f) => self.check_for_each(f),
            StmtKind::Break => self.check_loop_jump("break", stmt.span),
            StmtKind::Continue => self.check_loop_jump("continue", stmt.span),
        }
    }

    /// Checks `for v in array`: `v` has the array's element type.
    fn check_for_each(&mut self, f: &ForEachStmt) {
        let iter_ty = self.check_expr(&f.iterable);
        let elem = match iter_ty {
            Type::Array(elem) => elem.ty(),
            Type::Error => Type::Error,
            other => {
                self.diags.emit(
                    Diagnostic::error(DiagCode::NotIndexable, format!("`{other}` is not iterable"))
                        .with_primary(f.iterable.span, "`for ... in` needs an array"),
                );
                Type::Error
            }
        };
        self.tc.local_types.insert(f.id, elem);
        self.check_loop_body(&f.body);
    }

    /// Checks `for v in start..end`: both bounds and `v` are `i64`.
    fn check_for(&mut self, f: &ForStmt) {
        let start = self.check_expr(&f.start);
        self.expect(
            start,
            Type::Int,
            f.start.span,
            DiagCode::TypeMismatch,
            "range start",
        );
        let end = self.check_expr(&f.end);
        self.expect(
            end,
            Type::Int,
            f.end.span,
            DiagCode::TypeMismatch,
            "range end",
        );
        self.tc.local_types.insert(f.id, Type::Int);
        self.check_loop_body(&f.body);
    }

    fn check_loop_body(&mut self, body: &Block) {
        self.loop_depth += 1;
        self.check_block(body);
        self.loop_depth -= 1;
    }

    /// Reports `break` or `continue` outside a loop.
    fn check_loop_jump(&mut self, kw: &str, span: Span) {
        if self.loop_depth == 0 {
            self.diags.emit(
                Diagnostic::error(
                    DiagCode::BreakOutsideLoop,
                    format!("`{kw}` outside of a loop"),
                )
                .with_primary(span, format!("`{kw}` can only be used inside a loop")),
            );
        }
    }

    fn check_let(&mut self, l: &LetStmt) {
        let init = self.check_expr(&l.init);
        let ty = match &l.ty {
            Some(annot) => {
                let expected = self.resolve_type(annot);
                if !init.compatible(expected) {
                    self.diags.emit(
                        mismatch(
                            format!("mismatched types in `let {}`", l.name.name),
                            l.init.span,
                            expected,
                            init,
                        )
                        .with_label(annot.span, "expected due to this annotation"),
                    );
                }
                expected
            }
            None => init,
        };
        self.tc.local_types.insert(l.id, ty);
    }

    fn check_return(&mut self, value: Option<&Expr>, span: Span) {
        let actual = match value {
            Some(e) => self.check_expr(e),
            None => Type::Unit,
        };
        if !actual.compatible(self.cur_ret) {
            let span = value.map(|e| e.span).unwrap_or(span);
            self.diags.emit(
                Diagnostic::error(
                    DiagCode::ReturnTypeMismatch,
                    format!(
                        "returning `{actual}` from a function declared to return `{}`",
                        self.cur_ret
                    ),
                )
                .with_primary(
                    span,
                    format!("expected `{}`, found `{actual}`", self.cur_ret),
                ),
            );
        }
    }

    // ---- expressions ----

    /// Infers and records the type of `expr`.
    fn check_expr(&mut self, expr: &Expr) -> Type {
        let ty = match &expr.kind {
            ExprKind::Int(_) => Type::Int,
            ExprKind::Float(_) => Type::Float,
            ExprKind::Bool(_) => Type::Bool,
            ExprKind::Str(_) => Type::Str,
            ExprKind::Name(name) => self.check_name(expr.id, name, expr.span),
            ExprKind::Unary { op, rhs } => self.check_unary(*op, rhs),
            ExprKind::Binary { op, lhs, rhs } => self.check_binary(*op, lhs, rhs),
            ExprKind::Call { callee, args } => self.check_call(callee, args, expr.span),
            ExprKind::Assign { target, value } => self.check_assign(target, value),
            ExprKind::AssignOp { target, op, value } => self.check_assign_op(*op, target, value),
            ExprKind::ArrayLit(elems) => self.check_array_lit(elems, expr.span),
            ExprKind::Index { base, index } => self.check_index(base, index),
            ExprKind::StructLit { name, fields } => self.check_struct_lit(name, fields, expr.span),
            ExprKind::Field { base, field } => self.check_field(base, field),
            ExprKind::TupleLit(elems) => self.check_tuple_lit(elems),
            ExprKind::TupleIndex {
                base,
                index,
                index_span,
            } => self.check_tuple_index(base, *index, *index_span),
            ExprKind::If(if_expr) => self.check_if(if_expr, expr.span),
            ExprKind::Match(m) => self.check_match(m, expr.span),
            ExprKind::Block(block) => self.check_block(block),
        };
        self.tc.expr_types.insert(expr.id, ty);
        ty
    }

    fn check_name(&mut self, id: NodeId, name: &str, span: Span) -> Type {
        match self.res.use_of(id) {
            Some(Res::Local(def)) => self.tc.local_type(def),
            Some(Res::Const(cid)) => self.tc.const_type(cid),
            // A function name is only valid as a call target.
            Some(Res::Fn(_)) | Some(Res::Builtin(_)) => {
                self.diags.emit(
                    Diagnostic::error(
                        DiagCode::TypeMismatch,
                        format!("`{name}` is a function and cannot be used as a value"),
                    )
                    .with_primary(span, "functions are not first-class values in Lumen")
                    .with_help("call it with `()` instead"),
                );
                Type::Error
            }
            // Resolution already reported it.
            None => Type::Error,
        }
    }

    fn check_unary(&mut self, op: UnOp, rhs: &Expr) -> Type {
        let t = self.check_expr(rhs);
        match op {
            UnOp::Neg if t.is_numeric() || t.is_error() => t,
            UnOp::Not if matches!(t, Type::Bool) || t.is_error() => Type::Bool,
            _ => {
                self.diags.emit(
                    Diagnostic::error(
                        DiagCode::InvalidOperands,
                        format!("cannot apply unary `{}` to `{t}`", op.symbol()),
                    )
                    .with_primary(rhs.span, format!("operand has type `{t}`")),
                );
                Type::Error
            }
        }
    }

    fn check_binary(&mut self, op: BinOp, lhs: &Expr, rhs: &Expr) -> Type {
        let lt = self.check_expr(lhs);
        let rt = self.check_expr(rhs);
        // Comparisons and logic are `bool` even when ill-typed, which limits
        // cascading errors.
        let yields_bool = op.is_comparison() || op.is_logical();
        if lt.is_error() || rt.is_error() {
            return if yields_bool { Type::Bool } else { Type::Error };
        }
        use BinOp::*;
        let ok = match op {
            // `+` also concatenates strings.
            Add if lt == Type::Str && rt == Type::Str => return Type::Str,
            Add | Sub | Mul | Div | Rem | Lt | Le | Gt | Ge => lt == rt && lt.is_numeric(),
            Eq | Ne => lt == rt && lt != Type::Unit,
            And | Or => lt == Type::Bool && rt == Type::Bool,
        };
        if !ok {
            self.operand_error(op, lt, rt, lhs.span.to(rhs.span));
        }
        if yields_bool {
            Type::Bool
        } else if ok {
            lt
        } else {
            Type::Error
        }
    }

    fn operand_error(&mut self, op: BinOp, lt: Type, rt: Type, span: Span) {
        self.diags.emit(
            Diagnostic::error(
                DiagCode::InvalidOperands,
                format!("cannot apply `{}` to `{lt}` and `{rt}`", op.symbol()),
            )
            .with_primary(span, format!("`{lt}` {} `{rt}`", op.symbol())),
        );
    }

    fn check_call(&mut self, callee: &Expr, args: &[Expr], span: Span) -> Type {
        // Only a function or builtin name can be called; there are no function
        // values.
        if let ExprKind::Name(name) = &callee.kind {
            match self.res.use_of(callee.id) {
                Some(Res::Fn(id)) => {
                    let sig = self.tc.signature(id).clone();
                    self.check_args(&sig.params, args, &sig.name, span);
                    return sig.ret;
                }
                Some(Res::Builtin(b)) if b.is_generic() => {
                    return self.check_generic_builtin(b, args, span);
                }
                Some(Res::Builtin(b)) => {
                    self.check_args(b.params(), args, b.name(), span);
                    return b.ret();
                }
                Some(Res::Local(_)) | Some(Res::Const(_)) => {
                    self.diags.emit(
                        Diagnostic::error(
                            DiagCode::NotCallable,
                            format!("`{name}` is a value, not a function"),
                        )
                        .with_primary(callee.span, "cannot be called"),
                    );
                }
                None => {} // already reported by resolution
            }
        } else {
            let t = self.check_expr(callee);
            if !t.is_error() {
                self.diags.emit(
                    Diagnostic::error(DiagCode::NotCallable, format!("type `{t}` is not callable"))
                        .with_primary(callee.span, "cannot be called"),
                );
            }
        }
        // Check the arguments anyway, for their own errors.
        for arg in args {
            self.check_expr(arg);
        }
        Type::Error
    }

    /// Checks the argument count and each argument's type against `params`.
    fn check_args(&mut self, params: &[Type], args: &[Expr], callee: &str, span: Span) {
        if params.len() != args.len() {
            self.diags.emit(
                Diagnostic::error(
                    DiagCode::ArityMismatch,
                    format!(
                        "`{callee}` expects {} argument(s) but {} were supplied",
                        params.len(),
                        args.len()
                    ),
                )
                .with_primary(span, format!("expected {} argument(s)", params.len())),
            );
        }
        for (arg, &expected) in args.iter().zip(params) {
            let actual = self.check_expr(arg);
            if !actual.compatible(expected) {
                self.diags.emit(mismatch(
                    format!("argument to `{callee}` has the wrong type"),
                    arg.span,
                    expected,
                    actual,
                ));
            }
        }
        // Surplus arguments still get checked, for their own errors.
        for arg in args.iter().skip(params.len()) {
            self.check_expr(arg);
        }
    }

    /// Checks a call to a generic builtin; only `len`, which takes any array.
    fn check_generic_builtin(&mut self, builtin: Builtin, args: &[Expr], span: Span) -> Type {
        debug_assert!(matches!(builtin, Builtin::Len));
        if args.len() != 1 {
            self.diags.emit(
                Diagnostic::error(
                    DiagCode::ArityMismatch,
                    format!(
                        "`{}` expects 1 argument but {} were supplied",
                        builtin.name(),
                        args.len()
                    ),
                )
                .with_primary(span, "expected 1 argument"),
            );
            for arg in args {
                self.check_expr(arg);
            }
            return builtin.ret();
        }
        let arg_ty = self.check_expr(&args[0]);
        if !arg_ty.is_array() && !arg_ty.is_error() {
            self.diags.emit(
                Diagnostic::error(
                    DiagCode::TypeMismatch,
                    format!("`len` expects an array, found `{arg_ty}`"),
                )
                .with_primary(args[0].span, "not an array"),
            );
        }
        builtin.ret()
    }

    /// Checks `target = value`.
    ///
    /// Arrays, structs, and tuples are shared by reference, so assigning to an
    /// element or field mutates the shared value, not a binding, and needs no
    /// `mut`. That is what lets a function mutate an array argument. Only
    /// rebinding a variable requires `mut`.
    fn check_assign(&mut self, target: &Expr, value: &Expr) -> Type {
        match &target.kind {
            ExprKind::Index { base, index } => return self.check_index_assign(base, index, value),
            ExprKind::Field { base, field } => {
                let field_ty = self.check_field(base, field);
                return self.check_store(field_ty, value, "field");
            }
            ExprKind::TupleIndex {
                base,
                index,
                index_span,
            } => {
                let elem_ty = self.check_tuple_index(base, *index, *index_span);
                return self.check_store(elem_ty, value, "tuple");
            }
            _ => {}
        }
        let value_ty = self.check_expr(value);
        // A variable must be a mutable local.
        if let ExprKind::Name(name) = &target.kind
            && let Some(Res::Local(def)) = self.res.use_of(target.id)
        {
            let target_ty = self.tc.local_type(def);
            self.tc.expr_types.insert(target.id, target_ty);
            let mutable = self.res.local(def).map(|l| l.mutable).unwrap_or(false);
            if !mutable {
                self.diags.emit(
                    Diagnostic::error(
                        DiagCode::AssignToImmutable,
                        format!("cannot assign to immutable binding `{name}`"),
                    )
                    .with_primary(target.span, "cannot assign twice to an immutable binding")
                    .with_help(format!(
                        "declare it with `let mut {name}` to allow assignment"
                    )),
                );
            } else if !value_ty.compatible(target_ty) {
                self.diags.emit(mismatch(
                    format!("mismatched types assigning to `{name}`"),
                    value.span,
                    target_ty,
                    value_ty,
                ));
            }
            return Type::Unit;
        }
        // Anything else is not assignable.
        if !self.check_expr(target).is_error() {
            self.diags.emit(
                Diagnostic::error(DiagCode::InvalidAssignTarget, "invalid assignment target")
                    .with_primary(target.span, "cannot assign to this expression"),
            );
        }
        Type::Unit
    }

    /// Checks `base[index] = value`.
    fn check_index_assign(&mut self, base: &Expr, index: &Expr, value: &Expr) -> Type {
        let base_ty = self.check_expr(base);
        let index_ty = self.check_expr(index);
        let value_ty = self.check_expr(value);
        self.expect(
            index_ty,
            Type::Int,
            index.span,
            DiagCode::TypeMismatch,
            "array index",
        );
        let elem_ty = self.indexed_elem(base_ty, base.span);
        if !value_ty.compatible(elem_ty) {
            self.diags.emit(mismatch(
                "mismatched types in element assignment",
                value.span,
                elem_ty,
                value_ty,
            ));
        }
        Type::Unit
    }

    /// Checks `value` against a field or tuple element of type `place_ty`.
    fn check_store(&mut self, place_ty: Type, value: &Expr, what: &str) -> Type {
        let value_ty = self.check_expr(value);
        if !value_ty.compatible(place_ty) {
            self.diags.emit(mismatch(
                format!("mismatched types in {what} assignment"),
                value.span,
                place_ty,
                value_ty,
            ));
        }
        Type::Unit
    }

    /// Checks `target op= value`: the target must be a mutable local, and
    /// `target op value` must have the target's (numeric) type.
    fn check_assign_op(&mut self, op: BinOp, target: &Expr, value: &Expr) -> Type {
        let target_ty = self.check_expr(target);
        let value_ty = self.check_expr(value);

        let mutable = match (&target.kind, self.res.use_of(target.id)) {
            (ExprKind::Name(_), Some(Res::Local(def))) => {
                self.res.local(def).map(|l| l.mutable).unwrap_or(false)
            }
            _ => {
                if !target_ty.is_error() {
                    self.diags.emit(
                        Diagnostic::error(
                            DiagCode::InvalidAssignTarget,
                            "invalid compound-assignment target",
                        )
                        .with_primary(target.span, "cannot assign to this expression"),
                    );
                }
                return Type::Unit;
            }
        };
        if !mutable
            && !target_ty.is_error()
            && let ExprKind::Name(name) = &target.kind
        {
            self.diags.emit(
                Diagnostic::error(
                    DiagCode::AssignToImmutable,
                    format!("cannot assign to immutable binding `{name}`"),
                )
                .with_primary(target.span, "binding is not mutable")
                .with_help(format!("declare it with `let mut {name}`")),
            );
        }
        // Compound assignment cannot change the target's type.
        let operands_ok = target_ty.is_error()
            || value_ty.is_error()
            || (target_ty == value_ty && target_ty.is_numeric());
        if !operands_ok {
            self.operand_error(op, target_ty, value_ty, target.span.to(value.span));
        }
        Type::Unit
    }

    /// Checks an array literal: a non-empty list of one primitive type.
    fn check_array_lit(&mut self, elems: &[Expr], span: Span) -> Type {
        if elems.is_empty() {
            self.diags.emit(
                Diagnostic::error(
                    DiagCode::BadArrayType,
                    "cannot infer the type of an empty array literal",
                )
                .with_primary(span, "give it at least one element")
                .with_help("empty arrays are not yet supported"),
            );
            return Type::Error;
        }
        let first = self.check_expr(&elems[0]);
        for e in &elems[1..] {
            let t = self.check_expr(e);
            if !t.compatible(first) {
                self.diags.emit(mismatch(
                    "array elements have differing types",
                    e.span,
                    first,
                    t,
                ));
            }
        }
        if first.is_error() {
            return Type::Error;
        }
        match Elem::of(first) {
            Some(elem) => Type::Array(elem),
            None => {
                self.diags.emit(
                    Diagnostic::error(
                        DiagCode::BadArrayType,
                        format!("`{first}` is not a valid array element type"),
                    )
                    .with_primary(elems[0].span, "elements must be i64, f64, bool, or str"),
                );
                Type::Error
            }
        }
    }

    /// Checks `base[index]`.
    fn check_index(&mut self, base: &Expr, index: &Expr) -> Type {
        let base_ty = self.check_expr(base);
        let index_ty = self.check_expr(index);
        self.expect(
            index_ty,
            Type::Int,
            index.span,
            DiagCode::TypeMismatch,
            "array index",
        );
        self.indexed_elem(base_ty, base.span)
    }

    /// The element type of an array of type `base_ty`, reporting a base that is
    /// not an array.
    fn indexed_elem(&mut self, base_ty: Type, base_span: Span) -> Type {
        match base_ty {
            Type::Array(elem) => elem.ty(),
            Type::Error => Type::Error,
            other => {
                self.diags.emit(
                    Diagnostic::error(
                        DiagCode::NotIndexable,
                        format!("`{other}` cannot be indexed"),
                    )
                    .with_primary(base_span, "not an array"),
                );
                Type::Error
            }
        }
    }

    /// Checks `Name { field: value, ... }`: each declared field exactly once,
    /// with the right type.
    fn check_struct_lit(&mut self, name: &Ident, fields: &[FieldInit], span: Span) -> Type {
        let Some(id) = self.res.structs.lookup(&name.name) else {
            // Check the values anyway, for their own errors.
            for f in fields {
                self.check_expr(&f.value);
            }
            self.diags.emit(
                Diagnostic::error(
                    DiagCode::BadStructLiteral,
                    format!("`{}` is not a struct", name.name),
                )
                .with_primary(name.span, "unknown struct"),
            );
            return Type::Error;
        };
        let info = self.tc.struct_info(id).clone();

        let mut provided: HashSet<&str> = HashSet::new();
        for f in fields {
            let actual = self.check_expr(&f.value);
            match info.field(&f.name.name) {
                Some((_, expected)) => {
                    if !provided.insert(&f.name.name) {
                        self.diags.emit(
                            Diagnostic::error(
                                DiagCode::BadStructLiteral,
                                format!("field `{}` specified more than once", f.name.name),
                            )
                            .with_primary(f.name.span, "duplicate field"),
                        );
                    }
                    if !actual.compatible(expected) {
                        self.diags.emit(mismatch(
                            format!("field `{}` has the wrong type", f.name.name),
                            f.value.span,
                            expected,
                            actual,
                        ));
                    }
                }
                None => {
                    self.diags.emit(
                        Diagnostic::error(
                            DiagCode::UnknownField,
                            format!("struct `{}` has no field `{}`", info.name, f.name.name),
                        )
                        .with_primary(f.name.span, "no such field"),
                    );
                }
            }
        }
        let missing: Vec<&str> = info
            .fields
            .iter()
            .map(|(n, _)| n.as_str())
            .filter(|n| !provided.contains(n))
            .collect();
        if !missing.is_empty() {
            self.diags.emit(
                Diagnostic::error(
                    DiagCode::BadStructLiteral,
                    format!(
                        "missing field(s) {} in `{}` literal",
                        missing.join(", "),
                        info.name
                    ),
                )
                .with_primary(span, "all fields must be provided"),
            );
        }
        Type::Struct(id.0)
    }

    /// Checks `base.field`.
    fn check_field(&mut self, base: &Expr, field: &Ident) -> Type {
        let base_ty = self.check_expr(base);
        match base_ty {
            Type::Struct(sid) => {
                let info = self.tc.struct_info(StructId(sid));
                match info.field(&field.name) {
                    Some((_, ty)) => ty,
                    None => {
                        let name = info.name.clone();
                        self.diags.emit(
                            Diagnostic::error(
                                DiagCode::UnknownField,
                                format!("struct `{name}` has no field `{}`", field.name),
                            )
                            .with_primary(field.span, "no such field"),
                        );
                        Type::Error
                    }
                }
            }
            Type::Error => Type::Error,
            other => {
                self.diags.emit(
                    Diagnostic::error(
                        DiagCode::UnknownField,
                        format!("type `{other}` has no fields"),
                    )
                    .with_primary(base.span, "not a struct"),
                );
                Type::Error
            }
        }
    }

    /// Checks a tuple literal.
    fn check_tuple_lit(&mut self, elems: &[Expr]) -> Type {
        let tys: Vec<Type> = elems.iter().map(|e| self.check_expr(e)).collect();
        if tys.iter().any(|t| t.is_error()) {
            return Type::Error;
        }
        Type::Tuple(self.tc.intern_tuple(tys))
    }

    /// Checks `base.0`, `base.1`, ...
    fn check_tuple_index(&mut self, base: &Expr, index: usize, index_span: Span) -> Type {
        let base_ty = self.check_expr(base);
        match base_ty {
            Type::Tuple(id) => {
                let elems = self.tc.tuple_elems(id);
                match elems.get(index) {
                    Some(&ty) => ty,
                    None => {
                        let arity = elems.len();
                        self.diags.emit(
                            Diagnostic::error(
                                DiagCode::UnknownField,
                                format!(
                                    "tuple has {arity} element(s); index {index} is out of range"
                                ),
                            )
                            .with_primary(index_span, "no such tuple element"),
                        );
                        Type::Error
                    }
                }
            }
            Type::Error => Type::Error,
            other => {
                self.diags.emit(
                    Diagnostic::error(
                        DiagCode::UnknownField,
                        format!("type `{other}` is not a tuple"),
                    )
                    .with_primary(base.span, "not a tuple"),
                );
                Type::Error
            }
        }
    }

    fn check_if(&mut self, if_expr: &IfExpr, span: Span) -> Type {
        let cond = self.check_expr(&if_expr.cond);
        self.expect(
            cond,
            Type::Bool,
            if_expr.cond.span,
            DiagCode::NonBoolCondition,
            "`if` condition",
        );
        let then_ty = self.check_block(&if_expr.then_branch);
        match &if_expr.else_branch {
            Some(else_branch) => {
                let else_ty = self.check_expr(else_branch);
                if then_ty.compatible(else_ty) {
                    join(then_ty, else_ty)
                } else {
                    self.diags.emit(
                        Diagnostic::error(
                            DiagCode::IfBranchMismatch,
                            "`if` and `else` branches have incompatible types",
                        )
                        .with_primary(span, format!("`{then_ty}` vs `{else_ty}`"))
                        .with_help("both branches of an `if` expression must have the same type"),
                    );
                    Type::Error
                }
            }
            // Without `else` the `if` is `unit`, so its block must be too.
            None => {
                if !then_ty.compatible(Type::Unit) {
                    self.diags.emit(
                        Diagnostic::error(
                            DiagCode::IfBranchMismatch,
                            "`if` without `else` must have type `unit`",
                        )
                        .with_primary(
                            if_expr.then_branch.span,
                            format!("this block has type `{then_ty}`"),
                        )
                        .with_help("add an `else` branch that yields the same type"),
                    );
                }
                Type::Unit
            }
        }
    }

    /// Checks a `match`: an `i64` or `bool` scrutinee, patterns of its type, arms
    /// of one type, and exhaustive coverage.
    fn check_match(&mut self, m: &MatchExpr, span: Span) -> Type {
        let scrut = self.check_expr(&m.scrutinee);
        let scalar = matches!(scrut, Type::Int | Type::Bool);
        if !scrut.is_error() && !scalar {
            self.diags.emit(
                Diagnostic::error(
                    DiagCode::TypeMismatch,
                    format!("cannot `match` on a value of type `{scrut}`"),
                )
                .with_primary(m.scrutinee.span, "only `i64` and `bool` can be matched")
                .with_help("`match` compares the scrutinee against scalar literals"),
            );
        }

        let mut result: Option<Type> = None;
        let mut has_wild = false;
        let mut saw_true = false;
        let mut saw_false = false;
        for arm in &m.arms {
            self.check_arm_pattern(&arm.pattern, scrut, arm.span);
            match &arm.pattern {
                Pattern::Wild => has_wild = true,
                Pattern::Bool(true) => saw_true = true,
                Pattern::Bool(false) => saw_false = true,
                Pattern::Int(_) => {}
            }
            let body_ty = self.check_expr(&arm.body);
            result = Some(match result {
                None => body_ty,
                Some(prev) if prev.compatible(body_ty) => join(prev, body_ty),
                Some(prev) => {
                    self.diags.emit(
                        Diagnostic::error(
                            DiagCode::IfBranchMismatch,
                            "`match` arms have incompatible types",
                        )
                        .with_primary(arm.span, format!("`{prev}` vs `{body_ty}`"))
                        .with_help("every arm of a `match` must yield the same type"),
                    );
                    Type::Error
                }
            });
        }

        // Only `_`, or both `true` and `false`, make a match exhaustive.
        let exhaustive = has_wild || (scrut == Type::Bool && saw_true && saw_false);
        if !scrut.is_error() && scalar && !exhaustive {
            self.diags.emit(
                Diagnostic::error(
                    DiagCode::NonExhaustiveMatch,
                    "`match` does not cover every possible value",
                )
                .with_primary(span, "add the missing patterns or a `_` arm"),
            );
        }
        result.unwrap_or(Type::Unit)
    }

    /// Checks that `pattern` has the scrutinee's type.
    fn check_arm_pattern(&mut self, pattern: &Pattern, scrut: Type, span: Span) {
        let pat_ty = match pattern {
            Pattern::Int(_) => Type::Int,
            Pattern::Bool(_) => Type::Bool,
            Pattern::Wild => return,
        };
        if !scrut.is_error() && scrut != pat_ty {
            self.diags.emit(
                Diagnostic::error(
                    DiagCode::TypeMismatch,
                    format!("pattern of type `{pat_ty}` cannot match `{scrut}`"),
                )
                .with_primary(span, format!("expected a `{scrut}` pattern")),
            );
        }
    }

    /// Reports `code` if `actual` is not compatible with `expected`.
    fn expect(&mut self, actual: Type, expected: Type, span: Span, code: DiagCode, what: &str) {
        if !actual.compatible(expected) {
            self.diags.emit(
                Diagnostic::error(
                    code,
                    format!("{what} must be `{expected}`, found `{actual}`"),
                )
                .with_primary(span, format!("expected `{expected}`")),
            );
        }
    }
}

/// A type-mismatch error labelled "expected `expected`, found `found`".
fn mismatch(message: impl Into<String>, span: Span, expected: Type, found: Type) -> Diagnostic {
    Diagnostic::error(DiagCode::TypeMismatch, message)
        .with_primary(span, format!("expected `{expected}`, found `{found}`"))
}

/// The type of two compatible branches: the first, unless it is the error type.
fn join(a: Type, b: Type) -> Type {
    if a.is_error() { b } else { a }
}

/// A stand-in value of type `ty` for a constant whose value is unknown.
fn default_const_value(ty: Type) -> ConstValue {
    match ty {
        Type::Float => ConstValue::Float(0.0),
        Type::Bool => ConstValue::Bool(false),
        Type::Str => ConstValue::Str(String::new()),
        // `Int`, `Unit`, and `Error` all fall back to an integer zero.
        _ => ConstValue::Int(0),
    }
}

/// `op v`, if defined on constants.
fn eval_const_unary(op: UnOp, v: ConstValue) -> Option<ConstValue> {
    match (op, v) {
        (UnOp::Neg, ConstValue::Int(n)) => Some(ConstValue::Int(n.wrapping_neg())),
        (UnOp::Neg, ConstValue::Float(n)) => Some(ConstValue::Float(-n)),
        (UnOp::Not, ConstValue::Bool(b)) => Some(ConstValue::Bool(!b)),
        _ => None,
    }
}

/// `a op b`, if defined on constants.
fn eval_const_binary(op: BinOp, a: ConstValue, b: ConstValue) -> Option<ConstValue> {
    use ConstValue::{Bool, Float, Int, Str};
    match (a, b) {
        (Int(x), Int(y)) => op
            .fold_int(x, y)
            .map(Int)
            .or_else(|| op.compare(x, y).map(Bool)),
        (Float(x), Float(y)) => op
            .fold_float(x, y)
            .map(Float)
            .or_else(|| op.compare(x, y).map(Bool)),
        (Bool(x), Bool(y)) => match op {
            BinOp::Eq => Some(Bool(x == y)),
            BinOp::Ne => Some(Bool(x != y)),
            BinOp::And => Some(Bool(x && y)),
            BinOp::Or => Some(Bool(x || y)),
            _ => None,
        },
        (Str(x), Str(y)) => match op {
            BinOp::Eq => Some(Bool(x == y)),
            BinOp::Ne => Some(Bool(x != y)),
            _ => None,
        },
        _ => None,
    }
}

/// Whether every path through `block` ends in a `return`.
///
/// Conservative: conditions are not evaluated, and loops are assumed to run
/// zero times. A wrong `false` only means checking a fall-through value that
/// cannot happen.
fn block_diverges(block: &Block) -> bool {
    block.stmts.iter().any(stmt_diverges) || block.tail.as_deref().is_some_and(expr_diverges)
}

fn stmt_diverges(stmt: &Stmt) -> bool {
    match &stmt.kind {
        StmtKind::Return(_) => true,
        StmtKind::Expr(e) => expr_diverges(e),
        StmtKind::Let(l) => expr_diverges(&l.init),
        // Loops may run zero times; `break` and `continue` stay in the function.
        StmtKind::While(_)
        | StmtKind::For(_)
        | StmtKind::ForEach(_)
        | StmtKind::Break
        | StmtKind::Continue => false,
    }
}

fn expr_diverges(expr: &Expr) -> bool {
    match &expr.kind {
        ExprKind::Block(block) => block_diverges(block),
        // Only an `if` with an `else` can diverge, and only if both arms do.
        ExprKind::If(if_expr) => {
            let then_div = block_diverges(&if_expr.then_branch);
            let else_div = if_expr.else_branch.as_deref().is_some_and(expr_diverges);
            then_div && else_div
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests;
