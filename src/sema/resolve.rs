//! Name resolution: binds every name use to its definition.
//!
//! The result is a [`Resolution`] of side tables keyed by [`NodeId`]; the AST is
//! left untouched. Top-level names (functions, constants, structs) share one
//! namespace and are collected first, so items may refer to each other in any
//! order. Inside a function, parameters and `let`s live in nested scopes. A
//! `let` is visible only after its initialiser (`let x = x;` reads the outer
//! `x`) and may shadow an earlier binding; repeating a parameter is an error.

use std::collections::HashMap;

use crate::diagnostics::{Diagnostic, Diagnostics};
use crate::errors::DiagCode;
use crate::parser::ast::*;
use crate::sema::types::Builtin;
use crate::span::Span;

/// A user function, by its index in [`Resolution::functions`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FnId(pub u32);

/// A constant, by its index in [`Resolution::consts`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ConstId(pub u32);

/// A struct, by its index in [`Resolution::structs`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct StructId(pub u32);

/// What a name refers to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Res {
    /// A parameter or `let`, by its definition's [`NodeId`].
    Local(NodeId),
    Fn(FnId),
    Const(ConstId),
    Builtin(Builtin),
}

/// A local binding's name, mutability, and definition span.
#[derive(Clone, Debug)]
pub struct LocalInfo {
    pub name: String,
    pub mutable: bool,
    pub span: Span,
}

/// A dense id that indexes an [`ItemTable`].
pub trait ItemId: Copy {
    fn from_index(index: usize) -> Self;
    fn index(self) -> usize;
}

impl ItemId for FnId {
    fn from_index(index: usize) -> Self {
        FnId(index as u32)
    }
    fn index(self) -> usize {
        self.0 as usize
    }
}

impl ItemId for ConstId {
    fn from_index(index: usize) -> Self {
        ConstId(index as u32)
    }
    fn index(self) -> usize {
        self.0 as usize
    }
}

impl ItemId for StructId {
    fn from_index(index: usize) -> Self {
        StructId(index as u32)
    }
    fn index(self) -> usize {
        self.0 as usize
    }
}

/// The top-level items of one kind, by name and by dense id.
#[derive(Debug)]
pub struct ItemTable<Id> {
    by_name: HashMap<String, Id>,
    /// Id → index of the item in [`Ast::items`].
    item_index: Vec<usize>,
}

/// The user functions, by [`FnId`].
pub type FunctionTable = ItemTable<FnId>;
/// The constants, by [`ConstId`].
pub type ConstTable = ItemTable<ConstId>;
/// The structs, by [`StructId`].
pub type StructTable = ItemTable<StructId>;

impl<Id> Default for ItemTable<Id> {
    fn default() -> Self {
        ItemTable {
            by_name: HashMap::new(),
            item_index: Vec::new(),
        }
    }
}

impl<Id: ItemId> ItemTable<Id> {
    /// The id `name` binds to, if any.
    pub fn lookup(&self, name: &str) -> Option<Id> {
        self.by_name.get(name).copied()
    }

    /// The [`Ast::items`] index of `id`.
    pub fn item_index(&self, id: Id) -> usize {
        self.item_index[id.index()]
    }

    pub fn len(&self) -> usize {
        self.item_index.len()
    }

    pub fn is_empty(&self) -> bool {
        self.item_index.is_empty()
    }

    /// `(id, item index)` pairs in declaration order.
    pub fn iter(&self) -> impl Iterator<Item = (Id, usize)> + '_ {
        self.item_index
            .iter()
            .enumerate()
            .map(|(i, &idx)| (Id::from_index(i), idx))
    }

    /// Registers `name` for the item at `item_index` under the next id.
    fn insert(&mut self, name: String, item_index: usize) {
        let id = Id::from_index(self.item_index.len());
        self.item_index.push(item_index);
        self.by_name.insert(name, id);
    }

    /// Every registered name, in no particular order.
    fn names(&self) -> impl Iterator<Item = &str> {
        self.by_name.keys().map(String::as_str)
    }
}

/// Resolution's output: the side tables later phases read.
#[derive(Debug, Default)]
pub struct Resolution {
    pub uses: HashMap<NodeId, Res>,
    pub locals: HashMap<NodeId, LocalInfo>,
    pub functions: FunctionTable,
    pub consts: ConstTable,
    pub structs: StructTable,
}

impl Resolution {
    /// What the `Name` expression `id` refers to.
    pub fn use_of(&self, id: NodeId) -> Option<Res> {
        self.uses.get(&id).copied()
    }

    /// The binding defined by `id` (a `let`, parameter, or loop variable).
    pub fn local(&self, id: NodeId) -> Option<&LocalInfo> {
        self.locals.get(&id)
    }
}

/// Resolves every name in `ast`, reporting unresolved and duplicate names to
/// `diags`.
#[tracing::instrument(level = "debug", skip_all)]
pub fn resolve(ast: &Ast, diags: &mut Diagnostics) -> Resolution {
    let mut resolver = Resolver {
        res: Resolution::default(),
        scopes: Vec::new(),
        diags,
    };
    resolver.collect_globals(ast);
    for item in &ast.items {
        match &item.kind {
            ItemKind::Fn(decl) => resolver.resolve_fn(decl),
            ItemKind::Const(decl) => resolver.resolve_const(decl),
            // Field types are resolved by the type checker; a struct declaration
            // names no values.
            ItemKind::Struct(_) => {}
        }
    }
    tracing::debug!(
        functions = resolver.res.functions.len(),
        consts = resolver.res.consts.len(),
        uses = resolver.res.uses.len(),
        "resolution complete"
    );
    resolver.res
}

/// One lexical scope: local name → definition.
type Scope = HashMap<String, NodeId>;

struct Resolver<'a> {
    res: Resolution,
    scopes: Vec<Scope>,
    diags: &'a mut Diagnostics,
}

impl Resolver<'_> {
    /// Registers every top-level name. They share one namespace; a duplicate is
    /// reported and the first definition kept.
    fn collect_globals(&mut self, ast: &Ast) {
        // Name → span of its first definition.
        let mut seen: HashMap<String, Span> = HashMap::new();
        for (idx, item) in ast.items.iter().enumerate() {
            let ident = match &item.kind {
                ItemKind::Fn(decl) => &decl.name,
                ItemKind::Const(decl) => &decl.name,
                ItemKind::Struct(decl) => &decl.name,
            };
            let name = ident.name.clone();
            if let Some(&first) = seen.get(&name) {
                self.diags.emit(
                    Diagnostic::error(
                        DiagCode::DuplicateDefinition,
                        format!("`{name}` is defined multiple times"),
                    )
                    .with_primary(ident.span, "redefined here")
                    .with_label(first, "first defined here"),
                );
                continue;
            }
            seen.insert(name.clone(), ident.span);
            match &item.kind {
                ItemKind::Fn(_) => self.res.functions.insert(name, idx),
                ItemKind::Const(_) => self.res.consts.insert(name, idx),
                ItemKind::Struct(_) => self.res.structs.insert(name, idx),
            }
        }
    }

    /// Resolves a constant's initialiser, which sees only globals.
    fn resolve_const(&mut self, decl: &ConstDecl) {
        self.resolve_expr(&decl.value);
    }

    fn resolve_fn(&mut self, decl: &FnDecl) {
        self.push_scope();
        // Parameters share one scope, and may not repeat.
        let mut seen: HashMap<&str, Span> = HashMap::new();
        for param in &decl.params {
            if let Some(&first) = seen.get(param.name.name.as_str()) {
                self.diags.emit(
                    Diagnostic::error(
                        DiagCode::DuplicateParameter,
                        format!("duplicate parameter `{}`", param.name.name),
                    )
                    .with_primary(param.name.span, "redefined here")
                    .with_label(first, "first defined here"),
                );
            } else {
                seen.insert(&param.name.name, param.name.span);
            }
            self.declare(param.id, &param.name.name, false, param.name.span);
        }
        self.resolve_block(&decl.body);
        self.pop_scope();
    }

    fn resolve_block(&mut self, block: &Block) {
        self.push_scope();
        for stmt in &block.stmts {
            self.resolve_stmt(stmt);
        }
        if let Some(tail) = &block.tail {
            self.resolve_expr(tail);
        }
        self.pop_scope();
    }

    fn resolve_stmt(&mut self, stmt: &Stmt) {
        match &stmt.kind {
            StmtKind::Let(l) => {
                // The initialiser cannot see the binding it defines.
                self.resolve_expr(&l.init);
                self.declare(l.id, &l.name.name, l.mutable, l.name.span);
            }
            StmtKind::Expr(e) => self.resolve_expr(e),
            StmtKind::Return(e) => {
                if let Some(e) = e {
                    self.resolve_expr(e);
                }
            }
            StmtKind::While(w) => {
                self.resolve_expr(&w.cond);
                self.resolve_block(&w.body);
            }
            StmtKind::For(f) => {
                self.resolve_expr(&f.start);
                self.resolve_expr(&f.end);
                self.resolve_loop_body(f.id, &f.var, &f.body);
            }
            StmtKind::ForEach(f) => {
                self.resolve_expr(&f.iterable);
                self.resolve_loop_body(f.id, &f.var, &f.body);
            }
            StmtKind::Break | StmtKind::Continue => {}
        }
    }

    /// Resolves a `for` body in a new scope holding the loop variable.
    fn resolve_loop_body(&mut self, var_id: NodeId, var: &Ident, body: &Block) {
        self.push_scope();
        self.declare(var_id, &var.name, false, var.span);
        self.resolve_block(body);
        self.pop_scope();
    }

    fn resolve_expr(&mut self, expr: &Expr) {
        // Struct and field names are checked by the type checker, not here.
        match &expr.kind {
            ExprKind::Int(_) | ExprKind::Float(_) | ExprKind::Bool(_) | ExprKind::Str(_) => {}
            ExprKind::Name(name) => self.resolve_name(expr.id, name, expr.span),
            ExprKind::Unary { rhs: e, .. }
            | ExprKind::Field { base: e, .. }
            | ExprKind::TupleIndex { base: e, .. } => self.resolve_expr(e),
            ExprKind::Binary { lhs: a, rhs: b, .. }
            | ExprKind::Assign {
                target: a,
                value: b,
            }
            | ExprKind::AssignOp {
                target: a,
                value: b,
                ..
            }
            | ExprKind::Index { base: a, index: b } => {
                self.resolve_expr(a);
                self.resolve_expr(b);
            }
            ExprKind::Call { callee, args } => {
                self.resolve_expr(callee);
                for arg in args {
                    self.resolve_expr(arg);
                }
            }
            ExprKind::ArrayLit(elems) | ExprKind::TupleLit(elems) => {
                for e in elems {
                    self.resolve_expr(e);
                }
            }
            ExprKind::StructLit { fields, .. } => {
                for field in fields {
                    self.resolve_expr(&field.value);
                }
            }
            ExprKind::If(if_expr) => {
                self.resolve_expr(&if_expr.cond);
                self.resolve_block(&if_expr.then_branch);
                if let Some(else_branch) = &if_expr.else_branch {
                    self.resolve_expr(else_branch);
                }
            }
            ExprKind::Match(m) => {
                self.resolve_expr(&m.scrutinee);
                // Patterns are literals and bind nothing.
                for arm in &m.arms {
                    self.resolve_expr(&arm.body);
                }
            }
            ExprKind::Block(block) => self.resolve_block(block),
        }
    }

    /// Resolves a name use: locals first, then functions, constants, and
    /// builtins.
    fn resolve_name(&mut self, use_id: NodeId, name: &str, span: Span) {
        if let Some(def) = self.lookup_local(name) {
            self.res.uses.insert(use_id, Res::Local(def));
        } else if let Some(fn_id) = self.res.functions.lookup(name) {
            self.res.uses.insert(use_id, Res::Fn(fn_id));
        } else if let Some(const_id) = self.res.consts.lookup(name) {
            self.res.uses.insert(use_id, Res::Const(const_id));
        } else if let Some(builtin) = Builtin::from_name(name) {
            self.res.uses.insert(use_id, Res::Builtin(builtin));
        } else {
            let mut diag = Diagnostic::error(
                DiagCode::UnresolvedName,
                format!("cannot find `{name}` in this scope"),
            )
            .with_primary(span, "not found in this scope");
            if let Some(hint) = self.suggest_name(name) {
                diag = diag.with_help(format!("did you mean `{hint}`?"));
            }
            self.diags.emit(diag);
        }
    }

    /// The visible name closest to `name`, for a "did you mean" hint.
    fn suggest_name(&self, name: &str) -> Option<String> {
        let mut candidates: Vec<&str> = Vec::new();
        for scope in &self.scopes {
            candidates.extend(scope.keys().map(String::as_str));
        }
        candidates.extend(self.res.functions.names());
        candidates.extend(self.res.consts.names());
        candidates.extend(Builtin::ALL.iter().map(|b| b.name()));
        candidates.sort_unstable();
        candidates.dedup();
        crate::suggest::closest(name, candidates).map(str::to_string)
    }

    // ---- scope management ----

    fn push_scope(&mut self) {
        self.scopes.push(Scope::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    /// Binds `name` to `def` in the innermost scope and records the binding.
    fn declare(&mut self, def: NodeId, name: &str, mutable: bool, span: Span) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name.to_string(), def);
        }
        self.res.locals.insert(
            def,
            LocalInfo {
                name: name.to_string(),
                mutable,
                span,
            },
        );
    }

    /// The innermost binding of `name`.
    fn lookup_local(&self, name: &str) -> Option<NodeId> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).copied())
    }
}

#[cfg(test)]
mod tests;
