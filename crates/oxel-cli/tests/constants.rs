use std::{fmt::Write as _, fs, process::Command};

use oxel::{Diagnostic, References, check_project, inspect_project};

#[test]
fn constants_fold_across_files_and_preserve_runtime_types() {
    let folder = tempfile::tempdir().unwrap();
    let main = r#"
        const TOTAL = match OK && false || OK { true => STEP * 3, false => 1 / 0 };
        const OK = !false && (STEP >= 10);
        const SHORT = true || (1 % 0 == 0);
        const MAX: i64 = 9223372036854775807 + 1;
        const WRAPPED: i32 = 2147483647 + 1;
        const QUOTIENT = -17 / 5;
        const REMAINDER = -17 % 5;
        const TEXT = {"结果" + " 🌍"};
        const SAME = TEXT == "结果 🌍";

        fn main() {
            System.Console.WriteLine(TOTAL);
            System.Console.WriteLine(SHORT);
            System.Console.WriteLine(MAX);
            System.Console.WriteLine(WRAPPED);
            System.Console.WriteLine(QUOTIENT);
            System.Console.WriteLine(REMAINDER);
            System.Console.WriteLine(TEXT);
            System.Console.WriteLine(SAME);
            let STEP = 20;
            System.Console.WriteLine(STEP);
        }
    "#;
    let declarations = "const STEP: i32 = 10;";
    fs::write(folder.path().join("main.oxel"), main).unwrap();
    fs::write(folder.path().join("settings.oxel"), declarations).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_oxel"))
        .current_dir(folder.path())
        .env("PATH", "")
        .args(["check", "main.oxel", "settings.oxel"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = Command::new(env!("CARGO_BIN_EXE_oxel"))
        .current_dir(folder.path())
        .args(["run", "main.oxel", "settings.oxel"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        [
            "30",
            "True",
            "-9223372036854775808",
            "-2147483648",
            "-3",
            "-2",
            "结果 🌍",
            "True",
            "20"
        ]
    );
    let snapshots = inspect_project(&[main, declarations], &References::default());
    let reference = snapshots[0]
        .occurrences
        .iter()
        .find(|item| {
            item.definition_source == Some(1) && &main[item.span.start..item.span.end] == "STEP"
        })
        .unwrap();
    assert_eq!(reference.detail, "const STEP: System.Int32");
    let span = reference.definition.unwrap();
    assert_eq!(&declarations[span.start..span.end], "STEP");
}

#[test]
fn reports_const_cycles_errors_and_unsupported_expressions() {
    for (source, code) in [
        ("const A = A; fn main() {}", "E0502"),
        ("const A = B; const B = A; fn main() {}", "E0502"),
        ("const A = MISSING; fn main() {}", "E0203"),
        ("const A = 1; const A = 2; fn main() {}", "E0201"),
        ("const A = 1; fn A() {} fn main() {}", "E0201"),
        ("const A: bool = 1; fn main() {}", "E0301"),
        ("const A = 100 + (40 / 0); fn main() {}", "E0503"),
        ("const A = -2147483648 / -1; fn main() {}", "E0503"),
        (
            "const A: i64 = -9223372036854775808 % -1; fn main() {}",
            "E0503",
        ),
        ("const A = main(); fn main() {}", "E0501"),
        ("const A = { let local = 1; local }; fn main() {}", "E0501"),
        ("const A = if true { 1 }; fn main() {}", "E0501"),
        (
            "const A = if true { 1 } else { false }; fn main() {}",
            "E0301",
        ),
        ("const A = 3; fn main() { A = 4; }", "E0204"),
    ] {
        assert_eq!(oxel::check(source).unwrap_err()[0].code, code, "{source}");
    }
    let input = ["const A = B;", "const B = A; fn main() {}"];
    let error = check_project(&input, &References::default())
        .unwrap_err()
        .remove(0);
    assert_eq!((error.diagnostic.code, error.source), ("E0502", Some(0)));
    let source = "const VALUE = 100 + (40 / 0); fn main() {}";
    let Diagnostic { span, .. } = oxel::check(source).unwrap_err().remove(0);
    assert_eq!(&source[span.start..span.end], "(40 / 0)");
}

#[test]
fn evaluates_long_dependency_chains_without_recursive_symbol_resolution() {
    let mut source = "fn main() { System.Console.WriteLine(V0); }\n".to_owned();
    for index in 0..10_000 {
        writeln!(source, "const V{index} = V{} + 1;", index + 1).unwrap();
    }
    source.push_str("const V10000 = 0;");
    oxel::check(&source).unwrap();
}

#[test]
fn folds_floating_and_narrow_integer_arithmetic_without_coreclr() {
    let folder = tempfile::tempdir().unwrap();
    let source = r#"
        const BYTE: u8 = 0 - 1;
        const SHORT: i16 = 32767 + 1;
        const SIGNED: i8 = -128 / -1;
        const ONE: f32 = 1.0;
        const PRECISE: f32 = 1.0000000596046448;
        const WIDTH: f32 = (16777216.0 + ONE) - 16777216.0;
        const NAN32: f32 = 0.0 / 0.0;
        const NAN64 = 0.0 % 0.0;
        const INF = 1.0 / -0.0;
        const ORDER = NAN32 != NAN32 && !(NAN32 <= 0.0) && !(NAN64 >= 0.0);
        const REM: f32 = -5.5 % 2.0;

        fn main() {
            System.Console.WriteLine(BYTE);
            System.Console.WriteLine(SHORT);
            System.Console.WriteLine(SIGNED);
            System.Console.WriteLine(PRECISE > ONE);
            System.Console.WriteLine(WIDTH == 0.0);
            System.Console.WriteLine(ORDER);
            System.Console.WriteLine(NAN32 != NAN32 && NAN64 != NAN64);
            System.Console.WriteLine(INF < -1e300);
            System.Console.WriteLine(REM == -1.5);
        }
    "#;
    fs::write(folder.path().join("numbers.oxel"), source).unwrap();
    let check = Command::new(env!("CARGO_BIN_EXE_oxel"))
        .current_dir(folder.path())
        .env("PATH", "")
        .args(["check", "numbers.oxel"])
        .output()
        .unwrap();
    assert!(
        check.status.success(),
        "{}",
        String::from_utf8_lossy(&check.stderr)
    );
    let result = Command::new(env!("CARGO_BIN_EXE_oxel"))
        .current_dir(folder.path())
        .args(["run", "numbers.oxel"])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        String::from_utf8(result.stdout)
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        [
            "255", "-32768", "-128", "True", "True", "True", "True", "True", "True"
        ]
    );
}

#[test]
fn executes_comptime_functions_and_emits_only_runtime_methods() {
    let folder = tempfile::tempdir().unwrap();
    let main = r#"
        const TOTAL = calculate(10);
        const PARITY = even(19);
        const SEQUENCE = fibonacci(30);
        const TEXT = label("oxel");
        comptime fn even(n: i32) -> bool { n == 0 || odd(n - 1) }
        fn print(value: i32) { System.Console.WriteLine(value); }
        fn main() {
            print(TOTAL);
            System.Console.WriteLine(PARITY);
            print(SEQUENCE);
            System.Console.WriteLine(TEXT);
        }
    "#;
    let definitions = r#"
        const LIMIT = NEXT + 1;
        const NEXT = 4;
        comptime fn label(value: string) -> string { "language: " + value }
        comptime fn odd(n: i32) -> bool { n != 0 && even(n - 1) }
        comptime fn fibonacci(n: i32) -> i32 {
            if n < 2 { n } else { fibonacci(n - 1) + fibonacci(n - 2) }
        }
        comptime fn empty() {}
        comptime fn noop() { return empty(); }
        comptime fn calculate(LIMIT: i32) -> i32 {
            noop();
            let stop: i32;
            { let LIMIT = NEXT; stop = LIMIT; }
            let mut remaining = LIMIT;
            let mut sum = 0;
            while remaining > 0 {
                remaining = remaining - 1;
                if remaining == stop { break; }
                if remaining % 2 == 0 { continue; }
                sum = sum + remaining;
            }
            if sum > 0 { return sum + NEXT; }
            1 / 0
        }
    "#;
    fs::write(folder.path().join("main.oxel"), main).unwrap();
    fs::write(folder.path().join("definitions.oxel"), definitions).unwrap();
    for command in ["check", "compile"] {
        let output = Command::new(env!("CARGO_BIN_EXE_oxel"))
            .current_dir(folder.path())
            .env("PATH", "")
            .args([command, "main.oxel", "definitions.oxel"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let result = Command::new("dotnet")
        .current_dir(folder.path())
        .arg("target/oxel/main.dll")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        String::from_utf8(result.stdout)
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        ["25", "False", "832040", "language: oxel"]
    );

    let sources = [main, definitions];
    let references = References::default();
    let bytes = oxel::compile_project(&sources, "sample", &references).unwrap();
    assert_eq!(
        bytes,
        oxel::compile_project(&sources, "sample", &references).unwrap()
    );
    let mut imported = References::default();
    imported.add(&bytes).unwrap();
    assert!(oxel::check_with_references("fn main() { Program.print(30); }", &imported).is_ok());
    assert_eq!(
        oxel::check_with_references("fn main() { Program.calculate(30); }", &imported).unwrap_err()
            [0]
        .code,
        "E0203"
    );

    let documents = inspect_project(&sources, &references);
    let occurrence = documents[0]
        .occurrences
        .iter()
        .find(|item| {
            item.definition_source == Some(1)
                && &main[item.span.start..item.span.end] == "calculate"
        })
        .unwrap();
    assert_eq!(
        occurrence.detail,
        "comptime fn calculate(LIMIT: System.Int32) -> System.Int32"
    );
}

#[test]
fn comptime_scalar_patterns_share_runtime_rules() {
    let folder = tempfile::tempdir().unwrap();
    let source = r#"
        const DIRECT = match true { false => 1 / 0, true => 30 };
        const CLASS = classify("ready");
        const TICKS = ticks();
        const WIDE = match wide_input() { 0xffffffffffffffff => '界', _ => 'x' };
        const GATE = if let 30 | 31 = DIRECT { "pass" } else { "fail" };
        const STOP = early(false);

        comptime fn wide_input() -> u64 { 18446744073709551615 }

        comptime fn classify(input: string) -> i32 {
            let mut calls = 0;
            let result = match { calls += 1; input } {
                "ready" | "pending" if { calls += 1; false } => 1 / 0,
                "ready" => 10,
                _ => 0,
            };
            calls + result
        }

        comptime fn ticks() -> i32 {
            let mut next = 0;
            let mut total = 0;
            while let 1 | 2 | 3 = { next += 1; next } {
                if let 2 = next { continue; }
                total += next;
            }
            total + next
        }

        comptime fn early(value: bool) -> i32 {
            let true = value else { return 7; };
            match value {
                true if { return 8; } => 1 / 0,
                _ => 0,
            }
        }
        const EARLY_GUARD = early(true);

        fn main() {
            System.Console.WriteLine(DIRECT);
            System.Console.WriteLine(CLASS);
            System.Console.WriteLine(TICKS);
            System.Console.WriteLine(WIDE);
            System.Console.WriteLine(GATE);
            System.Console.WriteLine(STOP);
            System.Console.WriteLine(EARLY_GUARD);
        }
    "#;
    fs::write(folder.path().join("patterns.oxel"), source).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_oxel"))
        .current_dir(folder.path())
        .args(["run", "patterns.oxel"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        ["30", "12", "8", "界", "pass", "7", "8"]
    );

    for (source, code) in [
        ("const A = match true { true => 1 };", "E0331"),
        (
            "const A = match true { true if B => 1, _ => 0 }; const B = A == 1;",
            "E0502",
        ),
        ("comptime fn f() { while let _ = true {} }", "E0335"),
        (
            "const A = f(); comptime fn f() -> i32 { while let true = true {} 0 }",
            "E0504",
        ),
        (
            "const A = match true { true => 1 / 0, false => 0 };",
            "E0503",
        ),
    ] {
        let source = format!("{source} fn main() {{}}");
        let errors = oxel::check(&source).unwrap_err();
        assert_eq!(errors[0].code, code, "{source}: {errors:?}");
    }
}

#[test]
fn comptime_aggregates_support_patterns_and_propagation() {
    let folder = tempfile::tempdir().unwrap();
    let source = r#"
        struct Pair<T> { left: T, right: T }
        enum Reading { First(Pair<i32>), Second(Pair<i32>), Missing }
        enum Result<T, E> { Ok(T), Err(E) }
        const BASE = 10;
        const value = ORDER;

        comptime fn build(present: bool) -> Reading {
            if !present { return Reading.Missing; }
            let mut n = 0;
            Reading.Second(Pair {
                right: { n += 1; BASE + n },
                left: { n += 1; BASE + n },
            })
        }

        comptime fn read(input: Reading) -> i32 {
            let Reading.First(value) | Reading.Second(value) = input else { return -1; };
            value.left * 10 + value.right
        }

        comptime fn fetch(present: bool) -> Option<i32> {
            if let Reading.Second(value) = build(present) {
                Option.Some(value.left + value.right)
            } else {
                Option.None
            }
        }

        comptime fn plus(present: bool) -> Option<i32> {
            Option.Some(fetch(present)? + 1)
        }

        comptime fn unwrap(present: bool) -> i32 {
            let value: Option<i32> = try { plus(present)? + 6 };
            match value {
                Some(item) if item > 0 => item,
                _ => -1,
            }
        }

        comptime fn checked(present: bool) -> Result<i32, string> {
            if present { Result.Ok(29) } else { Result.Err("missing") }
        }

        comptime fn attempt(present: bool) -> Result<i32, string> {
            try { checked(present)? + 1 }
        }

        const ORDER = read(build(true));
        const ABSENT = read(build(false));
        const RESULT = unwrap(true);
        const EMPTY = unwrap(false);
        const PASSED = match attempt(true) { Ok(number) => number, Err(_) => 0 };
        const ERROR = match attempt(false) { Ok(_) => "unreachable", Err(reason) => reason };
        const PAIR: Pair<i32> = Pair { left: 40, right: 2 };
        const FIELD = PAIR.right;
        const READING: Reading = build(true);
        const ALIAS = READING;
        const OPTIONAL: Option<i32> = plus(true);

        fn runtime_read(input: Reading) -> i32 {
            match input { Second(pair) => pair.left, _ => -1 }
        }

        fn main() {
            System.Console.WriteLine(ORDER);
            System.Console.WriteLine(ABSENT);
            System.Console.WriteLine(RESULT);
            System.Console.WriteLine(EMPTY);
            System.Console.WriteLine(PASSED);
            System.Console.WriteLine(ERROR);
            System.Console.WriteLine(FIELD);
            let mut copy = PAIR;
            copy = Pair { left: 1, right: 2 };
            System.Console.WriteLine(copy.left);
            System.Console.WriteLine(PAIR.left);
            System.Console.WriteLine(runtime_read(ALIAS));
            System.Console.WriteLine(unwrap_or(OPTIONAL, 0));
        }
    "#;
    fs::write(folder.path().join("aggregates.oxel"), source).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_oxel"))
        .current_dir(folder.path())
        .args(["run", "aggregates.oxel"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        [
            "131", "-1", "30", "-1", "30", "missing", "2", "1", "40", "12", "24"
        ]
    );

    for declaration in [
        "struct RefCell { value: object } comptime fn read(input: RefCell) -> i32 { 1 }",
        "struct RefCell { value: object } const BAD: RefCell = RefCell { value: 1 };",
    ] {
        let source = format!("{declaration} fn main() {{}}");
        let errors = oxel::check(&source).unwrap_err();
        assert_eq!(errors[0].code, "E0501", "{source}: {errors:?}");
        assert!(
            errors[0].message.contains("supported value types"),
            "{source}: {errors:?}"
        );
    }
}

#[test]
fn comptime_arrays_preserve_aliases_and_evaluation_order() {
    let folder = tempfile::tempdir().unwrap();
    let source = r#"
        struct Pair<T> { left: T, right: T }
        struct Buffer { data: i32[] }
        struct Node { children: Node[] }
        const FIRST = [START, 0][0];
        const START = 30;
        const EMPTY = (new i32[0]).Length;
        const TOTAL = sum();
        const CYCLE_LENGTH = cycle();

        comptime fn make() -> i32[] { [1, 2, 3] }

        comptime fn cycle() -> i32 {
            let nodes = [Node { children: [] }];
            nodes[0] = Node { children: nodes };
            nodes[0].children[0].children.Length
        }

        comptime fn update(buffer: Buffer) -> i32 {
            buffer.data[0] += 10;
            buffer.data[0]
        }

        comptime fn sum() -> i32 {
            let original = make();
            let separate = make();
            let matrix = [original, separate];
            let read = update(Buffer { data: matrix[0] });
            let mut calls = 0;
            { calls += 1; matrix[0] }[{ calls += 1; 1 }] += 5;
            let mut result = 0;
            for item in original {
                result += item;
                if item == 11 { matrix[0][2] = 30; }
            }
            let slots = new Pair<i32>[2];
            slots[1] = Pair { left: 5, right: 3 };
            result + read + separate[0] + calls + slots[0].right + slots[1].left
        }

        fn main() {
            System.Console.WriteLine(FIRST);
            System.Console.WriteLine(EMPTY);
            System.Console.WriteLine(TOTAL);
            System.Console.WriteLine(CYCLE_LENGTH);
        }
    "#;
    fs::write(folder.path().join("arrays.oxel"), source).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_oxel"))
        .current_dir(folder.path())
        .args(["run", "arrays.oxel"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        ["30", "0", "67", "1"]
    );

    for (declaration, code, message) in [
        ("const BAD = [1][-1];", "E0503", "index is out of bounds"),
        (
            "const BAD = (new i32[-1]).Length;",
            "E0503",
            "array length is negative",
        ),
        (
            "const BAD = (new i32[2147483647]).Length;",
            "E0504",
            "step limit",
        ),
        (
            "const BAD = [1];",
            "E0501",
            "constants require supported value types",
        ),
        (
            "struct Buffer { data: i32[] } const BAD: Buffer = Buffer { data: [1] };",
            "E0501",
            "constants require supported value types",
        ),
        (
            "comptime fn f() -> i32 { let a = [1]; a[9] = 1 / 0; 0 } const BAD = f();",
            "E0503",
            "division",
        ),
        (
            "comptime fn f() -> i32 { let a = [1]; a[9] += 1 / 0; 0 } const BAD = f();",
            "E0503",
            "index is out of bounds",
        ),
    ] {
        let source = format!("{declaration} fn main() {{}}");
        let errors = oxel::check(&source).unwrap_err();
        assert_eq!(errors[0].code, code, "{source}: {errors:?}");
        assert!(errors[0].message.contains(message), "{source}: {errors:?}");
    }
}

#[test]
fn compiler_type_queries_return_declaration_order() {
    let folder = tempfile::tempdir().unwrap();
    let source = r#"
        struct Point<T> { y: T, x: T }
        struct Empty {}
        enum Mode { Start, Paused(i32), Stop }

        const NAME = compiler.type_name::<Point<i32>[]>();
        const VOID_NAME = compiler.type_name::<void>();
        const EMPTY = compiler.field_names::<Empty>().Length;
        const CASES = compiler.variant_names::<Mode>().Length;
        const OPTION_CASE = compiler.variant_names::<Option<string>>()[1];
        const FIELDS = field_list();
        const FIRST: compiler.FieldInfo = compiler.fields_of::<Point<i64>>()[0];
        comptime fn field_list() -> string {
            let mut text = "";
            for name in compiler.field_names::<Point<i32>>() {
                text += name;
            }
            text
        }

        fn main() {
            System.Console.WriteLine(NAME);
            System.Console.WriteLine(VOID_NAME);
            System.Console.WriteLine(EMPTY);
            System.Console.WriteLine(CASES);
            System.Console.WriteLine(OPTION_CASE);
            System.Console.WriteLine(FIELDS);
            describe(FIRST);
        }

        fn describe(field: compiler.FieldInfo) {
            System.Console.WriteLine(field.name);
            System.Console.WriteLine(field.type_name);
            System.Console.WriteLine(field.readonly);
        }
    "#;
    fs::write(folder.path().join("reflection.oxel"), source).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_oxel"))
        .current_dir(folder.path())
        .args(["run", "reflection.oxel"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        [
            "Point<System.Int32>[]",
            "System.Void",
            "0",
            "3",
            "Some",
            "yx",
            "y",
            "System.Int64",
            "True"
        ]
    );

    for (source, code) in [
        ("fn main() { compiler.type_name::<i32>(); }", "E0501"),
        ("fn main() { compiler.fields_of::<i32>(); }", "E0501"),
        ("const BAD = compiler.type_name(); fn main() {}", "E0205"),
        (
            "const BAD = compiler.type_name::<i32>(1); fn main() {}",
            "E0205",
        ),
        (
            "const BAD = compiler.type_name::<Missing>(); fn main() {}",
            "E0202",
        ),
        (
            "enum Mode { Active } const BAD = compiler.field_names::<Mode>().Length; fn main() {}",
            "E0501",
        ),
        (
            "struct Point { x: i32 } const BAD = compiler.variant_names::<Point>().Length; fn main() {}",
            "E0501",
        ),
    ] {
        let errors = oxel::check(source).unwrap_err();
        assert_eq!(errors[0].code, code, "{source}: {errors:?}");
    }
}

#[test]
fn generic_comptime_calls_specialize_values_queries_and_memoization() {
    let folder = tempfile::tempdir().unwrap();
    let source = r#"
        struct Box<T> { value: T }
        struct Pair { first: i32, second: i32 }

        comptime fn take<T>(input: Option<T>) -> Option<T> {
            let elements = [input?];
            let boxed: Box<T> = Box { value: elements[0] };
            Option.Some(boxed.value)
        }

        comptime fn forward<T>(input: T) -> Option<T> {
            take::<T>(Option.Some(input))
        }

        comptime fn first_field<T>() -> compiler.FieldInfo {
            compiler.fields_of::<Box<T>>()[0]
        }

        comptime fn name<T>() -> string {
            compiler.type_name::<T>()
        }

        comptime fn cases<T>() -> i32 {
            compiler.variant_names::<T>().Length
        }

        comptime fn count<T>() -> i32 {
            compiler.field_names::<T>().Length
        }

        comptime fn recurse<T>() -> i32 {
            if count::<T>() == 2 { recurse::<Box<i32>>() } else { 42 }
        }

        const NUMBER = forward(42);
        const TEXT = forward("oxel");
        const ABSENT = take::<bool>(Option.None);
        const FIELD = first_field::<i64>();
        const NAME = name::<Box<string>[]>();
        const COUNTS = count::<Pair>() * 10 + count::<Box<i32>>();
        const CYCLE_FREE = recurse::<Pair>();
        const CASES = cases::<Option<bool>>();

        fn main() {
            System.Console.WriteLine(unwrap_or(NUMBER, 0));
            System.Console.WriteLine(unwrap_or(TEXT, "missing"));
            System.Console.WriteLine(unwrap_or(ABSENT, true));
            System.Console.WriteLine(FIELD.name);
            System.Console.WriteLine(FIELD.type_name);
            System.Console.WriteLine(NAME);
            System.Console.WriteLine(COUNTS);
            System.Console.WriteLine(CYCLE_FREE);
            System.Console.WriteLine(CASES);
        }
    "#;
    fs::write(folder.path().join("generic.oxel"), source).unwrap();
    let command = || {
        let mut command = Command::new(env!("CARGO_BIN_EXE_oxel"));
        command.current_dir(folder.path());
        command
    };
    let checked = command()
        .env("PATH", "")
        .args(["check", "generic.oxel"])
        .output()
        .unwrap();
    assert!(
        checked.status.success(),
        "{}",
        String::from_utf8_lossy(&checked.stderr)
    );
    let output = command().args(["run", "generic.oxel"]).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        [
            "42",
            "oxel",
            "True",
            "value",
            "System.Int64",
            "Box<System.String>[]",
            "21",
            "42",
            "2"
        ]
    );
}

#[test]
fn nullable_string_constants_share_runtime_option_and_fallback_semantics() {
    let folder = tempfile::tempdir().unwrap();
    let source = r#"
        struct Label { text: string? }

        const ABSENT: string? = null;
        const EMPTY: string? = "";
        const PRESENT: string? = "你好 🌍";
        const RECORD = Label { text: ABSENT };
        const OPTION: Option<string> = PRESENT;
        const EMPTY_OPTION: Option<string> = EMPTY;
        const EMPTY_TEXT = EMPTY_OPTION.unwrap_or("unexpected");
        const NONE: Option<string> = ABSENT;
        const BACK: string? = NONE;
        const BACK_PRESENT: string? = OPTION;
        const EXPANDED = $"[{BACK}][{BACK_PRESENT}]";

        comptime fn score(first: string?, second: string?) -> i32 {
            let mut visits = 0;
            let picked = unwrap_or_else({ visits += 1; first }, {
                unwrap_or_else({ visits += 10; second }, { visits += 100; "fallback" })
            });
            if first != null && first.Length == 0 && null != picked {
                return visits;
            }
            visits + picked.Length
        }

        comptime fn from_array(first: string?) -> string {
            let values = new string?[2];
            values[0] = first;
            unwrap_or_else(values[0], { unwrap_or(values[1], "empty") })
        }

        const LAZY = score(EMPTY, PRESENT);
        const SECOND = score(ABSENT, PRESENT);
        const FALLBACK = score(ABSENT, ABSENT);
        const ARRAY = from_array(ABSENT);

        fn main() {
            System.Console.WriteLine(unwrap_or(RECORD.text, "missing"));
            System.Console.WriteLine(EMPTY_TEXT);
            System.Console.WriteLine(BACK == null);
            System.Console.WriteLine(EXPANDED);
            System.Console.WriteLine(LAZY);
            System.Console.WriteLine(SECOND);
            System.Console.WriteLine(FALLBACK);
            System.Console.WriteLine(ARRAY);
        }
    "#;
    fs::write(folder.path().join("nullable.oxel"), source).unwrap();
    let command = || {
        let mut command = Command::new(env!("CARGO_BIN_EXE_oxel"));
        command.current_dir(folder.path());
        command
    };
    let checked = command()
        .env("PATH", "")
        .args(["check", "nullable.oxel"])
        .output()
        .unwrap();
    assert!(
        checked.status.success(),
        "{}",
        String::from_utf8_lossy(&checked.stderr)
    );
    let output = command().args(["run", "nullable.oxel"]).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        [
            "missing",
            "",
            "True",
            "[][你好 🌍]",
            "1",
            "16",
            "119",
            "empty"
        ]
    );

    let errors = oxel::check("const UNTYPED = null; fn main() {}").unwrap_err();
    assert_eq!(errors[0].code, "E0501");
}

#[test]
fn generic_comptime_diagnoses_unsupported_instantiations_and_queries() {
    for (source, message) in [
        (
            "comptime fn f<T>() -> i32 { let unused: T; 1 } const BAD = f::<object>();",
            "unsupported value type 'System.Object'",
        ),
        (
            "struct Point {} comptime fn f<T>() -> i32 { compiler.variant_names::<T>().Length } const BAD = f::<Point>();",
            "compiler.variant_names requires an Oxel enum",
        ),
        (
            "comptime fn f<T>() -> i32 { compiler.field_names::<Option<T>>().Length } const BAD = f::<i32>();",
            "compiler field queries require an Oxel struct",
        ),
    ] {
        let errors = oxel::check(&format!("{source} fn main() {{}}")).unwrap_err();
        assert_eq!(errors[0].code, "E0501", "{source}: {errors:?}");
        assert!(errors[0].message.contains(message), "{source}: {errors:?}");
    }
}

#[test]
fn comptime_diagnostics_cover_cycles_limits_and_unavailable_operations() {
    for (source, code, message) in [
        (
            "const N = f(); comptime fn f() -> i32 { g() } comptime fn g() -> i32 { N }",
            "E0502",
            "depends on a cycle",
        ),
        (
            "const N = f(1); comptime fn f(n:i32) -> i32 { g(n) } comptime fn g(n:i32) -> i32 { f(n) }",
            "E0502",
            "active call",
        ),
        (
            "const N = f(1); comptime fn f(n:i32) -> i32 { f(n + 1) }",
            "E0504",
            "depth limit",
        ),
        (
            "const N = f(); comptime fn f() -> i32 { while true {} 0 }",
            "E0504",
            "step limit",
        ),
        (
            "const N = f(); comptime fn f() -> i32 { 2 / 0 }",
            "E0503",
            "division",
        ),
        (
            "comptime fn f() { if false { System.Console.WriteLine(1); } }",
            "E0501",
            "unsupported expression",
        ),
        (
            "comptime fn f() { main(); }",
            "E0501",
            "only call comptime functions",
        ),
        (
            "comptime fn f<T>(x:object, y:T) -> T { y }",
            "E0501",
            "signatures with supported value types",
        ),
        (
            "comptime fn f() { let x: object = 1; x; }",
            "E0501",
            "supported local value types",
        ),
        (
            "const N = f(); comptime fn f() {}",
            "E0501",
            "requires a value",
        ),
        (
            "comptime fn f() -> i32 { let x: i32; x }",
            "E0321",
            "may be unassigned",
        ),
        (
            "comptime fn f() -> i32 { missing() }",
            "E0203",
            "unknown function",
        ),
        (
            "comptime fn f() -> i32 { 1 } fn runtime() -> i32 { f() }",
            "E0501",
            "only be called during constant evaluation",
        ),
    ] {
        let source = format!("{source}\nfn main() {{}}");
        let errors = oxel::check(&source).unwrap_err();
        assert_eq!(errors[0].code, code, "{source}: {errors:?}");
        assert!(errors[0].message.contains(message), "{source}: {errors:?}");
    }
    assert_eq!(
        oxel::check("comptime fn main() {}").unwrap_err()[0].code,
        "E0206"
    );
}
