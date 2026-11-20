use std::{fs, process::Command};

use serde_json::Value;

#[test]
fn json_diagnostics_keep_file_spans_and_named_types() {
    let folder = tempfile::tempdir().unwrap();
    let broken = r#"fn broken() { let _ = "🌍"; let value: User = Other {}; }"#;
    fs::write(
        folder.path().join("main.oxel"),
        "struct User {} struct Other {} fn main() {}",
    )
    .unwrap();
    fs::write(folder.path().join("broken.oxel"), broken).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_oxel"))
        .current_dir(folder.path())
        .args([
            "check",
            "main.oxel",
            "broken.oxel",
            "--diagnostic-format",
            "json",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let stream = String::from_utf8(output.stderr).unwrap();
    let mut messages = stream
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap());
    let error = messages.next().unwrap();
    assert!(messages.next().is_none());
    assert_eq!(error["file"], "broken.oxel");
    assert_eq!(error["code"], "E0301");
    assert_eq!(error["severity"], "error");
    assert_eq!(error["expected"], "User");
    assert_eq!(error["actual"], "Other");
    assert_eq!(error["line"], 1);
    let start = broken.find("Other").unwrap();
    assert_eq!(error["column"], broken[..start].chars().count() + 1);
    assert_eq!(error["range"]["start"], start);
    assert_eq!(error["range"]["end"], start + "Other {}".len());
}

#[test]
fn call_diagnostics_explain_where_the_signature_is_declared() {
    let folder = tempfile::tempdir().unwrap();
    let caller = "fn main() { accept(false); }";
    let callee = "// expected signature 🌍\nfn accept(value: i32) {}";
    fs::write(folder.path().join("main.oxel"), caller).unwrap();
    fs::write(folder.path().join("api.oxel"), callee).unwrap();
    let check = |flags: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_oxel"))
            .current_dir(folder.path())
            .args(["check", "main.oxel", "api.oxel"])
            .args(flags)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        String::from_utf8(output.stderr).unwrap()
    };
    let output = check(&["--diagnostic-format=json"]);
    let error: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(error["file"], "main.oxel");
    assert_eq!(error["expected"], "System.Int32");
    assert_eq!(error["actual"], "System.Boolean");
    let start = callee.find("accept").unwrap();
    assert_eq!(
        error["related"],
        serde_json::json!({
            "file": "api.oxel",
            "range": {"start": start, "end": start + 6},
            "line": 2,
            "column": 4,
            "message": "parameter 'value' is declared by 'accept'",
        })
    );
    let text = check(&[]);
    assert!(text.contains("api.oxel:2:4: note: parameter 'value' is declared by 'accept'"));

    fs::write(folder.path().join("main.oxel"), "fn main() { accept(); }").unwrap();
    let error: Value = serde_json::from_str(&check(&["--diagnostic-format=json"])).unwrap();
    assert_eq!(error["code"], "E0205");
    assert_eq!(error["related"]["file"], "api.oxel");
    assert_eq!(error["related"]["message"], "'accept' is declared here");
}

#[test]
fn json_format_handles_cli_errors_without_changing_program_stdout() {
    let folder = tempfile::tempdir().unwrap();
    let cli = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_oxel"))
            .current_dir(folder.path())
            .args(args)
            .output()
            .unwrap()
    };
    let missing = cli(&["check", "--diagnostic-format=json", "missing.oxel"]);
    let error: Value = serde_json::from_slice(&missing.stderr).unwrap();
    assert_eq!(missing.status.code(), Some(1));
    assert_eq!(error["code"], "E0002");
    assert_eq!(error["file"], "missing.oxel");
    assert!(error.get("expected").is_none());

    let usage = cli(&["compile", "--diagnostic-format=json"]);
    assert_eq!(usage.status.code(), Some(2));
    let error: Value = serde_json::from_slice(&usage.stderr).unwrap();
    assert_eq!(error["code"], "E0001");

    let typo = cli(&["check", "--diagnostic-format", "json", "--bad-option"]);
    assert_eq!(typo.status.code(), Some(2));
    let error: Value = serde_json::from_slice(&typo.stderr).unwrap();
    assert_eq!(error["code"], "E0001");
    assert!(error["message"].as_str().unwrap().contains("--bad-option"));

    let help = cli(&["run", "--help"]);
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout).unwrap();
    assert!(help.contains("--framework") && help.contains("-- ARGUMENTS"));
    let spec = cli(&["__usage_spec__"]);
    assert!(spec.status.success());
    let spec = String::from_utf8(spec.stdout).unwrap();
    assert!(spec.contains("cmd run") && spec.contains("double_dash=required"));

    fs::write(
        folder.path().join("main.oxel"),
        "fn main() { System.Console.WriteLine(30); }",
    )
    .unwrap();
    let success = cli(&["run", "main.oxel", "--diagnostic-format=json"]);
    assert!(success.status.success());
    assert!(success.stderr.is_empty());
    assert_eq!(String::from_utf8(success.stdout).unwrap().trim(), "30");
}

#[test]
fn structured_fixes_target_the_correct_file_and_repair_programs() {
    let folder = tempfile::tempdir().unwrap();
    let cwd = folder.path();
    fs::write(cwd.join("main.oxel"), "fn main() {}").unwrap();
    for (source, code, anchor, replacement, deletion) in [
        (
            r#"fn helper() { let emoji = "😀" // preserve comment
    System.Console.WriteLine(emoji);
}"#,
            "E0102",
            "\" // preserve",
            ";",
            0,
        ),
        (
            r#"fn helper() { "🌍"; let count = 1; count += 2; }"#,
            "E0204",
            "count = 1",
            "mut ",
            0,
        ),
        (
            "fn helper() { let count: i32; count = 1; count = 2; }",
            "E0322",
            "count: i32",
            "mut ",
            0,
        ),
        (
            "fn helper() { let flag: bool? = false; }",
            "E0311",
            "? =",
            "",
            1,
        ),
    ] {
        fs::write(cwd.join("helper.oxel"), source).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_oxel"))
            .current_dir(cwd)
            .args([
                "check",
                "main.oxel",
                "helper.oxel",
                "--diagnostic-format",
                "json",
            ])
            .output()
            .unwrap();
        assert!(!output.status.success(), "{source}");
        let error: Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(error["code"], code, "{source}: {error}");
        assert_eq!(error["file"], "helper.oxel");
        let start = source.find(anchor).unwrap() + usize::from(code == "E0102");
        let end = start + deletion;
        assert_eq!(
            error["fix"]["range"],
            serde_json::json!({"start": start, "end": end})
        );
        assert_eq!(error["fix"]["replacement"], replacement);
        assert!(
            error["fix"]["title"]
                .as_str()
                .is_some_and(|title| !title.is_empty())
        );
        let mut revised = source.to_string();
        revised.replace_range(start..end, replacement);
        oxel::check_project(&["fn main() {}", &revised], &oxel::References::default()).unwrap();
    }
}

#[test]
fn mutability_fixes_apply_to_let_bindings_with_an_unambiguous_edit() {
    for source in [
        "fn set(value: i32) { value = 2; } fn main() {}",
        "const VALUE = 1; fn main() { VALUE = 2; }",
        "fn main() { for i in 0..2 { i = 2; } }",
        "enum E { Some(i32) } fn main() { match E.Some(1) { Some(x) => { x = 2; } }; }",
        "fn main() { let x: i32; x += 2; }",
        "fn main() { let x: string? = null; x = \"ready\"; }",
    ] {
        let error = &oxel::check(source).unwrap_err()[0];
        assert_eq!(error.code, "E0204", "{source}: {error:?}");
        assert!(error.fix().is_none(), "{source}: {error:?}");
    }
}
