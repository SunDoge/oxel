use std::{
    fs,
    path::{Path, PathBuf},
};

use roxmltree::Document;
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
    if [
        "input.dll",
        "intermediateassembly",
        "targetpath",
        "targetdir",
    ]
    .iter()
    .any(|value| text.to_ascii_lowercase().contains(value))
    {
        return false;
    }
    let root = document.root_element();
    root.has_tag_name("Project")
        && root
            .children()
            .filter(roxmltree::Node::is_element)
            .all(|node| match node.tag_name().name() {
                "PropertyGroup" | "ItemGroup" | "ItemDefinitionGroup" => true,
                "Target" => node
                    .children()
                    .filter(roxmltree::Node::is_element)
                    .all(|task| {
                        matches!(task.tag_name().name(), "Error" | "Warning" | "Message")
                            && !task.children().any(|child| child.is_element())
                    }),
                _ => false,
            })
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
}
