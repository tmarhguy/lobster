//! Lobster semantic types.
//!
//! [`Ty`] is the checked type language: primitives, composites, and named
//! ADTs. It deliberately has no inference variables — the checker is
//! bidirectional with literal adoption instead.
//!
//! `usize`/`isize` widths are target-dependent (32 on Tomato32). Until
//! target data layouts land, range checks assume 64 bits.

use std::fmt;

/// Signed/unsigned integer types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IntTy {
    /// `i8`
    I8,
    /// `i16`
    I16,
    /// `i32`
    I32,
    /// `i64`
    I64,
    /// `isize`
    Isize,
    /// `u8`
    U8,
    /// `u16`
    U16,
    /// `u32`
    U32,
    /// `u64`
    U64,
    /// `usize`
    Usize,
}

impl IntTy {
    /// Look up an integer type by name (`"i32"`, `"usize"`, …).
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "i8" => Self::I8,
            "i16" => Self::I16,
            "i32" => Self::I32,
            "i64" => Self::I64,
            "isize" => Self::Isize,
            "u8" => Self::U8,
            "u16" => Self::U16,
            "u32" => Self::U32,
            "u64" => Self::U64,
            "usize" => Self::Usize,
            _ => return None,
        })
    }

    /// True for signed types.
    #[must_use]
    pub const fn is_signed(self) -> bool {
        matches!(
            self,
            Self::I8 | Self::I16 | Self::I32 | Self::I64 | Self::Isize
        )
    }

    /// Bit width (64 for `usize`/`isize` until data layouts land).
    #[must_use]
    pub const fn bits(self) -> u32 {
        match self {
            Self::I8 | Self::U8 => 8,
            Self::I16 | Self::U16 => 16,
            Self::I32 | Self::U32 => 32,
            Self::I64 | Self::U64 | Self::Isize | Self::Usize => 64,
        }
    }

    /// Minimum representable value.
    #[must_use]
    pub const fn min(self) -> i128 {
        if self.is_signed() {
            -(1i128 << (self.bits() - 1))
        } else {
            0
        }
    }

    /// Maximum representable value.
    #[must_use]
    pub const fn max(self) -> i128 {
        if self.is_signed() {
            (1i128 << (self.bits() - 1)) - 1
        } else {
            (1i128 << self.bits()) - 1
        }
    }
}

/// Floating-point types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FloatTy {
    /// `f32`
    F32,
    /// `f64`
    F64,
}

impl FloatTy {
    /// Look up a float type by name.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "f32" => Some(Self::F32),
            "f64" => Some(Self::F64),
            _ => None,
        }
    }
}

/// A checked Lobster type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ty {
    /// The error type: poisons nothing, suppresses cascades.
    Error,
    /// `bool`
    Bool,
    /// `char`
    Char,
    /// `str` (always behind a reference: string literals are `&str`)
    Str,
    /// `()`
    Unit,
    /// Integer
    Int(IntTy),
    /// Float
    Float(FloatTy),
    /// `[T; N]` (`None` length = non-literal, const-eval deferred)
    Array(Box<Ty>, Option<u64>),
    /// `[T]`
    Slice(Box<Ty>),
    /// `(A, B)`
    Tuple(Vec<Ty>),
    /// `&T`, `&mut T`
    Ref {
        /// `true` for `&mut`.
        mutable: bool,
        /// Pointee type.
        inner: Box<Ty>,
    },
    /// `*const T`, `*mut T`
    Ptr {
        /// `true` for `*mut`.
        mutable: bool,
        /// Pointee type.
        inner: Box<Ty>,
    },
    /// Named struct or enum with generic arguments.
    Adt {
        /// Struct or enum.
        kind: AdtKind,
        /// Type name.
        name: String,
        /// Generic arguments.
        args: Vec<Ty>,
    },
    /// Function value: parameter types and return type.
    Fn {
        /// Parameter types.
        params: Vec<Ty>,
        /// Return type.
        ret: Box<Ty>,
    },
}

/// Struct vs enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdtKind {
    /// Struct
    Struct,
    /// Enum
    Enum,
}

impl Ty {
    /// True for integer types.
    #[must_use]
    pub const fn is_int(&self) -> bool {
        matches!(self, Self::Int(_))
    }

    /// True for float types.
    #[must_use]
    pub const fn is_float(&self) -> bool {
        matches!(self, Self::Float(_))
    }

    /// True for numeric (int or float) types.
    #[must_use]
    pub const fn is_numeric(&self) -> bool {
        self.is_int() || self.is_float()
    }

    /// Unify two types for a shared position (branch arms, operands).
    /// `Error` unifies with anything to suppress cascades.
    #[must_use]
    pub fn unify(a: &Self, b: &Self) -> Option<Self> {
        if matches!(a, Self::Error) {
            return Some(b.clone());
        }
        if matches!(b, Self::Error) {
            return Some(a.clone());
        }
        if a == b {
            return Some(a.clone());
        }
        None
    }
}

impl fmt::Display for IntTy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::I8 => "i8",
            Self::I16 => "i16",
            Self::I32 => "i32",
            Self::I64 => "i64",
            Self::Isize => "isize",
            Self::U8 => "u8",
            Self::U16 => "u16",
            Self::U32 => "u32",
            Self::U64 => "u64",
            Self::Usize => "usize",
        };
        f.write_str(s)
    }
}

impl fmt::Display for FloatTy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::F32 => "f32",
            Self::F64 => "f64",
        })
    }
}

impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Error => f.write_str("{error}"),
            Self::Bool => f.write_str("bool"),
            Self::Char => f.write_str("char"),
            Self::Str => f.write_str("str"),
            Self::Unit => f.write_str("()"),
            Self::Int(t) => write!(f, "{t}"),
            Self::Float(t) => write!(f, "{t}"),
            Self::Array(elem, Some(n)) => write!(f, "[{elem}; {n}]"),
            Self::Array(elem, None) => write!(f, "[{elem}; ?]"),
            Self::Slice(elem) => write!(f, "[{elem}]"),
            Self::Tuple(elems) => {
                f.write_str("(")?;
                for (i, e) in elems.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{e}")?;
                }
                f.write_str(")")
            }
            Self::Ref { mutable, inner } => {
                if *mutable {
                    write!(f, "&mut {inner}")
                } else {
                    write!(f, "&{inner}")
                }
            }
            Self::Ptr { mutable, inner } => {
                if *mutable {
                    write!(f, "*mut {inner}")
                } else {
                    write!(f, "*const {inner}")
                }
            }
            Self::Adt { name, args, .. } => {
                f.write_str(name)?;
                if !args.is_empty() {
                    f.write_str("<")?;
                    for (i, a) in args.iter().enumerate() {
                        if i > 0 {
                            f.write_str(", ")?;
                        }
                        write!(f, "{a}")?;
                    }
                    f.write_str(">")?;
                }
                Ok(())
            }
            Self::Fn { params, ret } => {
                f.write_str("fn(")?;
                for (i, p) in params.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{p}")?;
                }
                write!(f, ") -> {ret}")
            }
        }
    }
}

/// Valid number type suffixes (spec §1).
pub const NUMBER_SUFFIXES: &[&str] = &[
    "i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64", "f32", "f64", "usize", "isize",
];

/// Split a raw number literal into its core and type suffix.
///
/// The split mirrors the lexer: a suffix is only recognized where the
/// lexer scans one (after the digits), so hex cores like `0x1f32` never
/// mis-split into core `0x1` plus suffix `f32`. Returns `(core, None)`
/// when no valid suffix trails.
pub fn split_number_suffix(text: &str) -> (&str, Option<&str>) {
    // The core's digit set, from the text prefix (mirrors the lexer: a
    // hex core's letters are digits, so `0xffu8` splits but `0x1f32`
    // never does).
    let core_digits: &[u8] = if text.starts_with("0x") || text.starts_with("0X") {
        b"0123456789abcdefABCDEF_"
    } else if text.starts_with("0o")
        || text.starts_with("0O")
        || text.starts_with("0b")
        || text.starts_with("0B")
    {
        b"01234567_"
    } else {
        b"0123456789_"
    };
    // Suffix initials that belong to a hex core instead.
    let hex_letters: &[u8] = b"abcdefABCDEF";
    for suffix in NUMBER_SUFFIXES {
        let Some(core) = text.strip_suffix(suffix) else {
            continue;
        };
        // The suffix must follow a core digit (so `u64` alone never
        // splits, and `0xffu8` splits after its hex digits).
        if !core
            .as_bytes()
            .last()
            .is_some_and(|c| core_digits.contains(c))
        {
            continue;
        }
        // A suffix starting with a hex letter belongs to a hex core
        // (`0x1f32` keeps its digits).
        if (text.starts_with("0x") || text.starts_with("0X"))
            && suffix
                .as_bytes()
                .first()
                .is_some_and(|c| hex_letters.contains(c))
        {
            continue;
        }
        return (core, Some(suffix));
    }
    (text, None)
}
pub fn split_int_core(core: &str) -> Option<(u32, String)> {
    let (radix, rest) =
        if let Some(hex) = core.strip_prefix("0x").or_else(|| core.strip_prefix("0X")) {
            (16, hex)
        } else if let Some(oct) = core.strip_prefix("0o").or_else(|| core.strip_prefix("0O")) {
            (8, oct)
        } else if let Some(bin) = core.strip_prefix("0b").or_else(|| core.strip_prefix("0B")) {
            (2, bin)
        } else {
            (10, core)
        };
    let digits: String = rest.chars().filter(|&c| c != '_').collect();
    if digits.is_empty() {
        return None;
    }
    Some((radix, digits))
}

/// True when the integer literal `core` (no suffix) fits in `ty`.
/// Underscores are ignored; a trailing `_` is rejected.
pub fn int_literal_fits(ty: IntTy, core: &str) -> bool {
    if core.ends_with('_') {
        return false;
    }
    let Some((radix, digits)) = split_int_core(core) else {
        return false;
    };
    let Ok(value) = u128::from_str_radix(&digits, radix) else {
        return false;
    };
    // Literals are non-negative; negation is a separate unary node whose
    // range rule allows one extra unit for signed minima.
    value <= ty.max() as u128
}

/// True when `-lit` (negated literal with magnitude `core`) is valid for
/// signed `ty`: magnitudes up to `MAX + 1` are allowed so `-128i8` works.
pub fn neg_literal_fits(ty: IntTy, core: &str) -> bool {
    if !ty.is_signed() {
        // Unsigned negation wraps; always representable.
        return int_literal_fits(ty, core);
    }
    if core.ends_with('_') {
        return false;
    }
    let Some((radix, digits)) = split_int_core(core) else {
        return false;
    };
    let Ok(value) = u128::from_str_radix(&digits, radix) else {
        return false;
    };
    value <= (ty.max() as u128) + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn int_names_round_trip() {
        for name in [
            "i8", "i16", "i32", "i64", "isize", "u8", "u16", "u32", "u64", "usize",
        ] {
            let ty = IntTy::from_name(name).unwrap();
            assert_eq!(ty.to_string(), name);
        }
        assert!(IntTy::from_name("u128").is_none());
        assert_eq!(FloatTy::from_name("f32"), Some(FloatTy::F32));
    }

    #[test]
    fn int_ranges() {
        assert_eq!((IntTy::I8.min(), IntTy::I8.max()), (-128, 127));
        assert_eq!((IntTy::U8.min(), IntTy::U8.max()), (0, 255));
        assert_eq!(IntTy::U64.max(), (1i128 << 64) - 1);
    }

    #[test]
    fn literal_fits_decimal_hex_bin() {
        assert!(int_literal_fits(IntTy::U8, "255"));
        assert!(!int_literal_fits(IntTy::U8, "256"));
        assert!(!int_literal_fits(IntTy::U8, "300"));
        assert!(int_literal_fits(IntTy::I8, "127"));
        assert!(!int_literal_fits(IntTy::I8, "128"));
        assert!(int_literal_fits(IntTy::U8, "0xff"));
        assert!(!int_literal_fits(IntTy::U8, "0x100"));
        assert!(int_literal_fits(IntTy::U8, "0b1010"));
        assert!(int_literal_fits(IntTy::U32, "1_000_000"));
        assert!(!int_literal_fits(IntTy::U32, "1__"));
        assert!(!int_literal_fits(IntTy::U8, "12a"));
    }

    #[test]
    fn negated_minimum_fits() {
        assert!(neg_literal_fits(IntTy::I8, "128"));
        assert!(!neg_literal_fits(IntTy::I8, "129"));
        assert!(neg_literal_fits(IntTy::U8, "255"));
    }

    #[test]
    fn display_shapes() {
        assert_eq!(Ty::Unit.to_string(), "()");
        assert_eq!(Ty::Int(IntTy::U32).to_string(), "u32");
        assert_eq!(
            Ty::Ref {
                mutable: false,
                inner: Box::new(Ty::Str)
            }
            .to_string(),
            "&str"
        );
        assert_eq!(
            Ty::Tuple(vec![Ty::Int(IntTy::U32), Ty::Bool]).to_string(),
            "(u32, bool)"
        );
        assert_eq!(
            Ty::Adt {
                kind: AdtKind::Enum,
                name: "Option".to_string(),
                args: vec![Ty::Int(IntTy::U64)]
            }
            .to_string(),
            "Option<u64>"
        );
    }

    #[test]
    fn suffix_splits_like_the_lexer() {
        assert_eq!(split_number_suffix("42u64"), ("42", Some("u64")));
        assert_eq!(split_number_suffix("1.5f32"), ("1.5", Some("f32")));
        assert_eq!(split_number_suffix("2e10usize"), ("2e10", Some("usize")));
        assert_eq!(split_number_suffix("0xffu8"), ("0xff", Some("u8")));
        assert_eq!(split_number_suffix("0b101u8"), ("0b101", Some("u8")));
        assert_eq!(split_number_suffix("1f32"), ("1", Some("f32")));
        // Hex lookalikes never split.
        assert_eq!(split_number_suffix("0x1f32"), ("0x1f32", None));
        assert_eq!(split_number_suffix("0xabc"), ("0xabc", None));
        assert_eq!(split_number_suffix("255"), ("255", None));
        assert_eq!(split_number_suffix("u64"), ("u64", None));
    }

    #[test]
    fn unify_rules() {
        let u32 = Ty::Int(IntTy::U32);
        assert_eq!(Ty::unify(&u32, &u32), Some(u32.clone()));
        assert_eq!(Ty::unify(&Ty::Error, &u32), Some(u32.clone()));
        assert_eq!(Ty::unify(&u32, &Ty::Bool), None);
    }
}
