//! Regression tests: each pins a bug found while building the compiler, and
//! its comment names the original defect.

mod common;

use common::{compile_errors, stdout};

/// A mistyped `return` that is a function's whole body was reported twice: once
/// for the `return`, and again for a fall-through that cannot happen.
#[test]
fn explicit_return_reports_a_single_error() {
    assert_eq!(
        compile_errors("fn f() -> i64 { return true; } fn main() {}"),
        vec!["E0303"]
    );
}

/// An unresolved name in arithmetic also raised a spurious "invalid operands"
/// error.
#[test]
fn error_type_does_not_cascade() {
    assert_eq!(
        compile_errors("fn main() { let x = missing + 1; }"),
        vec!["E0200"]
    );
}

/// `let n = n;` with no outer `n` was accepted: the binding came into scope
/// before its initialiser was resolved.
#[test]
fn let_initialiser_cannot_reference_itself() {
    assert_eq!(compile_errors("fn main() { let n = n; }"), vec!["E0200"]);
}

/// `1 - 2 - 3` must group as `(1 - 2) - 3 = -4`, not `1 - (2 - 3) = 2`.
#[test]
fn subtraction_is_left_associative_at_runtime() {
    assert_eq!(stdout("fn main() { print_int(1 - 2 - 3); }"), "-4\n");
}

/// `x * 0` was folded to `0` even when `x` had side effects that must run.
#[test]
fn multiply_by_zero_keeps_side_effects() {
    let out = stdout(
        "fn noisy() -> i64 { print_int(42); 7 }\n\
         fn main() { let z = noisy() * 0; print_int(z); }",
    );
    // 42 from the call, then 0 from the result.
    assert_eq!(out, "42\n0\n");
}

/// Constant folding must leave `1 / 0` for the runtime to report.
#[test]
fn constant_division_by_zero_is_not_folded_away() {
    use common::{Outcome, run};
    match run("fn main() { print_int(1 / 0); }") {
        Outcome::RuntimeError(err) => assert_eq!(err.to_string(), "division by zero"),
        other => panic!("expected a runtime error, got {other:?}"),
    }
}

/// The values of an `if` without `else` and of a `while` body are discarded;
/// both must leave the operand stack balanced.
#[test]
fn statement_position_values_are_discarded() {
    let out = stdout(
        "fn main() {\n\
        \x20   let mut i = 0;\n\
        \x20   while i < 3 { i = i + 1; }\n\
        \x20   if true { print_int(i); }\n\
        \x20   print_int(99);\n\
         }",
    );
    assert_eq!(out, "3\n99\n");
}
