// Control flow: if/else, while, loop, for, break, continue, return, match.
// Every item here must pass `lobster check`.

fn classify(n: i32) -> i32 {
    if n < 0 {
        return -1;
    } else if n == 0 {
        return 0;
    }
    1
}

fn sum_to(n: u32) -> u32 {
    let mut total = 0u32;
    let mut i = 0u32;
    while i < n {
        total += i;
        i += 1;
    }
    total
}

fn find_first(limit: u32) -> u32 {
    let mut i = 0u32;
    loop {
        if i >= limit {
            break;
        }
        i += 1;
        continue;
    }
    i
}

fn consume_all(items: &[u32]) -> u32 {
    // Iteration protocol is an open question (spec §2); this shows syntax.
    let mut total = 0u32;
    for item in items {
        total += item;
    }
    total
}

fn describe(n: u32) -> u32 {
    match n {
        0 => 100u32,
        1 => 101u32,
        _ => 102u32,
    }
}

fn main() {
    let r = sum_to(10u32) + find_first(5u32);
    println(r);
}
