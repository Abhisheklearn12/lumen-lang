//! Unit tests for HIR lowering.

use crate::hir::{Callee, ExprKind, Stmt, lower, print_hir};
use crate::lexer::tokenize;
use crate::parser::parse;
use crate::sema::{check, resolve};
use crate::source::SourceFile;

/// Runs the full front-end and lowers `src`, asserting it is error-free.
fn lower_src(src: &str) -> crate::hir::Hir {
    let file = SourceFile::new("test.lm", src);
    let mut diags = crate::diagnostics::Diagnostics::new();
    let tokens = tokenize(&file, &mut diags);
    let ast = parse(tokens, &mut diags);
    let res = resolve(&ast, &mut diags);
    let tc = check(&ast, &res, &mut diags);
    assert!(
        !diags.has_errors(),
        "unexpected errors:\n{}",
        diags.render_all(&file)
    );
    lower(&ast, &res, &tc)
}

fn printed(src: &str) -> String {
    print_hir(&lower_src(src))
}

#[test]
fn parameters_become_leading_locals() {
    let hir = lower_src("fn f(a: i64, b: i64) -> i64 { a + b } fn main() {}");
    let f = &hir.functions[0];
    assert_eq!(f.param_count, 2);
    assert_eq!(f.locals[0].name, "a");
    assert_eq!(f.locals[1].name, "b");
    assert_eq!(f.locals.len(), 2);
}

#[test]
fn let_allocates_a_local_after_params() {
    let hir = lower_src("fn f(a: i64) -> i64 { let b = a; b } fn main() {}");
    let f = &hir.functions[0];
    assert_eq!(f.param_count, 1);
    assert_eq!(f.locals.len(), 2); // a (param) + b (let)
    assert_eq!(f.locals[1].name, "b");
}

#[test]
fn names_lower_to_local_reads() {
    let hir = lower_src("fn f(a: i64) -> i64 { a } fn main() {}");
    let f = &hir.functions[0];
    let tail = f.body.tail.as_ref().unwrap();
    assert!(matches!(tail.kind, ExprKind::Local(id) if id.0 == 0));
}

#[test]
fn calls_resolve_to_callees() {
    let hir = lower_src("fn g(x: i64) -> i64 { x } fn main() { let y = g(1); print_int(y); }");
    let main = hir.functions.iter().find(|f| f.name == "main").unwrap();
    // `let y = g(1)` calls a user function.
    let Stmt::Let { value, .. } = &main.body.stmts[0] else {
        panic!("expected let")
    };
    assert!(matches!(
        &value.kind,
        ExprKind::Call {
            callee: Callee::Fn(_),
            ..
        }
    ));
    // `print_int(y)` calls a builtin.
    let Stmt::Expr(call) = &main.body.stmts[1] else {
        panic!("expected expr-stmt")
    };
    assert!(matches!(
        &call.kind,
        ExprKind::Call {
            callee: Callee::Builtin(_),
            ..
        }
    ));
}

#[test]
fn assignment_targets_a_slot() {
    let hir = lower_src("fn main() { let mut x = 0; x = 5; }");
    let main = &hir.functions[0];
    let Stmt::Expr(assign) = &main.body.stmts[1] else {
        panic!("expected expr-stmt")
    };
    assert!(matches!(&assign.kind, ExprKind::Assign { local, .. } if local.0 == 0));
}

#[test]
fn main_is_marked_as_entry() {
    let hir = lower_src("fn helper() {} fn main() { helper(); }");
    // `main` is declared second, so it is FnId(1).
    assert_eq!(hir.main.0, 1);
}

#[test]
fn every_expression_is_typed() {
    use crate::sema::types::Type;
    let hir = lower_src("fn main() { let x = 1 + 2 < 4; }");
    // Walk the let initialiser: the comparison is bool, its operands int.
    let Stmt::Let { value, .. } = &hir.functions[0].body.stmts[0] else {
        panic!()
    };
    assert_eq!(value.ty, Type::Bool);
    let ExprKind::Binary { lhs, .. } = &value.kind else {
        panic!("expected comparison")
    };
    assert_eq!(lhs.ty, Type::Int);
}

#[test]
fn snapshot_of_lowered_fib() {
    let out = printed(
        "fn fib(n: i64) -> i64 {\n\
        \x20   if n < 2 { n } else { fib(n - 1) + fib(n - 2) }\n\
         }\n\
         fn main() { print_int(fib(10)); }",
    );
    let expected = "\
fn fib(0:n i64) -> i64
  block: i64
    tail
      if: i64
        cond
          binary <: bool
            local _0: i64
            int 2: i64
        block: i64
          tail
            local _0: i64
        else
          block: i64
            tail
              binary +: i64
                call fn#0: i64
                  binary -: i64
                    local _0: i64
                    int 1: i64
                call fn#0: i64
                  binary -: i64
                    local _0: i64
                    int 2: i64
fn main() -> unit [entry]
  block: unit
    expr-stmt
      call builtin print_int: unit
        call fn#0: i64
          int 10: i64
";
    assert_eq!(out, expected);
}
