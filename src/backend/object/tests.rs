//! Tests for the bytecode object format.

use super::*;
use crate::backend::{execute, generate};
use crate::diagnostics::Diagnostics;
use crate::hir::lower;
use crate::lexer::tokenize;
use crate::opt::{OptOptions, optimize};
use crate::parser::parse;
use crate::sema::{check, resolve};
use crate::source::SourceFile;

/// Compiles `src` to an optimized bytecode program.
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

const PROGRAMS: &[&str] = &[
    "fn main() { print_int(1 + 2 * 3); }",
    "fn fib(n: i64) -> i64 { if n < 2 { n } else { fib(n-1) + fib(n-2) } }\n\
     fn main() { print_int(fib(10)); }",
    "fn main() { let mut s = 0; for i in 0..10 { s += i; } print_int(s); }",
    r#"fn main() { print_str("hello, " + "world"); print_float(1.5); }"#,
    "fn main() { let a = [1, 2, 3]; print_int(a[0] + len(a)); }",
    "fn main() { let n = 2; print_int(len(array_new_int(n)) + len(array_new_float(n))); \
     print_int(len(array_new_bool(n)) + len(array_new_str(n))); }",
];

#[test]
fn round_trips_to_text_and_back() {
    for src in PROGRAMS {
        let program = compile(src);
        let text = to_text(&program);
        let parsed = from_text(&text).expect("parse object");
        // Re-serializing the parsed program must match the original text.
        assert_eq!(to_text(&parsed), text, "round-trip differs for:\n{src}");
    }
}

#[test]
fn deserialized_program_runs_identically() {
    for src in PROGRAMS {
        let program = compile(src);
        let original = execute(&program).unwrap().stdout;
        let parsed = from_text(&to_text(&program)).unwrap();
        let reloaded = execute(&parsed).unwrap().stdout;
        assert_eq!(original, reloaded, "reloaded program differs for:\n{src}");
    }
}

#[test]
fn header_records_the_entry_point() {
    let text = to_text(&compile("fn helper() {} fn main() { helper(); }"));
    assert!(
        text.starts_with("lumen-obj 1 main=1"),
        "header: {}",
        text.lines().next().unwrap()
    );
}

#[test]
fn rejects_garbage() {
    assert!(from_text("not an object file").is_err());
    assert!(
        from_text("lumen-obj 1 main=0\nfn x params=0 locals=0 consts=0 code=1\n  bogus").is_err()
    );
}

#[test]
fn strings_with_escapes_round_trip() {
    let program = compile(r#"fn main() { print_str("tab\there\nnewline"); }"#);
    let parsed = from_text(&to_text(&program)).unwrap();
    assert_eq!(
        execute(&program).unwrap().stdout,
        execute(&parsed).unwrap().stdout
    );
}

#[test]
fn rejects_unknown_array_element_type() {
    let text = "lumen-obj 1 main=0\nfn main params=0 locals=0 consts=0 code=1\n  new_array widget";
    let err = from_text(text).unwrap_err();
    assert!(err.contains("unknown array element type `widget`"), "{err}");
}
