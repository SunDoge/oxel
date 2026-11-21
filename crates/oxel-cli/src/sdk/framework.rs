//! Read the installed SDK's default framework pack without starting MSBuild.

use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

use roxmltree::Document;

use super::{
    ASP_NET_CORE_NAME, CORE_NAME, Framework, InstalledFramework, artifact_directory, invoke,
    sdk_version,
};

pub(super) fn installed_references(
    project: &Path,
    selection: Framework,
) -> Option<InstalledFramework> {
    // Leave SDK policies, response files, and custom imports to MSBuild.
    let physical_project = fs::canonicalize(project).ok()?;
    let artifacts = artifact_directory(project);
    if env::vars_os().any(|(name, _)| {
        let name = name.to_string_lossy().to_ascii_lowercase();
        name.starts_with("msbuild")
            || name.starts_with("dotnet_msbuild")
            || name.starts_with("custombefore")
            || name.starts_with("customafter")
            || name.starts_with("directorybuild")
            || name.starts_with("import")
            || matches!(
                name.as_str(),
                "netcoretargetingpackroot"
                    | "targetframework"
                    | "netcoreappmaximumversion"
                    | "disableimplicitframeworkreferences"
                    | "runtimeframeworkversion"
            )
    }) || artifacts
        .ancestors()
        .chain(physical_project.ancestors())
        .any(|path| {
            exists_or_inaccessible(&path.join("global.json"))
                || exists_or_inaccessible(&path.join("Directory.Build.rsp"))
        })
        || artifacts
            .ancestors()
            .take_while(|&path| path != project)
            .any(redirected)
    {
        return None;
    }

    let listing = invoke(project, "--list-sdks", &[] as &[&str]).ok()?;
    let mut lines = listing.lines().filter(|line| !line.trim().is_empty());
    let (version, base) = lines.next()?.trim().split_once(" [")?;
    if lines.next().is_some() || !stable_version(version) {
        return None;
    }
    let (_, framework) = sdk_version(version).ok()?;
    let base = Path::new(base.strip_suffix(']')?);
    if !base.is_absolute() || base.file_name()? != "sdk" {
        return None;
    }
    let sdk = base.join(version);
    if exists_or_inaccessible(&sdk.join("MSBuild.rsp")) {
        return None;
    }
    let root = base.parent()?;
    let props = fs::read_to_string(sdk.join("Microsoft.NETCoreSdk.BundledVersions.props")).ok()?;
    let doc = Document::parse(&props).ok()?;
    let (pack, runtime_version) = targeting_pack(&doc, root, &framework, CORE_NAME)?;
    let mut references = reference_files(&pack, &framework, CORE_NAME)?;
    let aspnet_runtime_version = if selection == Framework::AspNetCore {
        let (pack, version) = targeting_pack(&doc, root, &framework, ASP_NET_CORE_NAME)?;
        references.extend(reference_files(&pack, &framework, ASP_NET_CORE_NAME)?);
        references.sort_unstable();
        Some(version)
    } else {
        None
    };
    Some(InstalledFramework {
        framework,
        runtime_version,
        aspnet_runtime_version,
        references,
    })
}

fn exists_or_inaccessible(path: &Path) -> bool {
    path.try_exists().unwrap_or(true)
}

fn redirected(path: &Path) -> bool {
    match fs::symlink_metadata(path) {
        Ok(metadata) => !metadata.is_dir(),
        Err(error) => error.kind() != io::ErrorKind::NotFound,
    }
}

fn stable_version(version: &str) -> bool {
    version.split('.').count() == 3
        && version
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

fn targeting_pack(
    doc: &Document<'_>,
    root: &Path,
    framework: &str,
    name: &str,
) -> Option<(PathBuf, String)> {
    let pack_name = format!("{name}.Ref");
    let mut matches = doc.descendants().filter(|node| {
        node.has_tag_name("KnownFrameworkReference")
            && node.attribute("Include") == Some(name)
            && node.attribute("TargetFramework") == Some(framework)
    });
    let entry = matches.next()?;
    let group = entry.parent()?;
    if matches.next().is_some()
        || entry.attribute("Condition").is_some()
        || !group.has_tag_name("ItemGroup")
        || group.attribute("Condition").is_some()
        || group.parent() != Some(doc.root_element())
        || !doc.root_element().has_tag_name("Project")
        || entry.attribute("TargetingPackName") != Some(pack_name.as_str())
    {
        return None;
    }
    let version = entry.attribute("TargetingPackVersion")?;
    let runtime = entry.attribute("DefaultRuntimeFrameworkVersion")?;
    if !stable_version(version) || !stable_version(runtime) {
        return None;
    }
    Some((
        root.join("packs").join(pack_name).join(version),
        runtime.into(),
    ))
}

fn reference_files(pack: &Path, framework: &str, framework_name: &str) -> Option<Vec<PathBuf>> {
    let list = fs::read_to_string(pack.join("data/FrameworkList.xml")).ok()?;
    let doc = Document::parse(&list).ok()?;
    let root = doc.root_element();
    if !root.has_tag_name("FileList")
        || root.attribute("FrameworkName") != Some(framework_name)
        || root.attribute("TargetFrameworkVersion") != framework.strip_prefix("net")
    {
        return None;
    }
    let directory = pack.join("ref").join(framework);
    let mut references = Vec::new();
    for entry in root.children().filter(|node| node.is_element()) {
        if !entry.has_tag_name("File") {
            return None;
        }
        match entry.attribute("Type")? {
            "Analyzer" => continue,
            "Managed"
                if entry.attribute("Profile").is_none()
                    && entry.attribute("Language").is_none() => {}
            _ => return None,
        }
        let path = entry.attribute("Path")?;
        let (relative, name) = path.rsplit_once('/')?;
        if relative.strip_prefix("ref/") != Some(framework)
            || !name.ends_with(".dll")
            || name.contains(['\\', ':'])
        {
            return None;
        }
        let path = directory.join(name);
        if !path.is_file() {
            return None;
        }
        references.push(path);
    }
    references.sort_unstable();
    if references.windows(2).any(|pair| pair[0] == pair[1])
        || !references.iter().any(|path| {
            path.ends_with(match framework_name {
                CORE_NAME => "System.Runtime.dll",
                _ => "Microsoft.AspNetCore.dll",
            })
        })
    {
        return None;
    }
    Some(references)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_only_the_framework_pack_and_assemblies_listed_by_the_sdk() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        let sdk = root.join("sdk/8.0.100");
        fs::create_dir_all(&sdk).unwrap();
        fs::write(
            sdk.join("Microsoft.NETCoreSdk.BundledVersions.props"),
            r#"<Project><ItemGroup>
                <KnownFrameworkReference Include="Microsoft.NETCore.App" TargetFramework="net7.0"
                    TargetingPackName="Microsoft.NETCore.App.Ref" TargetingPackVersion="7.0.9"
                    DefaultRuntimeFrameworkVersion="7.0.0" />
                <KnownFrameworkReference Include="Microsoft.NETCore.App" TargetFramework="net8.0"
                    TargetingPackName="Microsoft.NETCore.App.Ref" TargetingPackVersion="8.0.1"
                    DefaultRuntimeFrameworkVersion="8.0.0" />
            </ItemGroup></Project>"#,
        )
        .unwrap();
        let pack = root.join("packs/Microsoft.NETCore.App.Ref/8.0.1");
        fs::create_dir_all(root.join("packs/Microsoft.NETCore.App.Ref/8.0.2")).unwrap();
        let props =
            fs::read_to_string(sdk.join("Microsoft.NETCoreSdk.BundledVersions.props")).unwrap();
        assert_eq!(
            targeting_pack(&Document::parse(&props).unwrap(), root, "net8.0", CORE_NAME),
            Some((pack.clone(), "8.0.0".into()))
        );
        let data = pack.join("data");
        let references = pack.join("ref/net8.0");
        fs::create_dir_all(&data).unwrap();
        fs::create_dir_all(&references).unwrap();
        let list = data.join("FrameworkList.xml");
        fs::write(
            &list,
            r#"<FileList FrameworkName="Microsoft.NETCore.App" TargetFrameworkVersion="8.0">
                <File Type="Analyzer" Path="analyzers/ignored.dll" Language="cs" />
                <File Type="Managed" Path="ref/net8.0/System.Runtime.dll" />
                <File Type="Managed" Path="ref/net8.0/System.Console.dll" />
            </FileList>"#,
        )
        .unwrap();
        let runtime = references.join("System.Runtime.dll");
        let console = references.join("System.Console.dll");
        for path in [&runtime, &console, &references.join("Extra.dll")] {
            fs::write(path, []).unwrap();
        }
        assert_eq!(
            reference_files(&pack, "net8.0", CORE_NAME),
            Some(vec![console, runtime.clone()])
        );
        fs::remove_file(&runtime).unwrap();
        assert!(reference_files(&pack, "net8.0", CORE_NAME).is_none());
    }

    #[test]
    fn installed_fast_path_matches_msbuild_when_available() {
        let project = tempfile::tempdir().unwrap();
        for selection in [Framework::Core, Framework::AspNetCore] {
            if let Some(references) = installed_references(project.path(), selection) {
                let msbuild =
                    super::super::probe_framework_msbuild(project.path(), "comparison", selection)
                        .unwrap();
                assert_eq!(references.references, msbuild);
            }
        }
    }
}
