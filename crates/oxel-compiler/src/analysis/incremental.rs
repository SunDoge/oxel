//! Keeps parsed source files alive between project analysis requests.

use std::sync::Arc;

use self_cell::self_cell;

use super::{
    BodyCache, DocumentAnalysis,
    body_cache::{FunctionInput, TypeInput},
    inspect_parsed, inspect_project_with_kind, shape,
};
use crate::{AssemblyKind, Diagnostic, References, project::SourceMap, syntax};

struct Parsed<'src> {
    file: syntax::File<'src>,
    diagnostics: Vec<Diagnostic>,
    types: Vec<TypeInput<'src>>,
    functions: Vec<FunctionInput<'src>>,
    constant_sources: Vec<(&'src str, blake3::Hash)>,
}

impl<'src> Parsed<'src> {
    fn new(source: &'src str, base: usize) -> Self {
        let (file, diagnostics) = syntax::parse_at(source, base);
        let text = |start: usize, end: usize| &source.as_bytes()[start - base..end - base];
        let mut functions = Vec::with_capacity(file.functions.len());
        for function in &file.functions {
            let start = function.declaration_span.start;
            functions.push(FunctionInput {
                name: function.name,
                signature: shape::signature(function),
                body: blake3::hash(text(start, function.declaration_span.end)),
                definition: function.name_span,
            });
        }
        let mut types = Vec::with_capacity(file.types.len());
        for structure in &file.types {
            let mut definitions = vec![structure.span];
            definitions.extend(structure.type_parameters.iter().map(|&(_, span)| span));
            match &structure.shape {
                syntax::TypeShape::Struct(fields) => {
                    definitions.extend(fields.iter().map(|(_, span, _)| *span));
                }
                syntax::TypeShape::Union(variants) => {
                    definitions.extend(variants.iter().map(|variant| variant.span));
                }
            }
            let mut dependencies = Vec::new();
            shape::declared_type_names(structure, |name| dependencies.push(name));
            types.push(TypeInput {
                name: structure.name,
                shape: shape::declared_type(structure),
                definitions,
                dependencies,
            });
        }
        let constant_sources = file
            .constants
            .iter()
            .map(|constant| {
                let span = constant.declaration_span;
                (constant.name, blake3::hash(text(span.start, span.end)))
            })
            .collect();
        Self {
            file,
            diagnostics,
            types,
            functions,
            constant_sources,
        }
    }
}

self_cell! {
    struct ParsedSource {
        owner: Arc<str>,

        #[covariant]
        dependent: Parsed,
    }
}

struct CachedFile {
    base: usize,
    reserved: usize,
    source: ParsedSource,
}

/// A reusable analysis cache for the ordered inputs of one oxel module.
///
/// Parsed files keep their syntax across edits. Function checks are reusable
/// while their source, signatures, type dependencies, and evaluated constants stay unchanged.
#[derive(Default)]
pub struct ProjectAnalyzer {
    kind: AssemblyKind,
    files: Vec<CachedFile>,
    parsed_files: usize,
    bodies: BodyCache,
}

impl ProjectAnalyzer {
    /// Creates an analysis cache for the selected kind of module.
    pub fn new(kind: AssemblyKind) -> Self {
        Self {
            kind,
            ..Self::default()
        }
    }

    /// Returns the number of source files parsed during the last inspection.
    pub fn parsed_files(&self) -> usize {
        self.parsed_files
    }

    /// Returns the number of function bodies checked through the cache in the last inspection.
    pub fn checked_functions(&self) -> usize {
        self.bodies.checked_functions
    }

    /// Returns the number of constant initializers executed through the cache in the last inspection.
    pub fn evaluated_constants(&self) -> usize {
        self.bodies.evaluation.evaluated_constants
    }

    /// Returns a fresh analysis snapshot for each input, reusing unchanged syntax.
    pub fn inspect(
        &mut self,
        sources: &[Arc<str>],
        references: &References,
    ) -> Vec<DocumentAnalysis> {
        self.parsed_files = 0;
        self.bodies.checked_functions = 0;
        self.bodies.evaluation.evaluated_constants = 0;
        self.files.truncate(sources.len());

        let mut base = 0usize;
        for (index, source) in sources.iter().enumerate() {
            let required = source.len() + 1; // Include the EOF span.
            let retained = self.files.get(index);
            let reserved = retained
                .map(|file| file.reserved)
                .filter(|&reserved| reserved >= required)
                .unwrap_or_else(|| {
                    required
                        .max(4096)
                        .checked_next_power_of_two()
                        .unwrap_or(required)
                });
            // On narrow hosts the padded layout may fill before a packed one.
            let Some(next_base) = base.checked_add(reserved) else {
                self.files.clear();
                self.bodies = BodyCache::default();
                self.parsed_files = sources.len();
                let text: Vec<_> = sources.iter().map(Arc::as_ref).collect();
                return inspect_project_with_kind(&text, references, self.kind);
            };
            let reusable = retained.is_some_and(|file| {
                let old = file.source.borrow_owner();
                file.base == base && (Arc::ptr_eq(old, source) || old == source)
            });
            if !reusable {
                let file = CachedFile {
                    base,
                    reserved,
                    source: ParsedSource::new(Arc::clone(source), |text| Parsed::new(text, base)),
                };
                if index == self.files.len() {
                    self.files.push(file);
                } else {
                    self.files[index] = file;
                }
                self.parsed_files += 1;
            }
            // Reserve room for edits so offsets in later files remain reusable.
            base = next_base;
        }

        let map = SourceMap {
            bases: self.files.iter().map(|file| file.base).collect(),
        };
        let parsed = self.files.iter().map(|file| file.source.borrow_dependent());
        let diagnostics: Vec<_> = parsed
            .clone()
            .flat_map(|source| source.diagnostics.iter().cloned())
            .collect();
        if diagnostics.is_empty() {
            let mut environment = blake3::Hasher::new();
            environment.update(&(references.assemblies().len() as u64).to_le_bytes());
            for assembly in references.assemblies() {
                environment.update(assembly.hash.as_bytes());
            }
            let functions = parsed
                .clone()
                .flat_map(|source| source.functions.iter().copied());
            let types = parsed.clone().flat_map(|source| &source.types);
            let constant_sources = parsed
                .clone()
                .flat_map(|source| source.constant_sources.iter().copied());
            self.bodies
                .prepare(environment.finalize(), functions, types, constant_sources);
        }
        let module = syntax::Module::new(parsed.map(|source| &source.file));
        inspect_parsed(
            &map,
            module,
            diagnostics,
            references,
            self.kind,
            Some(&mut self.bodies),
        )
    }
}
