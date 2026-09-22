//! Tests for the MIR interpreter, primarily that it agrees with the stack VM.

use crate::backend::{execute, generate};
use crate::diagnostics::Diagnostics;
use crate::hir::lower;
use crate::lexer::tokenize;
use crate::mir::{build, interpret, optimize};
use crate::opt::{OptOptions, optimize as optimize_hir};
use crate::parser::parse;
use crate::sema::{check, resolve};
use crate::source::SourceFile;

/// Compiles `src` to HIR (optimized).
fn hir_of(src: &str) -> crate::hir::Hir {
    let file = SourceFile::new("t.lm", src);
    let mut diags = Diagnostics::new();
    let tokens = tokenize(&file, &mut diags);
    let ast = parse(tokens, &mut diags);
    let res = resolve(&ast, &mut diags);
    let tc = check(&ast, &res, &mut diags);
    assert!(!diags.has_errors(), "errors:\n{}", diags.render_all(&file));
    let mut hir = lower(&ast, &res, &tc);
    optimize_hir(&mut hir, OptOptions::default());
    hir
}

/// Runs `src` through the stack VM.
fn vm_output(src: &str) -> String {
    execute(&generate(&hir_of(src))).unwrap().stdout
}

/// Runs `src` through the MIR interpreter (optimized MIR).
fn mir_output(src: &str) -> String {
    let mut mir = build(&hir_of(src));
    optimize(&mut mir);
    interpret(&mir).unwrap().stdout
}

/// A battery of programs the two engines must agree on.
const PROGRAMS: &[&str] = &[
    "fn main() { print_int(1 + 2 * 3 - 4); }",
    "fn fib(n: i64) -> i64 { if n < 2 { n } else { fib(n-1) + fib(n-2) } }\n\
     fn main() { let mut i = 0; while i <= 12 { print_int(fib(i)); i = i + 1; } }",
    "fn main() { let mut s = 0; for i in 0..50 { if i % 3 == 0 { s += i; } } print_int(s); }",
    "fn main() { print_bool(true && false); print_bool(false || true); print_bool(!true); }",
    "fn main() { print_float(1.5 * 2.0 + 0.25); print_bool(3.0 < 4.0); }",
    r#"fn main() { print_str("a" + "b" + "c"); print_int(str_len("hello")); }"#,
    "fn main() { let a = [10, 20, 30]; let mut t = 0; for i in 0..len(a) { t += a[i]; } print_int(t); }",
    "fn main() { let mut a = [1, 2, 3]; a[1] = 99; print_int(a[0] + a[1] + a[2]); }",
    "struct P { x: i64, y: i64 } fn main() { let p = P { x: 3, y: 4 }; print_int(p.x + p.y); }",
    "fn main() { let t = (5, 6); print_int(t.0 * t.1); }",
    "fn main() { print_int(min(3, 9)); print_int(max(3, 9)); print_int(abs(0 - 7)); }",
    // Runtime-length arrays: the VM runs `Op::NewArray`, MIR calls the builtin.
    "fn sq(n: i64) -> i64 { n * n }\n\
     fn main() { let a = array_new_int(sq(3)); for i in 0..len(a) { a[i] = i * 2; } print_int(a[8]); \
     print_float(array_new_float(2)[1]); print_bool(array_new_bool(1)[0]); print_int(len(array_new_str(4))); }",
];

#[test]
fn mir_interpreter_agrees_with_stack_vm() {
    for src in PROGRAMS {
        assert_eq!(
            mir_output(src),
            vm_output(src),
            "engines disagree on:\n{src}"
        );
    }
}

#[test]
fn division_by_zero_is_a_runtime_error() {
    let mut mir = build(&hir_of("fn main() { let z = 0; print_int(1 / z); }"));
    optimize(&mut mir);
    let err = interpret(&mir).unwrap_err();
    assert_eq!(err.to_string(), "division by zero");
}

#[test]
fn out_of_bounds_is_a_runtime_error() {
    let mut mir = build(&hir_of("fn main() { let a = [1, 2]; print_int(a[5]); }"));
    optimize(&mut mir);
    let err = interpret(&mir).unwrap_err();
    assert!(err.to_string().contains("out of bounds"));
}

#[test]
fn negative_array_length_is_a_runtime_error() {
    let mut mir = build(&hir_of(
        "fn main() { let n = 0 - 2; print_int(len(array_new_int(n))); }",
    ));
    optimize(&mut mir);
    let err = interpret(&mir).unwrap_err();
    assert_eq!(
        err.to_string(),
        "cannot create an array of negative length -2"
    );
}
