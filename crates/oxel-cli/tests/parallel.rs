use std::{
    fmt::Write,
    fs,
    path::Path,
    process::{Command, Output},
    sync::Arc,
};

use oxel::{ProjectAnalyzer, References, inspect_project};

fn workload(invalid: bool) -> String {
    let mut source = String::from("struct Record<T> { value: T }\n");
    for index in 0..256 {
        writeln!(source, "struct Tag{index} {{ value: i32 }}").unwrap();
        writeln!(source, "fn f{index}(x: i32) -> i32 {{").unwrap();
        writeln!(
            source,
            "    let record = Record::<Tag{index}> {{ value: Tag{index} {{ value: x }} }};"
        )
        .unwrap();
        source.push_str("    let mut total = record.value.value;\n");
        for _ in 0..29 {
            source.push_str("    total += 1;\n");
        }
        if invalid && matches!(index, 1 | 253) {
            source.push_str("    false\n}\n");
        } else {
            source.push_str("    total\n}\n");
        }
    }
    source.push_str("fn main() { System.Console.WriteLine(f253(1)); }");
    source
}

fn cli(folder: &Path, jobs: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_oxel"))
        .current_dir(folder)
        .env("OXEL_JOBS", jobs)
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn parallel_checks_match_sequential_diagnostics_and_assemblies() {
    let folder = tempfile::tempdir().unwrap();
    let cwd = folder.path();
    let file = cwd.join("workload.oxel");
    fs::write(&file, workload(false)).unwrap();

    let compile = |jobs| {
        let output = cli(cwd, jobs, &["compile", "workload.oxel"]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let output_path = String::from_utf8(output.stdout).unwrap();
        let path = cwd.join(output_path.trim());
        (fs::read(&path).unwrap(), path)
    };
    let (sequential, _) = compile("1");
    for _ in 0..3 {
        assert_eq!(compile("4").0, sequential);
    }
    let (_, assembly) = compile("4");
    let executed = Command::new("dotnet").arg(&assembly).output().unwrap();
    assert!(executed.status.success(), "{executed:?}");
    assert_eq!(String::from_utf8(executed.stdout).unwrap().trim(), "30");

    let source = workload(true);
    fs::write(&file, &source).unwrap();
    let errors = |jobs| {
        let output = cli(
            cwd,
            jobs,
            &["check", "workload.oxel", "--diagnostic-format", "json"],
        );
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        output.stderr
    };
    let sequential = errors("1");
    assert_eq!(errors("4"), sequential);
    let diagnostics = String::from_utf8(sequential).unwrap();
    let actual: Vec<_> = diagnostics
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .collect();
    let positions = source.match_indices("false").map(|(start, _)| start);
    assert_eq!(actual.len(), 2);
    for (diagnostic, start) in actual.iter().zip(positions) {
        assert_eq!(diagnostic["code"], "E0301");
        assert_eq!(diagnostic["range"]["start"], start);
    }
}

#[test]
fn incremental_inspection_reuses_checked_parallel_bodies() {
    let references = References::default();
    let mut analyzer = ProjectAnalyzer::default();
    let mut sources = [Arc::<str>::from(workload(true))];
    let first = analyzer.inspect(&sources, &references);
    assert_eq!(analyzer.checked_functions(), 257);
    assert_eq!(first[0].diagnostics.len(), 2);
    assert_eq!(first, inspect_project(&[&sources[0]], &references));
    assert_eq!(first, analyzer.inspect(&sources, &references));
    assert_eq!(analyzer.checked_functions(), 0);

    sources[0] = Arc::from(sources[0].replacen("    total += 1;", "    total += 200;", 1));
    let changed = analyzer.inspect(&sources, &references);
    assert_eq!(analyzer.checked_functions(), 1);
    assert_eq!(analyzer.parsed_files(), 1);
    assert_eq!(changed, inspect_project(&[&sources[0]], &references));
}
