//! Compare managed builds and execution using the same installed SDK and runtime.

use std::{
    ffi::OsString,
    fmt::Write,
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use serde_json::Value;

use super::{BODY_STEPS, WORKLOAD_FUNCTIONS, body_workload, completed, measure, output};

pub(super) fn compare_interpolation(oxel: &str, directory: &Path) {
    const ITERATIONS: f64 = 1_000_000.0;
    let sdk = Roslyn::new(directory);
    let source = include_str!("interpolation.oxel");
    fs::write(directory.join("interpolation.oxel"), source).unwrap();
    fs::write(
        directory.join("interpolation.cs"),
        include_str!("interpolation.cs"),
    )
    .unwrap();
    completed(&mut sdk.compile(
        "interpolation.cs",
        "interpolation-csharp.dll",
        [
            sdk.reference("System.Runtime.dll"),
            sdk.reference("System.Console.dll"),
        ],
    ));
    let compile = |cli: &Path, path: &Path| {
        let artifact = output(Command::new(cli).current_dir(path).args([
            "compile",
            "--framework",
            "interpolation.oxel",
        ]));
        path.join(std::str::from_utf8(&artifact).unwrap().trim())
    };
    let current = compile(Path::new(oxel), directory);
    let config = current.with_extension("runtimeconfig.json");
    let mut workloads = vec![
        ("Oxel", current),
        ("C#", directory.join("interpolation-csharp.dll")),
    ];
    if let Some(previous) = std::env::var_os("OXEL_BENCH_PREVIOUS_CLI") {
        let previous = PathBuf::from(previous).canonicalize().unwrap();
        let path = directory.join("previous");
        fs::create_dir(&path).unwrap();
        fs::write(path.join("interpolation.oxel"), source).unwrap();
        workloads.insert(0, ("Oxel previous", compile(&previous, &path)));
    }
    let mut samples: Vec<_> = workloads.iter().map(|_| Vec::new()).collect();
    for _ in 0..7 {
        for ((_, image), samples) in workloads.iter().zip(&mut samples) {
            let stdout = output(&mut managed_exec(directory, &config, image));
            let values: Vec<u64> = std::str::from_utf8(&stdout)
                .unwrap()
                .lines()
                .map(|line| line.parse().unwrap())
                .collect();
            let &[checksum, ticks, frequency, bytes] = values.as_slice() else {
                panic!("expected checksum, elapsed ticks, timer frequency, and allocated bytes");
            };
            assert_eq!(checksum, 6_000_000);
            samples.push((ticks as f64 * 1e9 / frequency as f64, bytes as f64));
        }
    }
    for ((label, _), samples) in workloads.iter().zip(&mut samples) {
        samples.sort_by(|a, b| a.0.total_cmp(&b.0));
        let (ns, bytes) = samples[samples.len() / 2];
        println!(
            "{label:<14} {:>6.2} ns/op {:>6.2} B/op",
            ns / ITERATIONS,
            bytes / ITERATIONS
        );
    }
}

pub(super) fn compare_csharp(oxel: &str, directory: &Path) {
    let sdk = Roslyn::new(directory);
    fs::write(directory.join("numeric.oxel"), body_workload(false)).unwrap();
    fs::write(directory.join("numeric.cs"), numeric_csharp()).unwrap();
    let runtime = sdk.reference("System.Runtime.dll");
    let console = sdk.reference("System.Console.dll");
    measure("numeric / oxel CLI", || {
        completed(
            Command::new(oxel)
                .current_dir(directory)
                .env("OXEL_JOBS", "1")
                .args(["compile", "numeric.oxel"]),
        );
    });
    measure("numeric / Roslyn CLI", || {
        completed(&mut sdk.compile("numeric.cs", "numeric-csharp.dll", [runtime]));
    });

    fs::write(
        directory.join("hello.cs"),
        "static class Program { static int Add(int a, int b) => a + b; static void Main() { System.Console.WriteLine(Add(10, 20)); } }",
    ).unwrap();
    completed(&mut sdk.compile("hello.cs", "hello-csharp.dll", [runtime, console]));

    fs::write(directory.join("kernel.cs"), include_str!("kernel.cs")).unwrap();
    fs::write(directory.join("kernel.oxel"), include_str!("kernel.oxel")).unwrap();
    completed(&mut sdk.compile("kernel.cs", "kernel-csharp.dll", [runtime, console]));
    let artifact = output(Command::new(oxel).current_dir(directory).args([
        "compile",
        "--framework",
        "kernel.oxel",
    ]));
    let oxel_kernel = directory.join(std::str::from_utf8(&artifact).unwrap().trim());
    let runtime_config = oxel_kernel.with_extension("runtimeconfig.json");
    let startup = [
        (
            "CoreCLR hello / oxel",
            directory.join("target/oxel/hello.dll"),
        ),
        ("CoreCLR hello / C#", directory.join("hello-csharp.dll")),
    ];
    for (label, image) in startup {
        let run = || output(&mut managed_exec(directory, &runtime_config, &image));
        assert_eq!(std::str::from_utf8(&run()).unwrap().trim(), "30");
        measure(label, || {
            run();
        });
    }

    let csharp_kernel = directory.join("kernel-csharp.dll");
    let mut oxel_times = Vec::new();
    let mut csharp_times = Vec::new();
    for _ in 0..5 {
        let oxel = kernel_sample(&mut managed_exec(directory, &runtime_config, &oxel_kernel));
        let csharp = kernel_sample(&mut managed_exec(
            directory,
            &runtime_config,
            &csharp_kernel,
        ));
        assert_eq!(
            oxel.0, csharp.0,
            "equivalent kernels must produce the same checksum"
        );
        oxel_times.push(oxel.1);
        csharp_times.push(csharp.1);
    }
    for (label, mut times) in [
        ("warm kernel / oxel", oxel_times),
        ("warm kernel / C#", csharp_times),
    ] {
        times.sort_unstable();
        let median = times[times.len() / 2];
        println!(
            "{label:<25} {median:>10.2?} median ({} processes)",
            times.len()
        );
    }
}

struct Roslyn<'a> {
    directory: &'a Path,
    compiler: PathBuf,
    references: Vec<PathBuf>,
}

impl<'a> Roslyn<'a> {
    fn new(directory: &'a Path) -> Self {
        fs::write(
            directory.join("References.csproj"),
            r#"
            <Project Sdk="Microsoft.NET.Sdk">
              <PropertyGroup>
                <TargetFramework>net$(NETCoreAppMaximumVersion)</TargetFramework>
                <EnableDefaultCompileItems>false</EnableDefaultCompileItems>
              </PropertyGroup>
            </Project>
        "#,
        )
        .unwrap();
        completed(Command::new("dotnet").current_dir(directory).args([
            "restore",
            "References.csproj",
            "-p:NuGetAudit=false",
        ]));
        let properties = output(Command::new("dotnet").current_dir(directory).args([
            "msbuild",
            "References.csproj",
            "-nologo",
            "-target:ResolveReferences",
            "-getProperty:MSBuildToolsPath,TargetFramework",
            "-getItem:ReferencePath",
        ]));
        let info: Value = serde_json::from_slice(&properties).unwrap();
        let tools = Path::new(info["Properties"]["MSBuildToolsPath"].as_str().unwrap());
        let compiler = tools.join("Roslyn/bincore/csc.dll");
        let references = info["Items"]["ReferencePath"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| PathBuf::from(item["FullPath"].as_str().unwrap()))
            .collect();
        let version = output(
            Command::new("dotnet")
                .current_dir(directory)
                .args(["exec"])
                .arg(&compiler)
                .arg("-version"),
        );
        println!(
            "Roslyn {} targeting {}",
            std::str::from_utf8(&version).unwrap().trim(),
            info["Properties"]["TargetFramework"].as_str().unwrap()
        );
        Self {
            directory,
            compiler,
            references,
        }
    }

    fn reference(&self, name: &str) -> &Path {
        self.references
            .iter()
            .find(|path| path.ends_with(name))
            .unwrap_or_else(|| panic!("missing SDK reference assembly {name}"))
    }

    fn compile<'path>(
        &self,
        source: &str,
        destination: &str,
        references: impl IntoIterator<Item = &'path Path>,
    ) -> Command {
        let mut command = Command::new("dotnet");
        command
            .current_dir(self.directory)
            .arg("exec")
            .arg(&self.compiler)
            .args([
                "-nologo",
                "-noconfig",
                "-nostdlib+",
                "-optimize+",
                "-deterministic+",
                "-parallel-",
                "-target:exe",
            ])
            .arg(format!("-out:{destination}"));
        for reference in references {
            let mut argument = OsString::from("-reference:");
            argument.push(reference);
            command.arg(argument);
        }
        command.arg(source);
        command
    }
}

fn numeric_csharp() -> String {
    let mut source = String::from("static class Program {\n");
    for index in 0..WORKLOAD_FUNCTIONS {
        writeln!(source, "static int work{index}(int x) {{ int n = x;").unwrap();
        for step in 0..BODY_STEPS {
            writeln!(
                source,
                "  if (n > {step}) {{ n += x * 2; }} else {{ n -= x; }}"
            )
            .unwrap();
        }
        source.push_str("  return n;\n}\n");
    }
    source.push_str("static void Main() {}\n}\n");
    source
}

fn managed_exec(directory: &Path, config: &Path, image: &Path) -> Command {
    let mut command = Command::new("dotnet");
    command
        .current_dir(directory)
        .env("DOTNET_TieredCompilation", "0")
        .args(["exec", "--runtimeconfig"])
        .arg(config)
        .arg(image);
    command
}

fn kernel_sample(command: &mut Command) -> (i32, Duration) {
    let stdout = output(command);
    let mut lines = std::str::from_utf8(&stdout).unwrap().lines();
    let checksum = lines.next().unwrap().parse().unwrap();
    let ticks: u64 = lines.next().unwrap().parse().unwrap();
    let frequency: u64 = lines.next().unwrap().parse().unwrap();
    assert!(ticks > 0 && frequency > 0 && lines.next().is_none());
    (
        checksum,
        Duration::from_secs_f64(ticks as f64 / frequency as f64),
    )
}
