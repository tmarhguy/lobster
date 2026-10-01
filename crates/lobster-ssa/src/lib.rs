//! Lobster SSA: single-assignment CFG with join phis over MIR.
//!
//! SSA is the Commit 05 stage. It converts [`MirProgram`] to
//! [`SsaProgram`]: every definition creates a fresh [`SsaValue`], and every
//! control-flow join carries [`Phi`] nodes merging the per-predecessor
//! versions. Later optimizer passes (Commit 06+) consume SSA; the reference
//! interpreter keeps running MIR, so construction must be
//! semantics-preserving by construction.
//!
//! Design notes (intentional, for the verifier and optimizer to rely on):
//! - **Simple join phis, not minimal phis.** Every multi-predecessor block
//!   gets one phi per MIR local, with [`SsaOperand::Undef`] arms where a
//!   predecessor never defined that local. Dead/trivial phis are left in
//!   place; pruning them is a Commit 06 optimizer job.
//! - **No new diagnostics.** The checker already accepted the program, so
//!   verifier failures are internal compiler bugs reported as
//!   [`VerifyError`], never user-facing `E2xx` codes.
//! - **Trapping operations keep their spans and order.** Construction only
//!   renames operands; it never hoists, sinks, merges, or reorders
//!   statements, terminators, or phi arms. Definite assignment stays
//!   dynamic: a use with no reaching definition becomes `Undef` (a future
//!   runtime `Uninit` trap), exactly like MIR.

use lobster_ast::{BinOp, UnOp};
use lobster_mir::{BlockId, Const_, Local, MirFunc, MirProgram};
use lobster_source::{FileId, Span};
use lobster_types::{IntTy, Ty};
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

/// A single-assignment value: defined exactly once (param, phi, assign,
/// call, or field-update result).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SsaValue(pub u32);

/// A lowered program: functions plus ADT layouts (mirroring MIR).
#[derive(Debug, Clone)]
pub struct SsaProgram {
    /// Functions by name.
    pub funcs: HashMap<String, SsaFunc>,
    /// Struct layouts: field names in declaration order.
    pub structs: HashMap<String, Vec<String>>,
    /// Enum layouts: variant names in declaration order.
    pub enums: HashMap<String, Vec<String>>,
}

/// A function in SSA form.
#[derive(Debug, Clone)]
pub struct SsaFunc {
    /// Function name.
    pub name: String,
    /// Parameter values in order.
    pub params: Vec<SsaValue>,
    /// Declared return type.
    pub ret: Ty,
    /// Type of every value by index.
    pub values: Vec<Ty>,
    /// Debug names of values (parallel to `values`).
    pub value_names: Vec<String>,
    /// Blocks by index; execution starts at `entry`.
    pub blocks: Vec<SsaBlock>,
    /// First block.
    pub entry: BlockId,
}

/// One basic block: phis, then straight-line statements, then a terminator.
#[derive(Debug, Clone)]
pub struct SsaBlock {
    /// Join phis (empty unless the block has multiple predecessors).
    pub phis: Vec<Phi>,
    /// Statements in order.
    pub stmts: Vec<SsaStmt>,
    /// How control leaves the block.
    pub term: SsaTerm,
}

/// A join phi: `dst` takes the arm value from whichever predecessor ran.
#[derive(Debug, Clone)]
pub struct Phi {
    /// Defined value.
    pub dst: SsaValue,
    /// Type of the value (type of the origin MIR local).
    pub ty: Ty,
    /// Origin MIR local (for dumps and debugging).
    pub origin: Local,
    /// One arm per predecessor, in predecessor-block order.
    pub arms: Vec<(BlockId, SsaOperand)>,
    /// Location for destruct-emitted copies (join block's first statement,
    /// else first predecessor statement, else `None`). Copies never trap,
    /// so this span never renders; [`destruct`] falls back to a synthetic
    /// span only when no statement exists anywhere nearby.
    pub span: Option<Span>,
}

/// An operand: an SSA value, a constant, or `Undef`.
///
/// `Undef` means "no reaching definition on this path" (definite
/// assignment is deferred past the checker, so this is a future runtime
/// `Uninit` trap, exactly like MIR — never a construction panic).
#[derive(Debug, Clone)]
pub enum SsaOperand {
    /// SSA value.
    Value(SsaValue),
    /// Inline constant.
    Const(Const_),
    /// No reaching definition on this path.
    Undef,
}

/// A computed right-hand side (mirrors MIR over [`SsaOperand`]).
#[derive(Debug, Clone)]
pub enum SsaRvalue {
    /// Copy an operand.
    Use(SsaOperand),
    /// `l op r`.
    Binary {
        /// Operator.
        op: BinOp,
        /// Left operand.
        l: SsaOperand,
        /// Right operand.
        r: SsaOperand,
    },
    /// `op v`.
    Unary {
        /// Operator.
        op: UnOp,
        /// Operand.
        v: SsaOperand,
    },
    /// `v as to`.
    Cast {
        /// Target type.
        to: Ty,
        /// Converted operand.
        v: SsaOperand,
    },
    /// Build a tuple.
    Tuple(Vec<SsaOperand>),
    /// Read a struct field or tuple position.
    Field {
        /// Aggregate operand.
        base: SsaOperand,
        /// Field index in the layout.
        field: usize,
    },
    /// Build an enum value.
    Enum {
        /// Parent enum.
        en: String,
        /// Variant index in the layout.
        variant: usize,
        /// Payload values.
        payload: Vec<SsaOperand>,
    },
    /// Discriminant of an enum value (variant index as `u32`).
    Discriminant(SsaOperand),
    /// One payload element of an enum value.
    VariantPayload {
        /// Enum operand.
        base: SsaOperand,
        /// Payload index.
        idx: usize,
    },
    /// Length of a slice/array value.
    SliceLen(SsaOperand),
    /// One element of a slice/array value.
    SliceIndex {
        /// Aggregate operand.
        base: SsaOperand,
        /// Element operand (`usize`-ish).
        idx: SsaOperand,
    },
}

/// Statement inside a block.
#[derive(Debug, Clone)]
pub enum SsaStmt {
    /// `dst = rv`.
    Assign {
        /// Defined value.
        dst: SsaValue,
        /// Computed value.
        rv: SsaRvalue,
        /// Source range.
        span: Span,
    },
    /// `dst = target(args)`. `dst` is `None` for `()` results.
    Call {
        /// Defined value, if the result is used.
        dst: Option<SsaValue>,
        /// What is called.
        target: SsaCallTarget,
        /// Arguments.
        args: Vec<SsaOperand>,
        /// Source range.
        span: Span,
    },
    /// `println(v0, v1, ...)`.
    Print {
        /// Printed values.
        values: Vec<SsaOperand>,
        /// Source range.
        span: Span,
    },
    /// Field update: defines a fresh version of the base aggregate.
    SetField {
        /// New version of the base aggregate.
        dst: SsaValue,
        /// Previous version of the base aggregate.
        base: SsaOperand,
        /// Field index in the layout.
        field: usize,
        /// New field value.
        value: SsaOperand,
        /// Source range.
        span: Span,
    },
}

/// Call target.
#[derive(Debug, Clone)]
pub enum SsaCallTarget {
    /// Named function.
    Fn(String),
    /// First-class function value (a `FuncRef` operand).
    Value(SsaOperand),
}

/// How control leaves a block (mirrors MIR; `Unreachable` is rejected by
/// [`verify`]).
#[derive(Debug, Clone)]
pub enum SsaTerm {
    /// Unconditional jump.
    Goto(BlockId),
    /// `if cond { then } else { els }`.
    Branch {
        /// Condition operand (`bool`).
        cond: SsaOperand,
        /// True successor.
        then_bb: BlockId,
        /// False successor.
        else_bb: BlockId,
    },
    /// `return [value]`.
    Return(Option<SsaOperand>),
    /// Deterministic trap with a source span.
    Trap {
        /// Trap reason (re-exported from MIR).
        kind: lobster_mir::TrapKind,
        /// Source range.
        span: Span,
    },
    /// Placeholder: never emitted by [`build`], rejected by [`verify`].
    Unreachable,
}

/// A verifier failure: an internal compiler bug, not a user error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyError {
    /// Function under verification.
    pub func: String,
    /// Block under verification, if any.
    pub block: Option<u32>,
    /// What is wrong.
    pub message: String,
}

impl std::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.block {
            Some(b) => write!(f, "{} bb{b}: {}", self.func, self.message),
            None => write!(f, "{}: {}", self.func, self.message),
        }
    }
}

/// Convert MIR to SSA. Total: poisoned MIR (`Unreachable` terminators,
/// out-of-range ids) becomes well-formed SSA that [`verify`] then rejects
/// with errors instead of panicking.
#[must_use]
pub fn build(mir: &MirProgram) -> SsaProgram {
    let mut funcs = HashMap::new();
    for (name, f) in &mir.funcs {
        funcs.insert(name.clone(), build_func(f));
    }
    SsaProgram {
        funcs,
        structs: mir.structs.clone(),
        enums: mir.enums.clone(),
    }
}

/// Synthetic span for generated code (phi copies, unreachable backstops).
/// Copies only move already-computed values, so they never trap and this
/// span never renders; it exists because MIR statements require one.
fn synthetic_span() -> Span {
    Span::new(FileId(0), 0, 0)
}

/// Convert SSA back to MIR by placing one copy per phi arm on its edge.
///
/// Critical edges (predecessor with several successors) are split with a
/// fresh block so the copy runs only when that edge runs. `Undef` operands
/// read a dedicated never-assigned local, preserving the dynamic `Uninit`
/// trap exactly like MIR. Total: anything [`verify`] would reject still
/// lowers to *something* runnable (usually trapping) instead of panicking.
#[must_use]
pub fn destruct(ssa: &SsaProgram) -> MirProgram {
    let mut funcs = HashMap::new();
    for (name, f) in &ssa.funcs {
        funcs.insert(name.clone(), destruct_func(f));
    }
    MirProgram {
        funcs,
        structs: ssa.structs.clone(),
        enums: ssa.enums.clone(),
    }
}

/// Check an SSA program. Returns `Ok(())` when every function is
/// well-formed; otherwise every violation found.
pub fn verify(program: &SsaProgram) -> Result<(), Vec<VerifyError>> {
    let mut errors = Vec::new();
    let mut names: Vec<&String> = program.funcs.keys().collect();
    names.sort();
    for name in names {
        verify_func(&program.funcs[name], program, &mut errors);
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Render an SSA program in human-readable form (snapshots, `--dump-ssa`).
#[must_use]
pub fn dump(program: &SsaProgram) -> String {
    let mut out = String::new();
    let mut names: Vec<&String> = program.funcs.keys().collect();
    names.sort();
    for name in names {
        let f = &program.funcs[name];
        let _ = writeln!(
            out,
            "fn {}({} values, {} blocks):",
            name,
            f.values.len(),
            f.blocks.len()
        );
        for (i, b) in f.blocks.iter().enumerate() {
            let _ = writeln!(out, "  bb{i}:");
            for p in &b.phis {
                let _ = writeln!(out, "    {}", dump_phi(p, f));
            }
            for s in &b.stmts {
                let _ = writeln!(out, "    {}", dump_stmt(s, f));
            }
            let _ = writeln!(out, "    {}", dump_term(&b.term));
        }
    }
    out
}

fn dump_operand(o: &SsaOperand, f: &SsaFunc) -> String {
    match o {
        SsaOperand::Value(v) => {
            let n = f.value_names.get(v.0 as usize).map_or("?", String::as_str);
            format!("%{}( {n} )", v.0)
        }
        SsaOperand::Const(c) => match c {
            Const_::Int(v, t) => format!("{v}{t}"),
            Const_::Float(v, t) => format!("{v}{t}"),
            Const_::Bool(b) => format!("{b}"),
            Const_::Char(c) => format!("'{c}'"),
            Const_::Str(s) => format!("\"{s}\""),
            Const_::FuncRef(n) => format!("fn({n})"),
            Const_::Unit => "()".to_string(),
        },
        SsaOperand::Undef => "undef".to_string(),
    }
}

fn dump_phi(p: &Phi, f: &SsaFunc) -> String {
    let arms: Vec<String> = p
        .arms
        .iter()
        .map(|(bb, o)| format!("bb{}: {}", bb.0, dump_operand(o, f)))
        .collect();
    format!(
        "%{} = phi [{}] (orig %{})",
        p.dst.0,
        arms.join(", "),
        p.origin.0
    )
}

fn dump_stmt(s: &SsaStmt, f: &SsaFunc) -> String {
    match s {
        SsaStmt::Assign { dst, rv, .. } => format!("%{} = {}", dst.0, dump_rv(rv, f)),
        SsaStmt::Call {
            dst, target, args, ..
        } => {
            let t = match target {
                SsaCallTarget::Fn(n) => format!("fn({n})"),
                SsaCallTarget::Value(o) => dump_operand(o, f),
            };
            let a: Vec<String> = args.iter().map(|x| dump_operand(x, f)).collect();
            match dst {
                Some(d) => format!("%{} = call {}({})", d.0, t, a.join(", ")),
                None => format!("call {}({})", t, a.join(", ")),
            }
        }
        SsaStmt::Print { values, .. } => {
            let a: Vec<String> = values.iter().map(|x| dump_operand(x, f)).collect();
            format!("println({})", a.join(", "))
        }
        SsaStmt::SetField {
            dst,
            base,
            field,
            value,
            ..
        } => {
            format!(
                "%{} = setfield {}.f{field} = {}",
                dst.0,
                dump_operand(base, f),
                dump_operand(value, f)
            )
        }
    }
}

fn dump_rv(r: &SsaRvalue, f: &SsaFunc) -> String {
    match r {
        SsaRvalue::Use(o) => format!("use {}", dump_operand(o, f)),
        SsaRvalue::Binary { op, l, r } => {
            format!("{op:?} {} {}", dump_operand(l, f), dump_operand(r, f))
        }
        SsaRvalue::Unary { op, v } => format!("{op:?} {}", dump_operand(v, f)),
        SsaRvalue::Cast { to, v } => format!("cast {} as {to}", dump_operand(v, f)),
        SsaRvalue::Tuple(es) => {
            let a: Vec<String> = es.iter().map(|x| dump_operand(x, f)).collect();
            format!("tuple({})", a.join(", "))
        }
        SsaRvalue::Field { base, field } => {
            format!("{}.f{field}", dump_operand(base, f))
        }
        SsaRvalue::Enum {
            en,
            variant,
            payload,
        } => {
            let a: Vec<String> = payload.iter().map(|x| dump_operand(x, f)).collect();
            format!("{en}::{variant}({})", a.join(", "))
        }
        SsaRvalue::Discriminant(b) => format!("discr({})", dump_operand(b, f)),
        SsaRvalue::VariantPayload { base, idx } => {
            format!("payload{}({})", idx, dump_operand(base, f))
        }
        SsaRvalue::SliceLen(b) => format!("len({})", dump_operand(b, f)),
        SsaRvalue::SliceIndex { base, idx } => {
            format!("{}[{}]", dump_operand(base, f), dump_operand(idx, f))
        }
    }
}

fn dump_term(t: &SsaTerm) -> String {
    match t {
        SsaTerm::Goto(b) => format!("goto bb{}", b.0),
        SsaTerm::Branch {
            then_bb, else_bb, ..
        } => {
            format!("branch bb{} else bb{}", then_bb.0, else_bb.0)
        }
        SsaTerm::Return(None) => "return".to_string(),
        SsaTerm::Return(Some(_)) => "return %_".to_string(),
        SsaTerm::Trap { kind, .. } => format!("trap {kind:?}"),
        SsaTerm::Unreachable => "unreachable(?)".to_string(),
    }
}

// --- construction ---

struct Builder<'m> {
    mir: &'m MirFunc,
    values: Vec<Ty>,
    names: Vec<String>,
}

impl<'m> Builder<'m> {
    fn fresh(&mut self, ty: Ty, name: String) -> SsaValue {
        let id = SsaValue(self.values.len() as u32);
        self.values.push(ty);
        self.names.push(name);
        id
    }

    fn local_ty(&self, local: Local) -> Ty {
        self.mir
            .locals
            .get(local.0 as usize)
            .cloned()
            .unwrap_or(Ty::Error)
    }

    fn local_name(&self, local: Local) -> String {
        self.mir
            .local_names
            .get(local.0 as usize)
            .cloned()
            .unwrap_or_else(|| format!("l{}", local.0))
    }
}

fn successors_of(term: &lobster_mir::Terminator) -> Vec<BlockId> {
    match term {
        lobster_mir::Terminator::Goto(t) => vec![*t],
        lobster_mir::Terminator::Branch {
            then_bb, else_bb, ..
        } => vec![*then_bb, *else_bb],
        _ => Vec::new(),
    }
}

/// Span of a MIR statement (every statement carries one).
fn stmt_span(stmt: &lobster_mir::MirStmt) -> Span {
    match stmt {
        lobster_mir::MirStmt::Assign { span, .. }
        | lobster_mir::MirStmt::Call { span, .. }
        | lobster_mir::MirStmt::Print { span, .. }
        | lobster_mir::MirStmt::SetField { span, .. } => *span,
    }
}

/// Location for a join block's phis: first statement of the join, else
/// first statement of the first predecessor that has one, else `None`.
fn join_span(mir: &MirFunc, preds: &[Vec<BlockId>], join: usize) -> Option<Span> {
    if let Some(span) = mir.blocks.get(join)?.stmts.first().map(stmt_span) {
        return Some(span);
    }
    preds
        .get(join)?
        .iter()
        .find_map(|p| mir.blocks.get(p.0 as usize)?.stmts.first().map(stmt_span))
}

fn reverse_postorder(entry: BlockId, blocks: &[lobster_mir::MirBlock]) -> Vec<BlockId> {
    let n = blocks.len();
    let mut visited = vec![false; n];
    let mut order = Vec::with_capacity(n);
    let mut stack = vec![(entry, false)];
    while let Some((bb, expanded)) = stack.pop() {
        let i = bb.0 as usize;
        if i >= n {
            continue;
        }
        if expanded {
            order.push(bb);
            continue;
        }
        if visited[i] {
            continue;
        }
        visited[i] = true;
        stack.push((bb, true));
        for s in successors_of(&blocks[i].term).into_iter().rev() {
            if (s.0 as usize) < n && !visited[s.0 as usize] {
                stack.push((s, false));
            }
        }
    }
    order.reverse();
    // Append any blocks unreachable from entry in index order so every
    // block still gets renamed (totality for hand-built MIR).
    for (i, v) in visited.iter().enumerate() {
        if !v {
            order.push(BlockId(i as u32));
        }
    }
    order
}

fn build_func(mir: &MirFunc) -> SsaFunc {
    let n = mir.blocks.len();
    let mut preds: Vec<Vec<BlockId>> = vec![Vec::new(); n];
    for (i, b) in mir.blocks.iter().enumerate() {
        for s in successors_of(&b.term) {
            if (s.0 as usize) < n {
                preds[s.0 as usize].push(BlockId(i as u32));
            }
        }
    }
    for p in &mut preds {
        p.sort_by_key(|b| b.0);
    }

    let mut builder = Builder {
        mir,
        values: Vec::new(),
        names: Vec::new(),
    };

    // Pre-create one phi per MIR local at every join (simple join phis).
    let mut phi_skel: Vec<Vec<(Local, SsaValue, Ty)>> = vec![Vec::new(); n];
    if n > 0 {
        for (i, p) in preds.iter().enumerate() {
            if p.len() > 1 {
                for local_idx in 0..mir.locals.len() {
                    let local = Local(local_idx as u32);
                    let ty = builder.local_ty(local);
                    let name = format!("phi:{}:bb{i}", builder.local_name(local));
                    let dst = builder.fresh(ty.clone(), name);
                    phi_skel[i].push((local, dst, ty));
                }
            }
        }
    }

    // Parameters are the first definitions at entry.
    let mut params = Vec::with_capacity(mir.params.len());
    let mut entry_cur: HashMap<Local, SsaValue> = HashMap::new();
    for p in &mir.params {
        let ty = builder.local_ty(*p);
        let name = format!("arg:{}", builder.local_name(*p));
        let v = builder.fresh(ty, name);
        entry_cur.insert(*p, v);
        params.push(v);
    }

    let order = if n == 0 {
        Vec::new()
    } else {
        reverse_postorder(mir.entry, &mir.blocks)
    };

    let mut cur: HashMap<Local, SsaValue> = entry_cur;
    let mut exits: Vec<Option<HashMap<Local, SsaValue>>> = vec![None; n];
    let mut out_stmts: Vec<Vec<SsaStmt>> = vec![Vec::new(); n];
    let mut out_terms: Vec<SsaTerm> = vec![SsaTerm::Unreachable; n];

    for bb in order {
        let i = bb.0 as usize;
        if i >= n {
            continue;
        }
        // Join blocks take phi versions; single-predecessor blocks inherit
        // the predecessor exit; entry keeps params; unreachable blocks
        // start empty (uses become `Undef`).
        if preds[i].len() > 1 {
            for (local, dst, _) in &phi_skel[i] {
                cur.insert(*local, *dst);
            }
        } else if preds[i].is_empty() {
            if bb != mir.entry {
                cur = HashMap::new();
            }
        } else if let Some(exit) = exits[preds[i][0].0 as usize].clone() {
            cur = exit;
        }
        let block = &mir.blocks[i];
        for stmt in &block.stmts {
            rename_stmt(&mut builder, &mut cur, stmt, &mut out_stmts[i]);
        }
        out_terms[i] = rename_term(&builder, &cur, &block.term);
        exits[i] = Some(cur.clone());
    }

    let mut blocks = Vec::with_capacity(n);
    for (i, skel) in phi_skel.into_iter().enumerate() {
        let span = join_span(mir, &preds, i);
        let phis = skel
            .into_iter()
            .map(|(origin, dst, ty)| {
                let arms = preds[i]
                    .iter()
                    .map(|p| {
                        let op = exits
                            .get(p.0 as usize)
                            .and_then(|e| e.as_ref())
                            .and_then(|m| m.get(&origin))
                            .map_or(SsaOperand::Undef, |v| SsaOperand::Value(*v));
                        (*p, op)
                    })
                    .collect();
                Phi {
                    dst,
                    ty,
                    origin,
                    arms,
                    span,
                }
            })
            .collect();
        blocks.push(SsaBlock {
            phis,
            stmts: std::mem::take(&mut out_stmts[i]),
            term: out_terms[i].clone(),
        });
    }

    SsaFunc {
        name: mir.name.clone(),
        params,
        ret: mir.ret.clone(),
        values: builder.values,
        value_names: builder.names,
        blocks,
        entry: mir.entry,
    }
}

fn destruct_func(ssa: &SsaFunc) -> MirFunc {
    use lobster_mir::{MirBlock, MirFunc, MirStmt as MirS};
    let n = ssa.blocks.len();
    // Identity mapping: MIR local `i` holds SSA value `i`.
    let undef = Local(ssa.values.len() as u32);
    let mut locals = ssa.values.clone();
    locals.push(Ty::Error);
    let mut local_names = ssa.value_names.clone();
    local_names.push("undef".to_string());

    let mut succs: Vec<Vec<BlockId>> = vec![Vec::new(); n];
    for (i, b) in ssa.blocks.iter().enumerate() {
        for s in successors_of_term(&b.term) {
            if (s.0 as usize) < n {
                succs[i].push(s);
            }
        }
    }

    // Map statements and terminators first; phi copies land on edges below.
    let mut blocks: Vec<MirBlock> = Vec::with_capacity(n);
    for b in &ssa.blocks {
        let stmts = b.stmts.iter().map(|s| destruct_stmt(s, undef)).collect();
        blocks.push(MirBlock {
            stmts,
            term: destruct_term(&b.term, undef),
        });
    }

    // One copy per phi arm, grouped by (predecessor, successor) edge.
    // Dead phis (dst never used) emit no copies: a copy reads its arm
    // operand, and reading `Undef` traps — but the original program never
    // reads a dead value, so emitting the copy would invent a trap.
    let live = live_values(ssa);
    let mut edge_copies: HashMap<(u32, u32), Vec<MirS>> = HashMap::new();
    let mut edge_order: Vec<(u32, u32)> = Vec::new();
    for (s, b) in ssa.blocks.iter().enumerate() {
        for phi in &b.phis {
            if !live.contains(&phi.dst.0) {
                continue;
            }
            let span = copy_span(ssa, s, &phi.span);
            for (pred, op) in &phi.arms {
                let key = (pred.0, s as u32);
                if !edge_copies.contains_key(&key) {
                    edge_order.push(key);
                }
                edge_copies.entry(key).or_default().push(MirS::Assign {
                    dst: Local(phi.dst.0),
                    rv: lobster_mir::Rvalue::Use(destruct_operand(op, undef)),
                    span,
                });
            }
        }
    }
    for (p, s) in edge_order {
        let copies = edge_copies.remove(&(p, s)).unwrap_or_default();
        if copies.is_empty() || (p as usize) >= blocks.len() {
            continue;
        }
        if succs.get(p as usize).is_some_and(|ss| ss.len() == 1) {
            // Single-successor edge: the copy runs whenever `p` runs.
            blocks[p as usize].stmts.extend(copies);
            continue;
        }
        // Critical edge: split it so copies run only on this edge.
        let split = BlockId(blocks.len() as u32);
        retarget_edge(&mut blocks[p as usize].term, BlockId(s), split);
        blocks.push(MirBlock {
            stmts: copies,
            term: lobster_mir::Terminator::Goto(BlockId(s)),
        });
    }

    MirFunc {
        name: ssa.name.clone(),
        params: ssa.params.iter().map(|v| Local(v.0)).collect(),
        ret: ssa.ret.clone(),
        locals,
        local_names,
        blocks,
        entry: ssa.entry,
    }
}

/// Values read anywhere (arms, rvalues, calls, prints, terminators).
fn live_values(func: &SsaFunc) -> HashSet<u32> {
    let mut live = HashSet::new();
    let mut mark = |op: &SsaOperand| {
        if let SsaOperand::Value(v) = op {
            live.insert(v.0);
        }
    };
    for block in &func.blocks {
        for phi in &block.phis {
            for (_, op) in &phi.arms {
                mark(op);
            }
        }
        for stmt in &block.stmts {
            match stmt {
                SsaStmt::Assign { rv, .. } => {
                    for o in ssa_rvalue_operands(rv) {
                        mark(o);
                    }
                }
                SsaStmt::Call { target, args, .. } => {
                    if let SsaCallTarget::Value(o) = target {
                        mark(o);
                    }
                    for a in args {
                        mark(a);
                    }
                }
                SsaStmt::Print { values, .. } => {
                    for v in values {
                        mark(v);
                    }
                }
                SsaStmt::SetField { base, value, .. } => {
                    mark(base);
                    mark(value);
                }
            }
        }
        match &block.term {
            SsaTerm::Branch { cond, .. } => mark(cond),
            SsaTerm::Return(Some(o)) => mark(o),
            _ => {}
        }
    }
    live
}

fn ssa_rvalue_operands(rv: &SsaRvalue) -> Vec<&SsaOperand> {
    match rv {
        SsaRvalue::Use(o) => vec![o],
        SsaRvalue::Binary { l, r, .. } => vec![l, r],
        SsaRvalue::Unary { v, .. } => vec![v],
        SsaRvalue::Cast { v, .. } => vec![v],
        SsaRvalue::Tuple(es) => es.iter().collect(),
        SsaRvalue::Field { base, .. } => vec![base],
        SsaRvalue::Enum { payload, .. } => payload.iter().collect(),
        SsaRvalue::Discriminant(b) | SsaRvalue::SliceLen(b) => vec![b],
        SsaRvalue::VariantPayload { base, .. } => vec![base],
        SsaRvalue::SliceIndex { base, idx } => vec![base, idx],
    }
}

/// Location for a phi copy: the phi's own span, else the first statement
/// of the successor, else a synthetic span (copies never trap, so it never
/// renders).
fn copy_span(ssa: &SsaFunc, succ: usize, phi_span: &Option<Span>) -> Span {
    if let Some(span) = phi_span {
        return *span;
    }
    if let Some(span) = ssa
        .blocks
        .get(succ)
        .and_then(|b| b.stmts.first())
        .map(ssa_stmt_span)
    {
        return span;
    }
    synthetic_span()
}

fn ssa_stmt_span(stmt: &SsaStmt) -> Span {
    match stmt {
        SsaStmt::Assign { span, .. }
        | SsaStmt::Call { span, .. }
        | SsaStmt::Print { span, .. }
        | SsaStmt::SetField { span, .. } => *span,
    }
}

/// Rewrite the `from` edge of a terminator to `to` (for edge splitting).
fn retarget_edge(term: &mut lobster_mir::Terminator, from: BlockId, to: BlockId) {
    match term {
        lobster_mir::Terminator::Goto(t) => {
            if *t == from {
                *t = to;
            }
        }
        lobster_mir::Terminator::Branch {
            then_bb, else_bb, ..
        } => {
            if *then_bb == from {
                *then_bb = to;
            }
            if *else_bb == from {
                *else_bb = to;
            }
        }
        _ => {}
    }
}

fn destruct_operand(op: &SsaOperand, undef: Local) -> lobster_mir::Operand {
    match op {
        SsaOperand::Value(v) => lobster_mir::Operand::Local(Local(v.0)),
        SsaOperand::Const(c) => lobster_mir::Operand::Const(c.clone()),
        SsaOperand::Undef => lobster_mir::Operand::Local(undef),
    }
}

fn destruct_rvalue(rv: &SsaRvalue, undef: Local) -> lobster_mir::Rvalue {
    use lobster_mir::Rvalue as R;
    let map = |o: &SsaOperand| destruct_operand(o, undef);
    match rv {
        SsaRvalue::Use(o) => R::Use(map(o)),
        SsaRvalue::Binary { op, l, r } => R::Binary {
            op: *op,
            l: map(l),
            r: map(r),
        },
        SsaRvalue::Unary { op, v } => R::Unary { op: *op, v: map(v) },
        SsaRvalue::Cast { to, v } => R::Cast {
            to: to.clone(),
            v: map(v),
        },
        SsaRvalue::Tuple(es) => R::Tuple(es.iter().map(map).collect()),
        SsaRvalue::Field { base, field } => R::Field {
            base: map(base),
            field: *field,
        },
        SsaRvalue::Enum {
            en,
            variant,
            payload,
        } => R::Enum {
            en: en.clone(),
            variant: *variant,
            payload: payload.iter().map(map).collect(),
        },
        SsaRvalue::Discriminant(b) => R::Discriminant(map(b)),
        SsaRvalue::VariantPayload { base, idx } => R::VariantPayload {
            base: map(base),
            idx: *idx,
        },
        SsaRvalue::SliceLen(b) => R::SliceLen(map(b)),
        SsaRvalue::SliceIndex { base, idx } => R::SliceIndex {
            base: map(base),
            idx: map(idx),
        },
    }
}

fn destruct_stmt(stmt: &SsaStmt, undef: Local) -> lobster_mir::MirStmt {
    use lobster_mir::MirStmt as MirS;
    match stmt {
        SsaStmt::Assign { dst, rv, span } => MirS::Assign {
            dst: Local(dst.0),
            rv: destruct_rvalue(rv, undef),
            span: *span,
        },
        SsaStmt::Call {
            dst,
            target,
            args,
            span,
        } => MirS::Call {
            dst: dst.map(|v| Local(v.0)),
            target: match target {
                SsaCallTarget::Fn(n) => lobster_mir::CallTarget::Fn(n.clone()),
                SsaCallTarget::Value(o) => {
                    lobster_mir::CallTarget::Value(destruct_operand(o, undef))
                }
            },
            args: args.iter().map(|a| destruct_operand(a, undef)).collect(),
            span: *span,
        },
        SsaStmt::Print { values, span } => MirS::Print {
            values: values.iter().map(|v| destruct_operand(v, undef)).collect(),
            span: *span,
        },
        SsaStmt::SetField {
            dst: _,
            base,
            field,
            value,
            span,
        } => {
            // MIR updates the slot in place; the SSA `dst` version and the
            // base slot coincide by the identity mapping.
            let base_local = match destruct_operand(base, undef) {
                lobster_mir::Operand::Local(l) => l,
                lobster_mir::Operand::Const(_) => undef,
            };
            MirS::SetField {
                base: base_local,
                field: *field,
                value: destruct_operand(value, undef),
                span: *span,
            }
        }
    }
}

fn destruct_term(term: &SsaTerm, undef: Local) -> lobster_mir::Terminator {
    use lobster_mir::Terminator as MirT;
    match term {
        SsaTerm::Goto(t) => MirT::Goto(*t),
        SsaTerm::Branch {
            cond,
            then_bb,
            else_bb,
        } => MirT::Branch {
            cond: destruct_operand(cond, undef),
            then_bb: *then_bb,
            else_bb: *else_bb,
        },
        SsaTerm::Return(v) => MirT::Return(v.as_ref().map(|o| destruct_operand(o, undef))),
        SsaTerm::Trap { kind, span } => MirT::Trap {
            kind: kind.clone(),
            span: *span,
        },
        SsaTerm::Unreachable => MirT::Trap {
            kind: lobster_mir::TrapKind::Unreachable("destruct of unreachable".to_string()),
            span: synthetic_span(),
        },
    }
}

fn rename_operand(
    _builder: &Builder<'_>,
    cur: &HashMap<Local, SsaValue>,
    op: &lobster_mir::Operand,
) -> SsaOperand {
    match op {
        lobster_mir::Operand::Local(l) => cur
            .get(l)
            .map_or(SsaOperand::Undef, |v| SsaOperand::Value(*v)),
        lobster_mir::Operand::Const(c) => SsaOperand::Const(c.clone()),
    }
}

fn rename_rvalue(
    builder: &Builder<'_>,
    cur: &HashMap<Local, SsaValue>,
    rv: &lobster_mir::Rvalue,
) -> SsaRvalue {
    use lobster_mir::Rvalue as R;
    match rv {
        R::Use(o) => SsaRvalue::Use(rename_operand(builder, cur, o)),
        R::Binary { op, l, r } => SsaRvalue::Binary {
            op: *op,
            l: rename_operand(builder, cur, l),
            r: rename_operand(builder, cur, r),
        },
        R::Unary { op, v } => SsaRvalue::Unary {
            op: *op,
            v: rename_operand(builder, cur, v),
        },
        R::Cast { to, v } => SsaRvalue::Cast {
            to: to.clone(),
            v: rename_operand(builder, cur, v),
        },
        R::Tuple(es) => {
            SsaRvalue::Tuple(es.iter().map(|e| rename_operand(builder, cur, e)).collect())
        }
        R::Field { base, field } => SsaRvalue::Field {
            base: rename_operand(builder, cur, base),
            field: *field,
        },
        R::Enum {
            en,
            variant,
            payload,
        } => SsaRvalue::Enum {
            en: en.clone(),
            variant: *variant,
            payload: payload
                .iter()
                .map(|e| rename_operand(builder, cur, e))
                .collect(),
        },
        R::Discriminant(b) => SsaRvalue::Discriminant(rename_operand(builder, cur, b)),
        R::VariantPayload { base, idx } => SsaRvalue::VariantPayload {
            base: rename_operand(builder, cur, base),
            idx: *idx,
        },
        R::SliceLen(b) => SsaRvalue::SliceLen(rename_operand(builder, cur, b)),
        R::SliceIndex { base, idx } => SsaRvalue::SliceIndex {
            base: rename_operand(builder, cur, base),
            idx: rename_operand(builder, cur, idx),
        },
    }
}

fn rename_stmt(
    builder: &mut Builder<'_>,
    cur: &mut HashMap<Local, SsaValue>,
    stmt: &lobster_mir::MirStmt,
    out: &mut Vec<SsaStmt>,
) {
    match stmt {
        lobster_mir::MirStmt::Assign { dst, rv, span } => {
            let rv = rename_rvalue(builder, cur, rv);
            let ty = builder.local_ty(*dst);
            let name = format!("ssa:{}", builder.local_name(*dst));
            let v = builder.fresh(ty, name);
            cur.insert(*dst, v);
            out.push(SsaStmt::Assign {
                dst: v,
                rv,
                span: *span,
            });
        }
        lobster_mir::MirStmt::Call {
            dst,
            target,
            args,
            span,
        } => {
            let target = match target {
                lobster_mir::CallTarget::Fn(n) => SsaCallTarget::Fn(n.clone()),
                lobster_mir::CallTarget::Value(o) => {
                    SsaCallTarget::Value(rename_operand(builder, cur, o))
                }
            };
            let args = args
                .iter()
                .map(|a| rename_operand(builder, cur, a))
                .collect();
            let dst = dst.map(|l| {
                let ty = builder.local_ty(l);
                let name = format!("call:{}", builder.local_name(l));
                let v = builder.fresh(ty, name);
                cur.insert(l, v);
                v
            });
            out.push(SsaStmt::Call {
                dst,
                target,
                args,
                span: *span,
            });
        }
        lobster_mir::MirStmt::Print { values, span } => {
            let values = values
                .iter()
                .map(|v| rename_operand(builder, cur, v))
                .collect();
            out.push(SsaStmt::Print {
                values,
                span: *span,
            });
        }
        lobster_mir::MirStmt::SetField {
            base,
            field,
            value,
            span,
        } => {
            let base_op = rename_operand(builder, cur, &lobster_mir::Operand::Local(*base));
            let value = rename_operand(builder, cur, value);
            let ty = builder.local_ty(*base);
            let name = format!("set:{}", builder.local_name(*base));
            let dst = builder.fresh(ty, name);
            cur.insert(*base, dst);
            out.push(SsaStmt::SetField {
                dst,
                base: base_op,
                field: *field,
                value,
                span: *span,
            });
        }
    }
}

fn rename_term(
    builder: &Builder<'_>,
    cur: &HashMap<Local, SsaValue>,
    term: &lobster_mir::Terminator,
) -> SsaTerm {
    match term {
        lobster_mir::Terminator::Goto(t) => SsaTerm::Goto(*t),
        lobster_mir::Terminator::Branch {
            cond,
            then_bb,
            else_bb,
        } => SsaTerm::Branch {
            cond: rename_operand(builder, cur, cond),
            then_bb: *then_bb,
            else_bb: *else_bb,
        },
        lobster_mir::Terminator::Return(v) => {
            SsaTerm::Return(v.as_ref().map(|o| rename_operand(builder, cur, o)))
        }
        lobster_mir::Terminator::Trap { kind, span } => SsaTerm::Trap {
            kind: kind.clone(),
            span: *span,
        },
        lobster_mir::Terminator::Unreachable => SsaTerm::Unreachable,
    }
}

// --- verifier ---

fn err(func: &str, block: Option<u32>, message: String, out: &mut Vec<VerifyError>) {
    out.push(VerifyError {
        func: func.to_string(),
        block,
        message,
    });
}

fn const_ty(c: &Const_) -> Ty {
    match c {
        Const_::Int(_, t) => Ty::Int(*t),
        Const_::Float(_, t) => Ty::Float(*t),
        Const_::Bool(_) => Ty::Bool,
        Const_::Char(_) => Ty::Char,
        Const_::Str(_) => Ty::Str,
        Const_::FuncRef(_) => Ty::Error,
        Const_::Unit => Ty::Unit,
    }
}

fn peel(mut ty: &Ty) -> &Ty {
    loop {
        match ty {
            Ty::Ref { inner, .. } => ty = inner,
            _ => return ty,
        }
    }
}

struct FuncCx<'a> {
    func: &'a SsaFunc,
    prog: &'a SsaProgram,
    defined: HashSet<u32>,
}

impl<'a> FuncCx<'a> {
    fn operand_ty(&self, op: &SsaOperand) -> Ty {
        match op {
            SsaOperand::Value(v) => self
                .func
                .values
                .get(v.0 as usize)
                .cloned()
                .unwrap_or(Ty::Error),
            SsaOperand::Const(c) => const_ty(c),
            SsaOperand::Undef => Ty::Error,
        }
    }

    fn check_operand(
        &self,
        op: &SsaOperand,
        what: &str,
        block: Option<u32>,
        out: &mut Vec<VerifyError>,
    ) {
        if let SsaOperand::Value(v) = op {
            if self.func.values.get(v.0 as usize).is_none() {
                err(
                    &self.func.name,
                    block,
                    format!("{what} uses unknown value %{}\n", v.0),
                    out,
                );
            } else if !self.defined.contains(&v.0) {
                err(
                    &self.func.name,
                    block,
                    format!("{what} uses undefined value %{}", v.0),
                    out,
                );
            }
        }
    }

    fn check_rvalue_operands(
        &self,
        rv: &SsaRvalue,
        what: &str,
        block: Option<u32>,
        out: &mut Vec<VerifyError>,
    ) {
        let mut operands = Vec::new();
        match rv {
            SsaRvalue::Use(o) => operands.push(o),
            SsaRvalue::Binary { l, r, .. } => operands.extend([l, r]),
            SsaRvalue::Unary { v, .. } => operands.push(v),
            SsaRvalue::Cast { v, .. } => operands.push(v),
            SsaRvalue::Tuple(es) => operands.extend(es),
            SsaRvalue::Field { base, .. } => operands.push(base),
            SsaRvalue::Enum { payload, .. } => operands.extend(payload),
            SsaRvalue::Discriminant(b) | SsaRvalue::SliceLen(b) => operands.push(b),
            SsaRvalue::VariantPayload { base, .. } => operands.push(base),
            SsaRvalue::SliceIndex { base, idx } => operands.extend([base, idx]),
        }
        for o in operands {
            self.check_operand(o, what, block, out);
        }
    }
}

fn rvalue_ty(cx: &FuncCx<'_>, rv: &SsaRvalue) -> Ty {
    match rv {
        SsaRvalue::Use(o) => cx.operand_ty(o),
        SsaRvalue::Binary { op, l, r } => {
            let lt = cx.operand_ty(l);
            let rt = cx.operand_ty(r);
            if matches!(lt, Ty::Error) || matches!(rt, Ty::Error) {
                return Ty::Error;
            }
            match op {
                BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => Ty::Bool,
                _ => lt,
            }
        }
        SsaRvalue::Unary { v, .. } => cx.operand_ty(v),
        SsaRvalue::Cast { to, .. } => to.clone(),
        SsaRvalue::Tuple(es) => Ty::Tuple(es.iter().map(|e| cx.operand_ty(e)).collect()),
        SsaRvalue::Field { base, field } => field_ty(cx, base, *field),
        SsaRvalue::Enum {
            en,
            variant: _,
            payload,
        } => Ty::Adt {
            kind: lobster_types::AdtKind::Enum,
            name: en.clone(),
            args: payload.iter().map(|_| Ty::Error).collect(),
        },
        SsaRvalue::Discriminant(_) => Ty::Int(IntTy::U32),
        SsaRvalue::VariantPayload { .. } => Ty::Error,
        SsaRvalue::SliceLen(_) => Ty::Int(IntTy::Usize),
        SsaRvalue::SliceIndex { base, .. } => match peel(&cx.operand_ty(base)) {
            Ty::Slice(elem) | Ty::Array(elem, _) => (**elem).clone(),
            _ => Ty::Error,
        },
    }
}

fn field_ty(cx: &FuncCx<'_>, base: &SsaOperand, field: usize) -> Ty {
    match peel(&cx.operand_ty(base)) {
        Ty::Tuple(items) => items.get(field).cloned().unwrap_or(Ty::Error),
        Ty::Adt { .. } | Ty::Error => Ty::Error,
        _ => Ty::Error,
    }
}

fn check_binary_operands(
    cx: &FuncCx<'_>,
    op: BinOp,
    l: &SsaOperand,
    r: &SsaOperand,
    block: Option<u32>,
    out: &mut Vec<VerifyError>,
) {
    let lt = cx.operand_ty(l);
    let rt = cx.operand_ty(r);
    if matches!(lt, Ty::Error) || matches!(rt, Ty::Error) {
        return;
    }
    let ok = match (op, &lt, &rt) {
        (BinOp::Shl | BinOp::Shr, Ty::Int(_), Ty::Int(_)) => true,
        (_, Ty::Int(a), Ty::Int(b)) => a == b,
        (_, Ty::Float(a), Ty::Float(b)) => a == b,
        (
            BinOp::And
            | BinOp::Or
            | BinOp::Eq
            | BinOp::Ne
            | BinOp::Lt
            | BinOp::Le
            | BinOp::Gt
            | BinOp::Ge,
            Ty::Bool,
            Ty::Bool,
        ) => true,
        (
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge,
            Ty::Char,
            Ty::Char,
        ) => true,
        _ => false,
    };
    if !ok {
        err(
            &cx.func.name,
            block,
            format!("cannot apply `{op:?}` to `{lt}` and `{rt}`"),
            out,
        );
    }
}

fn check_unary_operands(
    cx: &FuncCx<'_>,
    op: UnOp,
    v: &SsaOperand,
    block: Option<u32>,
    out: &mut Vec<VerifyError>,
) {
    let t = cx.operand_ty(v);
    if matches!(t, Ty::Error) {
        return;
    }
    let ok = matches!(
        (op, &t),
        (UnOp::Neg, Ty::Int(_) | Ty::Float(_)) | (UnOp::Not, Ty::Bool | Ty::Int(_))
    );
    if !ok {
        err(
            &cx.func.name,
            block,
            format!("cannot apply `{op:?}` to `{t}`"),
            out,
        );
    }
}

fn check_cast(cx: &FuncCx<'_>, from: &Ty, to: &Ty, block: Option<u32>, out: &mut Vec<VerifyError>) {
    if matches!(from, Ty::Error) || matches!(to, Ty::Error) {
        return;
    }
    let ok = match (from, to) {
        (Ty::Int(_), Ty::Int(_))
        | (Ty::Int(_), Ty::Float(_))
        | (Ty::Float(_), Ty::Float(_))
        | (Ty::Float(_), Ty::Int(_))
        | (Ty::Bool, Ty::Int(_))
        | (Ty::Char, Ty::Int(_))
        | (Ty::Int(_), Ty::Char) => true,
        (a, b) => a == b,
    };
    if !ok {
        err(
            &cx.func.name,
            block,
            format!("cannot cast `{from}` as `{to}`"),
            out,
        );
    }
}

fn verify_func(func: &SsaFunc, prog: &SsaProgram, out: &mut Vec<VerifyError>) {
    if func.blocks.is_empty() {
        err(&func.name, None, "function has no blocks".to_string(), out);
        return;
    }
    if func.entry.0 as usize >= func.blocks.len() {
        err(
            &func.name,
            None,
            format!("entry bb{} out of range", func.entry.0),
            out,
        );
        return;
    }
    let n = func.blocks.len();
    let mut preds: Vec<Vec<u32>> = vec![Vec::new(); n];
    for (i, block) in func.blocks.iter().enumerate() {
        for s in successors_of_term(&block.term) {
            if (s.0 as usize) < n {
                preds[s.0 as usize].push(i as u32);
            } else {
                err(
                    &func.name,
                    Some(i as u32),
                    format!("successor bb{} out of range", s.0),
                    out,
                );
            }
        }
    }

    // Definitions: each value exactly once.
    let mut defined: HashSet<u32> = HashSet::new();
    let mut defs: Vec<(SsaValue, u32, &'static str)> = Vec::new();
    for (i, block) in func.blocks.iter().enumerate() {
        let bb = i as u32;
        for p in &block.phis {
            defs.push((p.dst, bb, "phi"));
        }
        for stmt in &block.stmts {
            match stmt {
                SsaStmt::Assign { dst, .. } => defs.push((*dst, bb, "assign")),
                SsaStmt::Call { dst: Some(d), .. } => defs.push((*d, bb, "call")),
                SsaStmt::Call { dst: None, .. } | SsaStmt::Print { .. } => {}
                SsaStmt::SetField { dst, .. } => defs.push((*dst, bb, "setfield")),
            }
        }
    }
    for (v, bb, what) in defs {
        if (v.0 as usize) >= func.values.len() {
            err(
                &func.name,
                Some(bb),
                format!("{what} defines unknown value %{}", v.0),
                out,
            );
        } else if !defined.insert(v.0) {
            err(
                &func.name,
                Some(bb),
                format!("value %{} defined twice", v.0),
                out,
            );
        }
    }
    for p in &func.params {
        if (p.0 as usize) >= func.values.len() {
            err(
                &func.name,
                None,
                format!("param %{} out of range", p.0),
                out,
            );
        } else if !defined.insert(p.0) {
            err(
                &func.name,
                None,
                format!("param %{} defined twice", p.0),
                out,
            );
        }
    }

    let cx = FuncCx {
        func,
        prog,
        defined,
    };

    for (i, block) in func.blocks.iter().enumerate() {
        let bb = Some(i as u32);
        if matches!(block.term, SsaTerm::Unreachable) {
            err(
                &func.name,
                bb,
                "block ends in placeholder terminator".to_string(),
                out,
            );
        }
        // Phis only at joins; arms must match predecessors exactly.
        if !block.phis.is_empty() && preds[i].len() < 2 {
            err(
                &func.name,
                bb,
                "phi in block with fewer than two predecessors".to_string(),
                out,
            );
        }
        for phi in &block.phis {
            if phi.arms.len() != preds[i].len() {
                err(
                    &func.name,
                    bb,
                    format!(
                        "phi %{} has {} arms but block has {} predecessors",
                        phi.dst.0,
                        phi.arms.len(),
                        preds[i].len()
                    ),
                    out,
                );
            }
            let mut seen: HashSet<u32> = HashSet::new();
            for (pred, op) in &phi.arms {
                if (pred.0 as usize) >= n {
                    err(
                        &func.name,
                        bb,
                        format!("phi arm from bb{} out of range", pred.0),
                        out,
                    );
                    continue;
                }
                if !preds[i].contains(&pred.0) {
                    err(
                        &func.name,
                        bb,
                        format!("phi arm from non-predecessor bb{}", pred.0),
                        out,
                    );
                }
                if !seen.insert(pred.0) {
                    err(
                        &func.name,
                        bb,
                        format!("duplicate phi arm from bb{}", pred.0),
                        out,
                    );
                }
                cx.check_operand(op, "phi arm", bb, out);
            }
            let want = func
                .values
                .get(phi.dst.0 as usize)
                .cloned()
                .unwrap_or(Ty::Error);
            if want != phi.ty && !matches!(want, Ty::Error) && !matches!(phi.ty, Ty::Error) {
                err(
                    &func.name,
                    bb,
                    format!("phi %{} type mismatch", phi.dst.0),
                    out,
                );
            }
        }
        for stmt in &block.stmts {
            match stmt {
                SsaStmt::Assign { dst: _, rv, .. } => {
                    cx.check_rvalue_operands(rv, "assign", bb, out);
                    match rv {
                        SsaRvalue::Binary { op, l, r } => {
                            check_binary_operands(&cx, *op, l, r, bb, out);
                        }
                        SsaRvalue::Unary { op, v } => {
                            check_unary_operands(&cx, *op, v, bb, out);
                        }
                        SsaRvalue::Cast { to, v } => {
                            let from = cx.operand_ty(v);
                            check_cast(&cx, &from, to, bb, out);
                        }
                        SsaRvalue::Field { base, field } => {
                            check_field_base(&cx, base, *field, bb, out);
                        }
                        SsaRvalue::Enum {
                            en,
                            variant,
                            payload: _,
                        } => {
                            if let Some(layout) = prog.enums.get(en) {
                                if *variant >= layout.len() {
                                    err(
                                        &func.name,
                                        bb,
                                        format!("variant {variant} out of range for `{en}`"),
                                        out,
                                    );
                                }
                            }
                        }
                        SsaRvalue::Discriminant(base) => {
                            if !is_enum_or_error(&cx.operand_ty(base)) {
                                err(&func.name, bb, "discriminant of non-enum".to_string(), out);
                            }
                        }
                        SsaRvalue::VariantPayload { base, .. } => {
                            if !is_enum_or_error(&cx.operand_ty(base)) {
                                err(&func.name, bb, "payload of non-enum".to_string(), out);
                            }
                        }
                        SsaRvalue::SliceLen(base) => {
                            if !is_slice_like(&cx.operand_ty(base)) {
                                err(&func.name, bb, "length of non-slice".to_string(), out);
                            }
                        }
                        SsaRvalue::SliceIndex { base, idx } => {
                            if !is_slice_like(&cx.operand_ty(base)) {
                                err(&func.name, bb, "index into non-slice".to_string(), out);
                            }
                            if !is_int_or_error(&cx.operand_ty(idx)) {
                                err(&func.name, bb, "index with non-integer".to_string(), out);
                            }
                        }
                        SsaRvalue::Use(_) | SsaRvalue::Tuple(_) => {}
                    }
                    // Destination type matches the computed type.
                    if let SsaStmt::Assign { dst, .. } = stmt {
                        let got = rvalue_ty(&cx, rv);
                        let want = func
                            .values
                            .get(dst.0 as usize)
                            .cloned()
                            .unwrap_or(Ty::Error);
                        if !ty_compat(&want, &got) {
                            err(
                                &func.name,
                                bb,
                                format!("assign %{}: expected {want}, computed {got}", dst.0),
                                out,
                            );
                        }
                    }
                }
                SsaStmt::Call {
                    dst, target, args, ..
                } => {
                    for a in args {
                        cx.check_operand(a, "call argument", bb, out);
                    }
                    if let SsaCallTarget::Value(o) = target {
                        cx.check_operand(o, "call target", bb, out);
                    }
                    if let SsaCallTarget::Fn(name) = target {
                        if let Some(callee) = prog.funcs.get(name) {
                            if args.len() != callee.params.len() {
                                err(
                                    &func.name,
                                    bb,
                                    format!(
                                        "`{name}` takes {} arguments, {} given",
                                        callee.params.len(),
                                        args.len()
                                    ),
                                    out,
                                );
                            }
                            let unit = callee.ret == Ty::Unit;
                            if unit && dst.is_some() {
                                err(
                                    &func.name,
                                    bb,
                                    format!("`{name}` returns () but call defines a value"),
                                    out,
                                );
                            }
                            if !unit && dst.is_none() {
                                err(
                                    &func.name,
                                    bb,
                                    format!("`{name}` returns a value but call discards it"),
                                    out,
                                );
                            }
                        }
                    }
                }
                SsaStmt::Print { values, .. } => {
                    for v in values {
                        cx.check_operand(v, "println", bb, out);
                    }
                }
                SsaStmt::SetField {
                    dst: _,
                    base,
                    field,
                    value,
                    ..
                } => {
                    cx.check_operand(base, "setfield base", bb, out);
                    cx.check_operand(value, "setfield value", bb, out);
                    check_field_base(&cx, base, *field, bb, out);
                }
            }
        }
        match &block.term {
            SsaTerm::Goto(_) => {}
            SsaTerm::Branch { cond, .. } => {
                cx.check_operand(cond, "branch condition", bb, out);
                if !(matches!(cx.operand_ty(cond), Ty::Bool | Ty::Error)) {
                    err(&func.name, bb, "branch on non-bool".to_string(), out);
                }
            }
            SsaTerm::Return(v) => match (v, &func.ret) {
                (None, Ty::Unit) | (None, Ty::Error) => {}
                (None, ret) => {
                    err(
                        &func.name,
                        bb,
                        format!("returning () from function returning {ret}"),
                        out,
                    );
                }
                (Some(o), ret) => {
                    cx.check_operand(o, "return value", bb, out);
                    if !ty_compat(ret, &cx.operand_ty(o)) {
                        err(
                            &func.name,
                            bb,
                            format!("return type mismatch: expected {ret}"),
                            out,
                        );
                    }
                }
            },
            SsaTerm::Trap { .. } => {}
            SsaTerm::Unreachable => {}
        }
    }
}

fn successors_of_term(term: &SsaTerm) -> Vec<BlockId> {
    match term {
        SsaTerm::Goto(t) => vec![*t],
        SsaTerm::Branch {
            then_bb, else_bb, ..
        } => vec![*then_bb, *else_bb],
        _ => Vec::new(),
    }
}

fn ty_compat(want: &Ty, got: &Ty) -> bool {
    if want == got || matches!(want, Ty::Error) || matches!(got, Ty::Error) {
        return true;
    }
    // String literals (`Str`) inhabit `&str` bindings and arguments.
    if matches!(got, Ty::Str) {
        if let Ty::Ref { inner, .. } = want {
            if matches!(**inner, Ty::Str) {
                return true;
            }
        }
    }
    false
}

fn is_enum_or_error(ty: &Ty) -> bool {
    matches!(
        peel(ty),
        Ty::Adt {
            kind: lobster_types::AdtKind::Enum,
            ..
        } | Ty::Error
    )
}

fn is_slice_like(ty: &Ty) -> bool {
    matches!(peel(ty), Ty::Slice(_) | Ty::Array(_, _) | Ty::Error)
}

fn is_int_or_error(ty: &Ty) -> bool {
    matches!(peel(ty), Ty::Int(_) | Ty::Error)
}

fn check_field_base(
    cx: &FuncCx<'_>,
    base: &SsaOperand,
    field: usize,
    block: Option<u32>,
    out: &mut Vec<VerifyError>,
) {
    let ty = cx.operand_ty(base);
    match peel(&ty) {
        Ty::Tuple(items) => {
            if field >= items.len() {
                err(
                    &cx.func.name,
                    block,
                    format!("field {field} out of range"),
                    out,
                );
            }
        }
        Ty::Adt {
            kind: lobster_types::AdtKind::Struct,
            name,
            ..
        } => {
            if let Some(layout) = cx.prog.structs.get(name) {
                if field >= layout.len() {
                    err(
                        &cx.func.name,
                        block,
                        format!("field {field} out of range for `{name}`"),
                        out,
                    );
                }
            }
        }
        Ty::Error => {}
        other => {
            err(
                &cx.func.name,
                block,
                format!("cannot read a field of `{other}`"),
                out,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ssa_of(text: &str) -> SsaProgram {
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
        build(&lobster_mir::lower(&hir))
    }

    fn ssa_main(text: &str) -> SsaFunc {
        let prog = ssa_of(text);
        verify(&prog).expect("ssa verifies");
        prog.funcs.get("main").cloned().expect("main exists")
    }

    #[test]
    fn if_join_creates_phi() {
        let prog = ssa_of(
            "fn f(c: bool) -> u32 {\n    let x = if c {\n        1u32\n    } else {\n        2u32\n    };\n    x\n}\n",
        );
        verify(&prog).expect("ssa verifies");
        let text = dump(&prog);
        assert!(text.contains("phi"), "{text}");
    }

    #[test]
    fn while_header_has_phi() {
        let prog = ssa_of(
            "fn f(n: u32) -> u32 {\n    let mut i = 0u32;\n    while i < n {\n        i += 1;\n    }\n    i\n}\n",
        );
        verify(&prog).expect("ssa verifies");
        assert!(dump(&prog).contains("phi"));
    }

    #[test]
    fn fib_builds_and_verifies() {
        let prog = ssa_of(
            "fn fib(n: u64) -> u64 {\n    if n < 2 {\n        return n;\n    }\n\n    fib(n - 1) + fib(n - 2)\n}\n",
        );
        verify(&prog).expect("ssa verifies");
        assert!(dump(&prog).contains("call"));
    }

    #[test]
    fn match_end_has_phi() {
        let prog = ssa_of(
            "enum Dir { North, South, }\nfn f(d: Dir) -> u32 {\n    match d {\n        North => 1u32,\n        South => 2u32,\n    }\n}\n",
        );
        verify(&prog).expect("ssa verifies");
        assert!(dump(&prog).contains("discr"));
    }

    #[test]
    fn all_examples_verify() {
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
            let prog = ssa_of(&text);
            verify(&prog).unwrap_or_else(|e| panic!("{file}: {e:?}"));
        }
    }

    #[test]
    fn dump_shows_phis_and_values() {
        let f = ssa_main("fn main() {\n    let mut x = 1u32;\n    x += 2u32;\n}\n");
        let prog = SsaProgram {
            funcs: HashMap::from([("main".to_string(), f)]),
            structs: HashMap::new(),
            enums: HashMap::new(),
        };
        let text = dump(&prog);
        assert!(text.contains("values"), "{text}");
    }

    #[test]
    fn destruct_keeps_functions_and_materializes_phis() {
        let ssa = ssa_of(
            "fn f(c: bool) -> u32 {\n    let x = if c {\n        1u32\n    } else {\n        2u32\n    };\n    x\n}\nfn main() {\n    println(f(true));\n}\n",
        );
        verify(&ssa).expect("ssa verifies");
        let mir = destruct(&ssa);
        assert!(mir.funcs.contains_key("f"));
        assert!(mir.funcs.contains_key("main"));
        let ssa_blocks: usize = ssa.funcs.values().map(|f| f.blocks.len()).sum();
        let mir_blocks: usize = mir.funcs.values().map(|f| f.blocks.len()).sum();
        assert!(mir_blocks >= ssa_blocks, "copies add blocks, never remove");
    }

    #[test]
    fn verifier_rejects_phi_arm_mismatch() {
        let mut prog = ssa_of("fn main() {\n    println(1u32);\n}\n");
        let f = prog.funcs.get_mut("main").unwrap();
        let dst = SsaValue(f.values.len() as u32);
        f.values.push(Ty::Int(IntTy::U32));
        f.value_names.push("bogus".to_string());
        f.blocks[0].phis.push(Phi {
            dst,
            ty: Ty::Int(IntTy::U32),
            origin: Local(0),
            arms: Vec::new(),
            span: None,
        });
        let errs = verify(&prog).unwrap_err();
        assert!(!errs.is_empty(), "expected phi errors");
    }

    #[test]
    fn verifier_rejects_undefined_use() {
        let mut prog = ssa_of("fn main() {\n    println(1u32);\n}\n");
        let f = prog.funcs.get_mut("main").unwrap();
        // Reference a value id that was never defined.
        let ghost = SsaValue(f.values.len() as u32 + 40);
        let span = f.blocks[0].stmts.first().map_or_else(
            || panic!("expected a stmt for span"),
            |s| match s {
                SsaStmt::Print { span, .. } => *span,
                SsaStmt::Assign { span, .. } => *span,
                SsaStmt::Call { span, .. } => *span,
                SsaStmt::SetField { span, .. } => *span,
            },
        );
        f.blocks[0].stmts.push(SsaStmt::Print {
            values: vec![SsaOperand::Value(ghost)],
            span,
        });
        let errs = verify(&prog).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.message.contains("undefined") || e.message.contains("unknown")),
            "{errs:?}"
        );
    }

    #[test]
    fn verifier_rejects_double_def() {
        let mut prog = ssa_of("fn main() {\n    println(1u32);\n}\n");
        let f = prog.funcs.get_mut("main").unwrap();
        let existing = f.params.first().copied().or_else(|| {
            f.blocks
                .iter()
                .flat_map(|b| {
                    b.stmts.iter().filter_map(|s| match s {
                        SsaStmt::Assign { dst, .. } => Some(*dst),
                        _ => None,
                    })
                })
                .next()
        });
        if let Some(dup) = existing {
            let span = f.blocks[0].stmts.first().map_or_else(
                || panic!("expected a stmt for span"),
                |s| match s {
                    SsaStmt::Print { span, .. } => *span,
                    SsaStmt::Assign { span, .. } => *span,
                    SsaStmt::Call { span, .. } => *span,
                    SsaStmt::SetField { span, .. } => *span,
                },
            );
            f.blocks[0].stmts.push(SsaStmt::Assign {
                dst: dup,
                rv: SsaRvalue::Use(SsaOperand::Const(Const_::Unit)),
                span,
            });
            let errs = verify(&prog).unwrap_err();
            assert!(errs.iter().any(|e| e.message.contains("twice")), "{errs:?}");
        }
    }

    #[test]
    fn verifier_rejects_branch_on_int() {
        let mut prog = ssa_of(
            "fn f(c: bool) -> u32 {\n    if c {\n        1u32\n    } else {\n        2u32\n    }\n}\n",
        );
        let f = prog.funcs.get_mut("f").unwrap();
        // Find a branch and swap its condition to an integer constant.
        for b in &mut f.blocks {
            if let SsaTerm::Branch { cond, .. } = &mut b.term {
                *cond = SsaOperand::Const(Const_::Int(1, IntTy::U32));
            }
        }
        let errs = verify(&prog).unwrap_err();
        assert!(
            errs.iter().any(|e| e.message.contains("non-bool")),
            "{errs:?}"
        );
    }
}
