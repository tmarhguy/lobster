//! Lobster HIR: typed, desugared AST over checked programs.
//!
//! HIR is the first Commit 04 stage. It takes the parser AST plus the
//! outputs of resolution (`Resolved`) and checking (`Program`, `Tables`)
//! and produces a tree where:
//!
//! - every expression carries its checked type ([`Ty`]) and source [`Span`];
//! - paths are resolved to [`HirExprKind::Var`], [`HirExprKind::FnRef`],
//!   [`HirExprKind::Variant`], or the `println` builtin;
//! - short-circuit `&&` / `||` are desugared to `if` (MIR never sees them);
//! - patterns bind with their scrutinee types recorded.
//!
//! HIR adds no new diagnostics: the checker owns all errors. Lowering a
//! file that failed checking still produces a best-effort tree with
//! [`Ty::Error`] nodes so later stages can fail closed, but the CLI refuses
//! to execute such files.

use lobster_ast::{
    BinOp, Expr, ExprKind, File, FnItem, ItemKind, Literal, Pat, PatKind, Ty as SynTy, UnOp,
};
use lobster_diagnostics::Diagnostic;
use lobster_resolve::{span_pair, Resolved, ValueName};
use lobster_sema::{NodeId, Program, Tables};
use lobster_source::Span;
use lobster_types::Ty;
use std::collections::{HashMap, HashSet};

/// Identity of one HIR node, assigned pre-order during lowering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HirId(pub u32);

/// A checked, desugared program.
#[derive(Debug, Clone)]
pub struct HirProgram {
    /// Functions in source order.
    pub funcs: Vec<HirFunc>,
    /// Structs by name: fields with types (copied from [`Program`]).
    pub structs: HashMap<String, Vec<(String, Ty)>>,
    /// Enums by name: variants with payload types (copied from [`Program`]).
    pub enums: HashMap<String, Vec<(String, Vec<Ty>)>>,
}

/// A single function with a typed body.
#[derive(Debug, Clone)]
pub struct HirFunc {
    /// Function name.
    pub name: String,
    /// Parameter names with types.
    pub params: Vec<(String, Ty)>,
    /// Return type (`()` when omitted).
    pub ret: Ty,
    /// True when generic: body is a placeholder, calls are rejected by sema.
    pub generic: bool,
    /// Checked body (placeholder when generic).
    pub body: HirBlock,
    /// Span of the source item.
    pub span: Span,
}

/// A `{ stmt; ... tail? }` block.
#[derive(Debug, Clone)]
pub struct HirBlock {
    /// Statements in order.
    pub stmts: Vec<HirStmt>,
    /// Trailing expression without `;`, if any.
    pub tail: Option<Box<HirExpr>>,
    /// Span covering the braces.
    pub span: Span,
}

/// Statement.
#[derive(Debug, Clone)]
pub enum HirStmt {
    /// `let pat[: Ty] = init;` with the bound type recorded.
    Let {
        /// Bound pattern with types.
        pat: HirPat,
        /// Type of the binding (declared or inferred from init).
        ty: Ty,
        /// Initializer, if any.
        init: Option<HirExpr>,
        /// Span of the statement.
        span: Span,
    },
    /// `return [expr];`
    Return {
        /// Returned value, if any.
        value: Option<HirExpr>,
        /// Span of the statement.
        span: Span,
    },
    /// `while cond { ... }`
    While {
        /// Loop condition (always `bool` in checked code).
        cond: HirExpr,
        /// Loop body.
        body: HirBlock,
        /// Span of the statement.
        span: Span,
    },
    /// `loop { ... }`
    Loop(HirBlock),
    /// `for pat in iter { ... }` with the element type recorded.
    For {
        /// Loop variable pattern.
        pat: HirPat,
        /// Element type of the iteration.
        item_ty: Ty,
        /// Iterated expression.
        iter: HirExpr,
        /// Loop body.
        body: HirBlock,
        /// Span of the statement.
        span: Span,
    },
    /// `break;`
    Break(Span),
    /// `continue;`
    Continue(Span),
    /// Expression followed by `;`.
    Expr(HirExpr),
}

/// Expression with its checked type.
#[derive(Debug, Clone)]
pub struct HirExpr {
    /// Expression shape (desugared).
    pub kind: HirExprKind,
    /// Checked type.
    pub ty: Ty,
    /// Source range.
    pub span: Span,
    /// HIR node identity.
    pub id: HirId,
}

/// Desugared expression shapes.
#[derive(Debug, Clone)]
pub enum HirExprKind {
    /// Integer, float, string, char, or bool literal.
    Lit(Literal),
    /// Local binding or parameter.
    Var(String),
    /// Named function value.
    FnRef(String),
    /// Nullary enum variant value (`North`, `Dir::East` with no payload).
    Variant {
        /// Parent enum.
        en: String,
        /// Variant name.
        variant: String,
    },
    /// The `println` builtin (any arguments, `()` result).
    BuiltinPrintln,
    /// `lhs op rhs`. `&&` / `||` never appear: they lower to [`HirExprKind::If`].
    Binary {
        /// Operator.
        op: BinOp,
        /// Left operand.
        lhs: Box<HirExpr>,
        /// Right operand.
        rhs: Box<HirExpr>,
    },
    /// `-x`, `!x`.
    Unary {
        /// Operator.
        op: UnOp,
        /// Operand.
        operand: Box<HirExpr>,
    },
    /// `&x`, `&mut x`. Lowered faithfully; the interpreter traps on
    /// dereference (memory model is deferred past Commit 04).
    AddrOf {
        /// `true` for `&mut`.
        mutable: bool,
        /// Borrowed expression.
        operand: Box<HirExpr>,
    },
    /// `*p`. See [`HirExprKind::AddrOf`].
    Deref(Box<HirExpr>),
    /// `x as T`.
    Cast {
        /// Converted expression.
        expr: Box<HirExpr>,
        /// Target type.
        to: Ty,
    },
    /// `target = value`, `target op= value`.
    Assign {
        /// Assignment target.
        target: Box<HirExpr>,
        /// `None` for plain `=`; compound otherwise.
        op: Option<BinOp>,
        /// Assigned value.
        value: Box<HirExpr>,
    },
    /// `callee(args)`.
    Call {
        /// Called expression.
        callee: Box<HirExpr>,
        /// Argument expressions.
        args: Vec<HirExpr>,
    },
    /// `recv.field`.
    Field {
        /// Receiver expression.
        recv: Box<HirExpr>,
        /// Field name.
        field: String,
    },
    /// `if cond { ... } else { ... }`.
    If {
        /// Branch condition.
        cond: Box<HirExpr>,
        /// Then branch.
        then: HirBlock,
        /// Else branch (another `if` for `else if`).
        els: Option<Box<HirExpr>>,
    },
    /// `match scrut { pat => body, ... }`.
    Match {
        /// Scrutinee expression.
        scrut: Box<HirExpr>,
        /// Arms in source order.
        arms: Vec<HirArm>,
    },
    /// `{ ... }` block expression.
    Block(HirBlock),
    /// `(a, b)` tuple.
    Tuple(Vec<HirExpr>),
    /// `()`.
    Unit,
    /// Poisoned node from a file that failed checking. Later stages treat
    /// this as unreachable; the CLI never executes such files.
    Error,
}

/// One `match` arm with typed bindings.
#[derive(Debug, Clone)]
pub struct HirArm {
    /// Arm pattern.
    pub pat: HirPat,
    /// Arm body.
    pub body: HirExpr,
}

/// Pattern with recorded types.
#[derive(Debug, Clone)]
pub enum HirPat {
    /// `_`.
    Wildcard,
    /// `x` binding with its type.
    Ident(String, Ty),
    /// Literal pattern with its type.
    Lit(Literal, Ty),
    /// `Some(x)` / `Option::None` with payload patterns.
    Variant {
        /// Parent enum.
        en: String,
        /// Variant name.
        variant: String,
        /// Sub-patterns.
        args: Vec<HirPat>,
    },
    /// `(a, b)` tuple pattern.
    Tuple(Vec<HirPat>),
    /// `Point { x, y: q }` struct pattern.
    Struct {
        /// Struct name.
        name: String,
        /// Fields in source order: name, sub-pattern, name span.
        fields: Vec<(String, HirPat, Span)>,
    },
}

/// Lower a checked file to HIR.
///
/// Returns the program plus diagnostics (always empty today: the checker
/// owns errors; this signature reserves the channel for future use).
#[must_use]
pub fn lower(
    file: &File,
    resolved: &Resolved,
    program: &Program,
    tables: &Tables,
) -> (HirProgram, Vec<Diagnostic>) {
    Lowerer::new(resolved, program, tables).run(file)
}

struct Lowerer<'a> {
    resolved: &'a Resolved,
    program: &'a Program,
    tables: &'a Tables,
    next_id: u32,
    scopes: Vec<HashMap<String, Ty>>,
}

impl<'a> Lowerer<'a> {
    fn new(resolved: &'a Resolved, program: &'a Program, tables: &'a Tables) -> Self {
        Self {
            resolved,
            program,
            tables,
            next_id: 0,
            scopes: Vec::new(),
        }
    }

    fn run(mut self, file: &File) -> (HirProgram, Vec<Diagnostic>) {
        let mut funcs = Vec::new();
        for item in &file.items {
            if let ItemKind::Fn(f) = &item.node {
                funcs.push(self.lower_fn(f, item.span));
            }
        }
        (
            HirProgram {
                funcs,
                structs: self.program.structs.clone(),
                enums: self.program.enums.clone(),
            },
            Vec::new(),
        )
    }

    fn fresh(&mut self) -> HirId {
        let id = HirId(self.next_id);
        self.next_id += 1;
        id
    }

    fn ty_at(&self, span: Span) -> Ty {
        self.tables.type_at(span).unwrap_or(Ty::Error)
    }

    fn lower_syn_ty(&self, ty: &SynTy) -> Ty {
        self.resolved
            .lower_spanned(ty, &HashSet::new())
            .unwrap_or(Ty::Error)
    }

    fn lookup_local(&self, name: &str) -> Option<Ty> {
        self.scopes
            .iter()
            .rev()
            .find_map(|rib| rib.get(name).cloned())
    }

    fn bind(&mut self, name: &str, ty: Ty) {
        if let Some(rib) = self.scopes.last_mut() {
            rib.entry(name.to_string()).or_insert(ty);
        }
    }

    fn lower_fn(&mut self, f: &FnItem, span: Span) -> HirFunc {
        let sig = self.program.fns.get(&f.name);
        let params: Vec<(String, Ty)> = match sig {
            Some(s) => f
                .params
                .iter()
                .zip(s.params.iter())
                .map(|(p, t)| (p.name.clone(), t.clone()))
                .collect(),
            None => f
                .params
                .iter()
                .map(|p| (p.name.clone(), Ty::Error))
                .collect(),
        };
        let ret = sig.map_or(Ty::Error, |s| s.ret.clone());
        let generic = !f.generics.is_empty();
        self.scopes.push(HashMap::new());
        for (name, ty) in &params {
            self.bind(name, ty.clone());
        }
        let body = if generic {
            HirBlock {
                stmts: Vec::new(),
                tail: None,
                span,
            }
        } else {
            self.lower_block(&f.body, None)
        };
        self.scopes.pop();
        HirFunc {
            name: f.name.clone(),
            params,
            ret,
            generic,
            body,
            span,
        }
    }

    fn lower_block(&mut self, block: &lobster_ast::Block, _expected: Option<&Ty>) -> HirBlock {
        self.scopes.push(HashMap::new());
        let mut stmts = Vec::with_capacity(block.stmts.len());
        for stmt in &block.stmts {
            stmts.push(self.lower_stmt(stmt));
        }
        let tail = block.tail.as_ref().map(|t| Box::new(self.lower_expr(t)));
        self.scopes.pop();
        HirBlock {
            stmts,
            tail,
            span: block.span,
        }
    }

    fn lower_stmt(&mut self, stmt: &lobster_ast::Stmt) -> HirStmt {
        use lobster_ast::StmtKind as S;
        match &stmt.node {
            S::Let(l) => {
                let declared = l.ty.as_ref().map(|t| self.lower_syn_ty(t));
                let init = l.init.as_ref().map(|e| self.lower_expr(e));
                let init_ty = init.as_ref().map_or(Ty::Error, |e| e.ty.clone());
                let bound = declared.unwrap_or(init_ty);
                let pat = self.lower_pat(&l.pat, &bound);
                HirStmt::Let {
                    pat,
                    ty: bound,
                    init,
                    span: stmt.span,
                }
            }
            S::Return(e) => HirStmt::Return {
                value: e.as_ref().map(|v| self.lower_expr(v)),
                span: stmt.span,
            },
            S::While { cond, body } => HirStmt::While {
                cond: self.lower_expr(cond),
                body: self.lower_block(body, None),
                span: stmt.span,
            },
            S::Loop(body) => HirStmt::Loop(self.lower_block(body, None)),
            S::For { pat, iter, body } => {
                let iter_h = self.lower_expr(iter);
                let item_ty = peel_iter_item(&iter_h.ty);
                self.scopes.push(HashMap::new());
                let pat_h = self.lower_pat(pat, &item_ty);
                let body_h = self.lower_block(body, None);
                self.scopes.pop();
                HirStmt::For {
                    pat: pat_h,
                    item_ty,
                    iter: iter_h,
                    body: body_h,
                    span: stmt.span,
                }
            }
            S::Break => HirStmt::Break(stmt.span),
            S::Continue => HirStmt::Continue(stmt.span),
            S::Expr(e) => HirStmt::Expr(self.lower_expr(e)),
        }
    }

    fn mk(&mut self, kind: HirExprKind, ty: Ty, span: Span) -> HirExpr {
        let id = self.fresh();
        HirExpr { kind, ty, span, id }
    }

    fn lower_expr(&mut self, expr: &Expr) -> HirExpr {
        let ty = self.ty_at(expr.span);
        match &expr.node {
            ExprKind::Lit(lit) => self.mk(HirExprKind::Lit(lit.clone()), ty, expr.span),
            ExprKind::Path(segments) => self.lower_path(segments, expr.span, ty),
            ExprKind::Binary { op, lhs, rhs } => self.lower_binary(*op, lhs, rhs, expr.span, ty),
            ExprKind::Unary { op, operand } => {
                let inner = self.lower_expr(operand);
                self.mk(
                    HirExprKind::Unary {
                        op: *op,
                        operand: Box::new(inner),
                    },
                    ty,
                    expr.span,
                )
            }
            ExprKind::AddrOf { mutable, operand } => {
                let inner = self.lower_expr(operand);
                self.mk(
                    HirExprKind::AddrOf {
                        mutable: *mutable,
                        operand: Box::new(inner),
                    },
                    ty,
                    expr.span,
                )
            }
            ExprKind::Deref(operand) => {
                let inner = self.lower_expr(operand);
                self.mk(HirExprKind::Deref(Box::new(inner)), ty, expr.span)
            }
            ExprKind::Cast { expr, ty: to } => {
                let inner = self.lower_expr(expr);
                let target = self.lower_syn_ty(to);
                self.mk(
                    HirExprKind::Cast {
                        expr: Box::new(inner),
                        to: target,
                    },
                    ty,
                    expr.span,
                )
            }
            ExprKind::Assign { target, op, value } => {
                let t = self.lower_expr(target);
                let v = self.lower_expr(value);
                self.mk(
                    HirExprKind::Assign {
                        target: Box::new(t),
                        op: *op,
                        value: Box::new(v),
                    },
                    ty,
                    expr.span,
                )
            }
            ExprKind::Call { callee, args } => {
                let c = self.lower_expr(callee);
                let a = args.iter().map(|x| self.lower_expr(x)).collect();
                self.mk(
                    HirExprKind::Call {
                        callee: Box::new(c),
                        args: a,
                    },
                    ty,
                    expr.span,
                )
            }
            ExprKind::Field { recv, field } => {
                let r = self.lower_expr(recv);
                self.mk(
                    HirExprKind::Field {
                        recv: Box::new(r),
                        field: field.clone(),
                    },
                    ty,
                    expr.span,
                )
            }
            ExprKind::If { cond, then, els } => {
                let c = self.lower_expr(cond);
                let t = self.lower_block(then, None);
                let e = els.as_ref().map(|x| Box::new(self.lower_expr(x)));
                self.mk(
                    HirExprKind::If {
                        cond: Box::new(c),
                        then: t,
                        els: e,
                    },
                    ty,
                    expr.span,
                )
            }
            ExprKind::Match { scrut, arms } => {
                let s = self.lower_expr(scrut);
                let scrut_ty = s.ty.clone();
                let mut out = Vec::with_capacity(arms.len());
                for arm in arms {
                    self.scopes.push(HashMap::new());
                    let pat = self.lower_pat(&arm.pat, &scrut_ty);
                    let body = self.lower_expr(&arm.body);
                    self.scopes.pop();
                    out.push(HirArm { pat, body });
                }
                self.mk(
                    HirExprKind::Match {
                        scrut: Box::new(s),
                        arms: out,
                    },
                    ty,
                    expr.span,
                )
            }
            ExprKind::Block(b) => {
                let h = self.lower_block(b, None);
                self.mk(HirExprKind::Block(h), ty, expr.span)
            }
            ExprKind::Tuple(elems) => {
                let h = elems.iter().map(|e| self.lower_expr(e)).collect();
                self.mk(HirExprKind::Tuple(h), ty, expr.span)
            }
            ExprKind::Unit => self.mk(HirExprKind::Unit, ty, expr.span),
        }
    }

    fn lower_path(&mut self, segments: &[String], span: Span, ty: Ty) -> HirExpr {
        if segments.len() == 1 {
            let name = &segments[0];
            if self.lookup_local(name).is_some() {
                return self.mk(HirExprKind::Var(name.clone()), ty, span);
            }
        }
        match self.resolved.values.get(&span_pair(span)).cloned() {
            Some(ValueName::Local) => {
                let name = segments.last().cloned().unwrap_or_default();
                self.mk(HirExprKind::Var(name), ty, span)
            }
            Some(ValueName::Fn(name)) => self.mk(HirExprKind::FnRef(name), ty, span),
            Some(ValueName::Variant { en, variant }) => {
                self.mk(HirExprKind::Variant { en, variant }, ty, span)
            }
            Some(ValueName::BuiltinPrintln) => self.mk(HirExprKind::BuiltinPrintln, ty, span),
            Some(ValueName::Struct(_)) | None => {
                if ty == Ty::Error {
                    self.mk(HirExprKind::Error, ty, span)
                } else {
                    // Resolution failed but checking recovered; poison quietly.
                    self.mk(HirExprKind::Error, Ty::Error, span)
                }
            }
        }
    }

    /// Lower `&&` / `||` by desugaring; everything else stays binary.
    fn lower_binary(&mut self, op: BinOp, lhs: &Expr, rhs: &Expr, span: Span, ty: Ty) -> HirExpr {
        match op {
            BinOp::And | BinOp::Or => {
                let cond = self.lower_expr(lhs);
                let rhs_h = self.lower_expr(rhs);
                let rhs_ty = rhs_h.ty.clone();
                let rhs_span = rhs_h.span;
                let (then_b, else_e) = match op {
                    BinOp::And => {
                        let then_block = HirBlock {
                            stmts: Vec::new(),
                            tail: Some(Box::new(rhs_h)),
                            span: rhs_span,
                        };
                        let els = HirExpr {
                            kind: HirExprKind::Lit(lobster_ast::Literal::Bool(false)),
                            ty: Ty::Bool,
                            span,
                            id: self.fresh(),
                        };
                        (then_block, Some(Box::new(els)))
                    }
                    _ => {
                        let then_block = HirBlock {
                            stmts: Vec::new(),
                            tail: Some(Box::new(HirExpr {
                                kind: HirExprKind::Lit(lobster_ast::Literal::Bool(true)),
                                ty: Ty::Bool,
                                span,
                                id: self.fresh(),
                            })),
                            span,
                        };
                        let els = HirExpr {
                            kind: HirExprKind::Block(HirBlock {
                                stmts: Vec::new(),
                                tail: Some(Box::new(rhs_h)),
                                span: rhs_span,
                            }),
                            ty: rhs_ty,
                            span: rhs_span,
                            id: self.fresh(),
                        };
                        (then_block, Some(Box::new(els)))
                    }
                };
                self.mk(
                    HirExprKind::If {
                        cond: Box::new(cond),
                        then: then_b,
                        els: else_e,
                    },
                    ty,
                    span,
                )
            }
            _ => {
                let l = self.lower_expr(lhs);
                let r = self.lower_expr(rhs);
                self.mk(
                    HirExprKind::Binary {
                        op,
                        lhs: Box::new(l),
                        rhs: Box::new(r),
                    },
                    ty,
                    span,
                )
            }
        }
    }

    fn lower_pat(&mut self, pat: &Pat, scrut: &Ty) -> HirPat {
        match &pat.node {
            PatKind::Wildcard => HirPat::Wildcard,
            PatKind::Ident(name) => {
                self.bind(name, scrut.clone());
                HirPat::Ident(name.clone(), scrut.clone())
            }
            PatKind::Lit(lit) => HirPat::Lit(lit.clone(), scrut.clone()),
            PatKind::Tuple(elems) => match scrut {
                Ty::Tuple(items) if items.len() == elems.len() => HirPat::Tuple(
                    elems
                        .iter()
                        .zip(items.iter())
                        .map(|(p, t)| self.lower_pat(p, t))
                        .collect(),
                ),
                _ => HirPat::Tuple(
                    elems
                        .iter()
                        .map(|p| self.lower_pat(p, &Ty::Error))
                        .collect(),
                ),
            },
            PatKind::Variant { path, args } => {
                let (en, variant) = self.variant_head(path, pat.span, scrut);
                let payloads = self
                    .program
                    .enums
                    .get(&en)
                    .and_then(|vs| vs.iter().find(|(n, _)| *n == variant))
                    .map(|(_, tys)| tys.clone())
                    .unwrap_or_default();
                let mut out = Vec::with_capacity(args.len());
                for (i, sub) in args.iter().enumerate() {
                    let t = payloads.get(i).cloned().unwrap_or(Ty::Error);
                    out.push(self.lower_pat(sub, &t));
                }
                HirPat::Variant {
                    en,
                    variant,
                    args: out,
                }
            }
            PatKind::Struct { path, fields } => {
                let name = self.struct_head(path, pat.span, scrut);
                let decls = self.program.structs.get(&name).cloned().unwrap_or_default();
                let mut out = Vec::with_capacity(fields.len());
                for f in fields {
                    let t = decls
                        .iter()
                        .find(|(n, _)| *n == f.name)
                        .map(|(_, t)| t.clone())
                        .unwrap_or(Ty::Error);
                    out.push((f.name.clone(), self.lower_pat(&f.pat, &t), f.span));
                }
                HirPat::Struct { name, fields: out }
            }
        }
    }

    fn variant_head(&self, path: &[String], span: Span, scrut: &Ty) -> (String, String) {
        if let Some(ValueName::Variant { en, variant }) =
            self.resolved.values.get(&span_pair(span)).cloned()
        {
            return (en, variant);
        }
        if path.len() == 2 {
            return (path[0].clone(), path[1].clone());
        }
        let variant = path.last().cloned().unwrap_or_default();
        let en = match scrut {
            Ty::Adt { name, .. } => name.clone(),
            _ => String::new(),
        };
        (en, variant)
    }

    fn struct_head(&self, path: &[String], span: Span, scrut: &Ty) -> String {
        if let Some(name) = self
            .resolved
            .values
            .get(&span_pair(span))
            .cloned()
            .and_then(|v| match v {
                ValueName::Struct(n) => Some(n),
                ValueName::Variant { en, .. } => Some(en),
                _ => None,
            })
        {
            return name;
        }
        if let Some(first) = path.first() {
            return first.clone();
        }
        match scrut {
            Ty::Adt { name, .. } => name.clone(),
            _ => String::new(),
        }
    }
}

/// Element type for `for x in iter`: arrays, slices, and references to
/// either. Anything else yields [`Ty::Error`] (sema already reported it).
fn peel_iter_item(ty: &Ty) -> Ty {
    let mut t = ty;
    loop {
        match t {
            Ty::Array(elem, _) | Ty::Slice(elem) => return (**elem).clone(),
            Ty::Ref { inner, .. } => t = inner,
            Ty::Error => return Ty::Error,
            _ => return Ty::Error,
        }
    }
}

/// Node ids of an HIR tree in lowering order (for future passes).
#[allow(dead_code)]
fn _node_ids(_: &HirProgram) -> Vec<NodeId> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use lobster_lexer::lex;
    use lobster_parser::parse;
    use lobster_source::SourceManager;

    fn lower_text(text: &str) -> (HirProgram, Vec<Diagnostic>) {
        let mut sm = SourceManager::new();
        let id = sm.add_file("t.lobster", text);
        let lexed = lex(&sm, id, text);
        assert!(lexed.diagnostics.is_empty());
        let parsed = parse(id, &lexed.tokens);
        assert!(parsed.diagnostics.is_empty());
        let (resolved, rdiags) = lobster_resolve::resolve(&parsed.file);
        assert!(rdiags.is_empty(), "{rdiags:?}");
        let (program, tables, cdiags) = lobster_sema::check_program(&resolved, &parsed.file);
        assert!(cdiags.is_empty(), "{cdiags:?}");
        lower(&parsed.file, &resolved, &program, &tables)
    }

    #[test]
    fn fib_lowers_with_types() {
        let (prog, diags) = lower_text(
            "fn fib(n: u64) -> u64 {\n    if n < 2 {\n        return n;\n    }\n\n    fib(n - 1) + fib(n - 2)\n}\n",
        );
        assert!(diags.is_empty());
        assert_eq!(prog.funcs.len(), 1);
        let f = &prog.funcs[0];
        assert_eq!(f.params.len(), 1);
        assert!(matches!(f.ret, Ty::Int(_)));
    }

    #[test]
    fn and_or_desugar_to_if() {
        let (prog, _) = lower_text("fn f(a: bool, b: bool) -> bool { a && b }\n");
        let body = format!("{:?}", prog.funcs[0].body);
        assert!(body.contains("If"), "{body}");
        let (prog, _) = lower_text("fn f(a: bool, b: bool) -> bool { a || b }\n");
        let body = format!("{:?}", prog.funcs[0].body);
        assert!(body.contains("If"), "{body}");
    }

    #[test]
    fn variant_and_struct_pats_keep_types() {
        let (prog, _) = lower_text(
            "struct Point { x: f32, y: f32, }\nfn get_x(p: Point) -> f32 {\n    match p {\n        Point { x, y: _ } => x,\n    }\n}\n",
        );
        assert_eq!(prog.funcs.len(), 1);
        assert!(prog.structs.contains_key("Point"));
    }

    #[test]
    fn for_item_type_recorded() {
        let (prog, _) = lower_text(
            "fn sum(items: &[u32]) -> u32 {\n    let mut total = 0u32;\n    for item in items {\n        total += item;\n    }\n    total\n}\n",
        );
        let dump = format!("{:?}", prog.funcs[0].body);
        assert!(dump.contains("For"), "{dump}");
    }
}
