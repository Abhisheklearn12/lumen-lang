//! The bytecode: runtime [`Value`]s, the instruction set [`Op`], and the
//! [`Program`]/[`Chunk`] container the VM executes.
//!
//! # Design
//!
//! Instructions are a typed `enum` rather than packed bytes. The language
//! prioritises maintainability over raw decode speed, and a typed instruction
//! set is far easier to read, pattern-match, disassemble, and test than a byte
//! buffer  while still being a flat, index-addressed sequence the VM steps
//! through linearly.
//!
//! Because HIR is fully typed, arithmetic and comparison opcodes are
//! *monomorphic* (`AddInt` vs `AddFloat`): the code generator picks the right
//! one, so the VM never inspects operand types at runtime. Equality is the one
//! exception  a single [`Op::Eq`]/[`Op::Ne`] compares any two values
//! structurally, which is simpler than four typed variants and just as fast.
//!
//! Jumps are absolute instruction indices within a chunk, resolved by the code
//! generator via backpatching.

use std::cell::RefCell;
use std::rc::Rc;

use crate::sema::types::{Builtin, Elem};

/// A shared, mutable array value with reference semantics.
pub type Array = Rc<RefCell<Vec<Value>>>;

/// A runtime value. Strings and arrays are reference-counted so cloning a
/// `Value` (which the stack machine does constantly) is cheap. Arrays share
/// their contents, matching the language's reference semantics.
#[derive(Clone, Debug)]
pub enum Value {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(Rc<str>),
    Array(Array),
    Unit,
}

impl Value {
    /// Interprets the value as a boolean for conditional branches. Only `Bool`
    /// is ever produced in this position by the type checker; anything else
    /// (impossible in well-typed code) is treated as `false`.
    pub fn as_bool(&self) -> bool {
        matches!(self, Value::Bool(true))
    }

    /// Structural equality used by `Op::Eq`/`Op::Ne`. Mirrors source `==`:
    /// floats use IEEE equality, strings compare by contents.
    pub fn value_eq(&self, other: &Value) -> bool {
        match (self, other) {
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Float(a), Value::Float(b)) => a == b,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Str(a), Value::Str(b)) => a == b,
            (Value::Array(a), Value::Array(b)) => {
                let (a, b) = (a.borrow(), b.borrow());
                a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| x.value_eq(y))
            }
            (Value::Unit, Value::Unit) => true,
            _ => false,
        }
    }
}

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Int(v) => write!(f, "{v}"),
            Value::Float(v) => write!(f, "{v}"),
            Value::Bool(v) => write!(f, "{v}"),
            Value::Str(v) => write!(f, "{v}"),
            Value::Array(items) => {
                f.write_str("[")?;
                for (i, v) in items.borrow().iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{v}")?;
                }
                f.write_str("]")
            }
            Value::Unit => write!(f, "unit"),
        }
    }
}

/// A single VM instruction. Each is documented with its stack effect as
/// `[before] -> [after]` (top of stack on the right).
#[derive(Clone, Debug)]
pub enum Op {
    /// `[] -> [int]`
    PushInt(i64),
    /// `[] -> [float]`
    PushFloat(f64),
    /// `[] -> [bool]`
    PushBool(bool),
    /// `[] -> [unit]`
    PushUnit,
    /// Push string constant `consts[idx]`. `[] -> [str]`
    PushStr(u32),

    /// Push a copy of local slot `n`. `[] -> [v]`
    LoadLocal(u32),
    /// Pop and store into local slot `n`. `[v] -> []`
    StoreLocal(u32),
    /// Discard the top value. `[v] -> []`
    Pop,

    // Integer arithmetic: `[a, b] -> [a op b]`.
    AddInt,
    SubInt,
    MulInt,
    DivInt,
    RemInt,
    /// `[a] -> [-a]`
    NegInt,

    // Float arithmetic.
    AddFloat,
    SubFloat,
    MulFloat,
    DivFloat,
    RemFloat,
    NegFloat,

    // Integer ordering: `[a, b] -> [bool]`.
    LtInt,
    LeInt,
    GtInt,
    GeInt,

    // Float ordering.
    LtFloat,
    LeFloat,
    GtFloat,
    GeFloat,

    /// String concatenation. `[str, str] -> [str]`
    ConcatStr,

    /// Build an array from the top `n` values. `[v0..vn-1] -> [array]`
    MakeArray(u32),
    /// Pop a length and push an array of that many zero values of the given
    /// element type. `[int] -> [array]` (errors on a negative or oversized
    /// length).
    ///
    /// The length is a *runtime* value, but the stack effect is one-in
    /// one-out regardless of it, so the verifier can still prove stack height
    /// from the program counter alone. That is why this is a separate opcode
    /// rather than a variable-count [`Op::MakeArray`].
    NewArray(Elem),
    /// Read `base[index]`. `[array, int] -> [v]` (errors if out of bounds).
    Index,
    /// Store `base[index] = value`, yielding unit. `[array, int, v] -> [unit]`
    SetIndex,
    /// Push the length of an array. `[array] -> [int]`
    ArrayLen,

    /// Structural equality. `[a, b] -> [bool]`
    Eq,
    /// Structural inequality. `[a, b] -> [bool]`
    Ne,
    /// Boolean negation. `[bool] -> [bool]`
    NotBool,

    /// Unconditional jump to absolute index.
    Jump(usize),
    /// Pop a bool; jump to absolute index if it is `false`. `[bool] -> []`
    JumpIfFalse(usize),

    /// Call user function `func` with `argc` arguments from the stack top.
    Call {
        func: usize,
        argc: u8,
    },
    /// Call a builtin with `argc` arguments. `[args..] -> [result]`
    CallBuiltin {
        builtin: Builtin,
        argc: u8,
    },
    /// Return the top value to the caller.
    Return,
}

/// The compiled form of one function.
#[derive(Debug)]
pub struct Chunk {
    pub name: String,
    /// Total local slots, including parameters.
    pub n_locals: usize,
    /// How many of the locals are parameters (the leading slots).
    pub n_params: usize,
    pub code: Vec<Op>,
    /// String constant pool referenced by [`Op::PushStr`].
    pub consts: Vec<Rc<str>>,
}

/// A whole compiled program: one [`Chunk`] per function plus the entry index.
#[derive(Debug)]
pub struct Program {
    pub functions: Vec<Chunk>,
    /// Index into [`Program::functions`] of `main`.
    pub main: usize,
}
