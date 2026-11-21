//! Publishes complete library generations for a long-lived CoreCLR host.

use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::{Command, ExitCode, ExitStatus},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use oxel::{AssemblyKind, References};

use crate::{Package, diagnostics::Format, load_references, manifest::Manifest, sdk};

const POLL_INTERVAL: Duration = Duration::from_millis(100);

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
    let mut sources = snapshot(&project.sources);
    let bytes = compile(&project, &references, &sources, format)?;
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

    let mut pending = None;
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
        thread::sleep(POLL_INTERVAL);
        let next = snapshot(&project.sources);
        if next == sources {
            pending = None;
        } else if pending.as_ref() == Some(&next) {
            sources = next;
            pending = None;
            if let Ok(bytes) = compile(&project, &references, &sources, format) {
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
        } else {
            pending = Some(next);
        }
    }
}

fn snapshot(paths: &[PathBuf]) -> Vec<Result<String, String>> {
    paths
        .iter()
        .map(|path| fs::read_to_string(path).map_err(|error| error.to_string()))
        .collect()
}

fn compile(
    project: &Manifest,
    references: &References,
    snapshot: &[Result<String, String>],
    format: Format,
) -> Result<Vec<u8>, ExitCode> {
    let sources = project
        .sources
        .iter()
        .zip(snapshot)
        .map(|(path, source)| {
            source.as_deref().map_err(|error| {
                format.error(path, "E0002", format!("could not read source: {error}"))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    oxel::compile_project_with_kind(&sources, &project.name, references, project.kind)
        .map_err(|errors| format.report(&project.sources, &sources, errors))
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
                path,
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
