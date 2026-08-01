//! A deterministic disassembler for [`Program`] bytecode.
//!
//! Renders each chunk as a header plus a numbered instruction listing, for
//! snapshot tests and `lumenc --dump bytecode`. Instruction indices are shown
//! so jump targets are easy to follow. Output is stable across runs.

use std::fmt::Write as _;

use crate::backend::bytecode::{Op, Program};
use crate::sema::types::Builtin;

/// Renders a whole program's bytecode to a string.
pub fn disassemble(program: &Program) -> String {
    let mut out = String::new();
    for (idx, chunk) in program.functions.iter().enumerate() {
        let entry = if idx == program.main { " [entry]" } else { "" };
        let _ = writeln!(
            out,
            "fn#{} {} (params={}, locals={}){}",
            idx, chunk.name, chunk.n_params, chunk.n_locals, entry
        );
        for (i, op) in chunk.code.iter().enumerate() {
            let _ = writeln!(out, "  {i:>4}  {}", render_op(op, chunk));
        }
    }
    out
}

fn render_op(op: &Op, chunk: &crate::backend::bytecode::Chunk) -> String {
    match op {
        Op::PushInt(v) => format!("push_int {v}"),
        Op::PushFloat(v) => format!("push_float {v}"),
        Op::PushBool(v) => format!("push_bool {v}"),
        Op::PushUnit => "push_unit".to_string(),
        Op::PushStr(idx) => {
            let s = chunk.consts.get(*idx as usize).map(|s| &**s).unwrap_or("?");
            format!("push_str {s:?}")
        }
        Op::LoadLocal(n) => format!("load_local {n}"),
        Op::StoreLocal(n) => format!("store_local {n}"),
        Op::Pop => "pop".to_string(),
        Op::AddInt => "add.i".to_string(),
        Op::SubInt => "sub.i".to_string(),
        Op::MulInt => "mul.i".to_string(),
        Op::DivInt => "div.i".to_string(),
        Op::RemInt => "rem.i".to_string(),
        Op::NegInt => "neg.i".to_string(),
        Op::AddFloat => "add.f".to_string(),
        Op::SubFloat => "sub.f".to_string(),
        Op::MulFloat => "mul.f".to_string(),
        Op::DivFloat => "div.f".to_string(),
        Op::RemFloat => "rem.f".to_string(),
        Op::NegFloat => "neg.f".to_string(),
        Op::LtInt => "lt.i".to_string(),
        Op::LeInt => "le.i".to_string(),
        Op::GtInt => "gt.i".to_string(),
        Op::GeInt => "ge.i".to_string(),
        Op::LtFloat => "lt.f".to_string(),
        Op::LeFloat => "le.f".to_string(),
        Op::GtFloat => "gt.f".to_string(),
        Op::GeFloat => "ge.f".to_string(),
        Op::ConcatStr => "concat".to_string(),
        Op::MakeArray(n) => format!("make_array {n}"),
        Op::NewArray(elem) => format!("new_array {}", elem.ty()),
        Op::Index => "index".to_string(),
        Op::SetIndex => "set_index".to_string(),
        Op::ArrayLen => "array_len".to_string(),
        Op::Eq => "eq".to_string(),
        Op::Ne => "ne".to_string(),
        Op::NotBool => "not".to_string(),
        Op::Jump(t) => format!("jump {t}"),
        Op::JumpIfFalse(t) => format!("jump_if_false {t}"),
        Op::Call { func, argc } => format!("call fn#{func} argc={argc}"),
        Op::CallBuiltin { builtin, argc } => {
            format!("call_builtin {} argc={argc}", Builtin::name(*builtin))
        }
        Op::Return => "return".to_string(),
    }
}
