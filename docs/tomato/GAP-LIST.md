# Tomato ISA — Gap List

Unverifiable or unpublished items from the 2026-10-01 research pass
(`ISA-RESEARCH.md`), prioritized by how much they block Lobster's Tomato
backend (spec §47–§67). "Not public / unknown" means: not found in
`tmarhguy/tomato` (HEAD, verified 2026-09-16), `tomato.tmarhguy.com`, or
the author's public journals as of 2026-10-01.

## Blocking (needed before the backend can be finished)

1. **Full calling convention.** Known: r16 is the link register
   (`CALL label` → `JAL r16, label`; `RET` = `PC = link`). Not public:
   argument registers, return-value register(s), callee-saved set,
   stack-frame layout, stack alignment, varargs, TLS. Lobster will have to
   *define* this ABI itself with the owner's review. Best available
   evidence: `software/asm/call.s`, `fib.s`, and the TomatoOS sources.
2. **SP initialization and stack layout.** PUSH/POP use a hardware SP, but
   its reset value, the stack's location/size within dmem (`0x000000`),
   and any alignment rule are not public. Needed for Lobster's prologue/
   epilogue and runtime startup code.

## Needed for correct codegen

3. ~~Remaining carry-source select definitions~~ **RESOLVED 2026-10-01.**
   From `hardware/fpga/core/rtl/alu.v` (local clone at
   `docs/tomato/scratch/tomato`, HEAD `4a7f859`): csel 0 = const 0,
   1 = const 1, 2 = N, 3 = Z, 4 = C, 5 = GT, 6 = LT, 7 = V (flag register
   layout `csr: [0]Z [1]~Z [2]N [3]C [4]V [5]LT [6]GT [7]GTE`). The burned
   ISA only uses 0/1/4 today — flag-fed carry is datapath-legal but
   unburned (ROM-growth opportunity). Details in ISA-RESEARCH.md §3.2.
4. **Per-op flag semantics.** The flag file has Z, N, C, V (+ derived LT,
   GT, GTE) and `flags_we` is per-opcode, but exactly which flags each
   ALU op sets (e.g. does AND set C? does ADD set V?) is not public.
   Needed for correct CMP/branch lowering and for ADC/SBC chains.
5. **Interrupt / exception story beyond ECALL.** No interrupt controller
   found; keyboard and timer are polled (journal "One Press, One Key",
   2026-08-28). ECALL traps to word `0x100` with PC saved to the link
   register; EBREAK halts. If any timer/interrupt mechanism exists (or
   is planned for the discrete build), Lobster's runtime design changes
   materially. Currently: assume cooperative/poll-based only.

## Needed for validation

6. **A canonical simulator for Lobster CI.** Candidates exist
   (`tools/virtual_tomato.py`, `web/js/tomato-cpu.js` with an 80/80 test
   suite), but none is designated the ISA-conformance oracle, and their
   mutual consistency is unverified. Lobster needs one blessed oracle to test
   emitted binaries against.
7. **Assembler edge-case behavior.** `software/assembler.py` is the
   encoding ground truth, but its `--selftest` coverage of all 92 rows
   and of pseudo-op expansion corner cases is unverified. Lobster's encoder
   should be cross-checked row-by-row.

## Owner questions (Tyrone is the CPU owner — cheapest path to answers)

- Q1: Will you bless a Lobster-defined calling convention, or do you want to
  specify argument/return/callee-saved registers yourself? (gaps 1–2)
- Q2: ~~What are carry sources 2, 3, 5, 6, 7? (gap 3)~~ **Answered
  2026-10-01 from RTL:** 2=N, 3=Z, 4=C, 5=GT, 6=LT, 7=V (gap 3 resolved).
- Q3: Which flags does each ALU op write? (gap 4)
- Q4: Is any interrupt mechanism planned, or is polled-MMIO the permanent
  story? (gap 5)
- Q5: Which simulator is the conformance oracle? (gap 6)
- Q6: Is the 92-row burn set stable, or should Lobster expect ROM growth
  before the backend lands? (affects catalog versioning, ADR 004)

## Non-gaps (settled by this research)

- A local Tomato checkout is available at `../tomato` (sibling of this
  repo) for RTL-level questions; no copy is vendored under `docs/tomato/`.
- The datapath really is dual-LUT3 per bit (`out = f(a,b,c) + g(a,b,c) +
  carry_in`) — the Lobster spec's hardware hypothesis holds. (FPGA RTL ripples
  carry per byte via four `alu8b` blocks; the site's per-nibble description
  may refer to the discrete build.)
- All 8 carry-source selects are defined (csel: 0=const0, 1=const1, 2=N,
  3=Z, 4=C, 5=GT, 6=LT, 7=V); flag register layout is
  `csr: [0]Z [1]~Z [2]N [3]C [4]V [5]LT [6]GT [7]GTE` (`alu.v`).
- The ISA does **not** expose arbitrary LUT programming; synthesis must
  be catalog lookup (ADR 004).
- Tomato64 has no public ISA; the 130B-vector verification run is
  documented against the 32-bit ALU.
- The 32,768-register superbank is not in the FPGA RTL or burned ISA;
  Lobster targets 256 registers.
