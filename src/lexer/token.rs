//! Tokens: the [lexer](super)'s output and the [parser](crate::parser)'s input.
//!
//! Literal tokens carry their parsed value, so literal syntax lives only in the
//! lexer. The payloads make [`TokenKind`] neither `Copy` nor `Eq` (`f64`), so the
//! parser matches kinds with [`TokenKind::same_kind`].

use crate::span::Span;
use std::mem;

/// The lexical category of a token, plus any literal payload.
#[derive(Clone, Debug, PartialEq)]
pub enum TokenKind {
    // ---- Literals ----
    /// Integer literal, already range-checked to fit `i64`.
    Int(i64),
    /// Floating-point literal.
    Float(f64),
    /// String literal with escapes already resolved.
    Str(String),
    /// Identifier, including type names such as `i64`.
    Ident(String),

    // ---- Keywords ----
    Fn,
    Struct,
    Const,
    Let,
    Mut,
    Return,
    If,
    Else,
    While,
    For,
    In,
    Match,
    Break,
    Continue,
    True,
    False,

    // ---- Operators & punctuation ----
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    EqEq,
    BangEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    Eq,
    PlusEq,
    MinusEq,
    StarEq,
    SlashEq,
    PercentEq,
    AmpAmp,
    PipePipe,
    Bang,
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Semi,
    Colon,
    Arrow,
    FatArrow,
    Dot,
    DotDot,

    /// End of input; always the last token.
    Eof,
}

impl TokenKind {
    /// The keyword spelled `ident`, if any. Type names such as `i64` are not
    /// keywords; they resolve during type checking.
    pub fn keyword(ident: &str) -> Option<TokenKind> {
        Some(match ident {
            "fn" => TokenKind::Fn,
            "struct" => TokenKind::Struct,
            "const" => TokenKind::Const,
            "let" => TokenKind::Let,
            "mut" => TokenKind::Mut,
            "return" => TokenKind::Return,
            "if" => TokenKind::If,
            "else" => TokenKind::Else,
            "while" => TokenKind::While,
            "for" => TokenKind::For,
            "in" => TokenKind::In,
            "match" => TokenKind::Match,
            "break" => TokenKind::Break,
            "continue" => TokenKind::Continue,
            "true" => TokenKind::True,
            "false" => TokenKind::False,
            _ => return None,
        })
    }

    /// Whether two kinds are the same variant, ignoring payloads.
    pub fn same_kind(&self, other: &TokenKind) -> bool {
        mem::discriminant(self) == mem::discriminant(other)
    }

    /// How diagnostics name this token, e.g. `` `+` `` or `integer literal`.
    pub fn describe(&self) -> String {
        use TokenKind::*;
        match self {
            Int(_) => "integer literal".to_string(),
            Float(_) => "float literal".to_string(),
            Str(_) => "string literal".to_string(),
            Ident(name) => format!("identifier `{name}`"),
            Eof => "end of file".to_string(),
            other => format!("`{}`", other.symbol()),
        }
    }

    /// The spelling of a keyword or punctuation token. Literals, identifiers,
    /// and `Eof` have none and return `"<value>"`.
    pub fn symbol(&self) -> &'static str {
        use TokenKind::*;
        match self {
            Fn => "fn",
            Struct => "struct",
            Const => "const",
            Let => "let",
            Mut => "mut",
            Return => "return",
            If => "if",
            Else => "else",
            While => "while",
            For => "for",
            In => "in",
            Match => "match",
            Break => "break",
            Continue => "continue",
            True => "true",
            False => "false",
            Plus => "+",
            Minus => "-",
            Star => "*",
            Slash => "/",
            Percent => "%",
            EqEq => "==",
            BangEq => "!=",
            Lt => "<",
            LtEq => "<=",
            Gt => ">",
            GtEq => ">=",
            Eq => "=",
            PlusEq => "+=",
            MinusEq => "-=",
            StarEq => "*=",
            SlashEq => "/=",
            PercentEq => "%=",
            AmpAmp => "&&",
            PipePipe => "||",
            Bang => "!",
            LParen => "(",
            RParen => ")",
            LBrace => "{",
            RBrace => "}",
            LBracket => "[",
            RBracket => "]",
            Comma => ",",
            Semi => ";",
            Colon => ":",
            Arrow => "->",
            FatArrow => "=>",
            Dot => ".",
            DotDot => "..",
            Int(_) | Float(_) | Str(_) | Ident(_) | Eof => "<value>",
        }
    }
}

/// A token and the span it covers.
#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

impl Token {
    pub fn new(kind: TokenKind, span: Span) -> Token {
        Token { kind, span }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyword_lookup() {
        assert_eq!(TokenKind::keyword("fn"), Some(TokenKind::Fn));
        assert_eq!(TokenKind::keyword("while"), Some(TokenKind::While));
        assert_eq!(TokenKind::keyword("i64"), None);
        assert_eq!(TokenKind::keyword("foo"), None);
    }

    #[test]
    fn same_kind_ignores_payload() {
        assert!(TokenKind::Int(1).same_kind(&TokenKind::Int(999)));
        assert!(TokenKind::Ident("a".into()).same_kind(&TokenKind::Ident("b".into())));
        assert!(!TokenKind::Int(1).same_kind(&TokenKind::Float(1.0)));
    }

    #[test]
    fn describe_is_readable() {
        assert_eq!(TokenKind::Plus.describe(), "`+`");
        assert_eq!(TokenKind::Arrow.describe(), "`->`");
        assert_eq!(TokenKind::Fn.describe(), "`fn`");
        assert_eq!(TokenKind::Int(0).describe(), "integer literal");
    }
}
