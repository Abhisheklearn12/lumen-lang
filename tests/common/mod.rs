//! Helpers shared by the integration tests. They compile through the public
//! [`Session`] API and run on the VM, as the CLI does.

// Each test binary uses only some of these helpers.
#![allow(dead_code)]

use lumen::backend::{VmError, execute};
use lumen::session::{PipelineOptions, Session};

/// The outcome of compiling and running a program.
#[derive(Debug)]
pub enum Outcome {
    /// Compiled and ran; holds captured stdout.
    Ok(String),
    /// Failed to compile; holds the diagnostic codes that were reported.
    CompileError(Vec<String>),
    /// Compiled but the VM raised a runtime error.
    RuntimeError(VmError),
}

/// Compiles and runs `src` with optimization enabled, returning the [`Outcome`].
pub fn run(src: &str) -> Outcome {
    run_with(src, true)
}

/// Compiles and runs `src`, choosing whether the optimizer runs.
pub fn run_with(src: &str, optimize: bool) -> Outcome {
    let mut session = Session::new("test.lm", src);
    let opts = PipelineOptions {
        optimize: lumen::opt::OptOptions {
            enabled: optimize,
            ..Default::default()
        },
        ..Default::default()
    };
    let artifacts = session.compile(opts);

    if session.diagnostics().has_errors() {
        let codes = session
            .diagnostics()
            .items()
            .iter()
            .map(|d| d.code.as_str().to_string())
            .collect();
        return Outcome::CompileError(codes);
    }

    let program = artifacts
        .program
        .expect("a well-typed program must produce bytecode");
    match execute(&program) {
        Ok(execution) => Outcome::Ok(execution.stdout),
        Err(err) => Outcome::RuntimeError(err),
    }
}

/// Convenience: asserts the program runs and returns its stdout.
pub fn stdout(src: &str) -> String {
    match run(src) {
        Outcome::Ok(out) => out,
        other => panic!("expected success, got {other:?}"),
    }
}

/// Convenience: asserts compilation fails and returns the reported codes.
pub fn compile_errors(src: &str) -> Vec<String> {
    match run(src) {
        Outcome::CompileError(codes) => codes,
        other => panic!("expected a compile error, got {other:?}"),
    }
}
