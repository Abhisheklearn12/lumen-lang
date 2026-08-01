//! A bytecode verifier.
//!
//! The code generator only ever emits well-formed bytecode, but a [`Program`]
//! can also arrive from an *untrusted* source: an object file loaded with
//! [`object::from_text`](crate::backend::object::from_text) may have been
//! hand-edited or corrupted. Running such a program directly could make the VM
//! index past the end of the stack, jump to a bogus address, or read a local
//! that does not exist. [`verify`] rejects those programs up front with a clear
//! diagnostic instead.
//!
//! # What is checked
//!
//! The verifier abstractly interprets each function, tracking only the *height*
//! of the operand stack rather than concrete values. Starting from an empty
//! stack at the entry, it walks every reachable instruction along both edges of
//! each conditional jump and proves:
//!
//! * **No underflow.** Every instruction has enough operands beneath it.
//! * **Consistent merges.** Two control-flow paths that reach the same
//!   instruction agree on the stack height, so the height is a function of the
//!   program counter alone (the property the VM relies on).
//! * **In-range operands.** Local slots, string constants, jump targets, and
//!   call targets all refer to something that exists, and a call passes exactly
//!   as many arguments as the callee declares.
//! * **No fall-through off the end.** Control never runs past the last
//!   instruction; every path ends at a `return`.
//!
//! Verification is linear in the number of instructions: each is processed at
//! most once, when its stack height first becomes known.

use crate::backend::bytecode::{Op, Program};
use crate::sema::types::Builtin;

/// A reason a program was rejected, naming the offending function and offset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyError {
    /// Index of the function in [`Program::functions`].
    pub func: usize,
    /// Name of the function, for a readable message.
    pub name: String,
    /// Instruction offset at which the problem was found.
    pub pc: usize,
    pub kind: VerifyErrorKind,
}

/// The specific malformedness the verifier detected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyErrorKind {
    /// An instruction needed more operands than the stack held.
    StackUnderflow { needed: usize, found: usize },
    /// Two paths reached one instruction with differing stack heights.
    InconsistentStack { expected: usize, found: usize },
    /// A jump or fall-through left the bounds of the code.
    JumpOutOfRange { target: usize },
    /// A local slot index was not within the function's locals.
    BadLocal { slot: u32, locals: usize },
    /// A string-constant index was not within the constant pool.
    BadConst { index: u32, consts: usize },
    /// A call named a function index that does not exist.
    BadCallTarget { target: usize, functions: usize },
    /// A call passed the wrong number of arguments for the callee.
    ArityMismatch { expected: usize, found: usize },
    /// A builtin call passed the wrong number of arguments.
    BuiltinArity {
        builtin: &'static str,
        expected: usize,
        found: usize,
    },
    /// Control reached the end of the function without returning.
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

/// Verifies an entire program, returning the first problem found (if any).
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
    for (i, _) in program.functions.iter().enumerate() {
        verify_chunk(program, i)?;
    }
    Ok(())
}

/// The operand-stack effect of an instruction: the height it requires beneath
/// it, and the signed change it applies.
struct Effect {
    needed: usize,
    delta: isize,
}

fn effect(op: &Op) -> Effect {
    use Op::*;
    let bin = Effect {
        needed: 2,
        delta: -1,
    };
    let unary = Effect {
        needed: 1,
        delta: 0,
    };
    match op {
        PushInt(_) | PushFloat(_) | PushBool(_) | PushUnit | PushStr(_) | LoadLocal(_) => Effect {
            needed: 0,
            delta: 1,
        },
        StoreLocal(_) | Pop => Effect {
            needed: 1,
            delta: -1,
        },
        AddInt | SubInt | MulInt | DivInt | RemInt => bin,
        AddFloat | SubFloat | MulFloat | DivFloat | RemFloat => bin,
        LtInt | LeInt | GtInt | GeInt => bin,
        LtFloat | LeFloat | GtFloat | GeFloat => bin,
        ConcatStr | Eq | Ne => bin,
        // `NewArray` consumes a length and produces an array, so it is
        // one-in one-out however long the array turns out to be.
        NegInt | NegFloat | NotBool | ArrayLen | NewArray(_) => unary,
        MakeArray(n) => Effect {
            needed: *n as usize,
            delta: 1 - *n as isize,
        },
        Index => Effect {
            needed: 2,
            delta: -1,
        },
        SetIndex => Effect {
            needed: 3,
            delta: -2,
        },
        Jump(_) => Effect {
            needed: 0,
            delta: 0,
        },
        JumpIfFalse(_) => Effect {
            needed: 1,
            delta: -1,
        },
        Call { argc, .. } | CallBuiltin { argc, .. } => Effect {
            needed: *argc as usize,
            delta: 1 - *argc as isize,
        },
        Return => Effect {
            needed: 1,
            delta: -1,
        },
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

    // `heights[pc]` is the proven stack height on entry to instruction `pc`,
    // once known. A worklist visits each instruction the first time its height
    // is established and checks any later arrival agrees.
    let mut heights: Vec<Option<usize>> = vec![None; n];
    let mut work: Vec<(usize, usize)> = Vec::new();

    // Pushes a successor edge, validating range and merge consistency.
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
        let Effect { needed, delta } = effect(op);
        if height < needed {
            return Err(err(
                pc,
                VerifyErrorKind::StackUnderflow {
                    needed,
                    found: height,
                },
            ));
        }
        // Operand-table bounds, independent of stack height.
        check_operands(program, chunk, op, pc, &err)?;

        let next_height = (height as isize + delta) as usize;
        match op {
            Op::Return => {} // terminates this path
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

/// Checks the index/arity operands an instruction carries.
fn check_operands(
    program: &Program,
    chunk: &crate::backend::bytecode::Chunk,
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
            // `Len` is variadic over array element types but still unary.
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

/// The number of arguments a builtin expects.
fn builtin_arity(builtin: Builtin) -> usize {
    // `Len` reports no fixed parameter types (it is generic over the array
    // element) yet always takes exactly one argument; everything else is the
    // length of its declared parameter list.
    if builtin.is_generic() {
        1
    } else {
        builtin.params().len()
    }
}

#[cfg(test)]
mod tests;
