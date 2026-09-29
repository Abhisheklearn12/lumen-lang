//! Every program in `examples/` must run and print exactly what is expected.

mod common;

use common::stdout;

/// Compiles and runs the example file at `examples/<name>.lm`.
fn run_example(name: &str) -> String {
    let path = format!("{}/examples/{name}.lm", env!("CARGO_MANIFEST_DIR"));
    let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    stdout(&src)
}

#[test]
fn fib() {
    assert_eq!(run_example("fib"), "0\n1\n1\n2\n3\n5\n8\n13\n21\n34\n55\n");
}

#[test]
fn factorial() {
    assert_eq!(run_example("factorial"), "720\n720\n");
}

#[test]
fn fizzbuzz() {
    let out = run_example("fizzbuzz");
    let expected = "1\n2\nFizz\n4\nBuzz\nFizz\n7\n8\nFizz\nBuzz\n\
                    11\nFizz\n13\n14\nFizzBuzz\n16\n17\nFizz\n19\nBuzz\n";
    assert_eq!(out, expected);
}

#[test]
fn primes() {
    assert_eq!(
        run_example("primes"),
        "2\n3\n5\n7\n11\n13\n17\n19\n23\n29\n"
    );
}
