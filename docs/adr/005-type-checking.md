# 005 — Bidirectional checking without inference variables

- Status: accepted
- Date: 2026-10-01

## Context

Lobster needs type checking before HIR/MIR (Commit 04), but full Hindley-Milner
inference is out of scope: signatures are always annotated (spec §2), so
checking can be bidirectional — types flow in from annotations, and
unsuffixed literals adopt their context.

## Decision

`lobster-sema` checks with expected-type propagation and no inference
variables:

- unsuffixed integer/float literals adopt an expected numeric type, else
  default to `i32`/`f64`; suffixed literals take their suffix type;
- literals are range-checked (E216), with the negated-minimum rule so
  `-128i8` is valid; unparsable literals stay silent (the lexer already
  reported them);
- binary operators propagate the left type rightward and flip a defaulted
  literal left side (`2 + n` reads `2` as the type of `n`); node recording
  is idempotent per span so re-checks overwrite instead of duplicating;
- no implicit conversions anywhere; casts follow the spec §4 table
  (E212), with pointer casts allowed pending `unsafe` checking;
- `if` without `else` yields `()`; `loop` diverges; blocks unify tails
  against function returns as E214 (not E210);
- matches require exhaustive arms (E213): all variants, a wildcard, or —
  for single-shape structs — any struct pattern;
- function bodies are checked unconstrained and compared to the return
  type once, so tail mismatches read as return errors;
- generic definitions are accepted but unchecked; any use is E217;
  missing initializers and non-literal array lengths are E217;
- `break`/`continue` outside loops and bad assignment targets are E215;
- per-expression types are recorded by `NodeId` in `Tables` for Commit 04.

Deferred and documented, not silently accepted: definite assignment,
borrow checking, const evaluation, monomorphization, `unsafe` checking.

## Alternatives

- Full inference with variables: rejected — unnecessary given mandatory
  annotations, and harder to give span-precise errors.
- Never-type in the type language: rejected — divergence is tracked as a
  boolean alongside types, which is all Commit 04 needs.

## Tradeoffs

`2 + n` adoption is positional (left-to-right with a flip-back), not full
unification; exotic literal placements may need annotations. Error quality
wins over cleverness deliberately.

## Consequences

14 compile-fail corpus files pin E200–E203/E210–E218 rendering; the CLI
`check` runs the full pipeline; `Program` + `Tables` feed HIR lowering.
