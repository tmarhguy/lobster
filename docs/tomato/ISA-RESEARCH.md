# Tomato ISA Research — ISA-RESEARCH.md

Research on the **Tomato** CPU by **Tyrone Marhguy** (the same person building
Lobster — this is his own hardware project), conducted 2026-10-01 by the
TOMATO-ISA RESEARCH workstream (research + documentation only; no compiler
code). The Lobster spec (§47) requires the Tomato backend to be derived from the
*actual* Tomato ISA. This document is that foundation.

**Sources:** the public repository `github.com/tmarhguy/tomato` ("Discrete
32-bit Polymorphic Dual-LUT3 CPU"), read via the GitHub REST API and
`raw.githubusercontent.com` on 2026-10-01 (repo HEAD verified
2026-09-16 in `docs/status.md`), plus the companion site `tomato.tmarhguy.com`
and the author's design journals. File blob SHAs and exact URLs are listed in
§12 for reproducibility.

> **Headline finding.** The Tomato ISA is real, public, and well-documented —
> but it is **not** a raw LUT-programming ISA. The datapath genuinely contains
> two 3-input LUTs per bit (the Lobster spec's "polymorphic" hypothesis about the
> hardware is confirmed), yet the *instruction set* exposes only **92 burned
> opcodes** whose (f, g, carry-source) triples are fixed in a 512-row control
> ROM. There is no instruction field for an arbitrary truth-table byte and no
> software-visible way to reprogram the LUTs. So Lobster's polymorphic synthesis
> (§52–§58) cannot synthesize arbitrary `(f, g, cin)` triples into single
> machine ops — it must be a **catalog lookup over the 92 burned rows** (or
> future ROM growth). The spec's ADD3/LUT3 idea survives as "compound 3-input
> ops that already exist as fixed opcodes" (MASKADD, XORAND, ANDADD, ORADD,
> XORADD, CSEL, ADC/SBC, and ~30 nested-Boolean Dual-LUT forms).

---

## 1. What Tomato is (project overview)

Tomato is a 32-bit CPU designed by Tyrone Marhguy with a **dual-LUT3
polymorphic ALU**: every bit-slice of the ALU applies two independently
programmed 3-input lookup tables and combines them through the carry path:

```
out = f(a, b, c) + g(a, b, c) + carry_in      (per-bit, rippled per nibble)
```

The author advertises **524,288 theoretical configurations**
(= 256 f-tables × 256 g-tables × 8 carry sources). This is the number the
Tomato site puts on its landing page, and it is the origin of the "polymorphic"
claim that inspired Lobster's backend design.

Three implementations/artifacts exist:

| Artifact | What it is | State |
|---|---|---|
| `hardware/fpga/core/` (Nexys A7-100T) | The real, running CPU: 6.25 MHz (100 MHz / 16), 25 MHz pixel clock, ~2.13 CPI in Icarus samples | Working; runs TomatoOS v3.0 over HDMI |
| `hardware/discrete/` | Discrete-logic build (the original vision) | In progress |
| `web/js/tomato-cpu.js` | Functional browser emulator ("behavior not timing"), 80/80 test suite (`web/tests/tomato-emu.mjs`) | Working |

**Current ISA:** **Tomato32 v1**, frozen as a 512-row control ROM with
**92 burned rows** (91 instructions + NOP). The other 420 rows are unused and
read as NOP. The authority file is `docs/isa/tomato.v1.csv`; a companion
`docs/isa/README.md` documents the CSV schema.

**Tomato64:** the user's resume lists a "Tomato64 64-bit VLIW processor" with
formal verification and "130B+ test vectors". As of 2026-10-01 there is **no
public Tomato64 repository or ISA document** (GitHub repo search for
`tomato64` under user `tmarhguy` returns 0 results). The 130-billion-vector
run is documented against the **32-bit** ALU (`verification/gauntlet/README.md`,
`docs/status.md` "recorded 130-billion-vector 32-bit ALU run"). Treat Tomato64
as **not public / unknown**; everything below is Tomato32 v1.

### Project evolution (why the docs look the way they do)

- **2026-07-30** — journal: proposed a 32b → 40b redesign (10-bit opcode,
  three 6-bit fields; `alu_out = adder(f(a,b,c), g(a,b,c), carry_in)` with
  csel). (`docs/log/2026-07-30 - Redesign into 40b (Old design = 32b).md`)
- **2026-07-31** — journal: reverted to 32b (9-bit opcode, four 5-bit fields,
  512-row ROM). (`docs/log/2026-07-31 - Falling back to 32b.md`)
- **2026-08-15** — journal "ISA as a Wire": ISA design philosophy notes.
  (`tmarhguy/tmarhguy` repo, `log/2026-08-15 - ISA as a Wire.md`)
- **2026-08-28** — journal "One Press, One Key": keyboard/timer are
  **polled** MMIO, no interrupt controller.
- **2026-08-29** — proposed 32,768-register upgrade for the discrete build
  (external SRAM + superbank / `SETBANK2`).
- **2026-09-16** — `docs/status.md` canonical-facts verification: **the
  superbank is not present in the current FPGA RTL or burned ISA**; the
  256-entry FPGA register file is the addressable ISA reality.

---

## 2. ISA reference

### 2.1 Encoding formats

All instructions are 32 bits. The 9-bit opcode selects one of 512 control-ROM
rows; the remaining 23 bits are register fields / immediates / bank selects.

**R-type** (register–register):

```
{ op[8:0], rd[4:0], rA[4:0], rB[4:0], rC[4:0], bank[2:0] }   = 9+5+5+5+5+3 = 32
```

**I-type**: `{ op[8:0], rd[4:0], rA[4:0], imm13 }`. The immediate's exact
shape depends on the opcode's `ir_imm_sel` microcode field (see §5.1):

| `ir_imm_sel` | Meaning (from `tools/gen_microcode_v1.py`) |
|---|---|
| 1 | `reg_reg` — no immediate; rB is the second operand |
| 0 | 8-bit imm lane (`MOVIL`, `SHLI`, …) |
| 2 | sign-extended imm8 (`SEXTI`) |
| 3 | 13-bit offset / immediate (`ADDI`, loads: `addr = rA + imm13`) |
| 7 | zero-extended imm16 (`ANDI`, `ORI`, `XORI`, `MOVI`, …) |
| 11 | 20-bit immediate (`LUI`: `rd = imm20 << 12`) |
| 12 | 22-bit branch offset (`BEQ/BNE/BLT/BGE`) |
| 13 | 22-bit absolute jump target (`JMP`/`JAL`) |
| 14 | 22-bit absolute memory address (`LWABS`/`SWABS`) |

**Store-type**: `{ op[8:0], 0, rA_data[4:0], rB_base[4:0], imm8 }` —
`addr = rB + imm8` (`mem_sel=1`), data from rA.

**Branch-type**: `{ op[8:0], 0, off[21:0] }` — PC-relative, 22-bit offset.

### 2.2 Registers

- **256 × 32-bit** general registers, addressed as `{bank[2:0], register[4:0]}`
  (3 bank bits + 5 register bits = 8 bits → 256 entries).
- **r0 is hardwired to zero.** (The CSV's growth-template `MOVI` row notes
  r0 explicitly as the zero register.)
- 3-read / 1-write ports.
- The discrete architecture's claimed 32,768 registers (external SRAM +
  superbank / `SETBANK2`) are **not present** in the current FPGA RTL or the
  burned ISA (`docs/status.md`, verified 2026-09-16). Lobster targets the 256-entry
  reality.

### 2.3 Full opcode list (92 burned rows)

From `docs/isa/tomato.v1.csv` (status=burn). Unused rows are NOP (writing a
program to an unused opcode executes a no-op — fail-silent, not a trap).

**ALU — register (0x01–0x1F region):**

| Opcode | Mnemonic | Operation |
|---|---|---|
| 0x01 | ADD | rd = rA + rB |
| 0x02 | SUB | rd = rA − rB |
| 0x03 | AND | rd = rA & rB |
| 0x04 | OR | rd = rA \| rB |
| 0x05 | XOR | rd = rA ^ rB |
| 0x06 | MOV | rd = rB |
| 0x07 | CMP | flags = rA − rB (no writeback) |
| 0x08 | MASKADD | rd = rA + (rB & rC) |
| 0x09 | XORAND | rd = (rA ^ rB ^ rC) + (rA & rB & rC) |
| 0x0A | ANDN | rd = rA & ~rB |
| 0x0B | ORN | rd = rA \| ~rB |
| 0x0C | CSEL | rd = rC ? rA : rB |
| 0x0D | ANDADD | rd = (rA & rB) + rC |
| 0x0E | ORADD | rd = (rA \| rB) + rC |
| 0x0F | XORADD | rd = (rA ^ rB) + rC |
| 0x10 | MUL | rd = low(rA × rB) |
| 0x11 | MULH | rd = high(rA × rB) |
| 0x12 | DIV | rd = rA / rB |
| 0x13 | REM | rd = rA % rB |
| 0x14 | MULHU | high, unsigned |
| 0x15 | DIVU | / unsigned |
| 0x16 | REMU | % unsigned |
| 0x17 | ADC | rd = rA + rB + C |
| 0x18 | SBC | rd = rA − rB + C_in |
| 0x19 | RSB | rd = rB − rA |

**Nested Boolean Dual-LUT (0x1A–0x3F)** — each is `outer(rA, inner(rB, rC))`,
fusing a two-cycle Boolean nest into one Dual-LUT cycle with g=0
(`docs/isa/lut-nested-fuse.csv`, 32 rows):

| Opcode | Mnemonic | Operation | | Opcode | Mnemonic | Operation |
|---|---|---|---|---|---|---|
| 0x1A | XORBC | rA ^ (rB & rC) | | 0x32 | NOROR | ~(rA \| (rB \| rC)) |
| 0x1B | XORBO | rA ^ (rB \| rC) | | 0x33 | NORXOR | ~(rA \| (rB ^ rC)) |
| 0x1C | XORBX | rA ^ (rB ^ rC) | | 0x34 | NORNAND | ~(rA \| ~(rB & rC)) |
| 0x1D | ANDBO | rA & (rB \| rC) | | 0x35 | NORNOR | ~(rA \| ~(rB \| rC)) |
| 0x1E | ANDBX | rA & (rB ^ rC) | | 0x36 | NORXNOR | ~(rA \| ~(rB ^ rC)) |
| 0x1F | ANDBC | rA & (rB & rC) | | 0x37 | ANDNAND | rA & ~(rB & rC) |
| 0x25 | ORBC | rA \| (rB & rC) | | 0x38 | ANDNOR | rA & ~(rB \| rC) |
| 0x26 | ORBO | rA \| (rB \| rC) | | 0x39 | ANDXNOR | rA & ~(rB ^ rC) |
| 0x27 | ORBX | rA \| (rB ^ rC) | | 0x3A | ORNAND | rA \| ~(rB & rC) |
| 0x28 | NANDAND | ~(rA & (rB & rC)) | | 0x3B | ORNOR | rA \| ~(rB \| rC) |
| 0x2A | NANDOR | ~(rA & (rB \| rC)) | | 0x3C | ORXNOR | rA \| ~(rB ^ rC) |
| 0x2B | NANDXOR | ~(rA & (rB ^ rC)) | | 0x3D | XORNAND | rA ^ ~(rB & rC) |
| 0x2C | NANDNAND | ~(rA & ~(rB & rC)) | | 0x3E | XORNOR | rA ^ ~(rB \| rC) |
| 0x2D | NANDNOR | ~(rA & ~(rB \| rC)) | | 0x3F | XORXNOR | rA ^ ~(rB ^ rC) |
| 0x2E | NANDXNOR | ~(rA & ~(rB ^ rC)) | | | |
| 0x2F | NORAND | ~(rA \| (rB & rC)) | | | |

**ALU — immediate (0x20–0x3F region):**

| Opcode | Mnemonic | Operation | | Opcode | Mnemonic | Operation |
|---|---|---|---|---|---|---|
| 0x20 | ADDI | rd = rA + imm13 | | 0x30 | ZERO | rd = 0 |
| 0x21 | ANDI | rd = rA & imm16z | | 0x31 | ONE | rd = 1 |
| 0x22 | ORI | rd = rA \| imm16z | | | |
| 0x23 | XORI | rd = rA ^ imm16z | | | |
| 0x24 | LUI | rd = imm20 << 12 | | | |
| 0x29 | RORI | rd = rA ror imm | | | |

**Shifts (0x40–0x43):** LSL, LSR, ASR, ROR (`rd = rA <op> rB`; barrel shifter,
ALU LUTs idle).

**Memory (0x60–0x69):** LW (0x60), LB (0x61, sext byte), LH (0x62, sext half),
LBU (0x63, zext byte), LHU (0x64, zext half), SW (0x68), SB (0x69).
Loads are 2 cycles, `addr = rA + imm13`. Stores are 2 cycles,
`addr = rB + imm8`, data from rA.

**Branches (0x80–0x83):** BEQ, BNE, BLT, BGE — 1 cycle, 22-bit PC-relative
offset; conditions from the flag file (Z, ~Z, LT, GTE).

**Jumps (0xA0–0xA4):** JMP (PC = abs22), JAL (link; PC = abs22), RET
(PC = link register), JR (PC = rB), JALR (rd = PC+1; PC = ALU result).

**Stack / IO (0xC0–0xC9):** PUSH (0xC0: SP−=, mem[SP] = rA, 2 cycles),
POP (0xC1: rd = mem[SP], SP +=, 2 cycles), IN (0xC8: rd = keyboard data byte),
OUT (0xC9: io_out = rA[7:0]).

**System (0xE0–0xFF):** FENCE (0xE0, ordering nop), ECALL (0xE1, trap to word
0x100), EBREAK (0xE2, halt/debug), HALT (0xFF).

### 2.4 Pseudo-ops (assembler-level)

From `docs/isa/tomato.v1.pseudo.csv`: `NOP` → `MOV r0, r0`; `CALL label` →
`JAL r16, label` ("the calling convention — **r16 is the link register**");
`MV` → `ADD rd, rA, r0`; `NEG`/`NOT`/`INC`/`DEC` map to the corresponding
real ops.

### 2.5 Flags

The flag file carries **Z, N, C, V** plus derived **LT, GT, GTE** (branch
condition codes observed: 0=BEQ(Z), 1=BNE(~Z), 2=BMI(N), 3=BCS(C), 4=BVS(V),
5=BLT(LT), 6=BGT(GT), 7=BGE(GTE)). `flags_we` is per-opcode: CMP/TST/TEQ/CMN
write flags without a register result; MOV does **not** write flags. Exact
flag-setting semantics per op (e.g. which of Z/N/C/V each ALU op updates) are
**not fully public** — see GAP-LIST item 4.

---

## 3. ALU / datapath — reality check vs the Lobster spec hypothesis

The Lobster spec (§52–§58) hypothesized a polymorphic dual-LUT3 ALU and designed
ADD3/LUT3 synthesis around it. **The hardware hypothesis is confirmed; the
ISA hypothesis is not.** Details:

### 3.1 What the datapath really does

- Per bit: `out = f(a,b,c) + g(a,b,c) + carry_in`, where f and g are two
  independent 3-input LUTs (one truth-table byte each) and `carry_in` comes
  from a per-opcode-selectable source.
- Carry ripples between bit-groups: per 4-bit nibble per the project site;
  the FPGA RTL (`hardware/fpga/core/rtl/alu.v`) chains four `alu8b` blocks
  (`c8, c16, c24, c32`), i.e. byte-wise ripple in that build.
- Theoretical space: 256 × 256 × 8 carry sources = **524,288 configurations**
  (the number on tomato.tmarhguy.com).
- `docs/architecture.md` states explicitly: this theoretical space **"is not
  an instruction count."** The burned ISA is 92 rows.

### 3.2 The burned (f, g, carry) catalog

`tools/gen_microcode_v1.py` contains the authoritative `DUAL` table mapping
each mnemonic to its **(lutA, lutB, csel)** triple, burned into
`microcode/alu_control_1.hex` (lutA), `microcode/alu_lut_b.hex` (lutB), and
the csel field of the control ROM. Key entries (truth tables indexed
`idx = 4C+2B+A`, bit 0 = answer for input 000):

| Mnemonic | lutA (f) | lutB (g) | csel | Meaning |
|---|---|---|---|
| ADD | 0xAA (pass A) | 0xCC (pass B) | 0 | A + B + 0 |
| ADC | 0xAA | 0xCC | 4 | A + B + carry flag |
| SUB | 0xAA | 0x33 (~B) | 1 | A + ~B + 1 |
| SBC | 0xAA | 0x33 | 4 | A − B + C_in |
| RSB | 0x55 (~A) | 0xCC | 1 | ~A + B + 1 = B − A |
| NEG | 0x55 | 0x00 | 1 | ~A + 0 + 1 |
| INC | 0xAA | 0x00 | 1 | A + 1 |
| DEC | 0xAA | 0xFF | 0 | A + 0xFF = A − 1 |
| ONE | 0x00 | 0x00 | 1 | cin only → 1 |
| AND | 0x88 (A&B) | 0x00 | 0 | f only |
| XOR | 0x66 (A^B) | 0x00 | 0 | f only |
| MASKADD | 0xAA | 0xC0 (B&C) | 0 | A + (B&C) |
| XORAND | 0x96 (XOR3) | 0x80 (AND3) | 0 | (A^B^C) + (A&B&C) |
| ANDADD | 0x88 | 0xF0 (pass C) | 0 | (A&B) + C |
| ORADD | 0xEE (A\|B) | 0xF0 | 0 | (A\|B) + C |
| XORADD | 0x66 | 0xF0 | 0 | (A^B) + C |
| CSEL | 0xAC (C?A:B) | 0x00 | 0 | mux, no carry |

Known `csel` values (from `hardware/fpga/core/rtl/alu.v`, lines 70–81; the
flag register layout is `csr: [0]Z [1]~Z [2]N [3]C [4]V [5]LT [6]GT [7]GTE`):

| csel | Carry-in source |
|---|---|
| 0 | constant 0 |
| 1 | constant 1 |
| 2 | N flag (`flags[2]`) |
| 3 | Z flag (`flags[0]`) |
| 4 | C flag (`flags[3]`) |
| 5 | GT flag (`flags[6]`) |
| 6 | LT flag (`flags[5]`) |
| 7 | V flag (`flags[4]`, default branch) |

So the "8 carry-source selections" are: 0, 1, and six flag bits. Note the
burned ISA only uses csel 0, 1, and 4 today — flag-fed carry (2, 3, 5, 6, 7)
is datapath-legal but unburned, i.e. a ROM-growth opportunity (e.g. an op
that adds the Z flag directly).

Implementation nuance: the FPGA RTL chains carry through four `alu8b`
blocks (`c8, c16, c24, c32` in `alu.v`) — byte-wise ripple in this build,
not the per-nibble ripple described on the project site (which may describe
the discrete build).

### 3.3 What this means for Lobster's polymorphic synthesis (§52–§58)

1. **Arbitrary LUT synthesis is impossible at the ISA level.** There is no
   instruction field carrying a truth-table byte, and the LUT programs are
   bound to the 9-bit opcode by the control ROM. A Lobster pattern like
   "synthesize (f=0x96, g=0xE8, cin=1) as one op" has **no encoding** — unless
   that exact triple happens to be a burned row.
2. **The correct implementation is catalog lookup, not synthesis.**
   Lobster's Tomato backend should keep a table of the 92 burned rows with their
   (lutA, lutB, csel) triples (from the DUAL table + CSV) and match IR
   patterns against it. This is exactly what the spec's §58 "whole-ALU pattern
   recognition" should become: *pattern → burned opcode*, not *pattern →
   fresh (f,g,cin)*.
3. **The compound 3-input ops already exist.** MASKADD/XORAND/ANDADD/ORADD/
   XORADD/CSEL/ADC/SBC plus the 32 nested-Boolean forms give Lobster a rich
   fixed catalog of the "ADD3/LUT3" shapes the spec wanted — they just can't
   be extended without a ROM change.
4. **ROM growth is the escape hatch.** The CSV header says rows are added
   "as you need more", and `tools/gen_microcode_v1.py --rebuild` regenerates
   the CSV from burn rows + growth templates (unused rows are NOP-filled, "no
   phantom growth mnemonics"). The owner can burn new (f,g,csel) triples into
   currently-unused opcodes. Since Tyrone is both the CPU owner and the Lobster
   owner, Lobster could in principle request new rows — but the Lobster backend must
   treat the burned set as fixed input, versioned per ROM release.
5. **The "hardware-level compiler" doesn't help.** `hardware/fpga/core/rtl/
   compiler_fsm.v` is a counter FSM behind MMIO window `0x780080` that sweeps
   65,536 `{lutB, lutA}` pairs for *fixed* A/B/C/carry/expected-output
   values, stopping at the first match (~10.5 ms at 6.25 MHz). Per
   `docs/status.md`: "A hit verifies only that fixed example; it does not
   prove a full truth table or general function." It **cannot** program
   arbitrary LUTs from software — it is a search oracle for one frozen test
   vector, not a JIT.

### 3.4 LUT3 catalogs (for the Lobster pattern table)

- `docs/isa/lut.csv` — **98 practical LUT3 opcodes** (fixed A/B/C pin order),
  80 unique shapes → 98 routing encodings, with names (ZERO, NOR3, MUX_C_AB,
  XOR3, MAJ, BORROW, …), expressions, and truth tables. This is the raw
  material for Lobster's single-LUT (g=0) patterns.
- `docs/isa/luts.csv` — full 256 LUT3 truth tables.
- `docs/isa/lut-nested-fuse.csv` — 32 nested-Boolean → single-LUT fusions
  (the "two-cycle Boolean nests collapse to one f-LUT (g=0, cin=0)" table).
- `docs/isa/unique-ops.csv`, `alu8.csv`, `datapath-audit.csv`,
  `profiles.csv` (foreign-ISA mappings — useful for Lobster's lowering tables).

---

## 4. ABI

**Mostly unpublished.** What is known:

- **Link register: r16.** `CALL label` → `JAL r16, label`; `RET` = `PC =
  link`. (From `tomato.v1.pseudo.csv`.)
- **Trap/syscall convention** (from the sample `software/asm/ecall.s`):
  ECALL traps to word address `0x100`, saving PC to the link register;
  `r1` = syscall number, `r2` = arg0; handler returns with result in `r1`;
  `RET` resumes. Syscalls in the sample: 0 = exit, 1 = put (character out),
  2 = getc (character in). This is a *sample convention*, not a published
  ABI.
- **Stack:** PUSH/POP with a hardware SP (`sp_op` 2 = decrement on push,
  1 = increment on pop). Stack lives in dmem (`0x000000`).
- **Not public:** argument/return register assignment, callee-saved set,
  stack frame layout, stack alignment, SP initial value, varargs, TLS.
  → GAP-LIST items 1–2. Lobster's Tomato ABI (spec §47) will have to **define**
  these itself, in consultation with the owner.

---

## 5. Microarchitecture notes relevant to codegen

### 5.1 Writeback / operand steering (`wb_sel`, `ir_imm_sel`, `mem_sel`)

From `tools/gen_microcode_v1.py` burn rows:

- `wb_sel`: 0 = ALU result, 1 = shifter, 2 = PC+1 (JAL/JALR link), 3 = memory
  load data, 4 = store-data path, 5 = immediate (LUI/MOVI/SEXTI), 7 = mul/div
  high word. (Values 6 unused in burn set.)
- Loads: `addr = rA + imm13` (`ir_imm_sel=3`, `mem_sel=0`). Stores:
  `addr = rB + imm8` (`ir_imm_sel=0`, `mem_sel=1`).
- `byte_sel`: 0 = word, 4 = LB (sext byte0), 5/6/7 = byte lanes 1–3,
  8 = LBU (zext byte), 12 = LH (sext half), 14 = LHU (zext half), 2 = IN
  (keyboard data).
- Shifts go through the barrel shifter (`wb_sel=1`), not the LUT ALU.
- MUL/DIV/REM are single-cycle in the burn rows (`cycles=1`, `mul_en`/
  `div_en`); 2-cycle ops are loads, stores, PUSH, POP.

### 5.2 Memory map (word addresses)

From `hardware/fpga/core/README.md`:

| Base | Size | Purpose |
|---|---|---|
| `0x000000` | 16384 × 32-bit | dmem — program + stack |
| `0x300000` | — | framebuffer (80×60 tiles) |
| `0x780000` | — | keyboard/timer MMIO: keycode, ready/consume, ms timer, CPU-freq words — **polled**, no interrupt controller |
| `0x780080` | — | "compiler" FSM window (search oracle, §3.3.5) |
| `0x780100` | — | nRF8001 ACI radio mailbox |

16-bit dmem image: program loaded at word 0; ECALL vector at word `0x100`.

### 5.3 Interrupts / traps

- **No interrupt controller found** in any public doc. Keyboard and timer are
  polled (`menu_wait` in TomatoOS polls keyboard MMIO; the "One Press, One
  Key" journal describes the polled design).
- ECALL is a synchronous trap to word `0x100` with PC saved to the link
  register; RET resumes. EBREAK halts (debug).
- Lobster's runtime cannot rely on timer interrupts or preemptive scheduling on
  this target — cooperative / poll-based designs only, unless the owner adds
  interrupt hardware.

### 5.4 Performance model

- 6.25 MHz CPU clock (100 MHz / 16), 25 MHz pixel clock.
- ~2.13 CPI measured in Icarus samples; most ALU ops 1 cycle, loads/stores/
  PUSH/POP 2 cycles.
- For Lobster's cost model: start with `cost = cycles` from the CSV `cycles`
  column (1 for ALU, 2 for mem/stack), refined later against the emulator.

---

## 6. Toolchain (what exists today)

- **Assembler:** `software/assembler.py` — two-pass, driven directly by the
  ISA CSVs (`docs/isa/tomato.v1.csv` + pseudo-op CSV). Usage:
  `python3 software/assembler.py software/asm/counter.s -o <mem> --list`;
  has a `--selftest` mode. This is the ground truth for instruction
  *encoding* — Lobster's encoder should be cross-checked against it.
- **Build:** `make -C hardware/fpga/core asm/burn/os/test`.
- **Simulators:** Icarus Verilog testbenches (`hardware/fpga/core/tb/`);
  `tools/virtual_tomato.py` (Python); `tools/virtual_tomato.cpp`;
  the browser emulator `web/js/tomato-cpu.js` (functional, "behavior not
  timing") with an 80/80 test suite (`web/tests/tomato-emu.mjs`). Any of
  these can serve as Lobster's Tomato CI oracle; the Python or JS one is the
  cheapest to integrate.
- **OS:** TomatoOS v3.0 in `software/os/` (14 menu entries: games, demos,
  utilities), assembled into burn headers under `rtl/burn/`. `software/os/
  README.md` documents the menu and build.
- **Remote compute:** `tools/remote_compile.py` + `docs/compute.md` describe a
  "send Tomato program to a remote board, get the result back" flow — an
  interesting future Lobster deployment story (compile locally, run on hardware).
- **Samples:** `software/asm/` — `call.s`, `fib.s`, `ecall.s`, `counter.s`,
  etc. `fib.s` and `call.s` are the best available examples of calling
  convention in practice.

---

## 7. TomatoOS notes (for Lobster's runtime story)

- Menu-driven OS, 14 entries, polled keyboard input, framebuffer text output
  (80×60 tiles at `0x300000`).
- Programs are assembled to dmem images and burned; there is no loader or
  dynamic linking — Lobster binaries would be bare-metal images in the same
  style.
- The OS sources are the largest corpus of real Tomato assembly in existence;
  mining them for idiom patterns (addressing, loops, MMIO) is worthwhile
  before writing Lobster's instruction selector.

---

## 8. What Lobster's Tomato backend needs (derived requirements)

1. **Encoder** for the four formats (§2.1), validated against
   `software/assembler.py --selftest` vectors and the emulator.
2. **Pattern catalog**: the 92 burned rows with (lutA, lutB, csel) triples —
   machine-readable, generated from `tomato.v1.csv` + the DUAL table, so it
   tracks ROM growth.
3. **ABI definition** (Lobster-owned, owner-reviewed): arg/return regs,
   callee-saved set, frame layout, SP init — see GAP-LIST.
4. **Cost model**: cycles column + 2.13 CPI baseline; compound ops
   (MASKADD etc.) priced at 1 cycle make them strictly better than 2-op
   sequences.
5. **CI oracle**: `web/js/tomato-cpu.js` or `tools/virtual_tomato.py` to
   execute Lobster's emitted binaries.

---

## 9. Gaps (summary — full list in GAP-LIST.md)

1. Full calling convention (args, returns, callee-saved, frames).
2. SP init / stack layout / alignment.
3. Remaining carry-source select definitions (csel 2, 3, 5, 6, 7).
4. Per-op flag semantics (which of Z/N/C/V each op sets).
5. Whether any interrupt/exception mechanism beyond ECALL exists.

---

## 10. Glossary (Tomato-specific terms)

- **Polymorphic (Tomato sense):** the ALU's *theoretical* ability to take
  524,288 configurations — not a runtime-reconfigurable ISA feature.
- **Dual-LUT:** the two 3-input LUTs (f, g) per bit-slice.
- **Burn / burned row:** a control-ROM entry with status=burn (real
  instruction); vs `growth` (template for future rows) and `nop` (unused).
- **csel:** carry-source select, the third element of a DUAL triple.
- **ir_imm_sel:** microcode field selecting the immediate/operand shape.
- **wb_sel:** microcode field selecting the writeback source.
- **LUT3 index order:** `idx = 4C+2B+A`; bit 0 of the truth-table byte is the
  output for input 000 (e.g. 0xAA = pass A, 0xCC = pass B).

---

## 11. Changelog

- 2026-10-01: initial research (TOMATO-ISA RESEARCH workstream). Covers repo
  HEAD as verified 2026-09-16 by `docs/status.md`.

---

## 12. Sources

All URLs accessed 2026-10-01. Blob SHAs are git blob hashes at repo HEAD
(`main`).

**ISA authority:**
- `docs/isa/tomato.v1.csv` —
  https://github.com/tmarhguy/tomato/blob/main/docs/isa/tomato.v1.csv
  (blob `599789e3ee8e86c9789761c285e2156a3d28229f`)
- `docs/isa/README.md` —
  https://github.com/tmarhguy/tomato/blob/main/docs/isa/README.md
  (blob `8adf8e7262b2c516c2b54a23b4269caf42340679`)
- `docs/isa/lut.csv` (98 practical LUT3 opcodes) —
  https://github.com/tmarhguy/tomato/blob/main/docs/isa/lut.csv
- `docs/isa/luts.csv` (full 256 truth tables) —
  https://github.com/tmarhguy/tomato/blob/main/docs/isa/luts.csv
- `docs/isa/lut-nested-fuse.csv` (32 nested-Boolean fusions) —
  https://github.com/tmarhguy/tomato/blob/main/docs/isa/lut-nested-fuse.csv
  (blob `f60a2d37b0fd27782cb060a9b30924a12594bd14`)
- `docs/isa/tomato.v1.pseudo.csv` —
  https://github.com/tmarhguy/tomato/blob/main/docs/isa/tomato.v1.pseudo.csv
  (blob `4a26273d0d7175d5c303cf347382fe669f868c86`)
- `docs/isa/unique-ops.csv`, `alu8.csv`, `datapath-audit.csv`,
  `profiles.csv`
  (same directory)

**Microcode / datapath:**
- `tools/gen_microcode_v1.py` (DUAL table, burn rows, growth templates) —
  https://github.com/tmarhguy/tomato/blob/main/tools/gen_microcode_v1.py
  (blob `5c4225e57cfba69b1b3810e000141278d20d7099`)
- `docs/architecture.md` — https://github.com/tmarhguy/tomato/blob/main/docs/architecture.md
- `docs/status.md` (canonical facts, verified 2026-09-16) —
  https://github.com/tmarhguy/tomato/blob/main/docs/status.md
- `docs/compute.md` — https://github.com/tmarhguy/tomato/blob/main/docs/compute.md
- `hardware/fpga/core/rtl/compiler_fsm.v` (hardware "compiler" scope)
- `hardware/fpga/core/rtl/alu.v` (carry-source select definitions, flag
  register layout, byte-wise carry chain) — read from the sibling checkout
  at `../tomato`
- `hardware/fpga/core/README.md` (memory map, build flow)

**Software / OS / toolchain:**
- `software/README.md`, `software/assembler.py` —
  https://github.com/tmarhguy/tomato/blob/main/software/assembler.py
  (blob `25c6d72d4d4c907658e676f62b07b61383ebd5c8`)
- `software/os/README.md`
- `software/asm/call.s`, `fib.s`, `ecall.s`, `counter.s`
- `tools/virtual_tomato.py`, `tools/remote_compile.py`
- `web/js/tomato-cpu.js`, `web/tests/tomato-emu.mjs`

**Project site & journals:**
- https://tomato.tmarhguy.com (524,288 configurations, polymorphic explainer,
  RV32I comparison examples incl. XOR3+AND3 one-shot: F=0x96, G=0x80, cin=0)
- https://github.com/tmarhguy/tomato/blob/HEAD/docs/log/2026-07-31%20-%20Falling%20back%20to%2032b.md
- https://github.com/tmarhguy/tomato/blob/HEAD/docs/log/2026-07-30%20-%20Redesign%20into%2040b%20(Old%20design%20=%2032b).md
- https://github.com/tmarhguy/tomato/blob/HEAD/log/2026-08-15%20-%20ISA%20as%20a%20Wire.md
- https://github.com/tmarhguy/tomato/blob/HEAD/docs/log/2026-08-28%20-%20One%20Press,%20One%20Key.md
