//! Formats explicit files or the source set of the selected project.

use std::{
    collections::HashSet,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::ExitCode,
};

use crate::diagnostics::Format;

struct Change<'a> {
    path: &'a Path,
    target: PathBuf,
    before: String,
    after: String,
}

pub(super) fn run(paths: &[PathBuf], check: bool, format: Format) -> ExitCode {
    let mut changes = Vec::new();
    let mut seen = HashSet::new();
    let mut status = ExitCode::SUCCESS;
    // Validate every file before replacing any of them.
    for path in paths {
        let (target, source) = match read(path) {
            Ok(file) => file,
            Err(error) => {
                status = format.error(path, "E0002", format!("could not read source: {error}"));
                continue;
            }
        };
        if !seen.insert(target.clone()) {
            continue;
        }
        match oxel::format_source(&source) {
            Ok(after) if source != after => {
                if check {
                    status = format.error(path, "E0013", "file needs formatting");
                } else {
                    changes.push(Change {
                        path,
                        target,
                        before: source,
                        after,
                    });
                }
            }
            Ok(_) => {}
            Err(errors) => {
                for diagnostic in errors {
                    format.print(path, &source, &diagnostic);
                }
                status = ExitCode::FAILURE;
            }
        }
    }
    if status != ExitCode::SUCCESS {
        return status;
    }
    for change in changes {
        if let Err(error) = change.write() {
            status = format.error(
                change.path,
                "E0014",
                format!("could not write formatted source: {error}"),
            );
        }
    }
    status
}

fn read(path: &Path) -> io::Result<(PathBuf, String)> {
    let target = fs::canonicalize(path)?;
    let source = fs::read_to_string(&target)?;
    Ok((target, source))
}

impl Change<'_> {
    fn write(&self) -> io::Result<()> {
        // Resolve symlinks above so the atomic replacement keeps the original link.
        let directory = self.target.parent().expect("source file has a parent");
        let mut staged = tempfile::NamedTempFile::new_in(directory)?;
        staged.write_all(self.after.as_bytes())?;
        staged
            .as_file()
            .set_permissions(fs::metadata(&self.target)?.permissions())?;
        if fs::read_to_string(&self.target)? != self.before {
            return Err(io::Error::other("source changed during formatting; retry"));
        }
        staged.persist(&self.target)?;
        Ok(())
    }
}
