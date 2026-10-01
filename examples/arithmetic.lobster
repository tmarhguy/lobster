// Arithmetic: precedence, suffixes, casts, compound assignment.
// Every item here must pass `lobster check`.

fn add3(a: u32, b: u32, c: u32) -> u32 {
    a + b + c
}

fn bitmix(x: u32, y: u32) -> u32 {
    let shl = x << 2;
    let shr = y >> 1;
    let band = x & y;
    let bor = x | y;
    let bxor = x ^ y;
    shl + shr + band + bor + bxor
}

fn suffixed() -> u64 {
    let a = 42u64;
    let b = 0xffu64;
    let c = 1_000_000u64;
    a + b + c
}

fn converts(x: u32) -> u64 {
    let wide = x as u64;
    let back = wide as u32;
    back as u64
}

fn counter() -> u32 {
    let mut n = 0u32;
    n += 1;
    n *= 2;
    n -= 1;
    n
}

fn main() {
    let r = add3(1u32, 2u32, 3u32);
    println(r);
}
