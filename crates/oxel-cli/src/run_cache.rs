//! Reuses complete SDK-built bundles for unchanged, locked NuGet runs.

use std::{
    env,
    ffi::OsStr,
    fs, io,
    path::{Path, PathBuf},
};

use crate::{Package, artifact_directory, manifest, sdk, write_file};

pub(super) struct Inputs<'a> {
    pub package: Package<'a>,
    pub manifest_hash: blake3::Hash,
    pub sources: &'a [PathBuf],
    pub references: &'a [PathBuf],
    pub framework: sdk::Framework,
    pub debug: bool,
}

pub(super) struct Cache {
    lock: PathBuf,
    files: Vec<PathBuf>,
    project: PathBuf,
    manifest_hash: blake3::Hash,
    base: blake3::Hasher,
    entry: PathBuf,
    key: blake3::Hash,
}

impl Cache {
    pub fn prepare(inputs: Inputs<'_>, selection: &(String, String)) -> Option<Self> {
        let project =
            std::path::absolute(manifest::directory(inputs.package.manifest).join(".")).ok()?;
        let lock = sdk::lock_path(inputs.package.manifest, &selection.1, None);
        if !lock.is_file() {
            return None;
        }
        let files: Vec<_> = [inputs.package.manifest, lock.as_path()]
            .into_iter()
            .chain(inputs.sources.iter().map(PathBuf::as_path))
            .chain(inputs.references.iter().map(PathBuf::as_path))
            .chain(
                inputs
                    .package
                    .native_libraries
                    .iter()
                    .map(|library| library.path.as_path()),
            )
            .map(Path::to_path_buf)
            .collect();
        let mut base = blake3::Hasher::new_derive_key("oxel locked run bundle v1");
        field(&mut base, project.as_os_str().as_encoded_bytes());
        field(&mut base, selection.0.as_bytes());
        field(&mut base, selection.1.as_bytes());
        field(&mut base, format!("{:?}", inputs.framework).as_bytes());
        base.update(&[u8::from(inputs.debug)]);
        for length in [inputs.sources.len(), inputs.references.len()] {
            base.update(&(length as u64).to_le_bytes());
        }
        let mut vars: Vec<_> = env::vars_os().collect();
        vars.sort_unstable();
        for (name, value) in vars {
            if matches!(name.to_str(), Some("_" | "PWD" | "OLDPWD" | "SHLVL")) {
                continue;
            }
            let upper = name.to_string_lossy().to_ascii_uppercase();
            if [
                "MSBUILD",
                "DOTNET_MSBUILD",
                "CUSTOMBEFORE",
                "CUSTOMAFTER",
                "DIRECTORYBUILD",
                "IMPORT",
            ]
            .iter()
            .any(|prefix| upper.starts_with(prefix))
            {
                return None;
            }
            field(&mut base, name.as_encoded_bytes());
            field(&mut base, value.as_encoded_bytes());
        }
        let key = fingerprint(&base, &files, &project, inputs.manifest_hash).ok()?;
        let entry = artifact_directory(&project)
            .join("run-cache")
            .join(key.to_hex().as_str());
        Some(Self {
            lock,
            files,
            project,
            manifest_hash: inputs.manifest_hash,
            base,
            entry,
            key,
        })
    }

    pub fn lock(&self) -> &Path {
        &self.lock
    }

    pub fn lookup(&self, name: &str) -> Option<PathBuf> {
        let record = fs::read_to_string(self.entry.join("latest")).ok()?;
        let (directory, digest) = record.trim_end().split_once('\n')?;
        if !directory.starts_with("bundle-")
            || Path::new(directory).file_name() != Some(OsStr::new(directory))
        {
            return None;
        }
        let digest = blake3::Hash::from_hex(digest).ok()?;
        let bundle = self.entry.join(directory).join("output");
        let dll = bundle.join(format!("{name}.dll"));
        (fs::symlink_metadata(&bundle).ok()?.is_dir()
            && dll.is_file()
            && sdk::folder_hash(&bundle).ok()? == digest)
            .then_some(dll)
    }

    pub fn publish(&self, output: &Path) -> Option<PathBuf> {
        // The SDK and compiler read inputs independently. Publish only for a
        // snapshot that still matches the one taken before building.
        if fingerprint(&self.base, &self.files, &self.project, self.manifest_hash).ok()? != self.key
        {
            return None;
        }
        let digest = sdk::folder_hash(output).ok()?;
        fs::create_dir_all(&self.entry).ok()?;
        let stage = tempfile::Builder::new()
            .prefix("bundle-")
            .tempdir_in(&self.entry)
            .ok()?;
        let record = format!("{}\n{digest}\n", stage.path().file_name()?.to_str()?);
        let bundle = stage.path().join("output");
        // Rename the completed SDK output as a unit. The pointer is replaced
        // only after the bundle is complete, even when two builds finish together.
        fs::rename(output, &bundle).ok()?;
        let _ = stage.keep();
        let _ = write_file(&self.entry.join("latest"), record.as_bytes());
        Some(bundle)
    }
}

fn fingerprint(
    base: &blake3::Hasher,
    files: &[PathBuf],
    project: &Path,
    manifest_hash: blake3::Hash,
) -> io::Result<blake3::Hash> {
    let mut hash = base.clone();
    let (manifest, inputs) = files.split_first().expect("manifest input");
    if file_digest(manifest)? != manifest_hash {
        return Err(io::Error::other("project manifest changed"));
    }
    field(
        &mut hash,
        std::path::absolute(manifest)?
            .as_os_str()
            .as_encoded_bytes(),
    );
    hash.update(manifest_hash.as_bytes());
    for input in inputs {
        hash_file(&mut hash, input)?;
    }
    hash_file(&mut hash, &env::current_exe()?)?;
    hash_config(&mut hash, project)?;
    Ok(hash.finalize())
}

fn field(hash: &mut blake3::Hasher, bytes: &[u8]) {
    hash.update(&(bytes.len() as u64).to_le_bytes());
    hash.update(bytes);
}

fn file_digest(path: &Path) -> io::Result<blake3::Hash> {
    if !fs::metadata(path)?.is_file() {
        return Err(io::Error::other("cache input is not a file"));
    }
    let file = fs::File::open(path)?;
    Ok(blake3::Hasher::new().update_reader(file)?.finalize())
}

fn hash_file(hash: &mut blake3::Hasher, path: &Path) -> io::Result<()> {
    field(
        hash,
        std::path::absolute(path)?.as_os_str().as_encoded_bytes(),
    );
    hash.update(file_digest(path)?.as_bytes());
    Ok(())
}

fn hash_config(hash: &mut blake3::Hasher, project: &Path) -> io::Result<()> {
    let artifacts = artifact_directory(project);
    let physical = fs::canonicalize(project)?;
    for directory in artifacts.ancestors().chain(physical.ancestors()) {
        if directory.join("Directory.Build.rsp").try_exists()? {
            return Err(io::Error::other("custom MSBuild response file"));
        }
        let mut configs = config_files(directory, |name| {
            name.eq_ignore_ascii_case("NuGet.Config") || name == "global.json"
        })?;
        configs.sort_unstable();
        for config in configs {
            hash_file(hash, &config)?;
        }
    }
    let mut user = Vec::new();
    if let Some(home) = env::var_os("HOME").map(PathBuf::from) {
        user.extend([home.join(".nuget/NuGet"), home.join(".config/NuGet")]);
        hash_extra_config(hash, &home.join(".nuget/config"))?;
    }
    if let Some(appdata) = env::var_os("APPDATA") {
        user.push(PathBuf::from(appdata).join("NuGet"));
    }
    for directory in user {
        for config in config_files(&directory, |name| name.eq_ignore_ascii_case("NuGet.Config"))? {
            hash_file(hash, &config)?;
        }
        hash_extra_config(hash, &directory.join("config"))?;
    }
    let common = env::var_os("NUGET_COMMON_APPLICATION_DATA")
        .filter(|value| !value.is_empty())
        .or_else(|| env::var_os("ProgramFiles(x86)"))
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(if cfg!(target_os = "macos") {
                "/Library/Application Support"
            } else {
                "/etc/opt"
            })
        });
    hash_extra_config(hash, &common.join("NuGet/Config"))
}

fn hash_extra_config(hash: &mut blake3::Hasher, directory: &Path) -> io::Result<()> {
    let mut configs = config_files(directory, |name| {
        Path::new(name)
            .extension()
            .and_then(OsStr::to_str)
            .is_some_and(|extension| extension.eq_ignore_ascii_case("config"))
    })?;
    configs.sort_unstable();
    for config in configs {
        hash_file(hash, &config)?;
    }
    Ok(())
}

fn config_files(directory: &Path, matches: impl Fn(&str) -> bool) -> io::Result<Vec<PathBuf>> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry?;
        if entry.file_name().to_str().is_some_and(&matches) {
            files.push(entry.path());
        }
    }
    Ok(files)
}
