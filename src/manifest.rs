//! Loads the CLI's declarative inputs for one oxel project.

use std::{
    collections::{BTreeMap, HashSet},
    fs,
    ops::Range,
    path::{Path, PathBuf},
    process::ExitCode,
};

use oxel::{AssemblyKind, Diagnostic, Span};
use serde::Deserialize;
use toml::Spanned;

use crate::{diagnostics::Format, native_assets::NativeLibrary};

pub(super) const DEFAULT: &str = "oxel.toml";

pub(super) struct Manifest {
    pub name: String,
    pub kind: AssemblyKind,
    pub sources: Vec<PathBuf>,
    pub references: Vec<PathBuf>,
    pub native_libraries: Vec<NativeLibrary>,
    pub nuget: BTreeMap<String, String>,
    pub framework: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    package: Package,
    #[serde(default)]
    nuget: BTreeMap<String, Spanned<String>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Package {
    name: Spanned<String>,
    kind: Option<Spanned<String>>,
    sources: Spanned<Vec<Spanned<PathBuf>>>,
    #[serde(default)]
    references: Vec<Spanned<PathBuf>>,
    #[serde(default)]
    native_libraries: Vec<Spanned<PathBuf>>,
    #[serde(default)]
    framework: bool,
}

impl Manifest {
    pub(super) fn load(path: &Path, format: Format) -> Result<Self, ExitCode> {
        let source = fs::read_to_string(path).map_err(|error| {
            format.error(
                path,
                "E0008",
                format!("could not read project manifest: {error}"),
            )
        })?;
        Self::parse(&source, directory(path)).map_err(|error| {
            format.print(path, &source, &error);
            ExitCode::FAILURE
        })
    }

    fn parse(source: &str, directory: &Path) -> Result<Self, Diagnostic> {
        let Document { package, nuget } =
            toml::from_str(source).map_err(|error: toml::de::Error| {
                invalid(error.span().unwrap_or_default(), error.message())
            })?;
        let kind = match &package.kind {
            None => AssemblyKind::Executable,
            Some(kind) => match kind.get_ref().as_str() {
                "executable" => AssemblyKind::Executable,
                "library" => AssemblyKind::Library,
                _ => {
                    return Err(invalid(
                        kind.span(),
                        "package.kind must be 'executable' or 'library'",
                    ));
                }
            },
        };
        if !kind.valid_name(package.name.get_ref()) {
            let message = match kind {
                AssemblyKind::Executable => "invalid package assembly name",
                AssemblyKind::Library => {
                    "library package name must be a dotted CLR namespace using Oxel identifiers"
                }
            };
            return Err(invalid(package.name.span(), message));
        }
        if package.sources.get_ref().is_empty() {
            return Err(invalid(
                package.sources.span(),
                "package.sources must list at least one file",
            ));
        }
        Ok(Self {
            name: package.name.into_inner(),
            kind,
            sources: resolve(directory, package.sources.into_inner(), "source")?,
            references: resolve(directory, package.references, "reference")?,
            native_libraries: native_libraries(directory, package.native_libraries)?,
            nuget: packages(nuget)?,
            framework: package.framework,
        })
    }
}

fn native_libraries(
    directory: &Path,
    entries: Vec<Spanned<PathBuf>>,
) -> Result<Vec<NativeLibrary>, Diagnostic> {
    let mut names = HashSet::new();
    entries
        .into_iter()
        .map(|entry| {
            let path = entry.get_ref();
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .filter(|name| !name.contains('\0'))
                .ok_or_else(|| invalid(entry.span(), "native library path must name a file"))?;
            if !names.insert(name.to_ascii_lowercase()) {
                return Err(invalid(
                    entry.span(),
                    format!("duplicate native library filename '{name}'"),
                ));
            }
            Ok(NativeLibrary {
                path: directory.join(path),
                name: name.to_owned(),
            })
        })
        .collect()
}

fn packages(
    entries: BTreeMap<String, Spanned<String>>,
) -> Result<BTreeMap<String, String>, Diagnostic> {
    let mut names = HashSet::new();
    entries
        .into_iter()
        .map(|(id, version)| {
            if id.is_empty()
                || !id
                    .bytes()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'.' | b'_' | b'-'))
            {
                return Err(invalid(
                    version.span(),
                    format!("invalid NuGet package ID '{id}'"),
                ));
            }
            if !names.insert(id.to_ascii_lowercase()) {
                return Err(invalid(
                    version.span(),
                    format!("duplicate NuGet package ID '{id}'"),
                ));
            }
            let value = version.get_ref();
            if !value.as_bytes().first().is_some_and(u8::is_ascii_digit)
                || !value
                    .bytes()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'.' | b'+' | b'-'))
            {
                return Err(invalid(
                    version.span(),
                    "NuGet dependencies require an exact package version",
                ));
            }
            Ok((id, version.into_inner()))
        })
        .collect()
}

pub(super) fn directory(path: &Path) -> &Path {
    path.parent().unwrap_or_else(|| Path::new(""))
}

fn resolve(
    directory: &Path,
    entries: Vec<Spanned<PathBuf>>,
    kind: &str,
) -> Result<Vec<PathBuf>, Diagnostic> {
    let mut seen = HashSet::new();
    entries
        .into_iter()
        .map(|entry| {
            if entry.get_ref().as_os_str().is_empty() {
                return Err(invalid(
                    entry.span(),
                    format!("{kind} path must not be empty"),
                ));
            }
            let path = directory.join(entry.get_ref());
            let identity = path.canonicalize().unwrap_or_else(|_| path.clone());
            if !seen.insert(identity) {
                return Err(invalid(
                    entry.span(),
                    format!("duplicate {kind} path '{}'", path.display()),
                ));
            }
            Ok(path)
        })
        .collect()
}

fn invalid(range: Range<usize>, message: impl Into<String>) -> Diagnostic {
    Diagnostic::new(
        "E0008",
        Span {
            start: range.start,
            end: range.end,
        },
        message,
    )
}
