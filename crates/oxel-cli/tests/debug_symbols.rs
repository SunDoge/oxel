use std::{fs, process::Command};

#[test]
fn embedded_symbols_map_exception_frames_to_separate_oxel_files() {
    let folder = tempfile::tempdir().unwrap();
    let project = folder.path();
    fs::write(project.join("main.oxel"), "fn main() {\n    fail();\n}\n").unwrap();
    fs::write(
        project.join("helper.oxel"),
        concat!(
            "fn fail() {\n",
            "    let value = 30;\n",
            "    System.Console.WriteLine(value);\n",
            "    throw new System.FormatException(\"bad\")\n",
            "}\n"
        ),
    )
    .unwrap();

    let cli = env!("CARGO_BIN_EXE_oxel");
    let compile = || {
        let output = Command::new(cli)
            .current_dir(project)
            .args([
                "compile",
                "--debug",
                "--framework",
                "main.oxel",
                "helper.oxel",
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        project.join(String::from_utf8(output.stdout).unwrap().trim())
    };
    let artifact = compile();
    let image = fs::read(&artifact).unwrap();
    assert_eq!(image, fs::read(compile()).unwrap());
    let run = Command::new("dotnet")
        .current_dir(project)
        .arg("exec")
        .arg(artifact)
        .output()
        .unwrap();
    assert!(!run.status.success());
    assert_eq!(String::from_utf8(run.stdout).unwrap(), "30\n");
    let trace = String::from_utf8_lossy(&run.stderr);
    assert!(trace.contains("helper.oxel:line 4"), "{trace}");
    assert!(trace.contains("main.oxel:line 2"), "{trace}");
}
