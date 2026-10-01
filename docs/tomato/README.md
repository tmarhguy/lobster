# Tomato32 target

Lobster's first-class custom-hardware target.

- `ISA-RESEARCH.md` — reverse-engineered/documented Tomato ISA: registers,
  instruction encodings, ALU/LUT structure, ABI, assembler, simulator.
  (Owned by the tomato-isa research workstream; the Tomato backend of spec
  §47–§67 must be derived from the *actual* Tomato ISA, never invented.)
- Future: `targets/tomato32/` backend crates (ABI, selector, synth, lut,
  cost, encoder).
