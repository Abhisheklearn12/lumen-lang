//! The MIR printer behind `lumenc dump mir`: each block as a labelled list of
//! instructions ending in its terminator.

use std::fmt::Write as _;

use crate::hir::Callee;
use crate::mir::*;
use crate::sema::types::Builtin;

/// Renders a whole program.
pub fn print_mir(program: &Program) -> String {
    let mut out = String::new();
    for (i, func) in program.functions.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        print_function(&mut out, func);
    }
    out
}

fn print_function(out: &mut String, func: &Function) {
    let params = (0..func.param_count)
        .map(|i| format!("_{i}"))
        .collect::<Vec<_>>()
        .join(", ");
    let _ = writeln!(
        out,
        "fn {}({}) [{} regs] {{",
        func.name, params, func.reg_count
    );
    for (i, block) in func.blocks.iter().enumerate() {
        let marker = if BlockId(i as u32) == func.entry {
            " (entry)"
        } else {
            ""
        };
        let _ = writeln!(out, "  bb{i}:{marker}");
        for inst in &block.insts {
            let _ = writeln!(out, "    {}", inst_str(inst));
        }
        let _ = writeln!(out, "    {}", term_str(&block.term));
    }
    out.push_str("}\n");
}

pub(super) fn inst_str(inst: &Inst) -> String {
    match inst {
        Inst::Assign { dst, rvalue } => format!("{} = {}", reg(*dst), rvalue_str(rvalue)),
        Inst::Store { local, src } => format!("_{} <- {}", local.0, operand(src)),
        Inst::SetIndex { base, index, value } => {
            format!(
                "{}[{}] <- {}",
                operand(base),
                operand(index),
                operand(value)
            )
        }
        Inst::Call {
            dst, callee, args, ..
        } => {
            let args = args.iter().map(operand).collect::<Vec<_>>().join(", ");
            format!("{} = call {}({})", reg(*dst), callee_str(*callee), args)
        }
    }
}

fn rvalue_str(rvalue: &Rvalue) -> String {
    match rvalue {
        Rvalue::Use(o) => operand(o),
        Rvalue::Load(l) => format!("load _{}", l.0),
        Rvalue::Unary(op, o) => format!("{}{}", op.symbol(), operand(o)),
        Rvalue::Binary(op, a, b) => format!("{} {} {}", operand(a), op.symbol(), operand(b)),
        Rvalue::Concat(a, b) => format!("{} ++ {}", operand(a), operand(b)),
        Rvalue::MakeArray(elems) => {
            format!(
                "[{}]",
                elems.iter().map(operand).collect::<Vec<_>>().join(", ")
            )
        }
        Rvalue::Index(b, i) => format!("{}[{}]", operand(b), operand(i)),
    }
}

fn term_str(term: &Terminator) -> String {
    match term {
        Terminator::Goto(b) => format!("goto bb{}", b.0),
        Terminator::Branch {
            cond,
            then_bb,
            else_bb,
        } => {
            format!(
                "branch {} -> bb{}, bb{}",
                operand(cond),
                then_bb.0,
                else_bb.0
            )
        }
        Terminator::Return(o) => format!("return {}", operand(o)),
        Terminator::Unreachable => "unreachable".to_string(),
    }
}

pub(super) fn operand(o: &Operand) -> String {
    match o {
        Operand::Const(c) => const_str(c),
        Operand::Reg(r) => reg(*r),
    }
}

fn reg(r: Reg) -> String {
    format!("%{}", r.0)
}

fn const_str(c: &Const) -> String {
    match c {
        Const::Int(v) => v.to_string(),
        Const::Float(v) => v.to_string(),
        Const::Bool(v) => v.to_string(),
        Const::Str(v) => format!("{v:?}"),
        Const::Unit => "unit".to_string(),
    }
}

fn callee_str(callee: Callee) -> String {
    match callee {
        Callee::Fn(id) => format!("fn#{}", id.0),
        Callee::Builtin(b) => format!("@{}", Builtin::name(b)),
    }
}
