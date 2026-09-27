//! The bytecode: runtime [`Value`]s, the [`Op`] instruction set, and the
//! [`Program`] the VM runs.
//!
//! Instructions are an `enum`, not packed bytes: easier to read, match, and
//! test, at some cost in decode speed. Arithmetic and ordering opcodes are
//! typed (`AddInt`, `AddFloat`), chosen by the code generator from HIR types.
//! Only [`Op::Eq`] and [`Op::Ne`] compare any two values. Jump targets are
//! absolute indices into the chunk's code.

use std::cell::RefCell;
use std::rc::Rc;

use crate::sema::types::{Builtin, Elem};

/// An array: shared and mutable, so copies alias.
pub type Array = Rc<RefCell<Vec<Value>>>;

/// A runtime value. Strings and arrays are reference-counted, so cloning one
/// is cheap. Structs and tuples are arrays of their fields.
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
    /// Whether this is `Bool(true)`; any other value counts as `false`.
    pub fn as_bool(&self) -> bool {
        matches!(self, Value::Bool(true))
    }

    /// The equality behind `==`: IEEE for floats, by contents for strings and
    /// arrays.
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

/// A VM instruction. Stack effects are written `[before] -> [after]`, top of
/// stack on the right.
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
    /// Pushes `consts[idx]`. `[] -> [str]`
    PushStr(u32),

    /// Pushes local `n`. `[] -> [v]`
    LoadLocal(u32),
    /// Pops into local `n`. `[v] -> []`
    StoreLocal(u32),
    /// `[v] -> []`
    Pop,

    // Integer arithmetic: `[a, b] -> [a op b]`.
    AddInt,
    SubInt,
    MulInt,
    DivInt,
    RemInt,
    /// `[a] -> [-a]`
    NegInt,

    // Float arithmetic, likewise.
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

    // Float ordering, likewise.
    LtFloat,
    LeFloat,
    GtFloat,
    GeFloat,

    /// `[str, str] -> [str]`
    ConcatStr,

    /// Builds an array of the top `n` values. `[v0 .. vn-1] -> [array]`
    MakeArray(u32),
    /// An array of `len` zero values; fails on a negative or oversized `len`.
    /// `[len] -> [array]`
    ///
    /// Unlike a variable-count [`Op::MakeArray`], its stack effect does not
    /// depend on the runtime length, so the verifier can still prove the stack
    /// height at every instruction.
    NewArray(Elem),
    /// Fails if out of bounds. `[array, int] -> [v]`
    Index,
    /// Fails if out of bounds. `[array, int, v] -> [unit]`
    SetIndex,
    /// `[array] -> [int]`
    ArrayLen,

    /// `[a, b] -> [bool]`
    Eq,
    /// `[a, b] -> [bool]`
    Ne,
    /// `[bool] -> [bool]`
    NotBool,

    /// Jumps to an absolute index.
    Jump(usize),
    /// Pops a bool and jumps if it is `false`. `[bool] -> []`
    JumpIfFalse(usize),

    /// Calls function `func`, whose parameters are the top `argc` values.
    /// `[args..] -> [result]`
    Call {
        func: usize,
        argc: u8,
    },
    /// `[args..] -> [result]`
    CallBuiltin {
        builtin: Builtin,
        argc: u8,
    },
    /// Returns the top value to the caller.
    Return,
}

impl Op {
    /// The target of a `Jump` or `JumpIfFalse`.
    pub fn jump_target(&self) -> Option<usize> {
        match self {
            Op::Jump(t) | Op::JumpIfFalse(t) => Some(*t),
            _ => None,
        }
    }

    /// The target of a `Jump` or `JumpIfFalse`, for patching.
    pub fn jump_target_mut(&mut self) -> Option<&mut usize> {
        match self {
            Op::Jump(t) | Op::JumpIfFalse(t) => Some(t),
            _ => None,
        }
    }
}

/// One compiled function.
#[derive(Debug)]
pub struct Chunk {
    pub name: String,
    /// Local slots, including the parameters.
    pub n_locals: usize,
    /// How many leading locals are parameters.
    pub n_params: usize,
    pub code: Vec<Op>,
    /// The strings [`Op::PushStr`] refers to.
    pub consts: Vec<Rc<str>>,
}

/// A compiled program.
#[derive(Debug)]
pub struct Program {
    /// One chunk per function, indexed like [`Hir::functions`](crate::hir::Hir::functions).
    pub functions: Vec<Chunk>,
    /// The index of `main` in `functions`.
    pub main: usize,
}
