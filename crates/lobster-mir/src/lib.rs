//! Lobster MIR: control-flow graphs with basic blocks over typed HIR.
//!
//! MIR is the second Commit 04 stage. It lowers [`HirProgram`] to
//! [`MirProgram`]: per-function locals plus [`MirBlock`]s ending in a
//! [`Terminator`]. Structured control flow (`if`/`while`/`loop`/`for`/
//! `match`/`break`/`continue`/`return`) becomes explicit branches; every
//! trapping operation (`/`, `%`, `as char`) keeps its source [`Span`] so
//! the interpreter reports failures precisely.
//!
//! Assumptions (all guaranteed by the checker; violations fail closed with
//! [`TrapKind`] blocks instead of panics):
//! - types check; [`lobster_types::Ty::Error`] nodes become unreachable traps;
//! - matches are exhaustive, so the final `else` is an unreachable trap;
//! - `&` / `*` lower faithfully but trap at runtime (memory model deferred).

use lobster_ast::{BinOp, Literal, UnOp};
use lobster_hir::{HirArm, HirBlock, HirExpr, HirExprKind, HirFunc, HirPat, HirProgram, HirStmt};
use lobster_source::Span;
use lobster_types::{FloatTy, IntTy, Ty};
use std::collections::HashMap;
use std::fmt::Write as _;

/// A MIR local (frame slot).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Local(pub u32);

/// A MIR basic block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlockId(pub u32);

/// A lowered program: functions plus ADT layouts.
#[derive(Debug, Clone)]
pub struct MirProgram {
    /// Functions by name.
    pub funcs: HashMap<String, MirFunc>,
    /// Struct layouts: field names in declaration order.
    pub structs: HashMap<String, Vec<String>>,
    /// Enum layouts: variant names in declaration order.
    pub enums: HashMap<String, Vec<String>>,
}

/// A lowered function.
#[derive(Debug, Clone)]
pub struct MirFunc {
    /// Function name.
    pub name: String,
    /// Parameter locals in order.
    pub params: Vec<Local>,
    /// Declared return type.
    pub ret: Ty,
    /// Type of every local by index.
    pub locals: Vec<Ty>,
    /// Debug names of locals (parallel to `locals`).
    pub local_names: Vec<String>,
    /// Blocks by index; execution starts at `entry`.
    pub blocks: Vec<MirBlock>,
    /// First block.
    pub entry: BlockId,
}

/// One basic block: straight-line statements plus a terminator.
#[derive(Debug, Clone)]
pub struct MirBlock {
    /// Statements in order.
    pub stmts: Vec<MirStmt>,
    /// How control leaves the block.
    pub term: Terminator,
}

/// Statement inside a block.
#[derive(Debug, Clone)]
pub enum MirStmt {
    /// `dst = rv`.
    Assign {
        /// Destination local.
        dst: Local,
        /// Computed value.
        rv: Rvalue,
        /// Source range.
        span: Span,
    },
    /// `dst = target(args)`. `dst` is `None` for `()` results.
    Call {
        /// Destination local, if the result is used.
        dst: Option<Local>,
        /// What is called.
        target: CallTarget,
        /// Arguments.
        args: Vec<Operand>,
        /// Source range.
        span: Span,
    },
    /// `println(v0, v1, ...)`.
    Print {
        /// Printed values.
        values: Vec<Operand>,
        /// Source range.
        span: Span,
    },
    /// `base.field = value` (struct fields and tuple positions).
    SetField {
        /// Base local holding the aggregate.
        base: Local,
        /// Field index in the layout.
        field: usize,
        /// New field value.
        value: Operand,
        /// Source range.
        span: Span,
    },
}

/// Call target.
#[derive(Debug, Clone)]
pub enum CallTarget {
    /// Named function.
    Fn(String),
    /// First-class function value (a `FuncRef` operand).
    Value(Operand),
}

/// How control leaves a block.
#[derive(Debug, Clone)]
pub enum Terminator {
    /// Unconditional jump.
    Goto(BlockId),
    /// `if cond { then } else { els }`.
    Branch {
        /// Condition operand (`bool`).
        cond: Operand,
        /// True successor.
        then_bb: BlockId,
        /// False successor.
        else_bb: BlockId,
    },
    /// `return [value]`.
    Return(Option<Operand>),
    /// Deterministic trap with a source span (div-by-zero, bad `as`,
    /// unsupported `&`/`*`, exhaustiveness backstop).
    Trap {
        /// Trap reason.
        kind: TrapKind,
        /// Source range.
        span: Span,
    },
    /// Placeholder before the builder seals a block.
    Unreachable,
}

/// Why a [`Terminator::Trap`] fires.
#[derive(Debug, Clone)]
pub enum TrapKind {
    /// Should-be-unreachable code (poisoned `Error` node, empty match,
    /// non-exhaustive backstop the checker ruled out).
    Unreachable(String),
    /// `&`, `&mut`, or `*` executed: the memory model is deferred.
    RefOp(String),
    /// Generic function called: monomorphization is deferred.
    Generic(String),
    /// Value called that is not a function.
    BadCallee(String),
    /// Local read before any assignment.
    Uninit(String),
}

/// An operand: a local or a constant.
#[derive(Debug, Clone)]
pub enum Operand {
    /// Frame slot.
    Local(Local),
    /// Inline constant.
    Const(Const_),
}

/// A constant value.
#[derive(Debug, Clone)]
pub enum Const_ {
    /// Integer with its type.
    Int(i128, IntTy),
    /// Float with its type (stored as `f64`; narrowed for `f32`).
    Float(f64, FloatTy),
    /// `true` / `false`.
    Bool(bool),
    /// Character.
    Char(char),
    /// String contents.
    Str(String),
    /// First-class function reference.
    FuncRef(String),
    /// `()`.
    Unit,
}

/// A computed right-hand side.
#[derive(Debug, Clone)]
pub enum Rvalue {
    /// Copy an operand.
    Use(Operand),
    /// `l op r`.
    Binary {
        /// Operator.
        op: BinOp,
        /// Left operand.
        l: Operand,
        /// Right operand.
        r: Operand,
    },
    /// `op v`.
    Unary {
        /// Operator.
        op: UnOp,
        /// Operand.
        v: Operand,
    },
    /// `v as to`.
    Cast {
        /// Target type.
        to: Ty,
        /// Converted operand.
        v: Operand,
    },
    /// Build a tuple.
    Tuple(Vec<Operand>),
    /// Read a struct field or tuple position.
    Field {
        /// Aggregate operand.
        base: Operand,
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
        payload: Vec<Operand>,
    },
    /// Discriminant of an enum value (variant index as `u32`).
    Discriminant(Operand),
    /// One payload element of an enum value.
    VariantPayload {
        /// Enum operand.
        base: Operand,
        /// Payload index.
        idx: usize,
    },
    /// Length of a slice/array value.
    SliceLen(Operand),
    /// One element of a slice/array value.
    SliceIndex {
        /// Aggregate operand.
        base: Operand,
        /// Element operand (`usize`-ish).
        idx: Operand,
    },
}

/// Lower HIR to MIR. HIR is assumed checked; poisoned nodes become
/// explicit [`TrapKind::Unreachable`] blocks instead of panics.
#[must_use]
pub fn lower(program: &HirProgram) -> MirProgram {
    let mut funcs = HashMap::new();
    let structs: HashMap<String, Vec<String>> = program
        .structs
        .iter()
        .map(|(n, fs)| (n.clone(), fs.iter().map(|(f, _)| f.clone()).collect()))
        .collect();
    let enums: HashMap<String, Vec<String>> = program
        .enums
        .iter()
        .map(|(n, vs)| (n.clone(), vs.iter().map(|(v, _)| v.clone()).collect()))
        .collect();
    for f in &program.funcs {
        // Generic bodies are placeholders; keep the signature so dumps
        // stay complete, but calls trap honestly at runtime.
        funcs.insert(
            f.name.clone(),
            FnLower::new(program, &structs, &enums).run(f),
        );
    }
    MirProgram {
        funcs,
        structs,
        enums,
    }
}

/// Render a MIR program in human-readable form (snapshots, `--dump-mir`).
#[must_use]
pub fn dump(program: &MirProgram) -> String {
    let mut out = String::new();
    let mut names: Vec<&String> = program.funcs.keys().collect();
    names.sort();
    for name in names {
        let f = &program.funcs[name];
        let _ = writeln!(
            out,
            "fn {}({} locals, {} blocks):",
            name,
            f.locals.len(),
            f.blocks.len()
        );
        for (i, b) in f.blocks.iter().enumerate() {
            let _ = writeln!(out, "  bb{i}:");
            for s in &b.stmts {
                let _ = writeln!(out, "    {}", dump_stmt(s, f));
            }
            let _ = writeln!(out, "    {}", dump_term(&b.term));
        }
    }
    out
}

fn dump_operand(o: &Operand, f: &MirFunc) -> String {
    match o {
        Operand::Local(l) => {
            let n = f.local_names.get(l.0 as usize).map_or("?", String::as_str);
            format!("%{}( {n} )", l.0)
        }
        Operand::Const(c) => match c {
            Const_::Int(v, t) => format!("{v}{t}"),
            Const_::Float(v, t) => format!("{v}{t}"),
            Const_::Bool(b) => format!("{b}"),
            Const_::Char(c) => format!("'{c}'"),
            Const_::Str(s) => format!("\"{s}\""),
            Const_::FuncRef(n) => format!("fn({n})"),
            Const_::Unit => "()".to_string(),
        },
    }
}

fn dump_stmt(s: &MirStmt, f: &MirFunc) -> String {
    match s {
        MirStmt::Assign { dst, rv, .. } => format!("%{} = {}", dst.0, dump_rv(rv, f)),
        MirStmt::Call {
            dst, target, args, ..
        } => {
            let t = match target {
                CallTarget::Fn(n) => format!("fn({n})"),
                CallTarget::Value(o) => dump_operand(o, f),
            };
            let a: Vec<String> = args.iter().map(|x| dump_operand(x, f)).collect();
            match dst {
                Some(d) => format!("%{} = call {}({})", d.0, t, a.join(", ")),
                None => format!("call {}({})", t, a.join(", ")),
            }
        }
        MirStmt::Print { values, .. } => {
            let a: Vec<String> = values.iter().map(|x| dump_operand(x, f)).collect();
            format!("println({})", a.join(", "))
        }
        MirStmt::SetField {
            base, field, value, ..
        } => {
            format!("%{}.f{field} = {}", base.0, dump_operand(value, f))
        }
    }
}

fn dump_rv(r: &Rvalue, f: &MirFunc) -> String {
    match r {
        Rvalue::Use(o) => format!("use {}", dump_operand(o, f)),
        Rvalue::Binary { op, l, r } => {
            format!("{op:?} {} {}", dump_operand(l, f), dump_operand(r, f))
        }
        Rvalue::Unary { op, v } => format!("{op:?} {}", dump_operand(v, f)),
        Rvalue::Cast { to, v } => format!("cast {} as {to}", dump_operand(v, f)),
        Rvalue::Tuple(es) => {
            let a: Vec<String> = es.iter().map(|x| dump_operand(x, f)).collect();
            format!("tuple({})", a.join(", "))
        }
        Rvalue::Field { base, field } => format!("{}.f{field}", dump_operand(base, f)),
        Rvalue::Enum {
            en,
            variant,
            payload,
        } => {
            let a: Vec<String> = payload.iter().map(|x| dump_operand(x, f)).collect();
            format!("{en}::{variant}({})", a.join(", "))
        }
        Rvalue::Discriminant(b) => format!("discr({})", dump_operand(b, f)),
        Rvalue::VariantPayload { base, idx } => {
            format!("payload{}({})", idx, dump_operand(base, f))
        }
        Rvalue::SliceLen(b) => format!("len({})", dump_operand(b, f)),
        Rvalue::SliceIndex { base, idx } => {
            format!("{}[{}]", dump_operand(base, f), dump_operand(idx, f))
        }
    }
}

fn dump_term(t: &Terminator) -> String {
    match t {
        Terminator::Goto(b) => format!("goto bb{}", b.0),
        Terminator::Branch {
            then_bb, else_bb, ..
        } => {
            format!("branch bb{} else bb{}", then_bb.0, else_bb.0)
        }
        Terminator::Return(None) => "return".to_string(),
        Terminator::Return(Some(_)) => "return %_".to_string(),
        Terminator::Trap { kind, .. } => format!("trap {kind:?}"),
        Terminator::Unreachable => "unreachable(?)".to_string(),
    }
}

struct LoopCtx {
    head: BlockId,
    exit: BlockId,
}

struct FnLower<'p> {
    hir: &'p HirProgram,
    structs: &'p HashMap<String, Vec<String>>,
    enums: &'p HashMap<String, Vec<String>>,
    locals: Vec<Ty>,
    names: Vec<String>,
    blocks: Vec<BlockData>,
    current: BlockId,
    loop_stack: Vec<LoopCtx>,
    /// Name stack for `Var` resolution (shadowing-safe): name → local.
    /// Block-scoped via checkpoint/truncate in `lower_block_value`.
    pending_names: Vec<(String, Local)>,
}

struct BlockData {
    stmts: Vec<MirStmt>,
    term: Option<Terminator>,
}

impl<'p> FnLower<'p> {
    fn new(
        hir: &'p HirProgram,
        structs: &'p HashMap<String, Vec<String>>,
        enums: &'p HashMap<String, Vec<String>>,
    ) -> Self {
        Self {
            hir,
            structs,
            enums,
            locals: Vec::new(),
            names: Vec::new(),
            blocks: Vec::new(),
            current: BlockId(0),
            loop_stack: Vec::new(),
            pending_names: Vec::new(),
        }
    }

    fn run(mut self, f: &HirFunc) -> MirFunc {
        let mut params = Vec::new();
        for (name, ty) in &f.params {
            let l = self.fresh_local(ty.clone(), format!("arg:{name}"));
            self.pending_names.push((name.clone(), l));
            params.push(l);
        }
        let entry = self.fresh_block();
        self.current = entry;
        if f.generic {
            let span = f.span;
            self.seal(
                entry,
                Terminator::Trap {
                    kind: TrapKind::Generic(format!("`{}` is generic", f.name)),
                    span,
                },
            );
        } else {
            let ret_val = self.lower_block_value(&f.body, Some(&f.ret));
            if !self.terminated(self.current) {
                let term = match ret_val {
                    Some(o) => Terminator::Return(Some(o)),
                    None => {
                        if f.ret == Ty::Unit {
                            Terminator::Return(None)
                        } else {
                            // Diverging body with no value (e.g. ends in
                            // `loop {}`): the value never flows.
                            Terminator::Return(None)
                        }
                    }
                };
                self.seal_current(term);
            }
        }
        // Seal any orphan blocks so the program is well-formed.
        for i in 0..self.blocks.len() {
            if self.blocks[i].term.is_none() {
                self.blocks[i].term = Some(Terminator::Trap {
                    kind: TrapKind::Unreachable("unterminated block".to_string()),
                    span: f.span,
                });
            }
        }
        let blocks = self
            .blocks
            .into_iter()
            .map(|b| MirBlock {
                stmts: b.stmts,
                term: b.term.unwrap_or(Terminator::Unreachable),
            })
            .collect();
        MirFunc {
            name: f.name.clone(),
            params,
            ret: f.ret.clone(),
            locals: self.locals,
            local_names: self.names,
            blocks,
            entry,
        }
    }

    fn fresh_local(&mut self, ty: Ty, name: String) -> Local {
        let id = Local(self.locals.len() as u32);
        self.locals.push(ty);
        self.names.push(name);
        id
    }

    fn fresh_block(&mut self) -> BlockId {
        let id = BlockId(self.blocks.len() as u32);
        self.blocks.push(BlockData {
            stmts: Vec::new(),
            term: None,
        });
        id
    }

    fn terminated(&self, b: BlockId) -> bool {
        self.blocks[b.0 as usize].term.is_some()
    }

    fn seal(&mut self, b: BlockId, term: Terminator) {
        if self.blocks[b.0 as usize].term.is_none() {
            self.blocks[b.0 as usize].term = Some(term);
        }
    }

    fn seal_current(&mut self, term: Terminator) {
        let c = self.current;
        self.seal(c, term);
    }

    fn emit(&mut self, stmt: MirStmt) {
        if self.terminated(self.current) {
            // Dead code after a diverging statement: park it in a fresh
            // orphan block so the live CFG stays well-formed.
            self.current = self.fresh_block();
        }
        self.blocks[self.current.0 as usize].stmts.push(stmt);
    }

    fn goto(&mut self, target: BlockId) {
        self.seal_current(Terminator::Goto(target));
        self.current = self.fresh_block();
    }

    fn const_unit(&self) -> Operand {
        Operand::Const(Const_::Unit)
    }

    fn temp(&mut self, ty: Ty, rv: Rvalue, span: Span, hint: &str) -> Operand {
        let dst = self.fresh_local(ty, hint.to_string());
        self.emit(MirStmt::Assign { dst, rv, span });
        Operand::Local(dst)
    }

    fn error_operand(&mut self, span: Span, msg: &str) -> Operand {
        let trap_bb = self.fresh_block();
        let join = self.fresh_block();
        self.seal_current(Terminator::Goto(trap_bb));
        self.blocks[trap_bb.0 as usize].term = Some(Terminator::Trap {
            kind: TrapKind::Unreachable(msg.to_string()),
            span,
        });
        self.current = join;
        // Poison value; unreachable at runtime because the only live
        // predecessor traps. The interpreter still needs *something*.
        Operand::Const(Const_::Unit)
    }

    // ----- blocks and statements -----

    /// Lower a block for its value. Returns the value operand, or `None`
    /// when the block diverges (every path terminated).
    fn lower_block_value(&mut self, block: &HirBlock, _expected: Option<&Ty>) -> Option<Operand> {
        let checkpoint = self.pending_names.len();
        let mut diverged = false;
        for stmt in &block.stmts {
            if self.lower_stmt(stmt) {
                diverged = true;
            }
        }
        let out = match &block.tail {
            Some(tail) => {
                if diverged {
                    // Value is dead; still lower for dump completeness into
                    // an orphan block.
                    let saved = self.current;
                    self.current = self.fresh_block();
                    let _ = self.lower_expr(tail);
                    self.current = saved;
                    None
                } else {
                    Some(self.lower_expr(tail))
                }
            }
            None => {
                if diverged {
                    None
                } else {
                    Some(self.const_unit())
                }
            }
        };
        self.pending_names.truncate(checkpoint);
        out
    }

    /// Lower a statement. Returns true when it diverges.
    fn lower_stmt(&mut self, stmt: &HirStmt) -> bool {
        match stmt {
            HirStmt::Let {
                pat,
                ty: _,
                init,
                span,
            } => {
                if let Some(init_e) = init {
                    let v = self.lower_expr(init_e);
                    self.bind_pat(pat, v, *span);
                } else {
                    // Missing initializer (E217 in checked code): fail closed.
                    let _ = self.error_operand(*span, "let without initializer");
                }
                false
            }
            HirStmt::Return { value, .. } => {
                let v = value.as_ref().map(|e| self.lower_expr(e));
                self.seal_current(Terminator::Return(v));
                self.current = self.fresh_block();
                true
            }
            HirStmt::While { cond, body, .. } => {
                let head = self.fresh_block();
                let torso = self.fresh_block();
                let exit = self.fresh_block();
                self.goto(head);
                self.current = head;
                let c = self.lower_expr(cond);
                self.seal_current(Terminator::Branch {
                    cond: c,
                    then_bb: torso,
                    else_bb: exit,
                });
                self.current = torso;
                self.loop_stack.push(LoopCtx { head, exit });
                let _ = self.lower_block_value(body, None);
                self.loop_stack.pop();
                if !self.terminated(self.current) {
                    self.seal_current(Terminator::Goto(head));
                }
                self.current = exit;
                false
            }
            HirStmt::Loop(body) => {
                let head = self.fresh_block();
                let exit = self.fresh_block();
                self.goto(head);
                self.current = head;
                self.loop_stack.push(LoopCtx { head, exit });
                let _ = self.lower_block_value(body, None);
                self.loop_stack.pop();
                if !self.terminated(self.current) {
                    self.seal_current(Terminator::Goto(head));
                }
                self.current = exit;
                // A `loop` without `break` never falls through, but `break`
                // targets `exit`, so conservatively not divergent.
                false
            }
            HirStmt::For {
                pat,
                item_ty: _,
                iter,
                body,
                span,
            } => {
                // `for pat in iter`: index loop over len. `iter` is an
                // array, a slice, or a reference to either (checker).
                let iter_v = self.lower_expr(iter);
                let len_ty = Ty::Int(IntTy::Usize);
                let len = self.temp(len_ty, Rvalue::SliceLen(iter_v.clone()), *span, "for:len");
                let idx_ty = Ty::Int(IntTy::Usize);
                let idx_local = self.fresh_local(idx_ty.clone(), "for:idx".to_string());
                self.emit(MirStmt::Assign {
                    dst: idx_local,
                    rv: Rvalue::Use(Operand::Const(Const_::Int(0, IntTy::Usize))),
                    span: *span,
                });
                let head = self.fresh_block();
                let torso = self.fresh_block();
                let latch = self.fresh_block();
                let exit = self.fresh_block();
                self.goto(head);
                self.current = head;
                let idx_op = Operand::Local(idx_local);
                let more = self.temp(
                    Ty::Bool,
                    Rvalue::Binary {
                        op: BinOp::Lt,
                        l: idx_op.clone(),
                        r: len,
                    },
                    *span,
                    "for:more",
                );
                self.seal_current(Terminator::Branch {
                    cond: more,
                    then_bb: torso,
                    else_bb: exit,
                });
                self.current = torso;
                let elem = self.temp(
                    pat_ty_of(pat),
                    Rvalue::SliceIndex {
                        base: iter_v,
                        idx: idx_op.clone(),
                    },
                    *span,
                    "for:elem",
                );
                // Torso: bind `pat`, run the body, fall through to the
                // latch. `continue` targets the latch so the increment is
                // never skipped; `break` targets exit.
                self.bind_pat(pat, elem, *span);
                self.loop_stack.push(LoopCtx { head: latch, exit });
                let _ = self.lower_block_value(body, None);
                self.loop_stack.pop();
                let body_end = self.current;
                if !self.terminated(body_end) {
                    self.seal(body_end, Terminator::Goto(latch));
                }
                // Latch: `idx += 1`, back to head.
                self.current = latch;
                let one = Operand::Const(Const_::Int(1, IntTy::Usize));
                self.emit(MirStmt::Assign {
                    dst: idx_local,
                    rv: Rvalue::Binary {
                        op: BinOp::Add,
                        l: idx_op,
                        r: one,
                    },
                    span: *span,
                });
                self.seal_current(Terminator::Goto(head));
                self.current = exit;
                false
            }
            HirStmt::Break(_) => {
                if let Some(ctx) = self.loop_stack.last() {
                    let exit = ctx.exit;
                    self.seal_current(Terminator::Goto(exit));
                } else {
                    let span = break_span(stmt);
                    self.seal_current(Terminator::Trap {
                        kind: TrapKind::Unreachable("break outside loop".to_string()),
                        span,
                    });
                }
                self.current = self.fresh_block();
                true
            }
            HirStmt::Continue(_) => {
                if let Some(ctx) = self.loop_stack.last() {
                    let head = ctx.head;
                    self.seal_current(Terminator::Goto(head));
                } else {
                    let span = break_span(stmt);
                    self.seal_current(Terminator::Trap {
                        kind: TrapKind::Unreachable("continue outside loop".to_string()),
                        span,
                    });
                }
                self.current = self.fresh_block();
                true
            }
            HirStmt::Expr(e) => {
                let _ = self.lower_expr(e);
                false
            }
        }
    }

    fn bind_pat(&mut self, pat: &HirPat, value: Operand, span: Span) {
        match pat {
            HirPat::Wildcard => {}
            HirPat::Ident(name, ty) => {
                let dst = self.fresh_local(ty.clone(), format!("let:{name}"));
                self.emit(MirStmt::Assign {
                    dst,
                    rv: Rvalue::Use(value),
                    span,
                });
                self.bind_name(name, dst);
            }
            HirPat::Lit(..) => {
                // `let 0 = x;` is rejected by sema; ignore the test here.
            }
            HirPat::Tuple(elems) => {
                for (i, sub) in elems.iter().enumerate() {
                    let t = pat_ty_of(sub);
                    let proj = self.temp(
                        t,
                        Rvalue::Field {
                            base: value.clone(),
                            field: i,
                        },
                        span,
                        "tup:proj",
                    );
                    self.bind_pat(sub, proj, span);
                }
            }
            HirPat::Variant {
                en: _,
                variant: _,
                args,
            } => {
                for (i, sub) in args.iter().enumerate() {
                    let t = pat_ty_of(sub);
                    let proj = self.temp(
                        t,
                        Rvalue::VariantPayload {
                            base: value.clone(),
                            idx: i,
                        },
                        span,
                        "var:payload",
                    );
                    self.bind_pat(sub, proj, span);
                }
            }
            HirPat::Struct { name, fields } => {
                for (fname, sub, _) in fields {
                    let idx = self.field_index(name, fname, span);
                    let t = pat_ty_of(sub);
                    let proj = self.temp(
                        t,
                        Rvalue::Field {
                            base: value.clone(),
                            field: idx,
                        },
                        span,
                        "struct:proj",
                    );
                    self.bind_pat(sub, proj, span);
                }
            }
        }
    }

    fn bind_name(&mut self, name: &str, local: Local) {
        self.name_stack_push(name, local);
    }

    // Name stack for Var resolution (shadowing-safe).
    fn name_stack_push(&mut self, name: &str, local: Local) {
        self.pending_names.push((name.to_string(), local));
    }

    fn resolve_var(&self, name: &str) -> Option<Local> {
        self.pending_names
            .iter()
            .rev()
            .find(|(n, _)| n == name)
            .map(|(_, l)| *l)
    }

    // ----- expressions -----

    fn lower_expr(&mut self, expr: &HirExpr) -> Operand {
        match &expr.kind {
            HirExprKind::Lit(lit) => self.lower_lit(lit, &expr.ty, expr.span),
            HirExprKind::Var(name) => {
                if let Some(l) = self.resolve_var(name) {
                    Operand::Local(l)
                } else {
                    // Function parameters live in the same name stack
                    // (bound at fn entry); anything else is poisoned input.
                    self.error_operand(expr.span, &format!("unresolved variable `{name}`"))
                }
            }
            HirExprKind::FnRef(name) => Operand::Const(Const_::FuncRef(name.clone())),
            HirExprKind::Variant { en, variant } => {
                let idx = self.variant_index(en, variant, expr.span);
                self.temp(
                    expr.ty.clone(),
                    Rvalue::Enum {
                        en: en.clone(),
                        variant: idx,
                        payload: Vec::new(),
                    },
                    expr.span,
                    "enum:unit",
                )
            }
            HirExprKind::BuiltinPrintln => Operand::Const(Const_::FuncRef("println".to_string())),
            HirExprKind::Binary { op, lhs, rhs } => {
                let l = self.lower_expr(lhs);
                let r = self.lower_expr(rhs);
                self.temp(
                    expr.ty.clone(),
                    Rvalue::Binary { op: *op, l, r },
                    expr.span,
                    "bin",
                )
            }
            HirExprKind::Unary { op, operand } => {
                let v = self.lower_expr(operand);
                self.temp(
                    expr.ty.clone(),
                    Rvalue::Unary { op: *op, v },
                    expr.span,
                    "un",
                )
            }
            HirExprKind::AddrOf { .. } => {
                self.error_operand(expr.span, "references are not executable yet")
            }
            HirExprKind::Deref(_) => {
                self.error_operand(expr.span, "dereference is not executable yet")
            }
            HirExprKind::Cast { expr: inner, to } => {
                let v = self.lower_expr(inner);
                self.temp(
                    expr.ty.clone(),
                    Rvalue::Cast { to: to.clone(), v },
                    expr.span,
                    "cast",
                )
            }
            HirExprKind::Assign { target, op, value } => {
                let v = self.lower_expr(value);
                self.lower_assign(target, *op, v, expr.span);
                self.const_unit()
            }
            HirExprKind::Call { callee, args } => {
                self.lower_call(callee, args, &expr.ty, expr.span)
            }
            HirExprKind::Field { recv, field } => {
                let base = self.lower_expr(recv);
                let idx = self.recv_field_index(recv, field, expr.span);
                self.temp(
                    expr.ty.clone(),
                    Rvalue::Field { base, field: idx },
                    expr.span,
                    "field",
                )
            }
            HirExprKind::If { cond, then, els } => {
                self.lower_if(cond, then, els.as_deref(), &expr.ty, expr.span)
            }
            HirExprKind::Match { scrut, arms } => {
                self.lower_match(scrut, arms, &expr.ty, expr.span)
            }
            HirExprKind::Block(b) => self
                .lower_block_value(b, None)
                .unwrap_or_else(|| self.const_unit()),
            HirExprKind::Tuple(elems) => {
                let vs: Vec<Operand> = elems.iter().map(|e| self.lower_expr(e)).collect();
                self.temp(expr.ty.clone(), Rvalue::Tuple(vs), expr.span, "tuple")
            }
            HirExprKind::Unit => self.const_unit(),
            HirExprKind::Error => self.error_operand(expr.span, "poisoned expression"),
        }
    }

    fn lower_lit(&mut self, lit: &Literal, ty: &Ty, _span: Span) -> Operand {
        match lit {
            Literal::Int { text, suffix } => {
                let (core, _) = lobster_types::split_number_suffix(text);
                let _ = suffix;
                let (radix, digits) = lobster_types::split_int_core(&core.replace('_', ""))
                    .unwrap_or((10, "0".to_string()));
                let raw = u128::from_str_radix(&digits, radix).unwrap_or(0);
                match ty {
                    Ty::Int(t) => Operand::Const(Const_::Int(normalize_int(raw, *t), *t)),
                    Ty::Float(t) => Operand::Const(Const_::Float(raw as f64, *t)),
                    _ => Operand::Const(Const_::Int(raw as i128, IntTy::I32)),
                }
            }
            Literal::Float { text, suffix } => {
                let core = suffix
                    .as_deref()
                    .and_then(|s| text.strip_suffix(s))
                    .unwrap_or(text);
                let v: f64 = core.replace('_', "").parse().unwrap_or(0.0);
                match ty {
                    Ty::Float(t) => Operand::Const(Const_::Float(v, *t)),
                    _ => Operand::Const(Const_::Float(v, FloatTy::F64)),
                }
            }
            Literal::Str(s) => Operand::Const(Const_::Str(s.clone())),
            Literal::Char(c) => Operand::Const(Const_::Char(*c)),
            Literal::Bool(b) => Operand::Const(Const_::Bool(*b)),
        }
    }

    fn lower_assign(&mut self, target: &HirExpr, op: Option<BinOp>, value: Operand, span: Span) {
        match &target.kind {
            HirExprKind::Var(name) => {
                if let Some(dst) = self.resolve_var(name) {
                    match op {
                        None => self.emit(MirStmt::Assign {
                            dst,
                            rv: Rvalue::Use(value),
                            span,
                        }),
                        Some(bin) => {
                            let cur = Operand::Local(dst);
                            let ty = self.locals[dst.0 as usize].clone();
                            let rv = Rvalue::Binary {
                                op: bin,
                                l: cur,
                                r: value,
                            };
                            self.emit(MirStmt::Assign { dst, rv, span });
                            let _ = ty;
                        }
                    }
                } else {
                    let _ = self.error_operand(span, "bad assignment target");
                }
            }
            HirExprKind::Field { recv, field } => {
                let base_op = self.lower_expr(recv);
                let idx = self.recv_field_index(recv, field, span);
                // Compound field assignment: read, combine, write.
                match op {
                    None => {
                        if let Operand::Local(base) = base_op {
                            self.emit(MirStmt::SetField {
                                base,
                                field: idx,
                                value,
                                span,
                            });
                        } else {
                            let _ = self.error_operand(span, "bad assignment target");
                        }
                    }
                    Some(bin) => {
                        let base_for_read = base_op.clone();
                        let cur = self.temp(
                            target.ty.clone(),
                            Rvalue::Field {
                                base: base_for_read,
                                field: idx,
                            },
                            span,
                            "field:cur",
                        );
                        let combined = self.temp(
                            target.ty.clone(),
                            Rvalue::Binary {
                                op: bin,
                                l: cur,
                                r: value,
                            },
                            span,
                            "field:op",
                        );
                        if let Operand::Local(base) = base_op {
                            self.emit(MirStmt::SetField {
                                base,
                                field: idx,
                                value: combined,
                                span,
                            });
                        } else {
                            let _ = self.error_operand(span, "bad assignment target");
                        }
                    }
                }
            }
            HirExprKind::Deref(_) | HirExprKind::Block(_) => {
                let _ = self.error_operand(span, "dereference assignment is not executable yet");
            }
            _ => {
                let _ = self.error_operand(span, "invalid assignment target");
            }
        }
    }

    fn lower_call(
        &mut self,
        callee: &HirExpr,
        args: &[HirExpr],
        ret_ty: &Ty,
        span: Span,
    ) -> Operand {
        let arg_ops: Vec<Operand> = args.iter().map(|a| self.lower_expr(a)).collect();
        // `println` fast path (any arity, unit result).
        if matches!(callee.kind, HirExprKind::BuiltinPrintln) {
            self.emit(MirStmt::Print {
                values: arg_ops,
                span,
            });
            return self.const_unit();
        }
        // `Variant(args)` construction through call syntax (`Some(x)`).
        if let HirExprKind::Variant { en, variant } = &callee.kind {
            let idx = self.variant_index(en, variant, span);
            return self.temp(
                ret_ty.clone(),
                Rvalue::Enum {
                    en: en.clone(),
                    variant: idx,
                    payload: arg_ops,
                },
                span,
                "enum:build",
            );
        }
        let target = match &callee.kind {
            HirExprKind::FnRef(name) => CallTarget::Fn(name.clone()),
            HirExprKind::Var(name) => {
                // Could be a first-class function value.
                if let Some(l) = self.resolve_var(name) {
                    CallTarget::Value(Operand::Local(l))
                } else {
                    CallTarget::Fn(name.clone())
                }
            }
            _ => {
                let v = self.lower_expr(callee);
                CallTarget::Value(v)
            }
        };
        if ret_ty == &Ty::Unit {
            self.emit(MirStmt::Call {
                dst: None,
                target,
                args: arg_ops,
                span,
            });
            self.const_unit()
        } else {
            let dst = self.fresh_local(ret_ty.clone(), "call:ret".to_string());
            self.emit(MirStmt::Call {
                dst: Some(dst),
                target,
                args: arg_ops,
                span,
            });
            Operand::Local(dst)
        }
    }

    fn lower_if(
        &mut self,
        cond: &HirExpr,
        then: &HirBlock,
        els: Option<&HirExpr>,
        ty: &Ty,
        span: Span,
    ) -> Operand {
        let c = self.lower_expr(cond);
        let result = self.fresh_local(ty.clone(), "if:res".to_string());
        let then_bb = self.fresh_block();
        let else_bb = self.fresh_block();
        let join = self.fresh_block();
        self.seal_current(Terminator::Branch {
            cond: c,
            then_bb,
            else_bb,
        });
        // Then.
        self.current = then_bb;
        let then_v = self.lower_block_value(then, None);
        if !self.terminated(self.current) {
            if let Some(v) = then_v {
                self.emit(MirStmt::Assign {
                    dst: result,
                    rv: Rvalue::Use(v),
                    span,
                });
            }
            self.seal_current(Terminator::Goto(join));
        }
        // Else.
        self.current = else_bb;
        match els {
            Some(e) => {
                let ev = self.lower_expr(e);
                // `ev` may itself have sealed blocks (nested if/match); only
                // assign when the tail block is still open.
                if !self.terminated(self.current) {
                    // When `e` is an `if`/`match` expression its value already
                    // flowed to its own result local; `ev` names it.
                    self.emit(MirStmt::Assign {
                        dst: result,
                        rv: Rvalue::Use(ev.clone()),
                        span,
                    });
                    let _ = ev;
                    self.seal_current(Terminator::Goto(join));
                }
            }
            None => {
                self.seal_current(Terminator::Goto(join));
            }
        }
        self.current = join;
        Operand::Local(result)
    }

    fn lower_match(&mut self, scrut: &HirExpr, arms: &[HirArm], ty: &Ty, span: Span) -> Operand {
        let scrut_v = self.lower_expr(scrut);
        let scrut_ty = scrut.ty.clone();
        let result = self.fresh_local(ty.clone(), "match:res".to_string());
        let end = self.fresh_block();
        if arms.is_empty() {
            self.seal_current(Terminator::Trap {
                kind: TrapKind::Unreachable("match has no arms".to_string()),
                span,
            });
            self.current = end;
            return Operand::Local(result);
        }
        // Sequential test chain: test_bb[i] -> body_bb[i] | test_bb[i+1] ...
        // -> final trap (checker proved exhaustive).
        let mut test_blocks = Vec::with_capacity(arms.len());
        for _ in arms {
            test_blocks.push(self.fresh_block());
        }
        let fail = self.fresh_block();
        self.seal_current(Terminator::Goto(test_blocks[0]));
        for (i, arm) in arms.iter().enumerate() {
            self.current = test_blocks[i];
            let next = if i + 1 < arms.len() {
                test_blocks[i + 1]
            } else {
                fail
            };
            let body_bb = self.fresh_block();
            self.lower_pat_test(&arm.pat, &scrut_v, &scrut_ty, body_bb, next, span);
            // Body.
            self.current = body_bb;
            let checkpoint = self.pending_names.len();
            self.bind_match_pat(&arm.pat, &scrut_v, &scrut_ty, span);
            let body_v = self.lower_expr(&arm.body);
            self.pending_names.truncate(checkpoint);
            if !self.terminated(self.current) {
                self.emit(MirStmt::Assign {
                    dst: result,
                    rv: Rvalue::Use(body_v),
                    span,
                });
                self.seal_current(Terminator::Goto(end));
            }
        }
        self.current = fail;
        self.seal_current(Terminator::Trap {
            kind: TrapKind::Unreachable("non-exhaustive match".to_string()),
            span,
        });
        self.current = end;
        Operand::Local(result)
    }

    /// Emit a test for `pat` against `scrut`: goto `hit` or `miss`.
    fn lower_pat_test(
        &mut self,
        pat: &HirPat,
        scrut: &Operand,
        scrut_ty: &Ty,
        hit: BlockId,
        miss: BlockId,
        span: Span,
    ) {
        match pat {
            HirPat::Wildcard | HirPat::Ident(..) => self.seal_current(Terminator::Goto(hit)),
            HirPat::Lit(lit, _) => {
                let want = self.const_of_lit(lit, scrut_ty, span);
                let eq = self.temp(
                    Ty::Bool,
                    Rvalue::Binary {
                        op: BinOp::Eq,
                        l: scrut.clone(),
                        r: want,
                    },
                    span,
                    "match:eq",
                );
                self.seal_current(Terminator::Branch {
                    cond: eq,
                    then_bb: hit,
                    else_bb: miss,
                });
            }
            HirPat::Tuple(_) => {
                // Element-wise tests: literal elements compare for
                // equality, bindings always match, nested patterns
                // recurse. Any failure jumps to `miss`.
                self.current = test_blocks_fallback(self, hit, miss, pat, scrut, scrut_ty, span);
            }
            HirPat::Variant { en, variant, args } => {
                let want_idx = self.variant_index(en, variant, span);
                let discr = self.temp(
                    Ty::Int(IntTy::U32),
                    Rvalue::Discriminant(scrut.clone()),
                    span,
                    "match:discr",
                );
                let eq = self.temp(
                    Ty::Bool,
                    Rvalue::Binary {
                        op: BinOp::Eq,
                        l: discr,
                        r: Operand::Const(Const_::Int(want_idx as i128, IntTy::U32)),
                    },
                    span,
                    "match:vartest",
                );
                if args
                    .iter()
                    .all(|a| matches!(a, HirPat::Wildcard | HirPat::Ident(..)))
                {
                    self.seal_current(Terminator::Branch {
                        cond: eq,
                        then_bb: hit,
                        else_bb: miss,
                    });
                } else {
                    // Payload sub-patterns need nested tests: on discriminant
                    // match, continue into a nested block; else miss.
                    let nested = self.fresh_block();
                    self.seal_current(Terminator::Branch {
                        cond: eq,
                        then_bb: nested,
                        else_bb: miss,
                    });
                    self.current = nested;
                    // Payload types for nested tests.
                    let payload_tys: Vec<Ty> = self
                        .hir
                        .enums
                        .get(en)
                        .and_then(|vs| vs.iter().find(|(n, _)| n == variant))
                        .map(|(_, tys)| tys.clone())
                        .unwrap_or_default();
                    let mut stage = nested;
                    for (i, sub) in args.iter().enumerate() {
                        let pt = payload_tys.get(i).cloned().unwrap_or(Ty::Error);
                        let proj = self.temp(
                            pt.clone(),
                            Rvalue::VariantPayload {
                                base: scrut.clone(),
                                idx: i,
                            },
                            span,
                            "match:payload",
                        );
                        let next_stage = self.fresh_block();
                        self.current = stage;
                        self.lower_pat_test(sub, &proj, &pt, next_stage, miss, span);
                        stage = next_stage;
                        self.current = stage;
                    }
                    self.seal_current(Terminator::Goto(hit));
                }
            }
            HirPat::Struct { name, fields } => {
                // Any struct pattern covers a struct scrutinee (checked);
                // nested literal tests still apply per field.
                if fields
                    .iter()
                    .all(|(_, p, _)| matches!(p, HirPat::Wildcard | HirPat::Ident(..)))
                {
                    self.seal_current(Terminator::Goto(hit));
                } else {
                    let mut stage = self.current;
                    for (fname, sub, _) in fields {
                        let idx = self.field_index(name, fname, span);
                        let ft = self
                            .hir
                            .structs
                            .get(name)
                            .and_then(|fs| fs.iter().find(|(n, _)| n == fname))
                            .map(|(_, t)| t.clone())
                            .unwrap_or(Ty::Error);
                        let proj = self.temp(
                            ft.clone(),
                            Rvalue::Field {
                                base: scrut.clone(),
                                field: idx,
                            },
                            span,
                            "match:field",
                        );
                        let next_stage = self.fresh_block();
                        self.current = stage;
                        self.lower_pat_test(sub, &proj, &ft, next_stage, miss, span);
                        stage = next_stage;
                        self.current = stage;
                    }
                    self.seal_current(Terminator::Goto(hit));
                }
            }
        }
    }

    fn const_of_lit(&mut self, lit: &Literal, ty: &Ty, _span: Span) -> Operand {
        match lit {
            Literal::Int { text, suffix } => {
                let (core, _) = lobster_types::split_number_suffix(text);
                let _ = suffix;
                let (radix, digits) = lobster_types::split_int_core(&core.replace('_', ""))
                    .unwrap_or((10, "0".to_string()));
                let raw = u128::from_str_radix(&digits, radix).unwrap_or(0);
                match ty {
                    Ty::Int(t) => Operand::Const(Const_::Int(normalize_int(raw, *t), *t)),
                    _ => Operand::Const(Const_::Int(raw as i128, IntTy::I32)),
                }
            }
            Literal::Float { text, suffix } => {
                let core = suffix
                    .as_deref()
                    .and_then(|s| text.strip_suffix(s))
                    .unwrap_or(text);
                let v: f64 = core.replace('_', "").parse().unwrap_or(0.0);
                match ty {
                    Ty::Float(t) => Operand::Const(Const_::Float(v, *t)),
                    _ => Operand::Const(Const_::Float(v, FloatTy::F64)),
                }
            }
            Literal::Str(s) => Operand::Const(Const_::Str(s.clone())),
            Literal::Char(c) => Operand::Const(Const_::Char(*c)),
            Literal::Bool(b) => Operand::Const(Const_::Bool(*b)),
        }
    }

    /// Bind pattern identifiers after the test succeeded.
    fn bind_match_pat(&mut self, pat: &HirPat, scrut: &Operand, scrut_ty: &Ty, span: Span) {
        match pat {
            HirPat::Wildcard | HirPat::Lit(..) => {}
            HirPat::Ident(name, ty) => {
                let dst = self.fresh_local(ty.clone(), format!("match:{name}"));
                self.emit(MirStmt::Assign {
                    dst,
                    rv: Rvalue::Use(scrut.clone()),
                    span,
                });
                self.name_stack_push(name, dst);
                let _ = scrut_ty;
            }
            HirPat::Tuple(elems) => {
                for (i, sub) in elems.iter().enumerate() {
                    let t = pat_ty_of(sub);
                    let proj = self.temp(
                        t,
                        Rvalue::Field {
                            base: scrut.clone(),
                            field: i,
                        },
                        span,
                        "match:bind",
                    );
                    let elem_ty = match scrut_ty {
                        Ty::Tuple(items) => items.get(i).cloned().unwrap_or(Ty::Error),
                        _ => Ty::Error,
                    };
                    self.bind_match_pat(sub, &proj, &elem_ty, span);
                }
            }
            HirPat::Variant { args, .. } => {
                for (i, sub) in args.iter().enumerate() {
                    let t = pat_ty_of(sub);
                    let proj = self.temp(
                        t.clone(),
                        Rvalue::VariantPayload {
                            base: scrut.clone(),
                            idx: i,
                        },
                        span,
                        "match:bind",
                    );
                    self.bind_match_pat(sub, &proj, &t, span);
                }
            }
            HirPat::Struct { name, fields } => {
                for (fname, sub, _) in fields {
                    let idx = self.field_index(name, fname, span);
                    let t = pat_ty_of(sub);
                    let proj = self.temp(
                        t.clone(),
                        Rvalue::Field {
                            base: scrut.clone(),
                            field: idx,
                        },
                        span,
                        "match:bind",
                    );
                    self.bind_match_pat(sub, &proj, &t, span);
                }
            }
        }
    }

    fn field_index(&self, struct_name: &str, field: &str, _span: Span) -> usize {
        // Tuple positions (`pair.0`) parse as field names "0", "1", ...
        if let Ok(i) = field.parse::<usize>() {
            return i;
        }
        self.structs
            .get(struct_name)
            .and_then(|fs| fs.iter().position(|f| f == field))
            .unwrap_or(0)
    }

    fn recv_field_index(&self, recv: &HirExpr, field: &str, span: Span) -> usize {
        if let Ok(i) = field.parse::<usize>() {
            return i;
        }
        // Resolve the struct name from the receiver's type.
        match &recv.ty {
            Ty::Adt { name, .. } => self
                .structs
                .get(name)
                .and_then(|fs| fs.iter().position(|f| f == field))
                .unwrap_or_else(|| {
                    let _ = span;
                    0
                }),
            _ => 0,
        }
    }

    fn variant_index(&self, en: &str, variant: &str, _span: Span) -> usize {
        self.enums
            .get(en)
            .and_then(|vs| vs.iter().position(|v| v == variant))
            .unwrap_or(0)
    }
}

// --- end builder ---

/// Type carried by a pattern (bindings) or [`Ty::Error`] for wildcards.
fn pat_ty_of(pat: &HirPat) -> Ty {
    match pat {
        HirPat::Wildcard => Ty::Error,
        HirPat::Ident(_, t) => t.clone(),
        HirPat::Lit(_, t) => t.clone(),
        HirPat::Variant { .. } => Ty::Error,
        HirPat::Tuple(_) => Ty::Error,
        HirPat::Struct { .. } => Ty::Error,
    }
}

fn break_span(stmt: &HirStmt) -> Span {
    match stmt {
        HirStmt::Break(s) => *s,
        HirStmt::Continue(s) => *s,
        _ => unreachable!("break_span on non-break"),
    }
}

/// Normalize a raw literal magnitude into the type's wrapped representation.
fn normalize_int(raw: u128, ty: IntTy) -> i128 {
    let bits = ty.bits();
    let mask = if bits >= 128 {
        u128::MAX
    } else {
        (1u128 << bits) - 1
    };
    let v = raw & mask;
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

/// Fallback tuple-pattern test used by [`FnLower::lower_pat_test`].
/// Handles the common shapes honestly: literal elements compare for
/// equality, bindings always match. Struct and variant tuples recurse
/// through equality on literals only.
fn test_blocks_fallback(
    builder: &mut FnLower<'_>,
    hit: BlockId,
    miss: BlockId,
    pat: &HirPat,
    scrut: &Operand,
    scrut_ty: &Ty,
    span: Span,
) -> BlockId {
    // Returns the block the caller should continue in (always `hit`-bound:
    // this helper seals the current block with the full test chain).
    let HirPat::Tuple(elems) = pat else {
        builder.seal_current(Terminator::Goto(hit));
        return hit;
    };
    let elem_tys: Vec<Ty> = match scrut_ty {
        Ty::Tuple(items) => items.clone(),
        _ => vec![Ty::Error; elems.len()],
    };
    // Chain: test element 0, then 1, ...; any literal mismatch -> miss.
    let mut stages: Vec<BlockId> = Vec::with_capacity(elems.len() + 1);
    for _ in 0..elems.len() {
        stages.push(builder.fresh_block());
    }
    stages.push(hit);
    builder.seal_current(Terminator::Goto(stages[0]));
    for (i, sub) in elems.iter().enumerate() {
        builder.current = stages[i];
        let next = stages[i + 1];
        let et = elem_tys.get(i).cloned().unwrap_or(Ty::Error);
        match sub {
            HirPat::Wildcard | HirPat::Ident(..) => {
                builder.seal_current(Terminator::Goto(next));
            }
            HirPat::Lit(lit, _) => {
                let proj = builder.temp(
                    et.clone(),
                    Rvalue::Field {
                        base: scrut.clone(),
                        field: i,
                    },
                    span,
                    "match:tup",
                );
                let want = builder.const_of_lit(lit, &et, span);
                let eq = builder.temp(
                    Ty::Bool,
                    Rvalue::Binary {
                        op: BinOp::Eq,
                        l: proj,
                        r: want,
                    },
                    span,
                    "match:tupeq",
                );
                builder.seal_current(Terminator::Branch {
                    cond: eq,
                    then_bb: next,
                    else_bb: miss,
                });
            }
            _ => {
                // Nested tuple/variant/struct inside a tuple: project then
                // recurse with the same miss target.
                let proj = builder.temp(
                    et.clone(),
                    Rvalue::Field {
                        base: scrut.clone(),
                        field: i,
                    },
                    span,
                    "match:tupnest",
                );
                let cont = builder.fresh_block();
                builder.lower_pat_test(sub, &proj, &et, cont, miss, span);
                builder.current = cont;
                builder.seal_current(Terminator::Goto(next));
            }
        }
    }
    hit
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mir_of(text: &str) -> MirProgram {
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
        lower(&hir)
    }

    #[test]
    fn fib_has_branches_and_call() {
        let prog = mir_of(
            "fn fib(n: u64) -> u64 {\n    if n < 2 {\n        return n;\n    }\n\n    fib(n - 1) + fib(n - 2)\n}\n",
        );
        let text = dump(&prog);
        assert!(text.contains("branch"), "{text}");
        assert!(text.contains("call"), "{text}");
        assert!(text.contains("return"), "{text}");
    }

    #[test]
    fn while_and_loop_shape() {
        let prog = mir_of(
            "fn f(n: u32) -> u32 {\n    let mut i = 0u32;\n    while i < n {\n        i += 1;\n    }\n    i\n}\n",
        );
        let f = &prog.funcs["f"];
        assert!(f.blocks.len() >= 3, "{}", dump(&prog));
    }

    #[test]
    fn match_lowes_to_dispatch() {
        let prog = mir_of(
            "enum Dir { North, South, }\nfn f(d: Dir) -> u32 {\n    match d {\n        North => 1u32,\n        South => 2u32,\n    }\n}\n",
        );
        let text = dump(&prog);
        assert!(text.contains("discr"), "{text}");
    }

    #[test]
    fn for_loop_has_len_and_index() {
        let prog = mir_of(
            "fn sum(items: &[u32]) -> u32 {\n    let mut total = 0u32;\n    for item in items {\n        total += item;\n    }\n    total\n}\n",
        );
        let text = dump(&prog);
        assert!(text.contains("len("), "{text}");
    }
}
