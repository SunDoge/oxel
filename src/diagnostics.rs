//! Formats the compiler diagnostic stream independently of program output.

use std::{
    path::{Path, PathBuf},
    process::ExitCode,
};

use oxel::{Diagnostic, SourceDiagnostic, Span};
use serde_json::json;

#[derive(Clone, Copy, Default, usage_rs::ValueEnum)]
pub(super) enum Format {
    #[default]
    Text,
    Json,
}

impl Format {
    pub fn print(self, path: &Path, source: &str, diagnostic: &Diagnostic) {
        self.print_with_related(path, source, diagnostic, (path, source));
    }

    fn print_with_related(
        self,
        path: &Path,
        source: &str,
        diagnostic: &Diagnostic,
        (related_path, related_source): (&Path, &str),
    ) {
        let (line, column) = diagnostic.location(source);
        match self {
            Self::Text => {
                eprintln!(
                    "{}:{line}:{column}: error[{}]: {}",
                    path.display(),
                    diagnostic.code,
                    diagnostic.message
                );
                if let Some(related) = diagnostic.related() {
                    let (line, column) = related.location(related_source);
                    eprintln!(
                        "{}:{line}:{column}: note: {}",
                        related_path.display(),
                        related.message
                    );
                }
            }
            Self::Json => {
                let mut value = json!({
                    "code": diagnostic.code,
                    "severity": "error",
                    "message": diagnostic.message,
                    "file": path.to_string_lossy(),
                    "range": { "start": diagnostic.span.start, "end": diagnostic.span.end },
                    "line": line,
                    "column": column,
                });
                if let Some((expected, actual)) = diagnostic.type_names() {
                    value["expected"] = json!(expected);
                    value["actual"] = json!(actual);
                }
                if let Some(fix) = diagnostic.fix() {
                    value["fix"] = json!({
                        "title": fix.title,
                        "range": { "start": fix.span.start, "end": fix.span.end },
                        "replacement": fix.replacement,
                    });
                }
                if let Some(related) = diagnostic.related() {
                    let (line, column) = related.location(related_source);
                    value["related"] = json!({
                        "file": related_path.to_string_lossy(),
                        "range": { "start": related.span.start, "end": related.span.end },
                        "line": line,
                        "column": column,
                        "message": related.message,
                    });
                }
                eprintln!("{value}");
            }
        }
    }

    pub fn error(self, path: &Path, code: &'static str, message: impl Into<String>) -> ExitCode {
        self.print(path, "", &Diagnostic::new(code, Span::default(), message));
        ExitCode::FAILURE
    }

    pub fn report(
        self,
        paths: &[PathBuf],
        sources: &[&str],
        errors: Vec<SourceDiagnostic>,
    ) -> ExitCode {
        for error in errors {
            if let Some(index) = error.source {
                let related = error
                    .diagnostic
                    .related()
                    .and_then(|item| item.source)
                    .unwrap_or(index);
                self.print_with_related(
                    &paths[index],
                    sources[index],
                    &error.diagnostic,
                    (&paths[related], sources[related]),
                );
            } else {
                self.print(Path::new("<project>"), "", &error.diagnostic);
            }
        }
        ExitCode::FAILURE
    }
}
