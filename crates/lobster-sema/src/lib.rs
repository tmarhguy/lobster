//! Lobster semantic analysis: the type checker.
//!
//! Bidirectional checking with no inference variables: types flow in from
//! annotations and signatures, and unsuffixed literals adopt their context
//! (`10` is `u64` when a `u64` is expected, `i32` otherwise). There are no
//! implicit conversions of any kind — not even widening.
//!
//! The entry points are [`check_file`] (diagnostics only, used by the CLI)
//! and [`check_program`] (plus [`Program`] and [`Tables`] for later stages).
//!
//! Checker diagnostic codes:
//!
//! ```text
//! E210 type mismatch
//! E211 wrong number of arguments
//! E212 illegal cast
//! E213 non-exhaustive match
//! E214 return mismatch
//! E215 invalid target (bad assignment target, misplaced break/continue)
//! E216 literal out of range
//! E217 unsupported (generics, missing initializer, non-literal array length)
//! E218 unknown variant or field
//! ```
//!
//! Deferred (documented, not silently accepted): definite assignment,
//! borrow checking, const evaluation, monomorphization, `unsafe` checking.

use lobster_ast::*;
use lobster_diagnostics::{Diagnostic, Label};
use lobster_resolve::{span_pair, Resolved, SpanPair, TypeError, ValueName};
use lobster_source::{SourceManager, Span};
use lobster_types::{int_literal_fits, neg_literal_fits, AdtKind, FloatTy, IntTy, Ty as SemTy};
use std::collections::{HashMap, HashSet};

/// Identity of one expression node, assigned pre-order during checking.
/// Stable across runs for the same file; consumed by later stages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeId(pub u32);

/// A checked function signature.
#[derive(Debug, Clone)]
pub struct CheckedFn {
    /// Function name.
    pub name: String,
    /// Parameter types in order.
    pub params: Vec<SemTy>,
    /// Return type (`()` when omitted).
    pub ret: SemTy,
    /// True when generic (body unchecked; calls are rejected).
    pub generic: bool,
}

/// A checked program: item signatures for later stages.
#[derive(Debug, Default)]
pub struct Program {
    /// Functions by name.
    pub fns: HashMap<String, CheckedFn>,
    /// Structs by name: fields with types.
    pub structs: HashMap<String, Vec<(String, SemTy)>>,
    /// Enums by name: variants with payload types.
    pub enums: HashMap<String, Vec<(String, Vec<SemTy>)>>,
}

/// Per-node results for later stages.
#[derive(Debug, Default)]
pub struct Tables {
    /// Type of every checked expression, by [`NodeId`].
    pub node_types: HashMap<NodeId, SemTy>,
    /// Type of every checked expression, by source span.
    /// Populated alongside `node_types`; consumed by HIR lowering
    /// (Commit 04) without needing the internal id assignment order.
    pub span_types: HashMap<SpanPair, SemTy>,
}

impl Tables {
    /// Type recorded for the expression at `span`, if any.
    #[must_use]
    pub fn type_at(&self, span: Span) -> Option<SemTy> {
        self.span_types.get(&span_pair(span)).cloned()
    }
}

/// Check a file, returning only diagnostics (CLI entry point).
#[must_use]
pub fn check_file(_sources: &SourceManager, resolved: &Resolved, file: &File) -> Vec<Diagnostic> {
    check_program(resolved, file).2
}

/// Check a file, returning the program, tables, and diagnostics.
#[must_use]
pub fn check_program(resolved: &Resolved, file: &File) -> (Program, Tables, Vec<Diagnostic>) {
    Checker::new(resolved).run(file)
}

struct Checker<'a> {
    resolved: &'a Resolved,
    diagnostics: Vec<Diagnostic>,
    program: Program,
    tables: Tables,
    next_id: u32,
    /// Node ids by span (idempotent recording).
    span_ids: HashMap<SpanPair, NodeId>,
    /// Local value scopes: name → type (params live in the outermost).
    scopes: Vec<HashMap<String, SemTy>>,
    /// Declared return type of the enclosing function, if any.
    ret: Option<SemTy>,
    /// Loop nesting depth (for break/continue placement).
    in_loop: u32,
    /// Generic parameters in scope (from the enclosing item).
    generics: HashSet<String>,
}

impl<'a> Checker<'a> {
    fn new(resolved: &'a Resolved) -> Self {
        Self {
            resolved,
            diagnostics: Vec::new(),
            program: Program::default(),
            tables: Tables::default(),
            next_id: 0,
            span_ids: HashMap::new(),
            scopes: Vec::new(),
            ret: None,
            in_loop: 0,
            generics: HashSet::new(),
        }
    }

    fn run(mut self, file: &File) -> (Program, Tables, Vec<Diagnostic>) {
        // Signatures first (order-independent, like resolution).
        for item in &file.items {
            self.collect_signature(item);
        }
        for item in &file.items {
            self.check_item(item);
        }
        (self.program, self.tables, self.diagnostics)
    }

    fn error(&mut self, code: &str, message: String, span: Span, detail: String) {
        self.diagnostics.push(Diagnostic::error(
            code,
            message,
            Label::primary(span, detail),
        ));
    }

    fn mismatch(&mut self, expected: &SemTy, found: &SemTy, span: Span, what: &str) {
        if matches!(expected, SemTy::Error) || matches!(found, SemTy::Error) || expected == found {
            return;
        }
        self.error(
            "E210",
            format!("type mismatch in {what}"),
            span,
            format!("expected {expected}, found {found}"),
        );
    }

    fn fresh_id(&mut self) -> NodeId {
        let id = NodeId(self.next_id);
        self.next_id += 1;
        id
    }

    fn record(&mut self, expr: &Expr, ty: SemTy) {
        // Idempotent per node: re-checks (literal adoption flips) overwrite
        // instead of allocating a second id for one node.
        let key = span_pair(expr.span);
        let id = match self.span_ids.get(&key) {
            Some(id) => *id,
            None => {
                let id = self.fresh_id();
                self.span_ids.insert(key, id);
                id
            }
        };
        self.tables.node_types.insert(id, ty.clone());
        self.tables.span_types.insert(key, ty);
    }

    // ----- signatures -----

    fn lower_sig_type(&mut self, ty: &Ty, what: &str) -> SemTy {
        match self.resolved.lower_spanned(ty, &self.generics) {
            Ok(t) => t,
            Err(TypeError::Generic) => {
                self.error(
                    "E217",
                    "generics are not supported yet".to_string(),
                    ty.span,
                    format!("{what} uses a generic item (monomorphization is deferred)"),
                );
                SemTy::Error
            }
            Err(TypeError::Unknown) => SemTy::Error,
        }
    }

    fn collect_signature(&mut self, item: &Item) {
        match &item.node {
            ItemKind::Fn(f) => {
                let generics: HashSet<String> = f.generics.iter().map(|g| g.name.clone()).collect();
                let generic = !f.generics.is_empty();
                let save = std::mem::take(&mut self.generics);
                self.generics = generics;
                let mut params = Vec::new();
                for p in &f.params {
                    params.push(self.lower_sig_type(&p.ty, "parameter"));
                }
                let ret = match &f.ret {
                    Some(t) => self.lower_sig_type(t, "return type"),
                    None => SemTy::Unit,
                };
                self.generics = save;
                self.program.fns.insert(
                    f.name.clone(),
                    CheckedFn {
                        name: f.name.clone(),
                        params,
                        ret,
                        generic,
                    },
                );
            }
            ItemKind::Struct(s) => {
                let save = std::mem::take(&mut self.generics);
                self.generics = s.generics.iter().map(|g| g.name.clone()).collect();
                let mut fields = Vec::new();
                for fl in &s.fields {
                    fields.push((fl.name.clone(), self.lower_sig_type(&fl.ty, "field")));
                }
                self.generics = save;
                self.program.structs.insert(s.name.clone(), fields);
            }
            ItemKind::Enum(e) => {
                let save = std::mem::take(&mut self.generics);
                self.generics = e.generics.iter().map(|g| g.name.clone()).collect();
                let mut variants = Vec::new();
                for v in &e.variants {
                    let mut payload = Vec::new();
                    for t in &v.payload {
                        payload.push(self.lower_sig_type(t, "variant payload"));
                    }
                    variants.push((v.name.clone(), payload));
                }
                self.generics = save;
                self.program.enums.insert(e.name.clone(), variants);
            }
            ItemKind::Import(_) => {}
        }
    }

    // ----- items -----

    fn check_item(&mut self, item: &Item) {
        match &item.node {
            ItemKind::Fn(f) => {
                if !f.generics.is_empty() {
                    // Bodies of generic functions are unchecked until
                    // monomorphization lands; uses are rejected at call sites.
                    return;
                }
                let sig = self.program.fns.get(&f.name).cloned().unwrap_or(CheckedFn {
                    name: f.name.clone(),
                    params: Vec::new(),
                    ret: SemTy::Unit,
                    generic: false,
                });
                self.generics = HashSet::new();
                self.ret = Some(sig.ret.clone());
                self.scopes.push(HashMap::new());
                for (p, t) in f.params.iter().zip(sig.params.iter()) {
                    self.scopes
                        .last_mut()
                        .unwrap()
                        .insert(p.name.clone(), t.clone());
                }
                // The body is checked unconstrained; a tail mismatch is a
                // return mismatch (E214), not a generic block error.
                let (body_ty, diverges) = self.check_block(&f.body, None);
                if !diverges
                    && body_ty != SemTy::Error
                    && sig.ret != SemTy::Error
                    && body_ty != sig.ret
                {
                    self.error(
                        "E214",
                        "return mismatch".to_string(),
                        f.body.span,
                        format!("expected {}, found {body_ty}", sig.ret),
                    );
                }
                self.scopes.pop();
                self.ret = None;
            }
            ItemKind::Struct(_) | ItemKind::Enum(_) | ItemKind::Import(_) => {}
        }
    }

    // ----- blocks and statements -----

    /// Check a block against an optional expected type.
    /// Returns the block type and whether it diverges.
    fn check_block(&mut self, block: &Block, expected: Option<&SemTy>) -> (SemTy, bool) {
        self.scopes.push(HashMap::new());
        let mut diverges = false;
        for stmt in &block.stmts {
            if self.check_stmt(stmt) {
                diverges = true;
            }
        }
        let (ty, tail_diverges) = match &block.tail {
            Some(tail) => self.check_expr(tail, expected),
            None => (SemTy::Unit, diverges),
        };
        self.scopes.pop();
        let diverges = tail_diverges || diverges;
        if !diverges {
            if let Some(want) = expected {
                // A diverging block satisfies any expected type.
                if ty != SemTy::Error && *want != SemTy::Error && ty != *want {
                    self.mismatch(want, &ty, block.span, "block");
                }
            }
        }
        (ty, diverges)
    }

    /// Check a statement. Returns true when it diverges.
    fn check_stmt(&mut self, stmt: &Stmt) -> bool {
        match &stmt.node {
            StmtKind::Let(l) => {
                let declared = l.ty.as_ref().map(|t| self.lower_sig_type(t, "let binding"));
                let init_ty = match &l.init {
                    Some(init) => {
                        let (t, _) = self.check_expr(init, declared.as_ref());
                        t
                    }
                    None => {
                        self.error(
                            "E217",
                            "missing initializer".to_string(),
                            stmt.span,
                            "type inference needs an initializer (deferred)".to_string(),
                        );
                        SemTy::Error
                    }
                };
                let bound = match declared {
                    Some(d) => {
                        if init_ty != SemTy::Error && d != SemTy::Error && l.init.is_some() {
                            self.mismatch(&d, &init_ty, stmt.span, "let binding");
                        }
                        if d != SemTy::Error {
                            d
                        } else {
                            init_ty.clone()
                        }
                    }
                    None => init_ty.clone(),
                };
                self.bind_pat(&l.pat, &bound);
                false
            }
            StmtKind::Return(e) => {
                match (e, self.ret.clone()) {
                    (Some(v), Some(want)) => {
                        let (t, _) = self.check_expr(v, Some(&want));
                        if t != SemTy::Error && want != SemTy::Error && t != want {
                            self.error(
                                "E214",
                                "return mismatch".to_string(),
                                stmt.span,
                                format!("expected {want}, found {t}"),
                            );
                        }
                    }
                    (None, Some(want)) => {
                        if want != SemTy::Unit && want != SemTy::Error {
                            self.error(
                                "E214",
                                "return mismatch".to_string(),
                                stmt.span,
                                format!("expected {want}, found ()"),
                            );
                        }
                    }
                    (_, None) => {}
                }
                true
            }
            StmtKind::While { cond, body } => {
                let (t, _) = self.check_expr(cond, Some(&SemTy::Bool));
                if t != SemTy::Bool && t != SemTy::Error {
                    self.mismatch(&SemTy::Bool, &t, cond.span, "while condition");
                }
                self.check_block(body, None);
                false
            }
            StmtKind::Loop(body) => {
                self.in_loop += 1;
                self.check_block(body, None);
                self.in_loop -= 1;
                // A `loop` without a value never falls through.
                true
            }
            StmtKind::For { pat, iter, body } => {
                let (t, _) = self.check_expr(iter, None);
                let item = self.iter_item_type(&t, iter.span);
                self.scopes.push(HashMap::new());
                self.bind_pat(pat, &item);
                self.in_loop += 1;
                self.check_block(body, None);
                self.in_loop -= 1;
                self.scopes.pop();
                false
            }
            StmtKind::Break | StmtKind::Continue => {
                if self.in_loop == 0 {
                    let word = if matches!(stmt.node, StmtKind::Break) {
                        "break"
                    } else {
                        "continue"
                    };
                    self.error(
                        "E215",
                        format!("{word} outside loop"),
                        stmt.span,
                        format!("{word} is only valid inside a loop body"),
                    );
                }
                true
            }
            StmtKind::Expr(e) => {
                let (_, d) = self.check_expr(e, None);
                // Block-like statement expressions need no tail discipline,
                // except bare `if` without `else`, which must yield `()`.
                if let ExprKind::If { then, els, .. } = &e.node {
                    if els.is_none() {
                        let (t, div) = self.check_block(then, None);
                        if !div && t != SemTy::Unit && t != SemTy::Error {
                            self.mismatch(&SemTy::Unit, &t, e.span, "if without else");
                        }
                    }
                }
                d
            }
        }
    }

    /// Element type for `for x in iter`: arrays, slices, and references
    /// to either. Anything else is E210.
    fn iter_item_type(&mut self, ty: &SemTy, span: Span) -> SemTy {
        let mut t = ty;
        loop {
            match t {
                SemTy::Array(elem, _) | SemTy::Slice(elem) => return (**elem).clone(),
                SemTy::Ref { inner, .. } => t = inner,
                SemTy::Error => return SemTy::Error,
                _ => {
                    self.error(
                        "E210",
                        "type mismatch in for loop".to_string(),
                        span,
                        format!("cannot iterate over {ty} (expected an array or slice)"),
                    );
                    return SemTy::Error;
                }
            }
        }
    }

    /// Bind a pattern against a scrutinee type, introducing bindings.
    fn bind_pat(&mut self, pat: &Pat, scrut: &SemTy) {
        if matches!(scrut, SemTy::Error) {
            // Still bind names as Error to suppress cascades.
            self.bind_pat_names(pat);
            return;
        }
        match &pat.node {
            PatKind::Wildcard => {}
            PatKind::Ident(name) => {
                self.insert_binding(name, pat.span, scrut.clone());
            }
            PatKind::Lit(lit) => {
                let (t, _) = self.check_literal(lit, Some(scrut), pat.span);
                if t != SemTy::Error && t != *scrut {
                    self.mismatch(scrut, &t, pat.span, "pattern");
                }
            }
            PatKind::Tuple(elems) => match scrut {
                SemTy::Tuple(items) if items.len() == elems.len() => {
                    for (p, t) in elems.iter().zip(items.iter()) {
                        self.bind_pat(p, t);
                    }
                }
                SemTy::Tuple(items) => {
                    self.error(
                        "E210",
                        "type mismatch in pattern".to_string(),
                        pat.span,
                        format!(
                            "pattern has {} elements but the value has {}",
                            elems.len(),
                            items.len()
                        ),
                    );
                    self.bind_pat_names(pat);
                }
                _ => {
                    self.mismatch(scrut, &SemTy::Tuple(Vec::new()), pat.span, "pattern");
                    self.bind_pat_names(pat);
                }
            },
            PatKind::Variant { path, args } => {
                self.bind_variant_pat(path, args, pat.span, scrut, pat);
            }
            PatKind::Struct { path, fields } => {
                self.bind_struct_pat(path, fields, pat.span, scrut, pat);
            }
        }
    }

    /// Bind every name in a pattern as `Error` (cascade suppression).
    fn bind_pat_names(&mut self, pat: &Pat) {
        match &pat.node {
            PatKind::Ident(name) => self.insert_binding(name, pat.span, SemTy::Error),
            PatKind::Variant { args, .. } => {
                for a in args {
                    self.bind_pat_names(a);
                }
            }
            PatKind::Struct { fields, .. } => {
                for f in fields {
                    self.bind_pat_names(&f.pat);
                }
            }
            PatKind::Tuple(elems) => {
                for e in elems {
                    self.bind_pat_names(e);
                }
            }
            PatKind::Wildcard | PatKind::Lit(_) => {}
        }
    }

    fn insert_binding(&mut self, name: &str, span: Span, ty: SemTy) {
        if let Some(rib) = self.scopes.last_mut() {
            // Duplicates in one rib were already reported by resolution;
            // keep the first binding.
            rib.entry(name.to_string()).or_insert_with(|| {
                let _ = span;
                ty
            });
        }
    }

    /// Check a variant pattern against a scrutinee type.
    fn bind_variant_pat(
        &mut self,
        _path: &[String],
        args: &[Pat],
        span: Span,
        scrut: &SemTy,
        pat: &Pat,
    ) {
        let key = span_pair(span);
        let resolved_name = self.resolved.values.get(&key).cloned();
        let (en, variant) = match resolved_name {
            Some(ValueName::Variant { en, variant }) => (en, variant),
            _ => {
                // Unresolved (already reported) — bind names, move on.
                self.bind_pat_names(pat);
                return;
            }
        };
        let payload_tys = match scrut {
            SemTy::Adt {
                kind: AdtKind::Enum,
                name,
                ..
            } if *name == en => {
                let variants = self.program.enums.get(&en).cloned().unwrap_or_default();
                match variants.iter().find(|(n, _)| *n == variant) {
                    Some((_, tys)) => tys.clone(),
                    None => {
                        self.error(
                            "E218",
                            "unknown variant".to_string(),
                            span,
                            format!("enum `{en}` has no variant `{variant}`"),
                        );
                        self.bind_pat_names(pat);
                        return;
                    }
                }
            }
            SemTy::Adt {
                kind: AdtKind::Enum,
                name,
                ..
            } => {
                self.error(
                    "E210",
                    "type mismatch in pattern".to_string(),
                    span,
                    format!("pattern is a variant of `{en}` but the value is `{name}`"),
                );
                self.bind_pat_names(pat);
                return;
            }
            _ => {
                self.mismatch(
                    scrut,
                    &SemTy::Adt {
                        kind: AdtKind::Enum,
                        name: en.clone(),
                        args: Vec::new(),
                    },
                    span,
                    "pattern",
                );
                self.bind_pat_names(pat);
                return;
            }
        };
        if args.len() != payload_tys.len() {
            self.error(
                "E211",
                "wrong number of arguments".to_string(),
                span,
                format!(
                    "variant `{variant}` takes {} payloads, pattern gives {}",
                    payload_tys.len(),
                    args.len()
                ),
            );
            self.bind_pat_names(pat);
            return;
        }
        for (p, t) in args.iter().zip(payload_tys.iter()) {
            self.bind_pat(p, t);
        }
    }

    /// Check a struct pattern against a scrutinee type.
    fn bind_struct_pat(
        &mut self,
        _path: &[String],
        fields: &[lobster_ast::StructPatField],
        span: Span,
        scrut: &SemTy,
        pat: &Pat,
    ) {
        let key = span_pair(span);
        let en = match self.resolved.values.get(&key).cloned() {
            Some(ValueName::Variant { en, .. }) => en,
            Some(ValueName::Struct(name)) => name,
            _ => {
                self.bind_pat_names(pat);
                return;
            }
        };
        let struct_fields = match scrut {
            SemTy::Adt {
                kind: AdtKind::Struct,
                name,
                ..
            } if *name == en => self.program.structs.get(&en).cloned().unwrap_or_default(),
            _ => {
                self.mismatch(
                    scrut,
                    &SemTy::Adt {
                        kind: AdtKind::Struct,
                        name: en.clone(),
                        args: Vec::new(),
                    },
                    span,
                    "pattern",
                );
                self.bind_pat_names(pat);
                return;
            }
        };
        for f in fields {
            match struct_fields.iter().find(|(n, _)| *n == f.name) {
                Some((_, t)) => self.bind_pat(&f.pat, t),
                None => {
                    self.error(
                        "E218",
                        "unknown field".to_string(),
                        f.span,
                        format!("struct `{en}` has no field `{}`", f.name),
                    );
                    self.bind_pat_names(&f.pat);
                }
            }
        }
    }

    // ----- expressions -----

    /// Check an expression against an optional expected type.
    /// Returns the type and whether evaluation diverges.
    fn check_expr(&mut self, expr: &Expr, expected: Option<&SemTy>) -> (SemTy, bool) {
        let (ty, diverges) = self.check_expr_inner(expr, expected);
        self.record(expr, ty.clone());
        (ty, diverges)
    }

    fn check_expr_inner(&mut self, expr: &Expr, expected: Option<&SemTy>) -> (SemTy, bool) {
        match &expr.node {
            ExprKind::Lit(lit) => {
                let (t, _) = self.check_literal(lit, expected, expr.span);
                (t, false)
            }
            ExprKind::Path(segments) => (self.check_path(segments, expr.span), false),
            ExprKind::Binary { op, lhs, rhs } => self.check_binary(*op, lhs, rhs, expr.span),
            ExprKind::Unary { op, operand } => self.check_unary(*op, operand, expr.span, expected),
            ExprKind::AddrOf { mutable, operand } => {
                let (t, _) = self.check_expr(operand, None);
                (
                    SemTy::Ref {
                        mutable: *mutable,
                        inner: Box::new(t),
                    },
                    false,
                )
            }
            ExprKind::Deref(operand) => {
                let (t, _) = self.check_expr(operand, None);
                match t {
                    SemTy::Ref { inner, .. } | SemTy::Ptr { inner, .. } => {
                        ((*inner).clone(), false)
                    }
                    SemTy::Error => (SemTy::Error, false),
                    _ => {
                        self.mismatch(
                            &SemTy::Ref {
                                mutable: false,
                                inner: Box::new(SemTy::Error),
                            },
                            &t,
                            expr.span,
                            "dereference",
                        );
                        (SemTy::Error, false)
                    }
                }
            }
            ExprKind::Cast { expr, ty } => {
                let (t, _) = self.check_expr(expr, None);
                let target = self.lower_sig_type(ty, "cast");
                if t != SemTy::Error && target != SemTy::Error && !cast_allowed(&t, &target) {
                    self.error(
                        "E212",
                        "illegal cast".to_string(),
                        expr.span,
                        format!("cannot cast {t} as {target}"),
                    );
                    return (SemTy::Error, false);
                }
                (target, false)
            }
            ExprKind::Assign { target, op, value } => {
                let (target_ty, _) = self.check_expr(target, None);
                let (value_ty, _) = self.check_expr(value, Some(&target_ty));
                if !is_place(target) {
                    self.error(
                        "E215",
                        "invalid assignment target".to_string(),
                        target.span,
                        "can only assign to a variable, field, or dereference".to_string(),
                    );
                    return (SemTy::Error, false);
                }
                match op {
                    None => {
                        if target_ty != SemTy::Error
                            && value_ty != SemTy::Error
                            && target_ty != value_ty
                        {
                            self.mismatch(&target_ty, &value_ty, expr.span, "assignment");
                        }
                    }
                    Some(bin) => {
                        if !is_compound_ok(*bin, &target_ty, &value_ty) {
                            self.error(
                                "E210",
                                "type mismatch in compound assignment".to_string(),
                                expr.span,
                                format!("cannot apply `{bin:?}` to {target_ty} and {value_ty}"),
                            );
                        }
                    }
                }
                (SemTy::Unit, false)
            }
            ExprKind::Call { callee, args } => self.check_call(callee, args, expr.span),
            ExprKind::Field { recv, field } => {
                let (t, _) = self.check_expr(recv, None);
                match t {
                    SemTy::Adt {
                        kind: AdtKind::Struct,
                        ref name,
                        ..
                    } => match self.program.structs.get(name) {
                        Some(fields) => match fields.iter().find(|(n, _)| n == field) {
                            Some((_, ft)) => (ft.clone(), false),
                            None => {
                                self.error(
                                    "E218",
                                    "unknown field".to_string(),
                                    expr.span,
                                    format!("struct `{name}` has no field `{field}`"),
                                );
                                (SemTy::Error, false)
                            }
                        },
                        None => (SemTy::Error, false),
                    },
                    SemTy::Error => (SemTy::Error, false),
                    _ => {
                        self.mismatch(
                            &SemTy::Adt {
                                kind: AdtKind::Struct,
                                name: String::new(),
                                args: Vec::new(),
                            },
                            &t,
                            expr.span,
                            "field access",
                        );
                        (SemTy::Error, false)
                    }
                }
            }
            ExprKind::If { cond, then, els } => {
                let (ct, _) = self.check_expr(cond, Some(&SemTy::Bool));
                if ct != SemTy::Bool && ct != SemTy::Error {
                    self.mismatch(&SemTy::Bool, &ct, cond.span, "if condition");
                }
                let (then_ty, then_div) = self.check_block(then, expected);
                match els {
                    Some(els) => {
                        let (els_ty, els_div) = self.check_expr(els, expected);
                        match SemTy::unify(&then_ty, &els_ty) {
                            Some(t) => (t, then_div && els_div),
                            None => {
                                self.mismatch(&then_ty, &els_ty, expr.span, "if branches");
                                (SemTy::Error, false)
                            }
                        }
                    }
                    None => {
                        if !then_div && then_ty != SemTy::Unit && then_ty != SemTy::Error {
                            self.mismatch(&SemTy::Unit, &then_ty, expr.span, "if without else");
                        }
                        (SemTy::Unit, false)
                    }
                }
            }
            ExprKind::Match { scrut, arms } => self.check_match(scrut, arms, expr.span, expected),
            ExprKind::Block(block) => self.check_block(block, expected),
            ExprKind::Tuple(elems) => {
                let items: Vec<SemTy> = match expected {
                    Some(SemTy::Tuple(want)) if want.len() == elems.len() => elems
                        .iter()
                        .zip(want.iter())
                        .map(|(e, w)| self.check_expr(e, Some(w)).0)
                        .collect(),
                    _ => elems.iter().map(|e| self.check_expr(e, None).0).collect(),
                };
                (SemTy::Tuple(items), false)
            }
            ExprKind::Unit => (SemTy::Unit, false),
        }
    }

    fn check_path(&mut self, segments: &[String], span: Span) -> SemTy {
        if segments.len() == 1 {
            let name = &segments[0];
            for rib in self.scopes.iter().rev() {
                if let Some(t) = rib.get(name) {
                    return t.clone();
                }
            }
        }
        match self.resolved.values.get(&span_pair(span)).cloned() {
            Some(ValueName::Local) => {
                // Bound after this use in a way resolution missed (should
                // not happen); suppress.
                SemTy::Error
            }
            Some(ValueName::Fn(name)) => match self.program.fns.get(&name) {
                Some(f) if f.generic => {
                    self.error(
                        "E217",
                        "generics are not supported yet".to_string(),
                        span,
                        format!("`{name}` is generic (monomorphization is deferred)"),
                    );
                    SemTy::Error
                }
                Some(f) => SemTy::Fn {
                    params: f.params.clone(),
                    ret: Box::new(f.ret.clone()),
                },
                None => SemTy::Error,
            },
            Some(ValueName::Variant { en, variant }) => {
                // Membership is verified at use sites (patterns, and any
                // value use must name a real variant).
                match self.program.enums.get(&en) {
                    Some(variants) if variants.iter().any(|(n, _)| *n == variant) => SemTy::Adt {
                        kind: AdtKind::Enum,
                        name: en,
                        args: Vec::new(),
                    },
                    Some(_) => {
                        self.error(
                            "E218",
                            "unknown variant".to_string(),
                            span,
                            format!("enum `{en}` has no variant `{variant}`"),
                        );
                        SemTy::Error
                    }
                    None => match self.program.structs.get(&en) {
                        // Struct paths in value position are only valid as
                        // pattern heads; a bare use is meaningless.
                        Some(_) => {
                            self.error(
                                "E210",
                                "type mismatch".to_string(),
                                span,
                                format!("`{en}` is a struct, not a value"),
                            );
                            SemTy::Error
                        }
                        None => SemTy::Error,
                    },
                }
            }
            Some(ValueName::BuiltinPrintln) => SemTy::Fn {
                params: Vec::new(),
                ret: Box::new(SemTy::Unit),
            },
            // Struct heads are only meaningful in patterns; a bare use
            // is not a value.
            Some(ValueName::Struct(name)) => {
                self.error(
                    "E210",
                    "type mismatch".to_string(),
                    span,
                    format!("`{name}` is a struct, not a value"),
                );
                SemTy::Error
            }
            None => SemTy::Error,
        }
    }

    fn check_call(&mut self, callee: &Expr, args: &[Expr], span: Span) -> (SemTy, bool) {
        // Fast paths first: named functions and the builtin.
        if let ExprKind::Path(segments) = &callee.node {
            let hit = self.resolved.values.get(&span_pair(callee.span)).cloned();
            match hit {
                Some(ValueName::BuiltinPrintln) => {
                    for a in args {
                        self.check_expr(a, None);
                    }
                    return (SemTy::Unit, false);
                }
                Some(ValueName::Fn(name)) => {
                    let sig = self.program.fns.get(&name).cloned().unwrap_or(CheckedFn {
                        name: name.clone(),
                        params: Vec::new(),
                        ret: SemTy::Error,
                        generic: false,
                    });
                    if sig.generic {
                        self.error(
                            "E217",
                            "generics are not supported yet".to_string(),
                            span,
                            format!("`{name}` is generic (monomorphization is deferred)"),
                        );
                        for a in args {
                            self.check_expr(a, None);
                        }
                        return (SemTy::Error, false);
                    }
                    if args.len() != sig.params.len() {
                        self.error(
                            "E211",
                            "wrong number of arguments".to_string(),
                            span,
                            format!(
                                "`{name}` takes {} arguments, {} given",
                                sig.params.len(),
                                args.len()
                            ),
                        );
                        for a in args {
                            self.check_expr(a, None);
                        }
                        return (sig.ret, false);
                    }
                    for (a, p) in args.iter().zip(sig.params.iter()) {
                        let (t, _) = self.check_expr(a, Some(p));
                        if t != SemTy::Error && *p != SemTy::Error && t != *p {
                            self.mismatch(p, &t, a.span, "argument");
                        }
                    }
                    return (sig.ret, false);
                }
                _ => {
                    let _ = segments;
                }
            }
        }
        // General call through a function-typed value.
        let (ct, _) = self.check_expr(callee, None);
        match ct {
            SemTy::Fn { params, ret } => {
                if args.len() != params.len() {
                    self.error(
                        "E211",
                        "wrong number of arguments".to_string(),
                        span,
                        format!(
                            "function takes {} arguments, {} given",
                            params.len(),
                            args.len()
                        ),
                    );
                    for a in args {
                        self.check_expr(a, None);
                    }
                    return (*ret, false);
                }
                for (a, p) in args.iter().zip(params.iter()) {
                    let (t, _) = self.check_expr(a, Some(p));
                    if t != SemTy::Error && *p != SemTy::Error && t != *p {
                        self.mismatch(p, &t, a.span, "argument");
                    }
                }
                (*ret, false)
            }
            SemTy::Error => {
                for a in args {
                    self.check_expr(a, None);
                }
                (SemTy::Error, false)
            }
            _ => {
                self.error(
                    "E210",
                    "type mismatch in call".to_string(),
                    callee.span,
                    format!("cannot call a value of type {ct}"),
                );
                for a in args {
                    self.check_expr(a, None);
                }
                (SemTy::Error, false)
            }
        }
    }

    fn check_binary(&mut self, op: BinOp, lhs: &Expr, rhs: &Expr, span: Span) -> (SemTy, bool) {
        let (lt, _) = self.check_expr(lhs, None);
        // The right side adopts the left's type when it is concrete, so
        // `n < 2` reads `2` as the type of `n`.
        let rhs_expected = if lt != SemTy::Error { Some(&lt) } else { None };
        let (rt, _) = match rhs_expected {
            Some(t) => self.check_expr(rhs, Some(t)),
            None => self.check_expr(rhs, None),
        };
        // Symmetric case (`2 + n`): a defaulted literal on the left adopts
        // a concrete numeric right side. Recording is idempotent, so the
        // re-check only overwrites the node's type.
        let (lt, rt) = if is_defaulted_literal(lhs, &lt) && rt.is_numeric() && lt != rt {
            (self.check_expr(lhs, Some(&rt)).0, rt)
        } else {
            (lt, rt)
        };
        if lt == SemTy::Error || rt == SemTy::Error {
            return (SemTy::Error, false);
        }
        let mut bad = |detail: String| {
            self.error(
                "E210",
                "type mismatch in binary operation".to_string(),
                span,
                detail,
            );
        };
        match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => {
                if lt == rt && lt.is_numeric() {
                    (lt, false)
                } else {
                    bad(format!("cannot apply `{op:?}` to {lt} and {rt}"));
                    (SemTy::Error, false)
                }
            }
            BinOp::Shl | BinOp::Shr => {
                if lt.is_int() && rt.is_int() {
                    (lt, false)
                } else {
                    bad(format!("cannot shift {lt} by {rt}"));
                    (SemTy::Error, false)
                }
            }
            BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor => {
                if lt.is_int() && lt == rt {
                    (lt, false)
                } else {
                    bad(format!("cannot apply `{op:?}` to {lt} and {rt}"));
                    (SemTy::Error, false)
                }
            }
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                if lt == rt && (lt.is_numeric() || lt == SemTy::Bool || lt == SemTy::Char) {
                    (SemTy::Bool, false)
                } else {
                    bad(format!("cannot compare {lt} and {rt}"));
                    (SemTy::Error, false)
                }
            }
            BinOp::And | BinOp::Or => {
                if lt == SemTy::Bool && rt == SemTy::Bool {
                    (SemTy::Bool, false)
                } else {
                    bad(format!("logical operators need bool, found {lt} and {rt}"));
                    (SemTy::Error, false)
                }
            }
        }
    }

    /// Check a unary expression. Negated integer literals take the
    /// magnitude rule (`-128i8` is valid) using the suffix or the
    /// expected type, so the operand is never falsely rejected.
    fn check_unary(
        &mut self,
        op: UnOp,
        operand: &Expr,
        span: Span,
        expected: Option<&SemTy>,
    ) -> (SemTy, bool) {
        if matches!(op, UnOp::Neg) {
            if let ExprKind::Lit(Literal::Int { text, suffix }) = &operand.node {
                let (core, _) = suffix_aware_core(text, suffix.as_deref());
                // A float suffix here means float shapes, which take the
                // general path below; only integer targets stay special.
                let int_target: Option<IntTy> = match suffix.as_deref() {
                    Some(s) => match suffix_type(s) {
                        Some(SemTy::Int(t)) => Some(t),
                        _ => None,
                    },
                    None => match expected {
                        Some(SemTy::Int(t)) => Some(*t),
                        _ => Some(IntTy::I32),
                    },
                };
                if let Some(t) = int_target {
                    if !neg_literal_fits(t, core) {
                        self.error(
                            "E216",
                            "literal out of range".to_string(),
                            operand.span,
                            format!("-{core} does not fit in {t}"),
                        );
                    }
                    let ty = SemTy::Int(t);
                    self.record(operand, ty.clone());
                    return (ty, false);
                }
            }
        }
        let (t, _) = self.check_expr(operand, None);
        match (op, &t) {
            (UnOp::Neg, SemTy::Int(_)) | (UnOp::Neg, SemTy::Float(_)) => (t, false),
            (UnOp::Not, SemTy::Bool) => (SemTy::Bool, false),
            (UnOp::Not, SemTy::Int(_)) => (t, false),
            (_, SemTy::Error) => (SemTy::Error, false),
            _ => {
                self.error(
                    "E210",
                    "type mismatch in unary operation".to_string(),
                    span,
                    format!("cannot apply `{op:?}` to {t}"),
                );
                (SemTy::Error, false)
            }
        }
    }

    fn check_match(
        &mut self,
        scrut: &Expr,
        arms: &[MatchArm],
        span: Span,
        expected: Option<&SemTy>,
    ) -> (SemTy, bool) {
        let (st, _) = self.check_expr(scrut, None);
        let mut arm_ty: Option<SemTy> = None;
        let mut all_diverge = !arms.is_empty();
        // Coverage tracking for exhaustiveness.
        let mut saw_catchall = false;
        let mut covered_variants: HashSet<String> = HashSet::new();
        let scrut_variants: Option<Vec<String>> = match &st {
            SemTy::Adt {
                kind: AdtKind::Enum,
                name,
                ..
            } => self
                .program
                .enums
                .get(name)
                .map(|vs| vs.iter().map(|(n, _)| n.clone()).collect()),
            _ => None,
        };
        for arm in arms {
            self.scopes.push(HashMap::new());
            self.bind_pat(&arm.pat, &st);
            let (t, d) = self.check_expr(&arm.body, expected);
            self.scopes.pop();
            match &arm.pat.node {
                PatKind::Wildcard | PatKind::Ident(_) => saw_catchall = true,
                PatKind::Variant { path, .. } => {
                    if let Some(last) = path.last() {
                        covered_variants.insert(last.clone());
                    }
                }
                // One shape: any struct pattern covers a struct scrutinee.
                PatKind::Struct { .. }
                    if matches!(
                        st,
                        SemTy::Adt {
                            kind: AdtKind::Struct,
                            ..
                        }
                    ) =>
                {
                    saw_catchall = true;
                }
                _ => {}
            }
            match arm_ty.clone() {
                None => arm_ty = Some(t),
                Some(want) => {
                    if t != SemTy::Error && want != SemTy::Error && t != want {
                        self.mismatch(&want, &t, arm.body.span, "match arm");
                    }
                }
            }
            all_diverge = all_diverge && d;
        }
        if arms.is_empty() {
            self.error(
                "E213",
                "non-exhaustive match".to_string(),
                span,
                "match has no arms".to_string(),
            );
        } else if st != SemTy::Error && !saw_catchall {
            match scrut_variants {
                Some(all) => {
                    let missing: Vec<_> = all
                        .iter()
                        .filter(|v| !covered_variants.contains(*v))
                        .collect();
                    if !missing.is_empty() {
                        let missing: Vec<_> = missing.iter().map(|s| s.as_str()).collect();
                        self.error(
                            "E213",
                            "non-exhaustive match".to_string(),
                            span,
                            format!("missing variants: {}", missing.join(", ")),
                        );
                    }
                }
                None => {
                    self.error(
                        "E213",
                        "non-exhaustive match".to_string(),
                        span,
                        "add a wildcard arm (`_`)".to_string(),
                    );
                }
            }
        }
        (arm_ty.unwrap_or(SemTy::Unit), all_diverge)
    }

    /// Check a literal against an optional expected type. Returns the type.
    fn check_literal(
        &mut self,
        lit: &Literal,
        expected: Option<&SemTy>,
        span: Span,
    ) -> (SemTy, bool) {
        match lit {
            Literal::Int { text, suffix } => {
                let (core, _) = suffix_aware_core(text, suffix.as_deref());
                // Suffix decides the type; otherwise adopt context, else i32.
                if let Some(s) = suffix.as_deref() {
                    match suffix_type(s) {
                        Some(SemTy::Int(t)) => {
                            if !int_literal_fits(t, core) {
                                self.error(
                                    "E216",
                                    "literal out of range".to_string(),
                                    span,
                                    format!("{core} does not fit in {t}"),
                                );
                                return (SemTy::Error, false);
                            }
                            return (SemTy::Int(t), false);
                        }
                        Some(SemTy::Float(_)) => {
                            // `42f32`: float suffix on an integer shape.
                            return (float_suffix_ty(s), false);
                        }
                        // Unreachable: the lexer only emits listed suffixes.
                        None | Some(_) => {
                            return (SemTy::Error, false);
                        }
                    }
                }
                if text_is_float_shaped(core) {
                    return self.check_float_text(core, expected, span);
                }
                match expected {
                    Some(SemTy::Int(t)) => {
                        if !int_literal_fits(*t, core) {
                            self.error(
                                "E216",
                                "literal out of range".to_string(),
                                span,
                                format!("{core} does not fit in {t}"),
                            );
                            return (SemTy::Error, false);
                        }
                        (SemTy::Int(*t), false)
                    }
                    Some(SemTy::Error) | None => {
                        if !int_literal_fits(IntTy::I32, core) {
                            // Unconstrained and out of i32 range: still an
                            // error (literals never silently wrap).
                            self.error(
                                "E216",
                                "literal out of range".to_string(),
                                span,
                                format!("{core} does not fit in i32"),
                            );
                            return (SemTy::Error, false);
                        }
                        (SemTy::Int(IntTy::I32), false)
                    }
                    Some(other) => {
                        self.mismatch(other, &SemTy::Int(IntTy::I32), span, "literal");
                        (SemTy::Error, false)
                    }
                }
            }
            Literal::Float { text, suffix } => {
                let (core, _) = suffix_aware_core(text, suffix.as_deref());
                if let Some(s) = suffix.as_deref() {
                    match suffix_type(s) {
                        Some(SemTy::Float(t)) => return (SemTy::Float(t), false),
                        _ => {
                            self.error(
                                "E216",
                                "literal out of range".to_string(),
                                span,
                                format!("integer suffix `{s}` on a float literal"),
                            );
                            return (SemTy::Error, false);
                        }
                    }
                }
                self.check_float_text(core, expected, span)
            }
            Literal::Str(_) => (
                SemTy::Ref {
                    mutable: false,
                    inner: Box::new(SemTy::Str),
                },
                false,
            ),
            Literal::Char(_) => (SemTy::Char, false),
            Literal::Bool(_) => (SemTy::Bool, false),
        }
    }

    /// Check a float-shaped core against context (defaults to f64).
    fn check_float_text(
        &mut self,
        _core: &str,
        expected: Option<&SemTy>,
        span: Span,
    ) -> (SemTy, bool) {
        match expected {
            Some(SemTy::Float(t)) => (SemTy::Float(*t), false),
            Some(SemTy::Error) | None => (SemTy::Float(FloatTy::F64), false),
            Some(other) => {
                self.mismatch(other, &SemTy::Float(FloatTy::F64), span, "literal");
                (SemTy::Error, false)
            }
        }
    }
}

/// Integer or float type named by a literal suffix.
fn suffix_type(suffix: &str) -> Option<SemTy> {
    if let Some(t) = IntTy::from_name(suffix) {
        return Some(SemTy::Int(t));
    }
    if let Some(t) = FloatTy::from_name(suffix) {
        return Some(SemTy::Float(t));
    }
    None
}

/// Float type named by a float suffix (the caller checked the shape).
fn float_suffix_ty(suffix: &str) -> SemTy {
    // `f32` only reaches here from integer-shaped text with an `f` suffix;
    // `f64` likewise. Anything else is a caller bug, default to f64.
    match suffix {
        "f32" => SemTy::Float(FloatTy::F32),
        _ => SemTy::Float(FloatTy::F64),
    }
}

/// Core text for range checks: the full text minus a known suffix.
/// Falls back to the lexer-mirroring splitter for safety.
fn suffix_aware_core<'t>(text: &'t str, suffix: Option<&'t str>) -> (&'t str, Option<&'t str>) {
    match suffix {
        Some(s) => match text.strip_suffix(s) {
            Some(core) => (core, Some(s)),
            None => (text, None),
        },
        None => (text, None),
    }
}

/// True when the core has a fraction or exponent (`1.5`, `2e10`).
fn text_is_float_shaped(core: &str) -> bool {
    core.contains('.') || core.contains('e') || core.contains('E')
}

/// True when an expression is an unsuffixed literal sitting on its
/// default type (`10` as i32, `1.5` as f64): free to adopt context.
fn is_defaulted_literal(expr: &Expr, ty: &SemTy) -> bool {
    matches!(
        (&expr.node, ty),
        (
            ExprKind::Lit(Literal::Int { suffix: None, .. }),
            SemTy::Int(IntTy::I32)
        ) | (
            ExprKind::Lit(Literal::Float { suffix: None, .. }),
            SemTy::Float(FloatTy::F64)
        )
    )
}

/// True for assignment targets: paths, fields, dereferences.
fn is_place(target: &Expr) -> bool {
    match &target.node {
        ExprKind::Path(_) | ExprKind::Field { .. } | ExprKind::Deref(_) => true,
        ExprKind::Block(b) => b.stmts.is_empty() && b.tail.as_ref().is_some_and(|t| is_place(t)),
        _ => false,
    }
}

/// Legality of `target op= value` beyond plain assignment.
fn is_compound_ok(op: BinOp, target: &SemTy, value: &SemTy) -> bool {
    if matches!(target, SemTy::Error) || matches!(value, SemTy::Error) {
        return true;
    }
    match op {
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => {
            target.is_numeric() && target == value
        }
        BinOp::Shl | BinOp::Shr => target.is_int() && value.is_int(),
        BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor => target.is_int() && target == value,
        _ => false,
    }
}

/// Cast legality (spec §4 cast table). Pointer casts are allowed here;
/// `unsafe` checking is deferred (documented gap).
fn cast_allowed(from: &SemTy, to: &SemTy) -> bool {
    from == to
        || matches!(
            (from, to),
            (SemTy::Int(_), SemTy::Int(_))
                | (SemTy::Int(_), SemTy::Float(_))
                | (SemTy::Float(_), SemTy::Float(_))
                | (SemTy::Float(_), SemTy::Int(_))
                | (SemTy::Char, SemTy::Int(_))
                | (SemTy::Int(_), SemTy::Char)
                | (SemTy::Bool, SemTy::Int(_))
                | (SemTy::Ptr { .. }, SemTy::Int(_))
                | (SemTy::Int(_), SemTy::Ptr { .. })
                | (SemTy::Ptr { .. }, SemTy::Ptr { .. })
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use lobster_lexer::lex;
    use lobster_parser::parse;
    use lobster_source::SourceManager;

    fn check_text(text: &str) -> (Program, Tables, Vec<Diagnostic>) {
        let mut sm = SourceManager::new();
        let id = sm.add_file("t.lobster", text);
        let lexed = lex(&sm, id, text);
        assert!(lexed.diagnostics.is_empty());
        let parsed = parse(id, &lexed.tokens);
        assert!(parsed.diagnostics.is_empty());
        let (resolved, rdiags) = lobster_resolve::resolve(&parsed.file);
        assert!(rdiags.is_empty(), "{rdiags:?}");
        check_program(&resolved, &parsed.file)
    }

    fn codes(diags: &[Diagnostic]) -> Vec<String> {
        diags
            .iter()
            .map(|d| d.code.as_ref().map(|c| c.0.clone()).unwrap_or_default())
            .collect()
    }

    #[test]
    fn fib_checks_clean_with_u64_nodes() {
        let text = "fn fib(n: u64) -> u64 {\n    if n < 2 {\n        return n;\n    }\n\n    fib(n - 1) + fib(n - 2)\n}\n";
        let (program, tables, diags) = check_text(text);
        assert!(diags.is_empty(), "{diags:?}");
        let f = &program.fns["fib"];
        assert_eq!(f.ret, SemTy::Int(IntTy::U64));
        // The tail `+` node carries u64.
        assert!(tables
            .node_types
            .values()
            .any(|t| *t == SemTy::Int(IntTy::U64)));
    }

    #[test]
    fn let_mismatch_is_e210() {
        let (_, _, diags) = check_text("fn main() {\n    let x: i32 = true;\n}\n");
        assert_eq!(codes(&diags), ["E210"]);
    }

    #[test]
    fn unsuffixed_literal_adopts_context() {
        let (_, _, diags) = check_text("fn main() {\n    let x: u64 = 10;\n}\n");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn arity_is_e211() {
        let (_, _, diags) = check_text("fn f(a: u32) {}\nfn main() { f(1u32, 2u32); }\n");
        assert_eq!(codes(&diags), ["E211"]);
    }

    #[test]
    fn bad_cast_is_e212() {
        let (_, _, diags) = check_text("fn main() { let x = true as u32; }\n");
        // bool-as-int is allowed; int-as-bool is not.
        assert!(diags.is_empty(), "{diags:?}");
        let (_, _, diags) = check_text("fn main() { let x = 1u32 as bool; }\n");
        assert_eq!(codes(&diags), ["E212"]);
    }

    #[test]
    fn nonexhaustive_is_e213() {
        let (_, _, diags) =
            check_text("enum Dir { North, South, }\nfn f(d: Dir) -> u32 {\n    match d {\n        North => 1u32,\n    }\n}\n");
        assert_eq!(codes(&diags), ["E213"]);
    }

    #[test]
    fn return_mismatch_is_e214() {
        let (_, _, diags) = check_text("fn f() -> u32 { true; }\n");
        assert_eq!(codes(&diags), ["E214"]);
    }

    #[test]
    fn bad_assign_target_is_e215() {
        let (_, _, diags) = check_text("fn main() { 1u32 = 2u32; }\n");
        assert_eq!(codes(&diags), ["E215"]);
    }

    #[test]
    fn break_outside_loop_is_e215() {
        let (_, _, diags) = check_text("fn main() { break; }\n");
        assert_eq!(codes(&diags), ["E215"]);
    }

    #[test]
    fn int_range_is_e216() {
        let (_, _, diags) = check_text("fn main() { let x: u8 = 300u8; }\n");
        assert_eq!(codes(&diags), ["E216"]);
    }

    #[test]
    fn negated_minimum_is_allowed() {
        let (_, _, diags) = check_text("fn main() { let x: i8 = -128i8; }\n");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn missing_init_is_e217() {
        let (_, _, diags) = check_text("fn main() { let x: i32; }\n");
        assert_eq!(codes(&diags), ["E217"]);
    }

    #[test]
    fn generic_use_is_e217() {
        let (_, _, diags) =
            check_text("enum Option<T> { Some(T), None, }\nfn f(v: Option<u64>) {}\n");
        assert_eq!(codes(&diags), ["E217"]);
    }

    #[test]
    fn struct_pattern_is_exhaustive() {
        let (_, _, diags) = check_text(
            "struct Point { x: f32, y: f32, }\nfn get_x(p: Point) -> f32 {\n    match p {\n        Point { x, y: _ } => x,\n    }\n}\n",
        );
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn unknown_variant_is_e218() {
        let (_, _, diags) = check_text("enum Dir { North, }\nfn f() { Dir::Up; }\n");
        assert_eq!(codes(&diags), ["E218"]);
    }
}
