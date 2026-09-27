//! A bytecode verifier for programs from untrusted sources, such as object
//! files loaded by [`object::from_text`](crate::backend::object::from_text).
//!
//! It follows every reachable path through each function, tracking only the
//! stack height, and proves that:
//!
//! * no instruction pops more values than the stack holds;
//! * paths that meet agree on the height, so it depends only on the position;
//! * locals, string constants, jump targets, and callees exist, and every call
//!   passes the callee's parameter count;
//! * no path runs past the last instruction.
//!
//! An empty function is accepted, though the VM fails if it is called.
//! Each instruction is checked once, so verification is linear. Types are not
//! checked; the VM reports a type confusion as [`VmError::Internal`](crate::backend::VmError::Internal).

use crate::backend::bytecode::{Chunk, Op, Program};
use crate::sema::types::Builtin;

/// Why a program was rejected, and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyError {
    /// The function's index in [`Program::functions`].
    pub func: usize,
    pub name: String,
    /// The offending instruction's index.
    pub pc: usize,
    pub kind: VerifyErrorKind,
}

/// What is wrong with a rejected program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyErrorKind {
    StackUnderflow {
        needed: usize,
        found: usize,
    },
    /// Two paths reach one instruction with different stack heights.
    InconsistentStack {
        expected: usize,
        found: usize,
    },
    /// A jump target, or a `JumpIfFalse`'s fall-through, is past the end.
    JumpOutOfRange {
        target: usize,
    },
    BadLocal {
        slot: u32,
        locals: usize,
    },
    BadConst {
        index: u32,
        consts: usize,
    },
    /// A call to a function, or an entry point, that does not exist.
    BadCallTarget {
        target: usize,
        functions: usize,
    },
    ArityMismatch {
        expected: usize,
        found: usize,
    },
    BuiltinArity {
        builtin: &'static str,
        expected: usize,
        found: usize,
    },
    /// Control can run off the end of the code.
    FellOffEnd,
}

impl std::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use VerifyErrorKind::*;
        write!(f, "in `{}` at offset {}: ", self.name, self.pc)?;
        match &self.kind {
            StackUnderflow { needed, found } => {
                write!(f, "stack underflow (needed {needed}, had {found})")
            }
            InconsistentStack { expected, found } => {
                write!(f, "inconsistent stack height ({expected} vs {found})")
            }
            JumpOutOfRange { target } => write!(f, "jump target {target} is out of range"),
            BadLocal { slot, locals } => {
                write!(f, "local slot {slot} out of range (function has {locals})")
            }
            BadConst { index, consts } => {
                write!(
                    f,
                    "string constant {index} out of range (pool has {consts})"
                )
            }
            BadCallTarget { target, functions } => {
                write!(
                    f,
                    "call target {target} out of range ({functions} functions)"
                )
            }
            ArityMismatch { expected, found } => {
                write!(
                    f,
                    "call passes {found} arguments but callee takes {expected}"
                )
            }
            BuiltinArity {
                builtin,
                expected,
                found,
            } => {
                write!(
                    f,
                    "builtin `{builtin}` takes {expected} arguments, got {found}"
                )
            }
            FellOffEnd => write!(f, "control runs past the end without returning"),
        }
    }
}

impl std::error::Error for VerifyError {}

/// Verifies every function, returning the first problem found.
pub fn verify(program: &Program) -> Result<(), VerifyError> {
    if program.main >= program.functions.len() {
        return Err(VerifyError {
            func: program.main,
            name: "<entry>".to_string(),
            pc: 0,
            kind: VerifyErrorKind::BadCallTarget {
                target: program.main,
                functions: program.functions.len(),
            },
        });
    }
    for i in 0..program.functions.len() {
        verify_chunk(program, i)?;
    }
    Ok(())
}

/// How many values `op` pops, then pushes.
fn stack_effect(op: &Op) -> (usize, usize) {
    use Op::*;
    match op {
        PushInt(_) | PushFloat(_) | PushBool(_) | PushUnit | PushStr(_) | LoadLocal(_) => (0, 1),
        StoreLocal(_) | Pop | JumpIfFalse(_) | Return => (1, 0),
        NegInt | NegFloat | NotBool | ArrayLen | NewArray(_) => (1, 1),
        AddInt | SubInt | MulInt | DivInt | RemInt | AddFloat | SubFloat | MulFloat | DivFloat
        | RemFloat | LtInt | LeInt | GtInt | GeInt | LtFloat | LeFloat | GtFloat | GeFloat
        | ConcatStr | Eq | Ne | Index => (2, 1),
        SetIndex => (3, 1),
        MakeArray(n) => (*n as usize, 1),
        Call { argc, .. } | CallBuiltin { argc, .. } => (*argc as usize, 1),
        Jump(_) => (0, 0),
    }
}

fn verify_chunk(program: &Program, func: usize) -> Result<(), VerifyError> {
    let chunk = &program.functions[func];
    let code = &chunk.code;
    let n = code.len();
    let err = |pc: usize, kind: VerifyErrorKind| VerifyError {
        func,
        name: chunk.name.clone(),
        pc,
        kind,
    };

    // `heights[pc]` is the stack height on entry to `pc`, once known. An
    // instruction is queued when its height is first set; later arrivals must
    // agree.
    let mut heights: Vec<Option<usize>> = vec![None; n];
    let mut work: Vec<(usize, usize)> = Vec::new();

    // Records that control can go from `from` to `target` at `height`.
    let push_edge = |target: usize,
                     height: usize,
                     from: usize,
                     work: &mut Vec<(usize, usize)>,
                     heights: &mut Vec<Option<usize>>|
     -> Result<(), VerifyError> {
        if target >= n {
            return Err(err(from, VerifyErrorKind::JumpOutOfRange { target }));
        }
        match heights[target] {
            Some(existing) if existing != height => Err(err(
                target,
                VerifyErrorKind::InconsistentStack {
                    expected: existing,
                    found: height,
                },
            )),
            Some(_) => Ok(()),
            None => {
                heights[target] = Some(height);
                work.push((target, height));
                Ok(())
            }
        }
    };

    if n == 0 {
        return Ok(());
    }
    heights[0] = Some(0);
    work.push((0, 0));

    while let Some((pc, height)) = work.pop() {
        let op = &code[pc];
        let (pops, pushes) = stack_effect(op);
        if height < pops {
            return Err(err(
                pc,
                VerifyErrorKind::StackUnderflow {
                    needed: pops,
                    found: height,
                },
            ));
        }
        check_operands(program, chunk, op, pc, &err)?;

        let next_height = height - pops + pushes;
        match op {
            Op::Return => {}
            Op::Jump(target) => push_edge(*target, next_height, pc, &mut work, &mut heights)?,
            Op::JumpIfFalse(target) => {
                push_edge(*target, next_height, pc, &mut work, &mut heights)?;
                push_edge(pc + 1, next_height, pc, &mut work, &mut heights)?;
            }
            _ => {
                if pc + 1 >= n {
                    return Err(err(pc, VerifyErrorKind::FellOffEnd));
                }
                push_edge(pc + 1, next_height, pc, &mut work, &mut heights)?;
            }
        }
    }
    Ok(())
}

/// Checks the indices and argument counts `op` carries.
fn check_operands(
    program: &Program,
    chunk: &Chunk,
    op: &Op,
    pc: usize,
    err: &impl Fn(usize, VerifyErrorKind) -> VerifyError,
) -> Result<(), VerifyError> {
    match op {
        Op::LoadLocal(slot) | Op::StoreLocal(slot) => {
            if *slot as usize >= chunk.n_locals {
                return Err(err(
                    pc,
                    VerifyErrorKind::BadLocal {
                        slot: *slot,
                        locals: chunk.n_locals,
                    },
                ));
            }
        }
        Op::PushStr(index) => {
            if *index as usize >= chunk.consts.len() {
                return Err(err(
                    pc,
                    VerifyErrorKind::BadConst {
                        index: *index,
                        consts: chunk.consts.len(),
                    },
                ));
            }
        }
        Op::Call { func, argc } => {
            let Some(callee) = program.functions.get(*func) else {
                return Err(err(
                    pc,
                    VerifyErrorKind::BadCallTarget {
                        target: *func,
                        functions: program.functions.len(),
                    },
                ));
            };
            if callee.n_params != *argc as usize {
                return Err(err(
                    pc,
                    VerifyErrorKind::ArityMismatch {
                        expected: callee.n_params,
                        found: *argc as usize,
                    },
                ));
            }
        }
        Op::CallBuiltin { builtin, argc } => {
            let expected = builtin_arity(*builtin);
            if expected != *argc as usize {
                return Err(err(
                    pc,
                    VerifyErrorKind::BuiltinArity {
                        builtin: Builtin::name(*builtin),
                        expected,
                        found: *argc as usize,
                    },
                ));
            }
        }
        _ => {}
    }
    Ok(())
}

/// How many arguments `builtin` takes. `len`, the one generic builtin, has no
/// parameter list but takes one argument.
fn builtin_arity(builtin: Builtin) -> usize {
    if builtin.is_generic() {
        1
    } else {
        builtin.params().len()
    }
}

#[cfg(test)]
mod tests;
