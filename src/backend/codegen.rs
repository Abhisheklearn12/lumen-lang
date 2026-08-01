//! Code generation: [`Hir`] → [`Program`] bytecode.
//!
//! # The stack-discipline invariant
//!
//! Every expression compiles to a sequence of instructions that **net-pushes
//! exactly one value**; every statement net-pushes **zero**. This single rule
//! makes the generator local and obviously correct: a block evaluates its
//! statements (each balanced) and then its tail (or a `unit`) for the block's
//! value; an `if` arranges for both arms to leave one value; a call pushes its
//! arguments then a result. The VM relies on the same discipline.
//!
//! Because HIR is typed, operator instructions are chosen monomorphically from
//! the operand type, so no type information survives into the VM.
//!
//! Jumps are emitted with a placeholder target and **backpatched** once the
//! destination index is known.

use std::rc::Rc;

use crate::backend::bytecode::{Chunk, Op, Program};
use crate::hir::{BinOp, Block, Callee, Expr, ExprKind, Function, Hir, LocalId, Stmt, UnOp};
use crate::sema::types::{Builtin, Type};

/// Compiles a lowered, optimized program to bytecode.
#[tracing::instrument(level = "debug", skip_all)]
pub fn generate(hir: &Hir) -> Program {
    let functions = hir.functions.iter().map(compile_function).collect();
    tracing::debug!(functions = hir.functions.len(), "codegen complete");
    Program {
        functions,
        main: hir.main.0 as usize,
    }
}

fn compile_function(func: &Function) -> Chunk {
    let mut c = FnCompiler {
        code: Vec::new(),
        consts: Vec::new(),
        loops: Vec::new(),
    };
    // The body leaves one value (its block value); return it.
    c.block_value(&func.body);
    c.emit(Op::Return);
    Chunk {
        name: func.name.clone(),
        n_locals: func.locals.len(),
        n_params: func.param_count,
        code: c.code,
        consts: c.consts,
    }
}

/// Tracks the unresolved jumps for one enclosing loop, so `break` and
/// `continue` can be backpatched once the loop's exit and increment points are
/// known.
#[derive(Default)]
struct LoopCtx {
    /// `break` jumps, patched to the instruction after the loop.
    break_jumps: Vec<usize>,
    /// `continue` jumps, patched to the loop's continuation point (the
    /// condition for `while`, the increment for `for`).
    continue_jumps: Vec<usize>,
}

struct FnCompiler {
    code: Vec<Op>,
    consts: Vec<Rc<str>>,
    /// Stack of enclosing loops; the innermost is last.
    loops: Vec<LoopCtx>,
}

impl FnCompiler {
    fn emit(&mut self, op: Op) -> usize {
        self.code.push(op);
        self.code.len() - 1
    }

    /// Interns a string constant and returns its pool index.
    fn intern(&mut self, s: &str) -> u32 {
        if let Some(idx) = self.consts.iter().position(|c| &**c == s) {
            return idx as u32;
        }
        self.consts.push(Rc::from(s));
        (self.consts.len() - 1) as u32
    }

    /// Sets the absolute target of a previously emitted jump.
    fn patch_to_here(&mut self, jump_idx: usize) {
        let target = self.code.len();
        match &mut self.code[jump_idx] {
            Op::Jump(t) | Op::JumpIfFalse(t) => *t = target,
            other => unreachable!("patching non-jump op: {other:?}"),
        }
    }

    // ---- blocks & statements ----

    /// Compiles a block in value position: net stack effect `+1`.
    fn block_value(&mut self, block: &Block) {
        for stmt in &block.stmts {
            self.stmt(stmt);
        }
        match &block.tail {
            Some(tail) => self.expr(tail),
            None => {
                self.emit(Op::PushUnit);
            }
        }
    }

    /// Compiles a statement: net stack effect `0`.
    fn stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Let { local, value } => {
                self.expr(value);
                self.emit(Op::StoreLocal(local.0));
            }
            Stmt::Expr(e) => {
                self.expr(e);
                self.emit(Op::Pop);
            }
            Stmt::Return(value) => {
                match value {
                    Some(e) => self.expr(e),
                    None => {
                        self.emit(Op::PushUnit);
                    }
                }
                self.emit(Op::Return);
            }
            Stmt::While { cond, body } => self.while_loop(cond, body),
            Stmt::For {
                var,
                end_var,
                start,
                end,
                body,
            } => self.for_loop(*var, *end_var, start, end, body),
            Stmt::Break => self.loop_jump(true),
            Stmt::Continue => self.loop_jump(false),
        }
    }

    fn while_loop(&mut self, cond: &Expr, body: &Block) {
        let cond_start = self.code.len();
        self.expr(cond);
        let exit = self.emit(Op::JumpIfFalse(usize::MAX));
        self.loops.push(LoopCtx::default());
        // The body is a block in value position; its value is discarded.
        self.block_value(body);
        self.emit(Op::Pop);
        self.emit(Op::Jump(cond_start));
        let ctx = self.loops.pop().unwrap_or_default();
        // `break` leaves the loop; `continue` re-tests the condition.
        self.patch_to_here(exit);
        let end = self.code.len();
        self.patch_all(&ctx.break_jumps, end);
        self.patch_all(&ctx.continue_jumps, cond_start);
    }

    /// `for var in start..end { body }`: initialise `var` and the cached bound,
    /// then loop with the increment as the `continue` target.
    fn for_loop(&mut self, var: LocalId, end_var: LocalId, start: &Expr, end: &Expr, body: &Block) {
        self.expr(start);
        self.emit(Op::StoreLocal(var.0));
        self.expr(end);
        self.emit(Op::StoreLocal(end_var.0));

        let cond_start = self.code.len();
        self.emit(Op::LoadLocal(var.0));
        self.emit(Op::LoadLocal(end_var.0));
        self.emit(Op::LtInt);
        let exit = self.emit(Op::JumpIfFalse(usize::MAX));

        self.loops.push(LoopCtx::default());
        self.block_value(body);
        self.emit(Op::Pop);

        // Increment point - also where `continue` lands.
        let incr = self.code.len();
        self.emit(Op::LoadLocal(var.0));
        self.emit(Op::PushInt(1));
        self.emit(Op::AddInt);
        self.emit(Op::StoreLocal(var.0));
        self.emit(Op::Jump(cond_start));

        let ctx = self.loops.pop().unwrap_or_default();
        self.patch_to_here(exit);
        let end_label = self.code.len();
        self.patch_all(&ctx.break_jumps, end_label);
        self.patch_all(&ctx.continue_jumps, incr);
    }

    /// Emits a `break` (`is_break`) or `continue` jump, recording it for
    /// backpatching by the enclosing loop. Outside a loop it is a no-op (the
    /// type checker already reported the error).
    fn loop_jump(&mut self, is_break: bool) {
        let idx = self.emit(Op::Jump(usize::MAX));
        if let Some(ctx) = self.loops.last_mut() {
            if is_break {
                ctx.break_jumps.push(idx);
            } else {
                ctx.continue_jumps.push(idx);
            }
        }
    }

    /// Patches every jump in `jumps` to `target`.
    fn patch_all(&mut self, jumps: &[usize], target: usize) {
        for &idx in jumps {
            match &mut self.code[idx] {
                Op::Jump(t) | Op::JumpIfFalse(t) => *t = target,
                other => unreachable!("patching non-jump op: {other:?}"),
            }
        }
    }

    // ---- expressions (each nets +1) ----

    fn expr(&mut self, expr: &Expr) {
        match &expr.kind {
            ExprKind::Int(v) => {
                self.emit(Op::PushInt(*v));
            }
            ExprKind::Float(v) => {
                self.emit(Op::PushFloat(*v));
            }
            ExprKind::Bool(v) => {
                self.emit(Op::PushBool(*v));
            }
            ExprKind::Str(s) => {
                let idx = self.intern(s);
                self.emit(Op::PushStr(idx));
            }
            ExprKind::Local(id) => {
                self.emit(Op::LoadLocal(id.0));
            }
            ExprKind::Unary { op, rhs } => self.unary(*op, rhs),
            ExprKind::Binary { op, lhs, rhs } => self.binary(*op, lhs, rhs),
            ExprKind::Call { callee, args } => self.call(*callee, args),
            ExprKind::Assign { local, value } => {
                self.expr(value);
                self.emit(Op::StoreLocal(local.0));
                // Assignment evaluates to unit, preserving the +1 invariant.
                self.emit(Op::PushUnit);
            }
            ExprKind::ArrayLit(elems) => {
                for e in elems {
                    self.expr(e);
                }
                self.emit(Op::MakeArray(elems.len() as u32));
            }
            ExprKind::Index { base, index } => {
                self.expr(base);
                self.expr(index);
                self.emit(Op::Index);
            }
            ExprKind::SetIndex { base, index, value } => {
                self.expr(base);
                self.expr(index);
                self.expr(value);
                // SetIndex consumes the three operands and leaves unit.
                self.emit(Op::SetIndex);
            }
            // A struct is laid out as a fixed array of field values, so struct
            // construction and field access reuse the array opcodes.
            ExprKind::StructLit(fields) => {
                for f in fields {
                    self.expr(f);
                }
                self.emit(Op::MakeArray(fields.len() as u32));
            }
            ExprKind::GetField { base, idx } => {
                self.expr(base);
                self.emit(Op::PushInt(*idx as i64));
                self.emit(Op::Index);
            }
            ExprKind::SetField { base, idx, value } => {
                self.expr(base);
                self.emit(Op::PushInt(*idx as i64));
                self.expr(value);
                self.emit(Op::SetIndex);
            }
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => self.if_expr(cond, then_branch, else_branch.as_deref()),
            ExprKind::Block(block) => self.block_value(block),
        }
    }

    fn unary(&mut self, op: UnOp, rhs: &Expr) {
        self.expr(rhs);
        let instr = match (op, rhs.ty) {
            (UnOp::Neg, Type::Int) => Op::NegInt,
            (UnOp::Neg, _) => Op::NegFloat,
            (UnOp::Not, _) => Op::NotBool,
        };
        self.emit(instr);
    }

    fn binary(&mut self, op: BinOp, lhs: &Expr, rhs: &Expr) {
        // Logical operators short-circuit and so compile to control flow.
        match op {
            BinOp::And => return self.logical_and(lhs, rhs),
            BinOp::Or => return self.logical_or(lhs, rhs),
            _ => {}
        }
        self.expr(lhs);
        self.expr(rhs);
        // `+` on strings is concatenation, not arithmetic.
        let instr = if op == BinOp::Add && lhs.ty == Type::Str {
            Op::ConcatStr
        } else {
            arithmetic_op(op, lhs.ty)
        };
        self.emit(instr);
    }

    /// `a && b`: if `a` is false, the result is false and `b` is not evaluated.
    fn logical_and(&mut self, lhs: &Expr, rhs: &Expr) {
        self.expr(lhs);
        let to_false = self.emit(Op::JumpIfFalse(usize::MAX));
        self.expr(rhs);
        let end = self.emit(Op::Jump(usize::MAX));
        self.patch_to_here(to_false);
        self.emit(Op::PushBool(false));
        self.patch_to_here(end);
    }

    /// `a || b`: if `a` is true, the result is true and `b` is not evaluated.
    fn logical_or(&mut self, lhs: &Expr, rhs: &Expr) {
        self.expr(lhs);
        let eval_rhs = self.emit(Op::JumpIfFalse(usize::MAX));
        // `a` was true: result is true.
        self.emit(Op::PushBool(true));
        let end = self.emit(Op::Jump(usize::MAX));
        self.patch_to_here(eval_rhs);
        self.expr(rhs);
        self.patch_to_here(end);
    }

    fn call(&mut self, callee: Callee, args: &[Expr]) {
        for arg in args {
            self.expr(arg);
        }
        let argc = args.len() as u8;
        match callee {
            Callee::Fn(id) => {
                self.emit(Op::Call {
                    func: id.0 as usize,
                    argc,
                });
            }
            // `len` maps to a dedicated opcode rather than a builtin dispatch.
            Callee::Builtin(Builtin::Len) => {
                self.emit(Op::ArrayLen);
            }
            Callee::Builtin(builtin) => match builtin.new_array_elem() {
                // So does each `array_new_*`. Its element type is fixed at
                // compile time, so it rides along as an immediate and the VM
                // never has to inspect the value it is filling the array with.
                Some(elem) => {
                    self.emit(Op::NewArray(elem));
                }
                None => {
                    self.emit(Op::CallBuiltin { builtin, argc });
                }
            },
        }
    }

    fn if_expr(&mut self, cond: &Expr, then_branch: &Block, else_branch: Option<&Expr>) {
        self.expr(cond);
        let to_else = self.emit(Op::JumpIfFalse(usize::MAX));
        self.block_value(then_branch);
        let to_end = self.emit(Op::Jump(usize::MAX));
        self.patch_to_here(to_else);
        match else_branch {
            Some(else_expr) => self.expr(else_expr),
            // No else: the `if` yields unit.
            None => {
                self.emit(Op::PushUnit);
            }
        }
        self.patch_to_here(to_end);
    }
}

/// Selects the arithmetic/comparison instruction for an operator and operand
/// type. Equality is type-agnostic; ordering and arithmetic are monomorphic.
fn arithmetic_op(op: BinOp, operand: Type) -> Op {
    let is_float = matches!(operand, Type::Float);
    match op {
        BinOp::Add if is_float => Op::AddFloat,
        BinOp::Add => Op::AddInt,
        BinOp::Sub if is_float => Op::SubFloat,
        BinOp::Sub => Op::SubInt,
        BinOp::Mul if is_float => Op::MulFloat,
        BinOp::Mul => Op::MulInt,
        BinOp::Div if is_float => Op::DivFloat,
        BinOp::Div => Op::DivInt,
        BinOp::Rem if is_float => Op::RemFloat,
        BinOp::Rem => Op::RemInt,
        BinOp::Lt if is_float => Op::LtFloat,
        BinOp::Lt => Op::LtInt,
        BinOp::Le if is_float => Op::LeFloat,
        BinOp::Le => Op::LeInt,
        BinOp::Gt if is_float => Op::GtFloat,
        BinOp::Gt => Op::GtInt,
        BinOp::Ge if is_float => Op::GeFloat,
        BinOp::Ge => Op::GeInt,
        BinOp::Eq => Op::Eq,
        BinOp::Ne => Op::Ne,
        // Logical operators are handled before reaching here.
        BinOp::And | BinOp::Or => unreachable!("logical ops compile to control flow"),
    }
}
