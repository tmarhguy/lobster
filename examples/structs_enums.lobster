// Structs, enums, destructuring, patterns.
// Every item here must pass `lobster check`.
// (`import` is parsed but every import still dangles: modules are deferred,
// so no import appears in the passing examples.)

pub struct Point {
    x: f32,
    y: f32,
}

pub enum Option<T> {
    Some(T),
    None,
}

pub enum Dir {
    North,
    East,
    South,
    West,
}

fn get_x(p: Point) -> f32 {
    match p {
        Point { x, y: _ } => x,
    }
}

fn turn(d: Dir) -> Dir {
    match d {
        North => East,
        East => South,
        South => West,
        West => North,
    }
}

fn first(pair: (u32, u32)) -> u32 {
    let (a, b) = pair;
    a + b - b
}

fn main() {
    let r = turn(North);
    println(first((1u32, 2u32)));
}
