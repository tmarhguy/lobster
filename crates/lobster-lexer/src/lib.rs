//! Lobster lexer: tokens, numbers, strings, and comments.
//!
//! Produces a flat [`Token`] stream plus [`Diagnostic`]s. The lexer never
//! fails wholesale: on bad input it emits a diagnostic and recovers at the
//! next token boundary, so the parser still runs and can report its own
//! errors.
//!
//! Lexer diagnostic codes:
//!
//! ```text
//! E100 invalid character
//! E101 unterminated string literal
//! E102 unterminated char literal
//! E103 unterminated block comment
//! E104 invalid number literal
//! E105 invalid escape
//! ```

use lobster_diagnostics::{Diagnostic, Label};
use lobster_source::{FileId, SourceManager, Span};

/// A keyword. Matched from identifiers; the full set the parser accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keyword {
    /// `fn`
    Fn,
    /// `let`
    Let,
    /// `mut`
    Mut,
    /// `if`
    If,
    /// `else`
    Else,
    /// `while`
    While,
    /// `loop`
    Loop,
    /// `for`
    For,
    /// `break`
    Break,
    /// `continue`
    Continue,
    /// `return`
    Return,
    /// `match`
    Match,
    /// `struct`
    Struct,
    /// `enum`
    Enum,
    /// `import`
    Import,
    /// `module`
    Module,
    /// `pub`
    Pub,
    /// `unsafe`
    Unsafe,
    /// `const`
    Const,
    /// `static`
    Static,
    /// `type`
    Type,
    /// `impl`
    Impl,
    /// `trait`
    Trait,
    /// `true`
    True,
    /// `false`
    False,
    /// `self`
    SelfValue,
    /// `Self`
    SelfType,
    /// `as`
    As,
    /// `in`
    In,
    /// `where`
    Where,
    /// Reserved for future use; using one as a name is an error today.
    Async,
    /// Reserved for future use.
    Await,
    /// Reserved for future use.
    Dyn,
    /// Reserved for future use.
    Extern,
    /// Reserved for future use.
    Macro,
    /// Reserved for future use.
    Move,
    /// Reserved for future use.
    Ref,
    /// Reserved for future use.
    Try,
    /// Reserved for future use.
    Union,
}

impl Keyword {
    /// Look up a keyword by its spelling.
    #[must_use]
    pub fn from_spelling(s: &str) -> Option<Self> {
        Some(match s {
            "fn" => Self::Fn,
            "let" => Self::Let,
            "mut" => Self::Mut,
            "if" => Self::If,
            "else" => Self::Else,
            "while" => Self::While,
            "loop" => Self::Loop,
            "for" => Self::For,
            "break" => Self::Break,
            "continue" => Self::Continue,
            "return" => Self::Return,
            "match" => Self::Match,
            "struct" => Self::Struct,
            "enum" => Self::Enum,
            "import" => Self::Import,
            "module" => Self::Module,
            "pub" => Self::Pub,
            "unsafe" => Self::Unsafe,
            "const" => Self::Const,
            "static" => Self::Static,
            "type" => Self::Type,
            "impl" => Self::Impl,
            "trait" => Self::Trait,
            "true" => Self::True,
            "false" => Self::False,
            "self" => Self::SelfValue,
            "Self" => Self::SelfType,
            "as" => Self::As,
            "in" => Self::In,
            "where" => Self::Where,
            "async" => Self::Async,
            "await" => Self::Await,
            "dyn" => Self::Dyn,
            "extern" => Self::Extern,
            "macro" => Self::Macro,
            "move" => Self::Move,
            "ref" => Self::Ref,
            "try" => Self::Try,
            "union" => Self::Union,
            _ => return None,
        })
    }
}

/// Token kind. Identifier text and decoded literal values ride along so the
/// parser never re-slices source.
#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    /// `foo`, `fib`, `_x` (spelling in [`Token::text`]).
    Ident,
    /// Reserved word.
    Keyword(Keyword),
    /// Integer literal, raw spelling in [`Token::text`] including any
    /// type suffix (`10`, `0xff`, `1_000_000`, `42u64`). Use
    /// [`Token::number_suffix`] to split the suffix.
    Int {
        /// Type suffix, if any.
        suffix: Option<String>,
    },
    /// Float literal, raw spelling in [`Token::text`] including any
    /// type suffix (`3.14`, `2e10`, `1.5f32`).
    Float {
        /// Type suffix, if any.
        suffix: Option<String>,
    },
    /// `"..."` with escapes resolved.
    Str(String),
    /// `'a'`, `'\n'` with escapes resolved.
    Char(char),
    /// `+`
    Plus,
    /// `-`
    Minus,
    /// `*`
    Star,
    /// `/`
    Slash,
    /// `%`
    Percent,
    /// `=`
    Eq,
    /// `==`
    EqEq,
    /// `!`
    Bang,
    /// `!=`
    BangEq,
    /// `<`
    Lt,
    /// `<=`
    LtEq,
    /// `>`
    Gt,
    /// `>=`
    GtEq,
    /// `&`
    Amp,
    /// `&&`
    AmpAmp,
    /// `|`
    Pipe,
    /// `||`
    PipePipe,
    /// `^`
    Caret,
    /// `<<`
    LtLt,
    /// `>>`
    GtGt,
    /// `+=`
    PlusEq,
    /// `-=`
    MinusEq,
    /// `*=`
    StarEq,
    /// `/=`
    SlashEq,
    /// `%=`
    PercentEq,
    /// `&=`
    AmpEq,
    /// `|=`
    PipeEq,
    /// `^=`
    CaretEq,
    /// `<<=`
    LtLtEq,
    /// `>>=`
    GtGtEq,
    /// `->`
    Arrow,
    /// `=>`
    FatArrow,
    /// `:`
    Colon,
    /// `::`
    ColonColon,
    /// `;`
    Semi,
    /// `,`
    Comma,
    /// `.`
    Dot,
    /// `..` (ranges; the parser does not accept them yet)
    DotDot,
    /// `~` (reserved)
    Tilde,
    /// `#` (attributes arrive later; reserved today)
    Hash,
    /// `?` (reserved)
    Question,
    /// `@` (attributes such as `@test`, `@mmio`; parsed later).
    At,
    /// `(`
    OpenParen,
    /// `)`
    CloseParen,
    /// `{`
    OpenBrace,
    /// `}`
    CloseBrace,
    /// `[`
    OpenBracket,
    /// `]`
    CloseBracket,
}

/// One lexed token.
#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    /// Token kind.
    pub kind: TokenKind,
    /// Source range of the token.
    pub span: Span,
    /// Raw source spelling (names, numbers, punctuation).
    pub text: String,
}

/// Valid type suffixes for number literals (spec §1).
const NUMBER_SUFFIXES: &[&str] = &[
    "i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64", "f32", "f64", "usize", "isize",
];

impl Token {
    /// Type suffix of a number literal (`42u64` → `Some("u64")`).
    /// Returns `None` for unsuffixed numbers and non-number tokens.
    #[must_use]
    pub fn number_suffix(&self) -> Option<&str> {
        match &self.kind {
            TokenKind::Int { suffix } | TokenKind::Float { suffix } => suffix.as_deref(),
            _ => None,
        }
    }
    /// True for identifiers with the given spelling.
    #[must_use]
    pub fn is_ident(&self, name: &str) -> bool {
        matches!(self.kind, TokenKind::Ident) && self.text == name
    }

    /// True for the given keyword.
    #[must_use]
    pub fn is_keyword(&self, kw: Keyword) -> bool {
        matches!(self.kind, TokenKind::Keyword(k) if k == kw)
    }
}

/// Lexer output: tokens plus any diagnostics.
pub struct LexOutput {
    /// Tokens in source order (no EOF marker; the parser handles the end).
    pub tokens: Vec<Token>,
    /// Diagnostics in source order.
    pub diagnostics: Vec<Diagnostic>,
}

/// Lex `text` (already loaded under `file`) into tokens and diagnostics.
#[must_use]
pub fn lex(sources: &SourceManager, file: FileId, text: &str) -> LexOutput {
    Lexer::new(sources, file, text).run()
}

struct Lexer<'a> {
    sources: &'a SourceManager,
    file: FileId,
    text: &'a str,
    bytes: &'a [u8],
    pos: usize,
    tokens: Vec<Token>,
    diagnostics: Vec<Diagnostic>,
}

impl<'a> Lexer<'a> {
    fn new(sources: &'a SourceManager, file: FileId, text: &'a str) -> Self {
        Self {
            sources,
            file,
            text,
            bytes: text.as_bytes(),
            pos: 0,
            tokens: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    fn run(mut self) -> LexOutput {
        while self.pos < self.bytes.len() {
            let c = self.bytes[self.pos];
            match c {
                b' ' | b'\t' | b'\r' | b'\n' => self.pos += 1,
                b'/' if self.peek(1) == Some(b'/') => self.lex_line_comment(),
                b'/' if self.peek(1) == Some(b'*') => self.lex_block_comment(),
                b'"' => self.lex_string(),
                b'\'' => self.lex_char_or_lifetime(),
                b'0'..=b'9' => self.lex_number(),
                b'a'..=b'z' | b'A'..=b'Z' | b'_' => self.lex_ident(),
                _ => {
                    if !self.lex_punct() {
                        // Boundary-safe: `pos` can sit on a multibyte char
                        // (or, defensively, mid-char). Never slice blindly.
                        let (advance, detail) =
                            match self.text.get(self.pos..).and_then(|s| s.chars().next()) {
                                Some(ch) => (ch.len_utf8(), format!("unexpected character '{ch}'")),
                                None => {
                                    (1, format!("unexpected byte 0x{:02X}", self.bytes[self.pos]))
                                }
                            };
                        let span = self.span_here(self.pos, self.pos + advance);
                        self.error("E100", "invalid character", span, detail);
                        self.pos += advance;
                    }
                }
            }
        }
        LexOutput {
            tokens: self.tokens,
            diagnostics: self.diagnostics,
        }
    }

    fn peek(&self, off: usize) -> Option<u8> {
        self.bytes.get(self.pos + off).copied()
    }

    fn span_here(&self, start: usize, end: usize) -> Span {
        self.sources
            .span(self.file, start, end.min(self.text.len()))
            .unwrap_or_else(|| Span::empty(self.file, start as u32))
    }

    fn push(&mut self, kind: TokenKind, start: usize, end: usize) {
        let text = self.text[start..end.min(self.text.len())].to_string();
        self.tokens.push(Token {
            kind,
            span: self.span_here(start, end),
            text,
        });
    }

    fn error(&mut self, code: &str, message: &str, span: Span, detail: String) {
        self.diagnostics.push(Diagnostic::error(
            code,
            message,
            Label::primary(span, detail),
        ));
    }

    fn lex_line_comment(&mut self) {
        while self.pos < self.bytes.len() && self.bytes[self.pos] != b'\n' {
            self.pos += 1;
        }
    }

    fn lex_block_comment(&mut self) {
        let start = self.pos;
        self.pos += 2;
        let mut depth = 1usize;
        while self.pos < self.bytes.len() && depth > 0 {
            if self.bytes[self.pos] == b'/' && self.peek(1) == Some(b'*') {
                depth += 1;
                self.pos += 2;
            } else if self.bytes[self.pos] == b'*' && self.peek(1) == Some(b'/') {
                depth -= 1;
                self.pos += 2;
            } else {
                self.pos += 1;
            }
        }
        if depth > 0 {
            let span = self.span_here(start, self.text.len());
            self.error(
                "E103",
                "unterminated block comment",
                span,
                "comment starts here but never closes".to_string(),
            );
        }
    }

    fn lex_ident(&mut self) {
        let start = self.pos;
        while self.pos < self.bytes.len()
            && matches!(self.bytes[self.pos], b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_')
        {
            self.pos += 1;
        }
        let word = &self.text[start..self.pos];
        let kind = Keyword::from_spelling(word).map_or(TokenKind::Ident, TokenKind::Keyword);
        self.push(kind, start, self.pos);
    }

    fn is_digit_for_radix(b: u8, radix: u32) -> bool {
        match radix {
            2 => matches!(b, b'0'..=b'1'),
            8 => matches!(b, b'0'..=b'7'),
            10 => b.is_ascii_digit(),
            16 => b.is_ascii_hexdigit(),
            _ => false,
        }
    }

    fn lex_number(&mut self) {
        let start = self.pos;
        if self.bytes[self.pos] == b'0'
            && matches!(self.peek(1), Some(b'x' | b'X' | b'o' | b'O' | b'b' | b'B'))
        {
            let radix = match self.peek(1) {
                Some(b'x' | b'X') => 16,
                Some(b'o' | b'O') => 8,
                _ => 2,
            };
            self.pos += 2;
            let digits = self.pos;
            while self.pos < self.bytes.len()
                && (Self::is_digit_for_radix(self.bytes[self.pos], radix)
                    || self.bytes[self.pos] == b'_')
            {
                self.pos += 1;
            }
            let raw: String = self.text[digits..self.pos]
                .chars()
                .filter(|&c| c != '_')
                .collect();
            let mut invalid = raw.is_empty() || u64::from_str_radix(&raw, radix).is_err();
            let mut detail = format!("not a valid base-{radix} number");
            let mut suffix_out = None;
            match self.scan_number_suffix() {
                Some(s) if NUMBER_SUFFIXES.contains(&s.as_str()) => suffix_out = Some(s),
                Some(s) => {
                    invalid = true;
                    detail = format!("invalid number suffix '{s}'");
                }
                // A trailing digit run (e.g. the `2` in `0b102`) is an
                // invalid digit for this radix, not a second token.
                None => {
                    while self.pos < self.bytes.len()
                        && (self.bytes[self.pos].is_ascii_alphanumeric()
                            || self.bytes[self.pos] == b'_')
                    {
                        invalid = true;
                        self.pos += 1;
                    }
                }
            }
            if invalid {
                let span = self.span_here(start, self.pos);
                self.error("E104", "invalid number literal", span, detail);
            }
            self.push(TokenKind::Int { suffix: suffix_out }, start, self.pos);
            return;
        }
        while self.pos < self.bytes.len()
            && (self.bytes[self.pos].is_ascii_digit() || self.bytes[self.pos] == b'_')
        {
            self.pos += 1;
        }
        let mut is_float = false;
        // Fractional part: digit(s) `.` digit — a lone `.` is field access.
        if self.bytes.get(self.pos) == Some(&b'.')
            && matches!(self.bytes.get(self.pos + 1), Some(b'0'..=b'9'))
        {
            is_float = true;
            self.pos += 1;
            while self.pos < self.bytes.len()
                && (self.bytes[self.pos].is_ascii_digit() || self.bytes[self.pos] == b'_')
            {
                self.pos += 1;
            }
        }
        // Exponent.
        if matches!(self.bytes.get(self.pos), Some(b'e' | b'E')) {
            let mut j = self.pos + 1;
            if matches!(self.bytes.get(j), Some(b'+' | b'-')) {
                j += 1;
            }
            if matches!(self.bytes.get(j), Some(b'0'..=b'9')) {
                is_float = true;
                self.pos = j;
                while self.pos < self.bytes.len()
                    && (self.bytes[self.pos].is_ascii_digit() || self.bytes[self.pos] == b'_')
                {
                    self.pos += 1;
                }
            }
        }
        // Type suffix (`42u64`, `1.5f32`, `2e10usize`).
        let mut suffix_out = None;
        let mut suffix_is_float = false;
        match self.scan_number_suffix() {
            Some(s) if NUMBER_SUFFIXES.contains(&s.as_str()) => {
                suffix_is_float = s == "f32" || s == "f64";
                suffix_out = Some(s);
            }
            Some(s) => {
                let span = self.span_here(start, self.pos);
                self.error(
                    "E104",
                    "invalid number literal",
                    span,
                    format!("invalid number suffix '{s}'"),
                );
            }
            None => {}
        }
        self.push(
            if is_float || suffix_is_float {
                TokenKind::Float { suffix: suffix_out }
            } else {
                TokenKind::Int { suffix: suffix_out }
            },
            start,
            self.pos,
        );
    }

    /// Scan an identifier-shaped run after a number (`42u64` → `u64`).
    /// Consumes the run and returns it, or `None` when the next char
    /// cannot start a suffix.
    fn scan_number_suffix(&mut self) -> Option<String> {
        let is_start = matches!(
            self.bytes.get(self.pos),
            Some(b'a'..=b'z' | b'A'..=b'Z' | b'_')
        );
        if !is_start {
            return None;
        }
        let start = self.pos;
        while self.pos < self.bytes.len()
            && (self.bytes[self.pos].is_ascii_alphanumeric() || self.bytes[self.pos] == b'_')
        {
            self.pos += 1;
        }
        Some(self.text[start..self.pos].to_string())
    }

    /// Decode one escape starting after the backslash. Returns the char and
    /// the byte index just past the escape. Malformed escapes report E105
    /// and recover with U+FFFD so lexing always continues.
    fn escape(&mut self, backslash: usize) -> Option<(char, usize)> {
        // `backslash` is ASCII, so `backslash + 1` is a char boundary.
        let rest = self.text.get(backslash + 1..)?;
        let e = rest.chars().next()?;
        let simple = match e {
            'n' => Some('\n'),
            't' => Some('\t'),
            'r' => Some('\r'),
            '0' => Some('\0'),
            '\\' => Some('\\'),
            '"' => Some('"'),
            '\'' => Some('\''),
            _ => None,
        };
        if let Some(c) = simple {
            return Some((c, backslash + 1 + e.len_utf8()));
        }
        if e == 'x' {
            return Some(self.hex_escape(backslash));
        }
        if e == 'u' {
            return Some(self.unicode_escape(backslash));
        }
        let span = self.span_here(backslash, backslash + 1 + e.len_utf8());
        self.error(
            "E105",
            "invalid escape",
            span,
            format!("unknown escape '\\{e}'"),
        );
        // Recover: keep the escaped char literally.
        Some((e, backslash + 1 + e.len_utf8()))
    }

    /// Decode `\xHH`: exactly two hex digits, ASCII only (spec §1).
    fn hex_escape(&mut self, backslash: usize) -> (char, usize) {
        let rest = &self.text[backslash + 2..];
        let digits: String = rest.chars().take(2).collect();
        let ok = digits.len() == 2 && digits.chars().all(|c| c.is_ascii_hexdigit());
        let value = u32::from_str_radix(&digits, 16).unwrap_or(0xFFFD);
        // `\xHH` in a char/string escape must be ASCII; higher values need `\u{}`.
        if !ok || value > 0x7F {
            let end = backslash + 2 + digits.len();
            let span = self.span_here(backslash, end);
            self.error(
                "E105",
                "invalid escape",
                span,
                if ok {
                    "'\\x' escapes must be ASCII (use '\\u{...}' for the rest)".to_string()
                } else {
                    "expected two hex digits after '\\x'".to_string()
                },
            );
            return ('\u{FFFD}', end);
        }
        (char::from_u32(value).unwrap_or('\u{FFFD}'), backslash + 4)
    }

    /// Decode `\u{H…}`: one to six hex digits in braces, a Unicode scalar.
    fn unicode_escape(&mut self, backslash: usize) -> (char, usize) {
        if self
            .text
            .get(backslash + 2..)
            .and_then(|s| s.chars().next())
            != Some('{')
        {
            let span = self.span_here(backslash, backslash + 2);
            self.error(
                "E105",
                "invalid escape",
                span,
                "expected '\\u{H...}' with braces".to_string(),
            );
            return ('\u{FFFD}', backslash + 2);
        }
        // Bounded scan: stop at `}`, newline, or after a few chars so a
        // missing brace cannot swallow the rest of the file.
        let mut digits = String::new();
        let mut closed = false;
        let mut cur = backslash + 3; // past `\u{`
        while let Some(c) = self.text.get(cur..).and_then(|s| s.chars().next()) {
            if c == '}' {
                closed = true;
                cur += 1;
                break;
            }
            if c == '\n' || cur - (backslash + 3) > 8 {
                break;
            }
            digits.push(c);
            cur += c.len_utf8();
        }
        let valid = closed
            && (1..=6).contains(&digits.len())
            && digits.chars().all(|c| c.is_ascii_hexdigit())
            && u32::from_str_radix(&digits, 16)
                .ok()
                .and_then(char::from_u32)
                .is_some();
        if !valid {
            let span = self.span_here(backslash, cur);
            self.error(
                "E105",
                "invalid escape",
                span,
                format!("invalid unicode escape '\\u{{{digits}}}'"),
            );
            return ('\u{FFFD}', cur);
        }
        let value = u32::from_str_radix(&digits, 16).unwrap_or(0xFFFD);
        (char::from_u32(value).unwrap_or('\u{FFFD}'), cur)
    }

    fn lex_string(&mut self) {
        let start = self.pos;
        self.pos += 1;
        let mut value = String::new();
        while self.pos < self.bytes.len() && self.bytes[self.pos] != b'"' {
            if self.bytes[self.pos] == b'\\' {
                if let Some((c, next)) = self.escape(self.pos) {
                    value.push(c);
                    self.pos = next;
                } else {
                    break;
                }
            } else if self.bytes[self.pos] == b'\n' {
                break;
            } else {
                // Boundary-safe char step (see E100 note above).
                match self.text.get(self.pos..).and_then(|s| s.chars().next()) {
                    Some(ch) => {
                        value.push(ch);
                        self.pos += ch.len_utf8();
                    }
                    None => self.pos += 1,
                }
            }
        }
        if self.bytes.get(self.pos) == Some(&b'"') {
            self.pos += 1;
            let end = self.pos;
            self.tokens.push(Token {
                kind: TokenKind::Str(value),
                span: self.span_here(start, end),
                text: self.text[start..end].to_string(),
            });
        } else {
            let span = self.span_here(start, self.pos);
            self.error(
                "E101",
                "unterminated string literal",
                span,
                "string starts here but never closes".to_string(),
            );
            let end = self.pos;
            self.tokens.push(Token {
                kind: TokenKind::Str(value),
                span: self.span_here(start, end),
                text: self.text[start..end].to_string(),
            });
        }
    }

    fn lex_char_or_lifetime(&mut self) {
        let start = self.pos;
        self.pos += 1; // consume `'`
        if self.pos >= self.bytes.len() {
            let span = self.span_here(start, self.pos);
            self.error(
                "E102",
                "unterminated char literal",
                span,
                "expected a character after `'`".to_string(),
            );
            return;
        }
        let c = if self.bytes[self.pos] == b'\\' {
            match self.escape(self.pos) {
                Some((c, next)) => {
                    self.pos = next;
                    c
                }
                None => return,
            }
        } else {
            let ch = self.text[self.pos..].chars().next().unwrap_or('?');
            self.pos += ch.len_utf8();
            ch
        };
        if self.bytes.get(self.pos) == Some(&b'\'') {
            self.pos += 1;
            let end = self.pos;
            self.tokens.push(Token {
                kind: TokenKind::Char(c),
                span: self.span_here(start, end),
                text: self.text[start..end].to_string(),
            });
        } else {
            // Not a char literal (e.g. a lifetime or stray quote): let the
            // parser report it against the quote position.
            let span = self.span_here(start, start + 1);
            self.error(
                "E102",
                "unterminated char literal",
                span,
                "expected closing `'`".to_string(),
            );
        }
    }

    /// Lex one- to three-char punctuation. Returns false when `bytes[pos]`
    /// starts no known token (caller reports E100).
    fn lex_punct(&mut self) -> bool {
        let start = self.pos;
        let b0 = self.bytes[self.pos];
        let b1 = self.peek(1);
        let b2 = self.peek(2);
        let triple: Option<TokenKind> = match (b0, b1, b2) {
            (b'<', Some(b'<'), Some(b'=')) => Some(TokenKind::LtLtEq),
            (b'>', Some(b'>'), Some(b'=')) => Some(TokenKind::GtGtEq),
            _ => None,
        };
        if let Some(kind) = triple {
            self.pos += 3;
            self.push(kind, start, self.pos);
            return true;
        }
        let double: Option<TokenKind> = match (b0, b1) {
            (b'=', Some(b'=')) => Some(TokenKind::EqEq),
            (b'!', Some(b'=')) => Some(TokenKind::BangEq),
            (b'<', Some(b'=')) => Some(TokenKind::LtEq),
            (b'>', Some(b'=')) => Some(TokenKind::GtEq),
            (b'&', Some(b'&')) => Some(TokenKind::AmpAmp),
            (b'|', Some(b'|')) => Some(TokenKind::PipePipe),
            (b'<', Some(b'<')) => Some(TokenKind::LtLt),
            (b'>', Some(b'>')) => Some(TokenKind::GtGt),
            (b'+', Some(b'=')) => Some(TokenKind::PlusEq),
            (b'-', Some(b'=')) => Some(TokenKind::MinusEq),
            (b'*', Some(b'=')) => Some(TokenKind::StarEq),
            (b'/', Some(b'=')) => Some(TokenKind::SlashEq),
            (b'%', Some(b'=')) => Some(TokenKind::PercentEq),
            (b'&', Some(b'=')) => Some(TokenKind::AmpEq),
            (b'|', Some(b'=')) => Some(TokenKind::PipeEq),
            (b'^', Some(b'=')) => Some(TokenKind::CaretEq),
            (b'-', Some(b'>')) => Some(TokenKind::Arrow),
            (b'=', Some(b'>')) => Some(TokenKind::FatArrow),
            (b':', Some(b':')) => Some(TokenKind::ColonColon),
            (b'.', Some(b'.')) => Some(TokenKind::DotDot),
            _ => None,
        };
        if let Some(kind) = double {
            self.pos += 2;
            self.push(kind, start, self.pos);
            return true;
        }
        let single: Option<TokenKind> = match b0 {
            b'+' => Some(TokenKind::Plus),
            b'-' => Some(TokenKind::Minus),
            b'*' => Some(TokenKind::Star),
            b'/' => Some(TokenKind::Slash),
            b'%' => Some(TokenKind::Percent),
            b'=' => Some(TokenKind::Eq),
            b'!' => Some(TokenKind::Bang),
            b'<' => Some(TokenKind::Lt),
            b'>' => Some(TokenKind::Gt),
            b'&' => Some(TokenKind::Amp),
            b'|' => Some(TokenKind::Pipe),
            b'^' => Some(TokenKind::Caret),
            b':' => Some(TokenKind::Colon),
            b';' => Some(TokenKind::Semi),
            b',' => Some(TokenKind::Comma),
            b'.' => Some(TokenKind::Dot),
            b'~' => Some(TokenKind::Tilde),
            b'#' => Some(TokenKind::Hash),
            b'?' => Some(TokenKind::Question),
            b'@' => Some(TokenKind::At),
            b'(' => Some(TokenKind::OpenParen),
            b')' => Some(TokenKind::CloseParen),
            b'{' => Some(TokenKind::OpenBrace),
            b'}' => Some(TokenKind::CloseBrace),
            b'[' => Some(TokenKind::OpenBracket),
            b']' => Some(TokenKind::CloseBracket),
            _ => None,
        };
        match single {
            Some(kind) => {
                self.pos += 1;
                self.push(kind, start, self.pos);
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lobster_source::SourceManager;

    fn lex_text(text: &str) -> (SourceManager, LexOutput) {
        let mut sm = SourceManager::new();
        let id = sm.add_file("t.lobster", text);
        let out = lex(&sm, id, text);
        (sm, out)
    }

    fn kinds(text: &str) -> Vec<TokenKind> {
        lex_text(text)
            .1
            .tokens
            .iter()
            .map(|t| t.kind.clone())
            .collect()
    }

    #[test]
    fn punctuation_and_keywords() {
        let ks = kinds("fn main() { let mut x = 10; }");
        assert!(matches!(ks[0], TokenKind::Keyword(Keyword::Fn)));
        assert!(matches!(ks[1], TokenKind::Ident));
        assert!(matches!(ks[2], TokenKind::OpenParen));
        assert!(matches!(ks[3], TokenKind::CloseParen));
        assert!(matches!(ks[4], TokenKind::OpenBrace));
        assert!(matches!(ks[5], TokenKind::Keyword(Keyword::Let)));
        assert!(matches!(ks[6], TokenKind::Keyword(Keyword::Mut)));
        assert!(matches!(ks[8], TokenKind::Eq));
        assert!(matches!(ks[9], TokenKind::Int { .. }));
        assert!(matches!(ks[10], TokenKind::Semi));
    }

    #[test]
    fn number_forms() {
        let (_, out) = lex_text("0xff 0b1010 0o17 1_000_000 3.14 2e10");
        assert!(out.diagnostics.is_empty());
        let texts: Vec<_> = out.tokens.iter().map(|t| t.text.as_str()).collect();
        assert_eq!(
            texts,
            ["0xff", "0b1010", "0o17", "1_000_000", "3.14", "2e10"]
        );
        assert!(matches!(out.tokens[4].kind, TokenKind::Float { .. }));
        assert!(matches!(out.tokens[5].kind, TokenKind::Float { .. }));
    }

    #[test]
    fn bad_number_reports_e104() {
        let (_, out) = lex_text("0b102");
        assert!(out
            .diagnostics
            .iter()
            .any(|d| d.code.as_ref().is_some_and(|c| c.0 == "E104")));
    }

    #[test]
    fn strings_and_escapes() {
        let (_, out) = lex_text(r#""hi\n" 'a' '\'' "#);
        assert!(out.diagnostics.is_empty());
        assert!(matches!(&out.tokens[0].kind, TokenKind::Str(s) if s == "hi\n"));
        assert!(matches!(out.tokens[1].kind, TokenKind::Char('a')));
        assert!(matches!(out.tokens[2].kind, TokenKind::Char('\'')));
    }

    #[test]
    fn unterminated_string_reports_e101_and_continues() {
        let (_, out) = lex_text("\"abc\nlet x = 1;");
        assert!(out
            .diagnostics
            .iter()
            .any(|d| d.code.as_ref().is_some_and(|c| c.0 == "E101")));
        assert!(out.tokens.iter().any(|t| t.is_keyword(Keyword::Let)));
    }

    #[test]
    fn comments_are_skipped() {
        let ks = kinds("/* outer /* inner */ still */ let // trailing\n x");
        assert!(ks
            .iter()
            .any(|k| matches!(k, TokenKind::Keyword(Keyword::Let))));
        assert_eq!(ks.len(), 2); // let, x
    }

    #[test]
    fn unterminated_block_comment_reports_e103() {
        let (_, out) = lex_text("/* never ends");
        assert!(out
            .diagnostics
            .iter()
            .any(|d| d.code.as_ref().is_some_and(|c| c.0 == "E103")));
    }

    #[test]
    fn invalid_character_reports_e100() {
        let (_, out) = lex_text("let x = $;");
        assert!(out
            .diagnostics
            .iter()
            .any(|d| d.code.as_ref().is_some_and(|c| c.0 == "E100")));
    }

    #[test]
    fn compound_operators() {
        let ks = kinds("== != <= >= && || << >> += -> => ::");
        let names = [
            "EqEq",
            "BangEq",
            "LtEq",
            "GtEq",
            "AmpAmp",
            "PipePipe",
            "LtLt",
            "GtGt",
            "PlusEq",
            "Arrow",
            "FatArrow",
            "ColonColon",
        ];
        for (k, n) in ks.iter().zip(names) {
            assert!(format!("{k:?}").starts_with(n), "{k:?} != {n}");
        }
    }

    #[test]
    fn spans_point_at_source() {
        let (sm, out) = lex_text("let xy = 1;");
        let t = &out.tokens[1];
        assert_eq!(sm.slice(t.span), Some("xy"));
    }

    #[test]
    fn number_suffixes() {
        let (_, out) = lex_text("42u64 10i32 7usize 1.5f32 2e10f64 0xffu8");
        assert!(out.diagnostics.is_empty());
        let suffixes: Vec<_> = out.tokens.iter().map(|t| t.number_suffix()).collect();
        assert_eq!(
            suffixes,
            [
                Some("u64"),
                Some("i32"),
                Some("usize"),
                Some("f32"),
                Some("f64"),
                Some("u8")
            ]
        );
        // `42f32` without a fraction is still a float.
        let (_, out) = lex_text("42f32");
        assert!(matches!(out.tokens[0].kind, TokenKind::Float { .. }));
        assert_eq!(out.tokens[0].number_suffix(), Some("f32"));
    }

    #[test]
    fn bad_suffix_reports_e104() {
        let (_, out) = lex_text("42qux");
        assert!(out
            .diagnostics
            .iter()
            .any(|d| d.code.as_ref().is_some_and(|c| c.0 == "E104")));
        // One token, not two: the run is consumed as the bad suffix.
        assert_eq!(out.tokens.len(), 1);
    }

    #[test]
    fn hex_and_unicode_escapes() {
        let (_, out) = lex_text(r#"'\x41' "A\x41" '\u{1F600}'"#);
        assert!(out.diagnostics.is_empty());
        assert!(matches!(out.tokens[0].kind, TokenKind::Char('A')));
        assert!(matches!(&out.tokens[1].kind, TokenKind::Str(s) if s == "AA"));
        assert!(matches!(out.tokens[2].kind, TokenKind::Char('\u{1F600}')));
    }

    #[test]
    fn bad_escapes_report_e105_and_recover() {
        for text in [
            r"'\x4'",
            r"'\xFF'",
            r"'\u{}'",
            r"'\u{D800}'",
            r"'\q'",
            r"'\u{0041",
        ] {
            let (_, out) = lex_text(text);
            assert!(
                out.diagnostics
                    .iter()
                    .any(|d| d.code.as_ref().is_some_and(|c| c.0 == "E105")),
                "no E105 for {text}"
            );
        }
    }

    #[test]
    fn reserved_future_keywords_are_keywords_not_idents() {
        let ks = kinds("async await dyn extern macro move ref try union where");
        assert!(ks.iter().all(|k| matches!(k, TokenKind::Keyword(_))));
        assert_eq!(ks.len(), 10);
    }

    #[test]
    fn new_punctuation_lexes() {
        let ks = kinds("~ # ? ..");
        assert!(matches!(ks[0], TokenKind::Tilde));
        assert!(matches!(ks[1], TokenKind::Hash));
        assert!(matches!(ks[2], TokenKind::Question));
        assert!(matches!(ks[3], TokenKind::DotDot));
    }
}
