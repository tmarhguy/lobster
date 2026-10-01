# Lobster canonical facts and status

Last verified from repository sources: 2026-10-01.

This file is the short, current factual backbone for Lobster documentation. It
describes what the repository proves; it does not turn roadmap items
into implementation claims.

## Canonical vocabulary

- **Lobster** is the systems language and compiler ecosystem as a whole.
- **`lobster`** is the command-line interface built from `crates/lobster-cli/`.
- **Tomato32** means the `tomato32-none` Lobster target. Its ISA authority lives
  in the sibling tomato repository (`docs/isa/*.csv`,
  `software/assembler.py`); Lobster carries no private opcode table.
- **Commit NN** means one of the twenty primary milestone commits tracked below.

## Current canonical facts

| Topic | Adopted fact | Repository authority |
|---|---|---|
| Stage-0 language | The compiler is written in Rust. | `Cargo.toml`, `crates/*/Cargo.toml` |
| Workspace | Crates build to one `lobster` binary: `lobster-source`, `lobster-diagnostics`, `lobster-lexer`, `lobster-parser`, `lobster-ast`, `lobster-types`, `lobster-resolve`, `lobster-sema`, `lobster-hir`, `lobster-mir`, `lobster-interp`, `lobster-ssa`, `lobster-cli`. | `Cargo.toml`, `cargo build --workspace` |
| Source identity | Stable `FileId`, half-open byte spans, 1-based line/character-column mapping. | `crates/lobster-source/src/lib.rs` |
| Diagnostics | `error[E###]` rendering with primary/secondary spans, notes, suggestions; snapshot-tested. | `crates/lobster-diagnostics/src/lib.rs` |
| CLI surface | All section-4 subcommands exist; `check` lexes, parses, resolves names, type-checks, and reports `E1xx`/`E2xx` diagnostics; `run [--dump-mir --dump-ssa --verify-ssa]` lowers HIR/MIR/SSA, verifies SSA (`SSA-verify`, fail closed), and executes `main`, reporting `TRAP-*` on failure; the rest exit 2 as declared stubs. | `crates/lobster-cli/src/main.rs`, `crates/lobster-cli/tests/cli.rs` |
| Frontend | Lexer (`E100`–`E105`) with integer/float type suffixes and reserved-word set, Pratt parser (`E110`–`E112`) with struct patterns and let destructuring, span-annotated AST; recovery plus corpus and no-panic tests. | `crates/lobster-lexer/`, `crates/lobster-parser/`, `crates/lobster-ast/` |
| Name resolution | Single-file resolver (`lobster-resolve`): items, locals, params, variants, `println` builtin; diagnostics E200–E203. `lobster check` runs `lobster_resolve::resolve` then `lobster_sema::check_file`; dangling variant paths resolve to the parent enum so the checker emits E218. | `crates/lobster-resolve/src/lib.rs`, `docs/adr/003-name-resolution.md` |
| Type checking | Bidirectional checker, no inference variables; unsuffixed literals adopt context; no implicit conversions; literal range checks; cast table (E212/E216); exhaustiveness (E213); diagnostics E210–E218. 14 compile-fail corpus files plus unit tests. Deferred: definite assignment, borrow checking, const eval, generics. | `crates/lobster-sema/src/lib.rs`, `crates/lobster-sema/tests/compile-fail/`, `docs/adr/005-type-checking.md` |
| Execution | Reference interpreter runs MIR CFGs: `lobster run examples/hello.lobster` prints `55`. Integer `+`/`-`/`*` wrap, `/`/`%` by zero traps, shifts mask, floats are strict IEEE, `float as int` saturates, `u32 as char` traps, `&`/`*` trap as deferred memory model. Six examples run; `TRAP-*` codes never collide with `E2xx`. Every run builds SSA and verifies it first. | `crates/lobster-hir/`, `crates/lobster-mir/`, `crates/lobster-interp/`, `examples/` |
| SSA | SSA CFG with one phi per local at every join (simple join phis, dead phis kept for the optimizer); verifier checks structure, single-def, phi/predecessor agreement, operand and operator types; `lobster run --dump-ssa` prints it, `--verify-ssa` reports `ssa ok`. | `crates/lobster-ssa/` |
| Native targets | None. Backend order is macOS-first per ADR 002. | `docs/adr/002-macos-first.md` |
| License | MIT. | `LICENSE`, crate manifests |

## Milestone tracker

- [x] **Commit 01** — bootstrap workspace, diagnostics framework, CI, language specification
- [x] **Commit 02** — lexer, parser, AST, recoverable syntax diagnostics
- [x] **Commit 03** — name resolution, scopes, type checking, compile-fail tests
- [x] **Commit 04** — typed HIR, MIR lowering, CFG infrastructure, reference interpreter
- [x] **Commit 05** — SSA + verifier
- [ ] **Commits 06–08** — optimizer + levels, Machine IR + regalloc infra
- [ ] **Commits 09–13** — x86-64, differential testing, RISC-V + WASM, AArch64
- [ ] **Commits 14–18** — Tomato baseline, ADD3, LUT3 synthesis, Dual-LUT packing, polymorphic synthesis
- [ ] **Commits 19–20** — toolchain + JIT + explorer API, release + visualizer + benchmarks
