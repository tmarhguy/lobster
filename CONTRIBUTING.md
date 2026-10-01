# Contributing to Lobster

Lobster is an active, experimental language and compiler project. Small,
evidence-backed changes are easiest to review.

## Before opening a change

- Start from the current canonical facts in [`docs/status.md`](docs/status.md)
  and the [prime directive](docs/spec.md#2-prime-directive):
  correct semantics first; fast wrong code is failure.
- Keep roadmap claims distinct from repository-backed status
  (`docs/status.md`). Do not describe a milestone as done because one example
  compiles — every feature needs implementation plus unit, negative,
  integration, and documentation proof.
- Do not rewrite dated material under `log/` as if it were current prose.
- Do not include credentials, private device identifiers, downloaded
  toolchains, generated build trees (`target/`), or personal capture data.
- Respect the core shortcuts rule: no `Lobster → C → GCC`, no
  `Lobster → LLVM does everything` core compiler.

## Focused checks

Run all three before every change (they are also the CI gate):

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

On this machine's default toolchain, prefix each with
`SDKROOT=/Library/Developer/CommandLineTools/SDKs/MacOSX15.4.sdk`
until the linker/SDK skew is resolved — see the README macOS note.

## Contributions and licensing

By submitting a contribution, you represent that you have the right to
submit it under the repository's MIT [`LICENSE`](LICENSE). Call out third-party or generated
material explicitly rather than assuming the project licence applies.
