//! The compilation session: the one place the phases are wired together.
//!
//! ```text
//! lex → parse → resolve → typeck → lower → optimize → codegen → peephole
//! ```
//!
//! A [`Session`] owns the source and the [`Diagnostics`] sink, times each
//! phase, and returns what the phases produced as [`Artifacts`]. Resolution and
//! type checking run even after syntax errors, so one run reports everything;
//! lowering and the rest run only on an error-free program.

use std::time::{Duration, Instant};

use crate::backend::{Program, generate};
use crate::diagnostics::Diagnostics;
use crate::hir::{Hir, lower};
use crate::lexer::{Token, tokenize};
use crate::opt::{OptOptions, OptStats, optimize};
use crate::parser::ast::Ast;
use crate::parser::parse;
use crate::sema::resolve::Resolution;
use crate::sema::typeck::Typeck;
use crate::sema::{check, resolve};
use crate::source::SourceFile;

/// How far to run the pipeline; each `lumenc dump` form names one.
///
/// [`Session::compile`] stops after optimizing for `Mir`, `Cfg`, and `C`; the
/// CLI builds those from the optimized HIR.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stage {
    Tokens,
    Ast,
    /// HIR before optimization.
    Hir,
    /// HIR after optimization.
    OptimizedHir,
    /// Optimized MIR.
    Mir,
    /// The MIR control-flow graph, as Graphviz DOT.
    Cfg,
    /// C source.
    C,
    /// Bytecode: the full pipeline.
    Bytecode,
    /// The verifier's verdict on the bytecode.
    Verify,
}

/// What to compile.
#[derive(Debug, Clone, Copy)]
pub struct PipelineOptions {
    /// The last stage to run.
    pub stop_after: Stage,
    pub optimize: OptOptions,
}

impl Default for PipelineOptions {
    fn default() -> PipelineOptions {
        PipelineOptions {
            stop_after: Stage::Bytecode,
            optimize: OptOptions::default(),
        }
    }
}

/// What a run produced. `tokens` is kept only when stopping after lexing; each
/// other field is set if its phase ran.
#[derive(Debug, Default)]
pub struct Artifacts {
    pub tokens: Option<Vec<Token>>,
    pub ast: Option<Ast>,
    pub resolution: Option<Resolution>,
    pub typeck: Option<Typeck>,
    pub hir: Option<Hir>,
    pub opt_stats: Option<OptStats>,
    pub program: Option<Program>,
}

/// How long each phase took, in the order the phases ran.
#[derive(Debug, Default, Clone)]
pub struct Timings {
    entries: Vec<(&'static str, Duration)>,
}

impl Timings {
    /// `(phase, duration)` pairs.
    pub fn entries(&self) -> &[(&'static str, Duration)] {
        &self.entries
    }

    /// The sum of all durations.
    pub fn total(&self) -> Duration {
        self.entries.iter().map(|(_, d)| *d).sum()
    }
}

/// One compilation: the source, its diagnostics, and phase timings.
#[derive(Debug)]
pub struct Session {
    file: SourceFile,
    diagnostics: Diagnostics,
    timings: Timings,
}

impl Session {
    /// A session for source `src`, called `name` in diagnostics.
    pub fn new(name: impl Into<String>, src: impl Into<String>) -> Session {
        Session {
            file: SourceFile::new(name, src),
            diagnostics: Diagnostics::new(),
            timings: Timings::default(),
        }
    }

    /// The source file being compiled.
    pub fn file(&self) -> &SourceFile {
        &self.file
    }

    /// Every diagnostic reported so far.
    pub fn diagnostics(&self) -> &Diagnostics {
        &self.diagnostics
    }

    /// Every phase timing recorded so far, across all compiles.
    pub fn timings(&self) -> &Timings {
        &self.timings
    }

    /// Renders every diagnostic, in source order.
    pub fn render_diagnostics(&self) -> String {
        self.diagnostics.render_all(&self.file)
    }

    /// Runs the pipeline up to `options.stop_after`.
    #[tracing::instrument(level = "info", skip_all, fields(file = self.file.name()))]
    pub fn compile(&mut self, options: PipelineOptions) -> Artifacts {
        let mut artifacts = Artifacts::default();

        let tokens = self.timed("lex", |s| tokenize(&s.file, &mut s.diagnostics));
        if options.stop_after == Stage::Tokens {
            artifacts.tokens = Some(tokens);
            return artifacts;
        }

        let ast = self.timed("parse", |s| parse(tokens, &mut s.diagnostics));
        if options.stop_after == Stage::Ast {
            artifacts.ast = Some(ast);
            return artifacts;
        }

        let resolution = self.timed("resolve", |s| resolve(&ast, &mut s.diagnostics));
        let typeck = self.timed("typeck", |s| check(&ast, &resolution, &mut s.diagnostics));

        // Lowering assumes a well-typed program.
        let hir = if self.diagnostics.has_errors() {
            None
        } else {
            Some(self.timed("lower", |_| lower(&ast, &resolution, &typeck)))
        };
        artifacts.ast = Some(ast);
        artifacts.resolution = Some(resolution);
        artifacts.typeck = Some(typeck);
        let Some(mut hir) = hir else {
            return artifacts;
        };
        if options.stop_after == Stage::Hir {
            artifacts.hir = Some(hir);
            return artifacts;
        }

        let stats = self.timed("optimize", |_| optimize(&mut hir, options.optimize));
        artifacts.opt_stats = Some(stats);
        if matches!(
            options.stop_after,
            Stage::OptimizedHir | Stage::Mir | Stage::Cfg | Stage::C
        ) {
            artifacts.hir = Some(hir);
            return artifacts;
        }

        let mut program = self.timed("codegen", |_| generate(&hir));
        if options.optimize.enabled {
            self.timed("peephole", |_| {
                crate::backend::peephole::optimize(&mut program);
            });
        }
        artifacts.hir = Some(hir);
        artifacts.program = Some(program);
        artifacts
    }

    /// Runs `phase`, recording its duration under `name`.
    fn timed<T>(&mut self, name: &'static str, phase: impl FnOnce(&mut Session) -> T) -> T {
        let start = Instant::now();
        let result = phase(self);
        let elapsed = start.elapsed();
        self.timings.entries.push((name, elapsed));
        tracing::debug!(
            phase = name,
            micros = elapsed.as_micros() as u64,
            "phase complete"
        );
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiles_a_valid_program_to_bytecode() {
        let mut session = Session::new("ok.lm", "fn main() { print_int(1 + 1); }");
        let artifacts = session.compile(PipelineOptions::default());
        assert!(!session.diagnostics().has_errors());
        assert!(artifacts.program.is_some());
        // All eight phases recorded a timing (the final one is the bytecode
        // peephole pass, which runs because optimization is on by default).
        assert_eq!(session.timings().entries().len(), 8);
    }

    #[test]
    fn stops_before_codegen_on_type_error() {
        let mut session = Session::new("bad.lm", "fn main() { let x: i64 = true; }");
        let artifacts = session.compile(PipelineOptions::default());
        assert!(session.diagnostics().has_errors());
        assert!(artifacts.program.is_none());
        assert!(artifacts.hir.is_none());
        // The front-end artifacts are still available.
        assert!(artifacts.typeck.is_some());
    }

    #[test]
    fn dump_stages_stop_early() {
        let mut session = Session::new("t.lm", "fn main() {}");
        let opts = PipelineOptions {
            stop_after: Stage::Tokens,
            ..Default::default()
        };
        let artifacts = session.compile(opts);
        assert!(artifacts.tokens.is_some());
        assert!(artifacts.ast.is_none());
    }

    #[test]
    fn records_optimization_stats() {
        let mut session = Session::new("o.lm", "fn main() { print_int(2 * 3); }");
        let artifacts = session.compile(PipelineOptions::default());
        let stats = artifacts.opt_stats.expect("optimizer ran");
        assert!(stats.folded >= 1);
    }
}
