use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

use roxmltree::{Document, Node};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct References {
    pub packages: Vec<PathBuf>,
    pub framework: Vec<PathBuf>,
    pub build_files: Vec<PathBuf>,
}

pub(super) fn read(
    stage: &Path,
    packages: Vec<PathBuf>,
    framework: Vec<PathBuf>,
) -> Option<References> {
    let assets: Value =
        serde_json::from_slice(&fs::read(stage.join("obj/project.assets.json")).ok()?).ok()?;
    let folders = assets["packageFolders"].as_object()?;
    let mut build_files = Vec::new();
    for target in assets["targets"].as_object()?.values() {
        for (package, assets_for_package) in target.as_object()? {
            for (section, files) in assets_for_package.as_object()? {
                if !section.starts_with("build") {
                    continue;
                }
                let package_path = assets["libraries"][package]["path"].as_str()?;
                for file in files.as_object()?.keys() {
                    if Path::new(file).file_name()?.to_str()? == "_._" {
                        continue;
                    }
                    let mut matches = folders
                        .keys()
                        .map(|root| Path::new(root).join(package_path).join(file))
                        .filter(|path| path.is_file());
                    let path = matches.next()?;
                    if matches.next().is_some()
                        || !reusable_build_file(&fs::read_to_string(&path).ok()?)
                    {
                        return None;
                    }
                    build_files.push(path);
                }
            }
        }
    }
    build_files.sort_unstable();
    build_files.dedup();
    Some(References {
        packages,
        framework,
        build_files,
    })
}

fn reusable_build_file(text: &str) -> bool {
    // Dependency-only props and prerequisite checks may already be staged.
    // Keep running MSBuild for imports and arbitrary tasks that can examine or
    // transform the application assembly, or emit additional outputs.
    let Ok(document) = Document::parse(text.trim_start_matches('\u{feff}')) else {
        return false;
    };
    let lower = text.to_ascii_lowercase();
    if [
        "input.dll",
        "intermediateassembly",
        "targetpath",
        "targetdir",
    ]
    .iter()
    .any(|value| lower.contains(value))
    {
        return false;
    }
    let root = document.root_element();
    let targets: Vec<_> = root
        .children()
        .filter(|node| node.has_tag_name("Target"))
        .collect();
    let names: HashSet<_> = targets
        .iter()
        .filter_map(|node| node.attribute("Name"))
        .collect();
    let aliases: HashSet<_> = targets
        .iter()
        .flat_map(|target| {
            target
                .children()
                .filter(|node| node.has_tag_name("ItemGroup"))
        })
        .flat_map(|group| group.children().filter(Node::is_element))
        .filter(|&item| analyzer_alias(item))
        .map(|item| item.tag_name().name())
        .collect();
    root.has_tag_name("Project")
        && root
            .children()
            .filter(roxmltree::Node::is_element)
            .all(|node| match node.tag_name().name() {
                "PropertyGroup" | "ItemGroup" | "ItemDefinitionGroup" => true,
                "Target" => {
                    target_dependencies(node, &names)
                        && (node.children().filter(Node::is_element).all(|task| {
                            matches!(task.tag_name().name(), "Error" | "Warning" | "Message")
                                && !task.children().any(|child| child.is_element())
                        }) || root
                            .attributes()
                            .all(|attribute| attribute.name() == "ToolsVersion")
                            && analyzer_target(node, &aliases))
                }
                _ => false,
            })
}

fn analyzer_alias(item: Node<'_, '_>) -> bool {
    let name = item.tag_name().name();
    name.starts_with('_')
        && (name.ends_with("Analyzer") || name.ends_with("Analyzers"))
        && item.attribute("Include") == Some("@(Analyzer)")
}

fn target_dependencies(target: Node<'_, '_>, names: &HashSet<&str>) -> bool {
    target
        .attribute("DependsOnTargets")
        .is_none_or(|value| value.split(';').all(|name| names.contains(name.trim())))
}

fn analyzer_target(target: Node<'_, '_>, aliases: &HashSet<&str>) -> bool {
    // Oxel supplies the already-compiled assembly to the SDK and never runs
    // Roslyn analyzers. Recognize only item-list changes, with prerequisites
    // validated alongside the target in the same build file.
    target.attributes().all(|attribute| {
        let value = attribute.value();
        match attribute.name() {
            "Name" | "BeforeTargets" | "AfterTargets" => !value.contains(['$', '@', '%']),
            "DependsOnTargets" => true,
            "Condition" => analyzer_condition(value, aliases),
            _ => false,
        }
    }) && target.children().filter(Node::is_element).all(|group| {
        group.has_tag_name("ItemGroup")
            && group.attributes().all(|attribute| {
                attribute.name() == "Condition" && analyzer_condition(attribute.value(), aliases)
            })
            && group.children().filter(Node::is_element).all(|item| {
                let alias = analyzer_alias(item);
                let removes_analyzer = item.has_tag_name("Analyzer")
                    && item.attribute("Remove").is_some_and(|value| {
                        value
                            .strip_prefix("@(")
                            .and_then(|name| name.strip_suffix(')'))
                            .is_some_and(|name| name == "Analyzer" || aliases.contains(name))
                    });
                (alias || removes_analyzer)
                    && item.attributes().all(|attribute| match attribute.name() {
                        "Include" => alias,
                        "Remove" => removes_analyzer,
                        "Condition" => analyzer_condition(attribute.value(), aliases),
                        _ => false,
                    })
                    && !item.children().any(|child| child.is_element())
            })
    })
}

fn analyzer_condition(value: &str, aliases: &HashSet<&str>) -> bool {
    // Recognize the framework packages' static check for a Roslyn version
    // component in an analyzer path. Other static property functions can
    // perform I/O during a condition and require the regular SDK path.
    if let Some((item, suffix)) = value
        .trim()
        .strip_prefix("$([System.String]::Copy('%(")
        .and_then(|value| value.split_once(".Identity)').IndexOf('"))
        && let Some((literal, test)) = suffix.split_once("'))")
    {
        return (item == "Analyzer" || aliases.contains(item))
            && literal
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
            && test.trim() == ">= 0";
    }
    !value.contains(['[', ']'])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distinguishes_prerequisites_from_assembly_packaging_tasks() {
        assert!(reusable_build_file(
            "<Project><Target Name='Prerequisite'><Error Condition='false'/></Target></Project>"
        ));
        assert!(!reusable_build_file(
            "<Project><Target Name='Package'><Copy SourceFiles='file'/></Target></Project>"
        ));
        assert!(!reusable_build_file(
            "<Project><Import Project='another.targets'/></Project>"
        ));
    }

    #[test]
    fn reuses_assets_when_roslyn_targets_only_filter_analyzers() {
        let fixture = r#"
<Project>
  <Target Name="Gather">
    <ItemGroup>
      <_PackageAnalyzers Include="@(Analyzer)" Condition="'%(Analyzer.NuGetPackageId)' == 'Example'" />
    </ItemGroup>
  </Target>
  <Target Name="Filter" AfterTargets="ResolvePackageDependenciesForBuild" DependsOnTargets="Gather">
    <ItemGroup>
      <Analyzer Remove="@(_PackageAnalyzers)"
                Condition="$([System.String]::Copy('%(_PackageAnalyzers.Identity)').IndexOf('roslyn4')) &gt;= 0" />
    </ItemGroup>
  </Target>
</Project>
"#;
        assert!(reusable_build_file(fixture));
        for (original, replacement) in [
            (
                "DependsOnTargets=\"Gather\"",
                "DependsOnTargets=\"Gather;External\"",
            ),
            (
                "</Project>",
                "<Target Name=\"Diagnostic\" DependsOnTargets=\"External\"><Message /></Target></Project>",
            ),
            ("<Analyzer Remove", "<Content Remove"),
            ("@(_PackageAnalyzers)", "@(_UnknownAnalyzers)"),
            ("<Analyzer Remove", "<Analyzer Update"),
            ("[System.String]", "[System.IO.File]"),
            (
                "</ItemGroup>",
                "</ItemGroup><WriteLinesToFile File=\"side-effect.txt\" />",
            ),
            (
                "<Target Name=\"Gather\"",
                "<Target Outputs=\"@(Analyzer)\" Name=\"Gather\"",
            ),
        ] {
            assert!(
                !reusable_build_file(&fixture.replace(original, replacement)),
                "accepted {replacement}"
            );
        }
    }
}
