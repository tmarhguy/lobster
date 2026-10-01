//! Lobster reference interpreter: executes MIR CFGs with strict semantics.
//!
//! The interpreter is the third Commit 04 stage and the executable oracle
//! for everything before it. It runs [`MirProgram`] directly: no JIT, no
//! native code, just a frame stack over basic blocks with a fuel limit.
//!
//! Semantics follow `docs/language-spec.md` §§4–5 exactly:
//! - integer `+`/`-`/`*`/negation wrap modulo 2^N; `MIN / -1` wraps;
//! - `/` and `%` by zero trap; shift amounts are masked to `N - 1`;
//! - floats are strict IEEE (no reassociation anywhere in this crate);
//! - `float as int` saturates, `u32 as char` traps on invalid scalars;
//! - `&` / `*` trap honestly ([`TrapCode::RefOp`]): the memory model lands
//!   after Commit 04.
//!
//! Determinism: given the same program and arguments, `run` always
//! produces the same outcome. There is no randomness, no clock, no I/O
//! except captured `println` lines.

use lobster_ast::BinOp;
use lobster_mir::{CallTarget, Const_, MirFunc, MirProgram, Operand, Rvalue, Terminator};
use lobster_source::Span;
use lobster_types::{FloatTy, IntTy, Ty};

/// Trap codes. `TRAP-` prefixes never collide with checker `E2xx` codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrapCode {
    /// Division or remainder by zero.
    DivZero,
    /// Invalid `u32 as char` scalar value.
    BadChar,
    /// Slice index out of bounds.
    OutOfBounds,
    /// `&`, `&mut`, or `*` executed (deferred memory model).
    RefOp,
    /// Generic function called (deferred monomorphization).
    Generic,
    /// Bad callee, arity mismatch, or poisoned control flow.
    BadCall,
    /// Match fell through (checker proved this unreachable).
    NoMatch,
    /// Local read before assignment (poisoned input; checked code cannot).
    Uninit,
    /// Fuel exhausted (likely infinite loop).
    Fuel,
    /// Anything else the builder marked unreachable.
    Unreachable,
}

impl TrapCode {
    /// Stable code string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DivZero => "TRAP-DIV0",
            Self::BadChar => "TRAP-CHAR",
            Self::OutOfBounds => "TRAP-BOUNDS",
            Self::RefOp => "TRAP-REF",
            Self::Generic => "TRAP-GENERIC",
            Self::BadCall => "TRAP-CALL",
            Self::NoMatch => "TRAP-MATCH",
            Self::Uninit => "TRAP-UNINIT",
            Self::Fuel => "TRAP-FUEL",
            Self::Unreachable => "TRAP-UNREACHABLE",
        }
    }
}

/// A deterministic runtime failure with an optional source span.
#[derive(Debug, Clone)]
pub struct Trap {
    /// Trap code.
    pub code: TrapCode,
    /// Human-readable reason.
    pub message: String,
    /// Source range of the trapping operation, if known.
    pub span: Option<Span>,
}

impl Trap {
    fn at(code: TrapCode, message: String, span: Span) -> Self {
        Self {
            code,
            message,
            span: Some(span),
        }
    }

    fn unspanned(code: TrapCode, message: String) -> Self {
        Self {
            code,
            message,
            span: None,
        }
    }
}

impl std::fmt::Display for Trap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "error[{}]: {}", self.code.as_str(), self.message)
    }
}

/// A runtime value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// Integer in wrapped representation with its type.
    Int(i128, IntTy),
    /// Float with its type (stored as `f64`; `f32` values are rounded).
    Float(f64, FloatTy),
    /// `true` / `false`.
    Bool(bool),
    /// Unicode scalar value.
    Char(char),
    /// String contents.
    Str(String),
    /// Tuple elements.
    Tuple(Vec<Value>),
    /// Struct fields in declaration order.
    Struct(Vec<Value>),
    /// Enum value: parent name, variant index, payload.
    Enum {
        /// Parent enum.
        en: String,
        /// Variant index in the MIR layout.
        idx: usize,
        /// Payload values.
        payload: Vec<Value>,
    },
    /// Slice/array elements (only materialize as call arguments in 04).
    Slice(Vec<Value>),
    /// First-class function reference.
    FuncRef(String),
    /// `()`.
    Unit,
}

impl Value {
    /// Build a `u32` value.
    #[must_use]
    pub const fn u32(v: u32) -> Self {
        Self::Int(v as i128, IntTy::U32)
    }

    /// Build a `u64` value.
    #[must_use]
    pub const fn u64(v: u64) -> Self {
        Self::Int(v as i128, IntTy::U64)
    }

    /// Build an `i32` value.
    #[must_use]
    pub const fn i32(v: i32) -> Self {
        Self::Int(v as i128, IntTy::I32)
    }

    /// Build a `bool` value.
    #[must_use]
    pub const fn bool(v: bool) -> Self {
        Self::Bool(v)
    }

    /// Build a string value.
    #[must_use]
    pub fn str_(s: impl Into<String>) -> Self {
        Self::Str(s.into())
    }

    /// Build a slice value.
    #[must_use]
    pub const fn slice(v: Vec<Value>) -> Self {
        Self::Slice(v)
    }

    /// Render for `println` (one value, no trailing newline).
    #[must_use]
    pub fn display(&self) -> String {
        match self {
            Self::Int(v, _) => format!("{v}"),
            Self::Float(v, FloatTy::F32) => format!("{}", *v as f32),
            Self::Float(v, FloatTy::F64) => format!("{v}"),
            Self::Bool(b) => format!("{b}"),
            Self::Char(c) => format!("{c}"),
            Self::Str(s) => s.clone(),
            Self::Tuple(es) => {
                let inner: Vec<String> = es.iter().map(Self::display).collect();
                format!("({})", inner.join(", "))
            }
            Self::Struct(fs) => {
                let inner: Vec<String> = fs.iter().map(Self::display).collect();
                format!("{{ {} }}", inner.join(", "))
            }
            Self::Enum { en, idx, payload } => {
                if payload.is_empty() {
                    format!("{en}::{idx}")
                } else {
                    let inner: Vec<String> = payload.iter().map(Self::display).collect();
                    format!("{en}::{}({})", idx, inner.join(", "))
                }
            }
            Self::Slice(es) => {
                let inner: Vec<String> = es.iter().map(Self::display).collect();
                format!("[{}]", inner.join(", "))
            }
            Self::FuncRef(n) => format!("fn({n})"),
            Self::Unit => "()".to_string(),
        }
    }
}

/// A finished run: captured output plus the entry function's return value.
#[derive(Debug, Clone)]
pub struct Outcome {
    /// One entry per `println` call (arguments joined with spaces).
    pub printed: Vec<String>,
    /// Value returned by the entry function.
    pub returned: Value,
}

/// Default execution fuel: bounds total block transitions (and hence any
/// `loop {}` without `break`). Large enough for every checked-in example.
pub const DEFAULT_FUEL: u64 = 50_000_000;

/// Run `func` with `args`, returning its outcome or first trap.
pub fn run(program: &MirProgram, func: &str, args: Vec<Value>) -> Result<Outcome, Trap> {
    Machine::new(program, DEFAULT_FUEL).call(func, args)
}

/// Run the `main` function with no arguments.
pub fn run_main(program: &MirProgram) -> Result<Outcome, Trap> {
    run(program, "main", Vec::new())
}

struct Frame<'p> {
    func: &'p MirFunc,
    locals: Vec<Option<Value>>,
}

struct Machine<'p> {
    program: &'p MirProgram,
    fuel: u64,
    printed: Vec<String>,
}

impl<'p> Machine<'p> {
    fn new(program: &'p MirProgram, fuel: u64) -> Self {
        Self {
            program,
            fuel,
            printed: Vec::new(),
        }
    }

    fn call(mut self, func: &str, args: Vec<Value>) -> Result<Outcome, Trap> {
        let returned = self.exec_func(func, args, None)?;
        Ok(Outcome {
            printed: self.printed,
            returned,
        })
    }

    fn exec_func(
        &mut self,
        func: &str,
        args: Vec<Value>,
        call_span: Option<Span>,
    ) -> Result<Value, Trap> {
        let entry = self.program.funcs.get(func).ok_or_else(|| Trap {
            code: TrapCode::BadCall,
            message: format!("no function named `{func}`"),
            span: call_span,
        })?;
        if args.len() != entry.params.len() {
            return Err(Trap {
                code: TrapCode::BadCall,
                message: format!(
                    "`{func}` takes {} arguments, {} given",
                    entry.params.len(),
                    args.len()
                ),
                span: call_span,
            });
        }
        let mut locals: Vec<Option<Value>> = vec![None; entry.locals.len()];
        for (param, value) in entry.params.iter().zip(args) {
            locals[param.0 as usize] = Some(value);
        }
        let mut frame = Frame {
            func: entry,
            locals,
        };
        self.exec_frame(&mut frame)
    }

    fn exec_frame(&mut self, frame: &mut Frame<'p>) -> Result<Value, Trap> {
        let mut block = frame.func.entry;
        loop {
            if self.fuel == 0 {
                return Err(Trap::unspanned(
                    TrapCode::Fuel,
                    "execution fuel exhausted (possible infinite loop)".to_string(),
                ));
            }
            self.fuel -= 1;
            let bb = frame.func.blocks.get(block.0 as usize).ok_or_else(|| {
                Trap::unspanned(TrapCode::Unreachable, "jump to missing block".to_string())
            })?;
            // Clone the block's worklist: statements borrow nothing across
            // nested calls that need `&mut self` + frame access.
            let stmts = bb.stmts.clone();
            let term = bb.term.clone();
            for stmt in &stmts {
                self.exec_stmt(frame, stmt)?;
            }
            match term {
                Terminator::Goto(next) => block = next,
                Terminator::Branch {
                    cond,
                    then_bb,
                    else_bb,
                } => {
                    let c = Self::operand_of(frame, &cond)?;
                    match c {
                        Value::Bool(true) => block = then_bb,
                        Value::Bool(false) => block = else_bb,
                        other => {
                            return Err(Trap::unspanned(
                                TrapCode::Unreachable,
                                format!("branch on non-bool `{}`", other.display()),
                            ));
                        }
                    }
                }
                Terminator::Return(value) => {
                    return Ok(match value {
                        Some(o) => Self::operand_of(frame, &o)?,
                        None => Value::Unit,
                    });
                }
                Terminator::Trap { kind, span } => {
                    return Err(match kind {
                        lobster_mir::TrapKind::Unreachable(msg) => {
                            Trap::at(TrapCode::Unreachable, msg, span)
                        }
                        lobster_mir::TrapKind::RefOp(msg) => Trap::at(TrapCode::RefOp, msg, span),
                        lobster_mir::TrapKind::Generic(msg) => {
                            Trap::at(TrapCode::Generic, msg, span)
                        }
                        lobster_mir::TrapKind::BadCallee(msg) => {
                            Trap::at(TrapCode::BadCall, msg, span)
                        }
                        lobster_mir::TrapKind::Uninit(msg) => Trap::at(TrapCode::Uninit, msg, span),
                    });
                }
                Terminator::Unreachable => {
                    return Err(Trap::unspanned(
                        TrapCode::Unreachable,
                        "reached placeholder terminator".to_string(),
                    ));
                }
            }
        }
    }

    fn operand_of(frame: &Frame<'p>, operand: &Operand) -> Result<Value, Trap> {
        match operand {
            Operand::Local(l) => frame
                .locals
                .get(l.0 as usize)
                .cloned()
                .flatten()
                .ok_or_else(|| {
                    Trap::unspanned(
                        TrapCode::Uninit,
                        format!("read of unassigned local %{}", l.0),
                    )
                }),
            Operand::Const(c) => Ok(const_value(c)),
        }
    }

    fn exec_stmt(
        &mut self,
        frame: &mut Frame<'p>,
        stmt: &lobster_mir::MirStmt,
    ) -> Result<(), Trap> {
        use lobster_mir::MirStmt as S;
        match stmt {
            S::Assign { dst, rv, span } => {
                let v = self.eval_rvalue(frame, rv, *span)?;
                frame.locals[dst.0 as usize] = Some(v);
                Ok(())
            }
            S::Call {
                dst,
                target,
                args,
                span,
            } => {
                let mut values = Vec::with_capacity(args.len());
                for a in args {
                    values.push(Self::operand_of(frame, a)?);
                }
                let name = match target {
                    CallTarget::Fn(n) => n.clone(),
                    CallTarget::Value(o) => match Self::operand_of(frame, o)? {
                        Value::FuncRef(n) => n,
                        other => {
                            return Err(Trap::at(
                                TrapCode::BadCall,
                                format!("cannot call `{}`", other.display()),
                                *span,
                            ));
                        }
                    },
                };
                if name == "println" {
                    // Lowered `println` never reaches here (`Print` does),
                    // but stay total for hand-built MIR.
                    let line: Vec<String> = values.iter().map(Value::display).collect();
                    self.printed.push(line.join(" "));
                    if let Some(d) = dst {
                        frame.locals[d.0 as usize] = Some(Value::Unit);
                    }
                    return Ok(());
                }
                let returned = self.exec_func(&name, values, Some(*span))?;
                if let Some(d) = dst {
                    frame.locals[d.0 as usize] = Some(returned);
                }
                Ok(())
            }
            S::Print { values, .. } => {
                let mut parts = Vec::with_capacity(values.len());
                for v in values {
                    parts.push(Self::operand_of(frame, v)?.display());
                }
                self.printed.push(parts.join(" "));
                Ok(())
            }
            S::SetField {
                base,
                field,
                value,
                span,
            } => {
                let v = Self::operand_of(frame, value)?;
                let slot = frame.locals.get_mut(base.0 as usize).ok_or_else(|| {
                    Trap::unspanned(
                        TrapCode::Uninit,
                        format!("read of unassigned local %{}", base.0),
                    )
                })?;
                let current = slot.clone().ok_or_else(|| {
                    Trap::unspanned(
                        TrapCode::Uninit,
                        format!("read of unassigned local %{}", base.0),
                    )
                })?;
                let is_struct = matches!(&current, Value::Struct(_));
                match current {
                    Value::Struct(mut fs) | Value::Tuple(mut fs) => {
                        if *field < fs.len() {
                            fs[*field] = v;
                            // Preserve the aggregate kind.
                            *slot = Some(if is_struct {
                                Value::Struct(fs)
                            } else {
                                Value::Tuple(fs)
                            });
                            Ok(())
                        } else {
                            Err(Trap::at(
                                TrapCode::Unreachable,
                                format!("field {field} out of range"),
                                *span,
                            ))
                        }
                    }
                    other => Err(Trap::at(
                        TrapCode::BadCall,
                        format!("cannot set a field on `{}`", other.display()),
                        *span,
                    )),
                }
            }
        }
    }

    fn eval_rvalue(&mut self, frame: &Frame<'p>, rv: &Rvalue, span: Span) -> Result<Value, Trap> {
        match rv {
            Rvalue::Use(o) => Self::operand_of(frame, o),
            Rvalue::Binary { op, l, r } => {
                let a = Self::operand_of(frame, l)?;
                let b = Self::operand_of(frame, r)?;
                eval_binary(*op, a, b, span)
            }
            Rvalue::Unary { op, v } => {
                let a = Self::operand_of(frame, v)?;
                eval_unary(*op, a, span)
            }
            Rvalue::Cast { to, v } => {
                let a = Self::operand_of(frame, v)?;
                eval_cast(&a, to, span)
            }
            Rvalue::Tuple(es) => {
                let mut out = Vec::with_capacity(es.len());
                for e in es {
                    out.push(Self::operand_of(frame, e)?);
                }
                Ok(Value::Tuple(out))
            }
            Rvalue::Field { base, field } => {
                let a = Self::operand_of(frame, base)?;
                match a {
                    Value::Tuple(es) | Value::Struct(es) => {
                        es.get(*field).cloned().ok_or_else(|| {
                            Trap::at(
                                TrapCode::Unreachable,
                                format!("field {field} out of range"),
                                span,
                            )
                        })
                    }
                    other => Err(Trap::at(
                        TrapCode::BadCall,
                        format!("cannot read a field of `{}`", other.display()),
                        span,
                    )),
                }
            }
            Rvalue::Enum {
                en,
                variant,
                payload,
            } => {
                let mut out = Vec::with_capacity(payload.len());
                for p in payload {
                    out.push(Self::operand_of(frame, p)?);
                }
                Ok(Value::Enum {
                    en: en.clone(),
                    idx: *variant,
                    payload: out,
                })
            }
            Rvalue::Discriminant(base) => match Self::operand_of(frame, base)? {
                Value::Enum { idx, .. } => Ok(Value::Int(idx as i128, IntTy::U32)),
                other => Err(Trap::at(
                    TrapCode::BadCall,
                    format!("cannot take the discriminant of `{}`", other.display()),
                    span,
                )),
            },
            Rvalue::VariantPayload { base, idx } => match Self::operand_of(frame, base)? {
                Value::Enum { payload, .. } => payload.get(*idx).cloned().ok_or_else(|| {
                    Trap::at(
                        TrapCode::Unreachable,
                        format!("payload {idx} out of range"),
                        span,
                    )
                }),
                other => Err(Trap::at(
                    TrapCode::BadCall,
                    format!("cannot take payload {idx} of `{}`", other.display()),
                    span,
                )),
            },
            Rvalue::SliceLen(base) => match Self::operand_of(frame, base)? {
                Value::Slice(es) => Ok(Value::Int(es.len() as i128, IntTy::Usize)),
                other => Err(Trap::at(
                    TrapCode::BadCall,
                    format!("cannot take the length of `{}`", other.display()),
                    span,
                )),
            },
            Rvalue::SliceIndex { base, idx } => {
                let items = match Self::operand_of(frame, base)? {
                    Value::Slice(es) => es,
                    other => {
                        return Err(Trap::at(
                            TrapCode::BadCall,
                            format!("cannot index into `{}`", other.display()),
                            span,
                        ));
                    }
                };
                let i = match Self::operand_of(frame, idx)? {
                    Value::Int(v, t) if !t.is_signed() => v as usize,
                    Value::Int(v, _) => {
                        if v < 0 {
                            return Err(Trap::at(
                                TrapCode::OutOfBounds,
                                format!("negative index {v}"),
                                span,
                            ));
                        }
                        v as usize
                    }
                    other => {
                        return Err(Trap::at(
                            TrapCode::BadCall,
                            format!("cannot index with `{}`", other.display()),
                            span,
                        ));
                    }
                };
                items.get(i).cloned().ok_or_else(|| {
                    Trap::at(
                        TrapCode::OutOfBounds,
                        format!("index {i} out of bounds"),
                        span,
                    )
                })
            }
        }
    }
}

fn const_value(c: &Const_) -> Value {
    match c {
        Const_::Int(v, t) => Value::Int(*v, *t),
        Const_::Float(v, t) => Value::Float(*v, *t),
        Const_::Bool(b) => Value::Bool(*b),
        Const_::Char(c) => Value::Char(*c),
        Const_::Str(s) => Value::Str(s.clone()),
        Const_::FuncRef(n) => Value::FuncRef(n.clone()),
        Const_::Unit => Value::Unit,
    }
}

// ----- integers -----

/// Unsigned representation of a normalized value.
fn to_unsigned(v: i128, ty: IntTy) -> u128 {
    if ty.is_signed() {
        if v < 0 {
            (1u128 << ty.bits()).wrapping_add(v as u128)
        } else {
            v as u128
        }
    } else {
        v as u128
    }
}

/// Normalize an unsigned representation into the type's value range.
fn from_unsigned(u: u128, ty: IntTy) -> i128 {
    let bits = ty.bits();
    let mask = if bits >= 128 {
        u128::MAX
    } else {
        (1u128 << bits) - 1
    };
    let v = u & mask;
    if ty.is_signed() {
        let sign = 1u128 << (bits - 1);
        if v & sign != 0 {
            (v as i128).wrapping_sub(1i128 << bits)
        } else {
            v as i128
        }
    } else {
        v as i128
    }
}

/// Wrapping binary op on two values of the same integer type.
fn wrap_bin_ty(l: i128, r: i128, ty: IntTy, f: impl Fn(u128, u128) -> u128) -> Value {
    Value::Int(
        from_unsigned(f(to_unsigned(l, ty), to_unsigned(r, ty)), ty),
        ty,
    )
}

fn eval_int_bin(
    op: lobster_ast::BinOp,
    l: i128,
    lt: IntTy,
    r: i128,
    rt: IntTy,
    span: Span,
) -> Result<Value, Trap> {
    use lobster_ast::BinOp as B;
    // Shifts take any int amount; everything else needs matching types
    // (the checker guarantees it — trap honestly otherwise).
    match op {
        B::Shl | B::Shr => {
            let bits = lt.bits();
            let amt = (to_unsigned(r, rt) & u128::from(bits - 1)) as u32;
            let out = match op {
                B::Shl => from_unsigned(to_unsigned(l, lt) << amt, lt),
                _ => {
                    if lt.is_signed() {
                        // Arithmetic shift: sign-extends (Rust `>>` on i128).
                        l >> amt
                    } else {
                        from_unsigned(to_unsigned(l, lt) >> amt, lt)
                    }
                }
            };
            Ok(Value::Int(out, lt))
        }
        B::Add => Ok(wrap_bin_ty(l, r, lt, |a, b| a.wrapping_add(b))),
        B::Sub => Ok(wrap_bin_ty(l, r, lt, |a, b| a.wrapping_sub(b))),
        B::Mul => Ok(wrap_bin_ty(l, r, lt, |a, b| a.wrapping_mul(b))),
        B::Div => {
            if r == 0 {
                return Err(Trap::at(
                    TrapCode::DivZero,
                    "division by zero".to_string(),
                    span,
                ));
            }
            // `MIN / -1` wraps to `MIN` (spec §4): the only overflow case.
            let q = l.checked_div(r).unwrap_or(lt.min());
            // `checked_div` cannot overflow otherwise; re-wrap defensively.
            Ok(Value::Int(from_unsigned(to_unsigned(q, lt), lt), lt))
        }
        B::Rem => {
            if r == 0 {
                return Err(Trap::at(
                    TrapCode::DivZero,
                    "remainder by zero".to_string(),
                    span,
                ));
            }
            // `MIN % -1` is `0` (spec §4).
            let m = l.checked_rem(r).unwrap_or(0);
            Ok(Value::Int(from_unsigned(to_unsigned(m, lt), lt), lt))
        }
        B::BitAnd => Ok(wrap_bin_ty(l, r, lt, |a, b| a & b)),
        B::BitOr => Ok(wrap_bin_ty(l, r, lt, |a, b| a | b)),
        B::BitXor => Ok(wrap_bin_ty(l, r, lt, |a, b| a ^ b)),
        B::Eq => Ok(Value::Bool(int_eq(l, lt, r, rt))),
        B::Ne => Ok(Value::Bool(!int_eq(l, lt, r, rt))),
        B::Lt | B::Le | B::Gt | B::Ge => Ok(Value::Bool(int_ord(op, l, lt, r, rt))),
        B::And | B::Or => Err(Trap::at(
            TrapCode::Unreachable,
            "logical operator on integers".to_string(),
            span,
        )),
    }
}

fn int_eq(l: i128, lt: IntTy, r: i128, rt: IntTy) -> bool {
    if lt.is_signed() == rt.is_signed() {
        l == r
    } else {
        to_unsigned(l, lt) == to_unsigned(r, rt)
    }
}

fn int_ord(op: lobster_ast::BinOp, l: i128, lt: IntTy, r: i128, rt: IntTy) -> bool {
    use lobster_ast::BinOp as B;
    let ord = if lt.is_signed() && rt.is_signed() {
        l.cmp(&r)
    } else if !lt.is_signed() && !rt.is_signed() {
        to_unsigned(l, lt).cmp(&to_unsigned(r, rt))
    } else {
        // Mixed signedness cannot occur in checked code (no implicit
        // conversions); compare as signed defensively.
        l.cmp(&r)
    };
    match op {
        B::Lt => ord == std::cmp::Ordering::Less,
        B::Le => ord != std::cmp::Ordering::Greater,
        B::Gt => ord == std::cmp::Ordering::Greater,
        _ => ord != std::cmp::Ordering::Less,
    }
}

// ----- floats (strict IEEE; never reassociated) -----

fn eval_float_bin(
    op: lobster_ast::BinOp,
    l: f64,
    lt: FloatTy,
    r: f64,
    _rt: FloatTy,
    span: Span,
) -> Result<Value, Trap> {
    use lobster_ast::BinOp as B;
    let narrow = |v: f64| {
        if lt == FloatTy::F32 {
            f32_value(v)
        } else {
            v
        }
    };
    match op {
        B::Add => Ok(Value::Float(narrow(l + r), lt)),
        B::Sub => Ok(Value::Float(narrow(l - r), lt)),
        B::Mul => Ok(Value::Float(narrow(l * r), lt)),
        B::Div => Ok(Value::Float(narrow(l / r), lt)),
        B::Rem => Ok(Value::Float(narrow(l % r), lt)),
        B::Eq => Ok(Value::Bool(l == r)),
        B::Ne => Ok(Value::Bool(l != r)),
        B::Lt => Ok(Value::Bool(l < r)),
        B::Le => Ok(Value::Bool(l <= r)),
        B::Gt => Ok(Value::Bool(l > r)),
        B::Ge => Ok(Value::Bool(l >= r)),
        _ => Err(Trap::at(
            TrapCode::Unreachable,
            "bad float operator".to_string(),
            span,
        )),
    }
}

/// Round a computation result to `f32` precision (stored as `f64`).
fn f32_value(v: f64) -> f64 {
    f64::from(v as f32)
}

// ----- operators -----

fn eval_binary(op: lobster_ast::BinOp, a: Value, b: Value, span: Span) -> Result<Value, Trap> {
    use lobster_ast::BinOp as B;
    match (a, b) {
        (Value::Int(l, lt), Value::Int(r, rt)) => eval_int_bin(op, l, lt, r, rt, span),
        (Value::Float(l, lt), Value::Float(r, rt)) => eval_float_bin(op, l, lt, r, rt, span),
        (Value::Bool(l), Value::Bool(r)) => match op {
            B::And => Ok(Value::Bool(l && r)),
            B::Or => Ok(Value::Bool(l || r)),
            B::Eq => Ok(Value::Bool(l == r)),
            B::Ne => Ok(Value::Bool(l != r)),
            B::Lt => Ok(Value::Bool(!l & r)),
            B::Le => Ok(Value::Bool(!l | r)),
            B::Gt => Ok(Value::Bool(l & !r)),
            B::Ge => Ok(Value::Bool(l | !r)),
            _ => Err(Trap::at(
                TrapCode::Unreachable,
                "bad bool operator".to_string(),
                span,
            )),
        },
        (Value::Char(l), Value::Char(r)) => match op {
            B::Eq => Ok(Value::Bool(l == r)),
            B::Ne => Ok(Value::Bool(l != r)),
            B::Lt => Ok(Value::Bool(l < r)),
            B::Le => Ok(Value::Bool(l <= r)),
            B::Gt => Ok(Value::Bool(l > r)),
            B::Ge => Ok(Value::Bool(l >= r)),
            _ => Err(Trap::at(
                TrapCode::Unreachable,
                "bad char operator".to_string(),
                span,
            )),
        },
        (a, b) => Err(Trap::at(
            TrapCode::Unreachable,
            format!(
                "cannot apply `{op:?}` to `{}` and `{}`",
                a.display(),
                b.display()
            ),
            span,
        )),
    }
}

fn eval_unary(op: lobster_ast::UnOp, a: Value, span: Span) -> Result<Value, Trap> {
    use lobster_ast::UnOp as U;
    match (op, a) {
        (U::Neg, Value::Int(v, t)) => Ok(Value::Int(
            from_unsigned(to_unsigned(v, t).wrapping_neg(), t),
            t,
        )),
        (U::Neg, Value::Float(v, t)) => Ok(Value::Float(
            if t == FloatTy::F32 { f32_value(-v) } else { -v },
            t,
        )),
        (U::Not, Value::Bool(b)) => Ok(Value::Bool(!b)),
        (U::Not, Value::Int(v, t)) => Ok(Value::Int(from_unsigned(!to_unsigned(v, t), t), t)),
        (_, other) => Err(Trap::at(
            TrapCode::Unreachable,
            format!("cannot apply `{op:?}` to `{}`", other.display()),
            span,
        )),
    }
}

// ----- casts (spec §4 table) -----

fn eval_cast(v: &Value, to: &Ty, span: Span) -> Result<Value, Trap> {
    match (v, to) {
        (_, Ty::Error) => Err(Trap::at(
            TrapCode::Unreachable,
            "cast to error type".to_string(),
            span,
        )),
        (Value::Int(i, _), Ty::Int(t)) => Ok(Value::Int(
            from_unsigned(to_unsigned(*i, int_ty_of(v)), *t),
            *t,
        )),
        (Value::Int(i, _), Ty::Float(t)) => {
            let f = *i as f64;
            Ok(Value::Float(
                if *t == FloatTy::F32 { f32_value(f) } else { f },
                *t,
            ))
        }
        (Value::Float(f, _), Ty::Float(t)) => Ok(Value::Float(
            if *t == FloatTy::F32 {
                f32_value(*f)
            } else {
                *f
            },
            *t,
        )),
        (Value::Float(f, _), Ty::Int(t)) => Ok(Value::Int(saturating_f2i(*f, *t), *t)),
        (Value::Bool(b), Ty::Int(t)) => Ok(Value::Int(i128::from(*b as u8), *t)),
        (Value::Char(c), Ty::Int(t)) => Ok(Value::Int(*c as i128, *t)),
        (Value::Int(i, _), Ty::Char) => {
            let u = to_unsigned(*i, int_ty_of(v));
            char::from_u32(u as u32).map(Value::Char).ok_or_else(|| {
                Trap::at(
                    TrapCode::BadChar,
                    format!("{u} is not a Unicode scalar value"),
                    span,
                )
            })
        }
        (a, b) if value_ty_eq(a, b) => Ok(a.clone()),
        (a, b) => Err(Trap::at(
            TrapCode::Unreachable,
            format!("cannot cast `{}` as `{b}`", a.display()),
            span,
        )),
    }
}

fn int_ty_of(v: &Value) -> IntTy {
    match v {
        Value::Int(_, t) => *t,
        _ => IntTy::I32,
    }
}

/// Float → int with saturation (spec §4): NaN → 0, out-of-range → MIN/MAX.
fn saturating_f2i(f: f64, ty: IntTy) -> i128 {
    if f.is_nan() {
        return 0;
    }
    let bits = ty.bits();
    if ty.is_signed() {
        let lo = -(1i128 << (bits - 1));
        let hi = (1i128 << (bits - 1)) - 1;
        if f <= lo as f64 {
            return lo;
        }
        if f >= hi as f64 {
            return hi;
        }
        // `hi as f64` rounds up to 2^(N-1) for N = 64, so values in
        // (2^63 - 1024, 2^63) would wrongly saturate; truncate first when
        // strictly inside the exact-integer range.
        f.trunc() as i128
    } else {
        if f <= 0.0 {
            return 0;
        }
        let two_pow = 2f64.powi(bits as i32);
        if f >= two_pow {
            return (1i128 << bits) - 1;
        }
        f.trunc() as i128
    }
}

/// True when a value already has the target type (identity casts).
fn value_ty_eq(v: &Value, ty: &Ty) -> bool {
    matches!(
        (v, ty),
        (Value::Bool(_), Ty::Bool)
            | (Value::Char(_), Ty::Char)
            | (Value::Unit, Ty::Unit)
            | (Value::Str(_), Ty::Str)
    )
}

#[allow(dead_code)]
fn _binop_name(_op: BinOp) -> &'static str {
    "op"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pipeline(text: &str) -> MirProgram {
        let mut sm = lobster_source::SourceManager::new();
        let id = sm.add_file("t.lobster", text);
        let lexed = lobster_lexer::lex(&sm, id, text);
        assert!(lexed.diagnostics.is_empty());
        let parsed = lobster_parser::parse(id, &lexed.tokens);
        assert!(parsed.diagnostics.is_empty());
        let (resolved, rdiags) = lobster_resolve::resolve(&parsed.file);
        assert!(rdiags.is_empty(), "{rdiags:?}");
        let (program, tables, cdiags) = lobster_sema::check_program(&resolved, &parsed.file);
        assert!(cdiags.is_empty(), "{cdiags:?}");
        let (hir, _) = lobster_hir::lower(&parsed.file, &resolved, &program, &tables);
        lobster_mir::lower(&hir)
    }

    fn run_text(text: &str) -> Result<Outcome, Trap> {
        let prog = pipeline(text);
        run_main(&prog)
    }

    fn call(text: &str, func: &str, args: Vec<Value>) -> Result<Outcome, Trap> {
        let prog = pipeline(text);
        run(&prog, func, args)
    }

    #[test]
    fn fib_10_is_55() {
        let out = run_text(
            "fn fib(n: u64) -> u64 {\n    if n < 2 {\n        return n;\n    }\n\n    fib(n - 1) + fib(n - 2)\n}\nfn main() {\n    let result = fib(10);\n    println(result);\n}\n",
        )
        .unwrap();
        assert_eq!(out.printed, vec!["55"]);
    }

    #[test]
    fn all_examples_run() {
        for file in [
            "examples/hello.lobster",
            "examples/arithmetic.lobster",
            "examples/control_flow.lobster",
            "examples/structs_enums.lobster",
            "examples/strings.lobster",
            "examples/tomato_shapes.lobster",
        ] {
            let path = format!("../../{file}");
            let text = std::fs::read_to_string(&path).unwrap();
            let prog = pipeline(&text);
            run_main(&prog).unwrap_or_else(|e| panic!("{file}: {e}"));
        }
    }

    #[test]
    fn wrapping_and_traps() {
        // Wrapping add on u8.
        let out = call(
            "fn f(a: u8) -> u8 { a + 1u8 }\nfn main() { println(1u8); }\n",
            "f",
            vec![Value::Int(255, IntTy::U8)],
        )
        .unwrap();
        assert_eq!(out.returned, Value::Int(0, IntTy::U8));
        // Division by zero traps.
        let err = call(
            "fn f(a: u32) -> u32 { a / 0u32 }\nfn main() {}\n",
            "f",
            vec![Value::u32(1)],
        )
        .unwrap_err();
        assert_eq!(err.code, TrapCode::DivZero);
        // Bad char cast traps.
        let err = call(
            "fn f(a: u32) -> char { a as char }\nfn main() {}\n",
            "f",
            vec![Value::u32(0xD800)],
        )
        .unwrap_err();
        assert_eq!(err.code, TrapCode::BadChar);
    }

    #[test]
    fn shift_amounts_masked() {
        let out = call(
            "fn f(a: u32) -> u32 { a << 33u32 }\nfn main() {}\n",
            "f",
            vec![Value::u32(1)],
        )
        .unwrap();
        // 33 & 31 == 1, so 1 << 1 == 2.
        assert_eq!(out.returned, Value::u32(2));
    }

    #[test]
    fn for_loop_sums_slice() {
        let out = call(
            "fn consume_all(items: &[u32]) -> u32 {\n    let mut total = 0u32;\n    for item in items {\n        total += item;\n    }\n    total\n}\nfn main() {}\n",
            "consume_all",
            vec![Value::slice(vec![Value::u32(1), Value::u32(2), Value::u32(3)])],
        )
        .unwrap();
        assert_eq!(out.returned, Value::u32(6));
    }

    #[test]
    fn match_on_enum_dispatches() {
        let out = call(
            "enum Dir { North, East, South, West, }\nfn turn(d: Dir) -> Dir {\n    match d {\n        North => East,\n        East => South,\n        South => West,\n        West => North,\n    }\n}\nfn main() {}\n",
            "turn",
            vec![Value::Enum { en: "Dir".to_string(), idx: 0, payload: vec![] }],
        )
        .unwrap();
        assert_eq!(
            out.returned,
            Value::Enum {
                en: "Dir".to_string(),
                idx: 1,
                payload: vec![]
            }
        );
    }

    #[test]
    fn float_to_int_saturates() {
        assert_eq!(saturating_f2i(f64::NAN, IntTy::I32), 0);
        assert_eq!(
            saturating_f2i(f64::INFINITY, IntTy::I32),
            i128::from(i32::MAX)
        );
        assert_eq!(saturating_f2i(f64::NEG_INFINITY, IntTy::U8), 0);
        assert_eq!(saturating_f2i(1.9, IntTy::I32), 1);
    }
}
