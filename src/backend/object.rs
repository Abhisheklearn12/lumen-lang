//! A text object format, so a program can be built once (`lumenc build`) and
//! run later (`lumenc exec`).
//!
//! After a `lumen-obj 1 main=N` header, each function has a `fn` line with its
//! counts, one quoted `const` line per string constant, and one line per
//! instruction. [`from_text`] inverts [`to_text`]; a test checks the round
//! trip. Loaded programs must be [verified](crate::backend::verify()) before
//! they run.

use std::fmt::Write as _;
use std::rc::Rc;

use crate::backend::bytecode::{Chunk, Op, Program};
use crate::sema::types::{Builtin, Elem, Type};

/// Writes `program` as object text.
pub fn to_text(program: &Program) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "lumen-obj 1 main={}", program.main);
    for chunk in &program.functions {
        write_chunk(&mut out, chunk);
    }
    out
}

fn write_chunk(out: &mut String, chunk: &Chunk) {
    let _ = writeln!(
        out,
        "fn {} params={} locals={} consts={} code={}",
        chunk.name,
        chunk.n_params,
        chunk.n_locals,
        chunk.consts.len(),
        chunk.code.len()
    );
    for c in &chunk.consts {
        let _ = writeln!(out, "  const {}", quote(c));
    }
    for op in &chunk.code {
        let _ = writeln!(out, "  {}", op_text(op));
    }
}

/// One instruction as `mnemonic arg...`.
fn op_text(op: &Op) -> String {
    match op {
        Op::PushInt(v) => format!("push_int {v}"),
        Op::PushFloat(v) => format!("push_float {v}"),
        Op::PushBool(v) => format!("push_bool {v}"),
        Op::PushUnit => "push_unit".to_string(),
        Op::PushStr(i) => format!("push_str {i}"),
        Op::LoadLocal(n) => format!("load_local {n}"),
        Op::StoreLocal(n) => format!("store_local {n}"),
        Op::Pop => "pop".to_string(),
        Op::AddInt => "add_int".to_string(),
        Op::SubInt => "sub_int".to_string(),
        Op::MulInt => "mul_int".to_string(),
        Op::DivInt => "div_int".to_string(),
        Op::RemInt => "rem_int".to_string(),
        Op::NegInt => "neg_int".to_string(),
        Op::AddFloat => "add_float".to_string(),
        Op::SubFloat => "sub_float".to_string(),
        Op::MulFloat => "mul_float".to_string(),
        Op::DivFloat => "div_float".to_string(),
        Op::RemFloat => "rem_float".to_string(),
        Op::NegFloat => "neg_float".to_string(),
        Op::LtInt => "lt_int".to_string(),
        Op::LeInt => "le_int".to_string(),
        Op::GtInt => "gt_int".to_string(),
        Op::GeInt => "ge_int".to_string(),
        Op::LtFloat => "lt_float".to_string(),
        Op::LeFloat => "le_float".to_string(),
        Op::GtFloat => "gt_float".to_string(),
        Op::GeFloat => "ge_float".to_string(),
        Op::ConcatStr => "concat_str".to_string(),
        Op::MakeArray(n) => format!("make_array {n}"),
        Op::NewArray(elem) => format!("new_array {}", elem.ty()),
        Op::Index => "index".to_string(),
        Op::SetIndex => "set_index".to_string(),
        Op::ArrayLen => "array_len".to_string(),
        Op::Eq => "eq".to_string(),
        Op::Ne => "ne".to_string(),
        Op::NotBool => "not_bool".to_string(),
        Op::Jump(t) => format!("jump {t}"),
        Op::JumpIfFalse(t) => format!("jump_if_false {t}"),
        Op::Call { func, argc } => format!("call {func} {argc}"),
        Op::CallBuiltin { builtin, argc } => {
            format!("call_builtin {} {argc}", Builtin::name(*builtin))
        }
        Op::Return => "return".to_string(),
    }
}

/// Reads a program from object text. The header's version is not checked.
pub fn from_text(text: &str) -> Result<Program, String> {
    let mut lines = text.lines();
    let header = lines.next().ok_or("empty object")?;
    let main = header
        .split_whitespace()
        .find_map(|t| t.strip_prefix("main="))
        .ok_or("missing main= in header")?
        .parse::<usize>()
        .map_err(|_| "invalid main index")?;

    let mut functions = Vec::new();
    let mut pending = lines.peekable();
    while let Some(line) = pending.next() {
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }
        let header = parse_fn_header(line)?;
        let mut consts = Vec::with_capacity(header.consts);
        for _ in 0..header.consts {
            let l = pending.next().ok_or("unexpected end in consts")?.trim();
            let body = l.strip_prefix("const ").ok_or("expected const line")?;
            consts.push(unquote(body)?);
        }
        let mut code = Vec::with_capacity(header.code);
        for _ in 0..header.code {
            let l = pending.next().ok_or("unexpected end in code")?.trim();
            code.push(parse_op(l)?);
        }
        functions.push(Chunk {
            name: header.name,
            n_params: header.params,
            n_locals: header.locals,
            consts,
            code,
        });
    }
    Ok(Program { functions, main })
}

struct FnHeader {
    name: String,
    params: usize,
    locals: usize,
    consts: usize,
    code: usize,
}

fn parse_fn_header(line: &str) -> Result<FnHeader, String> {
    let mut parts = line.split_whitespace();
    if parts.next() != Some("fn") {
        return Err(format!("expected `fn`, got: {line}"));
    }
    let name = parts.next().ok_or("missing function name")?.to_string();
    let mut params = 0;
    let mut locals = 0;
    let mut consts = 0;
    let mut code = 0;
    for kv in parts {
        let (k, v) = kv.split_once('=').ok_or("malformed key=value")?;
        let n = v.parse::<usize>().map_err(|_| "invalid count")?;
        match k {
            "params" => params = n,
            "locals" => locals = n,
            "consts" => consts = n,
            "code" => code = n,
            _ => return Err(format!("unknown header field `{k}`")),
        }
    }
    Ok(FnHeader {
        name,
        params,
        locals,
        consts,
        code,
    })
}

/// Parses one instruction line.
fn parse_op(line: &str) -> Result<Op, String> {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    let (&mnemonic, rest) = tokens.split_first().ok_or("empty instruction")?;
    let arg = |i: usize, what: &str| {
        rest.get(i)
            .copied()
            .ok_or_else(|| format!("missing {what}"))
    };
    let u32_arg = |i: usize, what: &str| -> Result<u32, String> {
        arg(i, what)?.parse().map_err(|_| "bad u32".to_string())
    };
    let usize_arg = |i: usize, what: &str| -> Result<usize, String> {
        arg(i, what)?.parse().map_err(|_| "bad usize".to_string())
    };

    Ok(match mnemonic {
        "push_int" => Op::PushInt(arg(0, "int")?.parse().map_err(|_| "bad int")?),
        "push_float" => Op::PushFloat(arg(0, "float")?.parse().map_err(|_| "bad float")?),
        "push_bool" => Op::PushBool(rest.first() == Some(&"true")),
        "push_unit" => Op::PushUnit,
        "push_str" => Op::PushStr(u32_arg(0, "idx")?),
        "load_local" => Op::LoadLocal(u32_arg(0, "n")?),
        "store_local" => Op::StoreLocal(u32_arg(0, "n")?),
        "pop" => Op::Pop,
        "add_int" => Op::AddInt,
        "sub_int" => Op::SubInt,
        "mul_int" => Op::MulInt,
        "div_int" => Op::DivInt,
        "rem_int" => Op::RemInt,
        "neg_int" => Op::NegInt,
        "add_float" => Op::AddFloat,
        "sub_float" => Op::SubFloat,
        "mul_float" => Op::MulFloat,
        "div_float" => Op::DivFloat,
        "rem_float" => Op::RemFloat,
        "neg_float" => Op::NegFloat,
        "lt_int" => Op::LtInt,
        "le_int" => Op::LeInt,
        "gt_int" => Op::GtInt,
        "ge_int" => Op::GeInt,
        "lt_float" => Op::LtFloat,
        "le_float" => Op::LeFloat,
        "gt_float" => Op::GtFloat,
        "ge_float" => Op::GeFloat,
        "concat_str" => Op::ConcatStr,
        "make_array" => Op::MakeArray(u32_arg(0, "n")?),
        "new_array" => {
            // `op_text` writes the element's source type name.
            let name = arg(0, "element type")?;
            let elem = Type::from_name(name)
                .and_then(Elem::of)
                .ok_or_else(|| format!("unknown array element type `{name}`"))?;
            Op::NewArray(elem)
        }
        "index" => Op::Index,
        "set_index" => Op::SetIndex,
        "array_len" => Op::ArrayLen,
        "eq" => Op::Eq,
        "ne" => Op::Ne,
        "not_bool" => Op::NotBool,
        "jump" => Op::Jump(usize_arg(0, "target")?),
        "jump_if_false" => Op::JumpIfFalse(usize_arg(0, "target")?),
        "call" => Op::Call {
            func: usize_arg(0, "func")?,
            argc: u32_arg(1, "argc")? as u8,
        },
        "call_builtin" => {
            let name = arg(0, "builtin")?;
            let builtin =
                Builtin::from_name(name).ok_or_else(|| format!("unknown builtin `{name}`"))?;
            Op::CallBuiltin {
                builtin,
                argc: u32_arg(1, "argc")? as u8,
            }
        }
        "return" => Op::Return,
        other => return Err(format!("unknown instruction `{other}`")),
    })
}

/// Quotes `s`, escaping `"`, `\\`, newline, tab, and carriage return.
fn quote(s: &str) -> String {
    let mut out = String::from("\"");
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Parses a string written by [`quote`].
fn unquote(s: &str) -> Result<Rc<str>, String> {
    let inner = s
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .ok_or("unquoted string")?;
    let mut out = String::new();
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some(other) => out.push(other),
                None => return Err("dangling escape".to_string()),
            }
        } else {
            out.push(c);
        }
    }
    Ok(out.into())
}

#[cfg(test)]
mod tests;
