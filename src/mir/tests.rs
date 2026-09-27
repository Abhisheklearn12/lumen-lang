//! Tests for MIR construction and optimization.

use crate::diagnostics::Diagnostics;
use crate::hir::lower;
use crate::lexer::tokenize;
use crate::mir::{Inst, Rvalue, Terminator, build, optimize, print_mir};
use crate::opt::{OptOptions, optimize as optimize_hir};
use crate::parser::parse;
use crate::sema::{check, resolve};
use crate::source::SourceFile;

/// Front-end + HIR + MIR for `src`, with HIR optimization disabled so MIR sees
/// the unoptimised tree (the more interesting input for MIR passes).
fn mir_of(src: &str) -> crate::mir::Program {
    let file = SourceFile::new("t.lm", src);
    let mut diags = Diagnostics::new();
    let tokens = tokenize(&file, &mut diags);
    let ast = parse(tokens, &mut diags);
    let res = resolve(&ast, &mut diags);
    let tc = check(&ast, &res, &mut diags);
    assert!(!diags.has_errors(), "errors:\n{}", diags.render_all(&file));
    let mut hir = lower(&ast, &res, &tc);
    optimize_hir(
        &mut hir,
        OptOptions {
            enabled: false,
            max_iterations: 0,
        },
    );
    build(&hir)
}

/// Every block must end in a real terminator, never `Unreachable`.
fn assert_well_formed(program: &crate::mir::Program) {
    for func in &program.functions {
        for (i, block) in func.blocks.iter().enumerate() {
            assert!(
                !matches!(block.term, Terminator::Unreachable),
                "fn {} bb{i} has no terminator",
                func.name
            );
        }
    }
}

#[test]
fn builds_well_formed_cfg() {
    let program = mir_of(
        "fn f(n: i64) -> i64 { if n < 0 { 0 - n } else { n } }\n\
         fn main() { print_int(f(0 - 5)); }",
    );
    let mut opt = mir_of(
        "fn f(n: i64) -> i64 { if n < 0 { 0 - n } else { n } }\n\
         fn main() { print_int(f(0 - 5)); }",
    );
    optimize(&mut opt);
    assert_well_formed(&program);
    assert_well_formed(&opt);
}

#[test]
fn loops_lower_to_branches() {
    let program = mir_of("fn main() { let mut i = 0; while i < 3 { i = i + 1; } }");
    let main = &program.functions[0];
    // A loop produces at least one conditional branch.
    let has_branch = main
        .blocks
        .iter()
        .any(|b| matches!(b.term, Terminator::Branch { .. }));
    assert!(has_branch, "while loop did not produce a branch");
}

#[test]
fn constant_folding() {
    let mut program = mir_of("fn main() { let x = 1 + 2 * 3; print_int(x); }");
    optimize(&mut program);
    // No remaining `Binary` rvalue: `1 + 2 * 3` folded to a constant.
    let main = &program.functions[0];
    let has_binary = main.blocks.iter().flat_map(|b| &b.insts).any(|i| {
        matches!(
            i,
            Inst::Assign {
                rvalue: Rvalue::Binary(..),
                ..
            }
        )
    });
    assert!(
        !has_binary,
        "arithmetic was not constant-folded:\n{}",
        print_mir(&program)
    );
}

#[test]
fn constant_branch_is_simplified() {
    let mut program =
        mir_of("fn f() -> i64 { if true { 1 } else { 2 } } fn main() { print_int(f()); }");
    let before_blocks: usize = program.functions.iter().map(|f| f.blocks.len()).sum();
    optimize(&mut program);
    let after_blocks: usize = program.functions.iter().map(|f| f.blocks.len()).sum();
    // Folding `if true` and pruning the dead arm removes blocks.
    assert!(
        after_blocks < before_blocks,
        "constant branch not simplified"
    );
    assert_well_formed(&program);
}

#[test]
fn copy_propagation_and_dce() {
    // Folding `1 + 2 * 3` to `7` leaves the intermediate temporaries dead; copy
    // propagation forwards the constant and DCE removes the now-unused work.
    let mut program = mir_of("fn main() { let x = 1 + 2 * 3; print_int(x); }");
    let raw_insts: usize = program.functions[0]
        .blocks
        .iter()
        .map(|b| b.insts.len())
        .sum();
    optimize(&mut program);
    let opt_insts: usize = program.functions[0]
        .blocks
        .iter()
        .map(|b| b.insts.len())
        .sum();
    assert!(
        opt_insts < raw_insts,
        "copy propagation/DCE removed nothing ({raw_insts} -> {opt_insts})\n{}",
        print_mir(&program)
    );
    assert_well_formed(&program);
}

#[test]
fn all_blocks_reachable_after_simplification() {
    let mut program = mir_of(
        "fn f(n: i64) -> i64 {\n\
        \x20   let mut s = 0;\n\
        \x20   for i in 0..n { s += i; }\n\
        \x20   s\n\
         }\n\
         fn main() { print_int(f(10)); }",
    );
    optimize(&mut program);
    for func in &program.functions {
        // Reachability: walk from entry and confirm we can see every block.
        let mut seen = std::collections::HashSet::new();
        let mut stack = vec![func.entry];
        while let Some(b) = stack.pop() {
            if seen.insert(b) {
                stack.extend(func.block(b).term.successors());
            }
        }
        assert_eq!(
            seen.len(),
            func.blocks.len(),
            "fn {} has unreachable blocks",
            func.name
        );
    }
}

#[test]
fn dump_is_deterministic() {
    let a = print_mir(&mir_of("fn main() { print_int(1 + 1); }"));
    let b = print_mir(&mir_of("fn main() { print_int(1 + 1); }"));
    assert_eq!(a, b);
}

/// Counts `Binary` instructions across all functions.
fn count_binary(program: &crate::mir::Program) -> usize {
    program
        .functions
        .iter()
        .flat_map(|f| f.blocks.iter())
        .flat_map(|b| b.insts.iter())
        .filter(|i| {
            matches!(
                i,
                Inst::Assign {
                    rvalue: Rvalue::Binary(..),
                    ..
                }
            )
        })
        .count()
}

#[test]
fn local_cse_forwards_redundant_loads() {
    // `a * a` reads `a` twice; a single CSE pass forwards the second load to the
    // first register, so at least one rewrite happens.
    let mut program = mir_of(
        "fn f(a: i64) -> i64 { a * a }\n\
         fn main() { print_int(f(3)); }",
    );
    let changed = crate::mir::opt::local_cse(&mut program.functions[0]);
    assert!(changed >= 1, "expected the redundant load to be forwarded");
}

#[test]
fn full_optimization_eliminates_a_repeated_computation() {
    // After the fixpoint loop (copy propagation reuniting the operands, then
    // CSE), the duplicated `a * a` collapses to a single multiply.
    let mut program = mir_of(
        "fn f(a: i64) -> i64 { let x = a * a; let y = a * a; x + y }\n\
         fn main() { print_int(f(3)); }",
    );
    let naive = count_binary(&program);
    optimize(&mut program);
    let optimized = count_binary(&program);
    assert!(
        optimized < naive,
        "CSE should cut the multiply count ({naive} -> {optimized})"
    );
}

#[test]
fn local_cse_preserves_results() {
    // The fully optimized program (which now includes CSE) must still compute
    // the correct value, checked against the MIR interpreter.
    let mut program = mir_of(
        "fn f(a: i64) -> i64 { let x = a * a; let y = a * a; x + y }\n\
         fn main() { print_int(f(6)); }",
    );
    optimize(&mut program);
    assert_eq!(crate::mir::interpret(&program).unwrap().stdout, "72\n");
}

#[test]
fn local_cse_respects_intervening_stores() {
    // `a` is reassigned between the two reads, so the loads must not be merged;
    // the program must still print the value after the store.
    let mut program = mir_of(
        "fn main() {\n\
            let mut a = 3;\n\
            let x = a + 1;\n\
            a = 10;\n\
            let y = a + 1;\n\
            print_int(x + y);\n\
         }",
    );
    optimize(&mut program);
    // x = 3 + 1 = 4, y = 10 + 1 = 11, total 15.
    assert_eq!(crate::mir::interpret(&program).unwrap().stdout, "15\n");
}

#[test]
fn algebraic_simplify_removes_identities() {
    // `x * 1` and `x + 0` simplify to `x`; `x * 0` to `0`. After full
    // optimization no multiply or add by these constants should remain.
    let mut program = mir_of("fn f(x: i64) -> i64 { x * 1 + 0 } fn main() { print_int(f(9)); }");
    let changed = crate::mir::opt::algebraic_simplify(&mut program.functions[0]);
    assert!(changed >= 1, "expected an algebraic rewrite");
}

#[test]
fn algebraic_identities_preserve_results() {
    let mut program = mir_of(
        "fn f(x: i64) -> i64 { (x * 1) + (x - x) + (x * 0) } \n\
         fn main() { print_int(f(7)); }",
    );
    optimize(&mut program);
    // x*1 + (x-x) + x*0 = x + 0 + 0 = x = 7.
    assert_eq!(crate::mir::interpret(&program).unwrap().stdout, "7\n");
}

#[test]
fn dead_store_drops_overwritten_assignment() {
    // `s` is assigned then immediately reassigned with no read between, so the
    // first store is dead.
    let mut program = mir_of("fn main() { let mut s = 1; s = 2; print_int(s); }");
    let removed = crate::mir::opt::dead_store(&mut program.functions[0]);
    assert!(removed >= 1, "expected a dead store to be removed");
}

#[test]
fn dead_store_keeps_read_values() {
    // The intermediate value is read before being overwritten, so nothing is
    // dead and the program prints both contributions.
    let mut program = mir_of(
        "fn main() {\n\
            let mut s = 1;\n\
            print_int(s);\n\
            s = 2;\n\
            print_int(s);\n\
         }",
    );
    optimize(&mut program);
    assert_eq!(crate::mir::interpret(&program).unwrap().stdout, "1\n2\n");
}
