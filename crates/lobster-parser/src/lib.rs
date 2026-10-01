//! Lobster parser: recursive descent plus Pratt expressions, with recovery.
//!
//! Recursive descent for items/statements plus Pratt expressions, with
//! recovery: every error is collected as a [`Diagnostic`] and parsing
//! resumes at the next `;`, `}`, or item/statement boundary. A file with
//! syntax errors still yields a partial [`File`] for IDE tooling.
//!
//! Parser diagnostic codes:
//!
//! ```text
//! E110 unexpected token
//! E111 expected X, found Y
//! E112 supported-language subset exceeded (honest "not yet" errors)
//! ```
//!
//! Precedence (loosest to tightest) follows the table in [`infix_binding`]:
//! assignment, `||`, `&&`, comparison, `|`, `^`, `&`, shifts, addition,
//! multiplication, `as`, unary, postfix call/field. In particular
//! `a + b * c` parses as `ADD(a, MUL(b, c))`.

use lobster_ast::*;
use lobster_diagnostics::{Diagnostic, Label};
use lobster_lexer::{Keyword, Token, TokenKind};
use lobster_source::{FileId, Span};

/// Parser output: a (possibly partial) AST plus diagnostics in order.
pub struct ParseOutput {
    /// Parsed file; partial when diagnostics are present.
    pub file: File,
    /// Syntax errors in source order.
    pub diagnostics: Vec<Diagnostic>,
}

/// Parse lexed `tokens` (from `file`) into an AST and diagnostics.
#[must_use]
pub fn parse(file: FileId, tokens: &[Token]) -> ParseOutput {
    Parser::new(file, tokens).run()
}

/// Item-start keywords, used as recovery boundaries.
const ITEM_KW: &[Keyword] = &[
    Keyword::Fn,
    Keyword::Struct,
    Keyword::Enum,
    Keyword::Import,
    Keyword::Pub,
    Keyword::Module,
];

/// Statement-start keywords, used as recovery boundaries inside blocks.
const STMT_KW: &[Keyword] = &[
    Keyword::Let,
    Keyword::Return,
    Keyword::While,
    Keyword::Loop,
    Keyword::For,
    Keyword::Break,
    Keyword::Continue,
];

struct Parser<'a> {
    file: FileId,
    tokens: &'a [Token],
    pos: usize,
    /// End offset of the last consumed token (for span ends).
    prev_end: u32,
    /// Pending `>` from a split `>>` inside generic arguments.
    pending_gt: bool,
    diagnostics: Vec<Diagnostic>,
}

impl<'a> Parser<'a> {
    fn new(file: FileId, tokens: &'a [Token]) -> Self {
        Self {
            file,
            tokens,
            pos: 0,
            prev_end: 0,
            pending_gt: false,
            diagnostics: Vec::new(),
        }
    }

    fn run(mut self) -> ParseOutput {
        let mut items = Vec::new();
        while !self.at_end() && !self.too_many_errors() {
            let mark = self.pos;
            if let Some(item) = self.parse_item() {
                items.push(item);
            } else if self.pos == mark {
                // The error path consumed nothing (e.g. a stray `}` or a
                // failed `?` propagation). Skip one token so recovery
                // always terminates; the error is already recorded.
                self.bump();
            }
        }
        ParseOutput {
            file: File { items },
            diagnostics: self.diagnostics,
        }
    }

    /// True once error recovery should give up (adversarial-input guard).
    fn too_many_errors(&self) -> bool {
        self.diagnostics.len() > 1024
    }

    // ----- cursor -----

    fn at_end(&self) -> bool {
        self.pos >= self.tokens.len()
    }

    fn peek(&self) -> Option<&'a Token> {
        self.tokens.get(self.pos)
    }

    fn peek_kind(&self) -> Option<&'a TokenKind> {
        self.peek().map(|t| &t.kind)
    }

    fn at(&self, kind: &TokenKind) -> bool {
        self.peek_kind() == Some(kind)
    }

    fn at_keyword(&self, kw: Keyword) -> bool {
        self.peek().is_some_and(|t| t.is_keyword(kw))
    }

    fn bump(&mut self) -> &'a Token {
        let t = &self.tokens[self.pos];
        self.pos += 1;
        self.prev_end = t.span.end;
        t
    }

    fn eat(&mut self, kind: &TokenKind) -> bool {
        if self.at(kind) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn span_from(&self, start: u32) -> Span {
        Span::new(self.file, start, self.prev_end.max(start))
    }

    fn here(&self) -> Span {
        match self.peek() {
            Some(t) => Span::empty(self.file, t.span.start),
            None => Span::empty(self.file, self.prev_end),
        }
    }

    fn error(&mut self, code: &str, message: String, span: Span, detail: String) {
        self.diagnostics.push(Diagnostic::error(
            code,
            message,
            Label::primary(span, detail),
        ));
    }

    fn unexpected(&mut self, what: &str) -> Span {
        let span = self.here();
        let found = self
            .peek()
            .map_or_else(|| "end of file".to_string(), |t| format!("`{}`", t.text));
        self.error(
            "E110",
            format!("unexpected token while parsing {what}"),
            span,
            format!("found {found}"),
        );
        span
    }

    fn expected(&mut self, want: &str) -> Span {
        let span = self.here();
        let found = self
            .peek()
            .map_or_else(|| "end of file".to_string(), |t| format!("`{}`", t.text));
        self.error(
            "E111",
            format!("expected {want}"),
            span,
            format!("found {found}"),
        );
        span
    }

    fn expect(&mut self, kind: &TokenKind, want: &str) -> bool {
        if self.eat(kind) {
            true
        } else {
            self.expected(want);
            false
        }
    }

    /// Skip to the next `;` (consumed), `}`, item/statement keyword, or EOF.
    fn synchronize(&mut self, items: bool) {
        while !self.at_end() {
            if self.at(&TokenKind::Semi) {
                self.bump();
                return;
            }
            if self.at(&TokenKind::CloseBrace) {
                return;
            }
            if let Some(t) = self.peek() {
                if let TokenKind::Keyword(kw) = t.kind {
                    if ITEM_KW.contains(&kw) || (!items && STMT_KW.contains(&kw)) {
                        return;
                    }
                }
            }
            self.bump();
        }
    }

    /// Consume `>` (or one half of a lexed `>>`) for generic arguments.
    fn eat_gt(&mut self) -> bool {
        if self.pending_gt {
            self.pending_gt = false;
            return true;
        }
        match self.peek_kind() {
            Some(TokenKind::Gt) => {
                self.bump();
                true
            }
            Some(TokenKind::GtGt) => {
                let t = self.bump();
                // Split `>>`: consume one `>`, remember the other. The span
                // end stays approximate; generic-arg spans are best-effort.
                self.prev_end = t.span.start + 1;
                self.pending_gt = true;
                true
            }
            _ => false,
        }
    }

    // ----- items -----

    fn parse_item(&mut self) -> Option<Item> {
        let start_t = self.peek()?;
        let start = start_t.span.start;
        // `@attr` items (e.g. `@test`): honest subset error for now.
        if self.at(&TokenKind::At) {
            let span = self.bump().span;
            self.error(
                "E112",
                "attributes are not supported yet".to_string(),
                span,
                "e.g. `@test` arrives with the Commit 19 test framework".to_string(),
            );
            self.synchronize(true);
            return None;
        }
        let is_public = self.at_keyword(Keyword::Pub)
            && self.tokens.get(self.pos + 1).is_some_and(|t| {
                matches!(
                    t.kind,
                    TokenKind::Keyword(Keyword::Fn | Keyword::Struct | Keyword::Enum)
                )
            });
        if is_public {
            self.bump();
        }
        if self.at_end() {
            return None;
        }
        let kw = match self.peek_kind() {
            Some(TokenKind::Keyword(k)) => *k,
            _ => {
                self.unexpected("item");
                self.synchronize(true);
                return None;
            }
        };
        match kw {
            Keyword::Fn => self.parse_fn(is_public, start),
            Keyword::Struct => self.parse_struct(is_public, start),
            Keyword::Enum => self.parse_enum(is_public, start),
            Keyword::Import => self.parse_import(start),
            Keyword::Module => {
                let span = self.bump().span;
                self.error(
                    "E112",
                    "module blocks are not supported yet".to_string(),
                    span,
                    "one file is one module until Commit 03 name resolution".to_string(),
                );
                self.synchronize(true);
                None
            }
            _ => {
                self.unexpected("item");
                self.synchronize(true);
                None
            }
        }
    }

    fn parse_ident(&mut self, want: &str) -> Option<(String, Span)> {
        match self.peek() {
            Some(t) if matches!(t.kind, TokenKind::Ident) => {
                let t = self.bump();
                Some((t.text.clone(), t.span))
            }
            _ => {
                self.expected(want);
                None
            }
        }
    }

    fn parse_fn(&mut self, is_public: bool, start: u32) -> Option<Item> {
        self.bump(); // fn
        let (name, _) = self.parse_ident("function name")?;
        let generics = self.parse_generic_params();
        self.expect(&TokenKind::OpenParen, "`(`");
        let mut params = Vec::new();
        while !self.at(&TokenKind::CloseParen) && !self.at_end() {
            let pstart = self.peek()?.span.start;
            let (pname, pspan) = self.parse_ident("parameter name")?;
            self.expect(&TokenKind::Colon, "`:` after parameter name");
            let ty = self.parse_ty()?;
            params.push(FnParam {
                name: pname,
                span: pspan,
                ty,
            });
            if !self.eat(&TokenKind::Comma) {
                break;
            }
            let _ = pstart;
        }
        self.expect(&TokenKind::CloseParen, "`)`");
        let ret = if self.eat(&TokenKind::Arrow) {
            Some(self.parse_ty()?)
        } else {
            None
        };
        let body = self.parse_block()?;
        Some(Item::new(
            ItemKind::Fn(FnItem {
                is_public,
                name,
                generics,
                params,
                ret,
                body,
            }),
            self.span_from(start),
        ))
    }

    fn parse_generic_params(&mut self) -> Vec<GenericParam> {
        if !self.eat(&TokenKind::Lt) {
            return Vec::new();
        }
        let mut out = Vec::new();
        loop {
            if self.eat_gt() || self.at_end() {
                break;
            }
            let (name, span) = match self.parse_ident("generic parameter") {
                Some(v) => v,
                None => {
                    self.synchronize(true);
                    break;
                }
            };
            let mut bounds = Vec::new();
            if self.eat(&TokenKind::Colon) {
                while let Some(t) = self.parse_ty() {
                    bounds.push(t);
                    if !self.eat(&TokenKind::Plus) {
                        break;
                    }
                }
            }
            out.push(GenericParam { name, span, bounds });
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        if !self.eat_gt() {
            self.expected("`>`");
        }
        out
    }

    fn parse_struct(&mut self, is_public: bool, start: u32) -> Option<Item> {
        self.bump(); // struct
        let (name, _) = self.parse_ident("struct name")?;
        let generics = self.parse_generic_params();
        self.expect(&TokenKind::OpenBrace, "`{`");
        let mut fields = Vec::new();
        while !self.at(&TokenKind::CloseBrace) && !self.at_end() {
            let (fname, fspan) = match self.parse_ident("field name") {
                Some(v) => v,
                None => {
                    self.synchronize(true);
                    break;
                }
            };
            self.expect(&TokenKind::Colon, "`:` after field name");
            let ty = match self.parse_ty() {
                Some(t) => t,
                None => {
                    self.synchronize(true);
                    break;
                }
            };
            fields.push(FieldDecl {
                name: fname,
                span: fspan,
                ty,
            });
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        self.expect(&TokenKind::CloseBrace, "`}`");
        Some(Item::new(
            ItemKind::Struct(StructItem {
                is_public,
                name,
                generics,
                fields,
            }),
            self.span_from(start),
        ))
    }

    fn parse_enum(&mut self, is_public: bool, start: u32) -> Option<Item> {
        self.bump(); // enum
        let (name, _) = self.parse_ident("enum name")?;
        let generics = self.parse_generic_params();
        self.expect(&TokenKind::OpenBrace, "`{`");
        let mut variants = Vec::new();
        while !self.at(&TokenKind::CloseBrace) && !self.at_end() {
            let (vname, vspan) = match self.parse_ident("variant name") {
                Some(v) => v,
                None => {
                    self.synchronize(true);
                    break;
                }
            };
            let mut payload = Vec::new();
            if self.eat(&TokenKind::OpenParen) {
                while !self.at(&TokenKind::CloseParen) && !self.at_end() {
                    match self.parse_ty() {
                        Some(t) => payload.push(t),
                        None => {
                            self.synchronize(true);
                            break;
                        }
                    }
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                }
                self.expect(&TokenKind::CloseParen, "`)`");
            }
            variants.push(EnumVariant {
                name: vname,
                span: vspan,
                payload,
            });
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        self.expect(&TokenKind::CloseBrace, "`}`");
        Some(Item::new(
            ItemKind::Enum(EnumItem {
                is_public,
                name,
                generics,
                variants,
            }),
            self.span_from(start),
        ))
    }

    fn parse_import(&mut self, start: u32) -> Option<Item> {
        self.bump(); // import
        let mut path = Vec::new();
        loop {
            match self.parse_ident("import path segment") {
                Some((seg, _)) => path.push(seg),
                None => {
                    self.synchronize(true);
                    return None;
                }
            }
            if !self.eat(&TokenKind::ColonColon) {
                break;
            }
        }
        self.expect(&TokenKind::Semi, "`;`");
        Some(Item::new(
            ItemKind::Import(ImportItem { path }),
            self.span_from(start),
        ))
    }

    // ----- blocks and statements -----

    fn parse_block(&mut self) -> Option<Block> {
        let open = match self.peek() {
            Some(t) if matches!(t.kind, TokenKind::OpenBrace) => self.bump().span.start,
            _ => {
                self.expected("`{`");
                return None;
            }
        };
        let mut stmts = Vec::new();
        let mut tail = None;
        while !self.at(&TokenKind::CloseBrace) && !self.at_end() && !self.too_many_errors() {
            let mark = self.pos;
            // Tolerate stray semicolons.
            if self.eat(&TokenKind::Semi) {
                continue;
            }
            let kw = match self.peek_kind() {
                Some(TokenKind::Keyword(k)) => Some(*k),
                _ => None,
            };
            match kw {
                Some(
                    Keyword::Let
                    | Keyword::Return
                    | Keyword::While
                    | Keyword::Loop
                    | Keyword::For
                    | Keyword::Break
                    | Keyword::Continue,
                ) => match self.parse_stmt() {
                    Some(s) => stmts.push(s),
                    None => self.synchronize(false),
                },
                Some(Keyword::Fn | Keyword::Struct | Keyword::Enum | Keyword::Import) => {
                    let span = self.here();
                    self.error(
                        "E112",
                        "nested items are not supported yet".to_string(),
                        span,
                        "move the item to the top level".to_string(),
                    );
                    self.synchronize(false);
                }
                _ => {
                    let estart = self.peek()?.span.start;
                    match self.parse_expr(0) {
                        Some(e) => {
                            // Block-like expressions (`if`, `match`, `{ }`)
                            // are statements without a `;`, like in Rust;
                            // anything else without `;` is the block tail.
                            let block_like = matches!(
                                e.node,
                                ExprKind::Block(_) | ExprKind::If { .. } | ExprKind::Match { .. }
                            );
                            if self.eat(&TokenKind::Semi) || block_like {
                                stmts.push(Stmt::new(StmtKind::Expr(e), self.span_from(estart)));
                            } else {
                                tail = Some(Box::new(e));
                                break;
                            }
                        }
                        None => self.synchronize(false),
                    }
                }
            }
            if self.pos == mark && !self.at_end() && !self.at(&TokenKind::CloseBrace) {
                // No progress (e.g. a `?` propagation after an error was
                // already recorded). Skip one token so recovery terminates.
                self.bump();
            }
        }
        if !self.eat(&TokenKind::CloseBrace) {
            self.expected("`}`");
        }
        let _ = open;
        Some(Block {
            stmts,
            tail,
            span: self.span_from(open),
        })
    }

    fn parse_stmt(&mut self) -> Option<Stmt> {
        let start = self.peek()?.span.start;
        let kw = match self.peek_kind() {
            Some(TokenKind::Keyword(k)) => *k,
            _ => {
                self.unexpected("statement");
                return None;
            }
        };
        let kind = match kw {
            Keyword::Let => {
                self.bump();
                let mutable = self.at_keyword(Keyword::Mut) && {
                    self.bump();
                    true
                };
                let (name, span) = self.parse_ident("variable name")?;
                let ty = if self.eat(&TokenKind::Colon) {
                    Some(self.parse_ty()?)
                } else {
                    None
                };
                let init = if self.eat(&TokenKind::Eq) {
                    Some(self.parse_expr(0)?)
                } else {
                    None
                };
                self.expect(&TokenKind::Semi, "`;`");
                StmtKind::Let(LetStmt {
                    mutable,
                    name,
                    span,
                    ty,
                    init,
                })
            }
            Keyword::Return => {
                self.bump();
                let value = if self.at(&TokenKind::Semi) {
                    None
                } else {
                    Some(self.parse_expr(0)?)
                };
                self.expect(&TokenKind::Semi, "`;`");
                StmtKind::Return(value)
            }
            Keyword::While => {
                self.bump();
                let cond = self.parse_expr(0)?;
                let body = self.parse_block()?;
                StmtKind::While { cond, body }
            }
            Keyword::Loop => {
                self.bump();
                StmtKind::Loop(self.parse_block()?)
            }
            Keyword::For => {
                self.bump();
                let pat = self.parse_pat()?;
                if !self.at_keyword(Keyword::In) {
                    self.expected("`in`");
                    return None;
                }
                self.bump();
                let iter = self.parse_expr(0)?;
                let body = self.parse_block()?;
                StmtKind::For { pat, iter, body }
            }
            Keyword::Break => {
                self.bump();
                self.expect(&TokenKind::Semi, "`;`");
                StmtKind::Break
            }
            Keyword::Continue => {
                self.bump();
                self.expect(&TokenKind::Semi, "`;`");
                StmtKind::Continue
            }
            _ => {
                self.unexpected("statement");
                return None;
            }
        };
        Some(Stmt::new(kind, self.span_from(start)))
    }

    // ----- types -----

    fn parse_ty(&mut self) -> Option<Ty> {
        let start = self.peek()?.span.start;
        // `&T`, `&mut T`
        if self.eat(&TokenKind::Amp) {
            let mutable = self.at_keyword(Keyword::Mut) && {
                self.bump();
                true
            };
            let inner = self.parse_ty()?;
            return Some(Ty::new(
                TyKind::Ref {
                    mutable,
                    inner: Box::new(inner),
                },
                self.span_from(start),
            ));
        }
        // `*const T`, `*mut T`
        if self.eat(&TokenKind::Star) {
            let mutable = if self.at_keyword(Keyword::Mut) {
                self.bump();
                true
            } else if self.is_ident("const") {
                self.bump();
                false
            } else {
                self.expected("`mut` or `const`");
                return None;
            };
            let inner = self.parse_ty()?;
            return Some(Ty::new(
                TyKind::Ptr {
                    mutable,
                    inner: Box::new(inner),
                },
                self.span_from(start),
            ));
        }
        // `[T]`, `[T; N]`
        if self.eat(&TokenKind::OpenBracket) {
            let elem = self.parse_ty()?;
            let kind = if self.eat(&TokenKind::Semi) {
                let len = self.parse_expr(0)?;
                self.expect(&TokenKind::CloseBracket, "`]`");
                TyKind::Array {
                    elem: Box::new(elem),
                    len: Box::new(len),
                }
            } else {
                self.expect(&TokenKind::CloseBracket, "`]`");
                TyKind::Slice(Box::new(elem))
            };
            return Some(Ty::new(kind, self.span_from(start)));
        }
        // `(A, B)`, `()`
        if self.eat(&TokenKind::OpenParen) {
            if self.eat(&TokenKind::CloseParen) {
                return Some(Ty::new(TyKind::Unit, self.span_from(start)));
            }
            let first = self.parse_ty()?;
            if !self.eat(&TokenKind::Comma) {
                self.expect(&TokenKind::CloseParen, "`)`");
                return Some(first);
            }
            let mut elems = vec![first];
            while !self.at(&TokenKind::CloseParen) && !self.at_end() {
                match self.parse_ty() {
                    Some(t) => elems.push(t),
                    None => {
                        self.synchronize(false);
                        return None;
                    }
                }
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
            }
            self.expect(&TokenKind::CloseParen, "`)`");
            return Some(Ty::new(TyKind::Tuple(elems), self.span_from(start)));
        }
        // Path with optional `<args>`.
        let mut segments = vec![self.parse_ty_segment()?];
        while self.eat(&TokenKind::ColonColon) {
            segments.push(self.parse_ty_segment()?);
        }
        let mut args = Vec::new();
        if self.eat(&TokenKind::Lt) {
            loop {
                if self.eat_gt() || self.at_end() {
                    break;
                }
                match self.parse_ty() {
                    Some(t) => args.push(t),
                    None => {
                        self.synchronize(false);
                        return None;
                    }
                }
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
            }
            if !self.eat_gt() {
                self.expected("`>`");
                return None;
            }
        }
        Some(Ty::new(
            TyKind::Path { segments, args },
            self.span_from(start),
        ))
    }

    fn parse_ty_segment(&mut self) -> Option<String> {
        match self.peek() {
            Some(t) if matches!(t.kind, TokenKind::Ident) => Some(self.bump().text.clone()),
            Some(t) if matches!(t.kind, TokenKind::Keyword(Keyword::SelfType)) => {
                Some(self.bump().text.clone())
            }
            _ => {
                self.expected("type name");
                None
            }
        }
    }

    fn is_ident(&self, name: &str) -> bool {
        self.peek().is_some_and(|t| t.is_ident(name))
    }

    // ----- expressions (Pratt) -----

    /// Parse an expression with operands binding at `min_prec` or tighter.
    fn parse_expr(&mut self, min_prec: u32) -> Option<Expr> {
        let start = self.peek()?.span.start;
        let mut lhs = self.parse_prefix()?;
        while let Some(op) = self.peek_infix_op() {
            let (prec, right_assoc) = infix_binding(&op);
            if prec < min_prec {
                break;
            }
            self.bump(); // operator
                         // Assignment: `=` and compound forms, right-associative.
            if let Infix::Assign(assign_op) = op {
                let rhs = self.parse_expr(prec)?;
                let span = self.span_from(start);
                lhs = Expr::new(
                    ExprKind::Assign {
                        target: Box::new(lhs),
                        op: assign_op,
                        value: Box::new(rhs),
                    },
                    span,
                );
                continue;
            }
            if op == Infix::Cast {
                let ty = self.parse_ty()?;
                let span = self.span_from(start);
                lhs = Expr::new(
                    ExprKind::Cast {
                        expr: Box::new(lhs),
                        ty,
                    },
                    span,
                );
                continue;
            }
            let next_min = if right_assoc { prec } else { prec + 1 };
            let rhs = self.parse_expr(next_min)?;
            let span = self.span_from(start);
            let bin = match op {
                Infix::Bin(b) => b,
                Infix::Assign(_) | Infix::Cast => unreachable!("handled above"),
            };
            lhs = Expr::new(
                ExprKind::Binary {
                    op: bin,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                span,
            );
        }
        Some(lhs)
    }

    fn parse_prefix(&mut self) -> Option<Expr> {
        let start = self.peek()?.span.start;
        // Unary `-`, `!`.
        if self.eat(&TokenKind::Minus) {
            let operand = self.parse_expr(prefix_prec())?;
            return Some(Expr::new(
                ExprKind::Unary {
                    op: UnOp::Neg,
                    operand: Box::new(operand),
                },
                self.span_from(start),
            ));
        }
        if self.eat(&TokenKind::Bang) {
            let operand = self.parse_expr(prefix_prec())?;
            return Some(Expr::new(
                ExprKind::Unary {
                    op: UnOp::Not,
                    operand: Box::new(operand),
                },
                self.span_from(start),
            ));
        }
        // `&` / `&mut` borrow.
        if self.eat(&TokenKind::Amp) {
            let mutable = self.at_keyword(Keyword::Mut) && {
                self.bump();
                true
            };
            let operand = self.parse_expr(prefix_prec())?;
            return Some(Expr::new(
                ExprKind::AddrOf {
                    mutable,
                    operand: Box::new(operand),
                },
                self.span_from(start),
            ));
        }
        // `*` dereference.
        if self.eat(&TokenKind::Star) {
            let operand = self.parse_expr(prefix_prec())?;
            return Some(Expr::new(
                ExprKind::Deref(Box::new(operand)),
                self.span_from(start),
            ));
        }
        let mut node = self.parse_primary()?;
        // Postfix `(` args `)` and `.field` bind tightest, left-assoc.
        loop {
            if self.eat(&TokenKind::OpenParen) {
                let mut args = Vec::new();
                while !self.at(&TokenKind::CloseParen) && !self.at_end() {
                    match self.parse_expr(0) {
                        Some(a) => args.push(a),
                        None => {
                            self.synchronize(false);
                            return None;
                        }
                    }
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                }
                self.expect(&TokenKind::CloseParen, "`)`");
                let span = self.span_from(start);
                node = Expr::new(
                    ExprKind::Call {
                        callee: Box::new(node),
                        args,
                    },
                    span,
                );
            } else if self.eat(&TokenKind::Dot) {
                let (field, _) = self.parse_ident("field name")?;
                let span = self.span_from(start);
                node = Expr::new(
                    ExprKind::Field {
                        recv: Box::new(node),
                        field,
                    },
                    span,
                );
            } else {
                break;
            }
        }
        Some(node)
    }

    fn parse_primary(&mut self) -> Option<Expr> {
        let start = self.peek()?.span.start;
        let kind = self.peek_kind()?.clone();
        match kind {
            TokenKind::Int => {
                let t = self.bump();
                Some(Expr::new(
                    ExprKind::Lit(Literal::Int(t.text.clone())),
                    self.span_from(start),
                ))
            }
            TokenKind::Float => {
                let t = self.bump();
                Some(Expr::new(
                    ExprKind::Lit(Literal::Float(t.text.clone())),
                    self.span_from(start),
                ))
            }
            TokenKind::Str(s) => {
                self.bump();
                Some(Expr::new(
                    ExprKind::Lit(Literal::Str(s)),
                    self.span_from(start),
                ))
            }
            TokenKind::Char(c) => {
                self.bump();
                Some(Expr::new(
                    ExprKind::Lit(Literal::Char(c)),
                    self.span_from(start),
                ))
            }
            TokenKind::Keyword(Keyword::True) => {
                self.bump();
                Some(Expr::new(
                    ExprKind::Lit(Literal::Bool(true)),
                    self.span_from(start),
                ))
            }
            TokenKind::Keyword(Keyword::False) => {
                self.bump();
                Some(Expr::new(
                    ExprKind::Lit(Literal::Bool(false)),
                    self.span_from(start),
                ))
            }
            TokenKind::Keyword(Keyword::If) => self.parse_if(),
            TokenKind::Keyword(Keyword::Match) => self.parse_match(),
            TokenKind::OpenBrace => {
                let block = self.parse_block()?;
                let span = self.span_from(start);
                Some(Expr::new(ExprKind::Block(block), span))
            }
            TokenKind::OpenParen => {
                self.bump();
                if self.eat(&TokenKind::CloseParen) {
                    return Some(Expr::new(ExprKind::Unit, self.span_from(start)));
                }
                let first = self.parse_expr(0)?;
                if !self.eat(&TokenKind::Comma) {
                    self.expect(&TokenKind::CloseParen, "`)`");
                    return Some(first);
                }
                let mut elems = vec![first];
                while !self.at(&TokenKind::CloseParen) && !self.at_end() {
                    match self.parse_expr(0) {
                        Some(e) => elems.push(e),
                        None => {
                            self.synchronize(false);
                            return None;
                        }
                    }
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                }
                self.expect(&TokenKind::CloseParen, "`)`");
                Some(Expr::new(ExprKind::Tuple(elems), self.span_from(start)))
            }
            TokenKind::Ident | TokenKind::Keyword(_) => {
                // Path: `x`, `self`, `Option::Some`.
                let mut segments = vec![self.bump().text.clone()];
                while self.eat(&TokenKind::ColonColon) {
                    match self.peek() {
                        Some(t)
                            if matches!(t.kind, TokenKind::Ident)
                                || matches!(
                                    t.kind,
                                    TokenKind::Keyword(Keyword::SelfType | Keyword::SelfValue)
                                ) =>
                        {
                            segments.push(self.bump().text.clone());
                        }
                        _ => {
                            self.expected("path segment");
                            return None;
                        }
                    }
                }
                Some(Expr::new(ExprKind::Path(segments), self.span_from(start)))
            }
            _ => {
                self.unexpected("expression");
                self.synchronize(false);
                None
            }
        }
    }

    fn parse_if(&mut self) -> Option<Expr> {
        let start = self.peek()?.span.start;
        self.bump(); // if
        let cond = self.parse_expr(0)?;
        let then = self.parse_block()?;
        let els = if self.at_keyword(Keyword::Else) {
            self.bump();
            if self.at_keyword(Keyword::If) {
                Some(Box::new(self.parse_if()?))
            } else {
                let eblock = self.parse_block()?;
                let espan = eblock.span;
                Some(Box::new(Expr::new(ExprKind::Block(eblock), espan)))
            }
        } else {
            None
        };
        Some(Expr::new(
            ExprKind::If {
                cond: Box::new(cond),
                then,
                els,
            },
            self.span_from(start),
        ))
    }

    fn parse_match(&mut self) -> Option<Expr> {
        let start = self.peek()?.span.start;
        self.bump(); // match
        let scrut = self.parse_expr(0)?;
        self.expect(&TokenKind::OpenBrace, "`{`");
        let mut arms = Vec::new();
        while !self.at(&TokenKind::CloseBrace) && !self.at_end() && !self.too_many_errors() {
            let mark = self.pos;
            let pat = match self.parse_pat() {
                Some(p) => p,
                None => {
                    self.synchronize(false);
                    continue;
                }
            };
            if !self.eat(&TokenKind::FatArrow) {
                self.expected("`=>`");
                self.synchronize(false);
                continue;
            }
            let body = match self.parse_expr(0) {
                Some(b) => b,
                None => {
                    self.synchronize(false);
                    continue;
                }
            };
            // Block bodies don't need a trailing comma; others do unless `}`.
            let is_block = matches!(body.node, ExprKind::Block(_));
            if !is_block && !self.at(&TokenKind::CloseBrace) {
                self.expect(&TokenKind::Comma, "`,`");
            } else {
                self.eat(&TokenKind::Comma);
            }
            arms.push(MatchArm { pat, body });
            if self.pos == mark && !self.at_end() && !self.at(&TokenKind::CloseBrace) {
                self.bump();
            }
        }
        self.expect(&TokenKind::CloseBrace, "`}`");
        Some(Expr::new(
            ExprKind::Match {
                scrut: Box::new(scrut),
                arms,
            },
            self.span_from(start),
        ))
    }

    fn peek_infix_op(&self) -> Option<Infix> {
        if self.pending_gt {
            return None;
        }
        let kind = self.peek_kind()?;
        Some(match kind {
            TokenKind::Plus => Infix::Bin(BinOp::Add),
            TokenKind::Minus => Infix::Bin(BinOp::Sub),
            TokenKind::Star => Infix::Bin(BinOp::Mul),
            TokenKind::Slash => Infix::Bin(BinOp::Div),
            TokenKind::Percent => Infix::Bin(BinOp::Rem),
            TokenKind::EqEq => Infix::Bin(BinOp::Eq),
            TokenKind::BangEq => Infix::Bin(BinOp::Ne),
            TokenKind::Lt => Infix::Bin(BinOp::Lt),
            TokenKind::LtEq => Infix::Bin(BinOp::Le),
            TokenKind::Gt => Infix::Bin(BinOp::Gt),
            TokenKind::GtEq => Infix::Bin(BinOp::Ge),
            TokenKind::AmpAmp => Infix::Bin(BinOp::And),
            TokenKind::PipePipe => Infix::Bin(BinOp::Or),
            TokenKind::Amp => Infix::Bin(BinOp::BitAnd),
            TokenKind::Pipe => Infix::Bin(BinOp::BitOr),
            TokenKind::Caret => Infix::Bin(BinOp::BitXor),
            TokenKind::LtLt => Infix::Bin(BinOp::Shl),
            TokenKind::GtGt => Infix::Bin(BinOp::Shr),
            TokenKind::Eq => Infix::Assign(None),
            TokenKind::PlusEq => Infix::Assign(Some(BinOp::Add)),
            TokenKind::MinusEq => Infix::Assign(Some(BinOp::Sub)),
            TokenKind::StarEq => Infix::Assign(Some(BinOp::Mul)),
            TokenKind::SlashEq => Infix::Assign(Some(BinOp::Div)),
            TokenKind::PercentEq => Infix::Assign(Some(BinOp::Rem)),
            TokenKind::AmpEq => Infix::Assign(Some(BinOp::BitAnd)),
            TokenKind::PipeEq => Infix::Assign(Some(BinOp::BitOr)),
            TokenKind::CaretEq => Infix::Assign(Some(BinOp::BitXor)),
            TokenKind::LtLtEq | TokenKind::GtGtEq => {
                // `<<=` / `>>=` after an expression: treat as shift-assign.
                let op = if matches!(kind, TokenKind::LtLtEq) {
                    BinOp::Shl
                } else {
                    BinOp::Shr
                };
                Infix::Assign(Some(op))
            }
            TokenKind::Keyword(Keyword::As) => Infix::Cast,
            _ => return None,
        })
    }

    // ----- patterns -----

    fn parse_pat(&mut self) -> Option<Pat> {
        let start = self.peek()?.span.start;
        // `_`
        if self.peek().is_some_and(|t| t.is_ident("_")) {
            self.bump();
            return Some(Pat::new(PatKind::Wildcard, self.span_from(start)));
        }
        // Tuple pattern.
        if self.eat(&TokenKind::OpenParen) {
            let mut elems = Vec::new();
            while !self.at(&TokenKind::CloseParen) && !self.at_end() {
                match self.parse_pat() {
                    Some(p) => elems.push(p),
                    None => {
                        self.synchronize(false);
                        return None;
                    }
                }
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
            }
            self.expect(&TokenKind::CloseParen, "`)`");
            return Some(Pat::new(PatKind::Tuple(elems), self.span_from(start)));
        }
        let kind = self.peek_kind()?.clone();
        match kind {
            TokenKind::Int => {
                let t = self.bump();
                Some(Pat::new(
                    PatKind::Lit(Literal::Int(t.text.clone())),
                    self.span_from(start),
                ))
            }
            TokenKind::Str(s) => {
                self.bump();
                Some(Pat::new(
                    PatKind::Lit(Literal::Str(s)),
                    self.span_from(start),
                ))
            }
            TokenKind::Char(c) => {
                self.bump();
                Some(Pat::new(
                    PatKind::Lit(Literal::Char(c)),
                    self.span_from(start),
                ))
            }
            TokenKind::Keyword(Keyword::True) => {
                self.bump();
                Some(Pat::new(
                    PatKind::Lit(Literal::Bool(true)),
                    self.span_from(start),
                ))
            }
            TokenKind::Keyword(Keyword::False) => {
                self.bump();
                Some(Pat::new(
                    PatKind::Lit(Literal::Bool(false)),
                    self.span_from(start),
                ))
            }
            TokenKind::Ident => {
                let t = self.bump();
                let name = t.text.clone();
                // `Name(...)` or multi-segment paths are variant patterns;
                // a lone lowercase name is a binding.
                let is_variant = name.chars().next().is_some_and(|c| c.is_uppercase())
                    || self.at(&TokenKind::ColonColon)
                    || self.at(&TokenKind::OpenParen);
                if !is_variant {
                    return Some(Pat::new(PatKind::Ident(name), self.span_from(start)));
                }
                let mut path = vec![name];
                while self.eat(&TokenKind::ColonColon) {
                    match self.peek() {
                        Some(t) if matches!(t.kind, TokenKind::Ident) => {
                            path.push(self.bump().text.clone());
                        }
                        _ => {
                            self.expected("pattern path segment");
                            return None;
                        }
                    }
                }
                let mut args = Vec::new();
                if self.eat(&TokenKind::OpenParen) {
                    while !self.at(&TokenKind::CloseParen) && !self.at_end() {
                        match self.parse_pat() {
                            Some(p) => args.push(p),
                            None => {
                                self.synchronize(false);
                                return None;
                            }
                        }
                        if !self.eat(&TokenKind::Comma) {
                            break;
                        }
                    }
                    self.expect(&TokenKind::CloseParen, "`)`");
                }
                Some(Pat::new(
                    PatKind::Variant { path, args },
                    self.span_from(start),
                ))
            }
            _ => {
                self.unexpected("pattern");
                None
            }
        }
    }
}

/// Infix operator classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Infix {
    /// Arithmetic / comparison / logic operators.
    Bin(BinOp),
    /// `=` or compound assignment; payload is the combined op, if any.
    Assign(Option<BinOp>),
    /// `as` casts.
    Cast,
}

/// Binding power of prefix operators (unary, borrow, deref).
const fn prefix_prec() -> u32 {
    90
}

/// Binding power and right-associativity of an infix operator.
/// Assignment is the loosest (10) and right-associative; everything else is
/// left-associative with the spec example (`a + b * c` →
/// `ADD(a, MUL(b, c))`) falling out of `*` binding tighter than `+`.
fn infix_binding(op: &Infix) -> (u32, bool) {
    match op {
        Infix::Assign(_) => (10, true),
        Infix::Bin(BinOp::Or) => (20, false),
        Infix::Bin(BinOp::And) => (30, false),
        Infix::Bin(BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge) => {
            (40, false)
        }
        Infix::Bin(BinOp::BitOr) => (50, false),
        Infix::Bin(BinOp::BitXor) => (55, false),
        Infix::Bin(BinOp::BitAnd) => (60, false),
        Infix::Bin(BinOp::Shl | BinOp::Shr) => (65, false),
        Infix::Bin(BinOp::Add | BinOp::Sub) => (70, false),
        Infix::Bin(BinOp::Mul | BinOp::Div | BinOp::Rem) => (80, false),
        Infix::Cast => (85, false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lobster_lexer::lex;
    use lobster_source::SourceManager;

    fn parse_text(text: &str) -> (SourceManager, ParseOutput, usize) {
        let mut sm = SourceManager::new();
        let id = sm.add_file("t.lobster", text);
        let lexed = lex(&sm, id, text);
        let lex_diags = lexed.diagnostics.len();
        let out = parse(id, &lexed.tokens);
        (sm, out, lex_diags)
    }

    fn parse_ok(text: &str) -> File {
        let (_sm, out, lex_diags) = parse_text(text);
        assert_eq!(lex_diags, 0, "lexer reported errors");
        assert!(
            out.diagnostics.is_empty(),
            "errors: {:?}",
            out.diagnostics
                .iter()
                .map(|d| &d.message)
                .collect::<Vec<_>>()
        );
        out.file
    }

    #[test]
    fn precedence_follows_spec_example() {
        // `a + b * c` must be ADD(a, MUL(b, c)).
        let file = parse_ok("fn f() { a + b * c; }");
        let body = match &file.items[0].node {
            ItemKind::Fn(f) => &f.body,
            _ => panic!("expected fn"),
        };
        let stmt = &body.stmts[0];
        let ExprKind::Binary {
            op: BinOp::Add,
            lhs,
            rhs,
        } = &stmt.node_expr().node
        else {
            panic!("expected top-level add, got {:?}", stmt.node_expr())
        };
        assert!(matches!(&lhs.node, ExprKind::Path(_)));
        let ExprKind::Binary { op: BinOp::Mul, .. } = &rhs.node else {
            panic!("expected nested mul, got {:?}", rhs.node)
        };
    }

    #[test]
    fn fib_example_parses() {
        let file = parse_ok(
            "fn fib(n: u64) -> u64 {\n    if n < 2 {\n        return n;\n    }\n\n    fib(n - 1) + fib(n - 2)\n}\n",
        );
        assert_eq!(file.items.len(), 1);
        let ItemKind::Fn(f) = &file.items[0].node else {
            panic!("expected fn")
        };
        assert_eq!(f.name, "fib");
        assert_eq!(f.params.len(), 1);
        assert!(f.body.tail.is_some());
    }

    #[test]
    fn structs_enums_imports_match() {
        let file = parse_ok(
            "import math::sqrt;\nstruct Point { x: f32, y: f32, }\nenum Option<T> { Some(T), None, }\nfn f(v: Option<u64>) -> u64 {\n    match v {\n        Some(x) => x,\n        None => 0,\n    }\n}\n",
        );
        assert_eq!(file.items.len(), 4);
        let ItemKind::Enum(e) = &file.items[2].node else {
            panic!("expected enum")
        };
        assert_eq!(e.variants.len(), 2);
        assert_eq!(e.generics.len(), 1);
    }

    #[test]
    fn missing_semicolon_recovers_and_reports_e111() {
        let (_sm, out, _) = parse_text("fn f() { let x = 1 }\nfn g() {}\n");
        assert!(out
            .diagnostics
            .iter()
            .any(|d| d.code.as_ref().is_some_and(|c| c.0 == "E111")));
        // Second item still parsed despite the first item's error.
        assert_eq!(out.file.items.len(), 2);
    }

    #[test]
    fn garbage_item_recovers_to_next_item() {
        let (_sm, out, _) = parse_text("fn f() { }\n@@@\nfn g() { }\n");
        assert!(!out.diagnostics.is_empty());
        assert_eq!(out.file.items.len(), 2);
    }

    #[test]
    fn unclosed_brace_reports_and_ends() {
        let (_sm, out, _) = parse_text("fn f() { let x = 1;");
        assert!(out
            .diagnostics
            .iter()
            .any(|d| d.code.as_ref().is_some_and(|c| c.0 == "E111")));
    }

    #[test]
    fn assignment_is_right_associative() {
        let file = parse_ok("fn f() { x = y = 1; }");
        let body = match &file.items[0].node {
            ItemKind::Fn(f) => &f.body,
            _ => panic!("expected fn"),
        };
        let ExprKind::Assign { value, .. } = &body.stmts[0].node_expr().node else {
            panic!("expected assign")
        };
        assert!(matches!(&value.node, ExprKind::Assign { .. }));
    }

    #[test]
    fn compound_assign_and_cast() {
        let file = parse_ok("fn f() { x += 1; y = x as u64; }");
        let body = match &file.items[0].node {
            ItemKind::Fn(f) => &f.body,
            _ => panic!("expected fn"),
        };
        assert_eq!(body.stmts.len(), 2);
        let ExprKind::Assign {
            op: Some(BinOp::Add),
            ..
        } = &body.stmts[0].node_expr().node
        else {
            panic!("expected += ")
        };
        assert!(matches!(
            &body.stmts[1].node_expr().node,
            ExprKind::Assign { .. }
        ));
    }

    // Helper: unwrap expression statements in tests.
    trait StmtExpr {
        fn node_expr(&self) -> &Expr;
    }

    impl StmtExpr for Stmt {
        fn node_expr(&self) -> &Expr {
            match &self.node {
                StmtKind::Expr(e) => e,
                other => panic!("expected expr stmt, got {other:?}"),
            }
        }
    }
}
