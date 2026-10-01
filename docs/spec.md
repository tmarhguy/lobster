# Lobster — language specification (skeleton)

This file is the authority for implemented language behavior.

## 1. Status

- Commit 01: workspace, `SourceManager`, diagnostics, CLI stubs. No parsing.
- Commits 02–04 (planned): lexer/parser/AST, name resolution + types,
  HIR/MIR/CFG + reference interpreter.

## 2. Prime directive

Correct semantics > clear language model > IR correctness > optimizer
correctness > correct machine code > cross-target consistency >
target-specific quality > performance > DX > polish.

## 3. Shortcuts that are not allowed

- No `Lobster → C → GCC` definition.
- No `Lobster → LLVM IR → LLVM does everything` core compiler.
  LLVM may later appear only as validation backend / oracle.

## 4. Syntax (target)

Rust-like (`fn`, `let`/`mut`, `struct`, `enum`, `match`).
Exact grammar lands with Commit 02; examples in `examples/hello.lobster`.

## 5. Types (target)

Primitives `i8..i64/u8..u64/f32/f64/bool/char/usize/isize` plus
arrays, slices, tuples, structs, enums, fn types, references, raw pointers.
Integer overflow/wrapping/checked/shift/division/cast rules must be written
before any `ADD_N`/`ADD3` transform.
Floats keep IEEE-style behavior; no reassociation without `--fast-math`.

## 6. Memory and unsafe (target)

Systems language: stack/heap, references, raw pointers, deterministic
destruction, move semantics, explicit `unsafe` for MMIO/raw-pointer/FFI/
intrinsics. Full safety model deferred (see `adr/003-*`).

## 7. Targets

`x86-64`, `AArch64`, `RISC-V`, `WebAssembly`, `Tomato32`
(`tomato32-none`). Per plan decision: macOS native first, Linux ELF second
(see `adr/002-macos-first.md`). Tomato ISA authority lives outside this
repo: `../../tomato/docs/isa/` (CSV sources) and
`../../tomato/software/assembler.py`.

## 8. Tomato synthesis (forward reference)

Out of scope for Commits 01–04. Frontend/IR must not destroy structure
the synthesizer needs: preserve n-ary arithmetic (`ADD_N`) and Boolean
DAGs; Machine IR must support multi-result ops. Exhaustive LUT3
verification and simulator comparison are mandatory when synthesis lands.
