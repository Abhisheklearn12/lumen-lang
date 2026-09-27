//! MIR optimization passes, repeated until nothing changes (at most 8 rounds):
//!
//! * [`const_fold`]: operators on constants, and branches on a constant.
//! * [`algebraic_simplify`]: integer identities such as `x + 0` and `x - x`.
//! * [`copy_propagation`]: uses of `r = x` become uses of `x`, which with
//!   folding also propagates constants.
//! * [`local_cse`]: reuses an identical earlier computation in the same block.
//! * [`dead_store`]: drops a store overwritten later in its block, unread.
//! * [`dead_code`]: drops computations whose register is never read.
//! * [`simplify_cfg`]: collapses branches whose targets match, threads empty
//!   blocks, and removes unreachable ones.
//!
//! An instruction with side effects (`Call`, `Store`, `SetIndex`) is removed
//! only in an unreachable block, or as a dead store.

use std::collections::{HashMap, HashSet};

use crate::hir::{BinOp, UnOp};
use crate::mir::*;

/// Rewrites made by each pass.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct MirStats {
    pub folded: usize,
    pub simplified: usize,
    pub propagated: usize,
    pub cse: usize,
    pub dead_stores: usize,
    pub removed: usize,
    pub blocks_removed: usize,
}

impl MirStats {
    /// Total rewrites across all passes.
    pub fn total(&self) -> usize {
        self.folded
            + self.simplified
            + self.propagated
            + self.cse
            + self.dead_stores
            + self.removed
            + self.blocks_removed
    }
}

impl std::ops::AddAssign for MirStats {
    fn add_assign(&mut self, other: MirStats) {
        self.folded += other.folded;
        self.simplified += other.simplified;
        self.propagated += other.propagated;
        self.cse += other.cse;
        self.dead_stores += other.dead_stores;
        self.removed += other.removed;
        self.blocks_removed += other.blocks_removed;
    }
}

/// Optimizes every function in `program`.
#[tracing::instrument(level = "debug", skip_all)]
pub fn optimize(program: &mut Program) -> MirStats {
    let mut stats = MirStats::default();
    for func in &mut program.functions {
        for _ in 0..8 {
            // Fields are evaluated in the order written, which is the pass order.
            let round = MirStats {
                folded: const_fold(func),
                simplified: algebraic_simplify(func),
                propagated: copy_propagation(func),
                cse: local_cse(func),
                dead_stores: dead_store(func),
                removed: dead_code(func),
                blocks_removed: simplify_cfg(func),
            };
            stats += round;
            if round.total() == 0 {
                break;
            }
        }
    }
    tracing::debug!(?stats, "MIR optimization complete");
    stats
}

// ---- constant folding ----

/// Folds operators on constants, and branches on a constant into a `goto`.
/// Returns the number of rewrites.
pub fn const_fold(func: &mut Function) -> usize {
    let mut count = 0;
    for block in &mut func.blocks {
        for inst in &mut block.insts {
            if let Inst::Assign { rvalue, .. } = inst
                && let Some(folded) = fold_rvalue(rvalue)
            {
                *rvalue = Rvalue::Use(Operand::Const(folded));
                count += 1;
            }
        }
        if let Terminator::Branch {
            cond: Operand::Const(Const::Bool(b)),
            then_bb,
            else_bb,
        } = &block.term
        {
            let target = if *b { *then_bb } else { *else_bb };
            block.term = Terminator::Goto(target);
            count += 1;
        }
    }
    count
}

fn fold_rvalue(rvalue: &Rvalue) -> Option<Const> {
    match rvalue {
        Rvalue::Unary(op, Operand::Const(c)) => fold_unary(*op, c),
        Rvalue::Binary(op, Operand::Const(a), Operand::Const(b)) => fold_binary(*op, a, b),
        Rvalue::Concat(Operand::Const(Const::Str(a)), Operand::Const(Const::Str(b))) => {
            Some(Const::Str(format!("{a}{b}").into()))
        }
        _ => None,
    }
}

fn fold_unary(op: UnOp, c: &Const) -> Option<Const> {
    match (op, c) {
        (UnOp::Neg, Const::Int(v)) => Some(Const::Int(v.wrapping_neg())),
        (UnOp::Neg, Const::Float(v)) => Some(Const::Float(-v)),
        (UnOp::Not, Const::Bool(v)) => Some(Const::Bool(!v)),
        _ => None,
    }
}

fn fold_binary(op: BinOp, a: &Const, b: &Const) -> Option<Const> {
    use Const::{Bool, Float, Int};
    Some(match (a, b) {
        (Int(x), Int(y)) => match op {
            BinOp::Add => Int(x.wrapping_add(*y)),
            BinOp::Sub => Int(x.wrapping_sub(*y)),
            BinOp::Mul => Int(x.wrapping_mul(*y)),
            BinOp::Div => Int(x.checked_div(*y)?),
            BinOp::Rem => Int(x.checked_rem(*y)?),
            BinOp::Eq => Bool(x == y),
            BinOp::Ne => Bool(x != y),
            BinOp::Lt => Bool(x < y),
            BinOp::Le => Bool(x <= y),
            BinOp::Gt => Bool(x > y),
            BinOp::Ge => Bool(x >= y),
            BinOp::And | BinOp::Or => return None,
        },
        (Float(x), Float(y)) => match op {
            BinOp::Add => Float(x + y),
            BinOp::Sub => Float(x - y),
            BinOp::Mul => Float(x * y),
            BinOp::Div => Float(x / y),
            BinOp::Rem => Float(x % y),
            BinOp::Eq => Bool(x == y),
            BinOp::Ne => Bool(x != y),
            BinOp::Lt => Bool(x < y),
            BinOp::Le => Bool(x <= y),
            BinOp::Gt => Bool(x > y),
            BinOp::Ge => Bool(x >= y),
            BinOp::And | BinOp::Or => return None,
        },
        (Bool(x), Bool(y)) => match op {
            BinOp::Eq => Bool(x == y),
            BinOp::Ne => Bool(x != y),
            _ => return None,
        },
        _ => return None,
    })
}

// ---- algebraic simplification ----

/// Rewrites `Binary` rvalues with an identity or absorbing operand (`x + 0`,
/// `x * 1`, `x * 0`, `x - x`, ...) into a copy or a constant, even when the
/// other side is not constant.
///
/// The rules are for integers only: float `x + 0.0` is wrong for `-0.0`, and
/// `x * 0.0` for NaN or negative `x`. (`&&` and `||` are control flow in MIR,
/// never `Binary`.)
pub fn algebraic_simplify(func: &mut Function) -> usize {
    let mut count = 0;
    for block in &mut func.blocks {
        for inst in &mut block.insts {
            if let Inst::Assign { rvalue, .. } = inst
                && let Rvalue::Binary(op, a, b) = rvalue
                && let Some(simpler) = simplify_binary(*op, a, b)
            {
                *rvalue = simpler;
                count += 1;
            }
        }
    }
    count
}

fn is_int(op: &Operand, want: i64) -> bool {
    matches!(op, Operand::Const(Const::Int(v)) if *v == want)
}

fn same_reg(a: &Operand, b: &Operand) -> bool {
    matches!((a, b), (Operand::Reg(x), Operand::Reg(y)) if x == y)
}

/// The simpler rvalue for `a op b`, if an integer identity applies.
fn simplify_binary(op: BinOp, a: &Operand, b: &Operand) -> Option<Rvalue> {
    let use_op = |o: &Operand| Rvalue::Use(o.clone());
    let zero = || Rvalue::Use(Operand::Const(Const::Int(0)));
    match op {
        // x + 0 = 0 + x = x
        BinOp::Add if is_int(b, 0) => Some(use_op(a)),
        BinOp::Add if is_int(a, 0) => Some(use_op(b)),
        // x - 0 = x; x - x = 0
        BinOp::Sub if is_int(b, 0) => Some(use_op(a)),
        BinOp::Sub if same_reg(a, b) => Some(zero()),
        // x * 1 = 1 * x = x; x * 0 = 0 * x = 0
        BinOp::Mul if is_int(b, 1) => Some(use_op(a)),
        BinOp::Mul if is_int(a, 1) => Some(use_op(b)),
        BinOp::Mul if is_int(a, 0) || is_int(b, 0) => Some(zero()),
        // x / 1 = x; x % 1 = 0
        BinOp::Div if is_int(b, 1) => Some(use_op(a)),
        BinOp::Rem if is_int(b, 1) => Some(zero()),
        _ => None,
    }
}

// ---- copy / constant propagation ----

/// Replaces each use of a register defined as a copy (`r = x`) with `x`.
pub fn copy_propagation(func: &mut Function) -> usize {
    // Register → the operand it copies, following chains of copies.
    let mut copies: HashMap<Reg, Operand> = HashMap::new();
    for block in &func.blocks {
        for inst in &block.insts {
            if let Inst::Assign {
                dst,
                rvalue: Rvalue::Use(src),
            } = inst
            {
                let resolved = resolve(src, &copies);
                copies.insert(*dst, resolved);
            }
        }
    }
    if copies.is_empty() {
        return 0;
    }

    let mut count = 0;
    map_operands(func, &mut |op| {
        if let Operand::Reg(r) = op
            && let Some(repl) = copies.get(r)
        {
            *op = repl.clone();
            count += 1;
        }
    });
    count
}

/// Follows a chain of copies to its source (at most 1000 steps).
fn resolve(op: &Operand, copies: &HashMap<Reg, Operand>) -> Operand {
    let mut cur = op.clone();
    let mut guard = 0;
    while let Operand::Reg(r) = cur {
        match copies.get(&r) {
            Some(next) if guard < 1000 => {
                cur = next.clone();
                guard += 1;
            }
            _ => break,
        }
    }
    cur
}

// ---- dead-code elimination ----

/// Removes `Assign` instructions whose register is never read.
pub fn dead_code(func: &mut Function) -> usize {
    let used = used_regs(func);
    let mut count = 0;
    for block in &mut func.blocks {
        let before = block.insts.len();
        block.insts.retain(|inst| match inst {
            Inst::Assign { dst, rvalue } => used.contains(dst) || !rvalue_is_pure(rvalue),
            // Stores and calls have effects.
            _ => true,
        });
        count += before - block.insts.len();
    }
    count
}

/// The registers read anywhere in `func`.
fn used_regs(func: &Function) -> HashSet<Reg> {
    let mut used = HashSet::new();
    let mut record = |op: &Operand| {
        if let Some(r) = op.reg() {
            used.insert(r);
        }
    };
    for block in &func.blocks {
        for inst in &block.insts {
            match inst {
                Inst::Assign { rvalue, .. } => rvalue_operands(rvalue, &mut record),
                Inst::Store { src, .. } => record(src),
                Inst::SetIndex { base, index, value } => {
                    record(base);
                    record(index);
                    record(value);
                }
                Inst::Call { args, .. } => args.iter().for_each(&mut record),
            }
        }
        match &block.term {
            Terminator::Branch { cond, .. } => record(cond),
            Terminator::Return(o) => record(o),
            Terminator::Goto(_) | Terminator::Unreachable => {}
        }
    }
    used
}

/// Whether an rvalue has no side effects. Every rvalue is pure today; only
/// the `Call`, `Store`, and `SetIndex` instructions have effects.
fn rvalue_is_pure(_rvalue: &Rvalue) -> bool {
    true
}

// ---- dead-store elimination ----

/// Removes a `Store` that a later store to the same local, in the same block,
/// overwrites with no `Load` of it in between. Staying within one block makes
/// this sound without liveness analysis: no successor can see the dead value.
pub fn dead_store(func: &mut Function) -> usize {
    let mut count = 0;
    for block in &mut func.blocks {
        // Local → index of its latest store not yet read.
        let mut pending: HashMap<LocalId, usize> = HashMap::new();
        let mut dead: HashSet<usize> = HashSet::new();
        for (i, inst) in block.insts.iter().enumerate() {
            match inst {
                // A read keeps the pending store.
                Inst::Assign {
                    rvalue: Rvalue::Load(local),
                    ..
                } => {
                    pending.remove(local);
                }
                // A second store kills the first.
                Inst::Store { local, .. } => {
                    if let Some(prev) = pending.insert(*local, i) {
                        dead.insert(prev);
                    }
                }
                _ => {}
            }
        }
        if dead.is_empty() {
            continue;
        }
        let before = block.insts.len();
        let mut i = 0;
        block.insts.retain(|_| {
            let keep = !dead.contains(&i);
            i += 1;
            keep
        });
        count += before - block.insts.len();
    }
    count
}

// ---- CFG simplification ----

/// Collapses branches whose targets match, threads jumps through empty
/// blocks, and removes blocks unreachable from the entry. Returns the number of
/// blocks removed.
pub fn simplify_cfg(func: &mut Function) -> usize {
    // 1. A branch to the same block either way is a goto.
    for block in &mut func.blocks {
        if let Terminator::Branch {
            then_bb, else_bb, ..
        } = &block.term
            && then_bb == else_bb
        {
            block.term = Terminator::Goto(*then_bb);
        }
    }

    // 2. Jump past blocks that are just a `goto`, one hop per round.
    let forward: HashMap<BlockId, BlockId> = func
        .blocks
        .iter()
        .enumerate()
        .filter_map(|(i, b)| match (&b.term, b.insts.is_empty()) {
            (Terminator::Goto(t), true) if BlockId(i as u32) != *t => Some((BlockId(i as u32), *t)),
            _ => None,
        })
        .collect();
    if !forward.is_empty() {
        for block in &mut func.blocks {
            retarget(&mut block.term, &forward);
        }
    }

    // 3. Remove unreachable blocks.
    let reachable = reachable_blocks(func);
    if reachable.len() == func.blocks.len() {
        return 0;
    }
    remove_unreachable(func, &reachable)
}

/// Replaces each target of `term` found in `map` with its image.
fn retarget(term: &mut Terminator, map: &HashMap<BlockId, BlockId>) {
    let to = |b: BlockId| map.get(&b).copied().unwrap_or(b);
    match term {
        Terminator::Goto(t) => *t = to(*t),
        Terminator::Branch {
            then_bb, else_bb, ..
        } => {
            *then_bb = to(*then_bb);
            *else_bb = to(*else_bb);
        }
        Terminator::Return(_) | Terminator::Unreachable => {}
    }
}

fn reachable_blocks(func: &Function) -> HashSet<BlockId> {
    let mut seen = HashSet::new();
    let mut stack = vec![func.entry];
    while let Some(b) = stack.pop() {
        if seen.insert(b) {
            stack.extend(func.block(b).term.successors());
        }
    }
    seen
}

/// Drops unreachable blocks and renumbers the rest densely.
fn remove_unreachable(func: &mut Function, reachable: &HashSet<BlockId>) -> usize {
    let removed = func.blocks.len() - reachable.len();

    // Old id → new id.
    let mut remap: HashMap<BlockId, BlockId> = HashMap::new();
    let mut next = 0u32;
    for i in 0..func.blocks.len() as u32 {
        if reachable.contains(&BlockId(i)) {
            remap.insert(BlockId(i), BlockId(next));
            next += 1;
        }
    }

    let old = std::mem::take(&mut func.blocks);
    for (i, mut block) in old.into_iter().enumerate() {
        if !reachable.contains(&BlockId(i as u32)) {
            continue;
        }
        retarget(&mut block.term, &remap);
        func.blocks.push(block);
    }
    func.entry = remap[&func.entry];
    removed
}

// ---- operand traversal helpers ----

/// Applies `f` to every operand `func` reads.
fn map_operands(func: &mut Function, f: &mut impl FnMut(&mut Operand)) {
    for block in &mut func.blocks {
        for inst in &mut block.insts {
            match inst {
                Inst::Assign { rvalue, .. } => rvalue_operands_mut(rvalue, f),
                Inst::Store { src, .. } => f(src),
                Inst::SetIndex { base, index, value } => {
                    f(base);
                    f(index);
                    f(value);
                }
                Inst::Call { args, .. } => args.iter_mut().for_each(&mut *f),
            }
        }
        match &mut block.term {
            Terminator::Branch { cond, .. } => f(cond),
            Terminator::Return(o) => f(o),
            Terminator::Goto(_) | Terminator::Unreachable => {}
        }
    }
}

fn rvalue_operands(rvalue: &Rvalue, f: &mut impl FnMut(&Operand)) {
    match rvalue {
        Rvalue::Use(o) | Rvalue::Unary(_, o) => f(o),
        Rvalue::Binary(_, a, b) | Rvalue::Concat(a, b) | Rvalue::Index(a, b) => {
            f(a);
            f(b);
        }
        Rvalue::MakeArray(elems) => elems.iter().for_each(f),
        Rvalue::Load(_) => {}
    }
}

// ---- local common-subexpression elimination ----

/// Within each block, turns a repeat of an earlier computation into a copy of
/// its register: `a = x + y; b = x + y` becomes `b = a`. Registers are written
/// once, so the earlier result is still valid.
///
/// `Unary`, `Binary`, `Concat`, and `Load` are reused; a `Store` to a local
/// forgets its cached `Load`s. `Index` and `MakeArray` are not: array contents
/// can change, and each `MakeArray` creates a distinct array.
pub fn local_cse(func: &mut Function) -> usize {
    let mut count = 0;
    for block in &mut func.blocks {
        // Each computation seen so far in this block, and its register.
        let mut seen: Vec<(Rvalue, Reg)> = Vec::new();
        for inst in &mut block.insts {
            match inst {
                Inst::Assign { dst, rvalue } if cse_eligible(rvalue) => {
                    match seen
                        .iter()
                        .find_map(|(rv, r)| cse_eq(rv, rvalue).then_some(*r))
                    {
                        Some(prev) => {
                            *rvalue = Rvalue::Use(Operand::Reg(prev));
                            count += 1;
                        }
                        None => seen.push((rvalue.clone(), *dst)),
                    }
                }
                Inst::Store { local, .. } => {
                    seen.retain(|(rv, _)| !matches!(rv, Rvalue::Load(l) if l == local));
                }
                _ => {}
            }
        }
    }
    count
}

/// Whether `local_cse` may reuse this rvalue.
fn cse_eligible(rvalue: &Rvalue) -> bool {
    matches!(
        rvalue,
        Rvalue::Unary(..) | Rvalue::Binary(..) | Rvalue::Concat(..) | Rvalue::Load(_)
    )
}

/// Whether two reusable rvalues compute the same value: the same operator on
/// equal operands.
fn cse_eq(a: &Rvalue, b: &Rvalue) -> bool {
    match (a, b) {
        (Rvalue::Unary(o1, x1), Rvalue::Unary(o2, x2)) => o1 == o2 && x1 == x2,
        (Rvalue::Binary(o1, a1, b1), Rvalue::Binary(o2, a2, b2)) => {
            o1 == o2 && a1 == a2 && b1 == b2
        }
        (Rvalue::Concat(a1, b1), Rvalue::Concat(a2, b2)) => a1 == a2 && b1 == b2,
        (Rvalue::Load(l1), Rvalue::Load(l2)) => l1 == l2,
        _ => false,
    }
}

fn rvalue_operands_mut(rvalue: &mut Rvalue, f: &mut impl FnMut(&mut Operand)) {
    match rvalue {
        Rvalue::Use(o) | Rvalue::Unary(_, o) => f(o),
        Rvalue::Binary(_, a, b) | Rvalue::Concat(a, b) | Rvalue::Index(a, b) => {
            f(a);
            f(b);
        }
        Rvalue::MakeArray(elems) => elems.iter_mut().for_each(f),
        Rvalue::Load(_) => {}
    }
}
