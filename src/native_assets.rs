//! Stages manifest-declared dynamic libraries beside the managed or native host.

use std::{
    collections::HashSet,
    fs, io,
    path::{Path, PathBuf},
};

pub(super) struct NativeLibrary {
    pub path: PathBuf,
    pub name: String,
}

pub(super) fn copy_libraries(folder: &Path, libraries: &[NativeLibrary]) -> io::Result<()> {
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
        fs::copy(&library.path, folder.join(&library.name)).map_err(|error| {
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
