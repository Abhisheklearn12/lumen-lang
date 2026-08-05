# Contributing to Lumen

Lumen is maintained by one person in their spare time. The bar for merging is
one question: can a reviewer read this change, understand why it exists, and be
convinced it is correct? Everything below serves that.

## AI policy

Read this first. It is what most often decides whether a pull request gets
reviewed at all.

**Using an AI assistant is allowed. Submitting its output unexamined is not.**

The issue is not the tool, it is review cost. Generated code is fluent,
plausible, and confidently wrong in ways that take longer to disprove than to
write correctly from scratch. There are not enough maintainer hours to do that
checking on an author's behalf, so it stays with you.

If you use an assistant, you are still the author. Before opening a pull
request:

- You have read every line and can explain why it is there, from understanding,
  without going back to a model.
- You have compiled and run it. All three checks below pass locally.
- You have confirmed the APIs it calls actually exist and do what the code
  assumes. Assistants invent plausible names.
- Your tests assert the behaviour you intended and fail if the change is
  reverted. A test that just pins current output proves nothing.
- The comments are true. Delete generated prose that restates the code or
  describes behaviour the code does not have.
- The diff contains only your change. No drive-by reformatting or unrelated
  renames.

Please say in the description if an assistant wrote a substantial part. That is
not held against you, it tells the reviewer where to look hardest.

Closed without a full review: diffs the author cannot explain when asked
something specific, generated boilerplate descriptions that never state the
problem being solved, bulk generated tests or refactors with no stated purpose,
and anything that does not compile.

## Clear intent and value

Your description should answer three things:

1. What problem this solves. A bug with a reproducer, a gap in the language, a
   misleading diagnostic, a measurably slow phase.
2. Why this approach, especially if a simpler fix was possible.
3. How you know it works, meaning the test you added and what it would catch.

No answer to the first question is the usual reason a change is declined. That
covers cosmetic rewrites, abstraction for a second caller that does not exist,
new dependencies, and performance work with no benchmark behind it. For anything
larger than a small fix, open an issue and settle the approach before writing
the code.

## Workflow

The toolchain is pinned by `rust-toolchain.toml` (Rust 1.96, edition 2024), so
`rustup` fetches the right version for you.

```console
$ git clone git@github.com:Abhisheklearn12/lumen-lang-compiler.git
$ cd lumen-lang-compiler
$ cargo test                                                # unit, integration, regression
$ cargo clippy --all-targets --all-features -- -D warnings  # must be clean
$ cargo fmt --check
```

All three must pass before you open a pull request. There is no CI yet, so
running them locally is the only thing standing between a mistake and `main`.

While working, dumping either side of a phase beats adding print statements:

```console
$ cargo run -- dump ast      examples/fib.lm
$ cargo run -- dump hir-opt  examples/fib.lm
$ cargo run -- dump bytecode examples/fib.lm
$ cargo run -- run examples/fib.lm --time
$ RUST_LOG=lumen=debug cargo run -- run examples/fib.lm
```

Dump forms: `tokens`, `ast`, `hir`, `hir-opt`, `mir`, `cfg`, `c`, `bytecode`,
`verify`.

## Rules that are not preferences

Breaking one of these needs an explicit argument in the pull request.

- **No phase mutates another phase's output.** Each takes its input by
  reference, writes diagnostics into a shared sink, and returns a new
  representation. Resolution and type-check results live in side tables keyed by
  `NodeId`, so the AST stays immutable.
- **Representations stay separate.** Lexer types do not appear in the parser's
  API, AST types do not appear in the backend.
- **Diagnostic codes are append-only.** A shipped code's meaning is frozen. Add
  a new one in the right block (`E01xx` lexer, `E02xx` resolution, `E03xx` type
  checking) and give it an explanation in `src/explain.rs`, which a test
  enforces for every code.
- **The VM never panics.** Every runtime failure surfaces as a typed error. An
  `unwrap` on the VM path is a bug even when you believe it cannot fire.
- **Output is deterministic.** Optimizer, formatter, and disassembler must
  produce identical output for identical input.

Each phase has module-level docs (`//!`) explaining its own design. Read the one
for the phase you are touching, and `docs/DESIGN.md` for how they fit together.

## Tests

New behaviour needs a test.

- Unit tests sit next to the code in `src/`.
- Integration tests go in `tests/`, one binary per area. Drive the compiler
  through the helpers in `tests/common/mod.rs` (`run`, `run_with`, `stdout`,
  `compile_errors`) rather than reaching into internals.
- Fixed a bug? Pin it in `tests/regression.rs` with a comment naming the
  original defect. This is the most valuable kind of test here.
- A new program in `examples/` needs a matching case in `tests/examples.rs`,
  which asserts exact output.

Assert diagnostic codes (`vec!["E0303"]`), never rendered message text. Messages
are allowed to improve, codes are stable.

## Scope

The README's "Not yet implemented" list is the honest state of the language, and
most of it is open. The large items (generics, closures, modules) are design
work before they are coding work, so start with an issue. Native code generation
and separate compilation are non-goals. A new builtin has to justify itself
against the 43 that exist.

Reasonable places to start, most self-contained first: a regression test for a
bug you hit, a clearer diagnostic label or note, a new example with its test, a
builtin in a family that already exists, a MIR data-flow pass, widening the C
backend past the scalar subset.

## Commits, pull requests, and bug reports

Commit subjects follow the existing log: `type: summary`, imperative, lowercase
after the colon (`feat:`, `fix:`, `docs:`, `test:`, `refactor:`, `perf:`,
`chore:`). One logical change per commit, one concern per pull request. A fix
and a refactor in one diff take several times longer to review than they do
apart.

A bug report should be the smallest `.lm` program that reproduces the problem,
the command you ran, what you expected, and the exact output you got. If
behaviour differs between `-O0` and `-O1`, say so up front. That points straight
at the optimizer.

## Come and discuss

If you have an idea worth building, open a GitHub issue or a discussion before
writing the code. I would genuinely love to talk it through. Working out why a
design is right, what it costs, and what it rules out is worth far more than the
patch that comes after it, and it is the part I enjoy most. A half-formed idea
with good reasoning behind it is welcome here.

Enjoy the thing you are building.

## License

Contributions are licensed under the MIT License, the same as the rest of the
project. See `LICENSE`.
