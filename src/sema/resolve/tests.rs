//! Unit tests for name resolution.

use super::*;
use crate::lexer::tokenize;
use crate::parser::parse;
use crate::source::SourceFile;

/// Runs lex → parse → resolve over `src`, returning the artifacts for inspection.
fn run(src: &str) -> (Ast, Resolution, Diagnostics) {
    let file = SourceFile::new("test.lm", src);
    let mut diags = Diagnostics::new();
    let tokens = tokenize(&file, &mut diags);
    let ast = parse(tokens, &mut diags);
    assert!(
        !diags.has_errors(),
        "unexpected lex/parse errors:\n{}",
        diags.render_all(&file)
    );
    let res = resolve(&ast, &mut diags);
    (ast, res, diags)
}

/// Collects the codes of all emitted diagnostics.
fn codes(diags: &Diagnostics) -> Vec<DiagCode> {
    diags.items().iter().map(|d| d.code).collect()
}

#[test]
fn resolves_parameters_and_locals() {
    let (_ast, res, diags) = run("fn f(a: i64) -> i64 { let b = a; b }");
    assert!(!diags.has_errors());
    // Every `Name` use resolved to a local.
    assert!(res.uses.values().all(|r| matches!(r, Res::Local(_))));
    assert_eq!(res.uses.len(), 2); // `a` in init, `b` in tail
}

#[test]
fn resolves_function_calls_including_forward_and_recursive() {
    let (_ast, res, diags) = run("fn main() { helper(); }\n\
         fn helper() { main(); helper(); }");
    assert!(!diags.has_errors());
    let fn_uses = res
        .uses
        .values()
        .filter(|r| matches!(r, Res::Fn(_)))
        .count();
    assert_eq!(fn_uses, 3, "expected 3 function-name uses");
}

#[test]
fn resolves_builtins() {
    let (_ast, res, diags) = run(r#"fn main() { print_int(1); print_str("hi"); }"#);
    assert!(!diags.has_errors());
    let builtins = res
        .uses
        .values()
        .filter(|r| matches!(r, Res::Builtin(_)))
        .count();
    assert_eq!(builtins, 2);
}

#[test]
fn unresolved_name_is_reported() {
    let (_ast, _res, diags) = run("fn f() { nonexistent }");
    assert_eq!(codes(&diags), vec![DiagCode::UnresolvedName]);
}

#[test]
fn duplicate_function_is_reported() {
    let (_ast, _res, diags) = run("fn dup() {} fn dup() {}");
    assert_eq!(codes(&diags), vec![DiagCode::DuplicateDefinition]);
}

#[test]
fn duplicate_parameter_is_reported() {
    let (_ast, _res, diags) = run("fn f(x: i64, x: i64) {}");
    assert_eq!(codes(&diags), vec![DiagCode::DuplicateParameter]);
}

#[test]
fn shadowing_in_same_scope_is_allowed() {
    let (_ast, _res, diags) = run("fn f() { let x = 1; let x = 2; let y = x; }");
    assert!(!diags.has_errors(), "shadowing should be permitted");
}

#[test]
fn inner_scope_does_not_leak_outward() {
    // `inner` is declared in the block; using it after the block must fail.
    let (_ast, _res, diags) = run("fn f() { { let inner = 1; } inner }");
    assert_eq!(codes(&diags), vec![DiagCode::UnresolvedName]);
}

#[test]
fn let_initialiser_cannot_see_its_own_binding() {
    // The `n` in the initialiser must be unresolved (no outer `n`).
    let (_ast, _res, diags) = run("fn f() { let n = n; }");
    assert_eq!(codes(&diags), vec![DiagCode::UnresolvedName]);
}

#[test]
fn let_initialiser_sees_outer_binding_of_same_name() {
    // The initialiser's `n` is the parameter; the new `n` shadows it after.
    let (_ast, res, diags) = run("fn f(n: i64) -> i64 { let n = n; n }");
    assert!(!diags.has_errors());
    assert!(res.uses.values().all(|r| matches!(r, Res::Local(_))));
}

#[test]
fn while_condition_and_body_are_resolved() {
    let (_ast, _res, diags) = run("fn f() { let mut i = 0; while i < 3 { i = i + 1; } }");
    assert!(!diags.has_errors());
}

#[test]
fn local_info_records_mutability() {
    let (ast, res, _diags) = run("fn f() { let mut m = 1; let c = 2; }");
    let ItemKind::Fn(decl) = &ast.items[0].kind else {
        panic!("expected a function")
    };
    // Walk the let statements to find their node ids.
    let mut mutables = Vec::new();
    for stmt in &decl.body.stmts {
        if let StmtKind::Let(l) = &stmt.kind {
            mutables.push((l.name.name.clone(), res.local(l.id).unwrap().mutable));
        }
    }
    assert_eq!(
        mutables,
        vec![("m".to_string(), true), ("c".to_string(), false)]
    );
}
