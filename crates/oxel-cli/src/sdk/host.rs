//! Resolve the SDK with the native .NET host before starting a JIT run.

use std::{
    cell::Cell,
    env,
    ffi::{CStr, CString, OsStr, OsString, c_char, c_int},
    fs::{self, File},
    io::{self, Read},
    os::unix::{
        ffi::{OsStrExt, OsStringExt},
        fs::PermissionsExt,
    },
    path::{Path, PathBuf},
    process::Command,
};

thread_local! {
    static SDK_DIRECTORY: Cell<Option<OsString>> = const { Cell::new(None) };
}

type SdkCallback = extern "C" fn(c_int, *const c_char);
type ResolveSdk = unsafe extern "C" fn(*const c_char, *const c_char, c_int, SdkCallback) -> c_int;
type ErrorWriter = extern "C" fn(*const c_char);
type SetErrorWriter = unsafe extern "C" fn(Option<ErrorWriter>) -> Option<ErrorWriter>;

pub(super) fn resolve(project: &Path) -> Option<PathBuf> {
    let project = fs::canonicalize(project).ok()?;
    let dotnet = path_host(&project)?;
    let root = dotnet.parent()?;
    let library_path = hostfxr(root)?;
    let exe_dir = CString::new(root.as_os_str().as_bytes()).ok()?;
    let working_dir = CString::new(project.as_os_str().as_bytes()).ok()?;
    // The native host and the dotnet executable come from the same selected
    // installation. Keep the library loaded for the entire synchronous call.
    let library = unsafe { libloading::Library::new(library_path) }.ok()?;
    let resolve = unsafe { library.get::<ResolveSdk>(b"hostfxr_resolve_sdk2\0") }.ok()?;
    let set_error_writer =
        unsafe { library.get::<SetErrorWriter>(b"hostfxr_set_error_writer\0") }.ok()?;
    SDK_DIRECTORY.with(Cell::take);
    // Use the normal CLI diagnostic if the native probe fails.
    let previous = unsafe { set_error_writer(Some(ignore_probe_error)) };
    let status = unsafe { resolve(exe_dir.as_ptr(), working_dir.as_ptr(), 0, sdk_result) };
    unsafe { set_error_writer(previous) };
    let directory = SDK_DIRECTORY.with(Cell::take)?;
    let directory = PathBuf::from(directory);
    (status == 0 && directory.is_absolute() && directory.join("dotnet.dll").is_file())
        .then_some(directory)
}

extern "C" fn ignore_probe_error(_: *const c_char) {}

extern "C" fn sdk_result(key: c_int, value: *const c_char) {
    if key == 0 && !value.is_null() {
        // hostfxr owns this string only until the callback returns.
        let value = unsafe { CStr::from_ptr(value) };
        SDK_DIRECTORY.with(|directory| {
            directory.set(Some(OsString::from_vec(value.to_bytes().to_vec())));
        });
    }
}

fn path_host(project: &Path) -> Option<PathBuf> {
    for folder in env::split_paths(&env::var_os("PATH")?) {
        // Delegate unusual PATH resolution rules to Command.
        if !folder.is_absolute() {
            return None;
        }
        let candidate = folder.join("dotnet");
        match fs::metadata(&candidate) {
            Ok(metadata) if metadata.is_file() && metadata.permissions().mode() & 0o111 != 0 => {
                return selected_host(&candidate, project);
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(_) => return None,
        }
    }
    None
}

fn selected_host(candidate: &Path, project: &Path) -> Option<PathBuf> {
    let executable = fs::canonicalize(candidate).ok()?;
    let executable =
        if executable.file_name()? == "mise" && candidate.parent()?.file_name()? == "shims" {
            // Ask the shim's own tool selector so project-local mise config still
            // controls which .NET installation the host uses.
            let output = Command::new(executable)
                .args(["which", "dotnet"])
                .current_dir(project)
                .output()
                .ok()?;
            if !output.status.success() {
                return None;
            }
            let path = Path::new(OsStr::from_bytes(output.stdout.strip_suffix(b"\n")?));
            fs::canonicalize(path.is_absolute().then_some(path)?).ok()?
        } else {
            executable
        };
    if executable.file_name()? != "dotnet" {
        return None;
    }
    let mut magic = [0; 4];
    File::open(&executable).ok()?.read_exact(&mut magic).ok()?;
    is_native_image(magic).then_some(executable)
}

fn is_native_image(magic: [u8; 4]) -> bool {
    if cfg!(target_os = "linux") {
        magic == *b"\x7fELF"
    } else {
        matches!(
            u32::from_be_bytes(magic),
            0xfeedface
                | 0xcefaedfe
                | 0xfeedfacf
                | 0xcffaedfe
                | 0xcafebabe
                | 0xbebafeca
                | 0xcafebabf
                | 0xbfbafeca
        )
    }
}

fn hostfxr(root: &Path) -> Option<PathBuf> {
    let filename = libloading::library_filename("hostfxr");
    let mut libraries = Vec::new();
    for entry in fs::read_dir(root.join("host/fxr")).ok()? {
        let entry = entry.ok()?;
        let library = entry.path().join(&filename);
        if !library.is_file() {
            continue;
        }
        // Release versions can be ordered numerically. Leave mixed or
        // preview host installations to the muxer.
        let version = entry.file_name();
        let mut parts = version.to_str()?.split('.');
        let numbers = [
            parts.next()?.parse::<u32>().ok()?,
            parts.next()?.parse::<u32>().ok()?,
            parts.next()?.parse::<u32>().ok()?,
        ];
        if parts.next().is_some() {
            return None;
        }
        libraries.push((numbers, library));
    }
    libraries
        .into_iter()
        .max_by_key(|(version, _)| *version)
        .map(|(_, path)| path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_wrappers_use_the_cli_resolver() {
        let directory = tempfile::tempdir().unwrap();
        let wrapper = directory.path().join("dotnet");
        fs::write(&wrapper, "#!/bin/sh\necho 10.0.100\n").unwrap();
        assert!(selected_host(&wrapper, directory.path()).is_none());
    }

    #[test]
    fn selects_the_highest_stable_host_version() {
        let directory = tempfile::tempdir().unwrap();
        let add = |version| {
            let folder = directory.path().join("host/fxr").join(version);
            fs::create_dir_all(&folder).unwrap();
            let library = folder.join(libloading::library_filename("hostfxr"));
            fs::write(&library, []).unwrap();
            library
        };
        add("10.0.9");
        let latest = add("10.0.14");
        assert_eq!(hostfxr(directory.path()), Some(latest));
        add("11.0.0-preview.1");
        assert!(hostfxr(directory.path()).is_none());
    }

    #[test]
    fn installed_host_honors_global_json() {
        let directory = tempfile::tempdir().unwrap();
        let Some(sdk) = resolve(directory.path()) else {
            return;
        };
        let version = sdk.file_name().unwrap().to_str().unwrap();
        super::super::pin(directory.path(), version).unwrap();
        assert_eq!(resolve(directory.path()), Some(sdk));

        super::super::pin(directory.path(), "999.0.100").unwrap();
        assert!(resolve(directory.path()).is_none());
    }
}
