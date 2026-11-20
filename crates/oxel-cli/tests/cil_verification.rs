use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use oxel::{AssemblyKind, References, compile_test_project};

#[test]
#[ignore = "requires the .NET SDK and Microsoft's dotnet-ilverify tool"]
fn verifies_managed_assemblies_with_ilverify() {
    let folder = tempfile::tempdir().unwrap();
    let directory = folder.path();
    let runtimes = success(
        Command::new("dotnet")
            .current_dir(directory)
            .arg("--list-runtimes"),
    );
    let runtime = runtimes
        .lines()
        .rev()
        .find_map(|line| {
            let (version, path) = line
                .strip_prefix("Microsoft.NETCore.App ")?
                .split_once(" [")?;
            Some(PathBuf::from(path.strip_suffix(']')?).join(version))
        })
        .expect("installed Microsoft.NETCore.App runtime");
    let verifier = env::var_os("OXEL_ILVERIFY").unwrap_or_else(|| "ilverify".into());
    let verify = |assembly: &Path| {
        success(
            Command::new(&verifier)
                .current_dir(directory)
                .arg(assembly)
                .args(["-s", "System.Private.CoreLib", "-r"])
                .arg(runtime.join("*.dll")),
        );
    };

    let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    for (name, framework) in [
        ("hello.oxel", false),
        ("result_main.oxel", false),
        ("async_main.oxel", true),
        ("generic_structs.oxel", false),
        ("option_try.oxel", false),
        ("optional_casts.oxel", false),
        ("delegate_calls.oxel", true),
        ("generic_lambdas.oxel", true),
        ("alias_interfaces.oxel", true),
        ("mutable_captures.oxel", true),
        ("byref_arguments.oxel", true),
        ("catch.oxel", true),
        ("using.oxel", true),
    ] {
        let mut compile = Command::new(env!("CARGO_BIN_EXE_oxel"));
        compile.current_dir(directory).arg("compile");
        if framework {
            compile.arg("--framework");
        }
        let assembly = success(compile.arg(examples.join(name)));
        verify(Path::new(assembly.trim()));
    }

    let source = include_str!("../../../examples/unit_tests.oxel");
    let ignored = "#[test] #[ignore] fn skipped() { assert(false); }";
    let assembly = directory.join("test_runner.dll");
    let bytes = compile_test_project(
        &[source, ignored],
        "test_runner",
        &References::default(),
        AssemblyKind::Executable,
    )
    .unwrap();
    fs::write(&assembly, bytes).unwrap();
    verify(&assembly);
}

fn success(command: &mut Command) -> String {
    let Output {
        status,
        stdout,
        stderr,
    } = command
        .output()
        .unwrap_or_else(|error| panic!("could not start {command:?}: {error}"));
    assert!(
        status.success(),
        "{command:?} exited with {status}\n{}\n{}",
        String::from_utf8_lossy(&stdout),
        String::from_utf8_lossy(&stderr)
    );
    String::from_utf8(stdout).unwrap()
}
