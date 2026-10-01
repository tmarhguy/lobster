# ADR 004: Tomato backend targets the real Tomato32 v1 ISA

- **Status:** accepted
- **Date:** 2026-10-01
- **Context:** The Lobster spec (§47) makes Tomato32 a first-class backend target
  with polymorphic instruction synthesis (§52–§58) as its signature feature,
  and requires the backend to come from the *actual* Tomato ISA. The
  TOMATO-ISA RESEARCH workstream (2026-10-01) reverse-documented the public
  Tomato project (`github.com/tmarhguy/tomato`, Tomato32 v1) and found a
  critical constraint: the datapath really is dual-LUT3 per bit, but the ISA
  exposes only **92 burned opcodes** whose (f, g, carry-source) triples are
  fixed in a 512-row control ROM. There is no instruction field for an
  arbitrary truth-table byte and no software path to reprogram the LUTs.
  (Full findings: `docs/tomato/ISA-RESEARCH.md`.)
- **Decision:** Lobster's Tomato backend targets **Tomato32 v1 as burned** —
  the 92 rows of `docs/isa/tomato.v1.csv` at repo HEAD (verified 2026-09-16
  by the project's own `docs/status.md`) — as the fixed instruction
  catalog. Polymorphic synthesis (§52–§58) is implemented as **pattern
  matching against the burned (lutA, lutB, csel) catalog**, not as
  generation of arbitrary (f, g, cin) triples. The catalog is
  machine-generated from the ISA CSV + the DUAL table in
  `tools/gen_microcode_v1.py` so it tracks ROM growth.
- **Alternatives considered:**
  - *Invent a LUT-immediate instruction format.* Rejected: it would not run
    on any real Tomato hardware or emulator; violates spec §47.
  - *Treat the 524,288-configuration space as encodable.* Rejected: that
    number is the datapath's theoretical space, explicitly "not an
    instruction count" per `docs/architecture.md`.
  - *Wait for / assume Tomato64.* Rejected: no public Tomato64 ISA exists
    (2026-10-01); the backend must target what exists.
- **Tradeoffs:** We lose the spec's most aggressive claim (arbitrary
  single-op synthesis of any 3-input function). We keep everything that is
  real and valuable: a rich catalog of compound 3-input ops (MASKADD,
  XORAND, ANDADD/ORADD/XORADD, CSEL, ADC/SBC, 32 nested-Boolean Dual-LUT
  forms) that genuinely fuse multi-op sequences into one cycle — the
  practical payoff of the polymorphic datapath.
- **Consequences:**
  - `targets/tomato32/` gets a generated pattern table (92 rows with
    (lutA, lutB, csel) triples), an encoder validated against
    `software/assembler.py`, and a cost model from the CSV `cycles` column.
  - The Lobster-owned Tomato ABI (arg/return regs, callee-saved, frames) must
    be defined with the owner's review — see `docs/tomato/GAP-LIST.md`.
  - If the owner burns new rows into unused ROM slots, the Lobster backend
    picks them up by regenerating the catalog — no code changes.
  - Spec §52–§58 should be revised to describe catalog-based pattern
    recognition; the "synthesis" framing is misleading as written.
