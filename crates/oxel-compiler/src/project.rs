//! A compilation unit with independently parsed files and a shared symbol table.

use crate::{Diagnostic, References, cil, hir, syntax};

/// Selects the managed entry point and namespace of a compiled module.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AssemblyKind {
    #[default]
    Executable,
    Library,
}

impl AssemblyKind {
    /// Library names also serve as CLR namespaces and must be dotted Oxel identifiers.
    pub fn valid_name(self, name: &str) -> bool {
        crate::valid_assembly_name(name)
            && (self == Self::Executable || name.split('.').all(crate::valid_identifier))
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct SourceDiagnostic {
    /// Index in the original source slice. A project with no inputs has no file.
    pub source: Option<usize>,
    /// The byte span is relative to the indicated input.
    pub diagnostic: Diagnostic,
}

/// Type-checks all sources as one module with a single executable entry point.
pub fn check_project(
    sources: &[&str],
    references: &References,
) -> Result<(), Vec<SourceDiagnostic>> {
    check_project_with_kind(sources, references, AssemblyKind::Executable)
}

/// Type-checks one module as either an executable or a managed library.
pub fn check_project_with_kind(
    sources: &[&str],
    references: &References,
    kind: AssemblyKind,
) -> Result<(), Vec<SourceDiagnostic>> {
    Project::new(sources).analyze(|module| hir::check(module, references, kind))
}

/// Compiles an ordered set of files as one managed executable.
pub fn compile_project(
    sources: &[&str],
    assembly_name: &str,
    references: &References,
) -> Result<Vec<u8>, Vec<SourceDiagnostic>> {
    compile_project_with_kind(sources, assembly_name, references, AssemblyKind::Executable)
}

/// Emits an ordered set of source files as one managed assembly.
pub fn compile_project_with_kind(
    sources: &[&str],
    assembly_name: &str,
    references: &References,
    kind: AssemblyKind,
) -> Result<Vec<u8>, Vec<SourceDiagnostic>> {
    compile(sources, assembly_name, references, kind, false)
}

/// Compiles source and annotated tests with a synthetic entry point.
pub fn compile_test_project(
    sources: &[&str],
    assembly_name: &str,
    references: &References,
    kind: AssemblyKind,
) -> Result<Vec<u8>, Vec<SourceDiagnostic>> {
    compile(sources, assembly_name, references, kind, true)
}

fn compile(
    sources: &[&str],
    assembly_name: &str,
    references: &References,
    kind: AssemblyKind,
    tests: bool,
) -> Result<Vec<u8>, Vec<SourceDiagnostic>> {
    let project = Project::new(sources);
    let program = project.analyze(|module| hir::lower(module, references, kind, tests))?;
    cil::emit(&program, sources, assembly_name, references)
        .map_err(|error| vec![project.map.locate(error)])
}

pub(crate) struct Project<'src> {
    sources: &'src [&'src str],
    pub map: SourceMap,
}

impl<'src> Project<'src> {
    pub fn new(sources: &'src [&'src str]) -> Self {
        let mut cursor = 0;
        let bases = sources
            .iter()
            .map(|source| {
                let base = cursor;
                // Keep the EOF position of this file distinct from the start of
                // the next one, including when either file is empty.
                cursor += source.len() + 1;
                base
            })
            .collect();
        Self {
            sources,
            map: SourceMap { bases },
        }
    }

    pub fn parse(&self) -> (Vec<syntax::File<'src>>, Vec<Diagnostic>) {
        let mut files = Vec::with_capacity(self.sources.len());
        let mut errors = Vec::new();
        for (&source, &base) in self.sources.iter().zip(&self.map.bases) {
            let (file, diagnostics) = syntax::parse_at(source, base);
            errors.extend(diagnostics);
            files.push(file);
        }
        (files, errors)
    }

    fn analyze<T>(
        &self,
        check: impl FnOnce(syntax::Module<'_, 'src>) -> Result<T, Vec<Diagnostic>>,
    ) -> Result<T, Vec<SourceDiagnostic>> {
        let (files, errors) = self.parse();
        let result = if errors.is_empty() {
            check(syntax::Module::new(&files))
        } else {
            Err(errors)
        };
        result.map_err(|mut errors| {
            errors.sort_by_key(|error| error.span.start);
            errors
                .into_iter()
                .map(|error| self.map.locate(error))
                .collect()
        })
    }
}

pub(crate) struct SourceMap {
    pub bases: Vec<usize>,
}

impl SourceMap {
    pub fn local_span(&self, span: crate::Span) -> Option<(usize, crate::Span)> {
        let source = self
            .bases
            .partition_point(|&base| base <= span.start)
            .checked_sub(1)?;
        let base = self.bases[source];
        Some((
            source,
            crate::Span {
                start: span.start - base,
                end: span.end - base,
            },
        ))
    }

    pub fn locate(&self, mut diagnostic: Diagnostic) -> SourceDiagnostic {
        if let Some(related) = diagnostic.related_mut()
            && let Some((source, span)) = self.local_span(related.span)
        {
            related.source = Some(source);
            related.span = span;
        }
        let source = self.local_span(diagnostic.span).map(|(source, _)| {
            let base = self.bases[source];
            diagnostic.map_spans(|span| crate::Span {
                start: span.start - base,
                end: span.end - base,
            });
            source
        });
        SourceDiagnostic { source, diagnostic }
    }
}
