//! Graphviz output for `lumenc dump cfg`: a `digraph` per function with a node
//! per block listing its code, and edges for control flow. A branch's edges
//! are labelled `T` and `F`.

use std::fmt::Write as _;

use super::print::{inst_str, operand};
use crate::mir::*;

/// Renders each function's CFG as a DOT `digraph`.
pub fn to_dot(program: &Program) -> String {
    let mut out = String::new();
    for func in &program.functions {
        function_dot(&mut out, func);
    }
    out
}

fn function_dot(out: &mut String, func: &Function) {
    let _ = writeln!(out, "digraph \"{}\" {{", func.name);
    out.push_str("  node [shape=box, fontname=\"monospace\"];\n");
    for (i, block) in func.blocks.iter().enumerate() {
        let mut label = format!("bb{i}");
        if BlockId(i as u32) == func.entry {
            label.push_str(" (entry)");
        }
        for inst in &block.insts {
            let _ = write!(label, "\\l{}", escape(&inst_str(inst)));
        }
        let _ = write!(label, "\\l{}", escape(&term_str(&block.term)));
        let _ = writeln!(out, "  bb{i} [label=\"{label}\\l\"];");
    }
    for (i, block) in func.blocks.iter().enumerate() {
        match &block.term {
            Terminator::Goto(t) => {
                let _ = writeln!(out, "  bb{i} -> bb{};", t.0);
            }
            Terminator::Branch {
                then_bb, else_bb, ..
            } => {
                let _ = writeln!(out, "  bb{i} -> bb{} [label=\"T\"];", then_bb.0);
                let _ = writeln!(out, "  bb{i} -> bb{} [label=\"F\"];", else_bb.0);
            }
            Terminator::Return(_) | Terminator::Unreachable => {}
        }
    }
    out.push_str("}\n");
}

/// Escapes `\\` and `"` for a DOT label.
fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// A terminator without its targets, which the edges show.
fn term_str(term: &Terminator) -> String {
    match term {
        Terminator::Goto(b) => format!("goto bb{}", b.0),
        Terminator::Branch { cond, .. } => format!("branch {}", operand(cond)),
        Terminator::Return(o) => format!("return {}", operand(o)),
        Terminator::Unreachable => "unreachable".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::Diagnostics;
    use crate::hir::lower;
    use crate::lexer::tokenize;
    use crate::mir::build;
    use crate::parser::parse;
    use crate::sema::{check, resolve};
    use crate::source::SourceFile;

    fn dot_of(src: &str) -> String {
        let file = SourceFile::new("t.lm", src);
        let mut diags = Diagnostics::new();
        let tokens = tokenize(&file, &mut diags);
        let ast = parse(tokens, &mut diags);
        let res = resolve(&ast, &mut diags);
        let tc = check(&ast, &res, &mut diags);
        assert!(!diags.has_errors());
        let hir = lower(&ast, &res, &tc);
        to_dot(&build(&hir))
    }

    #[test]
    fn emits_a_digraph_with_edges() {
        let dot = dot_of("fn main() { let mut i = 0; while i < 3 { i = i + 1; } }");
        assert!(dot.contains("digraph \"main\""));
        assert!(dot.contains("->"), "no edges in CFG");
        assert!(dot.contains("[label=\"T\"]"), "branch arms not labelled");
    }

    #[test]
    fn is_deterministic() {
        let a = dot_of("fn main() { print_int(1); }");
        let b = dot_of("fn main() { print_int(1); }");
        assert_eq!(a, b);
    }
}
