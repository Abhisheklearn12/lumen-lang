//! The abstract syntax tree built by the [parser](super).
//!
//! The AST mirrors the source and is never mutated after parsing; desugaring
//! happens in [HIR lowering](crate::hir). Every node has a [`Span`] for
//! diagnostics.
//!
//! Nodes that later phases annotate carry a [`NodeId`]. Name resolution and type
//! checking record their results in side tables keyed by it
//! ([`Resolution`](crate::sema::Resolution), [`Typeck`](crate::sema::Typeck)).

use std::fmt;

use crate::span::Span;

/// A dense id for an AST node: the key of the side tables that resolution and
/// type checking build.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u32);

/// Hands out [`NodeId`]s in increasing order.
#[derive(Debug, Default)]
pub struct NodeIdGen {
    next: u32,
}

impl NodeIdGen {
    pub fn new() -> NodeIdGen {
        NodeIdGen::default()
    }

    /// An id never returned before.
    pub fn fresh(&mut self) -> NodeId {
        let id = NodeId(self.next);
        self.next += 1;
        id
    }

    /// How many ids have been handed out; every id is below this.
    pub fn count(&self) -> usize {
        self.next as usize
    }
}

/// An identifier and where it was written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ident {
    pub name: String,
    pub span: Span,
}

/// A parsed program: its top-level items in source order.
#[derive(Debug, Clone)]
pub struct Ast {
    pub items: Vec<Item>,
}

/// A top-level item.
#[derive(Debug, Clone)]
pub struct Item {
    pub id: NodeId,
    pub kind: ItemKind,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum ItemKind {
    Fn(FnDecl),
    Const(ConstDecl),
    Struct(StructDecl),
}

/// `struct Name { field: T, ... }`.
#[derive(Debug, Clone)]
pub struct StructDecl {
    pub id: NodeId,
    pub name: Ident,
    pub fields: Vec<FieldDef>,
}

/// A struct field declaration.
#[derive(Debug, Clone)]
pub struct FieldDef {
    pub name: Ident,
    pub ty: TypeExpr,
    pub span: Span,
}

/// `const NAME: T = value;`. The value must be a compile-time constant;
/// lowering inlines it at each use.
#[derive(Debug, Clone)]
pub struct ConstDecl {
    pub id: NodeId,
    pub name: Ident,
    pub ty: TypeExpr,
    pub value: Expr,
}

/// `fn name(params) -> ret { body }`.
#[derive(Debug, Clone)]
pub struct FnDecl {
    pub name: Ident,
    pub params: Vec<Param>,
    /// The declared return type; `None` means `unit`.
    pub ret: Option<TypeExpr>,
    pub body: Block,
}

/// A function parameter.
#[derive(Debug, Clone)]
pub struct Param {
    pub id: NodeId,
    pub name: Ident,
    pub ty: TypeExpr,
    pub span: Span,
}

/// A type as written, e.g. `[i64]`. Type checking resolves it to a
/// [`Type`](crate::sema::Type).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeExpr {
    pub kind: TypeExprKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeExprKind {
    /// A primitive or struct name, such as `i64`.
    Named(String),
    /// `[T]`.
    Array(Box<TypeExpr>),
    /// `(T1, T2, ...)`. Never one element: `(T)` and `(T,)` parse as `T`.
    Tuple(Vec<TypeExpr>),
    /// Stands in for a malformed type so parsing can continue. Type checking
    /// treats it as an error without reporting it again.
    Error,
}

/// `{ stmts; tail }`. The tail is the block's value (`unit` if absent).
#[derive(Debug, Clone)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub tail: Option<Box<Expr>>,
    pub span: Span,
}

/// A statement.
#[derive(Debug, Clone)]
pub struct Stmt {
    pub kind: StmtKind,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum StmtKind {
    /// `let [mut] name [: ty] = init;`
    Let(LetStmt),
    /// `expr;`, or a block-like expression without the `;`.
    Expr(Expr),
    /// `return [expr];`
    Return(Option<Expr>),
    /// `while cond { body }`
    While(WhileStmt),
    /// `for var in start..end { body }`
    For(ForStmt),
    /// `for var in array { body }`
    ForEach(ForEachStmt),
    /// `break;`
    Break,
    /// `continue;`
    Continue,
}

#[derive(Debug, Clone)]
pub struct LetStmt {
    pub id: NodeId,
    pub name: Ident,
    pub mutable: bool,
    pub ty: Option<TypeExpr>,
    pub init: Expr,
}

#[derive(Debug, Clone)]
pub struct WhileStmt {
    pub cond: Expr,
    pub body: Block,
}

/// `for var in start..end { body }`: `var` is an `i64` counting through
/// `[start, end)`.
#[derive(Debug, Clone)]
pub struct ForStmt {
    /// The loop variable's definition id.
    pub id: NodeId,
    pub var: Ident,
    pub start: Expr,
    pub end: Expr,
    pub body: Block,
}

/// `for var in array { body }`: `var` takes each element in turn.
#[derive(Debug, Clone)]
pub struct ForEachStmt {
    /// The loop variable's definition id.
    pub id: NodeId,
    pub var: Ident,
    pub iterable: Expr,
    pub body: Block,
}

/// An expression.
#[derive(Debug, Clone)]
pub struct Expr {
    pub id: NodeId,
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum ExprKind {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
    /// A variable, parameter, constant, or function name.
    Name(String),
    Unary {
        op: UnOp,
        rhs: Box<Expr>,
    },
    Binary {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
    },
    /// `target = value`. Type checking decides whether `target` is assignable.
    Assign {
        target: Box<Expr>,
        value: Box<Expr>,
    },
    /// `target op= value`, e.g. `x += 1`.
    AssignOp {
        target: Box<Expr>,
        op: BinOp,
        value: Box<Expr>,
    },
    /// `[e0, e1, ...]`
    ArrayLit(Vec<Expr>),
    /// `base[index]`
    Index {
        base: Box<Expr>,
        index: Box<Expr>,
    },
    /// `Name { field: value, ... }`
    StructLit {
        name: Ident,
        fields: Vec<FieldInit>,
    },
    /// `base.field`
    Field {
        base: Box<Expr>,
        field: Ident,
    },
    /// `(e0, e1, ...)`, including `()` and `(e,)`; plain `(e)` is grouping.
    TupleLit(Vec<Expr>),
    /// `base.0`, `base.1`, ...
    TupleIndex {
        base: Box<Expr>,
        index: usize,
        /// The index's span, for diagnostics.
        index_span: Span,
    },
    If(IfExpr),
    Match(MatchExpr),
    Block(Block),
}

/// `match scrutinee { pat => body, ... }` over an `i64` or `bool`. Lowering
/// turns it into an `if`/`else` chain, so HIR has no match node.
#[derive(Debug, Clone)]
pub struct MatchExpr {
    pub scrutinee: Box<Expr>,
    pub arms: Vec<MatchArm>,
}

/// One `pattern => body` arm of a [`MatchExpr`].
#[derive(Debug, Clone)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub body: Expr,
    pub span: Span,
}

/// A `match` pattern: a literal or `_`.
#[derive(Debug, Clone)]
pub enum Pattern {
    Int(i64),
    Bool(bool),
    /// `_`, matching anything.
    Wild,
}

impl fmt::Display for Pattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Pattern::Int(v) => write!(f, "{v}"),
            Pattern::Bool(b) => write!(f, "{b}"),
            Pattern::Wild => f.write_str("_"),
        }
    }
}

/// `field: value` in a struct literal.
#[derive(Debug, Clone)]
pub struct FieldInit {
    pub name: Ident,
    pub value: Expr,
}

/// `if cond { then } [else ...]`. Its value is the taken branch's, or `unit`
/// without an `else`.
#[derive(Debug, Clone)]
pub struct IfExpr {
    pub cond: Box<Expr>,
    pub then_branch: Block,
    /// A `Block` or `If` expression.
    pub else_branch: Option<Box<Expr>>,
}

/// Unary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    /// `-`
    Neg,
    /// `!`
    Not,
}

impl UnOp {
    pub fn symbol(self) -> &'static str {
        match self {
            UnOp::Neg => "-",
            UnOp::Not => "!",
        }
    }
}

/// Binary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

impl BinOp {
    pub fn symbol(self) -> &'static str {
        use BinOp::*;
        match self {
            Add => "+",
            Sub => "-",
            Mul => "*",
            Div => "/",
            Rem => "%",
            Eq => "==",
            Ne => "!=",
            Lt => "<",
            Le => "<=",
            Gt => ">",
            Ge => ">=",
            And => "&&",
            Or => "||",
        }
    }

    /// Binding strength, from 1 (`||`) to 6 (`*`, `/`, `%`). All binary
    /// operators are left-associative.
    pub fn precedence(self) -> u8 {
        use BinOp::*;
        match self {
            Or => 1,
            And => 2,
            Eq | Ne => 3,
            Lt | Le | Gt | Ge => 4,
            Add | Sub => 5,
            Mul | Div | Rem => 6,
        }
    }

    /// Whether this is `==`, `!=`, `<`, `<=`, `>`, or `>=`, which yield `bool`.
    pub fn is_comparison(self) -> bool {
        use BinOp::*;
        matches!(self, Eq | Ne | Lt | Le | Gt | Ge)
    }

    /// Whether this is the short-circuiting `&&` or `||`.
    pub fn is_logical(self) -> bool {
        matches!(self, BinOp::And | BinOp::Or)
    }

    /// `a op b` for an arithmetic operator on integers, wrapping on overflow
    /// like the VM. `None` for other operators, and for a division or
    /// remainder that traps at runtime (by zero, or `i64::MIN / -1`).
    pub fn fold_int(self, a: i64, b: i64) -> Option<i64> {
        match self {
            BinOp::Add => Some(a.wrapping_add(b)),
            BinOp::Sub => Some(a.wrapping_sub(b)),
            BinOp::Mul => Some(a.wrapping_mul(b)),
            BinOp::Div => a.checked_div(b),
            BinOp::Rem => a.checked_rem(b),
            _ => None,
        }
    }

    /// `a op b` for an arithmetic operator on floats; `None` for other
    /// operators.
    pub fn fold_float(self, a: f64, b: f64) -> Option<f64> {
        match self {
            BinOp::Add => Some(a + b),
            BinOp::Sub => Some(a - b),
            BinOp::Mul => Some(a * b),
            BinOp::Div => Some(a / b),
            BinOp::Rem => Some(a % b),
            _ => None,
        }
    }

    /// `a op b` for a comparison operator; `None` for other operators.
    pub fn compare<T: PartialOrd>(self, a: T, b: T) -> Option<bool> {
        match self {
            BinOp::Eq => Some(a == b),
            BinOp::Ne => Some(a != b),
            BinOp::Lt => Some(a < b),
            BinOp::Le => Some(a <= b),
            BinOp::Gt => Some(a > b),
            BinOp::Ge => Some(a >= b),
            _ => None,
        }
    }
}

impl Expr {
    /// Whether this is `{ ... }` or `if ...`, which may end a statement without
    /// a `;`.
    pub fn is_block_like(&self) -> bool {
        matches!(self.kind, ExprKind::Block(_) | ExprKind::If(_))
    }
}
