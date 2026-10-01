// Expression shapes Lobster's Tomato backend matches against the 92-row
// burned catalog (ADR 004): compound 3-input ops, nested-Boolean forms,
// and carry chains. Every item here must pass `lobster check`.
// Catalog reference: docs/tomato/ISA-RESEARCH.md §3.2.

// MASKADD: rd = rA + (rB & rC)
fn maskadd(a: u32, b: u32, c: u32) -> u32 {
    a + (b & c)
}

// ANDADD: rd = (rA & rB) + rC
fn andadd(a: u32, b: u32, c: u32) -> u32 {
    (a & b) + c
}

// ORADD: rd = (rA | rB) + rC
fn oradd(a: u32, b: u32, c: u32) -> u32 {
    (a | b) + c
}

// XORADD: rd = (rA ^ rB) + rC
fn xoradd(a: u32, b: u32, c: u32) -> u32 {
    (a ^ b) + c
}

// XORAND: rd = (rA ^ rB ^ rC) + (rA & rB & rC)
fn xorand(a: u32, b: u32, c: u32) -> u32 {
    (a ^ b ^ c) + (a & b & c)
}

// MUX shape: (a & b) | (!a & c)
fn mux(a: u32, b: u32, c: u32) -> u32 {
    (a & b) | ((!a) & c)
}

// Nested-Boolean forms (lut-nested-fuse catalog), e.g. XORBC.
fn xnor_nest(a: u32, b: u32, c: u32) -> u32 {
    a ^ (b & c)
}

fn or_nand(a: u32, b: u32, c: u32) -> u32 {
    a | (!(b & c))
}

// ADC shape: rd = rA + rB + carry
fn adc_chain(a: u32, b: u32, carry: u32) -> u32 {
    a + b + carry
}

// Two independent Boolean functions (Dual-LUT packing candidates).
fn dual(a: u32, b: u32, c: u32, d: u32) -> u32 {
    let x = !(a & b);
    let y = !(c ^ d);
    x + y
}

// Flagship compound shape: LUT A + LUT B + carry in one op.
fn poly(a: u32, b: u32, c: u32, d: u32, e: u32, carry: u32) -> u32 {
    ((a ^ b) & c) + ((!d) & e) + carry
}

fn main() {
    let r = maskadd(1u32, 2u32, 3u32) + poly(1u32, 2u32, 3u32, 4u32, 5u32, 0u32);
    println(r);
}
