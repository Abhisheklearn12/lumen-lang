//! MIR, the mid-level IR: each function is a control-flow graph of basic
//! blocks in three-address form.
//!
//! HIR is a tree, which suits code generation but not data-flow analysis. Here
//! a [`Function`] is a list of [`Block`]s, each a straight line of [`Inst`]s
//! ending in a [`Terminator`]. Nested expressions become single-assignment
//! registers ([`Reg`]), and `if`, loops, and `&&`/`||` become explicit
//! branches, the form the [optimizer](opt) works on.
//!
//! MIR is not on the path from source to bytecode. `lumenc dump mir` and
//! `dump cfg` show it, and its [interpreter](interp) cross-checks the VM.

pub mod build;
pub mod dot;
pub mod interp;
pub mod opt;
pub mod print;

pub use build::build;
pub use dot::to_dot;
pub use interp::interpret;
pub use opt::{MirStats, optimize};
pub use print::print_mir;

use crate::hir::{BinOp, Callee, LocalDecl, UnOp};
use crate::sema::types::Type;
use std::rc::Rc;

/// A block's index in [`Function::blocks`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BlockId(pub u32);

/// A virtual register, written by at most one instruction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Reg(pub u32);

/// Local slots are numbered as in HIR.
pub use crate::hir::LocalId;

/// A program in MIR form.
#[derive(Debug)]
pub struct Program {
    pub functions: Vec<Function>,
    /// The index of `main` in [`Program::functions`].
    pub main: usize,
}

/// A function: its locals and control-flow graph.
#[derive(Debug)]
pub struct Function {
    pub name: String,
    pub param_count: usize,
    pub locals: Vec<LocalDecl>,
    /// The number of registers used.
    pub reg_count: usize,
    pub blocks: Vec<Block>,
    pub entry: BlockId,
}

impl Function {
    pub fn block(&self, id: BlockId) -> &Block {
        &self.blocks[id.0 as usize]
    }

    pub fn block_mut(&mut self, id: BlockId) -> &mut Block {
        &mut self.blocks[id.0 as usize]
    }
}

/// A basic block: straight-line instructions, then a terminator.
#[derive(Debug)]
pub struct Block {
    pub insts: Vec<Inst>,
    pub term: Terminator,
}

/// A constant operand.
#[derive(Debug, Clone, PartialEq)]
pub enum Const {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(Rc<str>),
    Unit,
}

/// An instruction operand: a constant or a register.
#[derive(Debug, Clone, PartialEq)]
pub enum Operand {
    Const(Const),
    Reg(Reg),
}

impl Operand {
    /// The register read, if any.
    pub fn reg(&self) -> Option<Reg> {
        match self {
            Operand::Reg(r) => Some(*r),
            Operand::Const(_) => None,
        }
    }
}

/// A computation whose result is written to a register.
#[derive(Debug, Clone)]
pub enum Rvalue {
    /// A copy of an operand.
    Use(Operand),
    /// Reads a local.
    Load(LocalId),
    Unary(UnOp, Operand),
    Binary(BinOp, Operand, Operand),
    /// String concatenation, printed `a ++ b`.
    Concat(Operand, Operand),
    /// A new array, which also represents structs and tuples.
    MakeArray(Vec<Operand>),
    /// `base[index]`, which also reads struct and tuple fields.
    Index(Operand, Operand),
}

/// An instruction. All but the two stores write a register.
#[derive(Debug)]
pub enum Inst {
    /// `dst = rvalue`
    Assign { dst: Reg, rvalue: Rvalue },
    /// `local = src`
    Store { local: LocalId, src: Operand },
    /// `base[index] = value`
    SetIndex {
        base: Operand,
        index: Operand,
        value: Operand,
    },
    /// `dst = callee(args)`; may have side effects.
    Call {
        dst: Reg,
        callee: Callee,
        args: Vec<Operand>,
        ret: Type,
    },
}

/// How a block ends.
#[derive(Debug, Clone)]
pub enum Terminator {
    Goto(BlockId),
    /// Goes to `then_bb` if `cond` is true, else to `else_bb`.
    Branch {
        cond: Operand,
        then_bb: BlockId,
        else_bb: BlockId,
    },
    Return(Operand),
    /// Not yet terminated; only seen while MIR is being built.
    Unreachable,
}

impl Terminator {
    /// The blocks control may go to next.
    pub fn successors(&self) -> Vec<BlockId> {
        match self {
            Terminator::Goto(b) => vec![*b],
            Terminator::Branch {
                then_bb, else_bb, ..
            } => vec![*then_bb, *else_bb],
            Terminator::Return(_) | Terminator::Unreachable => vec![],
        }
    }
}

#[cfg(test)]
mod tests;
