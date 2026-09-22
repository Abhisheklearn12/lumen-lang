//! Integration tests for arrays: literals, runtime allocation, indexing,
//! mutation, `len`, and bounds checking.

mod common;

use common::{Outcome, compile_errors, run, run_with, stdout};
use lumen::backend::VmError;
use lumen::backend::vm::MAX_ARRAY_LEN;

#[test]
fn literal_indexing() {
    assert_eq!(
        stdout("fn main() { let a = [10, 20, 30]; print_int(a[1]); }"),
        "20\n"
    );
}

#[test]
fn length() {
    assert_eq!(
        stdout("fn main() { let a = [1, 2, 3, 4]; print_int(len(a)); }"),
        "4\n"
    );
}

#[test]
fn element_assignment() {
    let out =
        stdout("fn main() { let mut a = [1, 2, 3]; a[0] = 99; print_int(a[0]); print_int(a[2]); }");
    assert_eq!(out, "99\n3\n");
}

#[test]
fn iterate_and_sum() {
    let out = stdout(
        "fn main() {\n\
        \x20   let a = [5, 10, 15];\n\
        \x20   let mut total = 0;\n\
        \x20   for i in 0..len(a) { total += a[i]; }\n\
        \x20   print_int(total);\n\
         }",
    );
    assert_eq!(out, "30\n");
}

#[test]
fn arrays_pass_by_reference() {
    // Mutating an array inside a function is visible to the caller.
    let out = stdout(
        "fn set_first(a: [i64]) { a[0] = 100; }\n\
         fn main() { let mut a = [1, 2]; set_first(a); print_int(a[0]); }",
    );
    assert_eq!(out, "100\n");
}

#[test]
fn string_arrays() {
    let out = stdout(r#"fn main() { let s = ["a", "b", "c"]; print_str(s[2]); }"#);
    assert_eq!(out, "c\n");
}

#[test]
fn array_equality() {
    let out = stdout(
        "fn main() {\n\
        \x20   print_bool([1, 2, 3] == [1, 2, 3]);\n\
        \x20   print_bool([1, 2] == [1, 3]);\n\
         }",
    );
    assert_eq!(out, "true\nfalse\n");
}

#[test]
fn nested_array_type_annotation() {
    assert_eq!(
        stdout("fn main() { let a: [i64] = [7]; print_int(a[0]); }"),
        "7\n"
    );
}

#[test]
fn out_of_bounds_is_a_runtime_error() {
    match run("fn main() { let a = [1, 2]; print_int(a[2]); }") {
        Outcome::RuntimeError(e) => {
            assert!(e.to_string().contains("out of bounds"), "got {e}")
        }
        other => panic!("expected a runtime error, got {other:?}"),
    }
}

#[test]
fn negative_index_is_a_runtime_error() {
    match run("fn main() { let a = [1, 2]; let i = 0 - 1; print_int(a[i]); }") {
        Outcome::RuntimeError(e) => assert!(e.to_string().contains("out of bounds")),
        other => panic!("expected a runtime error, got {other:?}"),
    }
}

// ---- runtime-length allocation ----

#[test]
fn array_new_fills_with_zero_values() {
    // `n` comes from a call so the optimizer can't treat the length as a constant.
    let out = stdout(
        r#"fn three() -> i64 { 3 }
         fn main() {
             let n = three();
             let i = array_new_int(n);
             let f = array_new_float(n);
             let b = array_new_bool(n);
             let s = array_new_str(n);
             print_int(len(i) + len(f) + len(b) + len(s));
             print_int(i[2]);
             print_float(f[2]);
             print_bool(b[2]);
             print_bool(s[2] == "");
         }"#,
    );
    assert_eq!(out, "12\n0\n0\nfalse\ntrue\n");
}

#[test]
fn array_new_of_zero_is_empty() {
    assert_eq!(
        stdout("fn main() { print_int(len(array_new_int(0))); }"),
        "0\n"
    );
}

#[test]
fn sieve_sized_at_runtime() {
    // The motivating case from #3: storage sized by a value computed at runtime.
    let src = "fn primes_below(n: i64) -> i64 {\n\
        \x20   let composite = array_new_bool(n);\n\
        \x20   let mut count = 0;\n\
        \x20   for i in 2..n {\n\
        \x20       if !composite[i] {\n\
        \x20           count += 1;\n\
        \x20           let mut j = i * i;\n\
        \x20           while j < n { composite[j] = true; j += i; }\n\
        \x20       }\n\
        \x20   }\n\
        \x20   count\n\
         }\n\
         fn main() { print_int(primes_below(100)); }";
    for optimize in [true, false] {
        match run_with(src, optimize) {
            Outcome::Ok(out) => assert_eq!(out, "25\n", "optimize={optimize}"),
            other => panic!("expected success, got {other:?}"),
        }
    }
}

#[test]
fn array_new_negative_length_is_a_runtime_error() {
    match run("fn main() { let n = 0 - 1; let a = array_new_int(n); print_int(len(a)); }") {
        Outcome::RuntimeError(e) => assert_eq!(e, VmError::NegativeArrayLength(-1)),
        other => panic!("expected a runtime error, got {other:?}"),
    }
}

#[test]
fn array_new_above_the_ceiling_is_a_runtime_error() {
    let len = MAX_ARRAY_LEN + 1;
    let src = format!("fn main() {{ let a = array_new_str({len}); print_int(len(a)); }}");
    match run(&src) {
        Outcome::RuntimeError(e) => assert_eq!(
            e,
            VmError::ArrayTooLong {
                len,
                max: MAX_ARRAY_LEN
            }
        ),
        other => panic!("expected a runtime error, got {other:?}"),
    }
}

// ---- compile-time checks ----

#[test]
fn indexing_non_array_is_rejected() {
    assert_eq!(
        compile_errors("fn main() { let x = 1; print_int(x[0]); }"),
        vec!["E0315"]
    );
}

#[test]
fn non_integer_index_is_rejected() {
    assert_eq!(
        compile_errors("fn main() { let a = [1]; print_int(a[true]); }"),
        vec!["E0300"],
    );
}

#[test]
fn mismatched_element_types_are_rejected() {
    assert_eq!(
        compile_errors("fn main() { let a = [1, true]; }"),
        vec!["E0300"]
    );
}

#[test]
fn element_mutation_does_not_require_mut_binding() {
    // Arrays are reference types: mutating an element mutates the referent, so
    // the binding need not be `mut` (only rebinding the variable would).
    assert_eq!(
        stdout("fn main() { let a = [1, 2]; a[0] = 5; print_int(a[0]); }"),
        "5\n",
    );
}

#[test]
fn empty_array_literal_is_rejected() {
    assert_eq!(compile_errors("fn main() { let a = []; }"), vec!["E0314"]);
}

#[test]
fn array_new_length_must_be_an_integer() {
    assert_eq!(
        compile_errors("fn main() { let a = array_new_int(true); }"),
        vec!["E0300"]
    );
}

#[test]
fn array_new_result_has_its_element_type() {
    assert_eq!(
        compile_errors("fn main() { let a: [i64] = array_new_str(2); }"),
        vec!["E0300"]
    );
}
