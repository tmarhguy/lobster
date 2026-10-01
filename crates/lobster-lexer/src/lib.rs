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
    /// `10`, `0xff`, `0b1010`, `1_000_000` (raw spelling in [`Token::text`]).
    Int,
    /// `1.5`, `2e10` (raw spelling in [`Token::text`]).
    Float,
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

impl Token {
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
            // A trailing alphanumeric run (e.g. the `2` in `0b102`) is an
            // invalid digit for this radix, not a second token.
            let mut bad_tail = false;
            while self.pos < self.bytes.len()
                && (self.bytes[self.pos].is_ascii_alphanumeric() || self.bytes[self.pos] == b'_')
            {
                bad_tail = true;
                self.pos += 1;
            }
            if bad_tail || raw.is_empty() || u64::from_str_radix(&raw, radix).is_err() {
                let span = self.span_here(start, self.pos);
                self.error(
                    "E104",
                    "invalid number literal",
                    span,
                    format!("not a valid base-{radix} number"),
                );
            }
            self.push(TokenKind::Int, start, self.pos);
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
        self.push(
            if is_float {
                TokenKind::Float
            } else {
                TokenKind::Int
            },
            start,
            self.pos,
        );
    }

    /// Decode one escape starting after the backslash. Returns the char and
    /// the byte index just past the escape.
    fn escape(&mut self, backslash: usize) -> Option<(char, usize)> {
        // `backslash` is ASCII, so `backslash + 1` is a char boundary.
        let e = self.text.get(backslash + 1..)?.chars().next()?;
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
        assert!(matches!(ks[9], TokenKind::Int));
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
        assert!(matches!(out.tokens[4].kind, TokenKind::Float));
        assert!(matches!(out.tokens[5].kind, TokenKind::Float));
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
}
