# 002 — macOS native first (deviates from file.md section 43 order)

- Status: accepted
- Date: 2026-10-01

## Context

file.md section 43 orders `x86_64-linux` before `x86_64-macos`; section 44
targets `aarch64-linux`/`aarch64-macos`. The active dev machine is
`aarch64-apple-darwin`, so native execution cannot be dogfooded until a
macOS backend exists.

## Decision

Order native backends macOS-first (`aarch64-macos`, then `x86_64-macos`
if needed), Linux ELF second. Commit 09 planning must be reordered
accordingly: Mach-O + macOS ABI/syscalls before ELF + System V.

## Alternatives

- Follow file.md order literally: rejected — delays on-machine execution
  and differential testing.
- Interpreter/WASM-only until Linux backend matures: rejected — weakens
  the native dogfooding loop the prime directive demands.

## Tradeoffs

Mach-O/object/ABI work moves earlier; Linux CI coverage must still be
added so the reference target does not rot.

## Consequences

CI runs on `macos-latest` now; Linux runner and ELF output become explicit
follow-ups in the Commit 09 milestone.
