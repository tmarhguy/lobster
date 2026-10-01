# Security policy

Lobster is pre-release experimental software with no deployed attack surface:
no network services, no package downloads, no deserialization of untrusted
artifacts beyond local source files.

## Reporting

If you find a security-relevant issue (e.g. unsound `unsafe` in the
compiler host code, path traversal in `lobster new`, or CI secret handling),
open a GitHub issue in this repository and label it `security`.

## Scope notes

- Compiler crashes and miscompilations are correctness bugs, not security
  embargoes — file them as normal issues with a reproducer.
- Do not include credentials, private device identifiers, or personal data
  in reports.
