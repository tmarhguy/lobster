//! Lobster source manager.
//!
//! Central source representation (file.md section 18).
//!
//! Invariants:
//! - Every loaded file gets a stable [`FileId`] that never changes.
//! - A [`Span`] is always a half-open byte range `[start, end)` into exactly
//!   one file, with `start <= end <= file.len()`.
//! - Line numbers are 1-based, columns are 1-based character counts
//!   (Unicode scalar values) from the start of the line.
//! - All diagnostics resolve locations through [`SourceManager`] so rendered
//!   `file:line:col` triples are stable for a given file content.

use std::ops::Range;

/// Stable identifier for a loaded source file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FileId(pub u32);

/// A half-open byte range in a single source file.
///
/// Byte offsets (not char offsets) so slicing `&str` is always safe.
/// Construct via [`SourceManager::span`] or [`Span::new`] (which does not
/// validate bounds — validation happens on lookup).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Span {
    /// File this span belongs to.
    pub file: FileId,
    /// Inclusive start byte offset.
    pub start: u32,
    /// Exclusive end byte offset.
    pub end: u32,
}

impl Span {
    /// Create a span without bounds checking.
    ///
    /// Prefer [`SourceManager::span`] when bounds matter.
    #[must_use]
    pub const fn new(file: FileId, start: u32, end: u32) -> Self {
        Self { file, start, end }
    }

    /// Empty span at `offset` (useful for insertions / EOF errors).
    #[must_use]
    pub const fn empty(file: FileId, offset: u32) -> Self {
        Self {
            file,
            start: offset,
            end: offset,
        }
    }

    /// Byte length of the span.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.end.saturating_sub(self.start) as usize
    }

    /// True when the span covers no bytes.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.start == self.end
    }

    /// Byte range form for slicing.
    #[must_use]
    pub fn range(&self) -> Range<usize> {
        self.start as usize..self.end as usize
    }
}

/// One loaded source file with a precomputed line table.
#[derive(Debug)]
pub struct SourceFile {
    id: FileId,
    name: String,
    text: String,
    /// Byte offset of the first byte of each line. Always starts with 0.
    line_starts: Vec<usize>,
}

impl SourceFile {
    fn new(id: FileId, name: String, text: String) -> Self {
        let mut line_starts = vec![0];
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i + 1);
            }
        }
        Self {
            id,
            name,
            text,
            line_starts,
        }
    }

    /// File identifier.
    #[must_use]
    pub const fn id(&self) -> FileId {
        self.id
    }

    /// Display name as given to [`SourceManager::add_file`].
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Full file text.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Number of lines (a trailing newline does not create an extra line).
    #[must_use]
    pub fn line_count(&self) -> usize {
        if self.text.is_empty() {
            return 0;
        }
        if self.text.ends_with('\n') {
            self.line_starts.len() - 1
        } else {
            self.line_starts.len()
        }
    }

    /// 1-based line number containing byte `offset`.
    ///
    /// Returns `None` when `offset > text.len()`.
    #[must_use]
    pub fn line_number(&self, offset: usize) -> Option<usize> {
        if offset > self.text.len() {
            return None;
        }
        // Last line_start <= offset.
        let line = self
            .line_starts
            .partition_point(|&s| s <= offset)
            .saturating_sub(1);
        Some(line + 1)
    }

    /// Byte range of 1-based line `line`. Returns `None` when out of range.
    #[must_use]
    pub fn line_range(&self, line: usize) -> Option<Range<usize>> {
        if line == 0 || line > self.line_count() {
            return None;
        }
        let start = self.line_starts[line - 1];
        let end = if line < self.line_starts.len() {
            // Strip the trailing '\n' for display purposes, but keep other content.
            let raw_end = self.line_starts[line];
            if raw_end > start && self.text.as_bytes()[raw_end - 1] == b'\n' {
                raw_end - 1
            } else {
                raw_end
            }
        } else {
            self.text.len()
        };
        Some(start..end)
    }

    /// Text of 1-based line `line` without the trailing newline.
    #[must_use]
    pub fn line_text(&self, line: usize) -> Option<&str> {
        self.line_range(line).map(|r| &self.text[r])
    }

    /// 1-based `(line, column)` for byte `offset`.
    ///
    /// Column counts Unicode scalar values from the line start, so it stays
    /// stable and human-meaningful for non-ASCII sources.
    #[must_use]
    pub fn line_col(&self, offset: usize) -> Option<(usize, usize)> {
        let line = self.line_number(offset)?;
        let start = self.line_starts[line - 1];
        let col = self.text[start..offset].chars().count() + 1;
        Some((line, col))
    }
}

/// Owns all loaded sources and resolves spans to locations.
#[derive(Debug, Default)]
pub struct SourceManager {
    files: Vec<SourceFile>,
}

impl SourceManager {
    /// Create an empty manager.
    #[must_use]
    pub fn new() -> Self {
        Self { files: Vec::new() }
    }

    /// Load a file's text, returning its stable [`FileId`].
    pub fn add_file(&mut self, name: impl Into<String>, text: impl Into<String>) -> FileId {
        let id = FileId(self.files.len() as u32);
        self.files
            .push(SourceFile::new(id, name.into(), text.into()));
        id
    }

    /// Look up a file by id.
    #[must_use]
    pub fn get(&self, id: FileId) -> Option<&SourceFile> {
        self.files.get(id.0 as usize)
    }

    /// Number of loaded files.
    #[must_use]
    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    /// Build a span, clamping to the file length. Returns `None` for an
    /// unknown file or when `start > end`.
    #[must_use]
    pub fn span(&self, file: FileId, start: usize, end: usize) -> Option<Span> {
        let f = self.get(file)?;
        if start > end || end > f.text().len() {
            return None;
        }
        Some(Span::new(file, start as u32, end as u32))
    }

    /// Slice the text covered by `span`. Returns `None` for invalid spans
    /// (unknown file, out of bounds, or non-char-boundary).
    #[must_use]
    pub fn slice(&self, span: Span) -> Option<&str> {
        let f = self.get(span.file)?;
        f.text().get(span.range())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_manager_has_no_files() {
        let sm = SourceManager::new();
        assert_eq!(sm.file_count(), 0);
        assert!(sm.get(FileId(0)).is_none());
    }

    #[test]
    fn file_ids_are_stable_and_sequential() {
        let mut sm = SourceManager::new();
        let a = sm.add_file("a.lobster", "let x = 1;\n");
        let b = sm.add_file("b.lobster", "let y = 2;\n");
        assert_eq!((a.0, b.0), (0, 1));
        assert_eq!(sm.get(a).unwrap().name(), "a.lobster");
        assert_eq!(sm.file_count(), 2);
    }

    #[test]
    fn line_table_tracks_newlines() {
        let mut sm = SourceManager::new();
        let id = sm.add_file("demo.lobster", "ab\ncde\nf");
        let f = sm.get(id).unwrap();
        assert_eq!(f.line_count(), 3);
        assert_eq!(f.line_text(1), Some("ab"));
        assert_eq!(f.line_text(2), Some("cde"));
        assert_eq!(f.line_text(3), Some("f"));
        assert_eq!(f.line_text(4), None);
    }

    #[test]
    fn line_col_is_one_based() {
        let mut sm = SourceManager::new();
        let id = sm.add_file("demo.lobster", "ab\ncde\n");
        let f = sm.get(id).unwrap();
        assert_eq!(f.line_col(0), Some((1, 1)));
        assert_eq!(f.line_col(3), Some((2, 1)));
        assert_eq!(f.line_col(4), Some((2, 2)));
        assert_eq!(f.line_col(100), None);
    }

    #[test]
    fn column_counts_chars_not_bytes() {
        let mut sm = SourceManager::new();
        // 'é' is 2 bytes, 1 char.
        let id = sm.add_file("u.lobster", "éx\n");
        let f = sm.get(id).unwrap();
        // Byte offset 2 is start of 'x' (é takes bytes 0..2).
        assert_eq!(f.line_col(2), Some((1, 2)));
        assert_eq!(f.line_col(3), Some((1, 3)));
    }

    #[test]
    fn span_slice_round_trips() {
        let mut sm = SourceManager::new();
        let id = sm.add_file("demo.lobster", "let x = 10;");
        let span = sm.span(id, 4, 5).unwrap();
        assert_eq!(sm.slice(span), Some("x"));
        assert!(sm.span(id, 5, 4).is_none());
        assert!(sm.span(id, 0, 100).is_none());
        assert!(sm.span(FileId(99), 0, 1).is_none());
    }

    #[test]
    fn span_len_and_empty() {
        let s = Span::new(FileId(0), 3, 7);
        assert_eq!(s.len(), 4);
        assert!(!s.is_empty());
        assert!(Span::empty(FileId(0), 5).is_empty());
    }
}
