//! Tests for the C backend.
//!
//! Where a C compiler is available, scalar programs are transpiled, compiled,
//! run, and their output compared against the VM - proving the two backends
//! agree. Where no compiler is present, the compile-and-run tests are skipped
//! (the transpilation itself is still checked).

use crate::backend::{CError, VmError, emit_c, execute, generate};
use crate::diagnostics::Diagnostics;
use crate::hir::lower;
use crate::lexer::tokenize;
use crate::opt::{OptOptions, optimize};
use crate::parser::parse;
use crate::sema::{check, resolve};
use crate::source::SourceFile;

/// Compiles `src` to HIR (optimized) and returns it.
fn hir_of(src: &str) -> crate::hir::Hir {
    let file = SourceFile::new("t.lm", src);
    let mut diags = Diagnostics::new();
    let tokens = tokenize(&file, &mut diags);
    let ast = parse(tokens, &mut diags);
    let res = resolve(&ast, &mut diags);
    let tc = check(&ast, &res, &mut diags);
    assert!(!diags.has_errors(), "errors:\n{}", diags.render_all(&file));
    let mut hir = lower(&ast, &res, &tc);
    optimize(&mut hir, OptOptions::default());
    hir
}

/// Runs `src` on the VM, returning its stdout.
fn vm_output(src: &str) -> Result<String, VmError> {
    let program = generate(&hir_of(src));
    execute(&program).map(|execution| execution.stdout)
}

#[test]
fn transpiles_scalar_program() {
    let c = emit_c(&hir_of(
        "fn fib(n: i64) -> i64 { if n < 2 { n } else { fib(n - 1) + fib(n - 2) } }\n\
         fn main() { print_int(fib(10)); }",
    ))
    .unwrap();
    assert!(c.contains("int64_t lm_fn_fib(int64_t _0)"));
    assert!(c.contains("int main(void)"));
    assert!(c.contains("lm_fn_main()"));
}

#[test]
fn rejects_strings() {
    let err = emit_c(&hir_of(r#"fn main() { print_str("hi"); }"#)).unwrap_err();
    assert_eq!(err, CError::Unsupported("strings"));
}

#[test]
fn rejects_arrays() {
    let err = emit_c(&hir_of("fn main() { let a = [1, 2]; print_int(a[0]); }")).unwrap_err();
    assert!(matches!(err, CError::Unsupported(_)));
}

/// Finds an available C compiler, if any.
fn find_cc() -> Option<&'static str> {
    for cc in ["cc", "gcc", "clang"] {
        let ok = std::process::Command::new(cc)
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok {
            return Some(cc);
        }
    }
    None
}

/// Transpiles, compiles, and runs `src`, returning the compiled program's
/// stdout. Returns `None` if no C compiler is available.
fn compile_and_run(src: &str, name: &str) -> Option<String> {
    let cc = find_cc()?;
    let c_code = emit_c(&hir_of(src)).expect("scalar program transpiles");
    let dir = std::env::temp_dir();
    let c_path = dir.join(format!("lumen_{name}.c"));
    let bin_path = dir.join(format!("lumen_{name}.bin"));
    std::fs::write(&c_path, c_code).unwrap();

    let status = std::process::Command::new(cc)
        .arg("-O2")
        .arg("-o")
        .arg(&bin_path)
        .arg(&c_path)
        .status()
        .unwrap();
    assert!(status.success(), "C compilation failed for {name}");

    let output = std::process::Command::new(&bin_path).output().unwrap();
    let _ = std::fs::remove_file(&c_path);
    let _ = std::fs::remove_file(&bin_path);

    let mut result = String::from_utf8(output.stdout).unwrap();
    result.push_str(&String::from_utf8(output.stderr).unwrap());
    Some(result)
}

#[test]
fn c_backend_agrees_with_vm_on_fibonacci() {
    let src = "fn fib(n: i64) -> i64 { if n < 2 { n } else { fib(n - 1) + fib(n - 2) } }\n\
               fn main() { let mut i = 0; while i <= 10 { print_int(fib(i)); i = i + 1; } }";
    if let Some(c_out) = compile_and_run(src, "fib") {
        assert_eq!(c_out, vm_output(src).expect("vm run: "));
    }
}

#[test]
fn c_backend_agrees_with_vm_on_div_by_zero() {
    let src = "fn divide(a: i64, b: i64) -> i64 { a / b }
               fn main() { print_int(divide(10, 0)); }";

    let vm_result = vm_output(src);
    assert!(matches!(vm_result, Err(VmError::DivisionByZero)));

    if let Some(c_out) = compile_and_run(src, "divide") {
        assert_eq!(c_out, String::from("runtime error: division by zero\n"));
    }
}

#[test]
fn c_backend_agrees_with_vm_on_mod_by_zero() {
    let src = "fn rem(a: i64, b: i64) -> i64 { a % b }
               fn main() { print_int(rem(10, 0)); }";

    let vm_result = vm_output(src);
    assert!(matches!(vm_result, Err(VmError::DivisionByZero)));

    if let Some(c_out) = compile_and_run(src, "rem") {
        assert_eq!(c_out, String::from("runtime error: division by zero\n"));
    }
}

#[test]
fn c_backend_agrees_with_vm_on_loops_and_arithmetic() {
    let src = "fn main() {\n\
               \x20   let mut total = 0;\n\
               \x20   for i in 0..100 { if i % 2 == 0 { total = total + i; } }\n\
               \x20   print_int(total);\n\
               \x20   print_bool(total > 1000);\n\
               }";
    if let Some(c_out) = compile_and_run(src, "loops") {
        assert_eq!(c_out, vm_output(src).expect("vm run: "));
    }
}
