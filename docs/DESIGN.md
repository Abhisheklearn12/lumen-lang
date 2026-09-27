# Lumen Compiler Design

This document explains the architecture of the Lumen compiler: its goals, the
shape of each phase, the data that flows between phases, and the trade-offs
taken. Per-phase design notes live as module-level documentation (`//!`) next to
the code they describe; this file is the map that ties them together.

## Goals and non-goals

Lumen is built to be **correct, maintainable, and observable** first, and fast
second. It is intentionally small (a closed set of primitive types plus arrays,
structs, and tuples; no generics; no modules) so that every phase can be
understood, tested, and benchmarked in isolation. The priority order, applied at
every decision point, is:

> Correctness > Maintainability > Simplicity > Testability > Performance > Features

Non-goals: native code generation, a large standard library, separate
compilation, and language features whose complexity is not paid back in
expressiveness.

## Pipeline overview

```
source text
   |  lexer            -> Vec<Token>
   v
tokens
   |  parser           -> Ast            (+ syntax diagnostics)
   v
AST
   |  resolver         -> Resolution     (+ name diagnostics)
   |  type checker     -> Typeck         (+ type diagnostics)
   v                                      side tables keyed by NodeId
HIR (lowering)         -> Hir            (fully typed, desugared)
   |  optimizer        -> Hir            (inlined, folded, dead code removed)
   v
bytecode (codegen)     -> Program
   |  verifier         -> ok / VerifyError
   |  VM               -> output / value (+ runtime errors)
   v
result
```

Each phase has a single public entry function, takes its input by reference,
writes diagnostics into a shared sink, and returns a new representation. No phase
mutates the output of another, and no phase reaches into a representation it does
not own.

Two auxiliary backends branch off the optimized HIR for analysis and validation:
a CFG-based mid-level IR (`mir`) with its own data-flow optimizer and
interpreter, and a C transpiler (`backend::c`) for the scalar subset. Neither is
on the default execution path, but both are reachable through `lumenc dump` and
exercised by the test suite.

## Representations are distinct on purpose

A core rule of the architecture is that **phase-specific data never leaks into an
unrelated phase**. This is enforced by giving each stage its own types:

| Stage            | Representation                  | Key property                       |
|------------------|---------------------------------|------------------------------------|
| Lexer            | `lexer::Token`                  | flat, span-tagged                  |
| Parser           | `parser::ast`                   | immutable, `NodeId`-tagged         |
| Name resolution  | `sema::Resolution`              | side table: use to definition      |
| Type checking    | `sema::Typeck`                  | side tables: node to type          |
| Lowering         | `hir`                           | typed, desugared, self-contained   |
| Mid-level IR     | `mir::Program`                  | CFG of basic blocks, SSA registers |
| Code generation  | `backend::bytecode::Program`    | flat instructions + constants      |

Resolution and type checking deliberately **do not mutate the AST**. They produce
side tables keyed by `NodeId`, which lowering then consumes to build the HIR.
This keeps the AST a faithful, reusable picture of the source and keeps each
analysis's output inspectable on its own.

By the time we reach HIR, all of that external context has been "baked in":
names are dense `LocalId`s, calls name their target directly, and every node
carries its `Type`. The backend therefore depends only on HIR and never needs to
consult the AST, tokens, or diagnostics.

## Diagnostics are a first-class subsystem

Diagnostics are not return values; they are accumulated in a shared
`Diagnostics` sink so that one compiler run reports *many* problems instead of
stopping at the first. Every phase is **error-tolerant**:

- The lexer emits a diagnostic and resynchronises on a bad token.
- The parser reports an error, skips to a stable boundary (`fn`, `const`, `;`,
  `}`), and continues, always making progress so malformed input cannot loop.
- Resolution and type checking report every independent problem; the type
  checker uses a poison `Type::Error` that is compatible with everything, so one
  mistake does not cascade into a flood of follow-on errors.

Every user-facing error carries a stable code from a single central registry
(`errors::DiagCode`), and rendering is delegated to `miette` behind a private
adapter, so the rest of the compiler is decoupled from its presentation.

## The language and the VM

Lumen targets a **stack-based bytecode VM** rather than native code. This is the
single most consequential design choice, and it follows directly from the
priority order: a VM is self-contained (no external toolchain), deterministic,
and trivially testable end-to-end. Two invariants make codegen and the VM
provably consistent:

1. **Stack discipline.** Every expression compiles to code that net-pushes
   exactly one value; every statement net-pushes zero. The VM relies on the same
   rule, so the two cannot disagree.
2. **Monomorphic opcodes.** Because HIR is typed, the code generator picks
   `AddInt` vs `AddFloat` directly; the VM never inspects operand types at
   runtime.

Integer arithmetic uses wrapping semantics in *both* the constant folder and the
VM, and a division or remainder that would trap is never folded, so optimized and
unoptimized builds compute the same integer results. Two known exceptions make
their output differ: the HIR folder applies `x + 0 → x` and `x * 0 → 0` to
floats too, which is wrong for `-0.0`, negative, and non-finite values; and DCE
treats an array read as pure, so an unused out-of-bounds read is deleted instead
of trapping.

## Optimization happens at two levels

The HIR optimizer (`opt`) is a pass manager that runs to a bounded fixpoint:
function inlining of small pure expression functions, constant folding with
algebraic simplification, and dead-code elimination. Every removal is gated by a
single `is_pure` predicate, so a call or a store is never dropped.

The mid-level IR (`mir`) lowers HIR to a control-flow graph of basic blocks with
single-assignment registers, where classic data-flow optimizations are natural:
constant folding, copy propagation, local common-subexpression elimination,
dead-store and dead-code elimination, and CFG simplification. A separate MIR
interpreter executes the same programs and is asserted to agree with the stack
VM, which turns the second engine into a continuous cross-check on both.

## The bytecode is verifiable and serializable

Because a `Program` can also arrive from an untrusted source, `backend::verify`
abstractly interprets each function, tracking only the operand-stack height, and
proves three properties before the VM runs: no stack underflow, a consistent
stack height wherever control-flow paths merge, and in-range operands (locals,
constants, jump targets, and call arity). The `lumenc build` command writes a
compiled program to a textual object file; `lumenc exec` reads it back, verifies
it, and runs it without recompiling from source.

## Complexity

The phases are close to linear in the size of their input. The lexer and parser
are single-pass with no backtracking. Resolution collects the top-level names,
then makes one walk; type checking records signatures, then makes one walk. The
optimizer runs a fixed set of passes to a bounded fixpoint. Code generation is
one walk with backpatching. Source locations are resolved in `O(log n)` via a
binary search over a precomputed line index. A few steps use a linear search
where inputs are usually small, so they are quadratic in the worst case: string
constant and tuple type interning, and MIR common-subexpression elimination
within a block.

## Safety

The compiler is entirely safe Rust, with no `unsafe` anywhere, and is linted
under `clippy -D warnings`. The VM never panics on bad input: genuine
program faults become a `VmError`, and situations the front-end makes impossible
are still handled defensively rather than with `unwrap`. A step limit bounds
execution so a runaway loop fails cleanly.

## Observability

Every phase entry point is annotated with `#[tracing::instrument]` and emits
structured events (token counts, diagnostic counts, optimization statistics,
per-phase timings). With `RUST_LOG=lumen=debug` the entire compilation is
explained through logs, and `--time` prints a per-phase timing breakdown.

## Testing strategy

- **Unit tests** live beside each phase and exercise it in isolation.
- **Snapshot tests** pin the AST, HIR, and bytecode pretty-printers, which are
  deterministic by construction (no spans, addresses, or hashing in the output).
- **Property tests** (`proptest`) assert the lexer and parser never panic and
  always terminate on arbitrary input.
- **Integration tests** compile and run complete programs through the public
  API, covering success, compile-error, and runtime-error outcomes.
- **Regression tests** pin every bug found during development.
- **Benchmarks** (`criterion`) measure each phase and the end-to-end pipeline.
