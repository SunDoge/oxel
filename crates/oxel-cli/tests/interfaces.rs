use std::{fs, path::Path, process::Command, sync::Arc};

use oxel::{AssemblyKind, ProjectCompiler, References, compile_project_with_kind};

const EXAMPLE: &str = include_str!("../../../examples/interfaces.oxel");

fn run(command: &mut Command) -> String {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{command:?}\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn cli(directory: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_oxel"));
    command.current_dir(directory);
    command
}

#[test]
fn dispatches_source_interfaces_through_values_and_generic_constraints() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("main.oxel"), EXAMPLE).unwrap();
    let report = run(cli(directory.path()).args(["run", "main.oxel"]));
    assert_eq!(
        report.lines().collect::<Vec<_>>(),
        ["30", "30", "42", "43", "oxel"]
    );
}

#[test]
fn a_struct_can_implement_clr_interfaces_and_participate_in_using() {
    let directory = tempfile::tempdir().unwrap();
    let project = directory.path();
    fs::write(
        project.join("main.oxel"),
        r#"
struct Resource { value: i32 }

interface AsyncRead {
    fn read() -> System.Threading.Tasks.Task<i32>;
}

impl AsyncRead for Resource {
    async fn read(self: Resource) -> i32 {
        System.Threading.Tasks.Task.Delay(1).await;
        self.value
    }
}

impl System.IDisposable for Resource {
    fn Dispose(self: Resource) {
        System.Console.WriteLine(self.value);
    }
}

impl System.IComparable<i32> for Resource {
    fn CompareTo(self: Resource, other: i32) -> i32 {
        self.value - other
    }
}

fn comparable<T>(resource: System.IComparable<T>) -> System.IComparable<T> {
    resource
}

async fn display(resource: Resource) {
    let readers = new System.Collections.Generic.List<AsyncRead>();
    readers.Add(resource);
    System.Console.WriteLine(readers[0].read().await);
}

fn exercise(value: Resource) {
    using resource = value {
        System.Console.WriteLine(resource.CompareTo(10));
        System.Console.WriteLine(comparable(resource).CompareTo(20));
    }
}

async fn main() {
    let value = Resource { value: 30 };
    exercise(value);
    display(value).await;
}
"#,
    )
    .unwrap();
    let report = run(cli(project).args(["run", "--framework", "main.oxel"]));
    assert_eq!(report.lines().collect::<Vec<_>>(), ["20", "10", "30", "30"]);
}

#[test]
fn csharp_implements_and_calls_oxel_interfaces_with_clr_generic_constraints() {
    let directory = tempfile::tempdir().unwrap();
    let project = directory.path();
    let source = EXAMPLE.split("fn main()").next().unwrap();
    let source = format!("namespace Models;\n{source}");
    let assembly = compile_project_with_kind(
        &[&source],
        "Example",
        &References::default(),
        AssemblyKind::Library,
    )
    .unwrap();
    fs::write(project.join("Example.dll"), assembly).unwrap();
    let version = run(Command::new("dotnet").arg("--version"));
    let major = version.split('.').next().unwrap();
    fs::write(project.join("Consumer.csproj"), format!(r#"
<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup><OutputType>Exe</OutputType><TargetFramework>net{major}.0</TargetFramework><Nullable>enable</Nullable></PropertyGroup>
  <ItemGroup><Reference Include="Example" HintPath="Example.dll" /></ItemGroup>
</Project>
"#)).unwrap();
    fs::write(
        project.join("Consumer.cs"),
        r#"
using Example.Models;

Read<string> cell = new Cell<string> { value = "oxel" };
System.Console.WriteLine(cell.read());
System.Console.WriteLine(Example.Models.Program.offset(new Counter { value = 30 }, 12));
System.Console.WriteLine(Example.Models.Program.read<int>(new ManagedReader()));

sealed class ManagedReader : Read<int>
{
    public int read() => 50;
    public int offset(int by) => read() + by;
}
"#,
    )
    .unwrap();
    let output = run(Command::new("dotnet").current_dir(project).arg("run"));
    assert!(
        output
            .lines()
            .collect::<Vec<_>>()
            .ends_with(&["oxel", "42", "50"]),
        "{output}"
    );
}

#[test]
fn rejects_invalid_interface_implementations_and_bounds() {
    let base = "interface Read { fn read() -> i32; } struct Counter { value: i32, }";
    for (tail, error) in [
        (
            "impl Read for Counter {} fn main() {}",
            "missing implementation",
        ),
        (
            "impl Read for Counter { fn read(self: Counter) -> bool { true } } fn main() {}",
            "match the interface signature",
        ),
        (
            "impl Read for Counter { fn read() -> i32 { 1 } } fn main() {}",
            "first 'self",
        ),
        (
            "impl Read for Counter { fn read(self: Counter) -> i32 { 1 } } impl Read for Counter {} fn main() {}",
            "overlapping",
        ),
        (
            "fn check<T: Read>(value: T) {} fn main() { check(Counter { value: 1 }); }",
            "does not satisfy the bound",
        ),
        (
            "fn main() { let value = Read {}; }",
            "interfaces cannot be constructed",
        ),
    ] {
        let source = format!("{base} {tail}");
        let errors = oxel::check(&source).unwrap_err();
        assert!(errors[0].message.contains(error), "{errors:?}");
    }
}

#[test]
fn editing_an_implementation_body_reuses_other_checks_and_emits_the_same_bytes() {
    let references = References::default();
    let mut compiler = ProjectCompiler::new(AssemblyKind::Executable, &references);
    compiler.compile(&[Arc::from(EXAMPLE)], "example").unwrap();
    let edited = EXAMPLE.replace("self.value + by", "self.value + by + 5");
    let incremental = compiler
        .compile(&[Arc::from(edited.as_str())], "example")
        .unwrap();
    assert_eq!(compiler.checked_functions(), 1);
    assert_eq!(incremental, oxel::compile(&edited, "example").unwrap());
}
