# Lumen

A small, statically-typed programming language and its compiler, written in
idiomatic Rust. Lumen takes a program through a full, explicit compiler pipeline
(lexing, parsing, name resolution, type checking, a typed intermediate
representation, optimization, and code generation) and runs the result on a
stack-based bytecode virtual machine.

It is built as a study in professional compiler engineering: correct,
observable, thoroughly tested, and small enough to read end to end.

```lumen
fn fib(n: i64) -> i64 {
    if n < 2 { n } else { fib(n - 1) + fib(n - 2) }
}

fn main() {
    for i in 0..11 {
        print_int(fib(i));
    }
}
```

```console
$ lumenc run examples/fib.lm
0
1
1
2
3
5
8
13
21
34
55
```

## Architecture

The compiler is an explicit sequence of phases, each with a narrow public API,
its own diagnostics, and its own data type. No phase mutates another's output.

![Lumen compiler pipeline: source → lexer → parser → AST → (name resolution, type checking) → HIR → optimizer → bytecode → VM → output](docs/assets/pipeline.png)

| Phase            | Module             | Output                          |
|------------------|--------------------|---------------------------------|
| Lexer            | `lexer`            | `Vec<Token>`                    |
| Parser           | `parser`           | `Ast` (Pratt-based)             |
| Name resolution  | `sema::resolve`    | `Resolution` side tables        |
| Type checking    | `sema::typeck`     | `Typeck` side tables            |
| Lowering         | `hir`              | typed, desugared `Hir`          |
| Optimizer        | `opt`              | inlined, folded `Hir`           |
| Code generation  | `backend::codegen` | `Program` bytecode              |
| VM               | `backend::vm`      | program output / value          |

Beyond the core path, the same typed HIR feeds two more backends used for
analysis and validation: a CFG-based mid-level IR (`mir`) with its own data-flow
optimizer and interpreter, and a C transpiler (`backend::c`) for the scalar
subset. A bytecode verifier (`backend::verify`) checks any program (including one
loaded from an object file) before it runs.

The full design rationale is in [`docs/DESIGN.md`](docs/DESIGN.md); the language
reference is in [`docs/LANGUAGE.md`](docs/LANGUAGE.md).

## Current capabilities

### Language and syntax

- Hand-written single-pass lexer, no backtracking, linear in source length
- 16 keywords; primitive type names are contextual identifiers rather than keywords
- Line comments and nested block comments
- 64-bit integer and 64-bit float literals
- String literals with `\n`, `\t`, `\r`, `\0`, `\\` and `\"` escapes
- Pratt (precedence-climbing) expression parser
- Functions, top-level constants, and struct declarations
- `let` bindings with optional type annotation and optional `mut`
- Assignment and compound assignment (`+=`, `-=`, `*=`, `/=`, `%=`)
- `if`/`else` as an expression
- `while` loops
- Counted `for` loops over a half-open range
- `for`-each loops over arrays
- `match` on integer and boolean literals with a wildcard arm
- `break` and `continue`
- Blocks as expressions, with an optional tail expression supplying the value
- Unary arithmetic negation and logical not
- 13 binary operators, including short-circuiting `&&` and `||`
- Array literals and indexed read/write
- Struct literals and field access
- Tuple literals and positional access
- Recursive function calls

### Type system

- Full static type checking before any code runs
- `i64`, `f64`, `bool`, `str`, `unit`
- Arrays of `i64`, `f64`, `bool` or `str`
- User-defined structs with named, typed fields
- Structural tuple types, interned by their element list
- Type inference for `let` bindings
- No implicit conversions of any kind
- Divergence analysis, so a function whose body always returns needs no tail expression
- Compile-time constant evaluation for `const` items, inlined at each use
- `match` exhaustiveness checking (a `bool` scrutinee exhausts on `true` plus `false`; anything else needs a wildcard)
- Distinct tuple arities are distinct types
- Forward references to structs and functions
- A dedicated error type that absorbs follow-on errors, so a single bad expression does not cascade through every later check

### Diagnostics

- 30 stable error codes, `E0001` through `E0318`, grouped by phase in blocks of one hundred
- Codes are append-only; a shipped code's meaning is frozen
- Multi-span labels: one primary label plus any number of secondary labels
- Notes and help text on any diagnostic
- Rendered through `miette` with a fixed, deterministic theme
- Every front-end phase is error-tolerant and reports many problems in one run
- Levenshtein "did you mean" suggestions with a length-scaled threshold
- `lumenc explain <CODE>` prints a worked explanation for all 30 codes
- Byte-offset spans (8 bytes, `Copy`) attached to every AST and HIR node
- Offset to `line:column` resolution in `O(log n)` through a precomputed line index
- Columns counted in Unicode scalar values rather than bytes

### Compiler pipeline

- Eight independently timed phases: lex, parse, resolve, typeck, lower, optimize, codegen, peephole
- A single `Session` wires the phases together; each phase stays independently testable
- No phase mutates another phase's output
- Resolution and type-check results live in side tables keyed by `NodeId`, leaving the AST immutable
- The front-end always runs to completion for complete diagnostics; lowering runs only when error-free
- Typed HIR with desugaring: `match` becomes an `if`/`else` chain, compound assignment becomes plain assignment, `for`-each becomes an indexed loop over hidden slots, tuples become structs, constants are inlined
- Dense `LocalId` allocation per function, parameters first
- Compilation can stop after any stage for inspection

### Optimization

Over HIR, run to a fixpoint of at most eight iterations:

- Inlining of small, pure, non-recursive expression functions
- Constant folding
- Algebraic simplification
- Dead-code elimination: unreachable code after `return`, `while false`, and unused pure `let` bindings
- A single shared purity predicate decides what is safe to remove
- Fully deterministic, so optimized output is reproducible

Over generated bytecode:

- Jump threading through chains of unconditional jumps
- Push/pop elimination for values that are computed and immediately discarded
- Exact jump-target remapping; an instruction that is itself a jump target is never removed

### Bytecode and virtual machine

- Typed instruction enum rather than packed bytes
- Monomorphic arithmetic and ordering opcodes chosen at code generation (`AddInt` versus `AddFloat`), so the VM never inspects operand types for those
- One structural equality opcode covering every type, the single instruction that does dispatch on the operands
- Absolute jump targets resolved by backpatching
- Stack-based VM with one shared operand stack and a frame per active call
- Locals addressed relative to a frame base, with parameters as the leading slots
- Recursion
- Reference-counted strings and arrays; arrays carry reference semantics
- Structs and tuples represented as arrays at runtime
- The VM never panics: every failure surfaces as a typed error
- Runtime errors for division by zero, `i64::MIN / -1` and `i64::MIN % -1`, array index out of bounds, a negative or oversized array length, and step-limit exhaustion
- A 50,000,000 step budget bounds runaway loops so they fail cleanly instead of hanging, and a 16,777,216 element ceiling bounds a runaway allocation the same way
- Program output captured to a string, making execution deterministic and testable

### Bytecode verifier

- Abstract interpretation tracking operand stack height rather than concrete values
- Proves no stack underflow at any instruction
- Proves that two control-flow paths reaching the same instruction agree on stack height
- Proves local slots, string constants, jump targets and call targets all exist
- Proves a call passes exactly as many arguments as the callee declares
- Proves control never falls off the end, and every path terminates at a `return`
- Linear in instruction count
- Runs automatically before any object file is executed

### Ahead-of-time artifacts

- `lumenc build` writes a line-oriented textual bytecode object file
- Header line per function, a quoted-and-escaped constants section, one instruction per line
- Serialization and parsing are exact inverses, enforced by a round-trip test
- `lumenc exec` loads, verifies, then runs an object file without touching source

### Mid-level IR

- Control-flow graph of basic blocks over virtual registers with explicit terminators
- `goto`, conditional branch, and return terminators
- Seven data-flow passes run to a fixpoint: constant folding, algebraic simplification, copy propagation, local common-subexpression elimination, dead-store elimination, dead-code elimination, and CFG simplification
- CFG simplification drops unreachable blocks, collapses branches whose arms coincide, and threads `goto`-only blocks
- Side-effecting instructions are never removed by any pass
- A separate MIR interpreter, differentially tested against the stack VM across 11 programs covering integers, floats, strings, arrays, structs, tuples, recursion, loops, short-circuit operators and builtins, with both engines required to produce identical output
- Graphviz DOT export of the control-flow graph

### C backend

- Transpiles the scalar subset (`i64`, `f64`, `bool`, `unit`) to a self-contained C99 translation unit
- Functions, recursion, and all control-flow forms
- Value-position `if` flattened into explicit temporaries
- Integer addition, subtraction and multiplication emitted through explicit wrapping helpers, matching VM semantics
- Integer division and remainder emitted through checking helpers that trap on a zero divisor and on `i64::MIN / -1`, reporting the same message and exit code as the VM instead of leaving the C undefined
- Forward declarations, so functions may call each other in any order
- Reports a clear error on `str`, arrays, structs and tuples instead of emitting wrong code

### Tooling and observability

- `lumenc run`, `check`, `fmt`, `build`, `exec`, `dump` and `explain`
- Nine dump forms: `tokens`, `ast`, `hir`, `hir-opt`, `mir`, `cfg`, `c`, `bytecode`, `verify`
- `-O0` and `-O1`
- `--time` for per-phase timings in microseconds
- `-o` for the output path
- Source formatter emitting valid, re-parseable Lumen; idempotent and round-tripping, both properties tested
- Deterministic disassembler that shows instruction indices so jump targets are readable
- `#[tracing::instrument]` spans on every phase, filtered through `RUST_LOG`
- Logs go to stderr so they never mix with program output on stdout
- Exit codes: `0` on success, `1` on a compile or runtime error, `2` on a usage or I/O error

### Standard library

- 43 builtins
- Printing for `i64`, `f64`, `bool` and `str`
- Integer math: `abs`, `min`, `max`, `pow_int`, `gcd`, `lcm`, `sign`, `clamp`
- Float math: `sqrt`, `abs_float`, `floor`, `ceil`, `round`, `pow_float`, `min_float`, `max_float`
- Numeric conversions: `to_float`, `to_int`
- String conversions: `int_to_str`, `float_to_str`, `bool_to_str`, `char_to_str`, `parse_int`
- String queries: `str_len`, `char_at`, `starts_with`, `ends_with`, `contains`, `index_of`
- String transforms: `substring`, `str_repeat`, `to_upper`, `to_lower`, `trim`
- Array length via `len`
- Array allocation at a runtime length: `array_new_int`, `array_new_float`, `array_new_bool`, `array_new_str`

### Testing and quality

- 328 tests passing: 212 unit tests in the library plus 116 across 11 integration binaries
- Property tests through `proptest`: lexing arbitrary input never panics and always ends in exactly one `Eof`; every token span is well-formed and in bounds; parsing arbitrary token soup never panics
- A dedicated regression suite for previously fixed bugs
- Every example program is verified by the test suite
- `criterion` benchmarks per phase, plus end-to-end compile and execute
- `cargo clippy --all-targets --all-features -- -D warnings` passes clean
- `cargo fmt --check` passes clean
- Toolchain pinned to Rust 1.96, edition 2024, for reproducible builds

## Not yet implemented

Scope is intentionally incremental. The following are known gaps, not oversights:

- Integer addition, subtraction and multiplication wrap silently on overflow. They do not trap. Only `i64::MIN / -1` and `i64::MIN % -1` raise an overflow error.
- Arrays hold only `i64`, `f64`, `bool` or `str`. Nested arrays, arrays of structs, and arrays of tuples are rejected.
- Arrays have a fixed length once created. `array_new_*` sizes one from a runtime value, but there is still no `push`, no `pop`, and no way to grow or shrink an array afterwards.
- There is no input of any kind. No builtin reads stdin. Programs are pure computation to stdout.
- `match` patterns are scalar literals and the wildcard. No bindings, no destructuring, no ranges, no or-patterns.
- The C backend covers the scalar subset only. `str`, arrays, structs and tuples are rejected rather than transpiled.
- A recursive struct such as `struct Node { value: i64, next: Node }` passes type checking, though no value of it can ever be constructed because every struct literal must supply all fields and there is no null or optional type.
- The MIR interpreter is reachable from tests only. `lumenc` has no flag to execute a program on it.
- No generics, closures, function values, enums, methods, or nested functions.
- One source file per invocation. There is no module or import system.

## Using the compiler

First build the compiler:

```console
$ cargo build --release
```

This produces the `lumenc` binary at `target/release/lumenc`. The commands below
write it simply as `lumenc`, which only works if that binary is on your `PATH`.
Pick one of these:

- **Run it by path** (no setup): replace `lumenc` with `./target/release/lumenc`,
  e.g. `./target/release/lumenc run examples/primes.lm`.
- **Run it via cargo** (no separate build step): put the arguments after `--`,
  e.g. `cargo run --release -- run examples/primes.lm`.
- **Install it** so plain `lumenc` works: `cargo install --path .` puts `lumenc`
  in `~/.cargo/bin/` (on the default Rust `PATH`). Then the commands below work
  verbatim. Check it with `which lumenc`.

```console
$ lumenc run    examples/primes.lm     # compile and execute
$ lumenc check  examples/primes.lm     # type-check only
$ lumenc fmt    examples/primes.lm     # print canonically-formatted source
$ lumenc build  examples/fib.lm -o fib.lbc   # compile to a bytecode object
$ lumenc exec   fib.lbc                # verify and run a built object
$ lumenc explain E0318                 # explain a diagnostic code

$ lumenc dump   ast      examples/fib.lm
$ lumenc dump   hir-opt  examples/fib.lm
$ lumenc dump   mir      examples/fib.lm
$ lumenc dump   bytecode examples/fib.lm
$ lumenc run    examples/fib.lm --time # per-phase timings on stderr

$ RUST_LOG=lumen=debug lumenc run examples/fib.lm   # structured logs
```

Dump forms: `tokens`, `ast`, `hir`, `hir-opt`, `mir`, `cfg`, `c`, `bytecode`,
`verify`. Optimization is on by default; pass `-O0` to disable it.

## Examples

The [`examples/`](examples) directory holds runnable programs (`fib`,
`factorial`, `fizzbuzz`, and `primes`), each verified by the test suite.

## Development

```console
$ cargo test                                   # unit + integration + regression
$ cargo bench                                  # criterion benchmarks
$ cargo clippy --all-targets --all-features -- -D warnings
$ cargo fmt --check
```

The project pins a Rust toolchain via `rust-toolchain.toml` (Rust 1.96, edition
2024), so `rustup` gives everyone the same compiler.

### Nix

`rust-toolchain.toml` pins the compiler but not the C compiler that the C
backend's differential tests shell out to, nor the rest of the environment. For
a build that is pinned end to end, the repository is a Nix flake:

```console
$ nix develop                # dev shell: pinned rustc, cargo, clippy, rustfmt, cc, rust-analyzer
$ nix build                  # build lumenc into ./result/bin/lumenc
$ nix run . -- run examples/primes.lm    # run without installing anything
$ nix flake check            # release tests, clippy, and rustfmt in a sandbox
```

The flake reads `rust-toolchain.toml` itself, so the toolchain version has one
home and the two paths cannot drift. `flake.lock` pins nixpkgs and the Rust
overlay; `nix flake update` is the only thing that moves them.

Nix is optional. `cargo` alone remains the supported path, and CI uses it.

## Project layout

```
src/
  span.rs, source.rs        spans and the line-indexed source map
  errors.rs, diagnostics.rs error codes and the diagnostics subsystem
  explain.rs, suggest.rs    error explanations and "did you mean" hints
  lexer/                    tokens and the scanner
  parser/                   AST, the parser, and an AST printer
  sema/                     types, name resolution, type checking
  hir/                      typed IR, lowering, and an HIR printer
  opt/                      the pass manager: inlining, folding, DCE
  mir/                      CFG-based mid-level IR, passes, interpreter
  backend/                  bytecode, codegen, VM, disassembler, verifier,
                            peephole optimizer, object format, C transpiler
  format.rs                 the source formatter
  session.rs                the pipeline driver
  main.rs                   the `lumenc` CLI
docs/                       design and language documentation
examples/                   sample programs
benches/                    criterion benchmarks
tests/                      integration, regression, and example tests
```

## License

MIT. See [`LICENSE`](LICENSE).
