use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn fmt(directory: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_oxel"))
        .current_dir(directory)
        .env("PATH", "")
        .arg("fmt")
        .args(args)
        .output()
        .unwrap()
}

fn successful(output: Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn formats_a_file_and_checks_without_writing() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("main.oxel");
    let input = "fn main ( ) {let a=2;System . Console . WriteLine(a+ 3);}\r\n";
    fs::write(&path, input).unwrap();

    let check = fmt(folder.path(), &["--check", "main.oxel"]);
    assert!(!check.status.success());
    let stderr = String::from_utf8(check.stderr).unwrap();
    assert!(stderr.contains("main.oxel:1:1: error[E0013]"), "{stderr}");
    assert_eq!(fs::read_to_string(&path).unwrap(), input);

    successful(fmt(folder.path(), &["main.oxel"]));
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "fn main() {\n    let a = 2;\n    System.Console.WriteLine(a + 3);\n}\n"
    );
    successful(fmt(folder.path(), &["--check", "main.oxel"]));
}

#[test]
fn formats_manifest_sources_and_tests_without_restoring_dependencies() {
    let folder = tempfile::tempdir().unwrap();
    let project = folder.path().join("project");
    fs::create_dir(&project).unwrap();
    fs::write(
        project.join("oxel.toml"),
        r#"[package]
name = "format_example"
sources = ["main.oxel"]
test_sources = ["tests.oxel"]
framework = true

[nuget]
UninstalledReference = "1.0.0"
"#,
    )
    .unwrap();
    fs::write(project.join("main.oxel"), "fn main(){}").unwrap();
    fs::write(
        project.join("tests.oxel"),
        "#[test]fn verify(){assert(true);}",
    )
    .unwrap();
    successful(fmt(
        folder.path(),
        &["--manifest-path", "project/oxel.toml"],
    ));
    assert_eq!(
        fs::read_to_string(project.join("main.oxel")).unwrap(),
        "fn main() {}\n"
    );
    assert_eq!(
        fs::read_to_string(project.join("tests.oxel")).unwrap(),
        "#[test]\nfn verify() {\n    assert(true);\n}\n"
    );
    successful(fmt(&project, &["--check"]));
}

#[test]
fn a_syntax_error_leaves_all_inputs_untouched() {
    let folder = tempfile::tempdir().unwrap();
    let valid = "fn main(){}";
    let invalid = "fn broken(\n";
    fs::write(folder.path().join("main.oxel"), valid).unwrap();
    fs::write(folder.path().join("broken.oxel"), invalid).unwrap();
    let result = fmt(
        folder.path(),
        &["--diagnostic-format", "json", "main.oxel", "broken.oxel"],
    );
    assert!(!result.status.success());
    let diagnostic: serde_json::Value = serde_json::from_slice(&result.stderr).unwrap();
    assert_eq!(diagnostic["file"], "broken.oxel");
    assert_eq!(diagnostic["code"], "E0102");
    assert_eq!(
        fs::read_to_string(folder.path().join("main.oxel")).unwrap(),
        valid
    );
    assert_eq!(
        fs::read_to_string(folder.path().join("broken.oxel")).unwrap(),
        invalid
    );
}

#[cfg(unix)]
#[test]
fn preserves_symlinks_and_source_permissions() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("main.oxel");
    let link = folder.path().join("link.oxel");
    fs::write(&path, "fn main(){}").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    symlink("main.oxel", &link).unwrap();

    successful(fmt(folder.path(), &["link.oxel", "main.oxel"]));
    assert!(fs::symlink_metadata(link).unwrap().is_symlink());
    assert_eq!(fs::read_to_string(&path).unwrap(), "fn main() {}\n");
    assert_eq!(
        fs::metadata(path).unwrap().permissions().mode() & 0o777,
        0o640
    );
}
