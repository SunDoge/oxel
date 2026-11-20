//! A compilation unit with independently parsed files and a shared symbol table.

use crate::{Diagnostic, References, Span, cil, hir, syntax};

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

/// Selects the tests to run after checking the entire module.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TestOptions<'a> {
    /// Case-sensitive substring of a test name. The empty string matches all tests.
    pub filter: &'a str,
    /// Run matching tests marked with `#[ignore]` as well.
    pub include_ignored: bool,
}

/// A declared unit test in a checked project.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnitTest<'src> {
    pub name: &'src str,
    pub ignored: bool,
    /// Index in the original source slice.
    pub source: usize,
    /// Byte span of the name within that source.
    pub span: Span,
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
    Project::new(sources).analyze(|module| hir::check(module, references, Some(kind)))
}

/// Checks the tests and their helpers as one module without an application entry point.
pub fn check_test_project(
    sources: &[&str],
    references: &References,
) -> Result<(), Vec<SourceDiagnostic>> {
    Project::new(sources).analyze(|module| hir::check(module, references, None))
}

/// Checks the entire test module and lists matching tests in source order.
pub fn discover_test_project<'src>(
    sources: &[&'src str],
    references: &References,
    filter: &str,
) -> Result<Vec<UnitTest<'src>>, Vec<SourceDiagnostic>> {
    let project = Project::new(sources);
    project.analyze(|module| {
        let tests = module
            .functions
            .iter()
            .filter_map(|function| {
                let attribute = function.matching_test(filter)?;
                let (source, span) = project
                    .map
                    .local_span(function.name_span)
                    .expect("test source");
                Some(UnitTest {
                    name: function.name,
                    ignored: attribute == syntax::TestAttribute::Ignored,
                    source,
                    span,
                })
            })
            .collect();
        hir::check(module, references, None)?;
        Ok(tests)
    })
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
    compile(sources, assembly_name, references, kind, None)
}

/// Compiles source and annotated tests with a synthetic entry point.
pub fn compile_test_project(
    sources: &[&str],
    assembly_name: &str,
    references: &References,
    kind: AssemblyKind,
) -> Result<Vec<u8>, Vec<SourceDiagnostic>> {
    compile_test_project_filtered(sources, assembly_name, references, kind, "")
}

/// Checks the whole test module and runs tests whose names contain `filter`.
pub fn compile_test_project_filtered(
    sources: &[&str],
    assembly_name: &str,
    references: &References,
    kind: AssemblyKind,
    filter: &str,
) -> Result<Vec<u8>, Vec<SourceDiagnostic>> {
    compile_test_project_with_options(
        sources,
        assembly_name,
        references,
        kind,
        TestOptions {
            filter,
            ..TestOptions::default()
        },
    )
}

/// Compiles the test module with a synthetic entry point and the given selection.
pub fn compile_test_project_with_options(
    sources: &[&str],
    assembly_name: &str,
    references: &References,
    kind: AssemblyKind,
    options: TestOptions<'_>,
) -> Result<Vec<u8>, Vec<SourceDiagnostic>> {
    compile(sources, assembly_name, references, kind, Some(options))
}

fn compile(
    sources: &[&str],
    assembly_name: &str,
    references: &References,
    kind: AssemblyKind,
    tests: Option<TestOptions<'_>>,
) -> Result<Vec<u8>, Vec<SourceDiagnostic>> {
    let project = Project::new(sources);
    let program = project.analyze(|module| hir::lower(module, references, kind, tests))?;
    cil::emit(&program, sources, assembly_name, references, tests)
        .map_err(|error| vec![project.map.locate(error)])
}

pub(crate) struct Project<'sources, 'src> {
    sources: &'sources [&'src str],
    pub map: SourceMap,
}

impl<'sources, 'src> Project<'sources, 'src> {
    pub fn new(sources: &'sources [&'src str]) -> Self {
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
