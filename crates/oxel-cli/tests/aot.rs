use std::{fs, process::Command};

#[test]
fn publish_checks_source_before_starting_dotnet() {
    let folder = tempfile::tempdir().unwrap();
    fs::write(
        folder.path().join("invalid.oxel"),
        "fn main() { unknown(); }",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_oxel"))
        .current_dir(folder.path())
        .env("PATH", "")
        .args(["publish", "invalid.oxel"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("invalid.oxel:1:13: error[E0203]"));
    assert!(!folder.path().join("target").exists());
}

#[test]
#[ignore = "requires the .NET SDK Native AOT and platform linker toolchain"]
fn publishes_and_runs_native_executable_with_managed_dependency() {
    let folder = tempfile::tempdir().unwrap();
    let project = folder.path().join("package");
    fs::create_dir(&project).unwrap();
    let reference_name = "oxel_native_app0";
    let reference = format!("{reference_name}.dll");
    let bytes = oxel::compile_project_with_kind(
        &["fn greeting() -> string { \"Hello, \" }"],
        reference_name,
        &oxel::References::default(),
        oxel::AssemblyKind::Library,
    )
    .unwrap();
    fs::write(project.join(&reference), bytes).unwrap();
    let app = "console & café.oxel";
    let types = "types.oxel";
    fs::write(
        project.join(types),
        "struct User { name: string } const STEP = step(2); comptime fn step(value: i32) -> i32 { value * 3 } enum Measure<T> { Count(T, bool), Empty } enum Result<T, E> { Ok(T), Err(E) }",
    )
    .unwrap();
    fs::write(
        project.join(app),
        r#"
        fn amount(value: Measure<i32>) -> i32 {
            match value { Count(n, true) => n, Count(_, false) => 0, Empty => 0 }
        }
        fn measured(value: Result<Measure<i32>, string>) -> Result<i32, string> {
            Result.Ok(amount(value?))
        }
        fn report(value: Result<i32, string>) {
            match value {
                Ok(n) => System.Console.WriteLine(n),
                Err(message) => System.Console.WriteLine(message),
            }
        }
        fn main(args: string[]) -> i32 {
            let user = User { name: "Alice" };
            System.Console.WriteLine(oxel_native_app0.Program.greeting() + user.name);
            let mut remaining = 5;
            let mut sum = 0;
            while remaining > 0 {
                sum = sum + STEP;
                remaining = remaining - 1;
            }
            report(measured(Result.Ok(Measure.Count(sum, true))));
            report(measured(Result.Err("no measurement")));
            System.Console.WriteLine(args[0]);
            42
        }
    "#,
    )
    .unwrap();
    fs::write(
        project.join("oxel.toml"),
        format!("[package]\nname = 'console & café'\nsources = ['{app}', '{types}']\nreferences = ['{reference}']"),
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_oxel"))
        .current_dir(folder.path())
        .args(["publish", "--manifest-path", "package/oxel.toml"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let relative = String::from_utf8(output.stdout).unwrap();
    let expected = format!("console & café{}", std::env::consts::EXE_SUFFIX);
    let binary = project.join("target/oxel/native").join(expected);
    assert_eq!(folder.path().join(relative.trim()), binary);
    assert_eq!(fs::read_dir(binary.parent().unwrap()).unwrap().count(), 1);

    let argument = "--native café";
    let result = Command::new(binary)
        .env("PATH", "")
        .env_remove("DOTNET_ROOT")
        .arg(argument)
        .output()
        .unwrap();
    assert_eq!(
        result.status.code(),
        Some(42),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        String::from_utf8(result.stdout)
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        ["Hello, Alice", "30", "no measurement", argument]
    );
}
