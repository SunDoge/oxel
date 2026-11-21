//! Reuses the SDK's dependency outputs across edits to locked NuGet scripts.

use std::{
    collections::HashMap,
    env,
    ffi::OsStr,
    fs, io,
    path::{Path, PathBuf},
};

use crate::{Package, artifact_directory, manifest, sdk, write_file};
use serde::{Deserialize, Serialize};

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
    sources: Vec<PathBuf>,
    project: PathBuf,
    manifest_hash: blake3::Hash,
    base: blake3::Hasher,
    entry: PathBuf,
    key: blake3::Hash,
    source_key: blake3::Hash,
}

pub(super) enum Hit {
    Exact(PathBuf),
    Edit(EditBundle, VerifiedReferences),
}

pub(super) struct EditBundle {
    stage: tempfile::TempDir,
    assets_hash: [u8; 32],
}

impl EditBundle {
    pub fn path(&self) -> &Path {
        self.stage.path()
    }

    pub fn integrity(&self, assembly: &[u8]) -> BundleIntegrity {
        BundleIntegrity {
            assets_hash: self.assets_hash,
            assembly_hash: *blake3::hash(assembly).as_bytes(),
        }
    }
}

#[derive(Deserialize, Serialize)]
pub(super) struct BundleIntegrity {
    assets_hash: [u8; 32],
    assembly_hash: [u8; 32],
}

impl BundleIntegrity {
    pub fn read(output: &Path, name: &str) -> io::Result<Self> {
        let assembly = format!("{name}.dll");
        Ok(Self {
            assets_hash: *sdk::folder_hash_without(output, OsStr::new(&assembly))?.as_bytes(),
            assembly_hash: *assembly_digest(&output.join(assembly))?.as_bytes(),
        })
    }
}

pub(super) struct VerifiedReferences {
    pub record: ReferenceRecord,
    pub assemblies: HashMap<PathBuf, Vec<u8>>,
}

#[derive(Deserialize, Serialize)]
struct Record {
    directory: String,
    integrity: BundleIntegrity,
    source_hash: [u8; 32],
    references: Option<ReferenceRecord>,
}

#[derive(Clone, Deserialize, Serialize)]
pub(super) struct ReferenceRecord {
    pub inputs: sdk::RunReferences,
    hash: [u8; 32],
}

impl ReferenceRecord {
    pub fn capture(inputs: sdk::RunReferences) -> Option<Self> {
        let hash = *reference_hash(&inputs, None).ok()?.as_bytes();
        Some(Self { inputs, hash })
    }

    fn verify(self) -> Option<VerifiedReferences> {
        let mut assemblies = HashMap::new();
        let hash = reference_hash(&self.inputs, Some(&mut assemblies)).ok()?;
        (hash.as_bytes() == &self.hash).then_some(VerifiedReferences {
            record: self,
            assemblies,
        })
    }
}

impl Cache {
    pub fn prepare(
        inputs: Inputs<'_>,
        selection: &(String, String),
        sdk_path: Option<&Path>,
    ) -> Option<Self> {
        let project =
            std::path::absolute(manifest::directory(inputs.package.manifest).join(".")).ok()?;
        let lock = sdk::lock_path(inputs.package.manifest, &selection.1, None);
        if !lock.is_file() {
            return None;
        }
        let files: Vec<_> = [inputs.package.manifest, lock.as_path()]
            .into_iter()
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
        let mut base = blake3::Hasher::new_derive_key("oxel locked run bundle v3");
        field(&mut base, project.as_os_str().as_encoded_bytes());
        field(&mut base, selection.0.as_bytes());
        field(&mut base, selection.1.as_bytes());
        field(
            &mut base,
            sdk_path.map_or(&[][..], |path| path.as_os_str().as_encoded_bytes()),
        );
        field(&mut base, format!("{:?}", inputs.framework).as_bytes());
        base.update(&[u8::from(inputs.debug)]);
        base.update(&(inputs.references.len() as u64).to_le_bytes());
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
        let source_key = paths_hash("oxel run sources", inputs.sources).ok()?;
        let entry = artifact_directory(&project)
            .join("run-cache")
            .join(key.to_hex().as_str());
        Some(Self {
            lock,
            files,
            sources: inputs.sources.to_vec(),
            project,
            manifest_hash: inputs.manifest_hash,
            base,
            entry,
            key,
            source_key,
        })
    }

    pub fn lock(&self) -> &Path {
        &self.lock
    }

    pub fn lookup(&self, name: &str) -> Option<Hit> {
        let record: Record =
            serde_json::from_slice(&fs::read(self.entry.join("latest")).ok()?).ok()?;
        if !record.directory.starts_with("bundle-")
            || Path::new(&record.directory).file_name() != Some(OsStr::new(&record.directory))
        {
            return None;
        }
        let bundle = self.entry.join(record.directory).join("output");
        let assembly = format!("{name}.dll");
        let assembly_name = OsStr::new(&assembly);
        let dll = bundle.join(&assembly);
        if !fs::symlink_metadata(&bundle).ok()?.is_dir()
            || sdk::folder_hash_without(&bundle, assembly_name)
                .ok()?
                .as_bytes()
                != &record.integrity.assets_hash
        {
            return None;
        }
        if self.source_key.as_bytes() == &record.source_hash {
            return (assembly_digest(&dll).ok()?.as_bytes() == &record.integrity.assembly_hash)
                .then_some(Hit::Exact(dll));
        }
        let references = record.references?.verify()?;
        let stage = tempfile::Builder::new()
            .prefix(".edit-")
            .tempdir_in(&self.entry)
            .ok()?;
        stage_assets(&bundle, stage.path(), Some(assembly_name)).ok()?;
        Some(Hit::Edit(
            EditBundle {
                stage,
                assets_hash: record.integrity.assets_hash,
            },
            references,
        ))
    }

    pub fn publish(
        &self,
        output: &Path,
        references: Option<ReferenceRecord>,
        integrity: BundleIntegrity,
    ) -> Option<PathBuf> {
        // The SDK and compiler read inputs independently. Publish only for a
        // snapshot that still matches the one taken before building.
        if fingerprint(&self.base, &self.files, &self.project, self.manifest_hash).ok()? != self.key
            || paths_hash("oxel run sources", &self.sources).ok()? != self.source_key
        {
            return None;
        }
        fs::create_dir_all(&self.entry).ok()?;
        let stage = tempfile::Builder::new()
            .prefix("bundle-")
            .tempdir_in(&self.entry)
            .ok()?;
        let record = Record {
            directory: stage.path().file_name()?.to_str()?.to_owned(),
            integrity,
            source_hash: *self.source_key.as_bytes(),
            references,
        };
        let record = serde_json::to_vec(&record).ok()?;
        let bundle = stage.path().join("output");
        // Rename the completed SDK output as a unit. The pointer is replaced
        // only after the bundle is complete, even when two builds finish together.
        fs::rename(output, &bundle).ok()?;
        let _ = stage.keep();
        let _ = write_file(&self.entry.join("latest"), &record);
        Some(bundle)
    }
}

fn assembly_digest(path: &Path) -> io::Result<blake3::Hash> {
    if !fs::symlink_metadata(path)?.is_file() {
        return Err(io::Error::other("cached assembly is not a regular file"));
    }
    file_digest(path)
}

fn stage_assets(source: &Path, destination: &Path, skip: Option<&OsStr>) -> io::Result<()> {
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let name = entry.file_name();
        if Some(name.as_os_str()) == skip {
            continue;
        }
        let path = entry.path();
        let destination = destination.join(name);
        let kind = entry.file_type()?;
        if kind.is_dir() {
            fs::create_dir(&destination)?;
            stage_assets(&path, &destination, None)?;
        } else if kind.is_file() {
            if fs::hard_link(&path, &destination).is_err() {
                fs::copy(&path, &destination)?;
            }
        } else {
            return Err(io::Error::other("invalid cached runtime asset"));
        }
    }
    Ok(())
}

fn reference_hash(
    inputs: &sdk::RunReferences,
    mut assemblies: Option<&mut HashMap<PathBuf, Vec<u8>>>,
) -> io::Result<blake3::Hash> {
    let mut hash = blake3::Hasher::new_derive_key("oxel cached SDK references");
    for paths in [&inputs.packages, &inputs.framework] {
        hash.update(&(paths.len() as u64).to_le_bytes());
        for path in paths {
            if let Some(assemblies) = assemblies.as_deref_mut() {
                require_regular_file(path)?;
                let bytes = fs::read(path)?;
                hash_path(&mut hash, path, blake3::hash(&bytes))?;
                assemblies.insert(path.clone(), bytes);
            } else {
                hash_file(&mut hash, path)?;
            }
        }
    }
    hash.update(&(inputs.build_files.len() as u64).to_le_bytes());
    for path in &inputs.build_files {
        hash_file(&mut hash, path)?;
    }
    Ok(hash.finalize())
}

fn paths_hash(domain: &str, paths: &[PathBuf]) -> io::Result<blake3::Hash> {
    let mut hash = blake3::Hasher::new_derive_key(domain);
    for path in paths {
        hash_file(&mut hash, path)?;
    }
    Ok(hash.finalize())
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
    require_regular_file(path)?;
    let file = fs::File::open(path)?;
    Ok(blake3::Hasher::new().update_reader(file)?.finalize())
}

fn require_regular_file(path: &Path) -> io::Result<()> {
    if !fs::metadata(path)?.is_file() {
        return Err(io::Error::other("cache input is not a file"));
    }
    Ok(())
}

fn hash_file(hash: &mut blake3::Hasher, path: &Path) -> io::Result<()> {
    hash_path(hash, path, file_digest(path)?)
}

fn hash_path(hash: &mut blake3::Hasher, path: &Path, content: blake3::Hash) -> io::Result<()> {
    field(
        hash,
        std::path::absolute(path)?.as_os_str().as_encoded_bytes(),
    );
    hash.update(content.as_bytes());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verified_reference_bytes_stay_bound_to_the_cached_digest() {
        let folder = tempfile::tempdir().unwrap();
        let reference = folder.path().join("reference.dll");
        fs::write(&reference, b"first image").unwrap();
        let snapshot = ReferenceRecord::capture(sdk::RunReferences {
            packages: vec![reference.clone()],
            framework: Vec::new(),
            build_files: Vec::new(),
        })
        .unwrap();
        let verified = snapshot.clone().verify().unwrap();
        fs::write(&reference, b"other image").unwrap();
        assert_eq!(verified.assemblies[&reference], b"first image");
        assert!(snapshot.verify().is_none());
    }

    #[test]
    fn app_edits_and_runtime_asset_changes_have_independent_digests() {
        let folder = tempfile::tempdir().unwrap();
        let assembly = folder.path().join("app.dll");
        let asset = folder.path().join("dependency.dll");
        fs::write(&assembly, b"old code").unwrap();
        fs::write(&asset, b"old dependency").unwrap();
        let original = BundleIntegrity::read(folder.path(), "app").unwrap();

        fs::write(&assembly, b"new code").unwrap();
        let edited = BundleIntegrity::read(folder.path(), "app").unwrap();
        assert_ne!(original.assembly_hash, edited.assembly_hash);
        assert_eq!(original.assets_hash, edited.assets_hash);

        fs::write(&asset, b"new dependency").unwrap();
        let changed = BundleIntegrity::read(folder.path(), "app").unwrap();
        assert_eq!(edited.assembly_hash, changed.assembly_hash);
        assert_ne!(edited.assets_hash, changed.assets_hash);
    }
}
