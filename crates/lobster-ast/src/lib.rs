//! Lobster abstract syntax tree (file.md section 20).
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

/// Let statement.
#[derive(Debug, Clone, PartialEq)]
pub struct LetStmt {
    /// `true` for `let mut`.
    pub mutable: bool,
    /// Bound name.
    pub name: String,
    /// Span of the name.
    pub span: Span,
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
}

/// Literal value.
#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    /// Integer with its source text (`10`, `0xff`, `1_000_000`).
    /// The value is parsed on demand so lex/parse never fail on range.
    Int(String),
    /// Floating point with its source text.
    Float(String),
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
