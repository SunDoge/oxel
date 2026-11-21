//! Publishes complete library generations for a long-lived CoreCLR host.

mod watch;

use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::{Command, ExitCode, ExitStatus},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use oxel::{AssemblyKind, ProjectCompiler, References};

use crate::{Package, diagnostics::Format, load_references, manifest::Manifest, sdk};

use watch::SourceWatch;

pub(super) fn run(
    mut command: Command,
    path: &Path,
    locked: bool,
    format: Format,
) -> Result<ExitStatus, ExitCode> {
    let project = Manifest::load(path, format)?;
    if project.kind != AssemblyKind::Library || !project.native_libraries.is_empty() {
        return Err(format.error(
            path,
            "E0012",
            "the reload manifest must declare a managed library; declare native libraries in the host",
        ));
    }
    let references = resolve_references(path, &project, locked, format)?;
    let mut compiler = ProjectCompiler::new(project.kind, &references);
    let mut watch = SourceWatch::new(&project.sources);
    let mut sources: Vec<_> = project
        .sources
        .iter()
        .map(|path| read_source(path))
        .collect();
    let bytes = compile(&project, &mut compiler, &sources, format)?;
    let interrupted = Arc::new(AtomicBool::new(false));
    let signal = Arc::clone(&interrupted);
    ctrlc::set_handler(move || signal.store(true, Ordering::Relaxed)).map_err(|error| {
        format.error(
            path,
            "E0012",
            format!("could not install shutdown handler: {error}"),
        )
    })?;
    let publish_error = |error| {
        format.error(
            path,
            "E0012",
            format!("could not publish reload module: {error}"),
        )
    };
    let directory = tempfile::tempdir().map_err(publish_error)?;
    let mut generation = 0;
    publish(directory.path(), generation, &bytes).map_err(publish_error)?;
    let mut child = command
        .env("OXEL_RELOAD_DIRECTORY", directory.path())
        .spawn()
        .map_err(|error| {
            format.error(path, "E0005", format!("could not launch dotnet: {error}"))
        })?;

    loop {
        if interrupted.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(ExitCode::from(130));
        }
        if let Some(status) = child.try_wait().map_err(|error| {
            format.error(
                path,
                "E0005",
                format!("could not wait for the dev host: {error}"),
            )
        })? {
            return Ok(status);
        }
        let mut changed = false;
        for index in watch.changes() {
            let source = read_source(&project.sources[index]);
            if source != sources[index] {
                sources[index] = source;
                changed = true;
            }
        }
        if changed && let Ok(bytes) = compile(&project, &mut compiler, &sources, format) {
            match publish(directory.path(), generation + 1, &bytes) {
                Ok(()) => {
                    generation += 1;
                    // Keep the previous image available for a reader that just
                    // observed the old generation marker.
                    if generation >= 2 {
                        let _ = fs::remove_file(image(directory.path(), generation - 2));
                    }
                }
                Err(error) => {
                    let _ = publish_error(error);
                }
            }
        }
    }
}

fn read_source(path: &Path) -> Result<Arc<str>, String> {
    fs::read_to_string(path)
        .map(Arc::from)
        .map_err(|error| error.to_string())
}

fn compile(
    project: &Manifest,
    compiler: &mut ProjectCompiler<'_>,
    snapshot: &[Result<Arc<str>, String>],
    format: Format,
) -> Result<Vec<u8>, ExitCode> {
    let sources = project
        .sources
        .iter()
        .zip(snapshot)
        .map(|(path, source)| {
            source.as_ref().cloned().map_err(|error| {
                format.error(path, "E0002", format!("could not read source: {error}"))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    compiler.compile(&sources, &project.name).map_err(|errors| {
        let text: Vec<_> = sources.iter().map(Arc::as_ref).collect();
        format.report(&project.sources, &text, errors)
    })
}

fn resolve_references(
    path: &Path,
    project: &Manifest,
    locked: bool,
    format: Format,
) -> Result<References, ExitCode> {
    let use_framework = project.framework != sdk::Framework::None;
    let resolved = if use_framework && project.nuget.is_empty() {
        sdk::probe_framework(path, &project.name, project.framework).ok()
    } else {
        None
    };
    let resolved = match resolved {
        Some(resolution) => Some(resolution),
        None if use_framework || !project.nuget.is_empty() => {
            let mode = if locked {
                sdk::LockMode::Required
            } else {
                sdk::LockMode::IfPresent
            };
            let sdk = sdk::SdkProject::restore(
                (path, None),
                &project.name,
                &project.nuget,
                false,
                mode,
                project.framework,
                project.kind,
            )
            .map_err(|error| {
                format.error(
                    path,
                    "E0010",
                    format!("SDK reference resolution failed: {error}"),
                )
            })?;
            Some(sdk::Resolution::Project(sdk))
        }
        None => None,
    };
    let package = Package {
        manifest: path,
        name: &project.name,
        kind: project.kind,
        native_libraries: &project.native_libraries,
    };
    load_references(
        &project.references,
        resolved.as_ref(),
        Some(package),
        format,
    )
    .map(|(refs, _)| refs)
}

fn publish(directory: &Path, generation: u64, bytes: &[u8]) -> io::Result<()> {
    let image = image(directory, generation);
    fs::write(&image, bytes)?;
    let result = (|| {
        let mut marker = tempfile::NamedTempFile::new_in(directory)?;
        write!(marker, "{generation}")?;
        marker.persist(directory.join("current"))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(image);
    }
    result
}

fn image(directory: &Path, generation: u64) -> PathBuf {
    directory.join(format!("{generation}.dll"))
}
