//! HIR, the high-level IR: the AST with names resolved, types attached, and
//! sugar removed. The optimizer and every backend work on it.
//!
//! Unlike the AST it is self-contained, with no
//! [`NodeId`](crate::parser::ast::NodeId)s or side tables: every expression
//! carries its [`Type`], variables are dense [`LocalId`] slots, and calls name
//! a [`Callee`]. Lowering removes compound assignment, `match` (an `if`
//! chain), `for … in` over arrays (an index loop), constants (inlined), and
//! tuples (struct values).

pub mod lower;
pub mod print;

pub use lower::lower;
pub use print::print_hir;

#[cfg(test)]
mod tests;

use crate::sema::types::{Builtin, Type};
use crate::span::Span;

// Operators mean the same in every representation.
pub use crate::parser::ast::{BinOp, UnOp};

/// Function ids come from name resolution; [`Callee::Fn`] indexes
/// [`Hir::functions`] with them.
pub use crate::sema::resolve::FnId;

/// A local slot, indexing [`Function::locals`]: parameters first, then other
/// locals in allocation order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LocalId(pub u32);

/// A lowered program.
#[derive(Debug)]
pub struct Hir {
    /// Indexed by [`FnId`].
    pub functions: Vec<Function>,
    /// The entry point.
    pub main: FnId,
}

/// A lowered function.
#[derive(Debug)]
pub struct Function {
    pub name: String,
    /// Every local slot; the first `param_count` are the parameters.
    pub locals: Vec<LocalDecl>,
    pub param_count: usize,
    pub ret: Type,
    pub body: Block,
}

impl Function {
    /// The parameter slots.
    pub fn params(&self) -> &[LocalDecl] {
        &self.locals[..self.param_count]
    }
}

/// A local slot's name and type.
#[derive(Debug, Clone)]
pub struct LocalDecl {
    pub name: String,
    pub ty: Type,
}

/// Statements followed by an optional tail value.
#[derive(Debug, Clone)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub tail: Option<Box<Expr>>,
    /// The tail's type, or `unit`.
    pub ty: Type,
}

/// A statement.
#[derive(Debug, Clone)]
pub enum Stmt {
    /// Initialises a local.
    Let {
        local: LocalId,
        value: Expr,
    },
    /// Evaluates an expression and discards its value.
    Expr(Expr),
    Return(Option<Expr>),
    While {
        cond: Expr,
        body: Block,
    },
    /// Counts `var` through `[start, end)`. It is not desugared to `While` so
    /// that `continue` can jump to the increment. The hidden `end_var` holds
    /// `end`, which is evaluated once.
    For {
        var: LocalId,
        end_var: LocalId,
        start: Expr,
        end: Expr,
        body: Block,
    },
    Break,
    Continue,
}

/// A typed expression.
#[derive(Debug, Clone)]
pub struct Expr {
    pub kind: ExprKind,
    pub ty: Type,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum ExprKind {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
    /// Reads a local.
    Local(LocalId),
    Unary {
        op: UnOp,
        rhs: Box<Expr>,
    },
    Binary {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    /// A direct call; there are no function values.
    Call {
        callee: Callee,
        args: Vec<Expr>,
    },
    /// Stores to a local; evaluates to `unit`.
    Assign {
        local: LocalId,
        value: Box<Expr>,
    },
    /// Allocates a new array.
    ArrayLit(Vec<Expr>),
    /// Reads `base[index]`.
    Index {
        base: Box<Expr>,
        index: Box<Expr>,
    },
    /// Stores `base[index] = value`; evaluates to `unit`.
    SetIndex {
        base: Box<Expr>,
        index: Box<Expr>,
        value: Box<Expr>,
    },
    /// A struct or tuple value, fields in declaration order.
    StructLit(Vec<Expr>),
    /// Reads field `idx` of a struct or tuple.
    GetField {
        base: Box<Expr>,
        idx: u32,
    },
    /// Stores field `idx`; evaluates to `unit`.
    SetField {
        base: Box<Expr>,
        idx: u32,
        value: Box<Expr>,
    },
    If {
        cond: Box<Expr>,
        then_branch: Block,
        else_branch: Option<Box<Expr>>,
    },
    Block(Block),
}

/// A call target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Callee {
    Fn(FnId),
    Builtin(Builtin),
}

impl Expr {
    /// Creates an expression.
    pub fn new(kind: ExprKind, ty: Type, span: Span) -> Expr {
        Expr { kind, ty, span }
    }
}
