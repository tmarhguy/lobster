# Lobster language specification

Fleshed out from the project spec, §6–§19. This document is the normative
reference for the language surface; the compiler workstreams (lexer, parser,
name resolution, type system, MIR, optimizer) implement it.

Conventions used below:

- **Decided** — a rule the language commits to. The optimizer and backends may
  rely on it.
- **Open question** — explicitly undecided. Do not rely on it; it is tracked
  for a future edition or ADR.

The highest-stakes decisions live in §4 (integer semantics) and §5
(float semantics): target-aware optimization (e.g. reassociating
`(a + b) + c` into `ADD3(a, b, c)`) is legal *only* where these sections
allow it.

---

## 1. Lexical structure

Source files are UTF-8 and use the `.lobster` extension. A file is a sequence of
tokens; whitespace (spaces, tabs, newlines) separates tokens and is otherwise
insignificant. There are no significant-indentation rules.

### Keywords

The following are reserved and cannot be used as identifiers:

```text
as      break    const    continue  else     enum     false    fn
for     if       impl     import    in       let      loop     match
module   mut     pub      return    self     Self    static   struct
trait    true     type     unsafe    where    while
```

Additionally reserved for future use (using them as identifiers is an error
today so they can be adopted without breaking code): `async`, `await`,
`dyn`, `extern`, `macro`, `move`, `ref`, `try`, `union`.

**Open question:** the final keyword list (e.g. whether `in`, `where`, `type`
earn their reservation, and whether `use` is needed alongside `import`).

### Identifiers

```text
identifier := [A-Za-z_] [A-Za-z0-9_]*
```

Identifiers are ASCII-only. They may not start with a digit, and may not be a
keyword.

**Open question:** Unicode identifiers (XID rules) and raw identifiers
(`r#fn`-style escapes) are deferred to a later edition.

### Literals

Integer literals:

```text
123  1_000_000          decimal; `_` separators are ignored
0xff  0xFF             hexadecimal
0o17                   octal
0b1010                 binary
42u64  10i32  7usize    type suffixes: i8/i16/i32/i64, u8/u16/u32/u64,
                       f32/f64, usize/isize
```

An unsuffixed integer literal defaults to `i32` when its type is otherwise
unconstrained (decided; see §3). A literal that does not fit its type is a
compile error — literals never silently wrap.

Float literals: `1.0`, `1e10`, `1.5f32`, `2.0e-3`. Decimal→binary conversion
is correctly rounded. An unsuffixed float literal defaults to `f64`.

Character literals: `'a'`, `'\n'`, `'\x41'`, `'\u{1F600}'`. A `char` is a
Unicode scalar value. Supported escapes: `\n \t \r \\ \' \" \0 \xHH
\u{H…}`.

String literals: `"…"` — UTF-8 with the same escapes as `char`.

**Open question:** byte strings (`b"…"`), raw/multi-line strings, and string
interpolation are deferred.

### Comments

- Line comments: `//` to end of line.
- Block comments: `/* … */`, which **nest** (decided).
- Doc comments `///` (item) and `//!` (module) are lexed as comments today;
  `lobster doc` will consume them later.

### Operators and punctuation

```text
+  -  *  /  %  <<  >>  &  |  ^  !  ~ (reserved)
&&  ||  ==  !=  <  <=  >  >=
=  +=  -=  *=  /=  %=  <<=  >>=  &=  |=  ^= 
->  =>  :  ;  ,  .  ..  ::  #  ?  _
(  )  [  ]  {  }
```

The exact operator set is provisional until the parser workstream's grammar
is frozen — **open question** whether `~`, `..`, `...`, `@` earn a meaning.

---

## 2. Syntax

This section describes the accepted surface. The authoritative grammar lives
with the parser workstream (`lobster_parser`); this document states intent and
semantic notes.

### Items

A file is a sequence of *items*: functions, structs, enums, traits, impl
blocks, modules, imports, constants, statics, and type aliases.

```rust
fn fib(n: u64) -> u64 {
    if n < 2 {
        return n;
    }
    fib(n - 1) + fib(n - 2)
}

const MAX: u64 = 1_000_000;
static SEED: u64 = 0x1234_5678;
type Meters = f64;
```

- `fn` declares a function. Parameter and return types are always written
  explicitly (decided: no top-level type inference of signatures).
- Generic functions: `fn max<T: Ord>(a: T, b: T) -> T` (monomorphized; §13).
- Methods are `fn`s inside `impl` blocks; receivers are written explicitly:
  `fn area(self: &Self) -> f64`.
- Closures are **not** in the initial language (**open question:** capture
  semantics — by reference, by move, or both — when they are added).

### Variables

```rust
let x = 10;        // immutable, type inferred
let mut y = 20;    // mutable
y += x;
let (a, b) = pair; // tuple destructuring
```

`let` bindings support tuple and struct destructuring patterns (decided).
Whether `let` supports arbitrary `match`-style patterns (slices, guards) is
an **open question**.

### Structs and enums

```rust
struct Point {
    x: f32,
    y: f32,
}

enum Option<T> {
    Some(T),
    None,
}
```

Field and variant payloads are typed explicitly. Tuple structs, unit structs,
and `#[repr(...)]` layout control are **open questions**.

### Pattern matching

```rust
match value {
    Some(x) => use(x),
    None => fallback(),
}
```

`match` arms support literal, binding, wildcard (`_`), struct, tuple, and enum
variant patterns. Matches on non-`Copy` values move by default. Match guards
(`if` conditions on arms) and exhaustiveness checking are intended but their
exact rules are an **open question** pending the type-system workstream.

### Control flow

```text
if / else        (expression: `let x = if c { 1 } else { 2 };`)
while            (`while cond { … }`)
loop             (infinite; exits via `break`)
for              (`for x in iter { … }`)
break / continue
return
match
```

`if` without `else` in expression position yields `()` (decided).
Loop labels (`'outer:`) and `break 'outer` are **open questions**.
The `for` loop desugars to the iterator protocol, whose trait design is an
**open question**.

---

## 3. Types

### Primitive types

| Type | Meaning |
|---|---|
| `i8`, `i16`, `i32`, `i64` | signed two's-complement integers |
| `u8`, `u16`, `u32`, `u64` | unsigned integers |
| `f32`, `f64` | IEEE 754 binary32 / binary64 |
| `bool` | `true` / `false` |
| `char` | Unicode scalar value |
| `usize`, `isize` | pointer-sized integers (16/32/64 bits per target; 32 on Tomato32) |

### Composite types

- Arrays: `[T; N]` — `N` elements inline, `N` a compile-time constant.
- Slices: `[T]` — dynamically-sized views (always behind a reference).
- Tuples: `(T, U)`; the unit type `()` is the zero-tuple.
- Structs, enums (see §2).
- Function types: `fn(A, B) -> C`.
- References: `&T`, `&mut T` (see §6).
- Raw pointers: `*const T`, `*mut T` (see §6; dereference requires `unsafe`).

### Inference

Type inference is *local*: `let` bindings and subexpressions infer types, but
every `fn` signature, `struct` field, and `static` is annotated explicitly
(decided). Unsuffixed integer literals default to `i32`, unsuffixed float
literals to `f64`, unless context constrains them to another type.

**Open questions:** the never type (`!`), trait objects (`dyn Trait`),
const generics, and the contents of the prelude (`Option`, `Result`,
`println`, …).

---

## 4. Integer semantics

This section is normative for the optimizer: a transformation is legal only
if it preserves these semantics bit-for-bit, including traps.

Representation is two's complement for a fixed width N ∈ {8, 16, 32, 64}
(`usize`/`isize` use the target's pointer width).

### Wrapping arithmetic (default)

`+`, `-`, `*`, and unary `-` **wrap modulo 2^N**. Wrapping never traps and is
never undefined behavior, in any build mode (decided).

Rationale: wrapping makes reassociation semantics-preserving for *every*
target, which is what lets the optimizer fold `(a + b) + c` into `ADD3(a, b,
c)` or a LUT network without a legality proof per site. It also matches
hardware ALUs (including Tomato's LUT fabric) directly.

Explicit spellings (same semantics as the operators; they exist for
documentation and generic code):

```rust
a.wrapping_add(b)  a.wrapping_sub(b)  a.wrapping_mul(b)
```

### Checked arithmetic

```rust
a.checked_add(b) -> Option<T>   // None on overflow
```

`checked_sub`, `checked_mul`, `checked_div`, `checked_shl`-family likewise
return `None` instead of wrapping (decided).

**Open question:** whether `saturating_*` and `overflowing_* -> (T, bool)`
families ship in core, and whether `Option` or `Result` is the checked
return type long-term. Also open: a per-profile `overflow-checks = true`
flag that turns default operators into trapping arithmetic for debug builds.

### Division and remainder

`/` is *truncated* division (quotient rounds toward zero); `%` is the
remainder with the sign of the dividend. Consequences:

- **Division or remainder by zero traps** — deterministic abort (decided).
  Trapping operations are observable side effects: the optimizer may not
  remove, speculate, or reorder them across sequence points.
- `iN::MIN / -1` (and `iN::MIN % -1`) **wraps** to `iN::MIN` (resp. `0`)
  (decided) — consistent with wrap-everything; not a trap, not UB.

### Shifts

`<<` shifts left (low bits fill with zero). `>>` is an *arithmetic* shift on
signed types (sign-extending) and a *logical* shift on unsigned types
(decided).

The shift amount is **masked**: the effective amount is `amount & (N - 1)`
(decided). Overshifts are not errors, not traps, not UB — they shift by the
masked amount. This matches x86-64 and WebAssembly and keeps shift
lowering branchless.

### Bitwise operations and comparisons

`&`, `|`, `^`, `!` operate bitwise and are total. All six comparisons are
total orders on the integer's mathematical value.

### Casts (`as`)

Casts are explicit; there are **no implicit integer conversions** — not even
widening, and never between signed and unsigned of any width (decided, unlike
C). `let x: u64 = 1u32;` is a type error; write `1u32 as u64`.

| Cast | Semantics |
|---|---|
| widening `iN as iM` / `uN as uM`, M > N | value-preserving (sign- / zero-extend) |
| narrowing, M < N | keep the low M bits (two's-complement truncation) |
| same width, signedness change | bit reinterpretation |
| int → float | round-to-nearest-even; out of range → ±∞ |
| float → int | **saturating**: NaN → 0; ±∞ / out-of-range → MIN / MAX (decided) |
| `bool as int` | `false` → 0, `true` → 1 |
| int `as bool` | **rejected** — compare explicitly (`x != 0`) (decided) |
| `char as u32` | the Unicode scalar value |
| `u32 as char` | the scalar value; **traps** if not a valid scalar value (decided) |
| int ↔ `usize`/`isize` | as above, at pointer width |
| pointer ↔ int | allowed only in `unsafe` (see §6) |

### What the optimizer may do

Because `+` and `*` wrap, they are associative and commutative modulo 2^N:
reassociation, n-ary synthesis (`ADD3`, `MUL3`, LUT networks), and
reordering are **always legal** for `+`/`*` on integers. `-` is not
associative and must be normalized first (e.g. `a - b - c` →
`a + (-b) + (-c)`, noting `-iN::MIN` wraps). Division, remainder, shifts by
non-constant amounts, and any trapping operation may not be reassociated,
speculated, or eliminated. The rule: an optimization is legal iff it
preserves the observable sequence of wrapped results and traps.

---

## 5. Floating-point semantics

### Default: strict IEEE

`f32`/`f64` follow IEEE 754-2019 binary32/binary64 with round-to-nearest,
ties-to-even:

- NaNs propagate (quiet); NaN payload bits are unspecified — **open question**.
- Signed zeros, infinities, and gradual underflow (denormals) behave per IEEE.
- Comparisons follow IEEE: `NaN != NaN`, so `x == x` is false for NaN.
- **No reassociation**: `(a + b) + c` must not become `a + (b + c)`.
- **No contraction**: `a * b + c` must not fuse into an FMA unless the program
  calls an explicit fused intrinsic.

The optimizer treats every float operation as order-sensitive by default.

### `--fast-math`: explicit opt-in

A `--fast-math` build flag (and, later, a per-function attribute — **open
question**) permits, *only* when explicitly enabled and fully documented:

- reassociation of float additions/multiplications;
- contraction (`a * b + c` → FMA);
- reciprocal approximations for division;
- ignoring NaN/Inf edge cases (e.g. assuming no NaNs);
- treating denormals as zero / flush-to-zero.

Fast-math is never the default. Code compiled with it is not bit-identical
to strict code, and mixing strict and fast-math functions in one program has
defined-but-function-local semantics: each function follows its own mode
(decided).

### Conversions

- int → float: round-to-nearest-even; magnitude out of range → ±∞.
- float → int: saturating, as in §4 (NaN → 0, out-of-range → MIN/MAX).
- `f64 as f32`: round-to-nearest-even; overflow → ±∞.
- `f32 as f64`: value-preserving.

**Open questions:** exact NaN payload propagation rules; decimal float
types; the per-function fast-math attribute syntax.

---

## 6. Memory model

Lobster is a systems language: the programmer controls placement and lifetime,
and the compiler never inserts garbage collection or hidden allocation.

### Placement

- `let` bindings live in stack slots in the current frame. Fixed-size arrays
  and structs are inline — no indirection.
- Heap allocation is explicit via `Box<T>` (unique ownership). The global
  allocator interface is an **open question** (intrinsic vs. library trait,
  and what `Box::new` lowers to in MIR).

### References and pointers

- `&T`: shared reference. `&mut T`: exclusive mutable reference.
- Raw pointers `*const T` / `*mut T`: creating one (including from an integer
  address, for MMIO) is safe; *dereferencing* one requires `unsafe`.

### Aliasing (interim rule)

At any program point, a place is accessible through either any number of
shared references or exactly one mutable reference — never both, and never
two mutable references (decided as the *statement* of the rule).

**Open question:** the checking mechanism. Spec §15 explicitly does not want
Rust's borrow checker cloned wholesale; the analysis (lexical lifetimes?
simpler region system? runtime-checked in debug?) is undecided. Until it is
decided, `unsafe` is the escape hatch and the rule above is enforced
conservatively.

### Moves and copies

Values move by default; moving a value invalidates the source binding
(decided). Types whose values are trivially duplicable opt into `Copy`
(**open question:** marker trait vs. keyword vs. attribute).

### Destruction

Destruction is deterministic: when an owner's scope ends, its drop glue runs
in reverse declaration order (decided). There are no finalizers and no GC.
Per spec §17, destruction becomes *explicit* during MIR lowering — as
`drop` terminators on the CFG — so optimization passes cannot accidentally
alter resource-lifetime semantics.

### Unsafe

`unsafe { … }` blocks and `unsafe fn` mark code the compiler does not verify
for memory safety. Required for:

- dereferencing raw pointers;
- MMIO and arbitrary memory operations, e.g.
  ```rust
  unsafe {
      let ptr = 0x8000 as *mut u32;
      *ptr = 10;
  }
  ```
  - foreign calls across the FFI boundary (**open question:** `extern` syntax);
- custom target intrinsics (e.g. Tomato configuration words);
- mutable global state (**open question:** whether `static mut` exists).

The soundness goal: **safe code cannot cause memory unsafety**. `unsafe`
does not disable type checking — only the aliasing/lifetime obligations the
compiler cannot prove.

**Open questions:** the `Send`/`Sync`-style story for data-race freedom;
union types; the exact allocator API.

---

## 7. Name resolution and modules

### Modules

```rust
module math;

pub fn sqrt(x: f64) -> f64 { … }
```

Items are private to their module by default; `pub` makes an item visible to
importers (decided). Finer-grained visibility (`pub(crate)`-style) is an
**open question**.

**Open question:** the file↔module mapping (candidate: `math.lobster` ↔ `module
math;`, with `math/mod.lobster` for directory modules) and the module root
conventions.

### Imports

```rust
import math::sqrt;
import tomato::gpio;
```

Paths are `::`-separated. Whether glob imports (`import math::*`), renaming
(`import math::sqrt as root`), and the `crate::` / `self::` / `super::`
roots exist is an **open question**.

### Resolution rules

Name resolution (spec §22) resolves locals, globals, functions, modules,
imports, types, methods, and generic parameters, and reports as errors:
duplicate definitions, unknown identifiers, ambiguous imports, and
visibility violations. Resolution is order-independent within a module
(decided): items may be used before their textual definition.

**Open questions:** the prelude (which names are in scope without import);
whether resolution can change across editions.

---

## Appendix: decision log

Decisions marked "decided" above that most constrain downstream workstreams:

1. Integer `+`/`-`/`*` wrap (two's complement), never trap, never UB.
2. Division/remainder by zero traps; `MIN / -1` wraps.
3. Shift amounts are masked to `N - 1`.
4. Float→int casts saturate; `u32 as char` traps on invalid scalar values.
5. No implicit integer conversions of any kind.
6. Default float semantics are strict IEEE; reassociation needs `--fast-math`.
7. Deterministic destruction, explicit in MIR; no GC, ever.
8. Items private by default; order-independent resolution within a module.
9. ASCII-only identifiers; nestable block comments.

Everything labeled **open question** is fair game for future ADRs — it must
not be assumed by the lexer, parser, or optimizer until decided.
