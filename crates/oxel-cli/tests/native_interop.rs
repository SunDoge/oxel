#[test]
fn native_import_diagnostics_validate_the_declaration_before_linking() {
    for (declaration, code, expected) in [
        (
            r#"extern "cdecl" fn value() = "test";"#,
            "E0343",
            "C calling convention",
        ),
        (r#"extern "C" fn value() = "";"#, "E0343", "nonempty"),
        (
            r#"extern "C" fn value() = "test" as "";"#,
            "E0343",
            "nonempty",
        ),
        (r#"extern "C" fn value() = "test" as "\0";"#, "E0343", "NUL"),
        (
            r#"extern "C" fn value<T>(input: T) -> T = "test";"#,
            "E0343",
            "cannot be generic",
        ),
        (
            r#"extern "C" fn value(text: string) = "test";"#,
            "E0343",
            "native parameters require",
        ),
        (
            r#"extern "C" fn value() -> bool = "test";"#,
            "E0343",
            "native return types",
        ),
        (
            r#"extern "C" fn value() {}"#,
            "E0102",
            "native library name",
        ),
        (
            r#"extern "C" fn value() -> i32 = "test"; const VALUE = value();"#,
            "E0501",
            "only call comptime functions",
        ),
    ] {
        let source = format!("{declaration}\nfn main() {{}}");
        let errors = oxel::check(&source).unwrap_err();
        assert_eq!(errors[0].code, code, "{errors:?}");
        assert!(errors[0].message.contains(expected), "{errors:?}");
    }
    let invalid_entry = r#"extern "C" fn main() -> i32 = "test";"#;
    assert_eq!(oxel::check(invalid_entry).unwrap_err()[0].code, "E0206");
}

#[test]
fn native_utf8_parameters_are_explicit_and_keep_managed_nullability() {
    for (declaration, expected) in [
        (
            r#"extern "C" fn read(text: string?) = "test";"#,
            "explicit 'utf8'",
        ),
        (
            r#"extern "C" fn read(utf8 text: i32) = "test";"#,
            "requires a string",
        ),
        (
            r#"extern "C" fn read(out text: string?) = "test";"#,
            "explicit 'utf8'",
        ),
        (
            r#"extern "C" fn read() -> string = "test";"#,
            "native return types",
        ),
    ] {
        let source = format!("{declaration} fn main() {{}}");
        let errors = oxel::check(&source).unwrap_err();
        assert_eq!(errors[0].code, "E0343", "{errors:?}");
        assert!(errors[0].message.contains(expected), "{errors:?}");
    }

    let imports = r#"
        extern "C" fn read(utf8 text: string) = "test";
        extern "C" fn read_optional(utf8 text: string?) = "test";
    "#;
    let source = format!("{imports} fn main() {{ read(null); }}");
    let errors = oxel::check(&source).unwrap_err();
    assert_eq!(errors[0].code, "E0301", "{errors:?}");

    let source = format!(r#"{imports} fn main() {{ read_optional(null); read("你好"); }}"#);
    oxel::compile(&source, "utf8").unwrap();
    let signature = oxel::declared_function_signature(imports, "read_optional", None).unwrap();
    assert!(
        signature.label.contains("utf8 text: string?"),
        "{signature:?}"
    );
}

#[test]
fn native_reference_arguments_use_mode_mutability_and_assignment_checks() {
    let imports = r#"
        extern "C" fn update(ref value: i32) = "test";
        extern "C" fn produce(out value: i32, count: i32) = "test";
    "#;
    for (body, code, message) in [
        ("let mut n = 1; update(n);", "E0213", "expects a 'ref'"),
        ("let mut n = 1; update(out n);", "E0213", "expects a 'ref'"),
        (
            "let mut n = 1; produce(ref n, 1);",
            "E0213",
            "expects an 'out'",
        ),
        ("let mut n = 1; produce(out n, ref n);", "E0213", "by-value"),
        ("let n = 1; update(ref n);", "E0204", "immutable"),
        ("let mut n: i32; update(ref n);", "E0321", "unassigned"),
        (
            "let mut n: i64 = 1; update(ref n);",
            "E0301",
            "System.Int32",
        ),
        ("let n: i32; produce(out n, n);", "E0321", "unassigned"),
        (
            "let n: i32; produce(out n, 1); produce(out n, 2);",
            "E0322",
            "already be assigned",
        ),
        (
            "let n: i32; if false { produce(out n, 1); } update(ref n);",
            "E0204",
            "immutable",
        ),
        (
            "let n: i32; if false { produce(out n, 1); } System.Console.WriteLine(n);",
            "E0321",
            "unassigned",
        ),
    ] {
        let source = format!("{imports} fn main() {{ {body} }}");
        let errors = oxel::check(&source).unwrap_err();
        assert_eq!(errors[0].code, code, "{errors:?}");
        assert!(errors[0].message.contains(message), "{errors:?}");
    }
    let valid = format!(
        "{imports} fn main() {{ let n: i32; produce(out n, 1); let mut copy = n; update(ref copy); }}"
    );
    oxel::compile(&valid, "valid").unwrap();
}

#[test]
fn native_reference_modes_survive_managed_metadata_import() {
    let source = r#"extern "C" fn compute(ref previous: i32, out result: i32) = "test";"#;
    let mut references = oxel::References::default();
    let bytes = oxel::compile_project_with_kind(
        &[source],
        "Native.Facade",
        &references,
        oxel::AssemblyKind::Library,
    )
    .unwrap();
    references.add(&bytes).unwrap();
    oxel::check_with_references(
        "fn main() { let mut previous = 1; let result: i32; Native.Facade.Program.compute(ref previous, out result); System.Console.WriteLine(result); }",
        &references,
    ).unwrap();
    let signature = oxel::declared_function_signature(source, "compute", None).unwrap();
    assert!(
        signature
            .label
            .contains("(ref previous: i32, out result: i32)"),
        "{signature:?}"
    );
}

#[test]
fn editing_native_parameter_mode_invalidates_cross_file_calls() {
    use std::sync::Arc;

    let host = "fn main() { let mut value = 1; update(ref value); }";
    let by_ref = r#"extern "C" fn update(ref value: i32) = "test";"#;
    let by_out = r#"extern "C" fn update(out value: i32) = "test";"#;
    let references = oxel::References::default();
    let mut analyzer = oxel::ProjectAnalyzer::default();
    let mut sources = [Arc::from(host), Arc::from(by_ref)];
    let first = analyzer.inspect(&sources, &references);
    assert!(first.iter().all(|file| file.diagnostics.is_empty()));
    sources[1] = Arc::from(by_out);
    let changed = analyzer.inspect(&sources, &references);
    assert_eq!(changed, oxel::inspect_project(&[host, by_out], &references));
    assert_eq!(changed[0].diagnostics[0].code, "E0213");
    assert_eq!(changed[0].diagnostics[0].related().unwrap().source, Some(1));
}

#[test]
fn rejects_unsupported_native_struct_layouts_at_the_signature() {
    let declarations = r#"
        struct Empty {}
        struct Box<T> { value: T }
        struct Text { value: string }
        struct Outer { record: Text }
        struct Flags { enabled: bool }
        struct View { values: i32[] }
        enum Choice { Item(i32) }
    "#;
    for (ty, related) in [
        ("Empty", None),
        ("Box<i32>", None),
        ("Choice", None),
        ("Outer", Some("Text.value")),
        ("Flags", Some("Flags.enabled")),
        ("View", Some("View.values")),
    ] {
        let source =
            format!(r#"{declarations} extern "C" fn send(input: {ty}) = "test"; fn main() {{}}"#);
        let errors = oxel::check(&source).unwrap_err();
        let error = &errors[0];
        assert_eq!(error.code, "E0343", "{errors:?}");
        assert_eq!(&source[error.span.start..error.span.end], ty);
        match related {
            Some(field) => assert!(
                error.related().unwrap().message.contains(field),
                "{error:?}"
            ),
            None => assert!(error.related().is_none(), "{error:?}"),
        }
    }
}

#[test]
fn source_struct_layout_changes_revalidate_native_signatures_in_the_editor() {
    use std::sync::Arc;

    let client = r#"extern "C" fn transform(input: Outer) -> Outer = "test"; fn main() {}"#;
    let valid = "struct Outer { inner: Inner } struct Inner { value: i32 }";
    let invalid = "struct Outer { inner: Inner } struct Inner { value: string }";
    let references = oxel::References::default();
    let mut analyzer = oxel::ProjectAnalyzer::default();
    let mut sources = [Arc::from(client), Arc::from(valid)];
    assert!(
        analyzer
            .inspect(&sources, &references)
            .iter()
            .all(|file| file.diagnostics.is_empty())
    );
    sources[1] = Arc::from(invalid);
    let result = analyzer.inspect(&sources, &references);
    assert_eq!(
        result,
        oxel::inspect_project(&[client, invalid], &references)
    );
    assert_eq!(result[0].diagnostics[0].code, "E0343");
    assert_eq!(result[0].diagnostics[0].related().unwrap().source, Some(1));
    sources[1] = Arc::from(valid);
    assert!(
        analyzer
            .inspect(&sources, &references)
            .iter()
            .all(|file| file.diagnostics.is_empty())
    );
}

#[test]
fn compiles_without_resolving_native_libraries() {
    let source = r#"
        internal extern "C" fn invoke(value: u16) = "oxel_not_installed" as "test_symbol";
        fn main() { invoke(1); }
    "#;
    oxel::check(source).unwrap();
    let first = oxel::compile(source, "test").unwrap();
    assert_eq!(first, oxel::compile(source, "test").unwrap());
}

#[cfg(any(
    all(target_os = "linux", target_env = "gnu"),
    target_os = "macos",
    target_os = "windows"
))]
#[test]
fn coreclr_calls_native_symbols_with_default_and_aliased_names() {
    use std::{fs, process::Command};

    let library = if cfg!(target_os = "windows") {
        "ucrtbase.dll"
    } else if cfg!(target_os = "macos") {
        "/usr/lib/libSystem.B.dylib"
    } else {
        "libc.so.6"
    };
    let source = format!(
        r#"
        internal extern "C" fn abs(n: i32) -> i32 = "{library}";
        internal extern "C" fn magnitude(n: i64) -> i64 = "{library}" as "llabs";
        fn main() {{
            System.Console.WriteLine(abs(-30));
            System.Console.WriteLine(magnitude(-5000000000));
        }}
    "#
    );
    let folder = tempfile::tempdir().unwrap();
    fs::write(folder.path().join("native.oxel"), source).unwrap();
    let command = |action| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_oxel"));
        command
            .current_dir(folder.path())
            .args([action, "native.oxel"]);
        command
    };
    let checked = command("check").env("PATH", "").output().unwrap();
    assert!(
        checked.status.success(),
        "{}",
        String::from_utf8_lossy(&checked.stderr)
    );
    let ran = command("run").output().unwrap();
    assert!(
        ran.status.success(),
        "{}",
        String::from_utf8_lossy(&ran.stderr)
    );
    assert_eq!(
        String::from_utf8(ran.stdout)
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        ["30", "5000000000"]
    );
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires a C compiler and the .NET Native AOT toolchain"]
fn packages_custom_c_library_for_coreclr_and_native_aot() {
    use std::{fs, process::Command};

    let folder = tempfile::tempdir().unwrap();
    let root = folder.path();
    let project = root.join("app");
    fs::create_dir_all(project.join("target")).unwrap();
    for (name, source) in [
        (
            "math.c",
            include_str!("../../../examples/native_interop/math.c"),
        ),
        (
            "main.oxel",
            include_str!("../../../examples/native_interop/main.oxel"),
        ),
        (
            "oxel.toml",
            include_str!("../../../examples/native_interop/oxel.toml"),
        ),
    ] {
        fs::write(project.join(name), source).unwrap();
    }
    let built = Command::new("cc")
        .current_dir(&project)
        .args(["-shared", "-fPIC", "-o", "target/liboxel_math.so", "math.c"])
        .output()
        .unwrap();
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
    let oxel = |action, flags: &[&str]| {
        let ran = Command::new(env!("CARGO_BIN_EXE_oxel"))
            .current_dir(root)
            .env_remove("LD_LIBRARY_PATH")
            .args([action, "--manifest-path", "app/oxel.toml"])
            .args(flags)
            .output()
            .unwrap();
        assert!(
            ran.status.success(),
            "{}",
            String::from_utf8_lossy(&ran.stderr)
        );
        String::from_utf8(ran.stdout).unwrap()
    };

    let managed = oxel("run", &[]);
    assert_eq!(
        managed.lines().collect::<Vec<_>>(),
        [
            "30", "30", "7", "21.25", "12", "41.5", "30", "35", "46.5", "12", "46.5", "35", "10",
            "-1", "0", "1"
        ]
    );
    assert_eq!(managed, oxel("run", &["--framework"]));

    for flags in [&[][..], &["--framework"][..]] {
        let managed_path = root.join(oxel("compile", flags).trim());
        assert!(managed_path.with_file_name("liboxel_math.so").is_file());
        let dll = Command::new("dotnet")
            .current_dir(root)
            .env_remove("LD_LIBRARY_PATH")
            .arg(&managed_path)
            .output()
            .unwrap();
        assert!(
            dll.status.success(),
            "{}",
            String::from_utf8_lossy(&dll.stderr)
        );
        assert_eq!(dll.stdout, managed.as_bytes());

        let native_path = root.join(oxel("publish", flags).trim());
        assert!(native_path.with_file_name("liboxel_math.so").is_file());
        let native = Command::new(native_path)
            .current_dir(root)
            .env_remove("LD_LIBRARY_PATH")
            .env("PATH", "")
            .env_remove("DOTNET_ROOT")
            .output()
            .unwrap();
        assert!(
            native.status.success(),
            "{}",
            String::from_utf8_lossy(&native.stderr)
        );
        assert_eq!(native.stdout, managed.as_bytes());
    }
}

#[test]
fn editor_refreshes_call_signatures_when_a_function_becomes_a_native_import() {
    use std::sync::Arc;

    let host = "fn main() { magnitude(-30); }";
    let managed = "internal fn magnitude(n: i32) -> i32 { n }";
    let native = r#"internal extern "C" fn magnitude(n: i32) -> i32 = "c" as "abs";"#;
    let references = oxel::References::default();
    let mut analyzer = oxel::ProjectAnalyzer::default();
    let mut sources = [Arc::from(host), Arc::from(managed)];
    let before = analyzer.inspect(&sources, &references);
    assert!(
        before[0]
            .occurrences
            .iter()
            .any(|item| item.detail.starts_with("internal fn magnitude("))
    );

    sources[1] = Arc::from(native);
    let after = analyzer.inspect(&sources, &references);
    assert_eq!(after, oxel::inspect_project(&[host, native], &references));
    assert!(after.iter().all(|file| file.diagnostics.is_empty()));
    assert!(after[0].occurrences.iter().any(|item| {
        item.detail
            .starts_with(r#"internal extern "C" fn magnitude("#)
    }));
    let signature = oxel::declared_function_signature(native, "magnitude", None).unwrap();
    assert!(
        signature
            .label
            .starts_with(r#"internal extern "C" fn magnitude("#)
    );
}
