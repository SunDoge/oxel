use std::{fs, path::Path, process::Command};

const MANIFEST: &str = include_str!("../../../examples/onnx_inference/oxel.toml");
const LOCK: &str =
    include_str!("../../../examples/onnx_inference/oxel.toml.nuget.net10.0.lock.json");
const EXPECTED: &str = "3\n5\n7\n-3\n1\n9\n";

fn run(directory: &Path, executable: &str, args: &[&str]) -> String {
    let output = Command::new(executable)
        .current_dir(directory)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{executable} {args:?}\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .unwrap()
        .replace("\r\n", "\n")
}

#[test]
#[ignore = "requires the ONNX Runtime CPU package from NuGet"]
fn onnx_cpu_inference_matches_csharp_and_runs_from_a_nuget_package() {
    let folder = tempfile::tempdir().unwrap();
    let project = folder.path();
    fs::write(project.join("oxel.toml"), MANIFEST).unwrap();
    fs::write(project.join("oxel.toml.nuget.net10.0.lock.json"), LOCK).unwrap();
    fs::write(
        project.join("main.oxel"),
        include_str!("../../../examples/onnx_inference/main.oxel"),
    )
    .unwrap();
    fs::write(
        project.join("inference.oxel"),
        include_str!("../../../examples/onnx_inference/inference.oxel"),
    )
    .unwrap();
    fs::write(
        project.join("affine.onnx"),
        include_bytes!("../../../examples/onnx_inference/affine.onnx"),
    )
    .unwrap();

    let oxel = env!("CARGO_BIN_EXE_oxel");
    let direct = run(project, oxel, &["run", "--locked", "--", "affine.onnx"]);
    assert_eq!(direct, EXPECTED);

    fs::write(
        project.join("Reference.csproj"),
        include_str!("../../../examples/onnx_inference/reference/Reference.csproj"),
    )
    .unwrap();
    let csharp = include_str!("../../../examples/onnx_inference/reference/Program.cs");
    fs::write(project.join("Program.cs"), csharp).unwrap();
    run(
        project,
        "dotnet",
        &[
            "build",
            "Reference.csproj",
            "-p:NuGetAudit=false",
            "-o",
            "baseline",
        ],
    );
    assert_eq!(
        run(
            project,
            "dotnet",
            &["baseline/Reference.dll", "affine.onnx"]
        ),
        direct
    );

    let package_id = "Oxel.Examples.Onnx";
    let suffix = project
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .trim_start_matches('.');
    let version = format!("0.1.0-test.{suffix}");
    let library = MANIFEST
        .replace(
            "name = \"onnx-inference\"",
            &format!("name = \"{package_id}\"\nkind = \"library\""),
        )
        .replace(
            "sources = [\"main.oxel\", \"inference.oxel\"]",
            "sources = [\"inference.oxel\"]",
        );
    fs::write(project.join("library.toml"), library).unwrap();
    fs::write(project.join("library.toml.nuget.net10.0.lock.json"), LOCK).unwrap();
    let artifact = run(
        project,
        oxel,
        &["compile", "--locked", "--manifest-path", "library.toml"],
    );
    // Include the Oxel DLL and declare the ONNX Runtime dependency so NuGet
    // selects its managed and native assets for the consuming application.
    let dll = format!("{package_id}.dll");
    fs::copy(project.join(artifact.trim()), project.join(&dll)).unwrap();
    fs::write(
        project.join("Inference.nuspec"),
        format!(
            r#"<package>
  <metadata>
    <id>{package_id}</id>
    <version>{version}</version>
    <authors>Oxel</authors>
    <description>ONNX inference integration fixture</description>
    <dependencies><group targetFramework="net10.0">
      <dependency id="Microsoft.ML.OnnxRuntime" version="[1.23.2]" />
    </group></dependencies>
  </metadata>
  <files><file src="{dll}" target="lib/net10.0/" /></files>
</package>"#
        ),
    )
    .unwrap();
    run(
        project,
        "dotnet",
        &["pack", "Inference.nuspec", "-o", "feed"],
    );

    let consumer = project.join("consumer");
    fs::create_dir(&consumer).unwrap();
    fs::write(
        consumer.join("Consumer.csproj"),
        format!(
            r#"<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <OutputType>Exe</OutputType>
    <TargetFramework>net10.0</TargetFramework>
    <NuGetAudit>false</NuGetAudit>
    <RestoreAdditionalProjectSources>../feed</RestoreAdditionalProjectSources>
  </PropertyGroup>
  <ItemGroup><PackageReference Include="{package_id}" Version="{version}" /></ItemGroup>
</Project>"#
        ),
    )
    .unwrap();
    fs::write(
        consumer.join("Program.cs"),
        r#"using System;
using Microsoft.ML.OnnxRuntime;

using var session = new InferenceSession(args[0]);
using var options = new RunOptions();
foreach (var input in new float[][] { [1, 2, 3], [-2, 0, 4] })
{
    foreach (var value in Oxel.Examples.Onnx.Program.predict(session, options, input))
        Console.WriteLine(value);
}
"#,
    )
    .unwrap();
    run(
        &consumer,
        "dotnet",
        &["build", "Consumer.csproj", "-o", "out"],
    );
    assert_eq!(
        run(&consumer, "dotnet", &["out/Consumer.dll", "../affine.onnx"]),
        direct
    );
}
