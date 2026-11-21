mod aot;
mod arguments;
mod bindgen;
mod diagnostics;
mod lsp;
mod manifest;
mod native_assets;
mod sdk;

use std::{
    env,
    ffi::OsString,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::{Command, ExitCode},
};

use arguments::{Action, Options, usage_error};
use diagnostics::Format;
use native_assets::{NativeLibrary, copy_libraries};
use oxel::{AssemblyKind, References, TestOptions};

const RUNTIME_CONFIG: &str = r#"{
  "runtimeOptions": {
    "tfm": "net10.0",
    "rollForward": "Major",
    "framework": { "name": "Microsoft.NETCore.App", "version": "10.0.0" }
  }
}
"#;

#[derive(Default)]
struct Run {
    arguments: Vec<OsString>,
    status: Option<i32>,
}

fn main() -> ExitCode {
    let mut run = Run::default();
    let exit = cli(&mut run);
    // CLI-owned temporary directories are dropped before exiting with the child status.
    if let Some(status) = run.status {
        std::process::exit(status);
    }
    exit
}

fn cli(run: &mut Run) -> ExitCode {
    let Options {
        action,
        format,
        framework,
        locked,
        mut manifest_path,
        native,
        mut sources,
        mut references,
        arguments,
    } = match arguments::parse() {
        Ok(options) => options,
        Err(exit) => return exit,
    };
    run.arguments = arguments;
    if let Action::Bindgen(inputs) = action {
        return bindgen::run(inputs, format);
    }
    let lsp = matches!(action, Action::Lsp { .. });
    let restore_only = matches!(action, Action::Restore);
    if manifest_path.is_some() && !sources.is_empty() {
        return usage_error(format, "choose a manifest or explicit source files");
    }
    if manifest_path.is_none() && !lsp && sources.is_empty() {
        let default = Path::new(manifest::DEFAULT);
        match default.try_exists() {
            Ok(true) => manifest_path = Some(default.into()),
            Ok(false) => {
                return usage_error(format, "supply source files or an oxel.toml manifest");
            }
            Err(error) => {
                return format.error(
                    default,
                    "E0008",
                    format!("could not locate project manifest: {error}"),
                );
            }
        }
    }
    let mut framework = if framework {
        sdk::Framework::Core
    } else {
        sdk::Framework::None
    };
    let mut nuget = Default::default();
    let package = if let Some(path) = &manifest_path {
        let manifest = match manifest::Manifest::load(path, format) {
            Ok(manifest) => manifest,
            Err(exit) => return exit,
        };
        if manifest.framework != sdk::Framework::None {
            framework = manifest.framework;
        }
        nuget = manifest.nuget;
        sources = manifest.sources;
        if matches!(
            action,
            Action::Test { .. } | Action::Check { tests: true } | Action::Lsp { tests: true }
        ) {
            sources.extend(manifest.test_sources);
        }
        references.splice(..0, manifest.references);
        Some((
            path.as_path(),
            manifest.name,
            manifest.kind,
            manifest.native_libraries,
        ))
    } else {
        None
    };
    let package = package
        .as_ref()
        .map(|(path, name, kind, libraries)| Package {
            manifest: path,
            name,
            kind: *kind,
            native_libraries: libraries,
        });
    let kind = package.map_or(AssemblyKind::Executable, |package| package.kind);
    if kind == AssemblyKind::Library && (native || matches!(action, Action::Run)) {
        return usage_error(
            format,
            "managed libraries can be built with oxel compile and require an executable host",
        );
    }
    let has_framework = framework != sdk::Framework::None;
    let restored = if has_framework || !nuget.is_empty() {
        let lock_mode = if locked {
            sdk::LockMode::Required
        } else if restore_only {
            sdk::LockMode::Refresh
        } else {
            sdk::LockMode::IfPresent
        };
        let (path, name) = if let Some(package) = package {
            (package.manifest, package.name)
        } else if let Some(source) = sources.first() {
            let Some(name) = source.file_stem().and_then(|stem| stem.to_str()) else {
                return format.error(source, "E0003", "source filename must have a UTF-8 stem");
            };
            if !oxel::valid_assembly_name(name) {
                return format.error(source, "E0003", "invalid assembly name");
            }
            (Path::new(manifest::DEFAULT), name)
        } else {
            (Path::new(manifest::DEFAULT), "oxel_editor")
        };
        let resolution = if has_framework && nuget.is_empty() {
            match action {
                Action::Check { .. } | Action::Lsp { .. } | Action::Test { list: true, .. } => {
                    sdk::probe_framework(path, name, framework).ok()
                }
                Action::Compile | Action::Run | Action::Test { list: false, .. }
                    if references.is_empty() =>
                {
                    sdk::installed_framework(path, framework)
                }
                _ => None,
            }
        } else {
            None
        };
        let runtime_kind = if matches!(action, Action::Test { .. }) {
            AssemblyKind::Executable
        } else {
            kind
        };
        let resolution = resolution.map_or_else(
            || {
                sdk::SdkProject::restore(
                    path,
                    name,
                    &nuget,
                    native,
                    lock_mode,
                    framework,
                    runtime_kind,
                )
                .map(sdk::Resolution::Project)
            },
            Ok,
        );
        match resolution {
            Ok(project) => Some(project),
            Err(error) => {
                let failure = if has_framework {
                    "SDK framework/reference resolution failed"
                } else {
                    "NuGet restore failed"
                };
                return format.error(
                    manifest_path
                        .as_deref()
                        .or_else(|| sources.first().map(PathBuf::as_path))
                        .unwrap_or(path),
                    "E0010",
                    format!("{failure}: {error}"),
                );
            }
        }
    } else {
        None
    };

    if restore_only {
        if let Some(path) = restored
            .as_ref()
            .and_then(sdk::Resolution::project)
            .and_then(|project| project.lock.as_ref())
        {
            println!("{}", path.display());
        }
        return ExitCode::SUCCESS;
    }

    if lsp {
        return match load_references(&references, restored.as_ref(), package, format) {
            Ok((references, _)) => match lsp::run(&references, &sources, kind) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => format.error(
                    manifest_path
                        .as_deref()
                        .unwrap_or_else(|| Path::new("<language server>")),
                    "E0009",
                    error.to_string(),
                ),
            },
            Err(error) => error,
        };
    }
    execute(
        action,
        &sources,
        &references,
        package,
        restored.as_ref(),
        format,
        run,
    )
}

#[derive(Clone, Copy)]
struct Package<'a> {
    manifest: &'a Path,
    name: &'a str,
    kind: AssemblyKind,
    native_libraries: &'a [NativeLibrary],
}

struct Dependency {
    name: String,
    bytes: Vec<u8>,
}

fn execute(
    action: Action,
    paths: &[PathBuf],
    reference_paths: &[PathBuf],
    package: Option<Package<'_>>,
    restored: Option<&sdk::Resolution>,
    format: Format,
    application: &mut Run,
) -> ExitCode {
    let path = &paths[0];
    let mut sources = Vec::with_capacity(paths.len());
    for input in paths {
        match fs::read_to_string(input) {
            Ok(source) => sources.push(source),
            Err(err) => {
                return format.error(input, "E0002", format!("could not read source: {err}"));
            }
        }
    }
    let sources: Vec<_> = sources.iter().map(String::as_str).collect();
    let (references, dependencies) =
        match load_references(reference_paths, restored, package, format) {
            Ok(loaded) => loaded,
            Err(error) => return error,
        };
    let kind = package.map_or(AssemblyKind::Executable, |package| package.kind);
    if let Action::Check { tests } = action {
        let checked = if tests {
            oxel::check_test_project(&sources, &references)
        } else {
            oxel::check_project_with_kind(&sources, &references, kind)
        };
        return match checked {
            Ok(()) => ExitCode::SUCCESS,
            Err(errors) => format.report(paths, &sources, errors),
        };
    }
    if let Action::Test {
        list: true, filter, ..
    } = &action
    {
        let tests = match oxel::discover_test_project(
            &sources,
            &references,
            filter.as_deref().unwrap_or_default(),
        ) {
            Ok(tests) => tests,
            Err(errors) => return format.report(paths, &sources, errors),
        };
        for test in tests {
            let suffix = if test.ignored { " (ignored)" } else { "" };
            println!("{}: test{suffix}", test.name);
        }
        return ExitCode::SUCCESS;
    }
    let (origin, directory, name) = if let Some(Package { manifest, name, .. }) = package {
        (manifest, manifest::directory(manifest), name)
    } else {
        let Some(name) = path.file_stem().and_then(|stem| stem.to_str()) else {
            return format.error(path, "E0003", "source filename must have a UTF-8 stem");
        };
        (path.as_path(), Path::new(""), name)
    };
    let compiled = match &action {
        Action::Test {
            filter,
            include_ignored,
            ..
        } => oxel::compile_test_project_with_options(
            &sources,
            name,
            &references,
            kind,
            TestOptions {
                filter: filter.as_deref().unwrap_or_default(),
                include_ignored: *include_ignored,
            },
        ),
        _ => oxel::compile_project_with_kind(&sources, name, &references, kind),
    };
    let bytes = match compiled {
        Ok(bytes) => bytes,
        Err(errors) => return format.report(paths, &sources, errors),
    };

    let libraries = package.map_or(&[][..], |package| package.native_libraries);
    if let Some(restored) = restored.and_then(sdk::Resolution::project) {
        let output = match restored.build(name, &bytes, &dependencies) {
            Ok(output) => output,
            Err(error) => {
                return format.error(origin, "E0010", format!("SDK packaging failed: {error}"));
            }
        };
        if !matches!(action, Action::Publish)
            && let Err(error) = copy_libraries(&output, libraries)
        {
            return format.error(origin, "E0004", error.to_string());
        }
        match action {
            Action::Run | Action::Test { .. } => {
                return run_dll(
                    origin,
                    &output.join(format!("{name}.dll")),
                    format,
                    application,
                );
            }
            Action::Publish => {
                let executable = output.join(format!("{name}{}", env::consts::EXE_SUFFIX));
                return match aot::install(directory, name, &executable, libraries) {
                    Ok(artifact) => {
                        println!("{}", artifact.display());
                        ExitCode::SUCCESS
                    }
                    Err(error) => format.error(
                        origin,
                        "E0007",
                        format!("could not install Native AOT executable: {error}"),
                    ),
                };
            }
            Action::Compile => {
                return match sdk::store(directory, &output) {
                    Ok(folder) => {
                        println!("{}", folder.join(format!("{name}.dll")).display());
                        ExitCode::SUCCESS
                    }
                    Err(error) => format.error(
                        origin,
                        "E0004",
                        format!("could not write assembly: {error}"),
                    ),
                };
            }
            _ => unreachable!("selected SDK artifact action"),
        }
    }

    if matches!(action, Action::Publish) {
        return match aot::publish(directory, name, &bytes, &dependencies, libraries) {
            Ok(executable) => {
                println!("{}", executable.display());
                ExitCode::SUCCESS
            }
            Err(err) => format.error(
                origin,
                "E0007",
                format!("could not publish Native AOT executable: {err}"),
            ),
        };
    }
    let framework = restored.and_then(sdk::Resolution::installed);
    let output_kind = if matches!(action, Action::Test { .. }) {
        AssemblyKind::Executable
    } else {
        kind
    };
    let output = Artifacts {
        name,
        bytes: &bytes,
        dependencies: &dependencies,
        libraries,
        kind: output_kind,
        framework,
    };
    if matches!(action, Action::Run | Action::Test { .. }) {
        return run(origin, directory, &output, format, application);
    }
    let mut artifacts = artifact_directory(directory);
    if kind == AssemblyKind::Library && framework.is_none() {
        artifacts = artifacts.join("lib");
    }
    match output.store(&artifacts) {
        Ok(dll) => {
            println!("{}", dll.display());
            ExitCode::SUCCESS
        }
        Err(err) => format.error(origin, "E0004", format!("could not write assembly: {err}")),
    }
}

fn load_references(
    reference_paths: &[PathBuf],
    sdk: Option<&sdk::Resolution>,
    package: Option<Package<'_>>,
    format: Format,
) -> Result<(References, Vec<Dependency>), ExitCode> {
    let mut references = References::default();
    let mut dependencies = Vec::new();
    let framework = sdk
        .into_iter()
        .flat_map(sdk::Resolution::framework_references);
    let packages = sdk
        .and_then(sdk::Resolution::project)
        .into_iter()
        .flat_map(|project| &project.references);
    let paths = framework
        .map(|path| (path, false, true))
        .chain(reference_paths.iter().map(|path| (path, true, false)))
        .chain(packages.map(|path| (path, false, false)));
    for (reference, copy, framework) in paths {
        let bytes = match fs::read(reference) {
            Ok(bytes) => bytes,
            Err(err) => {
                return Err(format.error(
                    reference,
                    "E0006",
                    format!("could not read CLR reference: {err}"),
                ));
            }
        };
        let added = if framework {
            references.add_framework(&bytes)
        } else {
            references.add(&bytes)
        };
        let name = match added {
            Ok(name) => name.to_owned(),
            Err(err) => {
                return Err(format.error(
                    reference,
                    "E0006",
                    format!("could not load CLR reference: {err}"),
                ));
            }
        };
        if let Some(package) = package
            && name.eq_ignore_ascii_case(package.name)
        {
            return Err(format.error(
                package.manifest,
                "E0008",
                format!(
                    "package name conflicts with CLR reference '{}' ({})",
                    name,
                    reference.display()
                ),
            ));
        }
        if copy {
            dependencies.push(Dependency { name, bytes });
        }
    }
    Ok((references, dependencies))
}

fn run(
    source: &Path,
    directory: &Path,
    output: &Artifacts<'_>,
    format: Format,
    application: &mut Run,
) -> ExitCode {
    let cache = (output.framework.is_some() && !output.libraries.is_empty())
        .then(|| artifact_directory(directory).join("native-cache"));
    let folder = match &cache {
        Some(cache) => fs::create_dir_all(cache)
            .and_then(|()| tempfile::TempDir::with_prefix_in(".run-", cache)),
        None => tempfile::tempdir(),
    };
    let folder = match folder {
        Ok(folder) => folder,
        Err(err) => {
            return format.error(
                source,
                "E0004",
                format!("could not create run directory: {err}"),
            );
        }
    };
    let dll = match output.write(folder.path(), cache.as_deref()) {
        Ok(dll) => dll,
        Err(err) => {
            return format.error(source, "E0004", format!("could not write assembly: {err}"));
        }
    };
    run_dll(source, &dll, format, application)
}

fn run_dll(source: &Path, dll: &Path, format: Format, application: &mut Run) -> ExitCode {
    match Command::new("dotnet")
        .arg("exec")
        .arg(dll)
        .args(&application.arguments)
        .status()
    {
        Ok(status) => {
            application.status = status.code();
            if status.success() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(err) => format.error(source, "E0005", format!("could not launch dotnet: {err}")),
    }
}

fn artifact_directory(project: &Path) -> PathBuf {
    project.join("target").join("oxel")
}

struct Artifacts<'a> {
    name: &'a str,
    bytes: &'a [u8],
    dependencies: &'a [Dependency],
    libraries: &'a [NativeLibrary],
    kind: AssemblyKind,
    framework: Option<&'a sdk::InstalledFramework>,
}

impl Artifacts<'_> {
    fn store(&self, folder: &Path) -> io::Result<PathBuf> {
        if self.framework.is_none() && self.libraries.is_empty() {
            if self.dependencies.is_empty() {
                return self.write(folder, None);
            }
            let linked = folder
                .join("linked")
                .join(blake3::hash(self.bytes).to_hex().as_str());
            return self.write(&linked, None);
        }
        let linked = folder.join("linked");
        fs::create_dir_all(&linked)?;
        let stage = tempfile::TempDir::with_prefix_in(".compile-", &linked)?;
        let dll = self.write(stage.path(), None)?;
        let name = dll.file_name().expect("assembly filename");
        Ok(sdk::store_directory(&linked, stage.path())?.join(name))
    }

    fn write(&self, folder: &Path, cache: Option<&Path>) -> io::Result<PathBuf> {
        fs::create_dir_all(folder)?;
        let name = self.name;
        let dll = folder.join(format!("{name}.dll"));
        write_dependencies(folder, self.dependencies)?;
        if let Some(framework) = self.framework {
            let deps = folder.join(format!("{name}.deps.json"));
            write_file(&deps, framework.deps(name).as_bytes())?;
        }
        if self.kind == AssemblyKind::Executable {
            let config = folder.join(format!("{name}.runtimeconfig.json"));
            if let Some(framework) = self.framework {
                write_file(&config, framework.runtime_config().as_bytes())?;
            } else {
                write_file(&config, RUNTIME_CONFIG.as_bytes())?;
            }
        }
        write_file(&dll, self.bytes)?;
        if let Some(cache) = cache {
            native_assets::stage_cached_libraries(folder, cache, self.libraries)?;
        } else {
            copy_libraries(folder, self.libraries)?;
        }
        Ok(dll)
    }
}

fn write_dependencies(folder: &Path, dependencies: &[Dependency]) -> io::Result<()> {
    if !dependencies.is_empty() {
        fs::create_dir_all(folder)?;
    }
    for dependency in dependencies {
        let path = folder.join(format!("{}.dll", dependency.name));
        write_file(&path, &dependency.bytes)?;
    }
    Ok(())
}

fn write_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path.parent().expect("artifact has a directory");
    let directory = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };
    let mut temp = tempfile::NamedTempFile::new_in(directory)?;
    temp.write_all(bytes)?;
    temp.persist(path)?;
    Ok(())
}
