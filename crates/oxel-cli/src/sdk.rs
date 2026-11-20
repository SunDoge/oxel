//! Stages SDK projects for framework and NuGet references and packaging.

mod framework;

use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
    process::Command,
};

use oxel::AssemblyKind;
use snafu::{ResultExt, Snafu};

use crate::{Dependency, artifact_directory, manifest, write_dependencies, write_file};

type SdkResult<T> = Result<T, Error>;

#[derive(Debug, Snafu)]
pub(super) enum Error {
    #[snafu(context(false), display("{source}"))]
    Io { source: io::Error },
    #[snafu(display("could not launch dotnet {command} from {}: {source}", directory.display()))]
    Launch {
        command: String,
        directory: PathBuf,
        source: io::Error,
    },
    #[snafu(display("dotnet {command} exited with {status}\n{stdout}\n{stderr}"))]
    CommandFailed {
        command: String,
        status: std::process::ExitStatus,
        stdout: String,
        stderr: String,
    },
    #[snafu(display("dotnet {command} produced invalid UTF-8: {source}"))]
    InvalidOutput {
        command: String,
        source: std::string::FromUtf8Error,
    },
    #[snafu(display("SDK did not report a portable host runtime identifier"))]
    InvalidRid,
    #[snafu(display("oxel requires a .NET SDK 10 or newer for SDK projects"))]
    UnsupportedVersion,
    #[snafu(display("SDK did not resolve Microsoft.NETCore.App reference assemblies"))]
    NoFrameworkReferences,
    #[snafu(display("could not read NuGet lock file {}: {source}", path.display()))]
    ReadLock { path: PathBuf, source: io::Error },
    #[snafu(display("could not restore locked NuGet graph {}: {source}", path.display()))]
    RestoreLocked { path: PathBuf, source: Box<Error> },
    #[snafu(display("SDK changed NuGet lock file {} during locked restore", path.display()))]
    ChangedLock { path: PathBuf },
    #[snafu(display("SDK output does not contain the compiled oxel assembly"))]
    MissingAssembly,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum LockMode {
    IfPresent,
    Required,
    Refresh,
}

pub(super) struct SdkProject {
    stage: tempfile::TempDir,
    pub references: Vec<PathBuf>,
    pub framework_references: Vec<PathBuf>,
    pub lock: Option<PathBuf>,
    rid: Option<String>,
}

pub(super) enum Resolution {
    Installed(InstalledFramework),
    Framework(Vec<PathBuf>),
    Project(SdkProject),
}

impl Resolution {
    pub fn project(&self) -> Option<&SdkProject> {
        match self {
            Self::Project(project) => Some(project),
            _ => None,
        }
    }

    pub fn framework_references(&self) -> &[PathBuf] {
        match self {
            Self::Project(project) => &project.framework_references,
            Self::Framework(references) => references,
            Self::Installed(pack) => &pack.references,
        }
    }

    pub fn installed(&self) -> Option<&InstalledFramework> {
        match self {
            Self::Installed(pack) => Some(pack),
            _ => None,
        }
    }
}

pub(super) struct InstalledFramework {
    framework: String,
    runtime_version: String,
    references: Vec<PathBuf>,
}

impl InstalledFramework {
    pub fn runtime_config(&self) -> String {
        serde_json::json!({
            "runtimeOptions": {
                "tfm": self.framework,
                "framework": { "name": "Microsoft.NETCore.App", "version": self.runtime_version },
                "configProperties": {
                    "System.Reflection.Metadata.MetadataUpdater.IsSupported": false,
                    "System.Runtime.Serialization.EnableUnsafeBinaryFormatterSerialization": false
                }
            }
        })
        .to_string()
    }

    pub fn deps(&self, name: &str) -> String {
        let version = self
            .framework
            .strip_prefix("net")
            .expect("validated target framework");
        let target = format!(".NETCoreApp,Version=v{version}");
        let package = format!("{name}/1.0.0");
        let dll = format!("{name}.dll");
        serde_json::json!({
            "runtimeTarget": { "name": target, "signature": "" },
            "compilationOptions": {},
            "targets": { (target): { (package.as_str()): { "runtime": { (dll): {} } } } },
            "libraries": { (package): { "type": "project", "serviceable": false, "sha512": "" } }
        })
        .to_string()
    }
}

pub(super) fn installed_framework(manifest: &Path) -> Option<Resolution> {
    let project = manifest::directory(manifest);
    let project = std::path::absolute(if project.as_os_str().is_empty() {
        Path::new(".")
    } else {
        project
    })
    .ok()?;
    framework::installed_references(&project).map(Resolution::Installed)
}

/// Resolves installed targeting packs through the selected SDK without restoring packages.
/// The caller uses the normal restore path if the probe cannot resolve the framework.
pub(super) fn probe_framework(manifest: &Path, name: &str) -> SdkResult<Resolution> {
    if let Some(installed) = installed_framework(manifest) {
        return Ok(installed);
    }
    let project = manifest::directory(manifest);
    let project = std::path::absolute(if project.as_os_str().is_empty() {
        Path::new(".")
    } else {
        project
    })?;
    probe_framework_msbuild(&project, name).map(Resolution::Framework)
}

fn probe_framework_msbuild(project: &Path, name: &str) -> SdkResult<Vec<PathBuf>> {
    let stage = stage_project(project)?;
    let root = stage.path();
    // An intermediate global.json must not override the SDK selected from the
    // manifest directory. The restore path explicitly pins the staging SDK.
    for directory in root.ancestors().skip(1).take_while(|&path| path != project) {
        if directory.join("global.json").try_exists()? {
            return NoFrameworkReferencesSnafu.fail();
        }
    }
    let csproj = root.join("oxel.csproj");
    fs::write(
        &csproj,
        project_xml(
            name,
            "net$(NETCoreAppMaximumVersion)",
            false,
            &BTreeMap::new(),
            AssemblyKind::Executable,
        ),
    )?;
    let output = invoke(
        project,
        "msbuild",
        &[
            csproj.as_os_str(),
            "-nologo".as_ref(),
            "-target:OxelReferences".as_ref(),
            "-p:Configuration=Release".as_ref(),
            "-p:SkipResolvePackageAssets=true".as_ref(),
            "-p:OxelResolveFramework=ResolveTargetingPackAssets".as_ref(),
            "-getProperty:NETCoreSdkVersion,TargetFramework".as_ref(),
        ],
    )?;
    let output: serde_json::Value = serde_json::from_str(&output).map_err(io::Error::other)?;
    let properties = &output["Properties"];
    let (_, framework) = sdk_version(properties["NETCoreSdkVersion"].as_str().unwrap_or_default())?;
    if properties["TargetFramework"].as_str() != Some(&framework) {
        return NoFrameworkReferencesSnafu.fail();
    }
    framework_references(root)
}

fn stage_project(project: &Path) -> io::Result<tempfile::TempDir> {
    let artifacts = artifact_directory(project);
    fs::create_dir_all(&artifacts)?;
    let stage = tempfile::TempDir::with_prefix_in("nuget-", &artifacts)?;
    for name in [
        "Directory.Build.props",
        "Directory.Build.targets",
        "Directory.Packages.props",
    ] {
        fs::write(stage.path().join(name), "<Project />")?;
    }
    Ok(stage)
}

impl SdkProject {
    pub(super) fn restore(
        manifest: &Path,
        name: &str,
        packages: &BTreeMap<String, String>,
        native: bool,
        mode: LockMode,
        include_framework: bool,
        kind: AssemblyKind,
    ) -> SdkResult<Self> {
        let project = manifest::directory(manifest);
        let (sdk, framework) = version(project)?;
        // Keep the staging project beneath the manifest to pick up the normal
        // NuGet.Config hierarchy, including project-relative package feeds.
        let stage = stage_project(project)?;
        let root = stage.path();
        pin(root, &sdk)?;
        fs::write(
            root.join("oxel.csproj"),
            project_xml(name, &framework, native, packages, kind),
        )?;

        let rid = if native {
            let rid = invoke(
                root,
                "msbuild",
                &[
                    "oxel.csproj",
                    "-nologo",
                    "-getProperty:NETCoreSdkPortableRuntimeIdentifier",
                ],
            )?;
            let rid = rid.trim();
            if rid.is_empty()
                || !rid
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
            {
                return InvalidRidSnafu.fail();
            }
            Some(rid.to_owned())
        } else {
            None
        };
        let mut args = vec![
            "oxel.csproj".to_owned(),
            "-nologo".to_owned(),
            "-verbosity:minimal".to_owned(),
            "-restore".to_owned(),
            "-target:OxelReferences".to_owned(),
            "-p:Configuration=Release".to_owned(),
            "-p:NuGetLockFilePath=packages.lock.json".to_owned(),
        ];
        if let Some(rid) = &rid {
            args.push(format!("-p:RuntimeIdentifier={rid}"));
        }
        if include_framework {
            args.push("-p:OxelResolveFramework=ResolveTargetingPackAssets".into());
        }
        let lock = lock_path(manifest, &framework, rid.as_deref());
        let staged_lock = root.join("packages.lock.json");
        let previous = match mode {
            _ if packages.is_empty() => None,
            LockMode::Refresh => {
                args.push("-p:RestorePackagesWithLockFile=true".into());
                args.push("-p:RestoreForceEvaluate=true".into());
                args.push("-p:RestoreLockedMode=false".into());
                None
            }
            mode => match fs::read(&lock) {
                Ok(bytes) => {
                    fs::write(&staged_lock, &bytes)?;
                    args.push("-p:RestorePackagesWithLockFile=true".into());
                    args.push("-p:RestoreLockedMode=true".into());
                    args.push("-p:RestoreForceEvaluate=false".into());
                    Some(bytes)
                }
                Err(error)
                    if error.kind() == io::ErrorKind::NotFound && mode == LockMode::IfPresent =>
                {
                    None
                }
                Err(error) => return Err(error).context(ReadLockSnafu { path: lock }),
            },
        };
        let result = invoke(root, "msbuild", &args);
        if previous.is_some() {
            result
                .map_err(Box::new)
                .context(RestoreLockedSnafu { path: &lock })?;
        } else {
            result?;
        }
        let references = read_references(&root.join("oxel.references"), false)?;
        let framework_references = if include_framework {
            framework_references(root)?
        } else {
            Vec::new()
        };
        let lock = if !packages.is_empty() && mode == LockMode::Refresh {
            write_file(&lock, &fs::read(&staged_lock)?)?;
            Some(lock)
        } else if let Some(previous) = previous {
            if fs::read(&staged_lock)? != previous {
                return ChangedLockSnafu { path: lock }.fail();
            }
            Some(lock)
        } else {
            None
        };
        Ok(Self {
            stage,
            references,
            framework_references,
            lock,
            rid,
        })
    }

    pub(super) fn build(
        &self,
        name: &str,
        bytes: &[u8],
        dependencies: &[Dependency],
    ) -> SdkResult<PathBuf> {
        let root = self.stage.path();
        fs::write(root.join("input.dll"), bytes)?;
        write_dependencies(&root.join("references"), dependencies)?;
        let mut args = [
            "oxel.csproj",
            "--no-restore",
            "-c",
            "Release",
            "-o",
            "output",
            "--nologo",
            "-v",
            "minimal",
        ]
        .map(str::to_owned)
        .to_vec();
        let command = if let Some(rid) = &self.rid {
            args.extend(["-r".to_owned(), rid.clone()]);
            "publish"
        } else {
            "build"
        };
        invoke(root, command, &args)?;
        let output = root.join("output");
        if self.rid.is_none() && fs::read(output.join(format!("{name}.dll")))? != bytes {
            return MissingAssemblySnafu.fail();
        }
        Ok(output)
    }
}

fn framework_references(root: &Path) -> SdkResult<Vec<PathBuf>> {
    let references = read_references(&root.join("oxel.framework"), true)?;
    if references.is_empty() {
        return NoFrameworkReferencesSnafu.fail();
    }
    Ok(references)
}

fn read_references(path: &Path, required: bool) -> io::Result<Vec<PathBuf>> {
    let list = match fs::read_to_string(path) {
        Ok(list) => list,
        Err(error) if !required && error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut references: Vec<_> = list
        .trim_start_matches('\u{feff}')
        .lines()
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .collect();
    references.sort();
    references.dedup();
    Ok(references)
}

fn lock_path(manifest: &Path, framework: &str, rid: Option<&str>) -> PathBuf {
    let mut file = manifest
        .file_name()
        .expect("manifest has a file name")
        .to_os_string();
    file.push(format!(".nuget.{framework}"));
    if let Some(rid) = rid {
        file.push(format!(".{rid}.aot"));
    }
    file.push(".lock.json");
    manifest.with_file_name(file)
}

pub(super) fn store(project: &Path, output: &Path) -> io::Result<PathBuf> {
    store_directory(&artifact_directory(project).join("linked"), output)
}

pub(super) fn store_directory(root: &Path, output: &Path) -> io::Result<PathBuf> {
    let hash = folder_hash(output)?;
    let destination = root.join(hash.to_hex().as_str());
    fs::create_dir_all(root)?;
    match fs::rename(output, &destination) {
        Ok(()) => Ok(destination),
        Err(_) if destination.is_dir() && folder_hash(&destination)? == hash => Ok(destination),
        Err(error) => Err(error),
    }
}

fn folder_hash(root: &Path) -> io::Result<blake3::Hash> {
    fn walk(folder: &Path, root: &Path, hash: &mut blake3::Hasher) -> io::Result<()> {
        let mut entries = fs::read_dir(folder)?.collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(fs::DirEntry::file_name);
        for entry in entries {
            let path = entry.path();
            let name = path.strip_prefix(root).expect("SDK output within root");
            let name = name.as_os_str().as_encoded_bytes();
            hash.update(&(name.len() as u64).to_le_bytes());
            hash.update(name);
            let kind = entry.file_type()?;
            if kind.is_dir() {
                hash.update(b"D");
                walk(&path, root, hash)?;
            } else if kind.is_file() {
                hash.update(b"F");
                let file = fs::File::open(&path)?;
                hash.update(&file.metadata()?.len().to_le_bytes());
                hash.update_reader(file)?;
            } else {
                return Err(io::Error::other(format!(
                    "unexpected SDK output '{}'",
                    path.display()
                )));
            }
        }
        Ok(())
    }
    let mut hash = blake3::Hasher::new_derive_key("oxel SDK output");
    walk(root, root, &mut hash)?;
    Ok(hash.finalize())
}

fn invoke<T: AsRef<std::ffi::OsStr>>(
    directory: &Path,
    command: &str,
    args: &[T],
) -> SdkResult<String> {
    let output = Command::new("dotnet")
        .current_dir(directory)
        .arg(command)
        .args(args)
        .output()
        .context(LaunchSnafu { command, directory })?;
    if !output.status.success() {
        return CommandFailedSnafu {
            command,
            status: output.status,
            stdout: String::from_utf8_lossy(&output.stdout).trim(),
            stderr: String::from_utf8_lossy(&output.stderr).trim(),
        }
        .fail();
    }
    String::from_utf8(output.stdout).context(InvalidOutputSnafu { command })
}

pub(super) fn version(project: &Path) -> SdkResult<(String, String)> {
    let root = if project.as_os_str().is_empty() {
        Path::new(".")
    } else {
        project
    };
    let output = invoke(root, "--version", &[] as &[&str])?;
    sdk_version(output.trim())
}

fn sdk_version(version: &str) -> SdkResult<(String, String)> {
    let mut parts = version.split('.');
    let major = parts.next().and_then(|part| part.parse::<u32>().ok());
    let minor = parts.next().and_then(|part| part.parse::<u32>().ok());
    match (major, minor) {
        (Some(major), Some(minor))
            if major >= 10
                && parts.next().is_some()
                && version.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+')
                }) =>
        {
            Ok((version.into(), format!("net{major}.{minor}")))
        }
        _ => UnsupportedVersionSnafu.fail(),
    }
}

pub(super) fn pin(directory: &Path, version: &str) -> io::Result<()> {
    fs::write(
        directory.join("global.json"),
        format!(r#"{{"sdk":{{"version":"{version}","rollForward":"disable"}}}}"#),
    )
}

pub(super) fn project_xml(
    assembly: &str,
    framework: &str,
    native: bool,
    packages: &BTreeMap<String, String>,
    kind: AssemblyKind,
) -> String {
    use std::fmt::Write as _;
    let mut package_items = String::new();
    for (id, version) in packages {
        writeln!(
            package_items,
            r#"    <PackageReference Include="{id}" Version="[{version}]" />"#
        )
        .expect("write to string");
    }
    let assembly = msbuild_text(assembly);
    let output = match kind {
        AssemblyKind::Executable => "Exe",
        AssemblyKind::Library => "Library",
    };
    let mode = match (kind, native) {
        (AssemblyKind::Library, _) => {
            "<CopyLocalLockFileAssemblies>true</CopyLocalLockFileAssemblies>"
        }
        (AssemblyKind::Executable, true) => "<PublishAot>true</PublishAot>",
        (AssemblyKind::Executable, false) => "<UseAppHost>false</UseAppHost>",
    };
    format!(
        r#"<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <TargetFramework>{framework}</TargetFramework>
    <OutputType>{output}</OutputType>
    <AssemblyName>{assembly}</AssemblyName>
    {mode}
    <ProduceReferenceAssembly>false</ProduceReferenceAssembly>
    <DebugType>none</DebugType>
    <EnableDefaultCompileItems>false</EnableDefaultCompileItems>
    <SkipCompilerExecution>true</SkipCompilerExecution>
  </PropertyGroup>
  <ItemGroup>
    <Reference Include="references/*.dll" />
{package_items}  </ItemGroup>
  <Target Name="OxelReferences" DependsOnTargets="ResolvePackageAssets;$(OxelResolveFramework)">
    <WriteLinesToFile File="oxel.references" Lines="@(ResolvedCompileFileDefinitions)" Overwrite="true" Encoding="UTF-8" />
    <ItemGroup Condition="'$(OxelResolveFramework)' != ''">
      <OxelFrameworkReference Include="@(Reference)" Condition="'%(Reference.FrameworkReferenceName)' == 'Microsoft.NETCore.App'" />
    </ItemGroup>
    <WriteLinesToFile Condition="'$(OxelResolveFramework)' != ''" File="oxel.framework" Lines="@(OxelFrameworkReference)" Overwrite="true" Encoding="UTF-8" />
  </Target>
  <Target Name="UseOxelAssembly" AfterTargets="CoreCompile">
    <Copy SourceFiles="input.dll" DestinationFiles="@(IntermediateAssembly)" />
  </Target>
</Project>
"#
    )
}

fn msbuild_text(value: &str) -> String {
    use std::fmt::Write as _;
    let mut escaped = String::new();
    for ch in value.chars() {
        match ch {
            '%' | '$' | '@' | '\'' | '(' | ')' | ';' | '?' | '*' => {
                write!(escaped, "%{:02X}", ch as u32).expect("write to string");
            }
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requires_dotnet_sdk_10_or_newer() {
        for version in ["8.0.428", "9.0.100"] {
            assert!(matches!(
                sdk_version(version),
                Err(Error::UnsupportedVersion)
            ));
        }
        for (version, framework) in [("10.0.100", "net10.0"), ("11.0.100", "net11.0")] {
            assert_eq!(
                sdk_version(version).unwrap(),
                (version.into(), framework.into())
            );
        }
    }
}
