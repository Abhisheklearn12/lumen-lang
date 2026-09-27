//! The parser: recursive descent, with precedence climbing for binary
//! operators.
//!
//! On a syntax error the parser reports it, skips to a stable boundary (the
//! next `fn` or `const` at top level; past the next `;`, or up to the `}`, in
//! a block) and carries on, always consuming at least one token so it cannot
//! loop. Later phases still run on the partial [`Ast`].
//!
//! # Grammar
//!
//! ```text
//! program  := item*
//! item     := "fn" IDENT "(" list(IDENT ":" type) ")" ("->" type)? block
//!           | "const" IDENT ":" type "=" expr ";"
//!           | "struct" IDENT "{" list(IDENT ":" type) "}"
//! type     := IDENT | "[" type "]" | "(" list(type) ")"
//! block    := "{" stmt* expr? "}"
//! stmt     := "let" "mut"? IDENT (":" type)? "=" expr ";"
//!           | "return" expr? ";"
//!           | "while" expr block
//!           | "for" IDENT "in" expr (".." expr)? block
//!           | "break" ";" | "continue" ";"
//!           | expr ";" | block | if
//! expr     := binary (("=" | "+=" | "-=" | "*=" | "/=" | "%=") expr)?
//! binary   := unary (BINOP unary)*
//! unary    := ("-" | "!") unary | postfix
//! postfix  := primary ("(" list(expr) ")" | "[" expr "]" | "." (IDENT | INT))*
//! primary  := INT | FLOAT | STRING | "true" | "false" | IDENT
//!           | IDENT "{" list(IDENT ":" expr) "}"
//!           | "(" list(expr) ")" | "[" list(expr) "]"
//!           | block | if | match
//! if       := "if" expr block ("else" (if | block))?
//! match    := "match" expr "{" (pattern "=>" expr ","?)* "}"
//! pattern  := "-"? INT | "true" | "false" | "_"
//! list(x)  := (x ("," x)* ","?)?
//! ```
//!
//! Binary operators are left-associative; from loosest to tightest: `||`,
//! `&&`, `== !=`, `< <= > >=`, `+ -`, `* / %`. A struct literal cannot start in
//! an `if`, `while`, or `match` head or a `for` bound, where `{` opens the body.
//! A `match` arm needs a `,` unless its body is block-like or it is the last.

pub mod ast;
pub mod print;

use ast::*;

use crate::diagnostics::{Diagnostic, Diagnostics};
use crate::errors::DiagCode;
use crate::lexer::{Token, TokenKind};
use crate::span::Span;

/// Parses `tokens`, which must end with [`TokenKind::Eof`] as
/// [`tokenize`](crate::lexer::tokenize) guarantees, reporting syntax errors to
/// `diags`.
#[tracing::instrument(level = "debug", skip_all)]
pub fn parse(tokens: Vec<Token>, diags: &mut Diagnostics) -> Ast {
    let mut parser = Parser::new(tokens, diags);
    let ast = parser.parse_program();
    tracing::debug!(item_count = ast.items.len(), "parsing complete");
    ast
}

struct Parser<'a> {
    tokens: Vec<Token>,
    pos: usize,
    ids: NodeIdGen,
    diags: &'a mut Diagnostics,
    /// Whether `Name {` may start a struct literal. Off in `if`/`while`/`match`
    /// heads and `for` bounds, where `{` opens the body instead.
    struct_ok: bool,
}

/// One element of a block.
enum BlockElem {
    Stmt(Stmt),
    /// The block's trailing value expression.
    Tail(Expr),
    /// A syntax error, already reported.
    Error,
}

impl<'a> Parser<'a> {
    fn new(tokens: Vec<Token>, diags: &'a mut Diagnostics) -> Parser<'a> {
        Parser {
            tokens,
            pos: 0,
            ids: NodeIdGen::new(),
            diags,
            struct_ok: true,
        }
    }

    /// Runs `f` with `struct_ok` set to `allowed`, then restores it.
    fn with_struct_ok<T>(&mut self, allowed: bool, f: impl FnOnce(&mut Self) -> T) -> T {
        let saved = self.struct_ok;
        self.struct_ok = allowed;
        let result = f(self);
        self.struct_ok = saved;
        result
    }

    // ---- token cursor ----

    /// The token `n` places ahead. The stream ends in `Eof`, which is returned
    /// for any position past the end.
    fn nth(&self, n: usize) -> &Token {
        &self.tokens[(self.pos + n).min(self.tokens.len() - 1)]
    }

    fn cur(&self) -> &Token {
        self.nth(0)
    }

    fn kind(&self) -> &TokenKind {
        &self.cur().kind
    }

    fn span(&self) -> Span {
        self.cur().span
    }

    fn at_eof(&self) -> bool {
        matches!(self.kind(), TokenKind::Eof)
    }

    /// Whether the current token is the same variant as `k`.
    fn at(&self, k: &TokenKind) -> bool {
        self.kind().same_kind(k)
    }

    /// Consumes and returns the current token; `Eof` is never consumed.
    fn bump(&mut self) -> Token {
        let tok = self.cur().clone();
        if !self.at_eof() {
            self.pos += 1;
        }
        tok
    }

    /// Consumes the current token if it is a `k`.
    fn eat(&mut self, k: &TokenKind) -> Option<Token> {
        if self.at(k) { Some(self.bump()) } else { None }
    }

    /// Consumes a `k`, or reports "expected `k`, found ...".
    fn expect(&mut self, k: &TokenKind) -> Option<Token> {
        let tok = self.eat(k);
        if tok.is_none() {
            self.error_expected(k.describe());
        }
        tok
    }

    /// Consumes an identifier, if the current token is one.
    fn eat_ident(&mut self) -> Option<Ident> {
        let TokenKind::Ident(name) = self.kind() else {
            return None;
        };
        let ident = Ident {
            name: name.clone(),
            span: self.span(),
        };
        self.bump();
        Some(ident)
    }

    fn fresh_id(&mut self) -> NodeId {
        self.ids.fresh()
    }

    fn mk_expr(&mut self, kind: ExprKind, span: Span) -> Expr {
        Expr {
            id: self.fresh_id(),
            kind,
            span,
        }
    }

    /// Parses `list(item)` up to, but not including, `close`.
    fn parse_comma_list<T>(
        &mut self,
        close: &TokenKind,
        mut item: impl FnMut(&mut Self) -> Option<T>,
    ) -> Option<Vec<T>> {
        let mut items = Vec::new();
        while !self.at(close) && !self.at_eof() {
            items.push(item(self)?);
            if self.eat(&TokenKind::Comma).is_none() {
                break;
            }
        }
        Some(items)
    }

    // ---- diagnostics ----

    fn error_expected(&mut self, what: impl Into<String>) {
        let what = what.into();
        let (code, headline) = if self.at_eof() {
            (
                DiagCode::UnexpectedEof,
                format!("expected {what}, found end of file"),
            )
        } else {
            (
                DiagCode::UnexpectedToken,
                format!("expected {what}, found {}", self.kind().describe()),
            )
        };
        self.diags.emit(
            Diagnostic::error(code, headline).with_primary(self.span(), format!("expected {what}")),
        );
    }

    // ---- program / items ----

    fn parse_program(&mut self) -> Ast {
        let mut items = Vec::new();
        while !self.at_eof() {
            let before = self.pos;
            match self.parse_item() {
                Some(item) => items.push(item),
                None => {
                    self.recover_to_item();
                    self.force_progress(before);
                }
            }
        }
        Ast { items }
    }

    fn parse_item(&mut self) -> Option<Item> {
        match self.kind() {
            TokenKind::Fn => self.parse_fn(),
            TokenKind::Const => self.parse_const_item(),
            TokenKind::Struct => self.parse_struct_item(),
            _ => {
                self.error_expected("an item (`fn`, `const`, or `struct`)");
                None
            }
        }
    }

    fn parse_struct_item(&mut self) -> Option<Item> {
        let start = self.span();
        self.bump(); // `struct`
        let name = self.parse_ident()?;
        self.expect(&TokenKind::LBrace)?;
        let fields = self.parse_comma_list(&TokenKind::RBrace, |p| {
            let name = p.parse_ident()?;
            p.expect(&TokenKind::Colon)?;
            let ty = p.parse_type();
            let span = name.span.to(ty.span);
            Some(FieldDef { name, ty, span })
        })?;
        let close = self.expect(&TokenKind::RBrace)?;
        let span = start.to(close.span);
        Some(Item {
            id: self.fresh_id(),
            kind: ItemKind::Struct(StructDecl {
                id: self.fresh_id(),
                name,
                fields,
            }),
            span,
        })
    }

    fn parse_const_item(&mut self) -> Option<Item> {
        let start = self.span();
        self.bump(); // `const`
        let name = self.parse_ident()?;
        self.expect(&TokenKind::Colon)?;
        let ty = self.parse_type();
        self.expect(&TokenKind::Eq)?;
        let value = self.parse_expr()?;
        let semi = self.expect(&TokenKind::Semi)?;
        let span = start.to(semi.span);
        Some(Item {
            id: self.fresh_id(),
            kind: ItemKind::Const(ConstDecl {
                id: self.fresh_id(),
                name,
                ty,
                value,
            }),
            span,
        })
    }

    fn parse_fn(&mut self) -> Option<Item> {
        let start = self.span();
        self.bump(); // `fn`
        let name = self.parse_ident()?;
        self.expect(&TokenKind::LParen)?;
        let params = self.parse_comma_list(&TokenKind::RParen, |p| {
            let name = p.parse_ident()?;
            p.expect(&TokenKind::Colon)?;
            let ty = p.parse_type();
            let span = name.span.to(ty.span);
            Some(Param {
                id: p.fresh_id(),
                name,
                ty,
                span,
            })
        })?;
        self.expect(&TokenKind::RParen)?;
        let ret = if self.eat(&TokenKind::Arrow).is_some() {
            Some(self.parse_type())
        } else {
            None
        };
        let body = self.parse_block()?;
        let span = start.to(body.span);
        Some(Item {
            id: self.fresh_id(),
            kind: ItemKind::Fn(FnDecl {
                name,
                params,
                ret,
                body,
            }),
            span,
        })
    }

    fn parse_ident(&mut self) -> Option<Ident> {
        let ident = self.eat_ident();
        if ident.is_none() {
            self.error_expected("an identifier");
        }
        ident
    }

    /// Parses a type. Never fails: a malformed type is reported and becomes
    /// [`TypeExprKind::Error`] so the enclosing construct can carry on.
    fn parse_type(&mut self) -> TypeExpr {
        if self.at(&TokenKind::LBracket) {
            let start = self.span();
            self.bump(); // `[`
            let elem = self.parse_type();
            let close = self
                .expect(&TokenKind::RBracket)
                .map(|t| t.span)
                .unwrap_or(elem.span);
            return TypeExpr {
                kind: TypeExprKind::Array(Box::new(elem)),
                span: start.to(close),
            };
        }
        if self.at(&TokenKind::LParen) {
            let start = self.span();
            self.bump(); // `(`
            // `parse_type` never fails, so neither does the list.
            let elems = self
                .parse_comma_list(&TokenKind::RParen, |p| Some(p.parse_type()))
                .unwrap_or_default();
            let close = self
                .expect(&TokenKind::RParen)
                .map(|t| t.span)
                .unwrap_or(start);
            // `(T)` is just `T`.
            return if elems.len() == 1 {
                elems.into_iter().next().unwrap()
            } else {
                TypeExpr {
                    kind: TypeExprKind::Tuple(elems),
                    span: start.to(close),
                }
            };
        }
        if let Some(ident) = self.eat_ident() {
            return TypeExpr {
                kind: TypeExprKind::Named(ident.name),
                span: ident.span,
            };
        }
        let span = self.span();
        self.diags.emit(
            Diagnostic::error(
                DiagCode::ExpectedType,
                format!("expected a type, found {}", self.kind().describe()),
            )
            .with_primary(span, "expected a type name here"),
        );
        TypeExpr {
            kind: TypeExprKind::Error,
            span,
        }
    }

    // ---- blocks & statements ----

    fn parse_block(&mut self) -> Option<Block> {
        let open = self.expect(&TokenKind::LBrace)?;
        // A `{` inside the block cannot open this block's body, so struct
        // literals are unambiguous again.
        let (stmts, tail) = self.with_struct_ok(true, Self::parse_block_body);
        let close = self
            .expect(&TokenKind::RBrace)
            .map_or_else(|| self.span(), |t| t.span);
        Some(Block {
            stmts,
            tail,
            span: open.span.to(close),
        })
    }

    /// Parses a block's statements and tail, stopping before its `}`.
    fn parse_block_body(&mut self) -> (Vec<Stmt>, Option<Box<Expr>>) {
        let mut stmts = Vec::new();
        while !self.at(&TokenKind::RBrace) && !self.at_eof() {
            let before = self.pos;
            match self.parse_block_elem() {
                BlockElem::Stmt(s) => stmts.push(s),
                BlockElem::Tail(e) => return (stmts, Some(Box::new(e))),
                BlockElem::Error => {
                    self.recover_in_block();
                    self.force_progress(before);
                }
            }
        }
        (stmts, None)
    }

    fn parse_block_elem(&mut self) -> BlockElem {
        let stmt = match self.kind() {
            TokenKind::Let => self.parse_let(),
            TokenKind::Return => self.parse_return(),
            TokenKind::While => self.parse_while(),
            TokenKind::For => self.parse_for(),
            TokenKind::Break => self.parse_loop_jump(StmtKind::Break),
            TokenKind::Continue => self.parse_loop_jump(StmtKind::Continue),
            _ => return self.parse_expr_stmt_or_tail(),
        };
        stmt.map_or(BlockElem::Error, BlockElem::Stmt)
    }

    fn parse_let(&mut self) -> Option<Stmt> {
        let start = self.span();
        self.bump(); // `let`
        let mutable = self.eat(&TokenKind::Mut).is_some();
        let name = self.parse_ident()?;
        let ty = if self.eat(&TokenKind::Colon).is_some() {
            Some(self.parse_type())
        } else {
            None
        };
        self.expect(&TokenKind::Eq)?;
        let init = self.parse_expr()?;
        let semi = self.expect(&TokenKind::Semi)?;
        let span = start.to(semi.span);
        Some(Stmt {
            kind: StmtKind::Let(LetStmt {
                id: self.fresh_id(),
                name,
                mutable,
                ty,
                init,
            }),
            span,
        })
    }

    fn parse_return(&mut self) -> Option<Stmt> {
        let start = self.span();
        self.bump(); // `return`
        let value = if self.at(&TokenKind::Semi) {
            None
        } else {
            Some(self.parse_expr()?)
        };
        let semi = self.expect(&TokenKind::Semi)?;
        Some(Stmt {
            kind: StmtKind::Return(value),
            span: start.to(semi.span),
        })
    }

    fn parse_while(&mut self) -> Option<Stmt> {
        let start = self.span();
        self.bump(); // `while`
        let cond = self.with_struct_ok(false, Self::parse_expr)?;
        let body = self.parse_block()?;
        let span = start.to(body.span);
        Some(Stmt {
            kind: StmtKind::While(WhileStmt { cond, body }),
            span,
        })
    }

    /// Parses `for v in start..end` (a range loop) or `for v in array`.
    fn parse_for(&mut self) -> Option<Stmt> {
        let start = self.span();
        self.bump(); // `for`
        let var = self.parse_ident()?;
        self.expect(&TokenKind::In)?;
        let first = self.with_struct_ok(false, Self::parse_expr)?;
        let range_end = if self.eat(&TokenKind::DotDot).is_some() {
            Some(self.with_struct_ok(false, Self::parse_expr)?)
        } else {
            None
        };
        let body = self.parse_block()?;
        let span = start.to(body.span);
        let id = self.fresh_id();
        let kind = match range_end {
            Some(end) => StmtKind::For(ForStmt {
                id,
                var,
                start: first,
                end,
                body,
            }),
            None => StmtKind::ForEach(ForEachStmt {
                id,
                var,
                iterable: first,
                body,
            }),
        };
        Some(Stmt { kind, span })
    }

    /// Parses `break;` or `continue;`, producing `kind`.
    fn parse_loop_jump(&mut self, kind: StmtKind) -> Option<Stmt> {
        let start = self.span();
        self.bump(); // `break` / `continue`
        let semi = self.expect(&TokenKind::Semi)?;
        Some(Stmt {
            kind,
            span: start.to(semi.span),
        })
    }

    /// Parses an expression in statement position: `expr;`, a block-like
    /// expression standing alone, or the block's tail if `}` follows.
    fn parse_expr_stmt_or_tail(&mut self) -> BlockElem {
        let Some(expr) = self.parse_expr() else {
            return BlockElem::Error;
        };
        if let Some(semi) = self.eat(&TokenKind::Semi) {
            let span = expr.span.to(semi.span);
            BlockElem::Stmt(Stmt {
                kind: StmtKind::Expr(expr),
                span,
            })
        } else if self.at(&TokenKind::RBrace) {
            BlockElem::Tail(expr)
        } else if expr.is_block_like() {
            let span = expr.span;
            BlockElem::Stmt(Stmt {
                kind: StmtKind::Expr(expr),
                span,
            })
        } else {
            self.error_expected("`;` or `}`");
            BlockElem::Error
        }
    }

    // ---- expressions ----

    fn parse_expr(&mut self) -> Option<Expr> {
        self.parse_assign()
    }

    /// Parses `=` and the compound assignments, which bind loosest and
    /// associate to the right.
    fn parse_assign(&mut self) -> Option<Expr> {
        let lhs = self.parse_bp(0)?;
        if self.at(&TokenKind::Eq) {
            self.bump(); // `=`
            let value = self.parse_assign()?;
            let span = lhs.span.to(value.span);
            return Some(self.mk_expr(
                ExprKind::Assign {
                    target: Box::new(lhs),
                    value: Box::new(value),
                },
                span,
            ));
        }
        if let Some(op) = compound_assign_op(self.kind()) {
            self.bump(); // `+=` etc.
            let value = self.parse_assign()?;
            let span = lhs.span.to(value.span);
            return Some(self.mk_expr(
                ExprKind::AssignOp {
                    target: Box::new(lhs),
                    op,
                    value: Box::new(value),
                },
                span,
            ));
        }
        Some(lhs)
    }

    /// Precedence climbing: parses binary operators whose left binding power
    /// is at least `min_bp`.
    fn parse_bp(&mut self, min_bp: u8) -> Option<Expr> {
        let mut lhs = self.parse_unary()?;
        while let Some(op) = peek_binop(self.kind()) {
            let (lbp, rbp) = binding_power(op);
            if lbp < min_bp {
                break;
            }
            self.bump(); // operator
            let rhs = self.parse_bp(rbp)?;
            let span = lhs.span.to(rhs.span);
            lhs = self.mk_expr(
                ExprKind::Binary {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                span,
            );
        }
        Some(lhs)
    }

    fn parse_unary(&mut self) -> Option<Expr> {
        let op = match self.kind() {
            TokenKind::Minus => UnOp::Neg,
            TokenKind::Bang => UnOp::Not,
            _ => return self.parse_postfix(),
        };
        let start = self.span();
        self.bump();
        let rhs = self.parse_unary()?;
        let span = start.to(rhs.span);
        Some(self.mk_expr(
            ExprKind::Unary {
                op,
                rhs: Box::new(rhs),
            },
            span,
        ))
    }

    /// Parses a primary followed by any chain of calls `f(...)`, indexing
    /// `a[i]`, field accesses `a.b`, and tuple indexing `a.0`.
    fn parse_postfix(&mut self) -> Option<Expr> {
        let mut expr = self.parse_primary()?;
        loop {
            if self.at(&TokenKind::LParen) {
                self.bump(); // `(`
                let args = self.parse_comma_list(&TokenKind::RParen, |p| {
                    p.with_struct_ok(true, Self::parse_expr)
                })?;
                let close = self.expect(&TokenKind::RParen)?;
                let span = expr.span.to(close.span);
                expr = self.mk_expr(
                    ExprKind::Call {
                        callee: Box::new(expr),
                        args,
                    },
                    span,
                );
            } else if self.at(&TokenKind::LBracket) {
                self.bump(); // `[`
                let index = self.with_struct_ok(true, Self::parse_expr)?;
                let close = self.expect(&TokenKind::RBracket)?;
                let span = expr.span.to(close.span);
                expr = self.mk_expr(
                    ExprKind::Index {
                        base: Box::new(expr),
                        index: Box::new(index),
                    },
                    span,
                );
            } else if self.at(&TokenKind::Dot) {
                self.bump(); // `.`
                if let TokenKind::Int(n) = self.kind() {
                    let index = (*n).max(0) as usize;
                    let index_span = self.span();
                    self.bump();
                    let span = expr.span.to(index_span);
                    expr = self.mk_expr(
                        ExprKind::TupleIndex {
                            base: Box::new(expr),
                            index,
                            index_span,
                        },
                        span,
                    );
                } else {
                    let field = self.parse_ident()?;
                    let span = expr.span.to(field.span);
                    expr = self.mk_expr(
                        ExprKind::Field {
                            base: Box::new(expr),
                            field,
                        },
                        span,
                    );
                }
            } else {
                break;
            }
        }
        Some(expr)
    }

    fn parse_primary(&mut self) -> Option<Expr> {
        let span = self.span();
        let kind = match self.kind() {
            TokenKind::Int(v) => ExprKind::Int(*v),
            TokenKind::Float(v) => ExprKind::Float(*v),
            TokenKind::Str(s) => ExprKind::Str(s.clone()),
            TokenKind::True => ExprKind::Bool(true),
            TokenKind::False => ExprKind::Bool(false),
            TokenKind::Ident(_)
                if self.struct_ok && matches!(self.nth(1).kind, TokenKind::LBrace) =>
            {
                return self.parse_struct_lit();
            }
            TokenKind::Ident(name) => ExprKind::Name(name.clone()),
            TokenKind::LParen => return self.parse_grouping(),
            TokenKind::LBrace => {
                let block = self.parse_block()?;
                let span = block.span;
                return Some(self.mk_expr(ExprKind::Block(block), span));
            }
            TokenKind::If => return self.parse_if(),
            TokenKind::Match => return self.parse_match(),
            TokenKind::LBracket => return self.parse_array_lit(),
            _ => {
                self.error_expected("an expression");
                return None;
            }
        };
        self.bump();
        Some(self.mk_expr(kind, span))
    }

    fn parse_array_lit(&mut self) -> Option<Expr> {
        let start = self.span();
        self.bump(); // `[`
        let elems = self.parse_comma_list(&TokenKind::RBracket, |p| {
            p.with_struct_ok(true, Self::parse_expr)
        })?;
        let close = self.expect(&TokenKind::RBracket)?;
        let span = start.to(close.span);
        Some(self.mk_expr(ExprKind::ArrayLit(elems), span))
    }

    /// Parses `Name { field: value, ... }`.
    fn parse_struct_lit(&mut self) -> Option<Expr> {
        let name = self.parse_ident()?;
        self.expect(&TokenKind::LBrace)?;
        let fields = self.parse_comma_list(&TokenKind::RBrace, |p| {
            let name = p.parse_ident()?;
            p.expect(&TokenKind::Colon)?;
            let value = p.with_struct_ok(true, Self::parse_expr)?;
            Some(FieldInit { name, value })
        })?;
        let close = self.expect(&TokenKind::RBrace)?;
        let span = name.span.to(close.span);
        Some(self.mk_expr(ExprKind::StructLit { name, fields }, span))
    }

    /// Parses `( ... )`: `(e)` is a grouped expression; anything else, including
    /// `()` and `(e,)`, is a tuple literal.
    fn parse_grouping(&mut self) -> Option<Expr> {
        let start = self.span();
        self.bump(); // `(`
        let mut elems = Vec::new();
        let mut saw_comma = false;
        while !self.at(&TokenKind::RParen) && !self.at_eof() {
            elems.push(self.with_struct_ok(true, Self::parse_expr)?);
            if self.eat(&TokenKind::Comma).is_some() {
                saw_comma = true;
            } else {
                break;
            }
        }
        let close = self.expect(&TokenKind::RParen)?;
        if elems.len() == 1 && !saw_comma {
            return Some(elems.into_iter().next().unwrap());
        }
        let span = start.to(close.span);
        Some(self.mk_expr(ExprKind::TupleLit(elems), span))
    }

    fn parse_if(&mut self) -> Option<Expr> {
        let start = self.span();
        self.bump(); // `if`
        let cond = self.with_struct_ok(false, Self::parse_expr)?;
        let then_branch = self.parse_block()?;
        let mut end = then_branch.span;
        let else_branch = if self.eat(&TokenKind::Else).is_some() {
            let e = if self.at(&TokenKind::If) {
                self.parse_if()?
            } else {
                let block = self.parse_block()?;
                let span = block.span;
                self.mk_expr(ExprKind::Block(block), span)
            };
            end = e.span;
            Some(Box::new(e))
        } else {
            None
        };
        let span = start.to(end);
        Some(self.mk_expr(
            ExprKind::If(IfExpr {
                cond: Box::new(cond),
                then_branch,
                else_branch,
            }),
            span,
        ))
    }

    /// Parses `match scrutinee { pat => body, ... }`.
    fn parse_match(&mut self) -> Option<Expr> {
        let start = self.span();
        self.bump(); // `match`
        let scrutinee = self.with_struct_ok(false, Self::parse_expr)?;
        self.expect(&TokenKind::LBrace)?;
        let mut arms = Vec::new();
        while !self.at(&TokenKind::RBrace) && !self.at_eof() {
            let arm_start = self.span();
            let pattern = self.parse_pattern()?;
            self.expect(&TokenKind::FatArrow)?;
            let body = self.with_struct_ok(true, Self::parse_expr)?;
            let span = arm_start.to(body.span);
            let block_like = matches!(
                body.kind,
                ExprKind::Block(_) | ExprKind::If(_) | ExprKind::Match(_)
            );
            arms.push(MatchArm {
                pattern,
                body,
                span,
            });
            if self.eat(&TokenKind::Comma).is_none() && !block_like {
                break;
            }
        }
        let close = self.expect(&TokenKind::RBrace)?;
        let span = start.to(close.span);
        Some(self.mk_expr(
            ExprKind::Match(MatchExpr {
                scrutinee: Box::new(scrutinee),
                arms,
            }),
            span,
        ))
    }

    fn parse_pattern(&mut self) -> Option<Pattern> {
        match self.kind() {
            TokenKind::Int(v) => {
                let v = *v;
                self.bump();
                Some(Pattern::Int(v))
            }
            TokenKind::Minus => {
                self.bump();
                match self.kind() {
                    TokenKind::Int(v) => {
                        let v = *v;
                        self.bump();
                        Some(Pattern::Int(-v))
                    }
                    _ => {
                        self.error_expected("an integer after `-`");
                        None
                    }
                }
            }
            TokenKind::True => {
                self.bump();
                Some(Pattern::Bool(true))
            }
            TokenKind::False => {
                self.bump();
                Some(Pattern::Bool(false))
            }
            TokenKind::Ident(name) if name == "_" => {
                self.bump();
                Some(Pattern::Wild)
            }
            _ => {
                self.error_expected("a pattern (a literal or `_`)");
                None
            }
        }
    }

    // ---- recovery ----

    /// Skips to the next `fn` or `const`, or to EOF.
    fn recover_to_item(&mut self) {
        while !self.at_eof() && !self.at(&TokenKind::Fn) && !self.at(&TokenKind::Const) {
            self.bump();
        }
    }

    /// Skips past the next `;`, or up to the block's `}`.
    fn recover_in_block(&mut self) {
        while !self.at_eof() && !self.at(&TokenKind::RBrace) {
            let was_semi = self.at(&TokenKind::Semi);
            self.bump();
            if was_semi {
                break;
            }
        }
    }

    /// Skips one token if nothing was consumed since `before`, so error
    /// recovery always makes progress.
    fn force_progress(&mut self, before: usize) {
        if self.pos == before && !self.at_eof() {
            self.bump();
        }
    }
}

/// The operator a compound-assignment token (`+=`, ...) applies.
fn compound_assign_op(kind: &TokenKind) -> Option<BinOp> {
    Some(match kind {
        TokenKind::PlusEq => BinOp::Add,
        TokenKind::MinusEq => BinOp::Sub,
        TokenKind::StarEq => BinOp::Mul,
        TokenKind::SlashEq => BinOp::Div,
        TokenKind::PercentEq => BinOp::Rem,
        _ => return None,
    })
}

/// The binary operator a token spells, if any.
fn peek_binop(kind: &TokenKind) -> Option<BinOp> {
    Some(match kind {
        TokenKind::PipePipe => BinOp::Or,
        TokenKind::AmpAmp => BinOp::And,
        TokenKind::EqEq => BinOp::Eq,
        TokenKind::BangEq => BinOp::Ne,
        TokenKind::Lt => BinOp::Lt,
        TokenKind::LtEq => BinOp::Le,
        TokenKind::Gt => BinOp::Gt,
        TokenKind::GtEq => BinOp::Ge,
        TokenKind::Plus => BinOp::Add,
        TokenKind::Minus => BinOp::Sub,
        TokenKind::Star => BinOp::Mul,
        TokenKind::Slash => BinOp::Div,
        TokenKind::Percent => BinOp::Rem,
        _ => return None,
    })
}

/// The (left, right) binding powers of `op`. Right exceeds left, which makes
/// every operator left-associative.
fn binding_power(op: BinOp) -> (u8, u8) {
    let level = op.precedence();
    (level * 2, level * 2 + 1)
}

#[cfg(test)]
mod tests;
