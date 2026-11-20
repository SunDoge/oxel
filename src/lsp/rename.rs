use std::collections::HashMap;

use lsp_server::ErrorCode;
use lsp_types::{
    PrepareRenameResponse, RenameParams, TextDocumentPositionParams, TextEdit, Uri, WorkspaceEdit,
};
use oxel::{Occurrence, OccurrenceKind, Span, SymbolKind, inspect_project, valid_identifier};

use super::{Session, document::Document};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct BindingSite {
    file: usize,
    span: Span,
}

impl BindingSite {
    fn declaration(file: usize, span: Span) -> Self {
        Self { file, span }
    }

    fn reference(item: &Occurrence) -> Option<Self> {
        Some(Self {
            file: item.definition_source?,
            span: item.definition?,
        })
    }

    fn of(file: usize, item: &Occurrence) -> Option<Self> {
        if item.is_declaration {
            Some(Self::declaration(file, item.span))
        } else {
            Self::reference(item)
        }
    }
}

pub(super) struct SymbolOccurrences<'a> {
    files: Vec<(&'a Uri, &'a Document)>,
    binding: BindingSite,
    document: &'a Document,
    selection: Span,
    reserves_main: bool,
}

impl<'a> SymbolOccurrences<'a> {
    pub(super) fn matching(
        &self,
        include_declaration: bool,
    ) -> impl Iterator<Item = (&'a Uri, &'a Document, &'a Occurrence)> + '_ {
        self.files
            .iter()
            .enumerate()
            .flat_map(move |(file, &(uri, document))| {
                document
                    .analysis
                    .occurrences
                    .iter()
                    .filter_map(move |item| {
                        let matches = (!item.is_declaration || include_declaration)
                            && BindingSite::of(file, item) == Some(self.binding);
                        matches.then_some((uri, document, item))
                    })
            })
    }

    fn name(&self) -> Option<&'a str> {
        let &(_, document) = self.files.get(self.binding.file)?;
        document
            .text
            .get(self.binding.span.start..self.binding.span.end)
    }

    fn is_function(&self) -> bool {
        self.files[self.binding.file]
            .1
            .analysis
            .symbols
            .iter()
            .any(|symbol| {
                symbol.kind == SymbolKind::Function && symbol.name_span == self.binding.span
            })
    }

    fn can_rename(&self) -> bool {
        let Some(name) = self.name() else {
            return false;
        };
        valid_identifier(name)
            && name != "_"
            && !(self.reserves_main && name == "main" && self.is_function())
            && self
                .document
                .text
                .get(self.selection.start..self.selection.end)
                == Some(name)
            && self
                .files
                .iter()
                .all(|(_, document)| document.analysis.diagnostics.is_empty())
    }
}

type RenameError = (ErrorCode, String);

fn rejected(message: &str) -> RenameError {
    (ErrorCode::RequestFailed, message.to_owned())
}

impl Session<'_> {
    pub(super) fn symbol_occurrences<'a>(
        &'a self,
        point: &'a TextDocumentPositionParams,
    ) -> Option<SymbolOccurrences<'a>> {
        let uri = &point.text_document.uri;
        let document = self.documents.get(uri.as_str())?;
        let occurrence = document.occurrence(point.position)?;
        let project = self.workspace.iter().position(|(file, _)| file == uri);
        let binding = BindingSite::of(project.unwrap_or(0), occurrence)?;
        let files = if project.is_some() {
            self.workspace
                .iter()
                .map(|(file, _)| (file, &self.documents[file.as_str()]))
                .collect()
        } else {
            vec![(uri, document)]
        };
        Some(SymbolOccurrences {
            files,
            binding,
            document,
            selection: occurrence.span,
            reserves_main: project.is_none() || self.kind == oxel::AssemblyKind::Executable,
        })
    }

    pub(super) fn prepare_rename(
        &self,
        point: &TextDocumentPositionParams,
    ) -> Option<PrepareRenameResponse> {
        let symbol = self.symbol_occurrences(point)?;
        symbol
            .can_rename()
            .then(|| PrepareRenameResponse::RangeWithPlaceholder {
                range: symbol.document.range(symbol.selection),
                placeholder: symbol.name().expect("source binding").to_owned(),
            })
    }

    pub(super) fn rename(&self, params: &RenameParams) -> Result<WorkspaceEdit, RenameError> {
        let new_name = &params.new_name;
        if !valid_identifier(new_name) || new_name == "_" {
            return Err((
                ErrorCode::InvalidParams,
                format!("'{new_name}' is not a valid name for this rename"),
            ));
        }
        let symbol = self
            .symbol_occurrences(&params.text_document_position)
            .filter(SymbolOccurrences::can_rename)
            .ok_or_else(|| rejected("Rename requires a source symbol in an error-free module"))?;
        let old_name = symbol.name().expect("source binding");
        if symbol.reserves_main && new_name == "main" && symbol.is_function() {
            return Err(rejected("The name 'main' is reserved for the entry point"));
        }
        if old_name == new_name {
            return Ok(self.workspace_edit([]));
        }
        let uri = &params.text_document_position.text_document.uri;
        if self.is_project_file(uri) {
            for (file, path) in &self.workspace {
                let document = &self.documents[file.as_str()];
                if document.version.is_none() && !document.matches_file(path) {
                    return Err((
                        ErrorCode::ContentModified,
                        format!(
                            "{} changed on disk. Open it to refresh the module.",
                            path.display()
                        ),
                    ));
                }
            }
        }

        let mut sites = HashMap::<&str, Vec<(Span, Replacement)>>::new();
        for (file, document, item) in symbol.matching(true) {
            let span = item.span;
            if document.text.get(span.start..span.end) != Some(old_name) {
                return Err(rejected("The symbol has a reference that cannot be edited"));
            }
            let replacement = if item.kind == OccurrenceKind::ShorthandField {
                Replacement::Field
            } else {
                let occurrences = &document.analysis.occurrences;
                let first = occurrences.partition_point(|item| item.span.start < span.start);
                if occurrences[first..]
                    .iter()
                    .take_while(|item| item.span.start == span.start)
                    .any(|item| item.span == span && item.kind == OccurrenceKind::ShorthandField)
                {
                    Replacement::Binding
                } else {
                    Replacement::Name
                }
            };
            sites
                .entry(file.as_str())
                .or_default()
                .push((span, replacement));
        }
        let revised = symbol
            .files
            .iter()
            .map(|&(file, document)| {
                let mut spans = sites.remove(file.as_str()).unwrap_or_default();
                spans.sort_unstable_by_key(|(span, _)| (span.start, span.end));
                spans.dedup();
                RevisedDocument::new(document, spans, old_name, new_name)
            })
            .collect::<Vec<_>>();
        let sources = revised
            .iter()
            .map(|document| document.text.as_str())
            .collect::<Vec<_>>();
        let analyses = inspect_project(&sources, self.references);
        for ((_, original), (updated, analysis)) in
            symbol.files.iter().zip(revised.iter().zip(analyses))
        {
            if let Some(error) = analysis.diagnostics.first() {
                return Err(rejected(&format!(
                    "Rename would make the module invalid: {}",
                    error.message
                )));
            }
            // The edited program can still type check if one binding takes over
            // another's uses. Compare definition links after translating their spans.
            let expected = occurrence_keys(
                original.analysis.occurrences.iter(),
                |item| updated.translate(item.span, item.kind == OccurrenceKind::ShorthandField),
                |site| revised[site.file].translate(site.span, false),
            );
            let actual = occurrence_keys(
                analysis.occurrences.iter(),
                |item| item.span,
                |site| site.span,
            );
            if expected != actual {
                return Err(rejected(
                    "Rename would change which declaration a reference resolves to",
                ));
            }
        }
        let edits = symbol
            .files
            .iter()
            .zip(revised)
            .filter_map(|(&(file, document), updated)| {
                let edits: Vec<_> = updated
                    .edits
                    .iter()
                    .map(|edit| TextEdit {
                        range: document.range(edit.span),
                        new_text: updated.text[edit.new_start..edit.new_end].to_owned(),
                    })
                    .collect();
                (!edits.is_empty()).then_some((file.clone(), document.version, edits))
            });
        Ok(self.workspace_edit(edits))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Replacement {
    Name,
    Field,
    Binding,
}

struct Edit {
    span: Span,
    new_start: usize,
    new_end: usize,
    field_end: Option<usize>,
}

struct RevisedDocument {
    text: String,
    edits: Vec<Edit>,
}

impl RevisedDocument {
    fn new(
        document: &Document,
        spans: Vec<(Span, Replacement)>,
        old_name: &str,
        new_name: &str,
    ) -> Self {
        let mut text = String::with_capacity(document.text.len());
        let mut cursor = 0;
        let mut edits = Vec::with_capacity(spans.len());
        for (span, replacement) in spans {
            text.push_str(&document.text[cursor..span.start]);
            let new_start = text.len();
            let field_end = match replacement {
                Replacement::Name => {
                    text.push_str(new_name);
                    None
                }
                Replacement::Field | Replacement::Binding => {
                    let (field, value) = if replacement == Replacement::Field {
                        (new_name, old_name)
                    } else {
                        (old_name, new_name)
                    };
                    text.push_str(field);
                    let field_end = text.len();
                    text.push_str(": ");
                    text.push_str(value);
                    Some(field_end)
                }
            };
            edits.push(Edit {
                span,
                new_start,
                new_end: text.len(),
                field_end,
            });
            cursor = span.end;
        }
        text.push_str(&document.text[cursor..]);
        Self { text, edits }
    }

    fn translate(&self, span: Span, shorthand_field: bool) -> Span {
        let index = self
            .edits
            .partition_point(|edit| edit.span.start < span.start);
        if let Some(edit) = self.edits.get(index).filter(|edit| edit.span == span)
            && let Some(field_end) = edit.field_end
        {
            return if shorthand_field {
                Span {
                    start: edit.new_start,
                    end: field_end,
                }
            } else {
                Span {
                    start: field_end + ": ".len(),
                    end: edit.new_end,
                }
            };
        }
        let offset = |offset| {
            let count = self.edits.partition_point(|edit| edit.span.end <= offset);
            if count == 0 {
                offset
            } else {
                let edit = &self.edits[count - 1];
                edit.new_end + (offset - edit.span.end)
            }
        };
        Span {
            start: offset(span.start),
            end: offset(span.end),
        }
    }
}

type OccurrenceKey = ((usize, usize), bool, Option<(usize, (usize, usize))>);

fn occurrence_keys<'a>(
    occurrences: impl Iterator<Item = &'a Occurrence>,
    occurrence_span: impl Fn(&Occurrence) -> Span,
    definition_span: impl Fn(BindingSite) -> Span,
) -> Vec<OccurrenceKey> {
    let span_key = |span: Span| (span.start, span.end);
    let mut keys = occurrences
        .map(|item| {
            (
                span_key(occurrence_span(item)),
                item.is_declaration,
                BindingSite::reference(item)
                    .map(|site| (site.file, span_key(definition_span(site)))),
            )
        })
        .collect::<Vec<_>>();
    keys.sort_unstable();
    keys.dedup();
    keys
}
