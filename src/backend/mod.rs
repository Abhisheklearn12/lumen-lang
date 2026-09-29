//! The backend: bytecode, its compiler and VM, and a C emitter.
//!
//! * [`bytecode`]: [`Value`], the [`Op`] instruction set, and [`Program`].
//! * [`codegen`]: HIR to bytecode.
//! * [`peephole`]: bytecode clean-up after codegen.
//! * [`vm`]: runs a [`Program`].
//! * [`builtins`]: the builtin functions, shared with the MIR interpreter.
//! * [`mod@verify`]: checks untrusted bytecode before it runs.
//! * [`object`]: a text format for saving and loading programs.
//! * [`disasm`]: a readable bytecode listing.
//! * [`c`]: HIR to C, for the scalar subset of the language.
//!
//! It reads only [HIR](crate::hir) and the [types](crate::sema::types), never
//! the AST, tokens, or diagnostics.

pub mod builtins;
pub mod bytecode;
pub mod c;
pub mod codegen;
pub mod disasm;
pub mod object;
pub mod peephole;
pub mod verify;
pub mod vm;

pub use bytecode::{Op, Program, Value};
pub use c::{CError, emit_c};
pub use codegen::generate;
pub use disasm::disassemble;
pub use verify::{VerifyError, verify};
pub use vm::{Execution, VmError, execute, execute_with_limit};

#[cfg(test)]
mod tests;
