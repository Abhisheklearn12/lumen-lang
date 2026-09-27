//! # Lumen
//!
//! A small statically typed language and its compiler, built as a pipeline of
//! phases that each read the previous phase's output and return a new one:
//!
//! ```text
//! source ─▶ lexer ─▶ parser ─▶ AST
//!                                │  name resolution
//!                                ▼  type checking
//!                               HIR (lowering)
//!                                │  optimizer
//!                                ▼
//!                            bytecode ─▶ VM
//! ```
//!
//! Each phase has its own representation ([`lexer::Token`], [`parser::ast`],
//! [`hir`], [`backend::bytecode`]); [`span`], [`source`], [`diagnostics`], and
//! [`errors`] are shared by all. Off the main path, [`mir`] and
//! [`backend::c`] also start from HIR.
//!
//! [`Session`] runs the pipeline and is where most callers start.

// Lints beyond the defaults. CI also fails on any warning.
#![deny(rust_2018_idioms)]
#![warn(missing_debug_implementations)]

pub mod backend;
pub mod diagnostics;
pub mod errors;
pub mod explain;
pub mod format;
pub mod hir;
pub mod lexer;
pub mod mir;
pub mod opt;
pub mod parser;
pub mod sema;
pub mod session;
pub mod source;
pub mod span;
pub mod suggest;

pub use session::{Artifacts, PipelineOptions, Session, Stage};

#[cfg(test)]
mod foundation_tests {
    //! Spans, source files, and diagnostics working together.

    use crate::diagnostics::{Diagnostic, Diagnostics};
    use crate::errors::DiagCode;
    use crate::source::SourceFile;
    use crate::span::Span;

    #[test]
    fn diagnostic_points_at_correct_location() {
        let src = "fn main() {\n    let x = bogus;\n}\n";
        let file = SourceFile::new("main.lm", src);
        // Span of `bogus` on line 2.
        let start = src.find("bogus").unwrap() as u32;
        let span = Span::new(start, start + 5);
        let loc = file.location(span.lo);
        assert_eq!(loc.line, 2);

        let mut diags = Diagnostics::new();
        diags.emit(
            Diagnostic::error(DiagCode::UnresolvedName, "cannot find value `bogus`")
                .with_primary(span, "not found in this scope"),
        );
        let rendered = diags.render_all(&file);
        assert!(rendered.contains("E0200"));
        assert!(rendered.contains("bogus"));
    }
}
