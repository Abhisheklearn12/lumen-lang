//! Code generation: [`Hir`] to bytecode.
//!
//! One invariant keeps it local: an expression's code pushes exactly one value
//! net, and a statement's code pushes none. So a block is its statements then
//! its tail (or `unit`), both arms of an `if` leave one value, and a call
//! leaves its result.
//!
//! Operators are picked by operand type, so the VM never checks types. Forward
//! jumps are emitted with a placeholder target and patched once it is known.

use std::rc::Rc;

use crate::backend::bytecode::{Chunk, Op, Program};
use crate::hir::{BinOp, Block, Callee, Expr, ExprKind, Function, Hir, LocalId, Stmt, UnOp};
use crate::sema::types::{Builtin, Type};

/// Compiles a program to bytecode.
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
    // The body leaves its value on the stack.
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

/// The `break` and `continue` jumps of a loop, patched when the loop ends.
#[derive(Default)]
struct LoopCtx {
    /// Patched to just after the loop.
    break_jumps: Vec<usize>,
    /// Patched to the condition (`while`) or the increment (`for`).
    continue_jumps: Vec<usize>,
}

struct FnCompiler {
    code: Vec<Op>,
    consts: Vec<Rc<str>>,
    /// Enclosing loops, innermost last.
    loops: Vec<LoopCtx>,
}

impl FnCompiler {
    fn emit(&mut self, op: Op) -> usize {
        self.code.push(op);
        self.code.len() - 1
    }

    /// The pool index of string `s`, added if new.
    fn intern(&mut self, s: &str) -> u32 {
        if let Some(idx) = self.consts.iter().position(|c| &**c == s) {
            return idx as u32;
        }
        self.consts.push(Rc::from(s));
        (self.consts.len() - 1) as u32
    }

    /// Points the jump at `jump` to `target`.
    fn patch(&mut self, jump: usize, target: usize) {
        match self.code[jump].jump_target_mut() {
            Some(t) => *t = target,
            None => unreachable!("patching non-jump op: {:?}", self.code[jump]),
        }
    }

    /// Points the jump at `jump` to the next instruction emitted.
    fn patch_to_here(&mut self, jump: usize) {
        self.patch(jump, self.code.len());
    }

    // ---- blocks & statements ----

    /// Compiles a block for its value: pushes one value.
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

    /// Compiles a statement: pushes nothing.
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
        self.block_value(body);
        self.emit(Op::Pop);
        self.emit(Op::Jump(cond_start));
        self.finish_loop(exit, cond_start);
    }

    /// Compiles `for var in start..end`, with the increment as the `continue`
    /// target.
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

        let incr = self.code.len();
        self.emit(Op::LoadLocal(var.0));
        self.emit(Op::PushInt(1));
        self.emit(Op::AddInt);
        self.emit(Op::StoreLocal(var.0));
        self.emit(Op::Jump(cond_start));
        self.finish_loop(exit, incr);
    }

    /// Ends the innermost loop: its exit test and `break`s jump here, and its
    /// `continue`s to `continue_target`.
    fn finish_loop(&mut self, exit: usize, continue_target: usize) {
        let ctx = self.loops.pop().unwrap_or_default();
        self.patch_to_here(exit);
        let end = self.code.len();
        for &jump in &ctx.break_jumps {
            self.patch(jump, end);
        }
        for &jump in &ctx.continue_jumps {
            self.patch(jump, continue_target);
        }
    }

    /// Emits a `break` (`is_break`) or `continue` jump for the innermost loop to
    /// patch. Outside a loop, which type checking rejects, it is never patched.
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

    // ---- expressions: each pushes one value ----

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
                self.emit(Op::PushUnit);
            }
            // A struct is an array of its fields.
            ExprKind::ArrayLit(elems) | ExprKind::StructLit(elems) => {
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
                self.emit(Op::SetIndex);
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
        // `&&` and `||` short-circuit, so they compile to jumps.
        match op {
            BinOp::And => return self.logical_and(lhs, rhs),
            BinOp::Or => return self.logical_or(lhs, rhs),
            _ => {}
        }
        self.expr(lhs);
        self.expr(rhs);
        let instr = if op == BinOp::Add && lhs.ty == Type::Str {
            Op::ConcatStr
        } else {
            arithmetic_op(op, lhs.ty)
        };
        self.emit(instr);
    }

    /// `a && b`: `false` without evaluating `b` if `a` is false.
    fn logical_and(&mut self, lhs: &Expr, rhs: &Expr) {
        self.expr(lhs);
        let to_false = self.emit(Op::JumpIfFalse(usize::MAX));
        self.expr(rhs);
        let end = self.emit(Op::Jump(usize::MAX));
        self.patch_to_here(to_false);
        self.emit(Op::PushBool(false));
        self.patch_to_here(end);
    }

    /// `a || b`: `true` without evaluating `b` if `a` is true.
    fn logical_or(&mut self, lhs: &Expr, rhs: &Expr) {
        self.expr(lhs);
        let eval_rhs = self.emit(Op::JumpIfFalse(usize::MAX));
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
            // `len` and the `array_new_*` family have opcodes of their own.
            Callee::Builtin(Builtin::Len) => {
                self.emit(Op::ArrayLen);
            }
            Callee::Builtin(builtin) => match builtin.new_array_elem() {
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
            None => {
                self.emit(Op::PushUnit);
            }
        }
        self.patch_to_here(to_end);
    }
}

/// The instruction for `op` on operands of type `operand`: the float variant
/// for `f64`, else the int one. `==` and `!=` work on any type.
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
        BinOp::And | BinOp::Or => unreachable!("logical ops compile to control flow"),
    }
}
