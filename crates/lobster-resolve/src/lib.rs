//! Lobster name resolution.
//!
//! Single-file, order-independent item collection plus scoped binding
//! resolution. Every global reference is recorded in [`Resolved`] keyed by
//! source span so the checker can answer "what does this path mean" without
//! re-walking scopes. Locals stay in ribs: the checker rebuilds the same
//! ribs and resolves locals itself; only globals are exported.
//!
//! Resolution diagnostic codes:
//!
//! ```text
//! E200 unresolved name
//! E201 duplicate definition
//! E202 unresolved import (no modules exist yet)
//! E203 unresolved type
//! ```

use lobster_ast::*;
use lobster_diagnostics::{Diagnostic, Label};
use lobster_source::Span;
use lobster_types::{AdtKind, FloatTy, IntTy, Ty as SemTy};
use std::collections::{HashMap, HashSet};

/// Key into the name tables: the `(start, end)` of a path node.
pub type SpanPair = (u32, u32);

/// Span key for a node span.
#[must_use]
pub const fn span_pair(span: Span) -> SpanPair {
    (span.start, span.end)
}

/// A resolved function item.
#[derive(Debug, Clone)]
pub struct ResolvedFn {
    /// Span of the item (names are not separately spanned in the AST).
    pub span: Span,
    /// Number of generic parameters (bodies of generic fns are skipped).
    pub generic_count: usize,
    /// Parameter names in order.
    pub params: Vec<String>,
}

/// A resolved struct item.
#[derive(Debug, Clone)]
pub struct ResolvedStruct {
    /// Span of the item.
    pub span: Span,
    /// Number of generic parameters.
    pub generic_count: usize,
    /// Field names in order.
    pub fields: Vec<String>,
}

/// A resolved enum variant.
#[derive(Debug, Clone)]
pub struct ResolvedVariant {
    /// Variant name.
    pub name: String,
    /// Span of the variant name.
    pub span: Span,
    /// Number of tuple payload types.
    pub payload_arity: usize,
}

/// A resolved enum item.
#[derive(Debug, Clone)]
pub struct ResolvedEnum {
    /// Span of the item.
    pub span: Span,
    /// Number of generic parameters.
    pub generic_count: usize,
    /// Variants in source order (index = position).
    pub variants: Vec<ResolvedVariant>,
}

/// Meaning of a value-position path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValueName {
    /// A local binding or parameter (the checker resolves these by name
    /// in its own ribs).
    Local,
    /// A function item.
    Fn(String),
    /// An enum variant (membership is checked later; unknown variants
    /// of a known enum become E218, not E200).
    Variant {
        /// Parent enum.
        en: String,
        /// Variant name.
        variant: String,
    },
    /// A struct head for struct patterns (`Point { x }`).
    Struct(String),
    /// The `println` builtin (any arguments, unit result).
    BuiltinPrintln,
}

/// Meaning of a type-position path leaf.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeName {
    /// A primitive type.
    Prim(SemTy),
    /// A struct item.
    Struct(String),
    /// An enum item.
    Enum(String),
    /// A generic parameter in scope (lowers to `Error` silently; uses
    /// of generic items are rejected by the checker).
    GenericParam(String),
}

/// Failed type lowering (the checker maps these to diagnostics).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeError {
    /// Unknown type name (resolution already reported E203).
    Unknown,
    /// Use of a generic item (checker reports E217).
    Generic,
}

/// Fully resolved file: item tables plus per-occurrence name meanings.
#[derive(Debug, Default)]
pub struct Resolved {
    /// Functions by name.
    pub fns: HashMap<String, ResolvedFn>,
    /// Structs by name.
    pub structs: HashMap<String, ResolvedStruct>,
    /// Enums by name.
    pub enums: HashMap<String, ResolvedEnum>,
    /// Value-position path meanings by span.
    pub values: HashMap<SpanPair, ValueName>,
    /// Type-position path meanings by span.
    pub types: HashMap<SpanPair, TypeName>,
}

impl Resolved {
    /// Lower a syntactic type to a [`Ty`].
    ///
    /// `generics` holds the generic parameter names in scope (from the
    /// enclosing item). Returns `Err` without recording anything: callers
    /// map [`TypeError`] to diagnostics, except `Unknown`, which resolution
    /// already reported and must stay silent to avoid cascades.
    pub fn lower_type(&self, ty: &TyKind, generics: &HashSet<String>) -> Result<SemTy, TypeError> {
        match ty {
            TyKind::Path { segments, args } => {
                if segments.len() != 1 {
                    return Err(TypeError::Unknown);
                }
                let name = &segments[0];
                if let Some(t) = primitive_type(name) {
                    if !args.is_empty() {
                        return Err(TypeError::Unknown);
                    }
                    return Ok(t);
                }
                if generics.contains(name) {
                    return Ok(SemTy::Error);
                }
                if let Some(s) = self.structs.get(name) {
                    if s.generic_count > 0 {
                        return Err(TypeError::Generic);
                    }
                    if !args.is_empty() {
                        return Err(TypeError::Unknown);
                    }
                    return Ok(SemTy::Adt {
                        kind: AdtKind::Struct,
                        name: name.clone(),
                        args: Vec::new(),
                    });
                }
                if let Some(e) = self.enums.get(name) {
                    if e.generic_count > 0 {
                        return Err(TypeError::Generic);
                    }
                    if !args.is_empty() {
                        return Err(TypeError::Unknown);
                    }
                    return Ok(SemTy::Adt {
                        kind: AdtKind::Enum,
                        name: name.clone(),
                        args: Vec::new(),
                    });
                }
                Err(TypeError::Unknown)
            }
            TyKind::Ref { mutable, inner } => Ok(SemTy::Ref {
                mutable: *mutable,
                inner: Box::new(self.lower_spanned(inner.as_ref(), generics)?),
            }),
            TyKind::Ptr { mutable, inner } => Ok(SemTy::Ptr {
                mutable: *mutable,
                inner: Box::new(self.lower_spanned(inner.as_ref(), generics)?),
            }),
            TyKind::Array { elem, .. } => {
                // Length const-eval is deferred; only literals are accepted
                // (the checker validates the expression separately).
                Ok(SemTy::Array(
                    Box::new(self.lower_spanned(elem, generics)?),
                    None,
                ))
            }
            TyKind::Slice(inner) => {
                Ok(SemTy::Slice(Box::new(self.lower_spanned(inner, generics)?)))
            }
            TyKind::Tuple(elems) => {
                let mut out = Vec::with_capacity(elems.len());
                for e in elems {
                    out.push(self.lower_spanned(e, generics)?);
                }
                Ok(SemTy::Tuple(out))
            }
            TyKind::Unit => Ok(SemTy::Unit),
        }
    }

    /// Lower a spanned type (see [`Resolved::lower_type`]).
    pub fn lower_spanned(&self, ty: &Ty, generics: &HashSet<String>) -> Result<SemTy, TypeError> {
        self.lower_type(&ty.node, generics)
    }
}

/// Primitive type by name (`i32`, `bool`, `str`, …).
fn primitive_type(name: &str) -> Option<SemTy> {
    if let Some(t) = IntTy::from_name(name) {
        return Some(SemTy::Int(t));
    }
    if let Some(t) = FloatTy::from_name(name) {
        return Some(SemTy::Float(t));
    }
    match name {
        "bool" => Some(SemTy::Bool),
        "char" => Some(SemTy::Char),
        "str" => Some(SemTy::Str),
        _ => None,
    }
}

/// Resolve a file: collect items, then resolve every path occurrence.
///
/// Returns the tables plus diagnostics in source order. Resolution never
/// fails wholesale — unknown names are reported and skipped.
#[must_use]
pub fn resolve(file: &File) -> (Resolved, Vec<Diagnostic>) {
    Resolver::new().run(file)
}

struct Resolver {
    resolved: Resolved,
    diagnostics: Vec<Diagnostic>,
    /// Local ribs (function bodies only; item collection is separate).
    ribs: Vec<HashMap<String, Span>>,
}

impl Resolver {
    fn new() -> Self {
        Self {
            resolved: Resolved::default(),
            diagnostics: Vec::new(),
            ribs: Vec::new(),
        }
    }

    fn run(mut self, file: &File) -> (Resolved, Vec<Diagnostic>) {
        self.collect_items(file);
        for item in &file.items {
            self.resolve_item(item);
        }
        (self.resolved, self.diagnostics)
    }

    fn error(&mut self, code: &str, message: String, span: Span, detail: String) {
        self.diagnostics.push(Diagnostic::error(
            code,
            message,
            Label::primary(span, detail),
        ));
    }

    // ----- item collection (order-independent) -----

    fn collect_items(&mut self, file: &File) {
        let mut seen: HashMap<String, Span> = HashMap::new();
        for item in &file.items {
            let (name, kind) = match &item.node {
                ItemKind::Fn(f) => (f.name.clone(), "function"),
                ItemKind::Struct(s) => (s.name.clone(), "struct"),
                ItemKind::Enum(e) => (e.name.clone(), "enum"),
                ItemKind::Import(_) => continue,
            };
            if seen.contains_key(&name) {
                self.error(
                    "E201",
                    "duplicate definition".to_string(),
                    item.span,
                    format!("{kind} `{name}` is defined more than once"),
                );
                continue;
            }
            seen.insert(name.clone(), item.span);
            match &item.node {
                ItemKind::Fn(f) => {
                    self.resolved.fns.insert(
                        name,
                        ResolvedFn {
                            span: item.span,
                            generic_count: f.generics.len(),
                            params: f.params.iter().map(|p| p.name.clone()).collect(),
                        },
                    );
                }
                ItemKind::Struct(s) => {
                    self.resolved.structs.insert(
                        name,
                        ResolvedStruct {
                            span: item.span,
                            generic_count: s.generics.len(),
                            fields: s.fields.iter().map(|f| f.name.clone()).collect(),
                        },
                    );
                }
                ItemKind::Enum(e) => {
                    self.resolved.enums.insert(
                        name,
                        ResolvedEnum {
                            span: item.span,
                            generic_count: e.generics.len(),
                            variants: e
                                .variants
                                .iter()
                                .map(|v| ResolvedVariant {
                                    name: v.name.clone(),
                                    span: v.span,
                                    payload_arity: v.payload.len(),
                                })
                                .collect(),
                        },
                    );
                }
                ItemKind::Import(_) => {}
            }
        }
        // Duplicate variants within one enum.
        for e in file.items.iter().filter_map(|i| match &i.node {
            ItemKind::Enum(e) => Some(e),
            _ => None,
        }) {
            let mut seen_v: HashSet<&str> = HashSet::new();
            for v in &e.variants {
                if !seen_v.insert(v.name.as_str()) {
                    self.error(
                        "E201",
                        "duplicate definition".to_string(),
                        v.span,
                        format!("variant `{}` defined more than once", v.name),
                    );
                }
            }
        }
        // Duplicate struct fields within one struct.
        for s in file.items.iter().filter_map(|i| match &i.node {
            ItemKind::Struct(s) => Some(s),
            _ => None,
        }) {
            let mut seen_f: HashSet<&str> = HashSet::new();
            for f in &s.fields {
                if !seen_f.insert(f.name.as_str()) {
                    self.error(
                        "E201",
                        "duplicate definition".to_string(),
                        f.span,
                        format!("field `{}` defined more than once", f.name),
                    );
                }
            }
        }
    }

    // ----- per-item resolution -----

    fn resolve_item(&mut self, item: &Item) {
        match &item.node {
            ItemKind::Fn(f) => {
                let mut seen: HashSet<&str> = HashSet::new();
                for p in &f.params {
                    if !seen.insert(p.name.as_str()) {
                        self.error(
                            "E201",
                            "duplicate definition".to_string(),
                            p.span,
                            format!("parameter `{}` declared more than once", p.name),
                        );
                    }
                }
                // Generic bodies are skipped (monomorphization is deferred).
                if !f.generics.is_empty() {
                    return;
                }
                let generics = HashSet::new();
                for p in &f.params {
                    self.resolve_type(&p.ty, &generics);
                }
                if let Some(ret) = &f.ret {
                    self.resolve_type(ret, &generics);
                }
                self.ribs.push(HashMap::new());
                for p in &f.params {
                    self.bind(&p.name, p.span);
                }
                self.resolve_block(&f.body);
                self.ribs.pop();
            }
            ItemKind::Struct(s) => {
                let generics: HashSet<String> = s.generics.iter().map(|g| g.name.clone()).collect();
                for field in &s.fields {
                    self.resolve_type(&field.ty, &generics);
                }
            }
            ItemKind::Enum(e) => {
                let generics: HashSet<String> = e.generics.iter().map(|g| g.name.clone()).collect();
                for v in &e.variants {
                    for t in &v.payload {
                        self.resolve_type(t, &generics);
                    }
                }
            }
            ItemKind::Import(import) => {
                // No modules exist yet: every import dangles.
                self.error(
                    "E202",
                    "unresolved import".to_string(),
                    item.span,
                    format!(
                        "no module `{}` (multi-file modules are deferred)",
                        import.path.join("::")
                    ),
                );
            }
        }
    }

    /// Record a type-position path meaning (or report E203).
    fn resolve_type(&mut self, ty: &Ty, generics: &HashSet<String>) {
        match &ty.node {
            TyKind::Path { segments, .. } => {
                if segments.len() != 1 {
                    self.error(
                        "E203",
                        "unresolved type".to_string(),
                        ty.span,
                        "qualified type paths need modules (deferred)".to_string(),
                    );
                    return;
                }
                let name = &segments[0];
                let meaning = if let Some(t) = primitive_type(name) {
                    Some(TypeName::Prim(t))
                } else if self.resolved.structs.contains_key(name) {
                    Some(TypeName::Struct(name.clone()))
                } else if self.resolved.enums.contains_key(name) {
                    Some(TypeName::Enum(name.clone()))
                } else if generics.contains(name) {
                    Some(TypeName::GenericParam(name.clone()))
                } else {
                    None
                };
                match meaning {
                    Some(m) => {
                        self.resolved.types.insert(span_pair(ty.span), m);
                    }
                    None => {
                        self.error(
                            "E203",
                            "unresolved type".to_string(),
                            ty.span,
                            format!("no type named `{name}` in scope"),
                        );
                    }
                }
            }
            TyKind::Ref { inner, .. } | TyKind::Ptr { inner, .. } | TyKind::Slice(inner) => {
                self.resolve_type(inner, generics);
            }
            TyKind::Array { elem, .. } => self.resolve_type(elem, generics),
            TyKind::Tuple(elems) => {
                for e in elems {
                    self.resolve_type(e, generics);
                }
            }
            TyKind::Unit => {}
        }
    }

    fn bind(&mut self, name: &str, span: Span) {
        if let Some(rib) = self.ribs.last_mut() {
            if rib.contains_key(name) {
                self.error(
                    "E201",
                    "duplicate definition".to_string(),
                    span,
                    format!("`{name}` is already bound in this scope"),
                );
                return;
            }
            rib.insert(name.to_string(), span);
        }
    }

    fn lookup_local(&self, name: &str) -> bool {
        self.ribs.iter().rev().any(|rib| rib.contains_key(name))
    }

    // ----- bodies -----

    fn resolve_block(&mut self, block: &Block) {
        self.ribs.push(HashMap::new());
        for stmt in &block.stmts {
            self.resolve_stmt(stmt);
        }
        if let Some(tail) = &block.tail {
            self.resolve_expr(tail);
        }
        self.ribs.pop();
    }

    fn resolve_stmt(&mut self, stmt: &Stmt) {
        match &stmt.node {
            StmtKind::Let(l) => {
                if let Some(init) = &l.init {
                    self.resolve_expr(init);
                }
                self.bind_pat(&l.pat);
                if let Some(ty) = &l.ty {
                    self.resolve_type(ty, &HashSet::new());
                }
            }
            StmtKind::Return(e) => {
                if let Some(e) = e {
                    self.resolve_expr(e);
                }
            }
            StmtKind::While { cond, body } => {
                self.resolve_expr(cond);
                self.resolve_block(body);
            }
            StmtKind::Loop(body) => self.resolve_block(body),
            StmtKind::For { pat, iter, body } => {
                self.resolve_expr(iter);
                self.ribs.push(HashMap::new());
                self.bind_pat(pat);
                self.resolve_block(body);
                self.ribs.pop();
            }
            StmtKind::Break | StmtKind::Continue => {}
            StmtKind::Expr(e) => self.resolve_expr(e),
        }
    }

    fn bind_pat(&mut self, pat: &Pat) {
        match &pat.node {
            PatKind::Wildcard | PatKind::Lit(_) => {}
            PatKind::Ident(name) => self.bind(name, pat.span),
            PatKind::Variant { .. } | PatKind::Struct { .. } => {
                self.resolve_pat_path(pat);
                for sub in pat_subpatterns(pat) {
                    self.bind_pat(sub);
                }
            }
            PatKind::Tuple(elems) => {
                for e in elems {
                    self.bind_pat(e);
                }
            }
        }
    }

    /// Resolve a pattern path (variant or struct) in value position.
    fn resolve_pat_path(&mut self, pat: &Pat) {
        let path = match &pat.node {
            PatKind::Variant { path, .. } | PatKind::Struct { path, .. } => path,
            _ => return,
        };
        self.resolve_value_path(path, pat.span);
    }

    fn resolve_expr(&mut self, expr: &Expr) {
        match &expr.node {
            ExprKind::Lit(_) => {}
            ExprKind::Path(segments) => {
                self.resolve_value_path(segments, expr.span);
            }
            ExprKind::Binary { lhs, rhs, .. } => {
                self.resolve_expr(lhs);
                self.resolve_expr(rhs);
            }
            ExprKind::Unary { operand, .. }
            | ExprKind::Deref(operand)
            | ExprKind::AddrOf { operand, .. } => self.resolve_expr(operand),
            ExprKind::Cast { expr, ty } => {
                self.resolve_expr(expr);
                self.resolve_type(ty, &HashSet::new());
            }
            ExprKind::Assign { target, value, .. } => {
                self.resolve_expr(target);
                self.resolve_expr(value);
            }
            ExprKind::Call { callee, args } => {
                self.resolve_expr(callee);
                for a in args {
                    self.resolve_expr(a);
                }
            }
            ExprKind::Field { recv, .. } => self.resolve_expr(recv),
            ExprKind::If { cond, then, els } => {
                self.resolve_expr(cond);
                self.resolve_block(then);
                if let Some(els) = els {
                    self.resolve_expr(els);
                }
            }
            ExprKind::Match { scrut, arms } => {
                self.resolve_expr(scrut);
                for arm in arms {
                    self.ribs.push(HashMap::new());
                    self.bind_pat(&arm.pat);
                    self.resolve_expr(&arm.body);
                    self.ribs.pop();
                }
            }
            ExprKind::Block(block) => self.resolve_block(block),
            ExprKind::Tuple(elems) => {
                for e in elems {
                    self.resolve_expr(e);
                }
            }
            ExprKind::Unit => {}
        }
    }

    /// Resolve a value-position path to a [`ValueName`], recording it or
    /// reporting E200.
    fn resolve_value_path(&mut self, segments: &[String], span: Span) {
        let key = span_pair(span);
        if segments.len() == 1 {
            let name = &segments[0];
            if self.lookup_local(name) {
                self.resolved.values.insert(key, ValueName::Local);
                return;
            }
            if self.resolved.fns.contains_key(name) {
                self.resolved
                    .values
                    .insert(key, ValueName::Fn(name.clone()));
                return;
            }
            if name == "println" {
                self.resolved.values.insert(key, ValueName::BuiltinPrintln);
                return;
            }
            // Dangling variant: an Uppercase name resolving to a variant
            // records its parent enum; the checker verifies membership.
            // A struct name records a struct head for struct patterns.
            if starts_uppercase(name) {
                let mut hits = Vec::new();
                for (en, e) in &self.resolved.enums {
                    if e.variants.iter().any(|v| v.name == *name) {
                        hits.push(en.clone());
                    }
                }
                if hits.len() == 1 {
                    self.resolved.values.insert(
                        key,
                        ValueName::Variant {
                            en: hits.pop().unwrap_or_default(),
                            variant: name.clone(),
                        },
                    );
                    return;
                }
                if hits.len() > 1 {
                    self.error(
                        "E200",
                        "unresolved name".to_string(),
                        span,
                        format!("variant `{name}` is ambiguous"),
                    );
                    return;
                }
                if self.resolved.structs.contains_key(name) {
                    self.resolved
                        .values
                        .insert(key, ValueName::Struct(name.clone()));
                    return;
                }
            }
            self.error(
                "E200",
                "unresolved name".to_string(),
                span,
                format!("no value named `{name}` in scope"),
            );
            return;
        }
        if segments.len() == 2 {
            let head = &segments[0];
            let tail = &segments[1];
            if self.resolved.enums.contains_key(head) || self.resolved.structs.contains_key(head) {
                self.resolved.values.insert(
                    key,
                    ValueName::Variant {
                        en: head.clone(),
                        variant: tail.clone(),
                    },
                );
                return;
            }
        }
        self.error(
            "E200",
            "unresolved name".to_string(),
            span,
            format!("no value named `{}` in scope", segments.join("::")),
        );
    }
}

/// Sub-patterns of a variant or struct pattern.
fn pat_subpatterns(pat: &Pat) -> Vec<&Pat> {
    match &pat.node {
        PatKind::Variant { args, .. } => args.iter().collect(),
        PatKind::Struct { fields, .. } => fields.iter().map(|f| &f.pat).collect(),
        _ => Vec::new(),
    }
}

fn starts_uppercase(name: &str) -> bool {
    name.chars().next().is_some_and(|c| c.is_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lobster_lexer::lex;
    use lobster_parser::parse;
    use lobster_source::SourceManager;

    fn resolve_text(text: &str) -> (Resolved, Vec<Diagnostic>) {
        let mut sm = SourceManager::new();
        let id = sm.add_file("t.lobster", text);
        let lexed = lex(&sm, id, text);
        assert!(lexed.diagnostics.is_empty());
        let parsed = parse(id, &lexed.tokens);
        assert!(parsed.diagnostics.is_empty());
        resolve(&parsed.file)
    }

    fn codes(diags: &[Diagnostic]) -> Vec<String> {
        diags
            .iter()
            .map(|d| d.code.as_ref().map(|c| c.0.clone()).unwrap_or_default())
            .collect()
    }

    #[test]
    fn locals_params_and_fns_resolve() {
        let (r, diags) = resolve_text(
            "fn add(a: u32, b: u32) -> u32 { a + b }\nfn main() { add(1u32, 2u32); }\n",
        );
        assert!(diags.is_empty(), "{diags:?}");
        assert!(r.fns.contains_key("add"));
        assert!(r
            .values
            .values()
            .any(|v| matches!(v, ValueName::Fn(n) if n == "add")));
        assert!(r.values.values().any(|v| matches!(v, ValueName::Local)));
    }

    #[test]
    fn unknown_value_is_e200() {
        let (_, diags) = resolve_text("fn main() { nosuchfn(); }\n");
        assert_eq!(codes(&diags), ["E200"]);
    }

    #[test]
    fn duplicate_item_is_e201() {
        let (_, diags) = resolve_text("fn f() {}\nfn f() {}\n");
        assert_eq!(codes(&diags), ["E201"]);
    }

    #[test]
    fn duplicate_param_is_e201() {
        let (_, diags) = resolve_text("fn f(x: u32, x: u32) {}\n");
        assert!(codes(&diags).contains(&"E201".to_string()));
    }

    #[test]
    fn import_is_e202() {
        let (_, diags) = resolve_text("import math::sqrt;\n");
        assert_eq!(codes(&diags), ["E202"]);
    }

    #[test]
    fn unknown_type_is_e203() {
        let (_, diags) = resolve_text("fn f(x: Nope) {}\n");
        assert_eq!(codes(&diags), ["E203"]);
    }

    #[test]
    fn dangling_variant_records_parent() {
        let (r, diags) =
            resolve_text("enum Dir { North, South, }\nfn f(d: Dir) -> Dir { North }\n");
        assert!(diags.is_empty(), "{diags:?}");
        assert!(r.values.values().any(
            |v| matches!(v, ValueName::Variant { en, variant } if en == "Dir" && variant == "North")
        ));
    }

    #[test]
    fn qualified_variant_records_without_membership_check() {
        // `Dir::Up` does not exist; the checker reports E218, not E200.
        let (r, diags) = resolve_text("enum Dir { North, }\nfn f() { Dir::Up; }\n");
        assert!(diags.is_empty(), "{diags:?}");
        assert!(r.values.values().any(
            |v| matches!(v, ValueName::Variant { en, variant } if en == "Dir" && variant == "Up")
        ));
    }

    #[test]
    fn println_is_builtin() {
        let (r, diags) = resolve_text("fn main() { println(1u32); }\n");
        assert!(diags.is_empty(), "{diags:?}");
        assert!(r
            .values
            .values()
            .any(|v| matches!(v, ValueName::BuiltinPrintln)));
    }
}
