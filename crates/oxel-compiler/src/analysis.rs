//! Compiler snapshots for editor buffers, including recoverable declarations.

use crate::AssemblyKind;

mod body_cache;
mod incremental;
pub(crate) mod schedule;
mod shape;

pub(crate) use body_cache::{BodyCache, Lookup};
pub(crate) use shape::{signature_type_names, type_names};

pub use incremental::ProjectAnalyzer;

use crate::{
    Diagnostic, References, Span, hir,
    project::{Project, SourceMap},
    syntax,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SymbolKind {
    Function,
    Constant,
    Struct,
    Enum,
    EnumMember,
    Field,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    pub span: Span,
    pub name_span: Span,
    pub children: Vec<Symbol>,
}

/// Semantic classification at a source occurrence, independent of its hover text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OccurrenceKind {
    Function,
    Method,
    Constant,
    Parameter,
    Local {
        mutable: bool,
    },
    BuiltinType,
    TypeParameter,
    Struct,
    Enum,
    EnumMember,
    Class,
    Interface,
    Field {
        readonly: bool,
    },
    /// Field reference sharing a token with a local or constant in struct shorthand.
    ShorthandField,
    Property,
    Event,
    Operator,
    /// Hover information for a constructor already represented by its type.
    Constructor,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Occurrence {
    pub span: Span,
    pub kind: OccurrenceKind,
    pub definition: Option<Span>,
    /// Index of the definition's input in the project source slice.
    pub definition_source: Option<usize>,
    pub detail: String,
    /// Parameter ranges are byte offsets in `detail`, not source spans.
    pub call_parameters: Option<CallParameters>,
    pub is_declaration: bool,
    /// For local binding declarations, the inclusive cursor range where the binding is in scope.
    pub visibility: Option<Span>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallParameters {
    pub ranges: Box<[Span]>,
    pub variadic: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub struct CallableSignature {
    pub label: String,
    pub parameters: CallParameters,
}

impl CallableSignature {
    pub(crate) fn new(
        mut label: String,
        parameters: impl IntoIterator<Item = String>,
        result: &str,
        variadic: bool,
    ) -> Self {
        label.push('(');
        let mut ranges = Vec::new();
        for parameter in parameters {
            if !ranges.is_empty() {
                label.push_str(", ");
            }
            let start = label.len();
            label.push_str(&parameter);
            ranges.push(Span {
                start,
                end: label.len(),
            });
        }
        label.push_str(") -> ");
        label.push_str(result);
        Self {
            label,
            parameters: CallParameters {
                ranges: ranges.into_boxed_slice(),
                variadic,
            },
        }
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct DocumentAnalysis {
    pub diagnostics: Vec<Diagnostic>,
    pub symbols: Vec<Symbol>,
    pub occurrences: Vec<Occurrence>,
}

/// Analyzes an editor buffer using the compiler parser and semantic engine.
/// A buffer may contain declarations without a program entry point.
pub fn inspect_document(source: &str, references: &References) -> DocumentAnalysis {
    inspect_project(&[source], references)
        .pop()
        .expect("one document")
}

/// Analyzes documents as one module and returns one snapshot per input.
/// Occurrence and definition spans are both local to their respective files.
pub fn inspect_project(sources: &[&str], references: &References) -> Vec<DocumentAnalysis> {
    inspect_project_with_kind(sources, references, AssemblyKind::Executable)
}

/// Analyzes fresh source snapshots using the selected assembly's entry point rules.
pub fn inspect_project_with_kind(
    sources: &[&str],
    references: &References,
    kind: AssemblyKind,
) -> Vec<DocumentAnalysis> {
    let project = Project::new(sources);
    let (files, diagnostics) = project.parse();
    inspect_parsed(
        &project.map,
        syntax::Module::new(&files),
        diagnostics,
        references,
        kind,
        None,
    )
}

fn inspect_parsed(
    map: &SourceMap,
    module: syntax::Module<'_, '_>,
    mut diagnostics: Vec<Diagnostic>,
    references: &References,
    kind: AssemblyKind,
    cache: Option<&mut BodyCache>,
) -> Vec<DocumentAnalysis> {
    let symbols = outline(&module);
    let mut occurrences = Vec::new();
    if diagnostics.is_empty() {
        diagnostics = hir::inspect(module, references, kind, &mut occurrences, cache);
    }

    let mut documents: Vec<_> = map
        .bases
        .iter()
        .map(|_| DocumentAnalysis::default())
        .collect();
    diagnostics.sort_by_key(|error| error.span.start);
    for error in diagnostics {
        let error = map.locate(error);
        if let Some(source) = error.source {
            documents[source].diagnostics.push(error.diagnostic);
        }
    }
    for mut symbol in symbols {
        let source = localize_symbol(map, &mut symbol);
        documents[source].symbols.push(symbol);
    }
    occurrences.sort_by_key(|occurrence| occurrence.span.start);
    for mut occurrence in occurrences {
        let (source, span) = map.local_span(occurrence.span).expect("occurrence file");
        occurrence.span = span;
        if let Some(visibility) = occurrence.visibility {
            occurrence.visibility = map.local_span(visibility).map(|(_, span)| span);
        }
        if let Some(definition) = occurrence.definition {
            let (file, span) = map.local_span(definition).expect("definition file");
            occurrence.definition_source = Some(file);
            occurrence.definition = Some(span);
        }
        documents[source].occurrences.push(occurrence);
    }
    documents
}

fn localize_symbol(map: &SourceMap, symbol: &mut Symbol) -> usize {
    let (source, range) = map.local_span(symbol.span).expect("symbol file");
    let (_, name) = map.local_span(symbol.name_span).expect("symbol name file");
    symbol.span = range;
    symbol.name_span = name;
    for child in &mut symbol.children {
        localize_symbol(map, child);
    }
    source
}

fn outline(file: &syntax::Module<'_, '_>) -> Vec<Symbol> {
    let mut symbols =
        Vec::with_capacity(file.functions.len() + file.types.len() + file.constants.len());
    for function in &file.functions {
        symbols.push(Symbol {
            name: function.name.into(),
            kind: SymbolKind::Function,
            span: function.declaration_span,
            name_span: function.name_span,
            children: Vec::new(),
        });
    }
    for definition in &file.constants {
        symbols.push(Symbol {
            name: definition.name.into(),
            kind: SymbolKind::Constant,
            span: definition.declaration_span,
            name_span: definition.name_span,
            children: Vec::new(),
        });
    }
    for definition in &file.types {
        let (kind, children) = match &definition.shape {
            syntax::TypeShape::Struct(fields) => (
                SymbolKind::Struct,
                fields
                    .iter()
                    .map(|(name, span, ty)| Symbol {
                        name: (*name).into(),
                        kind: SymbolKind::Field,
                        span: Span {
                            start: span.start,
                            end: ty.span.end,
                        },
                        name_span: *span,
                        children: Vec::new(),
                    })
                    .collect(),
            ),
            syntax::TypeShape::Union(variants) => (
                SymbolKind::Enum,
                variants
                    .iter()
                    .map(|variant| Symbol {
                        name: variant.name.into(),
                        kind: SymbolKind::EnumMember,
                        span: variant.declaration_span,
                        name_span: variant.span,
                        children: Vec::new(),
                    })
                    .collect(),
            ),
        };
        symbols.push(Symbol {
            name: definition.name.into(),
            kind,
            span: definition.declaration_span,
            name_span: definition.span,
            children,
        });
    }
    symbols.sort_by_key(|symbol| symbol.span.start);

    symbols
}
