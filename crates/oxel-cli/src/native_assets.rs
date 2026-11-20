//! Stages manifest-declared dynamic libraries beside the managed or native host.

use std::{
    collections::HashSet,
    fs,
    io::{self, Seek},
    path::{Path, PathBuf},
};

pub(super) struct NativeLibrary {
    pub path: PathBuf,
    pub name: String,
}

pub(super) fn copy_libraries(folder: &Path, libraries: &[NativeLibrary]) -> io::Result<()> {
    install_libraries(folder, libraries, |library, destination| {
        fs::copy(&library.path, destination).map(|_| ())
    })
}

pub(super) fn stage_cached_libraries(
    folder: &Path,
    cache: &Path,
    libraries: &[NativeLibrary],
) -> io::Result<()> {
    install_libraries(folder, libraries, |library, destination| {
        let snapshot = snapshot(cache, library)?;
        if fs::hard_link(&snapshot, destination).is_err() {
            fs::copy(&snapshot, destination)?;
        }
        Ok(())
    })
}

fn install_libraries(
    folder: &Path,
    libraries: &[NativeLibrary],
    install: impl Fn(&NativeLibrary, &Path) -> io::Result<()>,
) -> io::Result<()> {
    if libraries.is_empty() {
        return Ok(());
    }
    let mut names = fs::read_dir(folder)?
        .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().to_ascii_lowercase()))
        .collect::<io::Result<HashSet<_>>>()?;
    for library in libraries {
        if !names.insert(library.name.to_ascii_lowercase()) {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!(
                    "native library '{}' conflicts with output asset '{}'",
                    library.path.display(),
                    library.name
                ),
            ));
        }
        install(library, &folder.join(&library.name)).map_err(|error| {
            io::Error::new(
                error.kind(),
                format!(
                    "could not copy native library '{}': {error}",
                    library.path.display()
                ),
            )
        })?;
    }
    Ok(())
}

fn snapshot(cache: &Path, library: &NativeLibrary) -> io::Result<PathBuf> {
    let source = &library.path;
    if !fs::metadata(source)?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a native library must be a regular file",
        ));
    }
    let mut file = fs::File::open(source)?;
    // Loaders can treat hard links to one inode as a single library.
    let mut hash = blake3::Hasher::new_derive_key("oxel native asset snapshot");
    hash.update(&(library.name.len() as u64).to_le_bytes());
    hash.update(library.name.as_bytes());
    let expected = hash.clone().update_reader(&mut file)?.finalize();
    let destination = cache.join(expected.to_hex().as_str());
    if destination.is_file() {
        return Ok(destination);
    }

    let mut staging = tempfile::NamedTempFile::new_in(cache)?;
    file.rewind()?;
    io::copy(&mut file, &mut staging)?;
    staging.as_file_mut().rewind()?;
    hash.update_reader(staging.as_file_mut())?;
    if hash.finalize() != expected {
        return Err(io::Error::other(
            "the native library changed while being copied",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = file.metadata()?.permissions().mode();
        staging
            .as_file()
            .set_permissions(fs::Permissions::from_mode(mode & 0o555))?;
    }
    match staging.persist_noclobber(&destination) {
        Ok(_) => Ok(destination),
        Err(error)
            if error.error.kind() == io::ErrorKind::AlreadyExists && destination.is_file() =>
        {
            Ok(destination)
        }
        Err(error) => Err(error.error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_native_code_under_distinct_import_names_has_separate_snapshots() {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("source.so");
        fs::write(&file, b"native code").unwrap();
        let first = NativeLibrary {
            path: file.clone(),
            name: "first.so".into(),
        };
        let second = NativeLibrary {
            path: file,
            name: "second.so".into(),
        };
        let cached = snapshot(folder.path(), &first).unwrap();
        assert_eq!(snapshot(folder.path(), &first).unwrap(), cached);
        assert_ne!(snapshot(folder.path(), &second).unwrap(), cached);
    }
}
