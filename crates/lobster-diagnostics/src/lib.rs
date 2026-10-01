//! Lobster diagnostics (file.md section 21).
//!
//! Every diagnostic carries a stable [`DiagnosticCode`], a primary labeled
//! span, optional secondary spans, notes, and an optional suggestion.
//! Rendering goes through [`Renderer`] so CLI, LSP, and tests share one
//! format:
//!
//! ```text
//! error[E021]: mismatched types
//!
//!   --> demo.lobster:17:18
//!    |
//! 17 |     let x: i32 = "hello";
//!    |            ---   ^^^^^^^ expected i32, found string
//! ```

use lobster_source::{SourceManager, Span};
use std::fmt::Write as _;

/// Severity of a diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Error,
    Warning,
    Note,
}

impl Level {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Note => "note",
        }
    }
}

/// Stable machine-readable diagnostic code, e.g. `E021` or `LOBSTER-001`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticCode(pub String);

impl DiagnosticCode {
    /// Create a code from any string.
    pub fn new(code: impl Into<String>) -> Self {
        Self(code.into())
    }
}

/// One labeled source range inside a diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Label {
    /// Source range the label points at.
    pub span: Span,
    /// Message shown under the carets, e.g. `expected i32, found string`.
    pub message: String,
}

impl Label {
    /// Primary label pointing at `span` with an explanatory message.
    pub fn primary(span: Span, message: impl Into<String>) -> Self {
        Self {
            span,
            message: message.into(),
        }
    }

    /// Secondary label (context, e.g. a prior definition).
    pub fn secondary(span: Span, message: impl Into<String>) -> Self {
        Self {
            span,
            message: message.into(),
        }
    }
}

/// A complete compiler diagnostic.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    /// Severity.
    pub level: Level,
    /// Stable code (`None` only for ad-hoc internal messages; all user-facing
    /// diagnostics must carry a code).
    pub code: Option<DiagnosticCode>,
    /// Short headline, e.g. `mismatched types`.
    pub message: String,
    /// The main labeled span.
    pub primary: Label,
    /// Additional context spans.
    pub secondaries: Vec<Label>,
    /// Free-form `note:` lines.
    pub notes: Vec<String>,
    /// Optional `suggestion:` line.
    pub suggestion: Option<String>,
}

impl Diagnostic {
    /// Start building an error diagnostic.
    pub fn error(code: impl Into<String>, message: impl Into<String>, primary: Label) -> Self {
        Self {
            level: Level::Error,
            code: Some(DiagnosticCode::new(code)),
            message: message.into(),
            primary,
            secondaries: Vec::new(),
            notes: Vec::new(),
            suggestion: None,
        }
    }

    /// Start building a warning diagnostic.
    pub fn warning(code: impl Into<String>, message: impl Into<String>, primary: Label) -> Self {
        Self {
            level: Level::Warning,
            code: Some(DiagnosticCode::new(code)),
            message: message.into(),
            primary,
            secondaries: Vec::new(),
            notes: Vec::new(),
            suggestion: None,
        }
    }

    /// Attach a secondary context label.
    #[must_use]
    pub fn with_secondary(mut self, label: Label) -> Self {
        self.secondaries.push(label);
        self
    }

    /// Attach a `note:` line.
    #[must_use]
    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }

    /// Attach a `suggestion:` line.
    #[must_use]
    pub fn with_suggestion(mut self, suggestion: impl Into<String>) -> Self {
        self.suggestion = Some(suggestion.into());
        self
    }
}

/// Renders diagnostics against a [`SourceManager`].
///
/// The format is intentionally close to `rustc` so Lobster diagnostics feel
/// familiar from day one.
pub struct Renderer<'a> {
    sources: &'a SourceManager,
}

impl<'a> Renderer<'a> {
    /// Borrow the source manager that all rendered spans resolve through.
    pub const fn new(sources: &'a SourceManager) -> Self {
        Self { sources }
    }

    /// Render one diagnostic to a string (used by tests and the CLI).
    #[must_use]
    pub fn render(&self, d: &Diagnostic) -> String {
        let mut out = String::new();
        let code = d
            .code
            .as_ref()
            .map_or_else(String::new, |c| format!("[{}]", c.0));
        let _ = writeln!(out, "{}{code}: {}", d.level.as_str(), d.message);
        out.push('\n');

        // Header: --> file:line:col (points at primary span start).
        let (loc, line_no, line_text) = self.primary_location(d);
        let _ = writeln!(out, "  --> {loc}");
        out.push_str("   |\n");

        if let (Some(no), Some(text)) = (line_no, line_text) {
            let gutter = format!("{no} | ");
            out.push_str(&gutter);
            out.push_str(&text);
            out.push('\n');
            out.push_str(&" ".repeat(gutter.len()));
            out.push_str(&self.caret_line(d, no));
            out.push('\n');
        }

        for label in &d.secondaries {
            if let Some((loc, _, _)) = self.span_location(label.span) {
                let _ = writeln!(out, "   = secondary: {loc}: {}", label.message);
            }
        }
        for note in &d.notes {
            let _ = writeln!(out, "   = note: {note}");
        }
        if let Some(s) = &d.suggestion {
            let _ = writeln!(out, "   = suggestion: {s}");
        }
        out
    }

    fn primary_location(&self, d: &Diagnostic) -> (String, Option<usize>, Option<String>) {
        match self.span_location(d.primary.span) {
            Some((loc, line, text)) => (loc, Some(line), Some(text)),
            None => ("<unknown location>".to_string(), None, None),
        }
    }

    fn span_location(&self, span: Span) -> Option<(String, usize, String)> {
        let file = self.sources.get(span.file)?;
        let (line, col) = file.line_col(span.start as usize)?;
        let text = file.line_text(line).unwrap_or_default().to_string();
        Some((format!("{}:{line}:{col}", file.name()), line, text))
    }

    /// Build the `---   ^^^^^^^ message` caret line for the primary span.
    ///
    /// Only spans on a single line get inline carets; multi-line primaries
    /// degrade to a message line (full multi-line rendering is a later
    /// milestone).
    fn caret_line(&self, d: &Diagnostic, line: usize) -> String {
        let Some(file) = self.sources.get(d.primary.span.file) else {
            return d.primary.message.clone();
        };
        let Some(range) = file.line_range(line) else {
            return d.primary.message.clone();
        };
        let start = d.primary.span.start as usize;
        let end = d.primary.span.end as usize;
        if start < range.start || end > range.end || start > end {
            return d.primary.message.clone();
        }
        // Column offset in characters (matches SourceFile::line_col).
        let prefix_chars = file.text()[range.start..start].chars().count();
        let span_chars = file.text()[start..end].chars().count().max(1);
        format!(
            "{}{} {}",
            " ".repeat(prefix_chars),
            "^".repeat(span_chars),
            d.primary.message
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lobster_source::SourceManager;

    fn fixture() -> (SourceManager, Span, Span) {
        let mut sm = SourceManager::new();
        let text = "fn main() {\n    let x: i32 = \"hello\";\n}\n";
        let id = sm.add_file("demo.lobster", text);
        let x_span = sm.span(id, 20, 21).unwrap(); // `x`
        let hello_span = sm.span(id, 29, 36).unwrap(); // `"hello"`
        (sm, x_span, hello_span)
    }

    #[test]
    fn renders_spec_example_shape() {
        let (sm, x_span, hello_span) = fixture();
        let d = Diagnostic::error(
            "E021",
            "mismatched types",
            Label::primary(hello_span, "expected i32, found string"),
        )
        .with_secondary(Label::secondary(x_span, "declared here"))
        .with_note("opaque note for snapshot");
        let out = Renderer::new(&sm).render(&d);
        assert!(out.contains("error[E021]: mismatched types"), "{out}");
        assert!(out.contains("--> demo.lobster:2:"), "{out}");
        assert!(out.contains("expected i32, found string"), "{out}");
        assert!(out.contains("secondary: demo.lobster:"), "{out}");
        assert!(out.contains("note: opaque note for snapshot"), "{out}");
        // Snapshot the full rendering: any format change must be deliberate.
        insta::assert_snapshot!(out);
    }

    #[test]
    fn warning_and_suggestion_render() {
        let (sm, _, hello_span) = fixture();
        let d = Diagnostic::warning("W001", "unused value", Label::primary(hello_span, "here"))
            .with_suggestion("prefix with _");
        let out = Renderer::new(&sm).render(&d);
        assert!(out.contains("warning[W001]"), "{out}");
        assert!(out.contains("suggestion: prefix with _"), "{out}");
    }
}
