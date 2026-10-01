# Lobster documentation

This directory separates current reference material from Lobster's dated
engineering record. If two documents disagree, start with
[`status.md`](status.md): it describes what the repository proves.

## Start here

| Document | Use it for |
|---|---|
| [`status.md`](status.md) | Current facts, milestone tracker, and claim boundaries |
| [`spec.md`](spec.md) | Evolving language specification (input: `../file.md`) |
| [`adr/`](adr/) | Architecture decision records |
| [`../file.md`](../file.md) | Frozen 123-section project brief (intent, not status) |
| [Tomato ISA](https://github.com/tmarhguy/tomato/tree/main/docs/isa) | Burned ISA contract Lobster's Tomato backend must target |

The [root README](../README.md) is the visual front door. Subsystem guides
live with their source once their milestones land; until then the crate
docs (`cargo doc`) and stage sections in `spec.md` are authoritative.

## Canonical versus historical

Current guides describe the repository as it exists now. The
[`../log/`](../log/) directory is a historical build journal: entries retain
the claims, terminology, and plans that were accurate to the author at the
time. Do not use a dated entry to establish present status unless a current
guide confirms it.

## Focused checks

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

These three are also the CI gate. Subsystem fuzzing, differential testing,
and benchmark smoke tests join in their respective milestones (Commits 02,
10, and 20).
