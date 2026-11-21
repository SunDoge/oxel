use std::{fs, path::Path, sync::Arc};

use lsp_types::{Position, Range, TextDocumentContentChangeEvent, TextEdit};
use oxel::{DocumentAnalysis, Occurrence, Span};

pub(super) struct Document {
    pub text: Arc<str>,
    lines: Lines,
    pub version: Option<i32>,
    pub analysis: DocumentAnalysis,
}

impl Document {
    pub fn new(text: String, version: Option<i32>) -> Self {
        let lines = Lines::new(&text);
        Self {
            text: Arc::from(text),
            lines,
            version,
            analysis: DocumentAnalysis::default(),
        }
    }

    pub fn matches_file(&self, path: &Path) -> bool {
        fs::read_to_string(path).is_ok_and(|text| text == *self.text)
    }

    pub fn change(
        &mut self,
        version: i32,
        changes: Vec<TextDocumentContentChangeEvent>,
    ) -> Option<bool> {
        if version <= self.version? {
            return None;
        }
        // Apply the whole notification before publishing a new snapshot.
        let mut text = self.text.to_string();
        for change in changes {
            if let Some(range) = change.range {
                let lines = Lines::new(&text);
                let (start, end) = lines
                    .offset(&text, range.start)
                    .zip(lines.offset(&text, range.end))?;
                if start > end {
                    return None;
                }
                text.replace_range(start..end, &change.text);
            } else {
                text = change.text;
            }
        }
        let changed = text != *self.text;
        if changed {
            *self = Self::new(text, Some(version));
        } else {
            self.version = Some(version);
        }
        Some(changed)
    }

    pub fn format(&self) -> Option<TextEdit> {
        let formatted = oxel::format_source(&self.text).ok()?;
        (formatted != *self.text).then(|| {
            TextEdit::new(
                self.range(Span {
                    start: 0,
                    end: self.text.len(),
                }),
                formatted,
            )
        })
    }

    pub fn occurrence(&self, position: Position) -> Option<&Occurrence> {
        let byte = self.offset(position)?;
        let occurrences = &self.analysis.occurrences;
        let upper = occurrences.partition_point(|item| item.span.start <= byte);
        let item = occurrences.get(upper.checked_sub(1)?)?;
        (byte < item.span.end).then_some(item)
    }

    pub fn offset(&self, position: Position) -> Option<usize> {
        self.lines.offset(&self.text, position)
    }

    pub fn range(&self, span: Span) -> Range {
        self.range_after(span, 0, Position::default())
    }

    /// Convert an ordered span using the last converted byte and position.
    pub fn range_after(&self, span: Span, byte: usize, position: Position) -> Range {
        let start = self.lines.position(&self.text, span.start, byte, position);
        let end = self.lines.position(&self.text, span.end, span.start, start);
        Range::new(start, end)
    }
}

struct Lines {
    starts: Vec<usize>,
    ends: Vec<usize>,
}

impl Lines {
    fn new(text: &str) -> Self {
        let mut starts = vec![0];
        let mut ends = Vec::new();
        let mut bytes = text.bytes().enumerate().peekable();
        while let Some((offset, byte)) = bytes.next() {
            if matches!(byte, b'\n' | b'\r') {
                ends.push(offset);
                let end_of_break =
                    if byte == b'\r' && bytes.peek().is_some_and(|(_, next)| *next == b'\n') {
                        bytes.next().expect("checked CRLF pair").0
                    } else {
                        offset
                    };
                starts.push(end_of_break + 1);
            }
        }
        ends.push(text.len());
        Self { starts, ends }
    }

    fn position(&self, text: &str, byte: usize, previous: usize, position: Position) -> Position {
        let line = self.starts.partition_point(|&start| start <= byte) - 1;
        let end = byte.min(self.ends[line]);
        let (start, column) = if line == position.line as usize {
            (previous.min(self.ends[line]), position.character)
        } else {
            (self.starts[line], 0)
        };
        Position::new(
            line as u32,
            column + text[start..end].encode_utf16().count() as u32,
        )
    }

    fn offset(&self, text: &str, position: Position) -> Option<usize> {
        let line = position.line as usize;
        let start = *self.starts.get(line)?;
        let end = self.ends[line];
        let target = position.character as usize;
        let mut column = 0;
        for (offset, character) in text[start..end].char_indices() {
            if column == target {
                return Some(start + offset);
            }
            column += character.len_utf16();
            if column > target {
                return None; // A UTF-16 surrogate pair cannot be split.
            }
        }
        Some(end)
    }
}
