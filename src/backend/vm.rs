//! The virtual machine: a stack-based interpreter for [`Program`] bytecode.
//!
//! # Model
//!
//! One operand [`Value`] stack is shared across calls. Each active call owns a
//! [`Frame`] recording its function, instruction pointer, and `base`  the
//! stack index of its first local. Locals (parameters first) live at
//! `stack[base .. base + n_locals]`; arguments pushed by the caller *become*
//! the leading locals, and the remaining slots are reserved with `unit` on
//! entry. A `Return` discards the callee's slots and leaves the single return
//! value for the caller, preserving the same stack discipline the code
//! generator emits.
//!
//! # Robustness
//!
//! The VM never panics. Genuine program errors (division by zero, integer
//! overflow) become a [`VmError`]; situations that the type checker and code
//! generator make impossible are still handled defensively as
//! [`VmError::Internal`] rather than with `unwrap`. A step limit bounds
//! execution so a runaway loop fails cleanly instead of hanging.
//!
//! Output from `print_*` builtins is captured into a string so execution is
//! deterministic and testable; the driver prints it.

use crate::backend::bytecode::{Op, Program, Value};
use crate::sema::types::Builtin;

/// A recoverable runtime error.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VmError {
    #[error("division by zero")]
    DivisionByZero,
    #[error("integer overflow")]
    IntegerOverflow,
    #[error("array index {index} out of bounds (length {len})")]
    IndexOutOfBounds { index: i64, len: usize },
    #[error("cannot create an array of negative length {0}")]
    NegativeArrayLength(i64),
    #[error("array length {len} exceeds the maximum of {max}")]
    ArrayTooLong { len: i64, max: i64 },
    #[error("execution exceeded the step limit ({0} steps)")]
    StepLimitExceeded(u64),
    /// An invariant the front-end should guarantee was violated. Indicates a
    /// compiler bug, surfaced rather than panicked on.
    #[error("internal VM error: {0}")]
    Internal(&'static str),
}

/// The result of running a program: its final value and captured output.
#[derive(Debug, Clone)]
pub struct Execution {
    /// The value `main` returned (always `unit` for a valid program).
    pub value: Value,
    /// Everything written by `print_*` builtins, in order.
    pub stdout: String,
}

/// Default step budget  generous for real programs, bounded enough that a
/// runaway loop in a test fails fast.
pub const DEFAULT_STEP_LIMIT: u64 = 50_000_000;

/// Largest array a program may allocate at runtime.
///
/// The step budget bounds runaway *execution*; this bounds runaway
/// *allocation*, and for the same reason: without it, `array_new_int` of a
/// huge length asks the allocator for terabytes and the process aborts, which
/// is neither a [`VmError`] nor something a program can recover from. A fixed
/// limit also keeps the outcome identical on every machine, rather than
/// depending on how much memory happens to be free.
pub const MAX_ARRAY_LEN: i64 = 1 << 24;

/// Executes `program` from its entry point with the default step limit.
#[tracing::instrument(level = "debug", skip_all)]
pub fn execute(program: &Program) -> Result<Execution, VmError> {
    execute_with_limit(program, DEFAULT_STEP_LIMIT)
}

/// Executes `program`, failing with [`VmError::StepLimitExceeded`] after
/// `max_steps` instructions.
pub fn execute_with_limit(program: &Program, max_steps: u64) -> Result<Execution, VmError> {
    let mut vm = Vm {
        program,
        stack: Vec::new(),
        frames: Vec::new(),
        stdout: String::new(),
    };
    let value = vm.run(max_steps)?;
    Ok(Execution {
        value,
        stdout: vm.stdout,
    })
}

/// A call frame.
struct Frame {
    func: usize,
    ip: usize,
    base: usize,
}

struct Vm<'a> {
    program: &'a Program,
    stack: Vec<Value>,
    frames: Vec<Frame>,
    stdout: String,
}

impl Vm<'_> {
    fn run(&mut self, max_steps: u64) -> Result<Value, VmError> {
        self.enter(self.program.main, 0)?;
        let mut steps = 0u64;

        while let Some(frame) = self.frames.last() {
            steps += 1;
            if steps > max_steps {
                return Err(VmError::StepLimitExceeded(max_steps));
            }

            let func = frame.func;
            let ip = frame.ip;
            let chunk = &self.program.functions[func];
            // A well-formed chunk always ends in `Return`, so running off the
            // end is an internal error rather than an implicit return.
            let op = chunk
                .code
                .get(ip)
                .ok_or(VmError::Internal("ip out of bounds"))?
                .clone();
            self.top_mut()?.ip += 1;

            if let Some(value) = self.step(op)? {
                // `step` returns `Some` only when the outermost frame returned.
                return Ok(value);
            }
        }
        Ok(Value::Unit)
    }

    /// Executes one instruction. Returns `Some(value)` exactly when the program
    /// has finished (the entry frame returned).
    fn step(&mut self, op: Op) -> Result<Option<Value>, VmError> {
        match op {
            Op::PushInt(v) => self.push(Value::Int(v)),
            Op::PushFloat(v) => self.push(Value::Float(v)),
            Op::PushBool(v) => self.push(Value::Bool(v)),
            Op::PushUnit => self.push(Value::Unit),
            Op::PushStr(idx) => {
                let func = self.top()?.func;
                let s = self.program.functions[func].consts[idx as usize].clone();
                self.push(Value::Str(s));
            }
            Op::LoadLocal(n) => {
                let base = self.top()?.base;
                let v = self
                    .stack
                    .get(base + n as usize)
                    .ok_or(VmError::Internal("bad local"))?
                    .clone();
                self.push(v);
            }
            Op::StoreLocal(n) => {
                let base = self.top()?.base;
                let v = self.pop()?;
                *self
                    .stack
                    .get_mut(base + n as usize)
                    .ok_or(VmError::Internal("bad local"))? = v;
            }
            Op::Pop => {
                self.pop()?;
            }

            Op::AddInt => self.int_binop(|a, b| Ok(a.wrapping_add(b)))?,
            Op::SubInt => self.int_binop(|a, b| Ok(a.wrapping_sub(b)))?,
            Op::MulInt => self.int_binop(|a, b| Ok(a.wrapping_mul(b)))?,
            Op::DivInt => self.int_binop(checked_div)?,
            Op::RemInt => self.int_binop(checked_rem)?,
            Op::NegInt => {
                let a = self.pop_int()?;
                self.push(Value::Int(a.wrapping_neg()));
            }

            Op::AddFloat => self.float_binop(|a, b| a + b)?,
            Op::SubFloat => self.float_binop(|a, b| a - b)?,
            Op::MulFloat => self.float_binop(|a, b| a * b)?,
            Op::DivFloat => self.float_binop(|a, b| a / b)?,
            Op::RemFloat => self.float_binop(|a, b| a % b)?,
            Op::NegFloat => {
                let a = self.pop_float()?;
                self.push(Value::Float(-a));
            }

            Op::LtInt => self.int_cmp(|a, b| a < b)?,
            Op::LeInt => self.int_cmp(|a, b| a <= b)?,
            Op::GtInt => self.int_cmp(|a, b| a > b)?,
            Op::GeInt => self.int_cmp(|a, b| a >= b)?,
            Op::LtFloat => self.float_cmp(|a, b| a < b)?,
            Op::LeFloat => self.float_cmp(|a, b| a <= b)?,
            Op::GtFloat => self.float_cmp(|a, b| a > b)?,
            Op::GeFloat => self.float_cmp(|a, b| a >= b)?,

            Op::Eq => {
                let b = self.pop()?;
                let a = self.pop()?;
                self.push(Value::Bool(a.value_eq(&b)));
            }
            Op::Ne => {
                let b = self.pop()?;
                let a = self.pop()?;
                self.push(Value::Bool(!a.value_eq(&b)));
            }
            Op::NotBool => {
                let a = self.pop_bool()?;
                self.push(Value::Bool(!a));
            }
            Op::ConcatStr => {
                let b = self.pop_str()?;
                let a = self.pop_str()?;
                let joined: String = format!("{a}{b}");
                self.push(Value::Str(joined.into()));
            }
            Op::MakeArray(n) => {
                let at = self
                    .stack
                    .len()
                    .checked_sub(n as usize)
                    .ok_or(stack_underflow())?;
                let items: Vec<Value> = self.stack.split_off(at);
                self.push(Value::Array(std::rc::Rc::new(std::cell::RefCell::new(
                    items,
                ))));
            }
            Op::NewArray(elem) => {
                let len = self.pop_int()?;
                self.push(crate::backend::builtins::new_array(elem, len)?);
            }
            Op::Index => {
                let idx = self.pop_int()?;
                let arr = self.pop_array()?;
                let borrowed = arr.borrow();
                let value = index_in_bounds(idx, borrowed.len())
                    .map(|i| borrowed[i].clone())
                    .ok_or(VmError::IndexOutOfBounds {
                        index: idx,
                        len: borrowed.len(),
                    })?;
                self.push(value);
            }
            Op::SetIndex => {
                let value = self.pop()?;
                let idx = self.pop_int()?;
                let arr = self.pop_array()?;
                let mut borrowed = arr.borrow_mut();
                let len = borrowed.len();
                let i = index_in_bounds(idx, len)
                    .ok_or(VmError::IndexOutOfBounds { index: idx, len })?;
                borrowed[i] = value;
                drop(borrowed);
                self.push(Value::Unit);
            }
            Op::ArrayLen => {
                let arr = self.pop_array()?;
                let len = arr.borrow().len() as i64;
                self.push(Value::Int(len));
            }

            Op::Jump(target) => self.top_mut()?.ip = target,
            Op::JumpIfFalse(target) => {
                if !self.pop_bool()? {
                    self.top_mut()?.ip = target;
                }
            }

            Op::Call { func, argc } => self.enter(func, argc as usize)?,
            Op::CallBuiltin { builtin, argc } => self.call_builtin(builtin, argc as usize)?,
            Op::Return => return self.ret(),
        }
        Ok(None)
    }

    /// Sets up a new call frame for `func`, consuming `argc` arguments already
    /// on the stack as the leading locals.
    fn enter(&mut self, func: usize, argc: usize) -> Result<(), VmError> {
        let chunk = self
            .program
            .functions
            .get(func)
            .ok_or(VmError::Internal("bad function"))?;
        if self.stack.len() < argc {
            return Err(VmError::Internal("not enough arguments on stack"));
        }
        let base = self.stack.len() - argc;
        // Reserve the non-parameter local slots.
        for _ in argc..chunk.n_locals {
            self.stack.push(Value::Unit);
        }
        self.frames.push(Frame { func, ip: 0, base });
        Ok(())
    }

    /// Returns from the current function, discarding its slots and leaving the
    /// return value for the caller. Yields `Some` when the entry frame returns.
    fn ret(&mut self) -> Result<Option<Value>, VmError> {
        let value = self.pop()?;
        let frame = self
            .frames
            .pop()
            .ok_or(VmError::Internal("return with no frame"))?;
        self.stack.truncate(frame.base);
        if self.frames.is_empty() {
            return Ok(Some(value));
        }
        self.push(value);
        Ok(None)
    }

    fn call_builtin(&mut self, builtin: Builtin, argc: usize) -> Result<(), VmError> {
        // Pop arguments (they were pushed left-to-right, so the last is on top).
        let mut args = Vec::with_capacity(argc);
        for _ in 0..argc {
            args.push(self.pop()?);
        }
        args.reverse();
        // Builtins are evaluated by the shared module so both execution engines
        // agree on their behaviour.
        let result = crate::backend::builtins::eval(builtin, &args, &mut self.stdout)?;
        self.push(result);
        Ok(())
    }

    // ---- frame & stack helpers ----

    /// The active call frame. The run loop only steps while a frame exists, so
    /// this is `Err` only on a compiler bug, surfaced rather than panicked on.
    fn top(&self) -> Result<&Frame, VmError> {
        self.frames
            .last()
            .ok_or(VmError::Internal("no active frame"))
    }

    fn top_mut(&mut self) -> Result<&mut Frame, VmError> {
        self.frames
            .last_mut()
            .ok_or(VmError::Internal("no active frame"))
    }

    fn push(&mut self, v: Value) {
        self.stack.push(v);
    }

    fn pop(&mut self) -> Result<Value, VmError> {
        self.stack.pop().ok_or(VmError::Internal("stack underflow"))
    }

    fn pop_int(&mut self) -> Result<i64, VmError> {
        match self.pop()? {
            Value::Int(v) => Ok(v),
            _ => Err(VmError::Internal("expected int")),
        }
    }

    fn pop_float(&mut self) -> Result<f64, VmError> {
        match self.pop()? {
            Value::Float(v) => Ok(v),
            _ => Err(VmError::Internal("expected float")),
        }
    }

    fn pop_str(&mut self) -> Result<std::rc::Rc<str>, VmError> {
        match self.pop()? {
            Value::Str(v) => Ok(v),
            _ => Err(VmError::Internal("expected str")),
        }
    }

    fn pop_array(&mut self) -> Result<crate::backend::bytecode::Array, VmError> {
        match self.pop()? {
            Value::Array(a) => Ok(a),
            _ => Err(VmError::Internal("expected array")),
        }
    }

    fn pop_bool(&mut self) -> Result<bool, VmError> {
        match self.pop()? {
            Value::Bool(v) => Ok(v),
            _ => Err(VmError::Internal("expected bool")),
        }
    }

    fn int_binop(&mut self, f: impl Fn(i64, i64) -> Result<i64, VmError>) -> Result<(), VmError> {
        let b = self.pop_int()?;
        let a = self.pop_int()?;
        self.push(Value::Int(f(a, b)?));
        Ok(())
    }

    fn int_cmp(&mut self, f: impl Fn(i64, i64) -> bool) -> Result<(), VmError> {
        let b = self.pop_int()?;
        let a = self.pop_int()?;
        self.push(Value::Bool(f(a, b)));
        Ok(())
    }

    fn float_binop(&mut self, f: impl Fn(f64, f64) -> f64) -> Result<(), VmError> {
        let b = self.pop_float()?;
        let a = self.pop_float()?;
        self.push(Value::Float(f(a, b)));
        Ok(())
    }

    fn float_cmp(&mut self, f: impl Fn(f64, f64) -> bool) -> Result<(), VmError> {
        let b = self.pop_float()?;
        let a = self.pop_float()?;
        self.push(Value::Bool(f(a, b)));
        Ok(())
    }
}

/// Converts a signed index to an in-bounds `usize`, or `None` if out of range
/// (negative or `>= len`). Shared with the builtin evaluator.
pub(crate) fn index_in_bounds(index: i64, len: usize) -> Option<usize> {
    if index < 0 {
        return None;
    }
    let i = index as usize;
    (i < len).then_some(i)
}

fn stack_underflow() -> VmError {
    VmError::Internal("stack underflow")
}

fn checked_div(a: i64, b: i64) -> Result<i64, VmError> {
    if b == 0 {
        Err(VmError::DivisionByZero)
    } else {
        a.checked_div(b).ok_or(VmError::IntegerOverflow)
    }
}

fn checked_rem(a: i64, b: i64) -> Result<i64, VmError> {
    if b == 0 {
        Err(VmError::DivisionByZero)
    } else {
        a.checked_rem(b).ok_or(VmError::IntegerOverflow)
    }
}
