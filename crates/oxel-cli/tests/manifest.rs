use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use serde_json::Value;

fn cli(directory: &Path, args: &[&str], without_runtime: bool) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_oxel"));
    command.current_dir(directory).args(args);
    if without_runtime {
        command.env("PATH", "");
    }
    command.output().unwrap()
}

fn successful(output: Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn test_runs_separate_manifest_test_sources_with_framework_references() {
    let folder = tempfile::tempdir().unwrap();
    let project = folder.path();
    fs::write(
        project.join("oxel.toml"),
        "[package]\nname = 'Samples.Suite'\nkind = 'library'\nframework = true\nsources = ['library.oxel']\ntest_sources = ['specs/parse.oxel']\n",
    )
    .unwrap();
    fs::write(
        project.join("library.oxel"),
        "internal fn parse(text: string) -> i32 { System.Int32.Parse(text) }",
    )
    .unwrap();
    fs::create_dir(project.join("specs")).unwrap();
    fs::write(
        project.join("specs/parse.oxel"),
        r#"
            #[test]
            fn parses_an_integer() {
                assert(parse("30") == 30);
            }
        "#,
    )
    .unwrap();
    assert_eq!(
        successful(cli(project, &["test", "--list"], false)),
        "parses_an_integer: test\n",
    );
    assert_eq!(
        successful(cli(project, &["test"], false)),
        "ok parses_an_integer\n1 test, 0 failed\n",
    );
    successful(cli(project, &["check", "--tests"], false));
    fs::write(
        project.join("specs/parse.oxel"),
        "#[test] fn broken() { missing(); }",
    )
    .unwrap();
    successful(cli(project, &["compile"], false));
    for args in [&["test"][..], &["check", "--tests"], &["test", "--list"]] {
        let output = cli(project, args, false);
        assert!(!output.status.success());
        let errors = String::from_utf8(output.stderr).unwrap();
        assert!(
            errors.contains("parse.oxel:1:") && errors.contains("E0203"),
            "{errors}"
        );
    }
}

#[test]
fn test_listing_filters_in_source_order_without_loading_coreclr() {
    let folder = tempfile::tempdir().unwrap();
    let project = folder.path();
    fs::write(
        project.join("oxel.toml"),
        "[package]\nname = 'Suite'\nkind = 'library'\nsources = ['library.oxel']\ntest_sources = ['suite.oxel']",
    )
    .unwrap();
    fs::write(project.join("library.oxel"), "#[test] fn inline_test() {}").unwrap();
    let suite = project.join("suite.oxel");
    fs::write(
        &suite,
        "#[test] #[ignore] fn service_test() { assert(false); } #[test] fn sample() {}",
    )
    .unwrap();
    assert_eq!(
        successful(cli(project, &["test", "--list"], true)),
        "inline_test: test\nservice_test: test (ignored)\nsample: test\n",
    );
    assert_eq!(
        successful(cli(project, &["test", "--list", "--filter=service"], true)),
        "service_test: test (ignored)\n",
    );
    assert!(successful(cli(project, &["test", "--list", "--filter=SERVICE"], true)).is_empty());
    assert!(!project.join("target").exists());

    fs::write(&suite, "#[test] #[ignore] fn broken() { missing(); }").unwrap();
    let rejected = cli(project, &["test", "--list", "--filter=inline"], true);
    assert!(!rejected.status.success());
    assert!(rejected.stdout.is_empty());
    let error = String::from_utf8(rejected.stderr).unwrap();
    assert!(
        error.contains("suite.oxel:1:") && error.contains("E0203"),
        "{error}"
    );
}

#[test]
fn check_tests_needs_neither_main_nor_coreclr() {
    let folder = tempfile::tempdir().unwrap();
    let project = folder.path();
    fs::write(
        project.join("suite.oxel"),
        "#[test] fn sample() { assert(true); }",
    )
    .unwrap();
    assert_eq!(
        successful(cli(project, &["check", "--tests", "suite.oxel"], true)),
        "",
    );
    assert!(!project.join("target").exists());

    let ordinary = cli(project, &["check", "suite.oxel"], true);
    assert!(!ordinary.status.success());
    assert!(
        String::from_utf8(ordinary.stderr)
            .unwrap()
            .contains("E0206")
    );
}

#[test]
fn manifest_drives_check_compile_and_run_from_another_directory() {
    let folder = tempfile::tempdir().unwrap();
    let package = folder.path().join("my project");
    fs::create_dir_all(package.join("src")).unwrap();
    fs::create_dir_all(package.join("lib")).unwrap();
    let source = "fn main() { System.Console.WriteLine(answer()); }";
    let helper = "fn answer() -> i32 { Program.twice(Program.extra()) }";
    fs::write(package.join("src/start.oxel"), source).unwrap();
    fs::write(package.join("src/calc.oxel"), helper).unwrap();
    let first = oxel::compile("fn main() {} fn twice(n: i32) -> i32 { n * 2 }", "MathLib").unwrap();
    let extra = oxel::compile("fn main() {} fn extra() -> i32 { 15 }", "Extra").unwrap();
    fs::write(package.join("lib/math.dll"), &first).unwrap();
    fs::write(folder.path().join("extra.dll"), &extra).unwrap();
    fs::write(
        package.join("oxel.toml"),
        r#"
        [package]
        name = "configured app"
        sources = [
            "src/start.oxel", # order is explicit
            'src/calc.oxel',
        ]
        references = ["lib/math.dll"]
    "#,
    )
    .unwrap();

    let selected = [
        "--manifest-path",
        "my project/oxel.toml",
        "--reference",
        "extra.dll",
    ];
    let invoke = |command, without_runtime| {
        cli(
            folder.path(),
            &[&[command][..], &selected].concat(),
            without_runtime,
        )
    };
    assert_eq!(successful(invoke("check", true)), "");
    assert!(!package.join("target").exists());
    let built = successful(invoke("compile", true));
    let artifact = folder.path().join(built.trim());
    assert_eq!(artifact.file_name().unwrap(), "configured app.dll");
    assert!(artifact.starts_with(package.join("target/oxel/linked")));
    assert!(artifact.with_extension("runtimeconfig.json").is_file());
    assert_eq!(
        fs::read(artifact.with_file_name("MathLib.dll")).unwrap(),
        first
    );
    assert_eq!(
        fs::read(artifact.with_file_name("Extra.dll")).unwrap(),
        extra
    );
    assert!(!folder.path().join("target").exists());
    assert_eq!(successful(invoke("run", false)).trim(), "30");
    let mut references = oxel::References::default();
    references.add(&first).unwrap();
    references.add(&extra).unwrap();
    assert_eq!(
        fs::read(&artifact).unwrap(),
        oxel::compile_project(&[source, helper], "configured app", &references).unwrap()
    );

    let implicit = successful(cli(
        &package,
        &["compile", "--reference", "../extra.dll"],
        true,
    ));
    assert_eq!(package.join(implicit.trim()), artifact);
}

#[test]
fn manifest_errors_keep_the_relevant_file_and_span() {
    let folder = tempfile::tempdir().unwrap();
    let manifest = folder.path().join("oxel.toml");
    fs::write(folder.path().join("main.oxel"), "fn main() {}").unwrap();
    for (text, offending, message) in [
        ("[package]\nname = 'x'\nsources = [", None, "array"),
        (
            "[package]\nname = 'x'\nsources = []",
            Some("[]"),
            "at least one",
        ),
        (
            "[package]\nname = '../x'\nsources = ['main.oxel']",
            Some("'../x'"),
            "invalid package",
        ),
        (
            "[package]\nname = 'x'\nkind = 'binary'\nsources = ['main.oxel']",
            Some("'binary'"),
            "package.kind",
        ),
        (
            "[package]\nname = 'invalid name'\nkind = 'library'\nsources = ['main.oxel']",
            Some("'invalid name'"),
            "dotted CLR namespace",
        ),
        (
            "[package]\nname = 'x'\nsources = ['']",
            Some("''"),
            "must not be empty",
        ),
        (
            "[package]\nname = 'x'\nsources = ['main.oxel', './main.oxel']",
            Some("'./main.oxel'"),
            "duplicate source",
        ),
        (
            "[package]\nname = 'x'\nsources = ['main.oxel']\ntest_sources = ['./main.oxel']",
            Some("'./main.oxel'"),
            "duplicate test source",
        ),
        (
            "[package]\nname = 'x'\nsources = ['main.oxel']\nreferenes = []",
            Some("referenes"),
            "unknown field",
        ),
        (
            "[package]\nname = 'x'\nsources = ['main.oxel']\n[nuget]\nAny = '1.*'",
            Some("'1.*'"),
            "exact package version",
        ),
        (
            "[package]\nname = 'x'\nsources = ['main.oxel']\n[nuget]\n'bad;id' = '1.0'",
            Some("'1.0'"),
            "invalid NuGet package ID",
        ),
    ] {
        fs::write(&manifest, text).unwrap();
        let output = cli(folder.path(), &["check", "--diagnostic-format=json"], true);
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        let error: Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(error["code"], "E0008");
        assert_eq!(error["file"], "oxel.toml");
        assert!(
            error["message"].as_str().unwrap().contains(message),
            "{error}"
        );
        if let Some(offending) = offending {
            assert_eq!(
                error["range"]["start"],
                text.find(offending).unwrap(),
                "{error}"
            );
        }
    }
    fs::write(
        &manifest,
        "[package]\nname = 'x'\nsources = ['main.oxel', 'other.oxel']",
    )
    .unwrap();
    fs::write(
        folder.path().join("other.oxel"),
        "fn read() -> i32 { missing }",
    )
    .unwrap();
    let output = cli(folder.path(), &["check", "--diagnostic-format=json"], true);
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["code"], "E0203");
    assert_eq!(
        PathBuf::from(error["file"].as_str().unwrap()),
        Path::new("other.oxel")
    );

    let conflict = cli(
        folder.path(),
        &["check", "--manifest-path", "oxel.toml", "main.oxel"],
        true,
    );
    assert_eq!(conflict.status.code(), Some(2));
    let direct = cli(folder.path(), &["check", "main.oxel"], true);
    assert!(direct.status.success());
    let linked = oxel::compile("fn main() {}", "Named").unwrap();
    fs::write(folder.path().join("linked.dll"), linked).unwrap();
    fs::write(
        &manifest,
        "[package]\nname = 'named'\nsources = ['main.oxel']\nreferences = ['linked.dll']",
    )
    .unwrap();
    let output = cli(folder.path(), &["check", "--diagnostic-format=json"], true);
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["code"], "E0008");
    assert_eq!(error["file"], "oxel.toml");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("conflicts with CLR reference")
    );
    assert!(!folder.path().join("target").exists());
}

#[test]
fn managed_library_projects_are_reusable_from_csharp_and_oxel() {
    let folder = tempfile::tempdir().unwrap();
    let root = folder.path();
    let manifest = include_str!("../../../examples/math_library/oxel.toml");
    let source = include_str!("../../../examples/math_library/math.oxel");
    fs::write(root.join("oxel.toml"), manifest).unwrap();
    fs::write(root.join("math.oxel"), source).unwrap();

    assert_eq!(successful(cli(root, &["check"], true)), "");
    let executable_check = cli(root, &["check", "math.oxel"], true);
    assert!(!executable_check.status.success());
    assert!(String::from_utf8_lossy(&executable_check.stderr).contains("E0206"));
    for args in [&["run"][..], &["publish"], &["restore", "--native"]] {
        let output = cli(root, args, true);
        assert_eq!(output.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&output.stderr).contains("executable host"));
    }

    let compiled = successful(cli(root, &["compile"], true));
    let artifact = root.join(compiled.trim());
    assert_eq!(artifact, root.join("target/oxel/lib/Samples.Math.dll"));
    assert!(!artifact.with_extension("runtimeconfig.json").exists());
    let bytes = fs::read(&artifact).unwrap();
    let references = oxel::References::default();
    assert_eq!(
        bytes,
        oxel::compile_project_with_kind(
            &[source],
            "Samples.Math",
            &references,
            oxel::AssemblyKind::Library,
        )
        .unwrap()
    );
    assert_eq!(successful(cli(root, &["compile"], true)), compiled);

    let extra = "fn main(value: i32) -> bool { value == 30 }";
    let other = root.join("Samples.Check.dll");
    fs::write(
        &other,
        oxel::compile_project_with_kind(
            &[extra],
            "Samples.Check",
            &references,
            oxel::AssemblyKind::Library,
        )
        .unwrap(),
    )
    .unwrap();
    fs::write(
        root.join("app.oxel"),
        r#"
        fn main() {
            let result = Samples.Math.Program.sample(Samples.Math.Program.add(10, 20));
            System.Console.WriteLine(result.value);
            System.Console.WriteLine(Samples.Check.Program.main(result.value));
        }
        "#,
    )
    .unwrap();
    let app = successful(cli(
        root,
        &[
            "run",
            "app.oxel",
            "--reference",
            compiled.trim(),
            "--reference",
            "Samples.Check.dll",
        ],
        false,
    ));
    assert_eq!(app.lines().collect::<Vec<_>>(), ["30", "True"]);

    for source in [
        "fn main() { Samples.Math.Program.evaluate(1); }",
        "fn read(value: Samples.Math.Operands) {} fn main() {}",
        "fn read(value: Samples.Math.Operation) {} fn main() {}",
    ] {
        fs::write(root.join("blocked.oxel"), source).unwrap();
        let blocked = cli(
            root,
            &["check", "blocked.oxel", "--reference", compiled.trim()],
            true,
        );
        assert!(!blocked.status.success());
        let error = String::from_utf8(blocked.stderr).unwrap();
        assert!(
            error.contains("E0202") || error.contains("E0203"),
            "{error}"
        );
    }

    let sdk = successful(Command::new("dotnet").arg("--version").output().unwrap());
    let major = sdk.split('.').next().unwrap();
    fs::write(
        root.join("Consumer.csproj"),
        format!(
            r#"<Project Sdk="Microsoft.NET.Sdk">
            <PropertyGroup><OutputType>Exe</OutputType><TargetFramework>net{major}.0</TargetFramework></PropertyGroup>
            <ItemGroup>
                <Reference Include="Samples.Math" HintPath="target/oxel/lib/Samples.Math.dll" />
                <Reference Include="Samples.Check" HintPath="Samples.Check.dll" />
            </ItemGroup>
            </Project>"#
        ),
    )
    .unwrap();
    fs::write(
        root.join("Consumer.cs"),
        r#"
        using System;
        using Math = Samples.Math.Program;
        using Check = Samples.Check.Program;
        Samples.Math.Sample<int> result = Math.sample(Math.add(10, 20));
        Console.WriteLine(result.value);
        Console.WriteLine(Check.main(result.value));
        Console.WriteLine(typeof(Math).Assembly.EntryPoint is null && typeof(Check).Assembly.EntryPoint is null);
        var hiddenType = typeof(Math).Assembly.GetType("Samples.Math.Operands")!;
        var helper = typeof(Math).GetMethod("evaluate", System.Reflection.BindingFlags.NonPublic | System.Reflection.BindingFlags.Static)!;
        Console.WriteLine(hiddenType.IsNotPublic && helper.IsAssembly);
        "#,
    )
    .unwrap();
    let host = successful(
        Command::new("dotnet")
            .current_dir(root)
            .args(["run", "--project", "Consumer.csproj"])
            .output()
            .unwrap(),
    );
    assert_eq!(
        host.lines().collect::<Vec<_>>(),
        ["30", "True", "True", "True"]
    );
}

#[test]
fn native_library_contents_version_the_complete_managed_bundle() {
    let folder = tempfile::tempdir().unwrap();
    let root = folder.path();
    let project = root.join("project");
    fs::create_dir_all(project.join("assets")).unwrap();
    fs::write(project.join("main.oxel"), "fn main() {}").unwrap();
    let manifest = project.join("oxel.toml");
    let text = "[package]\nname = 'bundled'\nsources = ['main.oxel']\nnative_libraries = ['assets/library.so.1']";
    fs::write(&manifest, text).unwrap();
    let invoke = |action| {
        cli(
            root,
            &[action, "--manifest-path", "project/oxel.toml"],
            true,
        )
    };

    assert!(invoke("check").status.success());
    let missing = invoke("compile");
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("library.so.1"));
    let linked = project.join("target/oxel/linked");
    assert_eq!(fs::read_dir(&linked).unwrap().count(), 0);

    let input = project.join("assets/library.so.1");
    fs::write(&input, "first").unwrap();
    let first = root.join(successful(invoke("compile")).trim());
    assert!(first.starts_with(&linked));
    assert_eq!(
        fs::read_to_string(first.with_file_name("library.so.1")).unwrap(),
        "first"
    );
    assert_eq!(root.join(successful(invoke("compile")).trim()), first);

    fs::write(&input, "second").unwrap();
    let second = root.join(successful(invoke("compile")).trim());
    assert_ne!(first, second);
    assert_eq!(fs::read(&first).unwrap(), fs::read(&second).unwrap());
    assert_eq!(
        fs::read_to_string(first.with_file_name("library.so.1")).unwrap(),
        "first"
    );
    assert_eq!(
        fs::read_to_string(second.with_file_name("library.so.1")).unwrap(),
        "second"
    );
    assert_eq!(fs::read_dir(&linked).unwrap().count(), 2);

    fs::write(project.join("assets/BUNDLED.dll"), "conflict").unwrap();
    fs::write(
        &manifest,
        text.replace("assets/library.so.1", "assets/BUNDLED.dll"),
    )
    .unwrap();
    let collision = invoke("compile");
    assert!(!collision.status.success());
    assert!(String::from_utf8_lossy(&collision.stderr).contains("conflicts with output asset"));
    assert_eq!(fs::read_dir(&linked).unwrap().count(), 2);
}

#[test]
fn rejects_duplicate_native_library_output_names_in_manifest() {
    let folder = tempfile::tempdir().unwrap();
    let source = "[package]\nname = 'bundled'\nsources = ['main.oxel']\nnative_libraries = ['first/libmath.so', 'second/LIBMATH.SO']";
    fs::write(folder.path().join("oxel.toml"), source).unwrap();
    let output = cli(folder.path(), &["check", "--diagnostic-format=json"], true);
    assert!(!output.status.success());
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["code"], "E0008");
    assert_eq!(error["range"]["start"], source.find("'second/").unwrap());
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("duplicate native library filename")
    );
}
