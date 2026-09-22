//! Tests for the bytecode verifier.
//!
//! Two halves: every program the compiler itself emits must verify, and a set
//! of hand-built malformed chunks must each be rejected with the right reason.

use super::*;
use crate::backend::bytecode::{Chunk, Op, Program};
use crate::backend::generate;
use crate::diagnostics::Diagnostics;
use crate::hir::lower;
use crate::lexer::tokenize;
use crate::opt::{OptOptions, optimize};
use crate::parser::parse;
use crate::sema::{check, resolve};
use crate::source::SourceFile;

/// Compiles `src` to an optimized program.
fn compile(src: &str) -> Program {
    let file = SourceFile::new("t.lm", src);
    let mut diags = Diagnostics::new();
    let tokens = tokenize(&file, &mut diags);
    let ast = parse(tokens, &mut diags);
    let res = resolve(&ast, &mut diags);
    let tc = check(&ast, &res, &mut diags);
    assert!(!diags.has_errors(), "errors:\n{}", diags.render_all(&file));
    let mut hir = lower(&ast, &res, &tc);
    optimize(&mut hir, OptOptions::default());
    generate(&hir)
}

/// A one-function program wrapping `code` with the given locals/consts.
fn program_of(code: Vec<Op>, n_params: usize, n_locals: usize) -> Program {
    Program {
        functions: vec![Chunk {
            name: "main".to_string(),
            n_locals,
            n_params,
            code,
            consts: Vec::new(),
        }],
        main: 0,
    }
}

const REAL_PROGRAMS: &[&str] = &[
    "fn main() { print_int(1 + 2 * 3); }",
    "fn fib(n: i64) -> i64 { if n < 2 { n } else { fib(n-1) + fib(n-2) } }\n\
     fn main() { print_int(fib(10)); }",
    "fn main() { let mut s = 0; for i in 0..10 { s += i; } print_int(s); }",
    "fn main() { for x in [1, 2, 3] { print_int(x); } }",
    r#"fn main() { print_str("hi" + "!"); }"#,
    "fn main() { let a = [1, 2, 3]; a[1] = 9; print_int(a[1] + len(a)); }",
    "fn main() { let n = 4; let a = array_new_int(n); a[0] = 1; print_int(len(a)); }",
];

#[test]
fn compiler_output_always_verifies() {
    for src in REAL_PROGRAMS {
        let program = compile(src);
        assert_eq!(verify(&program), Ok(()), "rejected valid program:\n{src}");
    }
}

#[test]
fn rejects_stack_underflow() {
    // `add_int` with nothing on the stack.
    let program = program_of(vec![Op::AddInt, Op::Return], 0, 0);
    let err = verify(&program).unwrap_err();
    assert!(
        matches!(err.kind, VerifyErrorKind::StackUnderflow { .. }),
        "{err:?}"
    );
    assert_eq!(err.pc, 0);
}

#[test]
fn rejects_fall_off_the_end() {
    // Pushes a value but never returns.
    let program = program_of(vec![Op::PushInt(1)], 0, 0);
    let err = verify(&program).unwrap_err();
    assert!(matches!(err.kind, VerifyErrorKind::FellOffEnd), "{err:?}");
}

#[test]
fn rejects_jump_out_of_range() {
    let program = program_of(vec![Op::Jump(99), Op::Return], 0, 0);
    let err = verify(&program).unwrap_err();
    assert!(
        matches!(err.kind, VerifyErrorKind::JumpOutOfRange { target: 99 }),
        "{err:?}"
    );
}

#[test]
fn rejects_bad_local_slot() {
    let program = program_of(vec![Op::LoadLocal(5), Op::Return], 0, 1);
    let err = verify(&program).unwrap_err();
    assert!(
        matches!(err.kind, VerifyErrorKind::BadLocal { slot: 5, .. }),
        "{err:?}"
    );
}

#[test]
fn rejects_bad_string_constant() {
    let program = program_of(vec![Op::PushStr(0), Op::Return], 0, 0);
    let err = verify(&program).unwrap_err();
    assert!(
        matches!(err.kind, VerifyErrorKind::BadConst { index: 0, .. }),
        "{err:?}"
    );
}

#[test]
fn rejects_inconsistent_merge() {
    // Offset 3 (`return`) is reached at height 1 by fall-through from offset 2
    // and at height 0 by the conditional jump, so its stack height is ambiguous.
    let clash = vec![
        Op::PushBool(true), // 0: h0 -> h1
        Op::JumpIfFalse(3), // 1: pops bool -> h0; targets 3 (h0) and 2 (h0)
        Op::PushInt(7),     // 2: h0 -> h1, falls through to 3 at h1
        Op::Return,         // 3: arrives at h1 (from 2) and h0 (from the jump)
    ];
    let program = program_of(clash, 0, 0);
    let err = verify(&program).unwrap_err();
    assert!(
        matches!(err.kind, VerifyErrorKind::InconsistentStack { .. }),
        "{err:?}"
    );
}

#[test]
fn rejects_arity_mismatch() {
    // Call function 0 (which takes 0 params) with 1 argument.
    let program = Program {
        functions: vec![Chunk {
            name: "main".to_string(),
            n_locals: 0,
            n_params: 0,
            code: vec![Op::PushInt(1), Op::Call { func: 0, argc: 1 }, Op::Return],
            consts: Vec::new(),
        }],
        main: 0,
    };
    let err = verify(&program).unwrap_err();
    assert!(
        matches!(
            err.kind,
            VerifyErrorKind::ArityMismatch {
                expected: 0,
                found: 1
            }
        ),
        "{err:?}"
    );
}

#[test]
fn rejects_bad_entry_point() {
    let program = Program {
        functions: Vec::new(),
        main: 0,
    };
    let err = verify(&program).unwrap_err();
    assert!(
        matches!(err.kind, VerifyErrorKind::BadCallTarget { .. }),
        "{err:?}"
    );
}

#[test]
fn error_message_names_function_and_offset() {
    let program = program_of(vec![Op::AddInt, Op::Return], 0, 0);
    let err = verify(&program).unwrap_err();
    let text = err.to_string();
    assert!(text.contains("`main`"), "{text}");
    assert!(text.contains("offset 0"), "{text}");
}
