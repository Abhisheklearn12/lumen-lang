//! The lexer: source text to a flat [`Token`] stream.
//!
//! A hand-written, single-pass scanner with at most two characters of
//! lookahead, linear in the input.
//!
//! Lexing never stops at an error. A malformed token gets a [`Diagnostic`] and
//! a best-effort result (a stray character is skipped, a bad number becomes
//! `0`, an unterminated string keeps what it has), so one typo does not hide
//! later tokens. The stream always ends in [`TokenKind::Eof`].

mod token;

pub use token::{Token, TokenKind};

use crate::diagnostics::{Diagnostic, Diagnostics};
use crate::errors::DiagCode;
use crate::source::SourceFile;
use crate::span::Span;

/// Tokenises `file`, reporting lexical errors to `diags`. The result ends with
/// an [`TokenKind::Eof`] whose span is empty and at the end of the input.
#[tracing::instrument(level = "debug", skip_all, fields(file = file.name()))]
pub fn tokenize(file: &SourceFile, diags: &mut Diagnostics) -> Vec<Token> {
    let tokens = Lexer::new(file.text(), diags).run();
    tracing::debug!(token_count = tokens.len(), "lexing complete");
    tokens
}

struct Lexer<'a> {
    src: &'a str,
    /// Byte offset into `src`; always on a `char` boundary.
    pos: usize,
    diags: &'a mut Diagnostics,
}

impl<'a> Lexer<'a> {
    fn new(src: &'a str, diags: &'a mut Diagnostics) -> Lexer<'a> {
        Lexer { src, pos: 0, diags }
    }

    fn run(mut self) -> Vec<Token> {
        let mut tokens = Vec::new();
        loop {
            self.skip_trivia();
            let start = self.pos;
            let Some(ch) = self.peek() else {
                let eof = Span::new(self.src.len() as u32, self.src.len() as u32);
                tokens.push(Token::new(TokenKind::Eof, eof));
                return tokens;
            };
            if let Some(token) = self.scan_token(ch, start) {
                tokens.push(token);
            }
        }
    }

    // ---- cursor primitives ----

    /// The current character.
    fn peek(&self) -> Option<char> {
        self.src[self.pos..].chars().next()
    }

    /// The character after the current one.
    fn peek2(&self) -> Option<char> {
        let mut chars = self.src[self.pos..].chars();
        chars.next();
        chars.next()
    }

    /// Consumes and returns the current character.
    fn bump(&mut self) -> Option<char> {
        let ch = self.peek()?;
        self.pos += ch.len_utf8();
        Some(ch)
    }

    /// Consumes the current character if it is `expected`.
    fn eat(&mut self, expected: char) -> bool {
        if self.peek() == Some(expected) {
            self.pos += expected.len_utf8();
            true
        } else {
            false
        }
    }

    fn span_from(&self, start: usize) -> Span {
        Span::new(start as u32, self.pos as u32)
    }

    // ---- trivia ----

    /// Skips whitespace and comments.
    fn skip_trivia(&mut self) {
        loop {
            match self.peek() {
                Some(c) if c.is_whitespace() => {
                    self.bump();
                }
                Some('/') if self.peek2() == Some('/') => self.skip_line_comment(),
                Some('/') if self.peek2() == Some('*') => self.skip_block_comment(),
                _ => return,
            }
        }
    }

    fn skip_line_comment(&mut self) {
        while let Some(c) = self.peek() {
            if c == '\n' {
                break;
            }
            self.bump();
        }
    }

    /// Skips a `/* ... */` comment, which may nest, reporting it if unterminated.
    fn skip_block_comment(&mut self) {
        let start = self.pos;
        self.bump(); // '/'
        self.bump(); // '*'
        let mut depth = 1u32;
        while depth > 0 {
            match self.bump() {
                Some('/') if self.peek() == Some('*') => {
                    self.bump();
                    depth += 1;
                }
                Some('*') if self.peek() == Some('/') => {
                    self.bump();
                    depth -= 1;
                }
                Some(_) => {}
                None => {
                    self.diags.emit(
                        Diagnostic::error(
                            DiagCode::UnterminatedComment,
                            "unterminated block comment",
                        )
                        .with_primary(self.span_from(start), "comment starts here")
                        .with_help("add a closing `*/`"),
                    );
                    return;
                }
            }
        }
    }

    // ---- token dispatch ----

    /// Scans the token starting with `ch` at `start`. `None` means `ch` begins
    /// no token; it has been reported and skipped.
    fn scan_token(&mut self, ch: char, start: usize) -> Option<Token> {
        if ch.is_ascii_digit() {
            return Some(self.scan_number(start));
        }
        if is_ident_start(ch) {
            return Some(self.scan_ident(start));
        }
        if ch == '"' {
            return Some(self.scan_string(start));
        }
        self.scan_symbol(ch, start)
    }

    /// Scans an operator or punctuation, preferring the longest match (`==`
    /// over `=`).
    fn scan_symbol(&mut self, ch: char, start: usize) -> Option<Token> {
        self.bump();
        let kind = match ch {
            '+' if self.eat('=') => TokenKind::PlusEq,
            '+' => TokenKind::Plus,
            '-' if self.eat('>') => TokenKind::Arrow,
            '-' if self.eat('=') => TokenKind::MinusEq,
            '-' => TokenKind::Minus,
            '*' if self.eat('=') => TokenKind::StarEq,
            '*' => TokenKind::Star,
            '/' if self.eat('=') => TokenKind::SlashEq,
            '/' => TokenKind::Slash,
            '%' if self.eat('=') => TokenKind::PercentEq,
            '%' => TokenKind::Percent,
            '=' if self.eat('=') => TokenKind::EqEq,
            '=' if self.eat('>') => TokenKind::FatArrow,
            '=' => TokenKind::Eq,
            '!' if self.eat('=') => TokenKind::BangEq,
            '!' => TokenKind::Bang,
            '<' if self.eat('=') => TokenKind::LtEq,
            '<' => TokenKind::Lt,
            '>' if self.eat('=') => TokenKind::GtEq,
            '>' => TokenKind::Gt,
            '&' if self.eat('&') => TokenKind::AmpAmp,
            '|' if self.eat('|') => TokenKind::PipePipe,
            '.' if self.eat('.') => TokenKind::DotDot,
            '.' => TokenKind::Dot,
            '(' => TokenKind::LParen,
            ')' => TokenKind::RParen,
            '{' => TokenKind::LBrace,
            '}' => TokenKind::RBrace,
            '[' => TokenKind::LBracket,
            ']' => TokenKind::RBracket,
            ',' => TokenKind::Comma,
            ';' => TokenKind::Semi,
            ':' => TokenKind::Colon,
            _ => {
                self.diags.emit(
                    Diagnostic::error(
                        DiagCode::UnexpectedChar,
                        format!("unexpected character `{ch}`"),
                    )
                    .with_primary(self.span_from(start), "not part of any token"),
                );
                return None;
            }
        };
        Some(Token::new(kind, self.span_from(start)))
    }

    /// Scans an integer or float literal beginning with an ASCII digit.
    fn scan_number(&mut self, start: usize) -> Token {
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.bump();
        }
        // A float needs a digit after the `.`, so `1..2` is a range, not `1.`.
        let is_float = self.peek() == Some('.') && self.peek2().is_some_and(|c| c.is_ascii_digit());
        if is_float {
            self.bump(); // '.'
            while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                self.bump();
            }
        }
        let span = self.span_from(start);
        let text = &self.src[start..self.pos];
        let kind = if is_float {
            match text.parse::<f64>() {
                Ok(v) => TokenKind::Float(v),
                Err(_) => self.bad_number(text, span),
            }
        } else {
            match text.parse::<i64>() {
                Ok(v) => TokenKind::Int(v),
                Err(_) => self.bad_number(text, span),
            }
        };
        Token::new(kind, span)
    }

    /// Reports a literal that does not fit its type and substitutes `0`.
    fn bad_number(&mut self, text: &str, span: Span) -> TokenKind {
        self.diags.emit(
            Diagnostic::error(
                DiagCode::InvalidNumber,
                format!("invalid numeric literal `{text}`"),
            )
            .with_primary(span, "does not fit in a 64-bit number")
            .with_note("integer literals must be in the range of a signed 64-bit integer"),
        );
        TokenKind::Int(0)
    }

    /// Scans an identifier or keyword.
    fn scan_ident(&mut self, start: usize) -> Token {
        while self.peek().is_some_and(is_ident_continue) {
            self.bump();
        }
        let span = self.span_from(start);
        let text = &self.src[start..self.pos];
        let kind = TokenKind::keyword(text).unwrap_or_else(|| TokenKind::Ident(text.to_string()));
        Token::new(kind, span)
    }

    /// Scans a double-quoted string, resolving escapes. An unterminated string
    /// keeps what was read; an unknown escape `\x` keeps `x`. Both are
    /// reported.
    fn scan_string(&mut self, start: usize) -> Token {
        self.bump(); // opening quote
        let mut value = String::new();
        loop {
            match self.bump() {
                Some('"') => break,
                Some('\\') => self.scan_escape(&mut value),
                Some(c) => value.push(c),
                None => {
                    self.diags.emit(
                        Diagnostic::error(
                            DiagCode::UnterminatedString,
                            "unterminated string literal",
                        )
                        .with_primary(self.span_from(start), "string starts here")
                        .with_help("add a closing `\"`"),
                    );
                    break;
                }
            }
        }
        Token::new(TokenKind::Str(value), self.span_from(start))
    }

    /// Scans the character after a backslash in a string.
    fn scan_escape(&mut self, value: &mut String) {
        let esc_start = self.pos - 1;
        match self.bump() {
            Some('n') => value.push('\n'),
            Some('t') => value.push('\t'),
            Some('r') => value.push('\r'),
            Some('0') => value.push('\0'),
            Some('\\') => value.push('\\'),
            Some('"') => value.push('"'),
            Some(other) => {
                value.push(other);
                self.diags.emit(
                    Diagnostic::error(
                        DiagCode::InvalidEscape,
                        format!("unknown escape `\\{other}`"),
                    )
                    .with_primary(self.span_from(esc_start), "not a valid escape")
                    .with_help("valid escapes are \\n \\t \\r \\0 \\\\ \\\""),
                );
            }
            // A backslash at EOF: `scan_string` reports the missing quote.
            None => {}
        }
    }
}

fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

fn is_ident_continue(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

#[cfg(test)]
mod tests;
