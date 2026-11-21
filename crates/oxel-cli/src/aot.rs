//! Publishes oxel's managed executable through the installed .NET SDK.

use std::{
    env, fs, io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use crate::{
    Dependency, NativeLibrary, artifact_directory, copy_libraries, sdk, write_dependencies,
};

type PublishResult<T> = Result<T, Box<dyn std::error::Error>>;

pub(super) fn publish(
    project: &Path,
    name: &str,
    bytes: &[u8],
    dependencies: &[Dependency],
    libraries: &[NativeLibrary],
) -> PublishResult<PathBuf> {
    let (sdk_version, framework) = sdk::version(project)?;
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
    let stage = tempfile::TempDir::with_prefix("oxel-aot-")?;
    let directory = stage.path();
    sdk::pin(directory, &sdk_version)?;
    fs::write(directory.join("input.dll"), bytes)?;
    write_dependencies(&directory.join("references"), dependencies)?;
    fs::write(
        directory.join("oxel.csproj"),
        sdk::project_xml(
            &project_name,
            &framework,
            true,
            &Default::default(),
            oxel::AssemblyKind::Executable,
            sdk::Framework::None,
        ),
    )?;

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

    let sdk_executable = format!("{project_name}{}", env::consts::EXE_SUFFIX);
    install(
        project,
        name,
        &directory.join("publish").join(sdk_executable),
        libraries,
    )
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
