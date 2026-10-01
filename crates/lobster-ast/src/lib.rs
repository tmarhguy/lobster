//! Lobster abstract syntax tree: span-annotated nodes close to the source surface.
//!
//! Every node that can appear in a diagnostic carries its [`Span`].
//! The tree is intentionally close to the source surface: desugaring
//! (`a && b()` into control flow, etc.) happens later in HIR (Commit 04),
//! so short-circuit operators and `if` survive here as written.

use lobster_source::Span;

/// A syntax node paired with its source span.
#[derive(Debug, Clone, PartialEq)]
pub struct Spanned<T> {
    /// The node itself.
    pub node: T,
    /// Source range covering the node.
    pub span: Span,
}

impl<T> Spanned<T> {
    /// Attach a span to a node.
    pub const fn new(node: T, span: Span) -> Self {
        Self { node, span }
    }
}

/// Shorthand for a spanned expression.
pub type Expr = Spanned<ExprKind>;
/// Shorthand for a spanned statement.
pub type Stmt = Spanned<StmtKind>;
/// Shorthand for a spanned item.
pub type Item = Spanned<ItemKind>;
/// Shorthand for a spanned type.
pub type Ty = Spanned<TyKind>;
/// Shorthand for a spanned pattern.
pub type Pat = Spanned<PatKind>;

/// A complete parsed file.
#[derive(Debug, Clone, PartialEq)]
pub struct File {
    /// Top-level items in source order.
    pub items: Vec<Item>,
}

/// Top-level item.
#[derive(Debug, Clone, PartialEq)]
pub enum ItemKind {
    /// `fn name<T>(params) -> Ret { body }`, optionally `pub`.
    Fn(FnItem),
    /// `struct Name<T> { field: Ty, ... }`, optionally `pub`.
    Struct(StructItem),
    /// `enum Name<T> { Variant(payload), ... }`, optionally `pub`.
    Enum(EnumItem),
    /// `import math::sqrt;`
    Import(ImportItem),
}

impl ItemKind {
    /// True when prefixed with `pub`.
    #[must_use]
    pub const fn is_public(&self) -> bool {
        match self {
            Self::Fn(f) => f.is_public,
            Self::Struct(s) => s.is_public,
            Self::Enum(e) => e.is_public,
            Self::Import(_) => false,
        }
    }
}

/// Function item.
#[derive(Debug, Clone, PartialEq)]
pub struct FnItem {
    /// `true` for `pub fn`.
    pub is_public: bool,
    /// Function name.
    pub name: String,
    /// Generic parameters (`<T: Ord>` keeps bounds as written).
    pub generics: Vec<GenericParam>,
    /// `(name: Ty, ...)` parameters.
    pub params: Vec<FnParam>,
    /// Return type after `->`, if any.
    pub ret: Option<Ty>,
    /// Function body.
    pub body: Block,
}

/// One function parameter.
#[derive(Debug, Clone, PartialEq)]
pub struct FnParam {
    /// Parameter name.
    pub name: String,
    /// Span of the name (for duplicate/undefined diagnostics later).
    pub span: Span,
    /// Declared type.
    pub ty: Ty,
}

/// Generic parameter with optional bounds, e.g. `T: Ord`.
#[derive(Debug, Clone, PartialEq)]
pub struct GenericParam {
    /// Parameter name, e.g. `T`.
    pub name: String,
    /// Span of the name.
    pub span: Span,
    /// Bounds after `:`, each a type path.
    pub bounds: Vec<Ty>,
}

/// Struct item.
#[derive(Debug, Clone, PartialEq)]
pub struct StructItem {
    /// `true` for `pub struct`.
    pub is_public: bool,
    /// Struct name.
    pub name: String,
    /// Generic parameters.
    pub generics: Vec<GenericParam>,
    /// `{ field: Ty, ... }` fields.
    pub fields: Vec<FieldDecl>,
}

/// One struct field declaration.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldDecl {
    /// Field name.
    pub name: String,
    /// Span of the name.
    pub span: Span,
    /// Declared type.
    pub ty: Ty,
}

/// Enum item.
#[derive(Debug, Clone, PartialEq)]
pub struct EnumItem {
    /// `true` for `pub enum`.
    pub is_public: bool,
    /// Enum name.
    pub name: String,
    /// Generic parameters.
    pub generics: Vec<GenericParam>,
    /// Variants in source order.
    pub variants: Vec<EnumVariant>,
}

/// One enum variant: `None`, `Some(T)`, or `Pair(T, U)`.
#[derive(Debug, Clone, PartialEq)]
pub struct EnumVariant {
    /// Variant name.
    pub name: String,
    /// Span of the name.
    pub span: Span,
    /// Tuple payload types, if any.
    pub payload: Vec<Ty>,
}

/// Import item: `import math::sqrt;`.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportItem {
    /// Path segments, e.g. `["math", "sqrt"]`.
    pub path: Vec<String>,
}

/// A `{ stmt; ... }` block; value is its trailing expression, if any.
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    /// Statements (and expression-statements) in order.
    pub stmts: Vec<Stmt>,
    /// Trailing expression without `;`, if any.
    pub tail: Option<Box<Expr>>,
    /// Span covering the braces.
    pub span: Span,
}

/// Statement.
#[derive(Debug, Clone, PartialEq)]
pub enum StmtKind {
    /// `let [mut] name[: Ty] [= init];`
    Let(LetStmt),
    /// `return [expr];`
    Return(Option<Expr>),
    /// `while cond { ... }`
    While {
        /// Loop condition.
        cond: Expr,
        /// Loop body.
        body: Block,
    },
    /// `loop { ... }`
    Loop(Block),
    /// `for pat in iter { ... }`
    For {
        /// Loop variable pattern.
        pat: Pat,
        /// Iterated expression.
        iter: Expr,
        /// Loop body.
        body: Block,
    },
    /// `break;`
    Break,
    /// `continue;`
    Continue,
    /// Expression followed by `;`.
    Expr(Expr),
}

/// Let statement: `let [mut] pat[: Ty] [= init];`.
#[derive(Debug, Clone, PartialEq)]
pub struct LetStmt {
    /// `true` for `let mut`.
    pub mutable: bool,
    /// Bound pattern (name, tuple, or struct pattern).
    pub pat: Pat,
    /// Declared type after `:`, if any.
    pub ty: Option<Ty>,
    /// Initializer after `=`, if any.
    pub init: Option<Expr>,
}

/// Expression.
#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    /// Integer, float, string, char, or bool literal.
    Lit(Literal),
    /// Variable or path reference (`x`, `Option::Some`).
    Path(Vec<String>),
    /// `lhs op rhs` (`a + b * c` keeps precedence in the tree shape).
    Binary {
        /// Operator.
        op: BinOp,
        /// Left operand.
        lhs: Box<Expr>,
        /// Right operand.
        rhs: Box<Expr>,
    },
    /// `-x`, `!x`.
    Unary {
        /// Operator.
        op: UnOp,
        /// Operand.
        operand: Box<Expr>,
    },
    /// `&x`, `&mut x`.
    AddrOf {
        /// `true` for `&mut`.
        mutable: bool,
        /// Borrowed expression.
        operand: Box<Expr>,
    },
    /// `*p`.
    Deref(Box<Expr>),
    /// `x as T`.
    Cast {
        /// Converted expression.
        expr: Box<Expr>,
        /// Target type.
        ty: Ty,
    },
    /// `target = value`, `target += value`, ...
    Assign {
        /// Assignment target (validated in Commit 03).
        target: Box<Expr>,
        /// `None` for plain `=`; compound otherwise.
        op: Option<BinOp>,
        /// Assigned value.
        value: Box<Expr>,
    },
    /// `callee(args)`.
    Call {
        /// Called expression.
        callee: Box<Expr>,
        /// Argument expressions.
        args: Vec<Expr>,
    },
    /// `recv.field`.
    Field {
        /// Receiver expression.
        recv: Box<Expr>,
        /// Field name.
        field: String,
    },
    /// `if cond { ... } else { ... }` / `else if ...`.
    If {
        /// Branch condition.
        cond: Box<Expr>,
        /// Then branch.
        then: Block,
        /// Else branch (another `if` for `else if`).
        els: Option<Box<Expr>>,
    },
    /// `match scrut { pat => body, ... }`.
    Match {
        /// Scrutinee expression.
        scrut: Box<Expr>,
        /// Arms in source order.
        arms: Vec<MatchArm>,
    },
    /// `{ ... }` block expression.
    Block(Block),
    /// `(a, b)` tuple (empty `()` is [`ExprKind::Unit`]).
    Tuple(Vec<Expr>),
    /// `()`.
    Unit,
}

/// One `match` arm.
#[derive(Debug, Clone, PartialEq)]
pub struct MatchArm {
    /// Arm pattern.
    pub pat: Pat,
    /// Arm body.
    pub body: Expr,
}

/// Pattern.
#[derive(Debug, Clone, PartialEq)]
pub enum PatKind {
    /// `_`.
    Wildcard,
    /// `x` binding.
    Ident(String),
    /// Literal pattern (`0`, `"quit"`).
    Lit(Literal),
    /// `Some(x)` / `Option::None`.
    Variant {
        /// Path segments of the variant.
        path: Vec<String>,
        /// Sub-patterns.
        args: Vec<Pat>,
    },
    /// `(a, b)` tuple pattern.
    Tuple(Vec<Pat>),
    /// `Point { x, y: q }` struct pattern.
    Struct {
        /// Path segments of the struct.
        path: Vec<String>,
        /// Fields in source order.
        fields: Vec<StructPatField>,
    },
}

/// One field of a struct pattern: `x` (binds `x`) or `x: pat`.
#[derive(Debug, Clone, PartialEq)]
pub struct StructPatField {
    /// Field name.
    pub name: String,
    /// Span of the name.
    pub span: Span,
    /// Sub-pattern (`Ident(name)` for the shorthand).
    pub pat: Pat,
}

/// Literal value. Number spellings keep their raw text and suffix so the
/// checker, not the parser, decides types and ranges.
#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    /// Integer with its source text and optional suffix (`10`, `0xff`,
    /// `42u64`).
    Int {
        /// Raw spelling including any suffix.
        text: String,
        /// Type suffix, if any.
        suffix: Option<String>,
    },
    /// Floating point with its source text and optional suffix.
    Float {
        /// Raw spelling including any suffix.
        text: String,
        /// Type suffix, if any.
        suffix: Option<String>,
    },
    /// String contents with escapes resolved.
    Str(String),
    /// Character with escapes resolved.
    Char(char),
    /// `true` / `false`.
    Bool(bool),
}

/// Binary operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    /// `+`
    Add,
    /// `-`
    Sub,
    /// `*`
    Mul,
    /// `/`
    Div,
    /// `%`
    Rem,
    /// `==`
    Eq,
    /// `!=`
    Ne,
    /// `<`
    Lt,
    /// `<=`
    Le,
    /// `>`
    Gt,
    /// `>=`
    Ge,
    /// `&&`
    And,
    /// `||`
    Or,
    /// `&`
    BitAnd,
    /// `|`
    BitOr,
    /// `^`
    BitXor,
    /// `<<`
    Shl,
    /// `>>`
    Shr,
}

/// Unary operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    /// `-x`
    Neg,
    /// `!x`
    Not,
}

/// Type expression.
#[derive(Debug, Clone, PartialEq)]
pub enum TyKind {
    /// `i32`, `Option<T>`, `math::Vec3`.
    Path {
        /// Path segments.
        segments: Vec<String>,
        /// Generic arguments, as in `Option<T>`.
        args: Vec<Ty>,
    },
    /// `&T`, `&mut T`.
    Ref {
        /// `true` for `&mut`.
        mutable: bool,
        /// Pointee type.
        inner: Box<Ty>,
    },
    /// `*const T`, `*mut T`.
    Ptr {
        /// `true` for `*mut`.
        mutable: bool,
        /// Pointee type.
        inner: Box<Ty>,
    },
    /// `[T; N]`.
    Array {
        /// Element type.
        elem: Box<Ty>,
        /// Length expression.
        len: Box<Expr>,
    },
    /// `[T]`.
    Slice(Box<Ty>),
    /// `(A, B)` (`()` is [`TyKind::Unit`]).
    Tuple(Vec<Ty>),
    /// `()`.
    Unit,
}

#[cfg(test)]
mod tests {
    use super::*;
    use lobster_source::{FileId, Span};

    fn span() -> Span {
        Span::new(FileId(0), 0, 1)
    }

    fn ident_expr(name: &str) -> Expr {
        Expr::new(ExprKind::Path(vec![name.to_string()]), span())
    }

    #[test]
    fn spanned_carries_node_and_span() {
        let e = ident_expr("x");
        assert_eq!(e.span, span());
        assert!(matches!(e.node, ExprKind::Path(_)));
    }

    #[test]
    fn precedence_shape_is_add_of_mul() {
        // Mirrors the parser guarantee: `a + b * c` is ADD(a, MUL(b, c)).
        let tree = Expr::new(
            ExprKind::Binary {
                op: BinOp::Add,
                lhs: Box::new(ident_expr("a")),
                rhs: Box::new(Expr::new(
                    ExprKind::Binary {
                        op: BinOp::Mul,
                        lhs: Box::new(ident_expr("b")),
                        rhs: Box::new(ident_expr("c")),
                    },
                    span(),
                )),
            },
            span(),
        );
        let ExprKind::Binary {
            op: BinOp::Add,
            rhs,
            ..
        } = &tree.node
        else {
            panic!("expected add")
        };
        assert!(matches!(&rhs.node, ExprKind::Binary { op: BinOp::Mul, .. }));
    }

    #[test]
    fn item_visibility() {
        let f = Item::new(
            ItemKind::Fn(FnItem {
                is_public: true,
                name: "f".to_string(),
                generics: Vec::new(),
                params: Vec::new(),
                ret: None,
                body: Block {
                    stmts: Vec::new(),
                    tail: None,
                    span: span(),
                },
            }),
            span(),
        );
        assert!(f.node.is_public());
        let i = Item::new(
            ItemKind::Import(ImportItem {
                path: vec!["m".to_string()],
            }),
            span(),
        );
        assert!(!i.node.is_public());
    }

    #[test]
    fn struct_pattern_fields() {
        let pat = Pat::new(
            PatKind::Struct {
                path: vec!["Point".to_string()],
                fields: vec![StructPatField {
                    name: "x".to_string(),
                    span: span(),
                    pat: Pat::new(PatKind::Ident("x".to_string()), span()),
                }],
            },
            span(),
        );
        let PatKind::Struct { path, fields } = &pat.node else {
            panic!("expected struct pat")
        };
        assert_eq!(path, &["Point"]);
        assert_eq!(fields.len(), 1);
    }

    #[test]
    fn types_nest() {
        let ty = Ty::new(
            TyKind::Ref {
                mutable: true,
                inner: Box::new(Ty::new(
                    TyKind::Path {
                        segments: vec!["Point".to_string()],
                        args: Vec::new(),
                    },
                    span(),
                )),
            },
            span(),
        );
        assert!(matches!(ty.node, TyKind::Ref { mutable: true, .. }));
    }
}
