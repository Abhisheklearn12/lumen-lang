//! Criterion benchmarks: each phase on its own, the whole compile, and a VM
//! run, all on one fixed program.

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};

use lumen::backend::{execute, generate};
use lumen::diagnostics::Diagnostics;
use lumen::hir::lower;
use lumen::lexer::tokenize;
use lumen::opt::{OptOptions, optimize};
use lumen::parser::parse;
use lumen::sema::{check, resolve};
use lumen::session::{PipelineOptions, Session};
use lumen::source::SourceFile;

/// A representative program: recursion, a loop, arithmetic, and calls.
const PROGRAM: &str = "\
fn fib(n: i64) -> i64 {
    if n < 2 { n } else { fib(n - 1) + fib(n - 2) }
}

fn sum_to(n: i64) -> i64 {
    let mut total = 0;
    let mut i = 1;
    while i <= n {
        total = total + i;
        i = i + 1;
    }
    total
}

fn main() {
    let mut k = 0;
    while k < 15 {
        print_int(fib(k));
        k = k + 1;
    }
    print_int(sum_to(100));
}
";

fn bench_phases(c: &mut Criterion) {
    let mut group = c.benchmark_group("phases");

    group.bench_function("lex", |b| {
        let file = SourceFile::new("bench.lm", PROGRAM);
        b.iter(|| {
            let mut diags = Diagnostics::new();
            black_box(tokenize(&file, &mut diags))
        });
    });

    group.bench_function("parse", |b| {
        let file = SourceFile::new("bench.lm", PROGRAM);
        let mut diags = Diagnostics::new();
        let tokens = tokenize(&file, &mut diags);
        b.iter(|| {
            let mut diags = Diagnostics::new();
            black_box(parse(tokens.clone(), &mut diags))
        });
    });

    group.bench_function("resolve_and_typeck", |b| {
        let file = SourceFile::new("bench.lm", PROGRAM);
        let mut diags = Diagnostics::new();
        let tokens = tokenize(&file, &mut diags);
        let ast = parse(tokens, &mut diags);
        b.iter(|| {
            let mut diags = Diagnostics::new();
            let res = resolve(&ast, &mut diags);
            black_box(check(&ast, &res, &mut diags))
        });
    });

    group.bench_function("optimize", |b| {
        let file = SourceFile::new("bench.lm", PROGRAM);
        let mut diags = Diagnostics::new();
        let tokens = tokenize(&file, &mut diags);
        let ast = parse(tokens, &mut diags);
        let res = resolve(&ast, &mut diags);
        let tc = check(&ast, &res, &mut diags);
        b.iter(|| {
            let mut hir = lower(&ast, &res, &tc);
            black_box(optimize(&mut hir, OptOptions::default()))
        });
    });

    group.bench_function("codegen", |b| {
        let file = SourceFile::new("bench.lm", PROGRAM);
        let mut diags = Diagnostics::new();
        let tokens = tokenize(&file, &mut diags);
        let ast = parse(tokens, &mut diags);
        let res = resolve(&ast, &mut diags);
        let tc = check(&ast, &res, &mut diags);
        let mut hir = lower(&ast, &res, &tc);
        optimize(&mut hir, OptOptions::default());
        b.iter(|| black_box(generate(&hir)));
    });

    group.finish();
}

fn bench_end_to_end(c: &mut Criterion) {
    c.bench_function("compile_end_to_end", |b| {
        b.iter(|| {
            let mut session = Session::new("bench.lm", PROGRAM);
            black_box(session.compile(PipelineOptions::default()))
        });
    });

    c.bench_function("execute", |b| {
        let mut session = Session::new("bench.lm", PROGRAM);
        let program = session.compile(PipelineOptions::default()).program.unwrap();
        b.iter(|| black_box(execute(&program).unwrap()));
    });
}

criterion_group!(benches, bench_phases, bench_end_to_end);
criterion_main!(benches);
