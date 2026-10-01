# Lobster canonical facts and status

Last verified from repository sources: 2026-10-01.

This file is the short, current factual backbone for Lobster documentation. It
describes what the repository proves; it does not turn roadmap items in
`file.md` into implementation claims.

## Canonical vocabulary

- **Lobster** is the systems language and compiler ecosystem as a whole.
- **`lobster`** is the command-line interface built from `crates/lobster-cli/`.
- **file.md** means the frozen 123-section project brief at the repository
  root. It states intent and the twenty milestone commits; it never
  establishes present status.
- **Tomato32** means the `tomato32-none` Lobster target. Its ISA authority lives
  in the sibling tomato repository (`docs/isa/*.csv`,
  `software/assembler.py`); Lobster carries no private opcode table.
- **Commit NN** means the twenty primary milestone commits defined in
  `file.md` section 114.

## Current canonical facts

| Topic | Adopted fact | Repository authority |
|---|---|---|
| Stage-0 language | The compiler is written in Rust. | `Cargo.toml`, `crates/*/Cargo.toml` |
| Workspace | Three crates build to one `lobster` binary: `lobster-source`, `lobster-diagnostics`, `lobster-cli`. | `Cargo.toml`, `cargo build --workspace` |
| Source identity | Stable `FileId`, half-open byte spans, 1-based line/character-column mapping. | `crates/lobster-source/src/lib.rs` |
| Diagnostics | `error[E###]` rendering with primary/secondary spans, notes, suggestions; snapshot-tested. | `crates/lobster-diagnostics/src/lib.rs` |
| CLI surface | All section-4 subcommands exist; `check` lexes, parses, and reports `E1xx` diagnostics, the rest exit 2 as declared stubs. | `crates/lobster-cli/src/main.rs`, `crates/lobster-cli/tests/cli.rs` |
| Frontend | Lexer (`E100`–`E105`), Pratt parser (`E110`–`E112`), span-annotated AST; recovery plus corpus and no-panic tests. | `crates/lobster-lexer/`, `crates/lobster-parser/`, `crates/lobster-ast/` |
| Execution | No program executes yet. `examples/hello.lobster` is a checked-in sample awaiting the Commit 04 interpreter. | `examples/hello.lobster` |
| Native targets | None. Backend order is macOS-first per ADR 002 (deviates from `file.md` section 43). | `docs/adr/002-macos-first.md` |
| License | MIT. | `LICENSE`, crate manifests |

## Milestone tracker

- [x] **Commit 01** — bootstrap workspace, diagnostics framework, CI, language specification
- [x] **Commit 02** — lexer, parser, AST, recoverable syntax diagnostics
- [ ] **Commit 03** — name resolution, scopes, type checking, compile-fail tests
- [ ] **Commit 04** — typed HIR, MIR lowering, CFG infrastructure, reference interpreter
- [ ] **Commits 05–08** — SSA + verifier, optimizer + levels, Machine IR + regalloc infra
- [ ] **Commits 09–13** — x86-64, differential testing, RISC-V + WASM, AArch64
- [ ] **Commits 14–18** — Tomato baseline, ADD3, LUT3 synthesis, Dual-LUT packing, polymorphic synthesis
- [ ] **Commits 19–20** — toolchain + JIT + explorer API, release + visualizer + benchmarks
