//! Compiler entry points shared by the CLI and future tooling.

mod analysis;
mod cil;
mod clr;
mod hir;
mod project;
mod syntax;

pub use analysis::{
    CallParameters, CallableSignature, DocumentAnalysis, Occurrence, OccurrenceKind,
    ProjectAnalyzer, Symbol, SymbolKind, inspect_document, inspect_project,
    inspect_project_with_kind,
};
pub use clr::{References, valid_assembly_name};
pub use hir::{Type, clr_function_signatures};
pub use project::{
    AssemblyKind, SourceDiagnostic, TestOptions, UnitTest, check_project, check_project_with_kind,
    check_test_project, compile_project, compile_project_with_kind, compile_test_project,
    compile_test_project_filtered, compile_test_project_with_options, discover_test_project,
};
pub use syntax::{CallSite, call_site, declared_function_signature, valid_identifier};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ArgumentMode {
    Value,
    In,
    Ref,
    Out,
}

impl ArgumentMode {
    fn call_prefix(self) -> &'static str {
        match self {
            Self::Value => "",
            Self::In => "&",
            Self::Ref => "&mut ",
            Self::Out => "out ",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

/// One suggested text edit in the same source file as its diagnostic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiagnosticFix {
    pub title: &'static str,
    pub span: Span,
    pub replacement: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiagnosticRelated {
    /// Input index in project diagnostics, or the current source in a single-file diagnostic.
    pub source: Option<usize>,
    pub span: Span,
    pub message: String,
}

impl DiagnosticRelated {
    /// Returns the one-based line and Unicode scalar column in the related source.
    pub fn location(&self, source: &str) -> (usize, usize) {
        source_location(source, self.span.start)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct DiagnosticDetails {
    fix: Option<DiagnosticFix>,
    related: Option<DiagnosticRelated>,
}

impl DiagnosticFix {
    pub(crate) fn insert(title: &'static str, offset: usize, replacement: &'static str) -> Self {
        Self {
            title,
            span: Span {
                start: offset,
                end: offset,
            },
            replacement,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub code: &'static str,
    pub message: String,
    pub span: Span,
    pub expected: Option<Type>,
    pub actual: Option<Type>,
    type_names: Option<std::sync::Arc<(String, String)>>,
    // Keep optional details off the recursive parser's error frames.
    details: Option<Box<DiagnosticDetails>>,
}

impl Diagnostic {
    pub fn new(code: &'static str, span: Span, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            span,
            expected: None,
            actual: None,
            type_names: None,
            details: None,
        }
    }

    pub(crate) fn with_fix(mut self, fix: DiagnosticFix) -> Self {
        self.details.get_or_insert_with(Default::default).fix = Some(fix);
        self
    }

    /// A suggested source edit, when the compiler can identify one.
    pub fn fix(&self) -> Option<&DiagnosticFix> {
        self.details.as_ref()?.fix.as_ref()
    }

    pub(crate) fn with_related(mut self, span: Span, message: impl Into<String>) -> Self {
        self.details.get_or_insert_with(Default::default).related = Some(DiagnosticRelated {
            source: None,
            span,
            message: message.into(),
        });
        self
    }

    /// A declaration or other source range that explains this diagnostic.
    pub fn related(&self) -> Option<&DiagnosticRelated> {
        self.details.as_ref()?.related.as_ref()
    }

    pub(crate) fn related_mut(&mut self) -> Option<&mut DiagnosticRelated> {
        self.details.as_mut()?.related.as_mut()
    }

    pub(crate) fn map_spans(&mut self, map: impl Fn(Span) -> Span) {
        self.span = map(self.span);
        if let Some(fix) = self
            .details
            .as_mut()
            .and_then(|details| details.fix.as_mut())
        {
            fix.span = map(fix.span);
        }
    }

    /// The expected and actual type names for a reported type mismatch.
    pub fn type_names(&self) -> Option<(&str, &str)> {
        self.type_names
            .as_deref()
            .map(|(expected, actual)| (expected.as_str(), actual.as_str()))
    }

    /// Returns the one-based line and Unicode scalar column.
    pub fn location(&self, source: &str) -> (usize, usize) {
        source_location(source, self.span.start)
    }
}

fn source_location(source: &str, offset: usize) -> (usize, usize) {
    let mut end = offset.min(source.len());
    while !source.is_char_boundary(end) {
        end -= 1;
    }
    let prefix = &source[..end];
    let line = prefix.bytes().filter(|&byte| byte == b'\n').count() + 1;
    let column = prefix.rsplit('\n').next().unwrap_or("").chars().count() + 1;
    (line, column)
}

pub fn check(source: &str) -> Result<(), Vec<Diagnostic>> {
    check_with_references(source, &References::default())
}

pub fn check_with_references(source: &str, references: &References) -> Result<(), Vec<Diagnostic>> {
    let file = syntax::parse(source)?;
    hir::check(
        syntax::Module::new([&file]),
        references,
        Some(AssemblyKind::Executable),
    )
}

pub fn compile(source: &str, assembly_name: &str) -> Result<Vec<u8>, Vec<Diagnostic>> {
    compile_with_references(source, assembly_name, &References::default())
}

pub fn compile_with_references(
    source: &str,
    assembly_name: &str,
    references: &References,
) -> Result<Vec<u8>, Vec<Diagnostic>> {
    let file = syntax::parse(source)?;
    let program = hir::lower(
        syntax::Module::new([&file]),
        references,
        AssemblyKind::Executable,
        None,
    )?;
    cil::emit(&program, &[source], assembly_name, references, None).map_err(|error| vec![error])
}
