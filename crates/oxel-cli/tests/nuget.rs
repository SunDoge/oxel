use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

fn success(output: Output) -> String {
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

struct Fixture {
    workspace: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        Self::with_leaf("1.0.0", "[1.0.0]")
    }

    fn with_leaf(leaf_version: &str, leaf_range: &str) -> Self {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path();
        for path in ["feed", "leaf", "facade", "app"] {
            fs::create_dir(root.join(path)).unwrap();
        }
        fs::write(
            root.join("NuGet.Config"),
            r#"<configuration>
          <packageSources><clear/><add key="fixture" value="feed"/></packageSources>
        </configuration>"#,
        )
        .unwrap();
        let version = success(Command::new("dotnet").arg("--version").output().unwrap());
        let prefix: Vec<_> = version.trim().split('.').take(2).collect();
        let framework = format!("net{}", prefix.join("."));
        let suffix = root
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .trim_start_matches('.');
        let leaf = format!("Oxel.Tests.Leaf.{suffix}");
        let facade = format!("Oxel.Tests.Facade.{suffix}");
        let project = |id, extra: &str| {
            format!(
                r#"<Project Sdk="Microsoft.NET.Sdk">
          <PropertyGroup>
            <TargetFramework>{framework}</TargetFramework>
            <PackageId>{id}</PackageId>
            <Version>1.0.0</Version>
            <AssemblyVersion>1.0.0.0</AssemblyVersion>
            <NuGetAudit>false</NuGetAudit>
          </PropertyGroup>
          {extra}
        </Project>"#
            )
        };
        fs::write(root.join("leaf/Leaf.csproj"), project(&leaf, "")).unwrap();
        let fixture = Self { workspace };
        fixture.pack_leaf(leaf_version, 15);
        let root = fixture.workspace.path();
        fs::write(
            root.join("facade/Facade.csproj"),
            project(&facade, &format!(r#"<ItemGroup>
              <PackageReference Include="{leaf}" Version="{leaf_range}" />
              <None Include="obj/Release/{framework}/ref/Facade.dll" Pack="true" PackagePath="ref/{framework}/Facade.dll" />
            </ItemGroup>"#)),
        ).unwrap();
        fs::write(root.join("facade/Facade.cs"), "namespace Fixture; public static class Facade { public static int Number() => Leaf.Number() * 2; }").unwrap();
        fixture.pack("facade", "Facade.csproj", &[]);
        fs::write(
            root.join("app/oxel.toml"),
            format!("[package]\nname = 'nuget & demo'\nsources = ['main.oxel']\nreferences = ['support.dll']\n[nuget]\n'{facade}' = '1.0.0'\n"),
        ).unwrap();
        fs::write(
            root.join("app/main.oxel"),
            r#"
            fn main(args: string[]) {
                System.Console.WriteLine(Program.subtract(Fixture.Facade.Number(), Fixture.Leaf.Number()) + 15);
                for arg in args { System.Console.WriteLine(arg); }
            }
            "#,
        )
        .unwrap();
        let support = oxel::compile(
            "fn main() {} fn subtract(a: i32, b: i32) -> i32 { a - b }",
            "Support",
        )
        .unwrap();
        fs::write(root.join("app/support.dll"), support).unwrap();
        fixture
    }

    fn pack_leaf(&self, version: &str, value: i32) {
        fs::write(
            self.workspace.path().join("leaf/Leaf.cs"),
            format!("namespace Fixture; public static class Leaf {{ public static int Number() => {value}; }}"),
        ).unwrap();
        self.pack("leaf", "Leaf.csproj", &[&format!("-p:Version={version}")]);
    }

    fn pack(&self, directory: &str, project: &str, args: &[&str]) {
        let root = self.workspace.path();
        success(
            Command::new("dotnet")
                .current_dir(root.join(directory))
                .env("NUGET_PACKAGES", root.join("packages"))
                .args([
                    "pack", project, "-c", "Release", "-o", "../feed", "--nologo",
                ])
                .args(args)
                .output()
                .unwrap(),
        );
    }

    fn oxel(&self, action: &str, toolchain_cache: Option<&Path>) -> Output {
        self.with_flags(action, &[], toolchain_cache)
    }

    fn with_flags(&self, action: &str, flags: &[&str], toolchain_cache: Option<&Path>) -> Output {
        let root = self.workspace.path();
        let mut command = Command::new(env!("CARGO_BIN_EXE_oxel"));
        command
            .current_dir(root)
            .args([
                action,
                "--manifest-path",
                "app/oxel.toml",
                "--diagnostic-format=json",
            ])
            .args(flags);
        command.env("NUGET_PACKAGES", root.join("packages"));
        if let Some(cache) = toolchain_cache {
            command.env("NUGET_FALLBACK_PACKAGES", cache);
        }
        command.output().unwrap()
    }

    fn artifact(&self, output: Output) -> PathBuf {
        self.workspace.path().join(success(output).trim())
    }
}

#[test]
fn sdk_resolves_nuget_references_and_packages_runtime_implementations() {
    let fixture = Fixture::new();
    assert_eq!(success(fixture.oxel("check", None)), "");
    let artifact = fixture.artifact(fixture.oxel("compile", None));
    assert_eq!(artifact.file_name().unwrap(), "nuget & demo.dll");
    assert!(artifact.with_extension("deps.json").is_file());
    assert!(artifact.with_extension("runtimeconfig.json").is_file());
    assert!(artifact.with_file_name("Facade.dll").is_file());
    assert!(artifact.with_file_name("Leaf.dll").is_file());
    assert!(artifact.with_file_name("Support.dll").is_file());
    let direct = Command::new("dotnet").arg(&artifact).output().unwrap();
    assert_eq!(success(direct).trim(), "30");
    let run = fixture.with_flags("run", &["--framework", "--", "--locked", "two words"], None);
    assert_eq!(
        success(run).lines().collect::<Vec<_>>(),
        ["30", "--locked", "two words"]
    );
    assert_eq!(fixture.artifact(fixture.oxel("compile", None)), artifact);
}

#[test]
fn locked_runs_reuse_complete_bundles_and_rebuild_after_changes() {
    let fixture = Fixture::new();
    let root = fixture.workspace.path();
    let lock = fixture.artifact(fixture.oxel("restore", None));
    let cache = root.join("app/target/oxel/run-cache");
    let record = |entries: &Path| {
        let entry = fs::read_dir(entries)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let pointer = fs::read_to_string(entry.join("latest")).unwrap();
        let metadata: serde_json::Value = serde_json::from_str(&pointer).unwrap();
        let bundle = entry
            .join(metadata["directory"].as_str().unwrap())
            .join("output");
        (entry, pointer, bundle)
    };
    let run = |argument: &str| {
        let result = fixture.with_flags("run", &["--locked", "--", argument], None);
        success(result)
            .lines()
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };

    assert_eq!(run("first"), ["30", "first"]);
    let (entry, pointer, bundle) = record(&cache);
    assert_eq!(run("again"), ["30", "again"]);
    assert_eq!(fs::read_to_string(entry.join("latest")).unwrap(), pointer);

    fs::remove_file(bundle.join("Leaf.dll")).unwrap();
    assert_eq!(run("repaired"), ["30", "repaired"]);
    let (_, new_pointer, _) = record(&cache);
    assert_ne!(new_pointer, pointer);
    assert_eq!(run("reused repair"), ["30", "reused repair"]);
    assert_eq!(
        fs::read_to_string(entry.join("latest")).unwrap(),
        new_pointer
    );

    let config = root.join("NuGet.Config");
    let text = fs::read_to_string(&config).unwrap();
    fs::write(config, format!("{text}\n<!-- feed config edited -->")).unwrap();
    assert_eq!(run("config edit"), ["30", "config edit"]);
    assert_eq!(fs::read_dir(&cache).unwrap().count(), 2);

    let support = oxel::compile(
        "fn main() {} fn subtract(a: i32, b: i32) -> i32 { a - b + 1 }",
        "Support",
    )
    .unwrap();
    fs::write(root.join("app/support.dll"), support).unwrap();
    assert_eq!(run("reference edit"), ["31", "reference edit"]);

    let source = root.join("app/main.oxel");
    let edited = "fn main() { System.Console.WriteLine(Fixture.Facade.Number() + 12); }";
    fs::write(&source, edited).unwrap();
    let dependencies = fs::read_dir(&cache).unwrap().count();
    assert_eq!(run("source edit"), ["42"]);
    assert_eq!(fs::read_dir(&cache).unwrap().count(), dependencies);
    assert_eq!(run("source edit reused"), ["42"]);
    fs::write(&source, "fn main() { missing(); }").unwrap();
    let error = fixture.with_flags("run", &["--locked"], None);
    assert!(!error.status.success());
    assert!(String::from_utf8(error.stderr).unwrap().contains("E0203"));

    fs::write(&source, edited).unwrap();
    fs::remove_file(lock).unwrap();
    let error = fixture.with_flags("run", &["--locked"], None);
    assert!(!error.status.success());
    assert!(String::from_utf8(error.stderr).unwrap().contains("E0010"));
}

#[test]
fn sdk_copies_runtime_dependencies_for_managed_libraries() {
    let fixture = Fixture::new();
    let root = fixture.workspace.path();
    let manifest = root.join("app/oxel.toml");
    let text = fs::read_to_string(&manifest).unwrap();
    fs::write(
        manifest,
        text.replace(
            "name = 'nuget & demo'",
            "name = 'Samples.Packaged'\nkind = 'library'",
        ),
    )
    .unwrap();
    fs::write(
        root.join("app/main.oxel"),
        "fn number() -> i32 { Program.subtract(Fixture.Facade.Number(), 0) }",
    )
    .unwrap();
    let artifact = fixture.artifact(fixture.with_flags("compile", &["--framework"], None));
    assert_eq!(artifact.file_name().unwrap(), "Samples.Packaged.dll");
    assert!(artifact.with_extension("deps.json").is_file());
    assert!(!artifact.with_extension("runtimeconfig.json").exists());
    fs::write(
        root.join("host.oxel"),
        "fn main() { System.Console.WriteLine(Samples.Packaged.Program.number()); }",
    )
    .unwrap();
    let mut host = Command::new(env!("CARGO_BIN_EXE_oxel"));
    host.current_dir(root)
        .args(["run", "host.oxel", "--reference"])
        .arg(&artifact);
    for name in ["Facade.dll", "Leaf.dll", "Support.dll"] {
        let dependency = artifact.with_file_name(name);
        assert!(dependency.is_file());
        host.arg("--reference").arg(dependency);
    }
    assert_eq!(success(host.output().unwrap()).trim(), "30");
}

#[test]
fn missing_sdk_is_reported_for_manifests_with_nuget_packages() {
    let workspace = tempfile::tempdir().unwrap();
    let root = workspace.path();
    fs::write(
        root.join("oxel.toml"),
        "[package]\nname = 'demo'\nsources = ['main.oxel']\n[nuget]\nExample = '1.0.0'",
    )
    .unwrap();
    fs::write(root.join("main.oxel"), "fn main() {}").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_oxel"))
        .current_dir(root)
        .env("PATH", "")
        .args(["check", "--diagnostic-format=json"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["file"], "oxel.toml");
    assert_eq!(error["code"], "E0010");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("NuGet restore failed")
    );
}

#[test]
#[ignore = "requires restored .NET Native AOT toolchain packages and a host linker"]
fn sdk_publishes_a_nuget_based_native_binary() {
    let fixture = Fixture::new();
    let output = success(
        Command::new("dotnet")
            .args(["nuget", "locals", "global-packages", "--list"])
            .output()
            .unwrap(),
    );
    let cache = Path::new(output.split_once(':').unwrap().1.trim());
    let managed = fixture.artifact(fixture.oxel("restore", None));
    let managed_bytes = fs::read(&managed).unwrap();
    let unlocked_native = fixture.with_flags("publish", &["--locked"], Some(cache));
    assert!(restore_failure(unlocked_native).contains(".aot.lock.json"));
    let native = fixture.artifact(fixture.with_flags("restore", &["--native"], Some(cache)));
    assert!(native.to_string_lossy().ends_with(".aot.lock.json"));
    assert_ne!(native, managed);
    let native_bytes = fs::read(&native).unwrap();
    let artifact = fixture.artifact(fixture.with_flags("publish", &["--locked"], Some(cache)));
    assert_eq!(fs::read(managed).unwrap(), managed_bytes);
    assert_eq!(fs::read(native).unwrap(), native_bytes);
    assert!(
        artifact.starts_with(
            fixture
                .workspace
                .path()
                .join(Path::new("app/target/oxel/native"))
        )
    );
    let output = Command::new(artifact)
        .env("PATH", "")
        .env_remove("DOTNET_ROOT")
        .output()
        .unwrap();
    assert_eq!(success(output).trim(), "30");
}

fn restore_failure(output: Output) -> String {
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["code"], "E0010");
    error["message"].as_str().unwrap().to_owned()
}

fn locked_leaf(bytes: &[u8]) -> String {
    let graph: serde_json::Value = serde_json::from_slice(bytes).unwrap();
    graph["dependencies"]
        .as_object()
        .unwrap()
        .values()
        .find_map(|target| {
            target.as_object()?.iter().find_map(|(id, entry)| {
                id.to_ascii_lowercase()
                    .starts_with("oxel.tests.leaf.")
                    .then(|| {
                        assert_eq!(entry["type"], "Transitive");
                        assert!(entry["contentHash"].is_string());
                        entry["resolved"].as_str().unwrap().to_owned()
                    })
            })
        })
        .unwrap()
}

#[test]
fn locks_the_restored_graph_until_explicitly_refreshed() {
    let fixture = Fixture::with_leaf("1.1.0", "[1.0.0,)");
    let missing = fixture.with_flags("check", &["--locked"], None);
    assert!(restore_failure(missing).contains(".lock.json"));

    // Restore from the manifest directory without spelling out its path.
    let root = fixture.workspace.path();
    let output = Command::new(env!("CARGO_BIN_EXE_oxel"))
        .current_dir(root.join("app"))
        .env("NUGET_PACKAGES", root.join("packages"))
        .args(["restore", "--diagnostic-format=json"])
        .output()
        .unwrap();
    let lock = root.join("app").join(success(output).trim());
    assert!(
        lock.file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("oxel.toml.nuget.net")
    );
    let initial = fs::read(&lock).unwrap();
    assert_eq!(locked_leaf(&initial), "1.1.0");

    fixture.pack_leaf("1.0.0", 16);
    assert_eq!(success(fixture.oxel("run", None)).trim(), "30");
    assert_eq!(
        fixture.artifact(fixture.with_flags("restore", &["--locked"], None)),
        lock,
    );
    assert_eq!(fs::read(&lock).unwrap(), initial);

    let manifest = root.join("app/oxel.toml");
    let original = fs::read_to_string(&manifest).unwrap();
    fs::write(&manifest, original.replace("'1.0.0'", "'2.0.0'")).unwrap();
    let stale = fixture.oxel("check", None);
    assert!(restore_failure(stale).contains("NU1004"));
    let refresh_failed = fixture.oxel("restore", None);
    assert!(restore_failure(refresh_failed).contains("2.0.0"));
    assert_eq!(fs::read(&lock).unwrap(), initial);
    fs::write(&manifest, original).unwrap();

    assert_eq!(fixture.artifact(fixture.oxel("restore", None)), lock);
    let refreshed = fs::read(&lock).unwrap();
    assert_eq!(locked_leaf(&refreshed), "1.0.0");
    let result = fixture.with_flags("run", &["--locked"], None);
    assert_eq!(success(result).trim(), "31");
    assert_eq!(fs::read(&lock).unwrap(), refreshed);
    let broken = b"invalid lock file";
    fs::write(&lock, broken).unwrap();
    assert!(restore_failure(fixture.oxel("check", None)).contains("locked NuGet graph"));
    assert_eq!(fs::read(&lock).unwrap(), broken);
}
