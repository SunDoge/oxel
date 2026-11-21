//! Publishes oxel's managed executable through the installed .NET SDK.

use std::{
    env, fs, io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use crate::{
    Dependency, NativeLibrary, artifact_directory, copy_libraries,
    run_cache::{config_files, field, file_digest, hash_config, hash_environment, hash_file},
    sdk, write_dependencies, write_file,
};

type PublishResult<T> = Result<T, Box<dyn std::error::Error>>;

pub(super) struct Cache {
    directory: PathBuf,
    _lock: fs::File,
}

impl Cache {
    pub fn open(project: &Path, sdk_path: Option<&Path>, settings: &[&[u8]]) -> Option<Self> {
        let project = fs::canonicalize(if project.as_os_str().is_empty() {
            Path::new(".")
        } else {
            project
        })
        .ok()?;
        let mut hash = blake3::Hasher::new_derive_key("oxel native publish stage v1");
        field(&mut hash, env::consts::OS.as_bytes());
        field(&mut hash, env::consts::ARCH.as_bytes());
        for setting in settings {
            field(&mut hash, setting);
        }
        if let Some(sdk) = sdk_path {
            field(&mut hash, sdk.as_os_str().as_encoded_bytes());
            hash_file(
                &mut hash,
                &sdk.join("Microsoft.NETCoreSdk.BundledVersions.props"),
            )
            .ok()?;
        }
        hash_environment(&mut hash).ok()?;
        hash_config(&mut hash, &project).ok()?;
        hash_file(&mut hash, &env::current_exe().ok()?).ok()?;
        let directory = artifact_directory(&project)
            .join("aot-cache")
            .join(hash.finalize().to_hex().as_str());
        fs::create_dir_all(&directory).ok()?;
        let lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(directory.join("stage.lock"))
            .ok()?;
        lock.lock().ok()?;
        Some(Self {
            directory,
            _lock: lock,
        })
    }

    pub fn path(&self) -> &Path {
        &self.directory
    }

    pub fn build<E: From<io::Error>>(
        &self,
        output: &Path,
        bytes: &[u8],
        dependencies: &[Dependency],
        reuse: bool,
        build: impl FnOnce() -> Result<(), E>,
    ) -> Result<(), E> {
        let record = self.path().join("published");
        if reuse
            && let Ok(cache) = fs::read_to_string(&record)
            && let Some((input, output_hash)) = cache.trim_end().split_once('\n')
            && let Ok(before) = self.input_hash(bytes, dependencies)
            && before.to_hex().as_str() == input
            && fs::symlink_metadata(output).is_ok_and(|metadata| metadata.is_dir())
            && sdk::folder_hash(output).is_ok_and(|hash| hash.to_hex().as_str() == output_hash)
        {
            return Ok(());
        }
        // A failed publish must not leave a previous cache record describing
        // partially updated output. Keep the SDK's separate obj/bin files.
        remove_if_present(fs::remove_file(&record))?;
        remove_if_present(fs::remove_dir_all(output))?;
        let reference_folder = self.path().join("references");
        fs::create_dir_all(&reference_folder)?;
        let expected: std::collections::HashSet<_> = dependencies
            .iter()
            .map(|dependency| format!("{}.dll", dependency.name))
            .collect();
        for entry in fs::read_dir(&reference_folder)? {
            let entry = entry?;
            if entry
                .file_name()
                .to_str()
                .is_none_or(|name| !expected.contains(name))
            {
                fs::remove_file(entry.path())?;
            }
        }
        for dependency in dependencies {
            write_if_changed(
                &reference_folder.join(format!("{}.dll", dependency.name)),
                &dependency.bytes,
            )?;
        }
        write_if_changed(&self.path().join("input.dll"), bytes)?;
        build()?;
        if reuse
            && let Ok(input) = self.input_hash(bytes, dependencies)
            && let Ok(output) = sdk::folder_hash(output)
        {
            let _ = write_file(&record, format!("{input}\n{output}\n").as_bytes());
        }
        Ok(())
    }

    fn input_hash(&self, bytes: &[u8], dependencies: &[Dependency]) -> io::Result<blake3::Hash> {
        let mut hash = blake3::Hasher::new_derive_key("oxel native publish inputs");
        field(&mut hash, bytes);
        for dependency in dependencies {
            field(&mut hash, dependency.name.as_bytes());
            field(&mut hash, &dependency.bytes);
        }
        for name in ["obj/project.assets.json", "packages.lock.json"] {
            match fs::read(self.path().join(name)) {
                Ok(contents) => field(&mut hash, &contents),
                Err(error) if error.kind() == io::ErrorKind::NotFound => field(&mut hash, &[]),
                Err(error) => return Err(error),
            }
        }
        Ok(hash.finalize())
    }
}

fn remove_if_present(result: io::Result<()>) -> io::Result<()> {
    match result {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

pub(super) fn write_if_changed(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if fs::read(path).is_ok_and(|current| current == bytes) {
        return Ok(());
    }
    write_file(path, bytes)
}

pub(super) fn publish(
    project: &Path,
    name: &str,
    bytes: &[u8],
    dependencies: &[Dependency],
    libraries: &[NativeLibrary],
) -> PublishResult<PathBuf> {
    let ((sdk_version, framework), sdk_path) = sdk::run_version(project)?;
    // Use a portable SDK filename while preserving the application's original
    // assembly identity inside the managed input.
    let project_name = (0..=dependencies.len())
        .map(|id| format!("oxel_native_app{id}"))
        .find(|candidate| {
            !dependencies
                .iter()
                .any(|dependency| dependency.name.eq_ignore_ascii_case(candidate))
        })
        .expect("a staging name outside the dependency set");
    let xml = sdk::project_xml(
        &project_name,
        &framework,
        true,
        &Default::default(),
        oxel::AssemblyKind::Executable,
        sdk::Framework::None,
    );
    let cache = if standalone_cache_compatible(project).unwrap_or(false) {
        Cache::open(
            project,
            sdk_path.as_deref(),
            &[
                b"standalone",
                name.as_bytes(),
                sdk_version.as_bytes(),
                xml.as_bytes(),
            ],
        )
    } else {
        None
    };
    let stage = if cache.is_none() {
        Some(tempfile::TempDir::with_prefix("oxel-aot-")?)
    } else {
        None
    };
    let directory = cache
        .as_ref()
        .map(Cache::path)
        .unwrap_or_else(|| stage.as_ref().expect("temporary AOT stage").path());
    if cache.is_some() {
        sdk::isolate_stage(directory)?;
    }
    sdk::pin(directory, &sdk_version)?;
    write_if_changed(&directory.join("oxel.csproj"), xml.as_bytes())?;
    let output = directory.join("publish");
    if let Some(cache) = &cache {
        cache.build(&output, bytes, dependencies, true, || {
            sdk_publish(directory)
        })?;
    } else {
        fs::write(directory.join("input.dll"), bytes)?;
        write_dependencies(&directory.join("references"), dependencies)?;
        sdk_publish(directory)?;
    }

    let sdk_executable = format!("{project_name}{}", env::consts::EXE_SUFFIX);
    install(project, name, &output.join(sdk_executable), libraries)
}

fn standalone_cache_compatible(project: &Path) -> io::Result<bool> {
    // The standalone SDK project previously ran under the temporary directory.
    // Preserve the source of NuGet and MSBuild settings when moving the stage.
    let project = fs::canonicalize(if project.as_os_str().is_empty() {
        Path::new(".")
    } else {
        project
    })?;
    let temporary = fs::canonicalize(env::temp_dir())?;
    for directory in temporary.ancestors() {
        for name in [
            "Directory.Build.props",
            "Directory.Build.targets",
            "Directory.Packages.props",
        ] {
            if directory.join(name).try_exists()? {
                return Ok(false);
            }
        }
    }
    for directory in artifact_directory(&project)
        .ancestors()
        .filter(|directory| !temporary.starts_with(directory))
        .chain(
            temporary
                .ancestors()
                .filter(|directory| !project.starts_with(directory)),
        )
    {
        if directory.join("Directory.Build.rsp").try_exists()?
            || !config_files(directory, |name| name.eq_ignore_ascii_case("NuGet.Config"))?
                .is_empty()
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn sdk_publish(directory: &Path) -> PublishResult<()> {
    let mut child = Command::new("dotnet")
        .current_dir(directory)
        .args([
            "publish",
            "oxel.csproj",
            "-c",
            "Release",
            "-o",
            "publish",
            "--nologo",
            "-v",
            "minimal",
        ])
        .stdout(Stdio::piped())
        .spawn()?;
    let log = io::copy(
        &mut child.stdout.take().expect("piped SDK output"),
        &mut io::stderr(),
    );
    let status = child.wait()?;
    log?;
    if !status.success() {
        return Err(format!("dotnet publish exited with {status}").into());
    }

    Ok(())
}

pub(super) fn install(
    project: &Path,
    name: &str,
    executable: &Path,
    libraries: &[NativeLibrary],
) -> PublishResult<PathBuf> {
    let metadata = fs::metadata(executable)?;
    if !metadata.is_file() {
        return Err(io::Error::other(format!(
            "expected a native executable at '{}'",
            executable.display()
        ))
        .into());
    }
    let file = format!("{name}{}", env::consts::EXE_SUFFIX);
    let output_folder = artifact_directory(project).join("native");
    let sdk_folder = executable.parent().expect("SDK publish directory");
    let sdk_file = executable
        .file_name()
        .and_then(|name| name.to_str())
        .expect("UTF-8 SDK executable name");
    let sdk_name = sdk_file
        .strip_suffix(env::consts::EXE_SUFFIX)
        .expect("SDK executable suffix");
    let symbol_files = [".dbg", ".pdb", ".dSYM"].map(|suffix| format!("{sdk_name}{suffix}"));
    let entries = fs::read_dir(sdk_folder)?.collect::<io::Result<Vec<_>>>()?;
    fs::create_dir_all(&output_folder)?;
    if !libraries.is_empty()
        || entries.iter().any(|entry| {
            let name = entry.file_name();
            name != sdk_file && !symbol_files.iter().any(|symbols| name == symbols.as_str())
        })
    {
        if entries.iter().any(|entry| {
            let name = entry.file_name();
            name != sdk_file && name.to_string_lossy().eq_ignore_ascii_case(&file)
        }) {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("SDK publish asset conflicts with the executable name '{file}'"),
            )
            .into());
        }
        // Stage on the destination filesystem so the complete bundle can be
        // installed with one rename, including when the SDK staged in /tmp.
        let stage = tempfile::TempDir::with_prefix_in(".publish-", &output_folder)?;
        for entry in entries {
            let name = entry.file_name();
            let target = if name == sdk_file {
                stage.path().join(&file)
            } else {
                stage.path().join(name)
            };
            copy_entry(&entry, &target)?;
        }
        copy_libraries(stage.path(), libraries)?;
        let bundle = sdk::store_directory(&output_folder, stage.path())?;
        return Ok(bundle.join(file));
    }
    let output = output_folder.join(&file);
    if fs::symlink_metadata(&output).is_ok_and(|existing| {
        existing.is_file()
            && existing.permissions() == metadata.permissions()
            && existing.len() == metadata.len()
    }) && file_digest(&output)
        .ok()
        .zip(file_digest(executable).ok())
        .is_some_and(|(a, b)| a == b)
    {
        return Ok(output);
    }
    let mut temp = tempfile::NamedTempFile::new_in(&output_folder)?;
    let mut executable = fs::File::open(executable)?;
    io::copy(&mut executable, temp.as_file_mut())?;
    temp.as_file().set_permissions(metadata.permissions())?;
    temp.persist(&output)?;
    Ok(output)
}

fn copy_entry(source: &fs::DirEntry, destination: &Path) -> io::Result<()> {
    let kind = source.file_type()?;
    if kind.is_file() {
        fs::copy(source.path(), destination)?;
    } else if kind.is_dir() {
        fs::create_dir(destination)?;
        for entry in fs::read_dir(source.path())? {
            let entry = entry?;
            copy_entry(&entry, &destination.join(entry.file_name()))?;
        }
    } else {
        return Err(io::Error::other(format!(
            "unexpected SDK output '{}'",
            source.path().display()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installs_native_assets_as_one_reusable_bundle() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("sdk-output");
        let executable = source.join(format!("sdk-app{}", env::consts::EXE_SUFFIX));
        fs::create_dir_all(source.join("assets/subdir")).unwrap();
        fs::write(&executable, "native executable").unwrap();
        let library = "runtime-library";
        fs::write(source.join(library), "native library").unwrap();
        let data = "assets/subdir/app.dat";
        fs::write(source.join(data), "package data").unwrap();

        let first = install(root.path(), "my-app", &executable, &[]).unwrap();
        assert_eq!(
            first.file_name().unwrap(),
            format!("my-app{}", env::consts::EXE_SUFFIX).as_str()
        );
        let bundle = first.parent().unwrap();
        assert_eq!(
            bundle.parent().unwrap(),
            root.path().join("target/oxel/native")
        );
        assert_eq!(fs::read_to_string(&first).unwrap(), "native executable");
        assert_eq!(
            fs::read_to_string(bundle.join(library)).unwrap(),
            "native library"
        );
        assert_eq!(
            fs::read_to_string(bundle.join(data)).unwrap(),
            "package data"
        );
        assert_eq!(
            install(root.path(), "my-app", &executable, &[]).unwrap(),
            first
        );

        fs::write(source.join(library), "updated library").unwrap();
        let second = install(root.path(), "my-app", &executable, &[]).unwrap();
        assert_ne!(first, second);
        assert_eq!(
            fs::read_to_string(bundle.join(library)).unwrap(),
            "native library"
        );
        assert_eq!(
            fs::read_to_string(second.parent().unwrap().join(library)).unwrap(),
            "updated library"
        );
    }
}
