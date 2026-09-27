//! The virtual machine: a stack interpreter for [`Program`] bytecode.
//!
//! All calls share one [`Value`] stack. A call's locals, parameters first,
//! occupy `stack[base .. base + n_locals]`: the caller's pushed arguments
//! become the parameters, and the other slots start as `unit`. `Return` drops
//! the callee's slots and leaves its result for the caller.
//!
//! Program faults (division by zero, a bad index, ...) are [`VmError`]s, and
//! states the compiler rules out are [`VmError::Internal`], never panics. A
//! step limit stops runaway loops. Printed output is collected in a string,
//! which the driver prints.

use std::cell::RefCell;
use std::rc::Rc;

use crate::backend::builtins;
use crate::backend::bytecode::{Array, Op, Program, Value};
use crate::sema::types::Builtin;

/// A runtime error.
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
    /// A state the compiler should rule out: a compiler bug, or unverified
    /// bytecode.
    #[error("internal VM error: {0}")]
    Internal(&'static str),
}

/// A finished run.
#[derive(Debug, Clone)]
pub struct Execution {
    /// What `main` returned: `unit` for a valid program.
    pub value: Value,
    /// Everything the `print_*` builtins wrote.
    pub stdout: String,
}

/// The default step budget: ample for real programs, small enough that a
/// runaway loop fails fast.
pub const DEFAULT_STEP_LIMIT: u64 = 50_000_000;

/// The longest array a program may allocate.
///
/// Without it, `array_new_int` of a huge length would make the allocator abort
/// the process instead of failing with a [`VmError`]. A fixed limit also gives
/// the same result on every machine, whatever memory is free.
pub const MAX_ARRAY_LEN: i64 = 1 << 24;

/// Runs `program` from `main` with the default step limit.
#[tracing::instrument(level = "debug", skip_all)]
pub fn execute(program: &Program) -> Result<Execution, VmError> {
    execute_with_limit(program, DEFAULT_STEP_LIMIT)
}

/// Runs `program`, failing with [`VmError::StepLimitExceeded`] after
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

/// An active call.
struct Frame {
    func: usize,
    ip: usize,
    /// The stack index of local 0.
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
            // Well-formed code never runs past its last `Return`.
            let op = chunk
                .code
                .get(ip)
                .ok_or(VmError::Internal("ip out of bounds"))?
                .clone();
            self.top_mut()?.ip += 1;

            if let Some(value) = self.step(op)? {
                return Ok(value);
            }
        }
        Ok(Value::Unit)
    }

    /// Executes one instruction. Returns `Some` with `main`'s result once
    /// `main` returns.
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
                    .ok_or(VmError::Internal("stack underflow"))?;
                let items: Vec<Value> = self.stack.split_off(at);
                self.push(Value::Array(Rc::new(RefCell::new(items))));
            }
            Op::NewArray(elem) => {
                let len = self.pop_int()?;
                self.push(builtins::new_array(elem, len)?);
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

    /// Enters `func`, whose parameters are the top `argc` stack values.
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
        // The other locals start as `unit`.
        for _ in argc..chunk.n_locals {
            self.stack.push(Value::Unit);
        }
        self.frames.push(Frame { func, ip: 0, base });
        Ok(())
    }

    /// Returns from the current call, leaving the result for the caller.
    /// Yields `Some(result)` when `main` returns.
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
        // The last argument is on top.
        let mut args = Vec::with_capacity(argc);
        for _ in 0..argc {
            args.push(self.pop()?);
        }
        args.reverse();
        let result = builtins::eval(builtin, &args, &mut self.stdout)?;
        self.push(result);
        Ok(())
    }

    // ---- frame & stack helpers ----

    /// The current call. The run loop steps only while one exists.
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

    fn pop_str(&mut self) -> Result<Rc<str>, VmError> {
        match self.pop()? {
            Value::Str(v) => Ok(v),
            _ => Err(VmError::Internal("expected str")),
        }
    }

    fn pop_array(&mut self) -> Result<Array, VmError> {
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

/// `index` as a `usize`, if it is in `0..len`.
pub(crate) fn index_in_bounds(index: i64, len: usize) -> Option<usize> {
    if index < 0 {
        return None;
    }
    let i = index as usize;
    (i < len).then_some(i)
}

/// `a / b`, failing on a zero divisor or overflow (`i64::MIN / -1`).
pub(crate) fn checked_div(a: i64, b: i64) -> Result<i64, VmError> {
    if b == 0 {
        Err(VmError::DivisionByZero)
    } else {
        a.checked_div(b).ok_or(VmError::IntegerOverflow)
    }
}

/// `a % b`, failing on a zero divisor or overflow (`i64::MIN % -1`).
pub(crate) fn checked_rem(a: i64, b: i64) -> Result<i64, VmError> {
    if b == 0 {
        Err(VmError::DivisionByZero)
    } else {
        a.checked_rem(b).ok_or(VmError::IntegerOverflow)
    }
}
