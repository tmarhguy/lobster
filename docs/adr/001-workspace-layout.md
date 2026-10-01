# 001 — Rust workspace layout for stage-0 compiler

- Status: accepted
- Date: 2026-10-01

## Context

file.md section 2 mandates Rust for the stage-0 compiler with our own
frontend/HIR/MIR/SSA/optimizer/target framework, and section 111 suggests
`compiler/`, `targets/`, `runtime/`, `tools/` groupings.

## Decision

Cargo workspace with one crate per compiler stage under `crates/`:
`lobster-source`, `lobster-diagnostics`, `lobster-cli` now; `lobster-lexer`, `lobster-parser`,
`lobster-ast`, `lobster-resolve`, `lobster-types`, `lobster-hir`, `lobster-mir`, `lobster-interp`
to follow in Commits 02–04. Target backends later under `crates/lobster-target-*`
or `targets/` per section 111 once Machine IR exists.

## Alternatives

- Single crate with modules: rejected — stage boundaries (and their test
  suites) would blur, against section 112 completion gates.
- Mirror section 111 directories exactly from day one: rejected — empty
  placeholder crates add noise before their milestone.

## Tradeoffs

More `Cargo.toml` files; clearer ownership and per-stage fuzz/test targets.

## Consequences

Commit 01 ships three crates; each later milestone adds crates with their
own unit, snapshot, and differential tests.
