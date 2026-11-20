use std::{
    fmt::Write as _,
    fs,
    path::Path,
    process::{Command, Output},
};

use oxel::Type;

const HELLO: &str = include_str!("../examples/hello.oxel");

#[test]
fn unit_tests_run_in_order_and_fail_independently_of_main() {
    let folder = tempfile::tempdir().unwrap();
    let directory = folder.path();
    let sample = directory.join("sample.oxel");
    fs::write(&sample, include_str!("../examples/unit_tests.oxel")).unwrap();
    assert_eq!(
        stdout(cli(directory, "test", &sample)),
        "ok adds_positive_numbers\nok zero_is_the_additive_identity\n2 tests, 0 failed\n",
    );

    let main = directory.join("main.oxel");
    let tests = directory.join("tests.oxel");
    fs::write(
        &main,
        "fn main() { System.Console.WriteLine(42); } internal fn value() -> i32 { 42 }",
    )
    .unwrap();
    fs::write(
        &tests,
        r#"
        #[test]
        fn fails() { assert(value() == 30, "wrong result"); }
        #[test]
        fn continues() {
            assert(value() == 42, { System.Console.WriteLine("eager message"); "wrong result" });
        }
        #[test]
        fn catches_runtime_exceptions() {
            let divisor = value() - 42;
            42 / divisor;
        }
    "#,
    )
    .unwrap();
    let run = |action| {
        Command::new(env!("CARGO_BIN_EXE_oxel"))
            .current_dir(directory)
            .arg(action)
            .args([&main, &tests])
            .output()
            .unwrap()
    };
    let result = run("test");
    assert_eq!(result.status.code(), Some(1));
    let output = String::from_utf8(result.stdout).unwrap();
    assert!(
        output.starts_with("FAIL fails\nSystem.InvalidOperationException: wrong result\n"),
        "{output}"
    );
    assert!(
        output.contains("\nok continues\nFAIL catches_runtime_exceptions\n"),
        "{output}"
    );
    assert!(output.contains("System.DivideByZeroException"), "{output}");
    assert!(output.ends_with("3 tests, 2 failed\n"), "{output}");
    assert!(!output.contains("eager message"), "{output}");
    assert_eq!(stdout(run("run")), "42\n");

    fs::write(&tests, "#[test] fn broken() { unknown(); }").unwrap();
    assert_eq!(stdout(run("run")), "42\n");
    let rejected = run("test");
    assert!(!rejected.status.success());
    let error = String::from_utf8(rejected.stderr).unwrap();
    assert!(
        error.contains("tests.oxel:1:") && error.contains("E0203"),
        "{error}"
    );

    for source in [
        "#[test] fn generic<T>() {}",
        "#[test] fn argument(value: i32) {}",
        "#[test] fn result() -> bool { true }",
        "fn main() { example(); } #[test] fn example() {}",
    ] {
        let errors = oxel::compile_test_project(
            &[source],
            "suite",
            &oxel::References::default(),
            oxel::AssemblyKind::Executable,
        )
        .unwrap_err();
        assert_eq!(errors[0].diagnostic.code, "E0346", "{source}: {errors:?}");
    }
}

#[test]
fn struct_patterns_destructure_and_check_nested_field_coverage() {
    let folder = tempfile::tempdir().unwrap();
    let program = folder.path().join("patterns.oxel");
    fs::write(
        &program,
        r#"
        struct Pair<T> { value: T, ready: bool }
        struct Record { pair: Pair<i32>, extra: Option<i32> }
        comptime fn read(pair: Pair<i32>) -> i32 {
            let Pair { value: item, .. } = pair;
            match pair {
                Pair { ready: true, .. } => item,
                Pair { ready: false, .. } => 0,
            }
        }
        const NUMBER = read(Pair::<i32> { value: 30, ready: true });

        fn total(record: Record) -> i32 {
            let Record {
                extra: Some(extra),
                pair: Pair { value: value, ready: true },
            } = record else { return 0; };
            value + extra
        }

        fn main() {
            let record = Record {
                pair: Pair::<i32> { value: NUMBER, ready: true },
                extra: Option.Some(12),
            };
            let Record { pair: Pair { value: item, .. }, .. } = record;
            System.Console.WriteLine(item);
            System.Console.WriteLine(total(record));
            System.Console.WriteLine(total(Record { extra: Option.None, ..record }));
            if let Record { extra: Some(extra), .. } = record {
                System.Console.WriteLine(extra);
            }
            let selected = match Option.Some::<Record>(record) {
                Some(Record { pair: Pair { value: item, .. }, .. }) if item < 0 => -item,
                Some(Record { pair: Pair { value: item, .. }, .. }) => item,
                None => 0,
            };
            System.Console.WriteLine(selected);
            let mut current = record;
            while let Record { extra: Some(extra), .. } = current {
                System.Console.WriteLine(extra + 1);
                current = Record { extra: Option.None, ..current };
            }
        }
        "#,
    )
    .unwrap();
    assert_eq!(
        stdout(cli(folder.path(), "run", &program)),
        "30\n42\n0\n12\n30\n13\n",
    );

    for (body, expected) in [
        (
            "let Pair { ready: true, .. } = pair;",
            "this pattern can fail",
        ),
        ("let Pair { ready: flag } = pair;", "omits fields"),
        (
            "let Pair { ready: x, ready: y, .. } = pair;",
            "duplicate pattern field",
        ),
        ("let Other { .. } = pair;", "does not match"),
        (
            "match pair { Pair { ready: true, .. } => 1, };",
            "non-exhaustive match",
        ),
        (
            "let Pair { value: x, ready: x } = pair;",
            "duplicate pattern binding",
        ),
    ] {
        let source = format!(
            "struct Pair {{ value: i32, ready: bool }} fn read(pair: Pair) {{ {body} }} fn main() {{}}"
        );
        let errors = oxel::check(&source).unwrap_err();
        assert!(errors[0].message.contains(expected), "{source}: {errors:?}");
    }
}

#[test]
fn struct_update_preserves_value_semantics_and_source_order() {
    let folder = tempfile::tempdir().unwrap();
    let program = folder.path().join("update.oxel");
    fs::write(&program, include_str!("../examples/struct_update.oxel")).unwrap();
    assert_eq!(
        stdout(cli(folder.path(), "run", &program)),
        "Oxel\n800\n640\n"
    );

    fs::write(
        &program,
        r#"
        struct Pair<T> { first: T, second: T }
        const UPDATED: Pair<i32> = Pair { first: 42, ..ORIGINAL };
        const ORIGINAL: Pair<i32> = Pair { first: 1, second: 2 };
        fn early() -> i32 {
            let value: Pair<i32> = Pair { first: 20, ..{ return 7; } };
            value.first
        }
        fn main() {
            let mut visits = 0;
            let original = Pair::<i32> { first: 1, second: 2 };
            let updated = Pair::<i32> {
                second: { visits = visits * 10 + 1; 30 },
                first: { visits = visits * 10 + 2; 20 },
                ..{ visits = visits * 10 + 3; original },
            };
            let copied = Pair::<i32> { ..updated };
            System.Console.WriteLine(visits);
            System.Console.WriteLine(copied.first + copied.second);
            System.Console.WriteLine(ORIGINAL.first);
            System.Console.WriteLine(UPDATED.first);
            System.Console.WriteLine(UPDATED.second);
            System.Console.WriteLine(early());
        }
        "#,
    )
    .unwrap();
    assert_eq!(
        stdout(cli(folder.path(), "run", &program)),
        "123\n50\n1\n42\n2\n7\n"
    );

    for (source, code) in [
        (
            "struct S { x: i32 } struct R { x: i32 } fn main() { S { x: 1, ..R { x: 2 } }; }",
            "E0301",
        ),
        (
            "struct S { x: i32 } fn update(s: S) { S { x: 1, x: 2, ..s }; } fn main() {}",
            "E0306",
        ),
        (
            "struct S { x: i32 } fn update(s: S) { S { ..s, x: 1 }; } fn main() {}",
            "E0102",
        ),
        (
            "struct S { x: i32 } const S1: S = S { ..S2 }; const S2: S = S { ..S1 }; fn main() {}",
            "E0502",
        ),
    ] {
        let errors = oxel::check(source).unwrap_err();
        assert_eq!(errors[0].code, code, "{source}: {errors:?}");
    }
}

#[test]
fn generic_construction_requires_a_valid_default_for_oxel_value_types() {
    let folder = tempfile::tempdir().unwrap();
    let program = folder.path().join("construction.oxel");
    fs::write(
        &program,
        r#"
        struct Slot<T> { label: Option<T>, index: i32 }
        struct Named { name: string }
        struct Box<T: struct> { value: T }
        struct Factory<T: new()> { value: T }
        struct Forward { factory: Factory<Slot<string>> }
        fn make<T: new()>() -> T { new T() }
        fn forward<T: struct + new()>() -> T { make::<T>() }
        fn main() {
            let slot = forward::<Slot<string>>();
            let explicit = Box::<Named> { value: Named { name: "explicit" } };
            let _unused: Forward;
            System.Console.WriteLine(slot.label ?? "empty");
            System.Console.WriteLine(slot.index);
            System.Console.WriteLine(explicit.value.name);
        }
        "#,
    )
    .unwrap();
    assert_eq!(
        stdout(cli(folder.path(), "run", &program)),
        "empty\n0\nexplicit\n",
    );

    for source in [
        "fn make<T: struct>() -> T { new T() } fn main() {}",
        "struct Record { name: string } fn make<T: new()>() -> T { new T() } fn main() { make::<Record>(); }",
        "enum State { Ready } fn make<T: new()>() -> T { new T() } fn main() { make::<State>(); }",
        "struct Wrapper<T> { value: T } fn make<T: new()>() -> T { new T() } fn main() { make::<Wrapper<string>>(); }",
        "struct Container { factory: Factory<Record> } struct Factory<T: new()> { value: T } struct Record { name: string } fn main() {}",
        "enum Container { Item(Factory<Record>) } struct Factory<T: new()> { value: T } struct Record { name: string } fn main() {}",
    ] {
        let errors = oxel::check(source).unwrap_err();
        assert_eq!(errors[0].code, "E0212", "{source}: {errors:?}");
        assert!(errors[0].message.contains("new()"), "{source}: {errors:?}");
    }
}

#[test]
fn clr_zero_initialized_options_are_none_at_runtime_and_comptime() {
    let folder = tempfile::tempdir().unwrap();
    let program = folder.path().join("options.oxel");
    fs::write(&program, include_str!("../examples/option_arrays.oxel")).unwrap();
    assert_eq!(stdout(cli(folder.path(), "run", &program)), "empty\noxel\n");

    fs::write(
        &program,
        r#"
        comptime fn empty() -> Option<string> {
            let values = new Option<string>[1];
            values[0]
        }
        comptime fn propagate(input: Option<string>) -> Option<string> {
            Option.Some(input?)
        }
        comptime fn inspect(value: Option<string>) -> string {
            match value {
                None if false => "wrong guard",
                None => "constant none",
                Some(value) => value,
            }
        }
        const ZERO = empty();
        const LABEL = inspect(propagate(ZERO));
        const FALLBACK = ZERO ?? "constant fallback";

        struct Slot { label: Option<string> }
        fn first(values: Slot[]) -> Option<string> {
            Option.Some(values[0].label?)
        }
        fn main() {
            System.Console.WriteLine(LABEL);
            System.Console.WriteLine(FALLBACK);
            let values = new Slot[1];
            let result = match first(values) {
                None if false => "wrong guard",
                None => "runtime none",
                Some(value) => value,
            };
            System.Console.WriteLine(result);
            let result: Option<string> = try { values[0].label? };
            System.Console.WriteLine(result ?? "local propagation");
        }
        "#,
    )
    .unwrap();
    assert_eq!(
        stdout(cli(folder.path(), "run", &program)),
        "constant none\nconstant fallback\nruntime none\nlocal propagation\n",
    );

    for source in [
        "enum Option<T> { None, Some(T) } fn main() { new Option<string>[1]; }",
        "struct Slot { label: string } fn main() { new Slot[1]; }",
    ] {
        let errors = oxel::check(source).unwrap_err();
        assert_eq!(errors[0].code, "E0314", "{source}: {errors:?}");
    }
}

fn cli(directory: &Path, command: &str, path: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_oxel"))
        .current_dir(directory)
        .arg(command)
        .arg(path)
        .output()
        .expect("launch oxel")
}

fn stdout(output: Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn hello_smoke_test() {
    let folder = tempfile::tempdir().unwrap();
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/hello.oxel");
    stdout(cli(folder.path(), "check", &source));
    assert!(!folder.path().join("target").exists());
    assert_eq!(stdout(cli(folder.path(), "run", &source)).trim(), "30");

    let printed_path = stdout(cli(folder.path(), "compile", &source));
    let assembly = folder.path().join(printed_path.trim());
    assert_eq!(
        fs::read(&assembly).unwrap(),
        oxel::compile(HELLO, "hello").unwrap(),
    );
    assert!(assembly.with_extension("runtimeconfig.json").is_file());
    let launched = Command::new("dotnet")
        .arg(assembly)
        .output()
        .expect("install .NET 8 or newer");
    assert_eq!(stdout(launched).trim(), "30");
}

#[test]
fn executable_entry_points_receive_arguments_and_return_exit_codes() {
    let folder = tempfile::tempdir().unwrap();
    let example = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/arguments.oxel");
    stdout(cli(folder.path(), "check", &example));
    let args = ["two words", "--reference", "", "咖啡", "--"];
    let launched = Command::new(env!("CARGO_BIN_EXE_oxel"))
        .current_dir(folder.path())
        .arg("run")
        .arg(&example)
        .arg("--")
        .args(args)
        .output()
        .unwrap();
    let expected = ["[two words]", "[--reference]", "[]", "[咖啡]", "[--]"];
    assert_eq!(stdout(launched).lines().collect::<Vec<_>>(), expected);

    let missing = cli(folder.path(), "run", &example);
    assert_eq!(missing.status.code(), Some(2), "{missing:?}");
    assert_eq!(
        String::from_utf8_lossy(&missing.stdout).trim(),
        "Usage: arguments <value>..."
    );
    assert!(missing.stderr.is_empty());

    let relative = stdout(cli(folder.path(), "compile", &example));
    let launched = Command::new("dotnet")
        .arg(folder.path().join(relative.trim()))
        .args(args)
        .output()
        .unwrap();
    assert_eq!(stdout(launched).lines().collect::<Vec<_>>(), expected);

    let source = folder.path().join("exit.oxel");
    fs::write(&source, "fn main() -> System.Int32 { 300 }").unwrap();
    let exit = cli(folder.path(), "run", &source);
    let compiled = stdout(cli(folder.path(), "compile", &source));
    let direct = Command::new("dotnet")
        .arg(folder.path().join(compiled.trim()))
        .output()
        .unwrap();
    assert!(!direct.status.success(), "{direct:?}");
    assert!(exit.stderr.is_empty(), "{exit:?}");
    assert_eq!(exit.status.code(), direct.status.code());
    oxel::check("fn main(args: System.String[]) {}").unwrap();

    for invalid in [
        "fn main() -> bool { false }",
        "fn main() -> i64 { 0 }",
        "fn main(args: i32[]) {}",
        "fn main(args: string[]?) {}",
        "fn main(args: string?[]) {}",
        "fn main(args: string[][]) {}",
        "fn main(args: string[], extra: string[]) {}",
    ] {
        let errors = oxel::check(invalid).unwrap_err();
        assert_eq!(errors[0].code, "E0206", "{invalid}: {errors:?}");
    }
    let misplaced = Command::new(env!("CARGO_BIN_EXE_oxel"))
        .current_dir(folder.path())
        .arg("check")
        .arg(&example)
        .arg("--")
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&misplaced.stderr).contains("error[E0001]"));
}

#[test]
fn arithmetic_locals_calls_and_comparisons_execute() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("control flow.oxel");
    fs::write(&path, include_str!("../examples/control_flow.oxel")).unwrap();
    let output = stdout(cli(folder.path(), "run", &path));
    assert_eq!(
        output.lines().collect::<Vec<_>>(),
        ["2", "True", "True", "True", "30", "-4001"]
    );
}

#[test]
fn same_scope_shadowing_survives_adding_a_distinct_binding() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("shadowing.oxel");
    fs::write(
        &path,
        r#"
        fn evaluate(value: i32) -> i32 {
            let value = value + 1;
            let value = value * 2;
            let other = value + 3;
            let value = value + other;
            value
        }
        fn main() { System.Console.WriteLine(evaluate(5)); }
        "#,
    )
    .unwrap();
    assert_eq!(stdout(cli(folder.path(), "run", &path)).trim(), "27");
}

#[test]
fn early_returns_inside_operands_preserve_valid_clr_stacks() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("returns.oxel");
    fs::write(
        &path,
        r#"
        fn add(a: i32, b: i32) -> i32 { a + b }
        fn test(a: bool, b: bool) -> i32 {
            2 + add(3, if a { return 40; } else if b { return 50; } else { 4 })
        }
        fn both(a: bool) -> i32 {
            if a { return 1; } else { return 2; }
        }
        fn minimum() -> i64 { -9223372036854775808 }
        fn main() {
            System.Console.WriteLine(test(true, false));
            System.Console.WriteLine(test(false, true));
            System.Console.WriteLine(test(false, false));
            System.Console.WriteLine(both(false));
            System.Console.WriteLine(minimum());
            System.Console.WriteLine();
            return;
        }
    "#,
    )
    .unwrap();
    let output = stdout(cli(folder.path(), "run", &path));
    assert_eq!(
        output.lines().collect::<Vec<_>>(),
        ["40", "50", "9", "2", "-9223372036854775808", ""]
    );
}

#[test]
fn wide_local_and_parameter_operands_execute() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("wide.oxel");
    let mut source = "fn last(".to_string();
    for index in 0..260 {
        write!(source, "p{index}: i32,").unwrap();
    }
    source.push_str(") -> i32 { p259 } fn main() {");
    for index in 0..260 {
        write!(source, "let x{index} = {index};").unwrap();
    }
    source.push_str("System.Console.WriteLine(last(");
    for index in 0..260 {
        write!(source, "x{index},").unwrap();
    }
    source.push_str(")); }");
    fs::write(&path, source).unwrap();
    assert_eq!(stdout(cli(folder.path(), "run", &path)).trim(), "259");
}

#[test]
fn emits_identical_bytes_for_same_source_and_name() {
    let bytes = oxel::compile(HELLO, "hello").unwrap();
    assert_eq!(&bytes[..2], b"MZ");
    assert_eq!(bytes, oxel::compile(HELLO, "hello").unwrap());
    assert_ne!(bytes, oxel::compile(HELLO, "other").unwrap());
}

#[test]
fn rejects_bad_programs_with_specific_diagnostics() {
    for (source, code) in [
        ("fn main( {", "E0102"),
        ("fn main() { let x = 1 }", "E0102"),
        ("fn main() { λ; }", "E0101"),
        ("fn main() { missing; }", "E0203"),
        ("fn main() { missing(); }", "E0203"),
        ("fn main() { System.Console.Missing(); }", "E0203"),
        ("fn main() { System.Console.WriteLine(1, 2); }", "E0205"),
        ("fn main() { System.Console.WriteLine(main()); }", "E0205"),
        ("fn f(a: i32) {} fn main() { f(); }", "E0205"),
        ("fn f(a: i32) {} fn main() { f(false); }", "E0301"),
        ("fn f() -> bool { return 1; } fn main() {}", "E0301"),
        (
            "fn f() -> bool { if true { return false; } } fn main() {}",
            "E0301",
        ),
        ("fn main() { let x: i32 = false; }", "E0301"),
        (
            "fn main() { let x = if true { 1 } else { false }; }",
            "E0301",
        ),
        ("fn main() { if 1 {} }", "E0301"),
        ("fn main() { true + false; }", "E0302"),
        ("fn main() { let x = 2147483648; }", "E0303"),
        ("fn main() { let x = 1; x = 2; }", "E0204"),
        ("fn main() { { let x = 1; } x; }", "E0203"),
        ("fn f(x: i32, x: bool) {} fn main() {}", "E0201"),
        ("fn main() {} fn main() {}", "E0201"),
        ("fn main() { let x: i128 = 0; }", "E0202"),
        ("fn main(x: i32) {}", "E0206"),
        ("fn f() {}", "E0206"),
    ] {
        let errors = oxel::check(source).expect_err(source);
        assert_eq!(errors[0].code, code, "{source}");
    }
}

#[test]
fn diagnostics_keep_source_spans_and_types() {
    let source = "fn main() {\n    let x: System.Int32 = false;\n}";
    let errors = oxel::check(source).unwrap_err();
    let error = &errors[0];
    assert_eq!(error.location(source), (2, 27));
    assert_eq!(&source[error.span.start..error.span.end], "false");
    assert_eq!(error.expected, Some(Type::I32));
    assert_eq!(error.actual, Some(Type::Bool));

    let folder = tempfile::tempdir().unwrap();
    let path = Path::new("bad.oxel");
    fs::write(folder.path().join(path), source).unwrap();
    let result = cli(folder.path(), "check", path);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("bad.oxel:2:27: error[E0301]"));
    assert!(!folder.path().join("target").exists());
}

#[test]
fn reports_errors_in_multiple_functions_and_limits_nesting() {
    let errors = oxel::check("fn a() { let = 1; } fn b() { let = 2; } fn main() {}").unwrap_err();
    assert_eq!(
        errors.iter().map(|error| error.code).collect::<Vec<_>>(),
        ["E0102", "E0102"]
    );
    for expression in [
        format!("{}0{}", "(".repeat(1000), ")".repeat(1000)),
        format!("{}0{}", "f(".repeat(1000), ")".repeat(1000)),
        "0+".repeat(1000) + "0",
        "value".to_owned() + &"[0]".repeat(1000),
        format!("{}0{}", "value[".repeat(1000), "]".repeat(1000)),
        format!("{}0{}", "match 0 { _ => ".repeat(1000), " }".repeat(1000)),
        format!("{}0{}", "match ".repeat(1000), " { _ => 0 }".repeat(1000)),
        "throw ".repeat(1000) + "0",
        format!("{}0{}", "try {".repeat(1000), "}".repeat(1000)),
        format!("{}0{}", "f() { ".repeat(30), " };".repeat(30)),
    ] {
        let errors = oxel::check(&format!("fn main() {{ {expression}; }}")).unwrap_err();
        assert_eq!(errors[0].code, "E0103");
    }
}

#[test]
fn strings_execute_with_ordinal_equality_and_unicode() {
    let folder = tempfile::tempdir().unwrap();
    let sample = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/strings.oxel");
    let result = stdout(cli(folder.path(), "run", &sample));
    assert_eq!(
        result.lines().collect::<Vec<_>>(),
        ["你好，oxel 🌍！", "True"]
    );

    let program = folder.path().join("escaped.oxel");
    let medium = "é".repeat(65);
    let large = "🌍".repeat(4096);
    let source = format!(
        r#"
        fn join(early: bool) -> string {{
            "pre" + if early {{ return "early"; }} else {{ "fix" }}
        }}
        fn main() {{
            System.Console.WriteLine(join(true));
            System.Console.WriteLine(join(false));
            System.Console.WriteLine("a\tb\\c\"d");
            System.Console.WriteLine("line 1\nline 2\rsuffix");
            System.Console.WriteLine("a\0b" != "ab");
            System.Console.WriteLine("É" == "é");
            System.Console.WriteLine("");
            System.Console.WriteLine("{medium}");
            System.Console.WriteLine("{large}");
        }}
        "#
    );
    fs::write(&program, source).unwrap();
    let result = stdout(cli(folder.path(), "run", &program));
    assert_eq!(
        result.lines().collect::<Vec<_>>(),
        [
            "early",
            "prefix",
            "a\tb\\c\"d",
            "line 1",
            "line 2\rsuffix",
            "True",
            "False",
            "",
            &medium,
            &large
        ],
    );
}

#[test]
fn rejects_bad_string_escapes_and_types() {
    let source = r#"fn main() { let x = "é\λ"; }"#;
    let errors = oxel::check(source).unwrap_err();
    assert_eq!(errors[0].code, "E0105");
    assert_eq!(&source[errors[0].span.start..errors[0].span.end], "\\λ");
    for (source, code) in [
        ("fn main() { \"unfinished", "E0104"),
        ("fn main() { \"multi\nline\"; }", "E0104"),
        (r#"fn main() { "string" + 1; }"#, "E0301"),
        (r#"fn main() { "a" > "b"; }"#, "E0302"),
        (r#"fn main() { "a" - "b"; }"#, "E0302"),
    ] {
        assert_eq!(oxel::check(source).unwrap_err()[0].code, code, "{source}");
    }
}

#[test]
fn struct_literals_preserve_source_order_and_value_semantics() {
    let folder = tempfile::tempdir().unwrap();
    let sample = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/structs.oxel");
    let result = stdout(cli(folder.path(), "run", &sample));
    assert_eq!(result.lines().collect::<Vec<_>>(), ["Hello, Alice", "30"]);

    let program = folder.path().join("values.oxel");
    fs::write(
        &program,
        r#"
        struct Pair { label: Label, n: i64, }
        struct Label { x: string }
        struct Empty {}

        fn get(stop: bool) -> Pair {
            Pair {
                n: { System.Console.WriteLine("first"); 100 },
                label: if stop { return Pair { n: 20, label: Label { x: "early" } }; }
                    else { Label { x: "second" } },
            }
        }

        fn main() {
            let _: Empty = Empty {};
            let mut pair = get(false);
            let old = pair;
            pair = get(true);
            System.Console.WriteLine(old.label.x);
            System.Console.WriteLine(pair.label.x);
            System.Console.WriteLine(pair.n);
            if (Label { x: "yes" }).x == "yes" {
                System.Console.WriteLine(old.n);
            }
        }
    "#,
    )
    .unwrap();
    let result = stdout(cli(folder.path(), "run", &program));
    assert_eq!(
        result.lines().collect::<Vec<_>>(),
        ["first", "first", "second", "early", "20", "100"]
    );
}

#[test]
fn rejects_invalid_struct_definitions_and_values() {
    for (source, code) in [
        ("struct S { next: S } fn main() {}", "E0304"),
        ("struct A { b: B } struct B { a: A } fn main() {}", "E0304"),
        ("struct S { x: i32, x: bool } fn main() {}", "E0201"),
        ("struct S {} struct S {} fn main() {}", "E0201"),
        ("struct Program {} fn main() {}", "E0201"),
        ("struct S { x: void } fn main() {}", "E0301"),
        ("struct S { x: Unknown } fn main() {}", "E0202"),
        ("struct S { x: i32 } fn main() { S {}; }", "E0306"),
        (
            "struct S { x: i32 } fn main() { S { x: 1, x: 2 }; }",
            "E0306",
        ),
        ("struct S { x: i32 } fn main() { S { x: true }; }", "E0301"),
        ("struct S { x: i32 } fn main() { S { y: 1 }; }", "E0305"),
        ("struct S {} fn main() { let x = S {}; x.y; }", "E0305"),
        ("struct S {} fn main() { S {} == S {}; }", "E0302"),
        (
            "struct S {} fn main() { System.Console.WriteLine(S {}, S {}); }",
            "E0205",
        ),
    ] {
        assert_eq!(oxel::check(source).unwrap_err()[0].code, code, "{source}");
    }
    let source = "struct A {} struct B {} fn main() { let _: A = B {}; }";
    assert_eq!(
        oxel::check(source).unwrap_err()[0].message,
        "expected A, found B"
    );
}

#[test]
fn nested_loop_transfers_preserve_enclosing_operands() {
    let folder = tempfile::tempdir().unwrap();
    let sample = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/loops.oxel");
    assert_eq!(stdout(cli(folder.path(), "run", &sample)).trim(), "30");

    let program = folder.path().join("loop-flows.oxel");
    fs::write(
        &program,
        r#"
        fn compute(stop: bool) -> i32 {
            10 + {
                let mut n = 0;
                let mut total = 0;
                while n < 6 {
                    n = n + 1;
                    let term = 99 + if n == 1 { continue; } else if n == 4 { break; } else { n };
                    total = total + term;
                    let mut k = 0;
                    while k < 2 {
                        k = k + 1;
                        if k == 1 { continue; }
                        break;
                    }
                    if stop { return total; }
                }
                total
            }
        }

        fn condition_transfer() -> i32 {
            let mut n = 0;
            while n < 3 {
                n = n + 1;
                while {
                    if n == 1 { continue; }
                    if n == 2 { break; }
                    false
                } {}
                System.Console.WriteLine(99);
            }
            n
        }

        fn early_condition() -> i32 {
            while { return 7; } {}
        }

        fn main() {
            System.Console.WriteLine(compute(false));
            System.Console.WriteLine(compute(true));
            System.Console.WriteLine(condition_transfer());
            System.Console.WriteLine(early_condition());
        }
        "#,
    )
    .unwrap();
    let result = stdout(cli(folder.path(), "run", &program));
    assert_eq!(result.lines().collect::<Vec<_>>(), ["213", "101", "2", "7"]);
}

#[test]
fn logical_operators_short_circuit_and_bind_below_comparisons() {
    let folder = tempfile::tempdir().unwrap();
    let program = folder.path().join("short-circuit.oxel");
    fs::write(
        &program,
        r#"
        fn print_false() -> bool {
            System.Console.WriteLine(20);
            false
        }
        fn branch(take: bool) -> i32 {
            let value = take && { return 9; };
            if value { return 10; }
            11
        }
        fn main() {
            System.Console.WriteLine(false && print_false());
            System.Console.WriteLine(true || print_false());
            System.Console.WriteLine(false || 3 < 4 && !false);
            System.Console.WriteLine(true || false && print_false());
            System.Console.WriteLine(true && print_false());
            System.Console.WriteLine(branch(false));
            System.Console.WriteLine(branch(true));
        }
        "#,
    )
    .unwrap();
    let result = stdout(cli(folder.path(), "run", &program));
    assert_eq!(
        result.lines().collect::<Vec<_>>(),
        ["False", "True", "True", "True", "20", "False", "11", "9"]
    );
}

#[test]
fn rejects_invalid_loop_conditions_and_control_transfers() {
    for (source, code) in [
        ("fn main() { break; }", "E0307"),
        ("fn main() { continue }", "E0307"),
        ("fn main() { while { break; } {} }", "E0307"),
        ("fn main() { while 1 {} }", "E0301"),
        ("fn main() { while false { 1 } }", "E0301"),
        ("fn main() { while true { break 1; } }", "E0102"),
        ("fn main() { let x = true && 1; }", "E0301"),
        ("fn main() { let x = false | 1; }", "E0301"),
        (
            "fn f() -> i32 { while true { break; } } fn main() {}",
            "E0301",
        ),
    ] {
        assert_eq!(oxel::check(source).unwrap_err()[0].code, code, "{source}");
    }
}

#[test]
fn generic_functions_execute_and_emit_clr_method_metadata() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("generics.oxel");
    let source = include_str!("../examples/generics.oxel");
    fs::write(&path, source).unwrap();
    assert_eq!(
        stdout(cli(folder.path(), "run", &path)),
        "30\n3000000000\noxel\nCLR\n"
    );

    let image = oxel::compile(source, "generics").unwrap();
    assert_eq!(image, oxel::compile(source, "generics").unwrap());
    // Importing the generated DLL exercises both its open signature and GenericParam rows.
    let mut references = oxel::References::default();
    references.add(&image).unwrap();
    assert!(
        oxel::check_with_references(
            "fn main() { System.Console.WriteLine(Program.identity::<i32>(30)); }",
            &references,
        )
        .is_ok()
    );
}

#[test]
fn rejects_invalid_generic_declarations_and_calls() {
    for (source, code) in [
        ("fn main<T>() {}", "E0206"),
        ("fn wrong<T, T>() {} fn main() {}", "E0201"),
        ("fn wrong<i32>() {} fn main() {}", "E0201"),
        ("fn wrong<>() {} fn main() {}", "E0102"),
        ("fn wrong(x: T) {} fn main() {}", "E0202"),
        ("fn unused<T>() {} fn main() { unused(); }", "E0210"),
        (
            "fn id<T>(x: T) -> T { x } fn main() { id::<i32, bool>(1); }",
            "E0205",
        ),
        (
            "fn id<T>(x: T) -> T { x } fn main() { id::<i32>(true); }",
            "E0301",
        ),
        (
            "fn id<T>(x: T) -> T { x } fn main() { id::<void>(1); }",
            "E0301",
        ),
        (
            "fn id<T>(x: T) -> T { x } fn main() { id::<>(1); }",
            "E0102",
        ),
        ("fn id<T>(x: T) -> T { 1 } fn main() {}", "E0301"),
        ("fn equal<T>(x: T) -> bool { x == x } fn main() {}", "E0302"),
        (
            "fn stop() {} fn log<T>(x: T) { System.Console.WriteLine(stop()); } fn main() {}",
            "E0205",
        ),
    ] {
        let errors = oxel::check(source).expect_err(source);
        assert_eq!(errors[0].code, code, "{source}: {errors:?}");
    }
    let diagnostic = oxel::check("fn id<T>(x: T) -> T { true } fn main() {}")
        .unwrap_err()
        .remove(0);
    assert_eq!(diagnostic.type_names(), Some(("T", "System.Boolean")));
}

#[test]
fn generic_structs_have_clr_value_semantics() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("generic_structs.oxel");
    let source = include_str!("../examples/generic_structs.oxel");
    fs::write(&path, source).unwrap();
    assert_eq!(stdout(cli(folder.path(), "run", &path)), "30\noxel\n");
    assert_eq!(
        oxel::compile(source, "structs").unwrap(),
        oxel::compile(source, "structs").unwrap()
    );

    let source = r#"
        struct Cell<T> { value: T }
        struct Marker<T> { value: i32 }
        struct Recursive<T> { marker: Marker<Recursive<T>> }
        fn main() {
            let mut original: Cell<Cell<i32>>= Cell { value: Cell { value: 30 } };
            let copy = original;
            original = Cell { value: Cell { value: 40 } };
            System.Console.WriteLine(copy.value.value);
            System.Console.WriteLine(original.value.value);
            let finite = Recursive::<i32> { marker: Marker { value: 42 } };
            System.Console.WriteLine(finite.marker.value);
        }
    "#;
    fs::write(&path, source).unwrap();
    assert_eq!(stdout(cli(folder.path(), "run", &path)), "30\n40\n42\n");
}

#[test]
fn rejects_invalid_generic_struct_instantiations_and_layouts() {
    for (source, code) in [
        ("struct Box<T, T> {} fn main() {}", "E0201"),
        ("struct Box<i32> {} fn main() {}", "E0201"),
        (
            "struct Box<T> { x: T } fn f(x: Box) {} fn main() {}",
            "E0205",
        ),
        (
            "struct Box<T> { x: T } fn f(x: Box<i32, bool>) {} fn main() {}",
            "E0205",
        ),
        (
            "struct Box<T> { x: T } fn f(x: Box<void>) {} fn main() {}",
            "E0301",
        ),
        ("struct Box<T> {} fn main() { Box {}; }", "E0205"),
        (
            "struct Box<T> { x: T } fn main() { Box::<i32> { x: true }; }",
            "E0301",
        ),
        (
            "struct Box<T> { x: T } fn main() { Box::<i32> {}; }",
            "E0306",
        ),
        (
            "struct Box<T> { x: T } fn f(x: Box<i32>) -> Box<bool> { x } fn main() {}",
            "E0301",
        ),
        ("struct Box<T> { x: Box<T> } fn main() {}", "E0304"),
        (
            "struct Box<T> { x: T } struct Recursive { x: Box<Recursive> } fn main() {}",
            "E0304",
        ),
        (
            "struct A<T> { x: B<T> } struct B<T> { x: A<T> } fn main() {}",
            "E0304",
        ),
        (
            "struct Box<T> { x: T } fn main() { System.Console.WriteLine(System.Console.WriteLine(Box::<i32> { x: 1 })); }",
            "E0205",
        ),
    ] {
        let errors = oxel::check(source).expect_err(source);
        assert_eq!(errors[0].code, code, "{source}: {errors:?}");
    }

    let errors =
        oxel::check("struct Box<T> { x: T } fn f(x: Box<i32>) -> Box<bool> { x } fn main() {}")
            .unwrap_err();
    assert_eq!(
        errors[0].type_names(),
        Some(("Box<System.Boolean>", "Box<System.Int32>"))
    );
}

#[test]
fn infers_generics_from_argument_shapes_and_result_context() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("inference.oxel");
    fs::write(
        &path,
        r#"
        struct Container<T> { value: T }
        struct Marker<T> { stamp: i32 }

        fn first<T>(first: T, second: T) -> T { first }
        fn wrap<T>(value: T) -> Container<T> { Container { value: value } }
        fn unwrap<T>(container: Container<T>) -> T { container.value }
        fn marker<T>() -> Marker<T> { Marker { stamp: 4 } }
        fn tag_as_i32() -> Marker<i32> { Marker { stamp: 5 } }
        fn same_nested<T>(a: Container<Container<T>>, b: T) -> T { a.value.value }
        fn probe(n: i32) -> i32 { System.Console.WriteLine(n); n }

        fn main() {
            let wide: i64 = 9000000000;
            System.Console.WriteLine(first(3000000000, wide));
            System.Console.WriteLine(first(wide, 3000000000));
            let tag: Marker<bool> = marker();
            System.Console.WriteLine(tag.stamp);
            let nested: Marker<i32> = first({
                let initialized: i32;
                initialized = 3;
                let _ = initialized;
                marker()
            }, tag_as_i32());
            System.Console.WriteLine(nested.stamp);
            System.Console.WriteLine(unwrap(wrap(30)));
            let x: i64 = unwrap(Container { value: 4000000000 });
            System.Console.WriteLine(x);
            let y = same_nested(Container { value: Container { value: 9 } }, 12);
            System.Console.WriteLine(y);
            first(probe(1), probe(2));
        }
        "#,
    )
    .unwrap();
    assert_eq!(
        stdout(cli(folder.path(), "run", &path)),
        "3000000000
9000000000
4
4
30
4000000000
9
1
2
"
    );
}

#[test]
fn reports_conflicting_and_missing_inferred_type_arguments() {
    for (source, message) in [
        (
            "fn f<T>(a: T, b: T) {} fn main() { f(true, \"text\"); }",
            "conflicting types for 'T'",
        ),
        (
            "fn f<T, U>(value: T) {} fn main() { f(false); }",
            "cannot infer type parameter 'U'",
        ),
        (
            "struct Box<T> { value: T } fn f<T>(a: Box<T>, b: Box<T>) {} fn main() { f(Box::<i32> { value: 1 }, Box::<bool> { value: false }); }",
            "conflicting types for 'T'",
        ),
    ] {
        let errors = oxel::check(source).expect_err(source);
        assert_eq!(errors[0].code, "E0210", "{source}: {errors:?}");
        assert!(errors[0].message.contains(message), "{errors:?}");
    }
}

#[test]
fn floating_point_operations_preserve_width_and_ieee_comparisons() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("floats.oxel");
    fs::write(&path, include_str!("../examples/floats.oxel")).unwrap();
    let output = stdout(cli(folder.path(), "run", &path));
    assert_eq!(output.lines().collect::<Vec<_>>(), ["25", "25", "True"]);

    fs::write(
        &path,
        r#"
        fn rounded(value: System.Single) -> f32 { (value + 1.0) - value }
        fn divide(a: f64, b: f64) -> System.Double { a / b }
        fn same<T>(a: T, b: T) -> T { a }
        fn main() {
            let one: f32 = 1.0;
            let nan = divide(0.0, 0.0);
            System.Console.WriteLine(!(nan < 0.0 || nan <= 0.0 || nan > 0.0 || nan >= 0.0 || nan == nan));
            System.Console.WriteLine(nan != nan);
            System.Console.WriteLine(divide(1.0, -0.0) < -1e300);
            System.Console.WriteLine(rounded(16777216.0) == 0.0);
            System.Console.WriteLine(same(1.0000000596046448, one) > one);
            System.Console.WriteLine(-5.5 % 2.0 == -1.5);
            System.Console.WriteLine(-(1e-2) == -0.01);
            System.Console.WriteLine(1.125E+3 == 1125.0);
        }
        "#,
    )
    .unwrap();
    assert_eq!(
        stdout(cli(folder.path(), "run", &path))
            .lines()
            .collect::<Vec<_>>(),
        ["True"; 8]
    );
}

#[test]
fn diagnoses_floating_point_literal_and_type_errors() {
    for (source, code) in [
        ("fn main() { 1e-; }", "E0106"),
        ("fn main() { 1.0e9999; }", "E0303"),
        ("fn main() { let n: f32 = 1.0e39; }", "E0303"),
        ("fn main() { let n: f64 = 1; }", "E0301"),
        (
            "fn main() { let a: f32 = 0.5; let b: f64 = 0.5; a + b; }",
            "E0301",
        ),
        ("fn main() { !0.5; }", "E0301"),
    ] {
        let errors = oxel::check(source).expect_err(source);
        assert_eq!(errors[0].code, code, "{source}: {errors:?}");
    }
}

#[test]
fn small_integer_operations_use_the_declared_width() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("integers.oxel");
    fs::write(&path, include_str!("../examples/integers.oxel")).unwrap();
    assert_eq!(
        stdout(cli(folder.path(), "run", &path))
            .lines()
            .collect::<Vec<_>>(),
        ["-128", "4", "-32768"]
    );

    fs::write(
        &path,
        r#"
        fn divide(value: i8) -> i8 { value / -1 }
        fn first<T>(a: T, b: T) -> T { a }
        fn main() {
            let signed: System.SByte = -128;
            let byte: System.Byte = 255;
            let zero: u8 = 0;
            System.Console.WriteLine(divide(signed));
            System.Console.WriteLine(-signed);
            System.Console.WriteLine(zero - 1);
            System.Console.WriteLine(byte > zero);
            System.Console.WriteLine(first(255, byte));
        }
        "#,
    )
    .unwrap();
    assert_eq!(
        stdout(cli(folder.path(), "run", &path))
            .lines()
            .collect::<Vec<_>>(),
        ["-128", "-128", "255", "True", "255"]
    );

    for (source, code) in [
        ("fn main() { let x: i8 = -129; }", "E0303"),
        ("fn main() { let x: u8 = -1; }", "E0303"),
        ("fn main() { let x: i16 = 32768; }", "E0303"),
        ("fn main() { let x: u8 = 1; -x; }", "E0302"),
        ("fn main() { let x: i16 = 1; let y: i32 = x; }", "E0301"),
    ] {
        let errors = oxel::check(source).expect_err(source);
        assert_eq!(errors[0].code, code, "{source}: {errors:?}");
    }
}

#[test]
fn boxes_casts_and_tests_clr_objects() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("objects.oxel");
    fs::write(&path, include_str!("../examples/objects.oxel")).unwrap();
    assert_eq!(
        stdout(cli(folder.path(), "run", &path))
            .lines()
            .collect::<Vec<_>>(),
        ["30", "oxel on CLR", "False", "True", "10"]
    );

    let source = r#"
        fn id<T>(value: T) -> T { value }
        fn log<T>(value: T) { System.Console.WriteLine(value); }
        fn main() {
            let narrow: u8 = 255;
            let boxed: object = narrow;
            System.Console.WriteLine(boxed is u8 && !(boxed is i32));
            System.Console.WriteLine(boxed as u8);
            System.Console.WriteLine(1 is i32);
            let mixed: object = if false { narrow } else { true };
            System.Console.WriteLine(mixed is bool);
            let value: object = id(1);
            System.Console.WriteLine(value is i32);
            log(30);
        }
    "#;
    fs::write(&path, source).unwrap();
    assert_eq!(
        stdout(cli(folder.path(), "run", &path))
            .lines()
            .collect::<Vec<_>>(),
        ["True", "255", "True", "True", "True", "30"]
    );
}

#[test]
fn preserves_clr_cast_checks_after_an_object_upcast() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("bad_cast.oxel");
    fs::write(
        &path,
        "fn main() { let value: object = 1; let restored: object = value as string; System.Console.WriteLine(restored); }",
    ).unwrap();
    let result = cli(folder.path(), "run", &path);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("System.InvalidCastException"));

    for (source, code) in [
        ("fn main() { true as i64; }", "E0310"),
        (
            "fn main() { let value: object = 10; value as void; }",
            "E0301",
        ),
        ("fn empty() {} fn main() { empty() is object; }", "E0301"),
        ("const BAD: object = 10; fn main() {}", "E0501"),
        (r#"const BAD: object = "text"; fn main() {}"#, "E0501"),
    ] {
        let errors = oxel::check(source).expect_err(source);
        assert_eq!(errors[0].code, code, "{source}: {errors:?}");
    }
}

#[test]
fn nullable_references_refine_and_coalesce_lazily() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("nullable.oxel");
    fs::write(&path, include_str!("../examples/nullable.oxel")).unwrap();
    assert_eq!(
        stdout(cli(folder.path(), "run", &path))
            .lines()
            .collect::<Vec<_>>(),
        ["Hello, guest", "Hello, oxel", "oxel"]
    );

    let source = r#"
        struct Row { value: string? }
        fn echo(value: string?) -> string? { value }
        fn mark(label: string, value: string?) -> string? {
            System.Console.WriteLine(label);
            value
        }
        fn stop() -> string { "unreachable fallback" }
        fn main() {
            let row = Row { value: echo(null) };
            System.Console.WriteLine(row.value == null);
            let known: string = mark("left", "ok") ?? mark("bad", null) ?? "fallback";
            System.Console.WriteLine(known);
            System.Console.WriteLine(mark("missing", null) ?? mark("right", "provided") ?? stop());
            let merged = if true { "value" } else { null };
            System.Console.WriteLine(merged ?? "other");
            let erased: object? = null;
            let cast = erased as string?;
            System.Console.WriteLine(cast is string);
            let restored = erased ?? 30;
            System.Console.WriteLine(restored as i32);
            let text = null ?? "text";
            System.Console.WriteLine(text);
            let a: string? = "alpha";
            let b: string? = "-beta";
            if a != null && b != null && (a + b) == "alpha-beta" {
                System.Console.WriteLine(a + b);
            }
            if a == null || b == null {
                System.Console.WriteLine("bad presence check");
            } else {
                System.Console.WriteLine(a + b);
            }
            while !(a == null) {
                System.Console.WriteLine(a);
                break;
            }
        }
    "#;
    fs::write(&path, source).unwrap();
    assert_eq!(
        stdout(cli(folder.path(), "run", &path))
            .lines()
            .collect::<Vec<_>>(),
        [
            "True",
            "left",
            "ok",
            "missing",
            "right",
            "provided",
            "value",
            "False",
            "30",
            "text",
            "alpha-beta",
            "alpha-beta",
            "alpha"
        ]
    );
}

#[test]
fn nullable_references_refine_after_diverging_guards() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("guards.oxel");
    let source = r#"
        fn direct(text: string?) -> string {
            if text == null { return "absent"; }
            text
        }
        fn joined(text: string?, flag: bool) -> string {
            if flag {
                if text == null { return "left absent"; }
            } else {
                if text != null {} else { return "right absent"; }
            }
            text
        }
        fn nested(text: string?) -> string {
            {
                if text == null { return "nested absent"; }
            };
            text
        }
        fn initializer(text: string?) -> string {
            let prefix = if text != null { "prefix: " } else { return "no prefix"; };
            prefix + text
        }
        fn loop_guard() {
            for value in 0..2 {
                let text: string? = if value == 0 { null } else { "continue" };
                if text == null { continue; }
                let present: string = text;
                System.Console.WriteLine(present);
            }
        }
        fn main() {
            System.Console.WriteLine(direct("direct"));
            System.Console.WriteLine(direct(null));
            System.Console.WriteLine(joined("both", true));
            System.Console.WriteLine(joined(null, false));
            System.Console.WriteLine(nested("nested"));
            System.Console.WriteLine(initializer("initializer"));
            loop_guard();
        }
    "#;
    fs::write(&path, source).unwrap();
    assert_eq!(
        stdout(cli(folder.path(), "run", &path))
            .lines()
            .collect::<Vec<_>>(),
        [
            "direct",
            "absent",
            "both",
            "right absent",
            "nested",
            "prefix: initializer",
            "continue"
        ]
    );

    for declaration in [
        r#"fn f(text: string?) -> string { if text == null {} text }"#,
        r#"fn f(text: string?, flag: bool) -> string {
            if flag { if text == null { return "absent"; } } text
        }"#,
        r#"fn f(text: string?) -> string { if text != null { return "present"; } text }"#,
        r#"fn f(text: string?) -> string { while text == null { break; } text }"#,
        r#"fn f(mutator: string?) -> string {
            let mut text = mutator; if text == null { return "absent"; } text
        }"#,
    ] {
        let source = format!("{declaration} fn main() {{}}");
        let errors = oxel::check(&source).unwrap_err();
        assert_eq!(errors[0].code, "E0301", "{source}: {errors:?}");
    }
}

#[test]
fn rejects_nullable_references_in_nonnullable_use_sites() {
    for (source, code) in [
        ("fn main() { let value: string = null; }", "E0301"),
        ("fn main() { let value = null; }", "E0311"),
        ("fn main() { let value: i32? = null; }", "E0311"),
        ("fn main() { let value = null ?? 30; }", "E0311"),
        (r#"fn main() { let value = "a" ?? "b"; }"#, "E0311"),
        (
            r#"fn main() { let value: string? = null; value + "!"; }"#,
            "E0302",
        ),
        (
            "fn main() { let value: object? = null; value as string; }",
            "E0310",
        ),
        (
            "fn main() { let value: object? = null; value is string?; }",
            "E0311",
        ),
        (
            r#"fn main() { let mut x: string? = "a"; if x != null { x = null; let y: string = x; } }"#,
            "E0301",
        ),
    ] {
        let errors = oxel::check(source).expect_err(source);
        assert_eq!(errors[0].code, code, "{source}: {errors:?}");
    }
}

#[test]
fn checks_delayed_initialization_through_control_flow() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("assignment.oxel");
    fs::write(&path, include_str!("../examples/assignment.oxel")).unwrap();
    assert_eq!(stdout(cli(folder.path(), "run", &path)).trim(), "30");

    let source = r#"
        fn short_circuit(yes: bool) -> i32 {
            let value: i32;
            if yes && { value = 7; true } {
                return value;
            }
            0
        }

        fn leave_outer() -> i32 {
            let value: i32;
            while true {
                while { value = 8; break; } {}
            }
            value
        }

        fn fallback(text: string?) -> i32 {
            let mut value: i32;
            let _ = text ?? { value = 14; "fallback" };
            if text == null {
                value = 14;
            } else {
                value = 15;
            }
            value
        }

        fn main() {
            System.Console.WriteLine(short_circuit(true));
            System.Console.WriteLine(short_circuit(false));
            System.Console.WriteLine(leave_outer());
            let mut i = 0;
            while i < 2 {
                let once: i32;
                i = i + 1;
                if i < 2 { continue; }
                once = 5;
                System.Console.WriteLine(once);
            }
            let mut value: i32;
            value = 1;
            value = value + 3;
            System.Console.WriteLine(value);
            let first: i32;
            let total = { first = 3; first } + first;
            System.Console.WriteLine(total);
            System.Console.WriteLine(fallback(null));
            System.Console.WriteLine(fallback("present"));
        }
    "#;
    fs::write(&path, source).unwrap();
    assert_eq!(
        stdout(cli(folder.path(), "run", &path))
            .lines()
            .collect::<Vec<_>>(),
        ["7", "0", "8", "5", "4", "6", "14", "15"]
    );
}

#[test]
fn rejects_reads_and_reassignments_without_pathwise_proof() {
    for (source, code) in [
        ("fn main() { let x; }", "E0320"),
        (
            "fn main() { let x: i32; System.Console.WriteLine(x); }",
            "E0321",
        ),
        ("fn main() { let x: i32; x = x + 1; }", "E0321"),
        ("fn main() { let x: i32; x = { x = 1; 2 }; }", "E0322"),
        ("fn main() { let x: i32; x = 1; x = 2; }", "E0322"),
        (
            "fn f(flag: bool) { let x: i32; if flag { x = 1; } x = 2; } fn main() {}",
            "E0322",
        ),
        (
            "fn f(flag: bool) { let x: i32; if flag { x = 1; } System.Console.WriteLine(x); } fn main() {}",
            "E0321",
        ),
        (
            "fn f(flag: bool) { let mut x: i32; while flag { x = 1; } System.Console.WriteLine(x); } fn main() {}",
            "E0321",
        ),
        (
            "fn f(flag: bool) { let x: i32; while flag { x = 1; } } fn main() {}",
            "E0322",
        ),
        (
            "fn f(flag: bool) { let x: i32; while true { if flag { break; } x = 1; break; } System.Console.WriteLine(x); } fn main() {}",
            "E0321",
        ),
        (
            "fn f(flag: bool) { let x: i32; while flag { x = 1; continue; } } fn main() {}",
            "E0322",
        ),
        (
            r#"fn f(text: string?) { let x: i32; let _ = text ?? { x = 1; "fallback" }; System.Console.WriteLine(x); } fn main() {}"#,
            "E0321",
        ),
        ("fn main() { let x: string?; if x != null {} }", "E0321"),
        (
            "fn f(flag: bool) { let x: i32; let _ = flag && { x = 1; true }; System.Console.WriteLine(x); } fn main() {}",
            "E0321",
        ),
    ] {
        let errors = oxel::check(source).expect_err(source);
        assert_eq!(errors[0].code, code, "{source}: {errors:?}");
    }
}

#[test]
fn discriminated_unions_execute_and_preserve_match_control_flow() {
    let folder = tempfile::tempdir().unwrap();
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/unions.oxel");
    assert_eq!(
        stdout(cli(folder.path(), "run", &source))
            .lines()
            .collect::<Vec<_>>(),
        ["30", "unavailable"]
    );
    let source = r#"
        enum Command { Add(i32, i32), Stop, Reset }
        fn value(tag: Command) -> i32 {
            let assigned: i32;
            if match tag {
                Command.Add(a, _) => { assigned = a; true },
                _ => false,
            } {
                return assigned;
            }
            1 + match tag {
                Stop => { return 9; },
                _ => 2,
            }
        }
        fn exit(tag: Command) -> i32 {
            1 + match tag { Add(a, _) => { return a; }, _ => { return 0; } }
        }
        fn effect() -> Command {
            System.Console.WriteLine("scrutinee");
            Command.Add(10, 20)
        }
        fn main() {
            System.Console.WriteLine(value(effect()));
            System.Console.WriteLine(value(Command.Stop));
            System.Console.WriteLine(value(Command.Reset));
            System.Console.WriteLine(exit(Command.Add(20, 40)));
        }
    "#;
    let path = folder.path().join("unions.oxel");
    fs::write(&path, source).unwrap();
    assert_eq!(
        stdout(cli(folder.path(), "run", &path))
            .lines()
            .collect::<Vec<_>>(),
        ["scrutinee", "10", "9", "3", "20"]
    );
    assert_eq!(
        oxel::compile(source, "unions").unwrap(),
        oxel::compile(source, "unions").unwrap(),
    );
}

#[test]
fn rejects_invalid_discriminated_unions_and_match_arms() {
    let declaration = "enum Command { Add(i32, i32), Stop, Reset }";
    for (source, code, message) in [
        (
            "enum Empty {} fn main() {}",
            "E0330",
            "at least one variant",
        ),
        ("enum E { X, X } fn main() {}", "E0330", "duplicate variant"),
        ("enum E { X(E) } fn main() {}", "E0304", "recursive"),
        ("fn main() { Command.Add(1); }", "E0330", "expects 2"),
        (
            "fn main() { Command.Add(true, 1); }",
            "E0301",
            "System.Int32",
        ),
        (
            "fn main() { Command {}; }",
            "E0330",
            "using one of its variants",
        ),
        (
            "fn main() { Command.Unknown; }",
            "E0330",
            "unknown enum variant",
        ),
        (
            "fn main() { Command.Stop.$tag; }",
            "E0101",
            "unexpected character",
        ),
        (
            "fn main() { match 1.0 { _ => {} } }",
            "E0331",
            "requires an enum",
        ),
        (
            "fn main() { match Command.Stop { Stop => {} } }",
            "E0331",
            "missing Add, Reset",
        ),
        (
            "fn main() { match Command.Stop { Stop => {}, Stop => {}, _ => {} } }",
            "E0331",
            "duplicate match arm",
        ),
        (
            "fn main() { match Command.Stop { _ => {}, Stop => {} } }",
            "E0331",
            "wildcard must",
        ),
        (
            "fn main() { match Command.Stop { Wrong.Stop => {}, _ => {} } }",
            "E0331",
            "expected variant of 'Command'",
        ),
        (
            "fn main() { match Command.Stop { Add(a) => {}, _ => {} } }",
            "E0331",
            "requires 2",
        ),
        (
            "fn main() { match Command.Stop { Add(a, a) => {}, _ => {} } }",
            "E0331",
            "duplicate pattern binding",
        ),
        (
            "fn main() { match Command.Stop { Add(a, _) => { a; }, _ => { a; } } }",
            "E0203",
            "unknown variable",
        ),
        (
            "fn main() { let x: i32; match Command.Stop { Stop => { x = 1; }, _ => {} }; x; }",
            "E0321",
            "may be unassigned",
        ),
        (
            "fn main() { let a = match Command.Stop { Stop => 3, _ => true }; }",
            "E0301",
            "System.Int32",
        ),
        (
            "fn main() { match Command.Stop { Stop -> 0, _ => 1 }; }",
            "E0102",
            "'=>'",
        ),
    ] {
        let source = format!("{declaration}\n{source}");
        let errors = oxel::compile(&source, "invalid").unwrap_err();
        assert_eq!(errors[0].code, code, "{source}: {errors:?}");
        assert!(errors[0].message.contains(message), "{source}: {errors:?}");
    }
}

#[test]
fn nested_patterns_dispatch_in_source_order_and_bind_active_payloads() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("nested.oxel");
    let source = r#"
        enum Maybe<T> { Some(T), None }
        enum Pair { Values(Maybe<i32>, Maybe<i32>), Stop }
        fn number(pair: Pair) -> i32 {
            let result: i32;
            match pair {
                Values(Some(left), Some(right)) => { result = left + right; },
                Values(_, Some(right)) => { result = right; },
                Values(Some(left), _) => { return left; },
                Values(Maybe.None, None()) => { result = 0; },
                Stop => { return -1; },
            }
            result
        }
        fn choose(a: bool, b: bool) -> i32 {
            match a {
                true => 1 + match b { true => 2, false => 3 },
                false => 0,
            }
        }
        fn main() {
            System.Console.WriteLine(number(Pair.Values(Maybe.Some(10), Maybe.Some(20))));
            System.Console.WriteLine(number(Pair.Values(Maybe.None, Maybe.Some(20))));
            System.Console.WriteLine(number(Pair.Values(Maybe.Some(10), Maybe.None)));
            System.Console.WriteLine(number(Pair.Values(Maybe.None, Maybe.None)));
            System.Console.WriteLine(number(Pair.Stop));
            System.Console.WriteLine(choose(true, false));
        }
    "#;
    fs::write(&path, source).unwrap();
    assert_eq!(
        stdout(cli(folder.path(), "run", &path))
            .lines()
            .collect::<Vec<_>>(),
        ["30", "20", "10", "0", "-1", "4"],
    );
    assert_eq!(
        oxel::compile(source, "nested").unwrap(),
        oxel::compile(source, "nested").unwrap(),
    );
}

#[test]
fn nested_pattern_coverage_reports_holes_and_subsumed_arms() {
    let declarations = "enum Flag { On, Off } enum Pair { Values(Flag, Flag) }";
    for (body, code, message) in [
        (
            "Values(Flag.On, _) => 1, Values(_, Flag.On) => 2",
            "E0331",
            "non-exhaustive match, missing Values (some payload combinations)",
        ),
        (
            "Values(Flag.On, _) => 1, Values(Flag.Off, _) => 2, Values(_, Flag.On) => 3",
            "E0331",
            "pattern already covered",
        ),
        (
            "Values(Flag.On, Flag.On) => 1, Values(Flag.On, Flag.On) => 2, Values(_, _) => 3",
            "E0331",
            "duplicate match arm",
        ),
        (
            "Values(Wrong.On, _) => 1",
            "E0331",
            "expected variant of 'Flag'",
        ),
        ("Values(true, _) => 1", "E0301", "System.Boolean"),
    ] {
        let source = format!(
            "{declarations} fn inspect(x: Pair) -> i32 {{ match x {{ {body} }} }} fn main() {{}}"
        );
        let errors = oxel::check(&source).unwrap_err();
        assert_eq!(errors[0].code, code, "{source}: {errors:?}");
        assert!(errors[0].message.contains(message), "{source}: {errors:?}");
    }
}

#[test]
fn deeply_nested_patterns_report_a_syntax_diagnostic() {
    let open = "Case(".repeat(1000);
    let close = ")".repeat(1000);
    let source = format!("fn main() {{ match true {{ {open}_{close} => 1 }}; }}");
    let failures = oxel::check(&source).unwrap_err();
    assert_eq!(failures[0].code, "E0103");
}

#[test]
fn try_blocks_capture_the_nearest_failure_and_preserve_surrounding_operands() {
    let folder = tempfile::tempdir().unwrap();
    let example = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/try_blocks.oxel");
    assert_eq!(
        stdout(cli(folder.path(), "run", &example))
            .lines()
            .collect::<Vec<_>>(),
        ["30", "missing value"]
    );
    let source = folder.path().join("nested.oxel");
    fs::write(
        &source,
        r#"
        enum Result<T, E> { Err(E), Ok(T) }

        fn trace(value: i32) -> i32 {
            System.Console.WriteLine(value);
            value
        }

        fn finish(first: i32, result: Result<i32, object>) -> i32 {
            first + match result {
                Ok(value) => value,
                Err(error) => error as i32,
            }
        }

        fn nested(input: Result<i32, i32>) -> i32 {
            let offset: i32;
            let total = finish(trace(10), try {
                offset = 2;
                let value = try { trace(20) + input? };
                trace(30) + value?
            });
            total + offset
        }

        fn main() {
            System.Console.WriteLine(nested(Result.Ok(1)));
            System.Console.WriteLine(nested(Result.Err(7)));
        }
        "#,
    )
    .unwrap();
    assert_eq!(
        stdout(cli(folder.path(), "run", &source))
            .lines()
            .collect::<Vec<_>>(),
        ["10", "20", "30", "63", "10", "20", "30", "19"]
    );
}

#[test]
fn try_blocks_validate_success_types_transfers_and_definite_assignment() {
    let definition = "enum Result<T, E> { Ok(T), Err(E) }";
    for (source, code, message) in [
        ("fn f() { try { 1 }; }", "E0334", "requires a '?'"),
        (
            "fn f(input: Result<i32, string>) { try { input?; }; }",
            "E0334",
            "requires a success value",
        ),
        (
            "fn f(a: Result<i32, string>, b: Result<i32, bool>) { try { a? + b? }; }",
            "E0301",
            "expected System.String, found System.Boolean",
        ),
        (
            "fn f(a: Result<i32, string>) { try { a?; return; }; }",
            "E0334",
            "'return'",
        ),
        (
            "fn f(a: Result<i32, string>) { while true { try { a?; break; }; } }",
            "E0334",
            "loop control",
        ),
        (
            "fn f(a: Result<i32, string>) { let x: i32; try { a?; x = 10; x }; System.Console.WriteLine(x); }",
            "E0321",
            "binding 'x' may be unassigned",
        ),
        (
            "fn f(a: Result<i32, string>) { let x: i32; try { x = 10; a? }; x = 20; }",
            "E0322",
            "binding 'x' may already be assigned",
        ),
    ] {
        let source = format!("{definition}\n{source}\nfn main() {{}}");
        let errors = oxel::check(&source).unwrap_err();
        assert_eq!(errors[0].code, code, "{source}: {errors:?}");
        assert!(errors[0].message.contains(message), "{source}: {errors:?}");
    }
}

#[test]
fn option_propagation_wraps_success_and_preserves_nested_result_contexts() {
    let folder = tempfile::tempdir().unwrap();
    let example = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/option_try.oxel");
    assert_eq!(
        stdout(cli(folder.path(), "run", &example))
            .lines()
            .collect::<Vec<_>>(),
        ["Count: 30", "no count"]
    );
    let path = folder.path().join("nested-carriers.oxel");
    fs::write(
        &path,
        r#"
        enum Option<T> { Some(T), None }
        enum Result<T, E> { Err(E), Ok(T) }

        fn score(a: Option<i32>, b: Result<i32, string>) -> Result<i32, string> {
            try {
                let input = try { 20 + a? };
                match input {
                    Some(value) => value + b?,
                    None => 0,
                }
            }
        }

        fn print_score(score: Result<i32, string>) {
            match score {
                Ok(value) => System.Console.WriteLine(value),
                Err(message) => System.Console.WriteLine(message),
            };
        }

        fn main() {
            print_score(score(Option.Some(1), Result.Ok(9)));
            print_score(score(Option.None, Result.Err("unused")));
            print_score(score(Option.Some(1), Result.Err("missing")));
        }
        "#,
    )
    .unwrap();
    assert_eq!(
        stdout(cli(folder.path(), "run", &path))
            .lines()
            .collect::<Vec<_>>(),
        ["30", "0", "missing"]
    );
}

#[test]
fn dense_literal_matches_keep_guards_and_defaults() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("dense.oxel");
    let source = r#"
        fn signed(value: i32, guard: bool) -> i32 {
            10 + match value {
                1 => 4,
                -3 => 1,
                -2 if guard => 2,
                -2 => 3,
                2 => 5,
                _ => 0,
            }
        }
        fn unsigned(value: u32) -> i32 {
            match value {
                2147483649 => 4,
                2147483647 => 2,
                2147483646 => 1,
                2147483648 => 3,
                _ => 0,
            }
        }
        fn character(value: Option<char>) -> i32 {
            match value {
                Some('a') => 1,
                Some('b') => 2,
                Some('d') => 3,
                Some('e') => 4,
                Some(_) => 5,
                None => 6,
            }
        }
        fn main() {
            System.Console.WriteLine(signed(-2147483648, true));
            System.Console.WriteLine(signed(-3, false));
            System.Console.WriteLine(signed(-2, true));
            System.Console.WriteLine(signed(-2, false));
            System.Console.WriteLine(signed(0, true));
            System.Console.WriteLine(signed(2, true));
            System.Console.WriteLine(signed(2147483647, true));
            System.Console.WriteLine(unsigned(0));
            System.Console.WriteLine(unsigned(2147483647));
            System.Console.WriteLine(unsigned(2147483648));
            System.Console.WriteLine(unsigned(2147483649));
            System.Console.WriteLine(unsigned(4294967295));
            System.Console.WriteLine(character(Option.Some('b')));
            System.Console.WriteLine(character(Option.Some('c')));
            System.Console.WriteLine(character(Option.None));
        }
    "#;
    fs::write(&path, source).unwrap();
    assert_eq!(
        stdout(cli(folder.path(), "run", &path))
            .lines()
            .collect::<Vec<_>>(),
        [
            "10", "11", "12", "13", "10", "15", "10", "0", "2", "3", "4", "0", "2", "5", "6"
        ]
    );
}

#[test]
fn literal_patterns_match_widths_and_check_scalar_coverage() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("literal_patterns.oxel");
    fs::write(&path, include_str!("../examples/literal_patterns.oxel")).unwrap();
    assert_eq!(
        stdout(cli(folder.path(), "run", &path))
            .lines()
            .collect::<Vec<_>>(),
        [
            "cache hit",
            "not found",
            "disconnected",
            "matched character"
        ]
    );

    let source = r#"
        fn widest(value: u64, flag: bool) -> i32 {
            match value {
                0xffff_ffff_ffff_ffff if flag => 1,
                18446744073709551615 => 2,
                _ => 0,
            }
        }
        fn narrow(value: i8) -> i32 {
            let -128 = value else { return 0; };
            8
        }
        fn text(input: Option<string?>) -> i32 {
            match input {
                Some("line\n") => 3,
                Some("Case") => 4,
                _ => 5,
            }
        }
        fn main() {
            System.Console.WriteLine(widest(18446744073709551615, false));
            System.Console.WriteLine(widest(18446744073709551615, true));
            System.Console.WriteLine(widest(10, true));
            System.Console.WriteLine(narrow(-128));
            System.Console.WriteLine(narrow(127));
            System.Console.WriteLine(text(Option.Some("line\n")));
            System.Console.WriteLine(text(Option.Some("case")));
            System.Console.WriteLine(text(Option.Some(null)));
        }
    "#;
    fs::write(&path, source).unwrap();
    assert_eq!(
        stdout(cli(folder.path(), "run", &path))
            .lines()
            .collect::<Vec<_>>(),
        ["2", "1", "0", "8", "0", "3", "5", "5"]
    );

    let arms = (0..=255)
        .map(|value| format!("{value} => {value},"))
        .collect::<String>();
    let complete = format!(
        "fn f(value: u8) -> i32 {{ match value {{ {arms} }} }} fn main() {{ System.Console.WriteLine(f(255)); }}"
    );
    fs::write(&path, complete).unwrap();
    assert_eq!(stdout(cli(folder.path(), "run", &path)).trim(), "255");

    for (declaration, code, message) in [
        (
            "fn f(v: i32) -> i32 { match v { 1 => 0 } }",
            "E0331",
            "other values",
        ),
        (
            "fn f(v: string) -> i32 { match v { \"a\" => 0 } }",
            "E0331",
            "other values",
        ),
        (
            "fn f(v: i32) -> i32 { match v { 0xa => 0, 10 => 1, _ => 2 } }",
            "E0331",
            "duplicate",
        ),
        (
            "fn f(v: string) -> i32 { match v { \"a\" => 0, \"a\" => 1, _ => 2 } }",
            "E0331",
            "duplicate",
        ),
        (
            "fn f(v: u8) -> i32 { match v { 256 => 0, _ => 1 } }",
            "E0303",
            "out of range",
        ),
        (
            "fn f(v: string) -> i32 { match v { 1 => 0, _ => 1 } }",
            "E0331",
            "require an integer",
        ),
        (
            "fn f(v: i32) { let x: i32; match v { _ if { x = 1; false } => {}, _ => { x = 2; } }; }",
            "E0322",
            "may already be assigned",
        ),
    ] {
        let source = format!("{declaration} fn main() {{}}");
        let errors = oxel::check(&source).expect_err(&source);
        assert_eq!(errors[0].code, code, "{source}: {errors:?}");
        assert!(errors[0].message.contains(message), "{source}: {errors:?}");
    }
}

#[test]
fn pattern_alternatives_share_bindings_and_match_in_order() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("alternatives.oxel");
    fs::write(&path, include_str!("../examples/pattern_alternatives.oxel")).unwrap();
    assert_eq!(
        stdout(cli(folder.path(), "run", &path))
            .lines()
            .collect::<Vec<_>>(),
        ["success", "denied", "42", "30", "command"]
    );

    let source = r#"
        enum Choice { Pair(i32, i32), Single(i32), End }
        enum Sample { Reading(i32, i32, i32), Missing }

        fn first(input: Choice) -> i32 {
            let Pair(value, _) | Pair(_, value) | Single(value) = input else { return -1; };
            value
        }

        fn guard(input: Choice) -> i32 {
            let assigned: i32;
            match input {
                Pair(value, _) | Pair(_, value) | Single(value) if { assigned = value; value > 0 } => assigned,
                _ => -1,
            }
        }

        fn next(value: i32) -> Choice {
            match value {
                0 => Choice.Pair(0, 10),
                1 => Choice.Single(11),
                _ => Choice.End,
            }
        }

        fn reading(input: Sample) -> i32 {
            match input {
                Reading(1 | 2, 3 | 4, value) => value,
                _ => -1,
            }
        }

        fn main() {
            System.Console.WriteLine(first(Choice.Pair(3, 4)));
            System.Console.WriteLine(first(Choice.Single(5)));
            System.Console.WriteLine(first(Choice.End));
            System.Console.WriteLine(guard(Choice.Pair(0, 42)));
            System.Console.WriteLine(guard(Choice.Pair(7, 42)));
            System.Console.WriteLine(guard(Choice.Single(8)));
            let mut i = 0;
            while let Pair(_, value) | Single(value) = next(i) {
                System.Console.WriteLine(value);
                i += 1;
            }
            System.Console.WriteLine(match Option.Some::<bool>(true) { Some((false | true)) | None => 6 });
            System.Console.WriteLine(reading(Sample.Reading(1, 4, 13)));
            System.Console.WriteLine(reading(Sample.Reading(2, 3, 14)));
            System.Console.WriteLine(reading(Sample.Reading(2, 5, 15)));
        }
    "#;
    fs::write(&path, source).unwrap();
    assert_eq!(
        stdout(cli(folder.path(), "run", &path))
            .lines()
            .collect::<Vec<_>>(),
        [
            "3", "5", "-1", "-1", "7", "8", "10", "11", "6", "13", "14", "-1"
        ]
    );

    for (declaration, message) in [
        (
            "fn f(x: i32) -> i32 { match x { 1 | 2 => 0 } }",
            "other values",
        ),
        (
            "fn f(x: i32) -> i32 { match x { 1 | 2 => 0, 2 => 1, _ => 3 } }",
            "duplicate",
        ),
        (
            "fn f(x: Option<i32>) -> i32 { match x { Some(value) | None => 0 } }",
            "same names",
        ),
        (
            "enum E { A(i32), B(bool) } fn f(x: E) -> i32 { match x { A(value) | B(value) => 0 } }",
            "same types",
        ),
        (
            "enum E { A(i32, i32), B(i32, i32) } fn f(x: E) -> i32 { match x { A(a, b) | B(a, a) => 0 } }",
            "duplicate pattern binding",
        ),
        (
            "fn f(x: bool) { if let true | false = x {} }",
            "always matches",
        ),
    ] {
        let source = format!("{declaration} fn main() {{}}");
        let errors = oxel::check(&source).expect_err(&source);
        assert!(errors[0].message.contains(message), "{source}: {errors:?}");
    }

    let fields = ["bool"; 24].join(", ");
    let choices = ["false | true"; 24].join(", ");
    let source = format!(
        "enum Wide {{ Bits({fields}) }} fn f(x: Wide) -> i32 {{ match x {{ Bits({choices}) => 0 }} }} fn main() {{}}"
    );
    let errors = oxel::check(&source).unwrap_err();
    assert_eq!(errors[0].code, "E0332");
}

#[test]
fn match_guards_resume_lazily_and_preserve_assignment_effects() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("match_guards.oxel");
    fs::write(&path, include_str!("../examples/match_guards.oxel")).unwrap();
    assert_eq!(
        stdout(cli(folder.path(), "run", &path))
            .lines()
            .collect::<Vec<_>>(),
        [
            "positive?",
            "positive: 30",
            "positive?",
            "zero?",
            "zero",
            "positive?",
            "zero?",
            "negative: -2",
            "missing"
        ]
    );

    let source = r#"
        enum Result<T, E> { Ok(T), Err(E) }
        fn result(input: Result<Option<i32>, string>) -> i32 {
            match input {
                Ok(Some(value)) if value > 30 => 1,
                Ok(Some(value)) => value,
                Ok(None()) => 2,
                Err(_) => 3,
            }
        }
        fn defined(flag: bool) -> i32 {
            let value: i32;
            match flag {
                _ if { value = 7; flag } => value,
                _ => value + 10,
            }
        }
        fn early(flag: bool) -> i32 {
            match flag {
                true if { return 8; } => 1,
                _ => 9,
            }
        }
        fn label(input: Option<string?>) -> string {
            match input {
                Some(value) if value != null => value,
                _ => "empty",
            }
        }
        fn main() {
            System.Console.WriteLine(result(Result.Ok(Option.Some(30))));
            System.Console.WriteLine(result(Result.Ok(Option.None)));
            System.Console.WriteLine(defined(true));
            System.Console.WriteLine(defined(false));
            System.Console.WriteLine(early(true));
            System.Console.WriteLine(early(false));
            System.Console.WriteLine(label(Option.Some("value")));
            System.Console.WriteLine(label(Option.Some(null)));
        }
    "#;
    fs::write(&path, source).unwrap();
    assert_eq!(
        stdout(cli(folder.path(), "run", &path))
            .lines()
            .collect::<Vec<_>>(),
        ["30", "2", "7", "17", "8", "9", "value", "empty"]
    );

    for (declaration, code) in [
        (
            "fn f(input: Option<i32>) { match input { Some(x) if x > 0 => {}, None => {}, }; }",
            "E0331",
        ),
        (
            "fn f(input: Option<i32>) { match input { Some(_) => {}, Some(x) if x > 0 => {}, None => {}, }; }",
            "E0331",
        ),
        (
            "fn f(input: Option<i32>) { match input { Some(x) if x => {}, _ => {}, }; }",
            "E0301",
        ),
        (
            "fn f(flag: bool) { let value: i32; match flag { _ if { value = 1; false } => {}, _ => { value = 2; }, }; }",
            "E0322",
        ),
    ] {
        let source = format!("{declaration} fn main() {{}}");
        let errors = oxel::check(&source).expect_err(&source);
        assert_eq!(errors[0].code, code, "{source}: {errors:?}");
    }
}

#[test]
fn pattern_conditions_bind_on_success_and_scope_each_iteration() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("pattern_conditions.oxel");
    fs::write(&path, include_str!("../examples/pattern_conditions.oxel")).unwrap();
    assert_eq!(
        stdout(cli(folder.path(), "run", &path))
            .lines()
            .collect::<Vec<_>>(),
        ["first: 0", "0", "2", "3", "5"]
    );

    let source = r#"
        fn choose(first: Option<i32>, second: Option<i32>) -> i32 {
            let result: i32;
            if let Some(value) = first {
                result = value;
            } else if let Some(value) = second {
                result = value;
            } else {
                result = 40;
            }
            result
        }

        fn main() {
            System.Console.WriteLine(choose(Option.Some(10), Option.Some(20)));
            System.Console.WriteLine(choose(Option.None, Option.Some(20)));
            System.Console.WriteLine(choose(Option.None, Option.None));
            let mut calls = 0;
            let input: Option<i32> = Option.Some(30);
            while let Some(value) = { calls += 1; input } {
                System.Console.WriteLine(value);
                break;
            }
            System.Console.WriteLine(calls);
        }
    "#;
    fs::write(&path, source).unwrap();
    assert_eq!(
        stdout(cli(folder.path(), "run", &path))
            .lines()
            .collect::<Vec<_>>(),
        ["10", "20", "40", "30", "1"]
    );

    for (declaration, code) in [
        (
            "fn f(input: Option<i32>) { if let Some(x) = input {} else { x; } }",
            "E0203",
        ),
        (
            "fn f(input: Option<i32>) { while let Some(x) = input { break; } x; }",
            "E0203",
        ),
        (
            "fn f(input: Option<i32>) -> i32 { if let Some(x) = input { x } }",
            "E0301",
        ),
        (
            "fn f(input: Option<i32>) { if let Some(a, b) = input {} }",
            "E0331",
        ),
        ("fn f() { while let _ = true {} }", "E0335"),
    ] {
        let source = format!("{declaration} fn main() {{}}");
        let errors = oxel::check(&source).expect_err(&source);
        assert_eq!(errors[0].code, code, "{source}: {errors:?}");
    }
}

#[test]
fn let_else_destructures_once_and_checks_failure_scope_and_control_flow() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("let_else.oxel");
    fs::write(&path, include_str!("../examples/let_else.oxel")).unwrap();
    assert_eq!(
        stdout(cli(folder.path(), "run", &path))
            .lines()
            .collect::<Vec<_>>(),
        ["30", "unavailable", "unavailable", "1", "3"]
    );

    let source = r#"
        fn evaluate(input: Option<i32>) -> Option<i32> {
            System.Console.WriteLine("evaluate");
            input
        }
        fn choose(input: Option<i32>) -> i32 {
            let result: i32;
            let Some(value) = evaluate(input) else {
                result = 7;
                return result;
            };
            result = value;
            result
        }
        fn main() {
            System.Console.WriteLine(choose(Option.Some(30)));
            System.Console.WriteLine(choose(Option.None));
            let None: Option<i32> = Option.None else { return; };
            System.Console.WriteLine("none");
        }
    "#;
    fs::write(&path, source).unwrap();
    assert_eq!(
        stdout(cli(folder.path(), "run", &path))
            .lines()
            .collect::<Vec<_>>(),
        ["evaluate", "30", "evaluate", "7", "none"]
    );

    for (declaration, code) in [
        (
            "fn f(input: Option<i32>) { let Some(value) = input else {}; }",
            "E0335",
        ),
        (
            "fn f(input: Option<i32>) { let Some(value) = input else { System.Console.WriteLine(value); return; }; }",
            "E0203",
        ),
        (
            "fn f(input: Option<i32>) { let Some(value) = input else { return; }; value = 20; }",
            "E0204",
        ),
        (
            "fn f(input: Option<i32>) { let Some(value) = input; }",
            "E0335",
        ),
        (
            "fn f(input: Option<i32>) { let Some(a, b) = input else { return; }; }",
            "E0331",
        ),
        (
            "enum Only { Value(i32) } fn f(input: Only) { let Value(x) = input else { return; }; }",
            "E0335",
        ),
    ] {
        let source = format!("{declaration} fn main() {{}}");
        let errors = oxel::check(&source).expect_err(&source);
        assert_eq!(errors[0].code, code, "{source}: {errors:?}");
    }
}

#[test]
fn option_propagation_checks_carriers_and_failure_paths() {
    let declarations = "enum Option<T> { None, Some(T) } enum Result<T, E> { Ok(T), Err(E) }";
    for (function, code, message) in [
        (
            "fn f(value: Option<i32>) -> Result<i32, string> { Result.Ok(value?) }",
            "E0333",
            "cannot propagate Option to Result",
        ),
        (
            "fn f(value: Result<i32, string>) -> Option<i32> { try { value? } }",
            "E0333",
            "cannot propagate Result to Option",
        ),
        (
            "fn f(a: Option<i32>, b: Result<i32, string>) { try { a? + b? }; }",
            "E0333",
            "cannot propagate Result to Option",
        ),
        (
            "fn f(a: Option<i32>) { let x: i32; try { a?; x = 3; x }; System.Console.WriteLine(x); }",
            "E0321",
            "binding 'x' may be unassigned",
        ),
    ] {
        let source = format!("{declarations}\n{function}\nfn main() {{}}");
        let errors = oxel::check(&source).unwrap_err();
        assert_eq!(errors[0].code, code, "{source}: {errors:?}");
        assert!(errors[0].message.contains(message), "{source}: {errors:?}");
    }
    let incompatible =
        "enum Option<T> { None(T), Some(T) } fn f(a: Option<i32>) { try { a? }; } fn main() {}";
    assert_eq!(oxel::check(incompatible).unwrap_err()[0].code, "E0333");
}

#[test]
fn result_propagation_preserves_evaluation_order_and_clr_types() {
    let folder = tempfile::tempdir().unwrap();
    let example = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/errors.oxel");
    assert_eq!(
        stdout(cli(folder.path(), "run", &example))
            .lines()
            .collect::<Vec<_>>(),
        ["30", "division by zero"],
    );
    let path = folder.path().join("propagate.oxel");
    fs::write(
        &path,
        r#"
        enum Result<Value, Failure> { Err(Failure), Ok(Value) }
        struct Number { value: i32 }
        fn trace(value: i32) -> i32 {
            System.Console.WriteLine(value);
            value
        }
        fn operand(fail: bool) -> Result<Number, i32> {
            System.Console.WriteLine("operand");
            if fail { Result.Err(7) } else { Result.Ok(Number { value: 10 }) }
        }
        fn add(a: i32, b: i32, c: i32) -> i32 { a + b + c }
        fn calculate(fail: bool) -> Result<i32, object> {
            let begin: i32;
            Result.Ok(add({ begin = trace(1); begin }, operand(fail)?.value, trace(2)) + begin)
        }
        fn forward<T, E>(value: Result<T, E>) -> Result<T, E> {
            Result.Ok(value?)
        }
        fn erase_error<E>(value: Result<i32, E>) -> Result<i32, object> {
            Result.Ok(value?)
        }
        fn diverge() -> Result<i32, object> {
            ({ return Result.Err("diverged"); })?;
        }
        fn print(value: Result<i32, object>) {
            match value {
                Ok(answer) => System.Console.WriteLine(answer),
                Err(error) => System.Console.WriteLine(error),
            }
        }
        fn main() {
            print(forward(calculate(false)));
            print(forward(calculate(true)));
            print(erase_error(Result.Err::<i32, bool>(true)));
            print(diverge());
        }
    "#,
    )
    .unwrap();
    assert_eq!(
        stdout(cli(folder.path(), "run", &path))
            .lines()
            .collect::<Vec<_>>(),
        [
            "1", "operand", "2", "14", "1", "operand", "7", "True", "diverged"
        ],
    );
}

#[test]
fn result_propagation_reports_invalid_context_operands_and_error_types() {
    let definition = "enum Result<T, E> { Ok(T), Err(E) }";
    for (source, code, message) in [
        (
            "fn use_value(value: Result<i32, string>) -> i32 { value? }",
            "E0333",
            "requires a function returning Result<T, E>",
        ),
        (
            "fn use_value(value: i32) -> Result<i32, string> { Result.Ok(value?) }",
            "E0333",
            "requires a Result<T, E> operand",
        ),
        (
            "fn use_value(value: Result<i32, bool>) -> Result<i32, string> { Result.Ok(value?) }",
            "E0301",
            "expected System.String, found System.Boolean",
        ),
        (
            "fn use_value(value: Result<string, string>) -> Result<i32, string> { Result.Ok(value?) }",
            "E0301",
            "expected System.Int32, found System.String",
        ),
        (
            "enum Other<T, E> { Ok(T), Err(E) } fn use_value(value: Other<i32, string>) -> Result<i32, string> { Result.Ok(value?) }",
            "E0333",
            "requires a Result<T, E> operand",
        ),
    ] {
        let source = format!("{definition}\n{source}\nfn main() {{}}");
        let errors = oxel::check(&source).unwrap_err();
        assert_eq!(errors[0].code, code, "{source}: {errors:?}");
        assert!(errors[0].message.contains(message), "{source}: {errors:?}");
        if code == "E0333" {
            assert_eq!(&source[errors[0].span.start..errors[0].span.end], "?");
        }
    }
    for wrong in [
        "enum Result<T, E> { Ok(E), Err(T) }",
        "enum Result<T, E> { Ok(T), Err(E), Pending }",
        "struct Result<T, E> { value: T, error: E }",
    ] {
        let source = format!(
            "{wrong} fn use_value(value: Result<i32, string>) -> Result<i32, string> {{ value?; value }} fn main() {{}}"
        );
        let errors = oxel::check(&source).unwrap_err();
        assert_eq!(errors[0].code, "E0333", "{source}: {errors:?}");
    }
}

#[test]
fn propagation_tracks_definite_assignment_in_the_operand() {
    let declarations = "enum Result<T, E> { Ok(T), Err(E) } fn main() {}";
    let assigned = format!(
        "{declarations}
        fn pass(input: Result<i32, string>) -> Result<i32, string> {{
            let count: i32;
            {{ count = 30; input }}?;
            Result.Ok(count)
        }}
    "
    );
    assert!(oxel::check(&assigned).is_ok());
    let unassigned = format!(
        "{declarations}
        fn pass(input: Result<i32, string>) -> Result<i32, string> {{
            let value: Result<i32, string>;
            Result.Ok(value?)
        }}
    "
    );
    assert_eq!(oxel::check(&unassigned).unwrap_err()[0].code, "E0321");
}

#[test]
fn numeric_casts_match_comptime_and_coreclr_at_boundaries() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("casts.oxel");
    let source = r#"
        comptime fn fold(value: f64) -> i32 { value as i32 }
        const NEGATIVE = fold(-30.75);
        const ZERO = -0.75 as u8;
        const LARGE = 3000000000 as i64;
        const MIN = -9223372036854775808.0 as i64;
        const MAX = 9223372036854775807 as i64;
        const ROUND = 4611686293305294849 as f32;
        fn truncate(value: f64) -> i32 { value as i32 }
        fn byte(value: f64) -> u8 { value as u8 }
        fn long(value: f64) -> i64 { value as i64 }
        fn single(value: i64) -> f32 { value as f32 }
        fn early() -> i64 { { return 30; } as i64 }
        fn main() {
            System.Console.WriteLine(truncate(-30.75) == NEGATIVE);
            System.Console.WriteLine(byte(-0.75) == ZERO);
            System.Console.WriteLine(byte(255.75));
            System.Console.WriteLine((-128.75 as i8) as i32);
            System.Console.WriteLine((32767.75 as i16) as i32);
            let high: u8 = 255;
            System.Console.WriteLine(high as i64);
            System.Console.WriteLine(high as f64);
            let low: i8 = -128;
            System.Console.WriteLine(low as i64);
            System.Console.WriteLine(LARGE as f64);
            System.Console.WriteLine(long(-9223372036854775808.0) == MIN);
            System.Console.WriteLine(MAX);
            System.Console.WriteLine(single(4611686293305294849) == ROUND);
            let fraction: f32 = -3.75;
            System.Console.WriteLine(fraction as i32);
            System.Console.WriteLine(fraction as f64);
            System.Console.WriteLine((1e40 as f32) > (3e38 as f32));
            System.Console.WriteLine(1 + 59 as System.Int32 / 2);
            System.Console.WriteLine(early());
        }
    "#;
    fs::write(&path, source).unwrap();
    assert_eq!(
        stdout(cli(folder.path(), "run", &path))
            .lines()
            .collect::<Vec<_>>(),
        [
            "True",
            "True",
            "255",
            "-128",
            "32767",
            "255",
            "255",
            "-128",
            "3000000000",
            "True",
            "9223372036854775807",
            "True",
            "-3",
            "-3.75",
            "True",
            "30",
            "30",
        ],
    );

    let example = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/numeric_casts.oxel");
    assert_eq!(
        stdout(cli(folder.path(), "run", &example))
            .lines()
            .collect::<Vec<_>>(),
        ["-129", "3000000000", "255", "30"],
    );
}

#[test]
fn checked_numeric_casts_report_overflow_and_preserve_type_checking() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("overflow.oxel");
    for cast in [
        "256 as u8",
        "-1 as u8",
        "128 as i8",
        "-32769 as i16",
        "2147483648 as i32",
        "9223372036854775808.0 as i64",
        "(0.0 / 0.0) as i32",
        "(1.0 / 0.0) as i64",
    ] {
        let program = format!("const BAD = {cast}; fn main() {{}}");
        let errors = oxel::check(&program).unwrap_err();
        assert_eq!(errors[0].code, "E0503", "{cast}: {errors:?}");
        assert!(errors[0].message.contains("numeric cast"), "{errors:?}");

        let program = format!("fn main() {{ System.Console.WriteLine({cast}); }}");
        fs::write(&path, program).unwrap();
        let output = cli(folder.path(), "run", &path);
        assert!(!output.status.success(), "{cast}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("System.OverflowException"),
            "{cast}: {stderr}"
        );
    }

    for (program, expected) in [
        ("fn main() { let value: i32; value as i64; }", "E0321"),
        ("fn main() { 1 as bool; }", "E0310"),
        ("fn main() { 1 as string; }", "E0310"),
        ("const A = 1 as object; fn main() {}", "E0501"),
        ("fn main() { 9223372036854775808 as i32; }", "E0303"),
    ] {
        let errors = oxel::check(program).unwrap_err();
        assert_eq!(errors[0].code, expected, "{program}: {errors:?}");
    }
}

#[test]
fn interpolation_handles_nested_expressions_braces_and_nulls() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("interpolation.oxel");
    let source = r#"
        const LABEL = wrap(TEXT);
        const TEXT = "你好🚀";
        comptime fn wrap(text: string) -> string { $"{{{text}}}" }
        fn render<T>(value: T) -> string { $"[{value}]" }
        fn early() -> string { $"before { { return "done"; } } after" }
        fn main() {
            let maybe: string? = null;
            let erased: object? = null;
            let seq: i32;
            System.Console.WriteLine($"{ { seq = 20; seq } }:{seq + 10}:{true}");
            System.Console.WriteLine($"outer { $"inner {{{"quoted }"}}}" } end");
            System.Console.WriteLine($"{if seq == 20 { "yes" } else { "no" }}");
            System.Console.WriteLine($"<{maybe}>{erased}{null}");
            System.Console.WriteLine($"{{plain}}\t\"text\"");
            System.Console.WriteLine($"block { // closing brace here is ignored: }
                3 + 4
            }");
            System.Console.WriteLine(render(30));
            System.Console.WriteLine(render("x"));
            System.Console.WriteLine(early());
            System.Console.WriteLine(LABEL);
            System.Console.WriteLine($"");
        }
    "#;
    fs::write(&path, source).unwrap();
    assert_eq!(
        stdout(cli(folder.path(), "run", &path))
            .lines()
            .collect::<Vec<_>>(),
        [
            "20:30:True",
            "outer inner {quoted }} end",
            "yes",
            "<>",
            "{plain}\t\"text\"",
            "block 7",
            "[30]",
            "[x]",
            "done",
            "{你好🚀}",
            "",
        ],
    );
    let example = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/interpolation.oxel");
    assert_eq!(
        stdout(cli(folder.path(), "run", &example)).trim(),
        "Hello, Alice. You are 20 years old.",
    );
}

#[test]
fn interpolation_reports_invalid_parts_at_source_locations() {
    for (program, code) in [
        (r#"fn main() { $"{missing}"; }"#, "E0203"),
        (r#"fn main() { $"{}"; }"#, "E0102"),
        (r#"fn main() { $"unmatched }"; }"#, "E0108"),
        (r#"fn main() { $"unfinished {1"#, "E0108"),
        ("fn main() { $\"unfinished\n}", "E0108"),
        (r#"fn main() { let value: i32; $"{value}"; }"#, "E0321"),
        (r#"fn main() { $"a\q{30}"; }"#, "E0105"),
        (
            r#"fn main() { $"{System.Console.WriteLine(1)}"; }"#,
            "E0312",
        ),
        (r#"const BAD = $"{4}"; fn main() {}"#, "E0501"),
    ] {
        let errors = oxel::check(program).unwrap_err();
        let error = errors
            .iter()
            .find(|error| error.code == code)
            .unwrap_or_else(|| panic!("{program}: {errors:?}"));
        assert!(error.span.start <= error.span.end && error.span.end <= program.len());
        if code == "E0203" || code == "E0321" {
            let name = if code == "E0203" { "missing" } else { "value" };
            assert_eq!(&program[error.span.start..error.span.end], name);
        }
    }
    let deep = format!(
        "fn main() {{ {}\"x\"{}; }}",
        "$\"{".repeat(150),
        "}\"".repeat(150)
    );
    assert!(oxel::check(&deep).is_err());
}

#[test]
fn integer_ranges_execute_with_single_evaluation_and_bounded_increments() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("range.oxel");
    let source = r#"
        comptime fn sum(n: i32) -> i32 {
            let mut sum = 0;
            for i in -2..=n {
                if i == 0 { continue; }
                if i == 7 { break; }
                sum = sum + i;
            }
            sum
        }
        comptime fn at_limit(maximum: i64) -> i32 {
            let mut count = 0;
            for value in maximum - 1..=maximum {
                count = count + 1;
                continue;
            }
            count
        }
        const FOLDED = sum(10);
        const LIMIT_COUNT = at_limit(9223372036854775807);
        fn early() -> i32 {
            for i in { return 30; }..=2 { System.Console.WriteLine(i); }
        }
        fn main() {
            let mut trace = 0;
            let lower: i32;
            let upper: i32;
            let mut runtime = 0;
            for i in { trace = trace * 10 + 1; lower = -2; lower }
                ..={ trace = trace * 10 + 2; upper = 10; upper } {
                if i == 0 { continue; }
                if i == 7 { break; }
                runtime = runtime + i;
            }
            System.Console.WriteLine(trace);
            System.Console.WriteLine(lower + upper);
            System.Console.WriteLine(runtime);
            System.Console.WriteLine(runtime == FOLDED);

            let maximum: i64 = 9223372036854775807;
            let mut count = 0;
            for value in maximum - 1..=maximum {
                count = count + 1;
                continue;
            }
            System.Console.WriteLine(count == LIMIT_COUNT);
            let byte: u8 = 255;
            for value in 254..=byte {
                System.Console.WriteLine(value);
            }
            for value in 5..5 { System.Console.WriteLine(value); }
            for value in 5..=4 { System.Console.WriteLine(value); }
            for value in 5..=5 { System.Console.WriteLine(value); }

            let i = 30;
            let mut nested = 0;
            for i in 0..3 {
                for j in { if i == 1 { continue; } 0 }..3 {
                    if j == 2 { break; }
                    nested = nested + 1;
                }
                nested = nested + 10;
            }
            System.Console.WriteLine(nested);
            System.Console.WriteLine(i);
            System.Console.WriteLine(early());
        }
    "#;
    fs::write(&path, source).unwrap();
    assert_eq!(
        stdout(cli(folder.path(), "run", &path))
            .lines()
            .collect::<Vec<_>>(),
        [
            "12", "8", "18", "True", "True", "254", "255", "5", "24", "30", "30"
        ],
    );

    let example = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/ranges.oxel");
    assert_eq!(
        stdout(cli(folder.path(), "run", &example))
            .lines()
            .collect::<Vec<_>>(),
        ["720", "Step 1", "Step 2", "Step 3"],
    );
}

#[test]
fn integer_ranges_preserve_scope_and_assignment_diagnostics() {
    for (program, code) in [
        ("fn main() { for i in false..true {} }", "E0313"),
        ("fn main() { for i in 1.0..2.0 {} }", "E0313"),
        (
            "fn main() { let lo: i32 = 0; let hi: i64 = 1; for i in lo..hi {} }",
            "E0301",
        ),
        ("fn main() { for i in 0..3 { i = 1; } }", "E0204"),
        ("fn main() { for i in 0..3 {} i; }", "E0203"),
        ("fn main() { for i in i..3 {} }", "E0203"),
        ("fn main() { for i from 0..3 {} }", "E0102"),
        ("fn main() { for i in 0.. {} }", "E0102"),
        ("fn main() { let x: i32; for i in 1..1 {} x; }", "E0321"),
        (
            "fn main() { let x: i32; for i in 0..2 { x = i; } }",
            "E0322",
        ),
        ("fn main() { for i in { break; }..2 {} }", "E0307"),
        (
            "comptime fn large() -> i32 { for i in 0..500000 {} 0 } const X = large(); fn main() {}",
            "E0504",
        ),
    ] {
        let errors = oxel::check(program).expect_err(program);
        assert_eq!(errors[0].code, code, "{program}: {errors:?}");
    }
    let deep = format!(
        "fn main() {{ {}{} }}",
        "for i in 0..1 {".repeat(150),
        "}".repeat(150)
    );
    let errors = oxel::check(&deep).unwrap_err();
    assert_eq!(errors[0].code, "E0103");
}

#[test]
fn bitwise_operators_preserve_width_and_evaluate_both_operands() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("bitwise.oxel");
    fs::write(
        &path,
        r#"
        const BYTE: u8 = ~0;
        const BOOLEAN = true ^ false;
        comptime fn flags() -> u8 {
            let mut value: u8 = 3;
            value |= 128;
            value ^= 1;
            value &= 255;
            value
        }
        const FOLDED = flags();

        fn main() {
            let mut value: u8 = 3;
            value |= 128;
            value ^= 1;
            value &= 255;
            let complement = ~value;
            System.Console.WriteLine(value == FOLDED);
            System.Console.WriteLine(complement == BYTE ^ value);
            System.Console.WriteLine(1 | 2 ^ 3 & 6 == 1);
            let mut evaluated = false;
            System.Console.WriteLine(false & { evaluated = true; true });
            System.Console.WriteLine(evaluated | false ^ false);
            System.Console.WriteLine(BOOLEAN);
        }
        "#,
    )
    .unwrap();
    assert_eq!(
        stdout(cli(folder.path(), "run", &path))
            .lines()
            .collect::<Vec<_>>(),
        ["True", "True", "True", "False", "True", "True"]
    );
    for source in [
        "fn main() { 1.0 & 2.0; }",
        "fn main() { ~true; }",
        "fn main() { \"a\" | \"b\"; }",
    ] {
        assert_eq!(
            oxel::check(source).unwrap_err()[0].code,
            "E0302",
            "{source}"
        );
    }
    assert_eq!(
        oxel::check("fn main() { let x: i64 = 1; let y: i32 = 1; x & y; }").unwrap_err()[0].code,
        "E0301",
    );
}

#[test]
fn numeric_literals_share_radix_and_separator_rules_across_evaluation() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("numbers.oxel");
    fs::write(
        &path,
        r#"
        const MASK: u8 = 0b1111_0000 | 0x0f;
        const MIN: i64 = -0x8000_0000_0000_0000;
        const MAX: u64 = 0xFFFF_FFFF_FFFF_FFFF;
        const FRACTION: f32 = 1_2.5_0e-0_1;

        fn after<T>(left: T, right: T) -> T { right }

        fn main() {
            System.Console.WriteLine(MASK == 0O377);
            System.Console.WriteLine(MIN);
            System.Console.WriteLine(after(MAX, 0XFFFF_FFFF_FFFF_FFFF));
            System.Console.WriteLine(FRACTION == 1.25);
            let runtime = 1_000 + 0B1010 + 0o12;
            System.Console.WriteLine(runtime);
            let mut sum = 0;
            for i in 0x1..=0x3 { sum += i; }
            System.Console.WriteLine(sum);
        }
    "#,
    )
    .unwrap();
    let result = stdout(cli(folder.path(), "run", &path));
    assert_eq!(
        result.lines().collect::<Vec<_>>(),
        [
            "True",
            "-9223372036854775808",
            "18446744073709551615",
            "True",
            "1020",
            "6"
        ],
    );

    for literal in [
        "0x", "0x1g", "0b012", "0o18", "0x_ff", "0x__", "0b1_", "1__0", "5_", "1_.5", "1.2__5",
        "1e+_2", "1e-",
    ] {
        let source = format!("fn main() {{ {literal}; }}");
        let errors = oxel::check(&source).unwrap_err();
        assert_eq!(errors.len(), 1, "{source}: {errors:?}");
        let error = &errors[0];
        assert_eq!(error.code, "E0106", "{source}");
        assert_eq!(&source[error.span.start..error.span.end], literal);
    }
    for source in [
        "fn main() { 0x8000_0000; }",
        "fn main() { let _: u8 = 0x100; }",
        "fn main() { let _: u64 = 0x1_0000_0000_0000_0000; }",
    ] {
        assert_eq!(
            oxel::check(source).unwrap_err()[0].code,
            "E0303",
            "{source}"
        );
    }
}

#[test]
fn integer_shifts_mask_counts_and_preserve_nested_generic_syntax() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("shifts.oxel");
    fs::write(
        &path,
        r#"
        struct Cell<T> { value: T }
        fn echo<T>(value: T) -> T { value }

        const NARROW: u8 = 129 << 9;
        const SIGNED: i8 = -128 >> -1;
        const WIDE: u64 = 1 << 63;
        const TOP = WIDE >> 63;
        const WRAPPED = WIDE << 64;

        fn main() {
            let count: i32 = 9;
            let mut byte: u8 = 129;
            byte <<= count;
            System.Console.WriteLine(byte == NARROW && byte == 2);
            let mut signed: i8 = -128;
            signed >>= -1;
            System.Console.WriteLine(signed == SIGNED && signed == -1);
            let mut wide: u64 = 1;
            wide <<= 63;
            System.Console.WriteLine(wide == WIDE && wide << 64 == WRAPPED);
            System.Console.WriteLine(wide >> 63 == TOP && TOP == 1);
            System.Console.WriteLine(1 << 2 + 3 == 32);
            System.Console.WriteLine(1 | 3 << 2 == 13);
            System.Console.WriteLine((1 as i64 << 6 >> 2) == 16);

            let nested: Cell<Cell<i32>>= Cell { value: Cell { value: 30 } };
            let value = echo::<Cell<Cell<i32>>>(nested).value.value >> 1;
            System.Console.WriteLine(value);

            let values: u16[] = [65535];
            values[0] >>= 17;
            System.Console.WriteLine(values[0]);
        }
    "#,
    )
    .unwrap();
    let result = stdout(cli(folder.path(), "run", &path));
    assert_eq!(
        result.lines().collect::<Vec<_>>(),
        [
            "True", "True", "True", "True", "True", "True", "True", "15", "32767"
        ],
    );
    for (source, code) in [
        ("fn main() { true >> 1; }", "E0302"),
        ("fn main() { let count: i64 = 1; 2 << count; }", "E0301"),
        ("fn main() { 1 < < 2; }", "E0102"),
        ("fn main() { 1 > // gap\n > 2; }", "E0102"),
    ] {
        assert_eq!(oxel::check(source).unwrap_err()[0].code, code, "{source}");
    }
}

#[test]
fn compound_assignments_use_binary_types_and_require_initialized_mutable_bindings() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("compound.oxel");
    let source = r#"
        comptime fn factorial(n: i32) -> i32 {
            let mut result = 1;
            for i in 1..=n { result *= i; }
            result
        }
        const FACTORIAL = factorial(5);
        fn early() -> i32 {
            let mut value = 1;
            value += { return 42; };
            value
        }
        fn main() {
            let mut value: u8 = 250;
            value += 14;
            value -= 2;
            value *= 9;
            value /= 2;
            value %= 11;
            System.Console.WriteLine(value);

            let mut text = "ox";
            text += "el";
            System.Console.WriteLine(text);

            let mut before = 20;
            before += { before = 99; 10 };
            System.Console.WriteLine(before);
            System.Console.WriteLine(FACTORIAL);
            System.Console.WriteLine(early());

            let mut total = 0;
            for i in 1..=4 {
                total += { if i == 2 { continue; } if i == 4 { break; } i };
            }
            System.Console.WriteLine(total);
        }
    "#;
    fs::write(&path, source).unwrap();
    assert_eq!(
        stdout(cli(folder.path(), "run", &path)),
        "5\noxel\n30\n120\n42\n4\n",
    );

    for (source, code) in [
        ("fn main() { let value = 1; value += 3; }", "E0204"),
        ("fn main() { let value: i32; value += 3; }", "E0204"),
        ("fn main() { let mut value: i32; value += 3; }", "E0321"),
        (
            "fn main() { let mut value = true; value += false; }",
            "E0302",
        ),
        ("fn main() { let mut value = 1; value /= false; }", "E0301"),
        ("fn main() { missing *= 3; }", "E0203"),
        ("fn main() { let mut value = 1; value += ; }", "E0102"),
    ] {
        let errors = oxel::check(source).unwrap_err();
        assert_eq!(errors[0].code, code, "{source}: {errors:?}");
        assert!(errors[0].span.end <= source.len());
    }
}

#[test]
fn arrays_allocate_and_update_clr_vectors() {
    let folder = tempfile::tempdir().unwrap();
    let sample = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/arrays.oxel");
    assert_eq!(stdout(cli(folder.path(), "run", &sample)).trim(), "30");

    let program = folder.path().join("vectors.oxel");
    let source = r#"
        struct Counter { amount: i32 }
        struct Wrapper { name: string }
        enum Choice { Missing, String(string) }
        fn first<T>(values: T[]) -> T { values[0] }
        fn singleton<T>(value: T) -> T[] { [value] }
        fn replace<T>(values: T[], value: T) { values[0] = value; }
        fn early(values: i32[]) -> i32 {
            values[{ return 7; }] += 2;
            0
        }
        fn main() {
            let mut trace = 0;
            let mut values = new i32[1];
            let alias = values;
            alias[0] = 10;
            { trace = trace * 10 + 1; values }[
                { trace = trace * 10 + 2; values = [90]; 0 }
            ] += { trace = trace * 10 + 3; 20 };
            System.Console.WriteLine(trace);
            System.Console.WriteLine(alias[0]);
            System.Console.WriteLine(values[0]);
            System.Console.WriteLine(early(values));

            let words = singleton("before");
            replace(words, "oxel");
            System.Console.WriteLine(first(words));
            let optional = new string?[2];
            optional[1] = "ready";
            System.Console.WriteLine(optional[0] ?? optional[1] ?? "missing");
            let nullable: i32[]? = null;
            System.Console.WriteLine((nullable ?? [42])[0]);

            let grid = [[1, 2], [3, 4]];
            System.Console.WriteLine(grid[1][0]);
            let nested = new i32[]?[2];
            nested[0] = [41];
            System.Console.WriteLine((nested[0] ?? [0])[0]);
            let counts = new Counter[1];
            System.Console.WriteLine(counts[0].amount);
            counts[0] = Counter { amount: 25 };
            System.Console.WriteLine(counts[0].amount);
            let wrappers = [Wrapper { name: "valid" }];
            System.Console.WriteLine(wrappers[0].name);
            let enums = [Choice.Missing];
            System.Console.WriteLine(match enums[0] { Missing => 30, String(_) => 0 });
            let empty: i64[] = [];
            System.Console.WriteLine(empty.Length);
            let erased: object = values;
            System.Console.WriteLine(erased is i32[]);
        }
    "#;
    fs::write(&program, source).unwrap();
    assert_eq!(
        stdout(cli(folder.path(), "run", &program))
            .lines()
            .collect::<Vec<_>>(),
        [
            "123", "30", "90", "7", "oxel", "ready", "42", "3", "41", "0", "25", "valid", "30",
            "0", "True"
        ],
    );

    for (source, code) in [
        ("fn main() { let items = []; }", "E0314"),
        ("fn main() { let items = new string[1]; }", "E0314"),
        (
            "enum Choice { Missing } fn main() { new Choice[1]; }",
            "E0314",
        ),
        (
            "struct Unsafe { name: string } fn main() { new Unsafe[1]; }",
            "E0314",
        ),
        ("fn f<T: struct>() { new T[1]; } fn main() {}", "E0314"),
        ("fn main() { let items = [1, true]; }", "E0301"),
        ("fn main() { new i32[true]; }", "E0301"),
        ("fn main() { [1][0, 1]; }", "E0205"),
        ("fn main() { [1][true]; }", "E0301"),
        ("fn main() { let items: i32[]? = null; items[0]; }", "E0311"),
        (
            "fn main() { let items: object[] = [\"xel\"]; let words: string[] = items; }",
            "E0301",
        ),
        ("fn main() { let items: i32[]; items[0] += 2; }", "E0321"),
    ] {
        let errors = oxel::check(source).unwrap_err();
        assert_eq!(errors[0].code, code, "{source}: {errors:?}");
    }
}

#[test]
fn clr_characters_and_string_indices_use_utf16_code_units() {
    let folder = tempfile::tempdir().unwrap();
    let sample = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/characters.oxel");
    assert_eq!(stdout(cli(folder.path(), "run", &sample)), "A\n3\n55357\n");
    let program = folder.path().join("character.oxel");
    fs::write(
        &program,
        r#"
        const TEXT = "界🚀";
        const LENGTH = TEXT.Length;
        comptime fn first(value: string) -> char { value[0] }
        const FIRST = first(TEXT);
        const LOW = TEXT[LENGTH - 1] as i32;
        const MATCH = '\u{D83D}' == TEXT[1] && 'A' < '界';
        fn main() {
            let chars = new char[2];
            chars[0] = FIRST;
            chars[1] = 65 as char;
            System.Console.WriteLine(chars[0]);
            System.Console.WriteLine(chars[1]);
            System.Console.WriteLine(chars[0] > chars[1]);
            System.Console.WriteLine(MATCH);
            System.Console.WriteLine(LOW);
            System.Console.WriteLine('\'' == '\u{27}');
            System.Console.WriteLine('\n' as i32);
            let text = "a🚀";
            System.Console.WriteLine(text.Length);
            System.Console.WriteLine(text[1] as i32);
            let boxed: object = '界';
            System.Console.WriteLine(boxed is char);
        }
    "#,
    )
    .unwrap();
    assert_eq!(
        stdout(cli(folder.path(), "run", &program)),
        "界\nA\nTrue\nTrue\n56960\nTrue\n10\n3\n55357\nTrue\n"
    );

    for (source, code) in [
        ("fn main() { '🚀'; }", "E0109"),
        ("fn main() { ''; }", "E0109"),
        ("fn main() { 'ab'; }", "E0109"),
        (r"fn main() { '\u{10000}'; }", "E0109"),
        (r#"fn main() { "abc"[0] = 'x'; }"#, "E0204"),
        (r#"fn main() { "abc"[true]; }"#, "E0301"),
        ("fn main() { 'A' + 'B'; }", "E0302"),
        ("fn main() { let n: char = 65; }", "E0301"),
        ("const BAD = 65536 as char; fn main() {}", "E0503"),
        (r#"const BAD = "x"[1]; fn main() {}"#, "E0503"),
        (r#"const TEXT = "x" + TEXT[0]; fn main() {}"#, "E0502"),
    ] {
        let errors = oxel::check(source).unwrap_err();
        assert_eq!(errors[0].code, code, "{source}: {errors:?}");
    }
}

#[test]
fn sequence_loops_visit_vector_elements_and_utf16_units() {
    let folder = tempfile::tempdir().unwrap();
    let program = folder.path().join("sequences.oxel");
    fs::write(
        &program,
        r#"
        const LABEL = "A🚀";
        comptime fn checksum(text: string) -> i32 {
            let mut total = 0;
            for unit in text {
                if unit == 'A' { continue; }
                total += unit as i32;
            }
            total
        }
        const CHECKSUM = checksum(LABEL);
        fn early() -> i32 {
            for value in { return 30; } { System.Console.WriteLine(value); }
        }
        fn first_or<T>(items: T[], fallback: T) -> T {
            for item in items { return item; }
            fallback
        }
        fn main() {
            let array = [10, 0, 20, 9, 100];
            let mut source = array;
            let mut calls = 0;
            let mut total = 0;
            for value in { calls += 1; source } {
                if value == 10 {
                    source = [100];
                    array[2] = 20 + 1;
                }
                if value == 0 { continue; }
                if value == 9 { break; }
                total += value;
            }
            System.Console.WriteLine(calls);
            System.Console.WriteLine(total);

            let optional: string? = LABEL;
            if optional != null {
                let mut sum = 0;
                for unit in optional {
                    if unit == 'A' { continue; }
                    sum += unit as i32;
                }
                System.Console.WriteLine(sum == CHECKSUM);
            }
            System.Console.WriteLine(CHECKSUM);

            let empty: i32[] = [];
            for value in empty { System.Console.WriteLine(value); }
            for value in "" { System.Console.WriteLine(value); }
            for value in 0..3 {
                for item in {
                    if value == 1 { continue; };
                    [value]
                } {
                    System.Console.WriteLine(item);
                }
            }
            System.Console.WriteLine(early());
            for row in [[10], [20]] {
                for value in row { System.Console.WriteLine(value); }
            }
            System.Console.WriteLine(first_or([40], 50));
            System.Console.WriteLine(first_or(empty, 50));
            let count: i32;
            for _ in { count = 60; empty } {}
            System.Console.WriteLine(count);
        }
        "#,
    )
    .unwrap();
    assert_eq!(
        stdout(cli(folder.path(), "run", &program)),
        "1\n31\nTrue\n112317\n0\n2\n30\n10\n20\n40\n50\n60\n"
    );

    for (source, code) in [
        ("fn main() { for value in 3 {} }", "E0315"),
        (
            "fn main() { let array: i32[]? = null; for value in array {} }",
            "E0315",
        ),
        (
            r#"fn main() { let text: string? = null; for value in text {} }"#,
            "E0315",
        ),
        ("fn main() { for value in [1] { value = 2; } }", "E0204"),
        (
            "fn main() { for value in [1] {} System.Console.WriteLine(value); }",
            "E0203",
        ),
        (
            "comptime fn bad() { for value in [1 as object] {} } fn main() {}",
            "E0501",
        ),
        (
            "fn main() { let mut last: i32; for value in [1] { last = value; } last; }",
            "E0321",
        ),
    ] {
        let errors = oxel::check(source).unwrap_err();
        assert_eq!(errors[0].code, code, "{source}: {errors:?}");
    }
}

#[test]
fn unsigned_integer_widths_agree_at_runtime_and_comptime() {
    let folder = tempfile::tempdir().unwrap();
    let file = folder.path().join("unsigned.oxel");
    fs::write(&file, include_str!("../examples/unsigned_integers.oxel")).unwrap();
    assert_eq!(
        stdout(cli(folder.path(), "run", &file))
            .lines()
            .collect::<Vec<_>>(),
        [
            "0",
            "9223372036854775807",
            "18446744073709551614",
            "18446744073709551615"
        ]
    );

    fs::write(
        &file,
        r#"
        const MAX = 18446744073709551615 as u64;
        const HALF = MAX / 2;
        const MOD = MAX % 4;
        const NEAR_TIE: u64 = 9223372586610589697;
        const ROUNDED = NEAR_TIE as f32;
        const WIDE = 9223372036854775808.0 as u64;
        comptime fn steps(n: u64) -> i32 {
            let mut count = 0;
            for value in n - 1..=n {
                if value >= n - 1 { count += 1; }
            }
            count
        }
        const COUNT = steps(MAX);
        fn main() {
            let n = MAX;
            System.Console.WriteLine(n / 2 == HALF && n % 4 == MOD && MOD == 3);
            System.Console.WriteLine(n > 2 && n >= 2 && 2 < n && 2 <= n);
            System.Console.WriteLine(n + 1 == 0);
            let rounding = NEAR_TIE;
            System.Console.WriteLine(rounding as f32 == ROUNDED);
            let start = 9223372036854775808.0;
            System.Console.WriteLine(start as u64 == WIDE && WIDE > 0);
            let mut count = 0;
            for value in n - 1..=n {
                if value > n { break; }
                count += 1;
            }
            System.Console.WriteLine(count == COUNT && count == 2);
            let b: u32 = 4294967295;
            let a: u16 = 65535;
            System.Console.WriteLine(b + 1 == 0 && a + 1 == 0);
            System.Console.WriteLine(b as i64);
        }
    "#,
    )
    .unwrap();
    assert_eq!(
        stdout(cli(folder.path(), "run", &file))
            .lines()
            .collect::<Vec<_>>(),
        [
            "True",
            "True",
            "True",
            "True",
            "True",
            "True",
            "True",
            "4294967295"
        ]
    );
    for (source, code) in [
        ("fn main() { let n: u64 = 18446744073709551616; }", "E0303"),
        ("fn main() { let n: u32 = -1; }", "E0303"),
        ("fn main() { let n: u16 = 1; -n; }", "E0302"),
        (
            "const BAD = 18446744073709551615 as u64 as i64; fn main() {}",
            "E0503",
        ),
        (
            "const BAD = 18446744073709551616.0 as u64; fn main() {}",
            "E0503",
        ),
        ("const BAD = -1 as u32; fn main() {}", "E0503"),
    ] {
        let errors = oxel::check(source).unwrap_err();
        assert_eq!(errors[0].code, code, "{source}: {errors:?}");
    }
}

#[test]
fn public_signatures_reject_internal_types_at_every_export_boundary() {
    let hidden = "internal struct Hidden { value: i32 }";
    for (declaration, annotation) in [
        ("fn use_hidden(value: Hidden) {}", "Hidden"),
        (
            "fn boxed() -> Option<Hidden[]> { Option.None }",
            "Option<Hidden[]>",
        ),
        ("struct Visible { payload: Hidden[] }", "Hidden[]"),
        ("enum Visible { Item(Hidden) }", "Hidden"),
    ] {
        let source = format!("{hidden}\n{declaration}\nfn main() {{}}");
        let errors = oxel::check(&source).unwrap_err();
        assert_eq!(errors[0].code, "E0342", "{errors:?}");
        assert_eq!(
            &source[errors[0].span.start..errors[0].span.end],
            annotation
        );
        let related = errors[0].related().unwrap();
        assert_eq!(&source[related.span.start..related.span.end], "Hidden");
    }
}

#[test]
fn internal_is_contextual_and_internal_main_can_be_an_entry_point() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("internal.oxel");
    fs::write(
        &path,
        r#"
        internal struct Hidden { value: i32 }
        internal comptime fn seed() -> Hidden { Hidden { value: 30 } }
        const SEED = seed();
        fn internal(internal: i32) -> i32 { internal }
        internal fn main() { System.Console.WriteLine(internal(SEED.value)); }
        "#,
    )
    .unwrap();
    assert_eq!(stdout(cli(folder.path(), "run", &path)).trim(), "30");
}
