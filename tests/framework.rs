use std::{fs, path::Path, process::Command, process::Output};

fn oxel(directory: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_oxel"))
        .current_dir(directory)
        .args(args)
        .output()
        .unwrap()
}

fn success(output: Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

const EXAMPLE: &str = include_str!("../examples/file_lines.oxel");
const ENUM_EXAMPLE: &str = include_str!("../examples/enum_interop.oxel");

#[test]
fn framework_null_guard_refines_its_argument_after_normal_return() {
    let folder = tempfile::tempdir().unwrap();
    let project = folder.path();
    fs::write(
        project.join("main.oxel"),
        r#"
        fn require(text: string?) -> string {
            System.ArgumentNullException.ThrowIfNull(text);
            text
        }
        fn main() {
            System.Console.WriteLine(require("oxel"));
            try { require(null); } catch (_: System.ArgumentNullException) {
                System.Console.WriteLine("missing");
            }
        }
        "#,
    )
    .unwrap();
    assert_eq!(
        success(oxel(project, &["run", "--framework", "main.oxel"])),
        "oxel\nmissing\n",
    );
    fs::write(
        project.join("main.oxel"),
        r#"
        fn require(text: string?) -> string {
            try { System.ArgumentNullException.ThrowIfNull(text); }
            catch (_: System.ArgumentNullException) {}
            text
        }
        fn main() {}
        "#,
    )
    .unwrap();
    let failure = oxel(project, &["check", "--framework", "main.oxel"]);
    assert!(!failure.status.success());
    let error = String::from_utf8(failure.stderr).unwrap();
    assert!(error.contains("E0301"), "{error}");
}

#[test]
fn traverses_framework_lists_and_lazy_linq_sequences() {
    let folder = tempfile::tempdir().unwrap();
    let project = folder.path();
    fs::write(
        project.join("main.oxel"),
        include_str!("../examples/collections.oxel"),
    )
    .unwrap();
    assert_eq!(
        success(oxel(project, &["run", "--framework", "main.oxel"])),
        "30\n6\n",
    );
}

#[test]
fn flags_combine_and_pass_to_framework_apis() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path();
    fs::write(
        path.join("main.oxel"),
        include_str!("../examples/bitwise.oxel"),
    )
    .unwrap();
    let result = success(oxel(path, &["run", "--framework", "main.oxel"]));
    assert_eq!(
        result.lines().collect::<Vec<_>>(),
        ["True", "True", "-30", "60"]
    );
}

#[test]
fn framework_events_subscribe_and_unsubscribe_clr_delegates() {
    let folder = tempfile::tempdir().unwrap();
    let project = folder.path();
    fs::write(
        project.join("main.oxel"),
        include_str!("../examples/events.oxel"),
    )
    .unwrap();
    let result = success(oxel(project, &["run", "--framework", "main.oxel"]));
    assert_eq!(result.lines().collect::<Vec<_>>(), ["1", "2"]);
}

#[test]
fn trailing_lambdas_bind_constructed_and_inferred_framework_delegates() {
    let folder = tempfile::tempdir().unwrap();
    let project = folder.path();
    fs::write(
        project.join("main.oxel"),
        include_str!("../examples/generic_callbacks.oxel"),
    )
    .unwrap();
    let result = success(oxel(project, &["run", "--framework", "main.oxel"]));
    assert_eq!(result.lines().collect::<Vec<_>>(), ["30", "oxel", "CLR"]);
}

#[test]
fn nested_lambdas_keep_each_synthetic_method_and_capture_in_scope() {
    let folder = tempfile::tempdir().unwrap();
    let project = folder.path();
    fs::write(
        project.join("main.oxel"),
        include_str!("../examples/nested_lambdas.oxel"),
    )
    .unwrap();
    let result = success(oxel(project, &["run", "--framework", "main.oxel"]));
    assert_eq!(
        result.lines().collect::<Vec<_>>(),
        ["23", "33", "25", "35", "0"]
    );
}

#[test]
fn mutable_lambdas_share_storage_and_keep_loop_instances_distinct() {
    let folder = tempfile::tempdir().unwrap();
    let project = folder.path();
    fs::write(
        project.join("main.oxel"),
        include_str!("../examples/mutable_captures.oxel"),
    )
    .unwrap();
    let result = success(oxel(project, &["run", "--framework", "main.oxel"]));
    assert_eq!(
        result.lines().collect::<Vec<_>>(),
        ["30", "32", "1", "2", "2", "3"]
    );
}

// Windows HTTP.sys can require URL registration privileges for an ephemeral prefix.
#[cfg(unix)]
#[test]
fn http_server_example_serves_real_loopback_requests() {
    use std::{
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        process::{Child, Stdio},
        thread,
        time::{Duration, Instant},
    };

    struct Server(Child);
    impl Drop for Server {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    let folder = tempfile::tempdir().unwrap();
    let project = folder.path();
    fs::write(
        project.join("http_server.oxel"),
        include_str!("../examples/http_server.oxel"),
    )
    .unwrap();
    let artifact = success(oxel(
        project,
        &["compile", "--framework", "http_server.oxel"],
    ));
    let artifact = project.join(artifact.trim());
    let invalid = Command::new("dotnet")
        .arg(&artifact)
        .arg("0")
        .output()
        .unwrap();
    assert_eq!(invalid.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("Usage: http_server"));

    let reservation = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let address = reservation.local_addr().unwrap();
    drop(reservation);
    let mut server = Server(
        Command::new("dotnet")
            .arg(&artifact)
            .arg(address.port().to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    let ready = loop {
        if let Some(status) = server.0.try_wait().unwrap() {
            let mut error = String::new();
            server
                .0
                .stderr
                .take()
                .unwrap()
                .read_to_string(&mut error)
                .unwrap();
            panic!("HTTP example exited ({status}): {error}");
        }
        if let Ok(socket) = TcpStream::connect_timeout(&address, Duration::from_millis(100)) {
            break socket;
        }
        assert!(
            Instant::now() < deadline,
            "HTTP example did not listen on {address}"
        );
        thread::sleep(Duration::from_millis(25));
    };
    let request = |mut socket: TcpStream, method, path, status, expected_body| {
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        socket
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        write!(
            socket,
            "{method} {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\nContent-Length: 0\r\n\r\n"
        )
        .unwrap();
        let mut response = String::new();
        socket.read_to_string(&mut response).unwrap();
        let (headers, body) = response.split_once("\r\n\r\n").unwrap();
        assert_eq!(
            headers.split_whitespace().nth(1),
            Some(status),
            "{response}"
        );
        let headers = headers.to_ascii_lowercase();
        assert!(headers.contains("content-type: text/plain; charset=utf-8"));
        assert_eq!(body, expected_body);
        headers
    };
    request(ready, "GET", "/health", "200", "ok\n");
    for (path, status, body) in [
        ("/add?left=10&right=20", "200", "30\n"),
        (
            "/add?left=2147483647&right=2147483647",
            "200",
            "4294967294\n",
        ),
        (
            "/add?left=bad&right=20",
            "400",
            "left and right must be 32-bit integers\n",
        ),
        (
            "/add?left=10",
            "400",
            "left and right must be 32-bit integers\n",
        ),
        ("/missing", "404", "not found\n"),
    ] {
        let socket = TcpStream::connect_timeout(&address, Duration::from_secs(1)).unwrap();
        request(socket, "GET", path, status, body);
    }
    let socket = TcpStream::connect_timeout(&address, Duration::from_secs(1)).unwrap();
    let headers = request(socket, "POST", "/health", "405", "method not allowed\n");
    assert!(headers.lines().any(|line| line == "allow: get"));
}

#[test]
fn clr_throw_terminates_an_expression_with_the_original_exception() {
    let folder = tempfile::tempdir().unwrap();
    let project = folder.path();
    fs::write(
        project.join("main.oxel"),
        include_str!("../examples/exceptions.oxel"),
    )
    .unwrap();
    assert_eq!(
        success(oxel(project, &["run", "--framework", "main.oxel"])).trim(),
        "30"
    );
    let result = oxel(
        project,
        &["run", "--framework", "main.oxel", "--", "disable"],
    );
    assert!(!result.status.success());
    assert!(
        String::from_utf8_lossy(&result.stderr)
            .contains("System.InvalidOperationException: offset is disabled"),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn clr_throw_checks_operand_types_and_definite_assignment() {
    let folder = tempfile::tempdir().unwrap();
    let project = folder.path();
    fs::write(
        project.join("main.oxel"),
        r#"
fn offset(ok: bool) -> i32 {
    let value: i32;
    if ok {
        value = 20;
    } else {
        throw new System.Exception();
    }
    value
}

fn main() {
    let result = 10 + {
        while true {
            throw { break; };
        }
        offset(true)
    };
    System.Console.WriteLine(result);
}
"#,
    )
    .unwrap();
    assert_eq!(
        success(oxel(project, &["run", "--framework", "main.oxel"])).trim(),
        "30"
    );
    for (source, code, message) in [
        (
            "fn main() { throw 42; }",
            "E0301",
            "expected System.Exception",
        ),
        (
            "fn fail(ex: System.Exception?) { throw ex; } fn main() {}",
            "E0301",
            "found System.Exception?",
        ),
        (
            "fn main() { let ex: System.Exception; throw ex; }",
            "E0321",
            "ex",
        ),
        (
            "const VALUE: i32 = throw new System.Exception(); fn main() {}",
            "E0501",
            "constant initializer",
        ),
    ] {
        fs::write(project.join("main.oxel"), source).unwrap();
        let result = oxel(project, &["check", "--framework", "main.oxel"]);
        assert!(!result.status.success(), "{source}");
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(
            stderr.contains(code) && stderr.contains(message),
            "{stderr}"
        );
    }

    fs::write(project.join("main.oxel"), "fn main() { throw 42; }").unwrap();
    let result = oxel(project, &["check", "main.oxel"]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("E0316"));
}

#[test]
fn clr_framework_value_structs_execute_through_constructors_and_interface_calls() {
    let folder = tempfile::tempdir().unwrap();
    let project = folder.path();
    fs::write(
        project.join("main.oxel"),
        include_str!("../examples/date_ranges.oxel"),
    )
    .unwrap();
    let result = success(oxel(project, &["run", "main.oxel", "--framework"]));
    assert_eq!(result.lines().collect::<Vec<_>>(), ["22", "True"]);
}

#[test]
fn clr_framework_generic_structs_keep_constructed_value_signatures() {
    let folder = tempfile::tempdir().unwrap();
    let project = folder.path();
    let source = include_str!("../examples/key_value_pairs.oxel");
    fs::write(project.join("main.oxel"), source).unwrap();
    let result = success(oxel(project, &["run", "main.oxel", "--framework"]));
    assert_eq!(result.lines().collect::<Vec<_>>(), ["score", "30"]);
}

#[test]
fn maps_clr_nullable_values_to_the_standard_option() {
    let folder = tempfile::tempdir().unwrap();
    let project = folder.path();
    fs::write(
        project.join("main.oxel"),
        include_str!("../examples/clr_option.oxel"),
    )
    .unwrap();
    assert_eq!(
        success(oxel(project, &["run", "--framework", "main.oxel"]))
            .lines()
            .collect::<Vec<_>>(),
        [
            "Grace",
            "Found Ada",
            "missing",
            "no match",
            "0",
            "no number",
            "1",
            "Grace",
            "empty label",
            "Ada",
            "0",
            "False",
            "2",
            "Grace",
            "fallback",
            "3",
            "first",
            "second"
        ]
    );
}

#[test]
fn framework_annotated_generic_returns_use_the_concrete_type() {
    let folder = tempfile::tempdir().unwrap();
    let project = folder.path();
    fs::write(
        project.join("main.oxel"),
        include_str!("../examples/annotated_clr_generics.oxel"),
    )
    .unwrap();
    assert_eq!(
        success(oxel(project, &["run", "--framework", "main.oxel"]))
            .lines()
            .collect::<Vec<_>>(),
        ["missing", "0"]
    );
}

#[test]
fn framework_metadata_supports_io_collections_and_runtime_packaging() {
    let folder = tempfile::tempdir().unwrap();
    let project = folder.path();
    fs::write(project.join("main.oxel"), EXAMPLE).unwrap();
    fs::write(project.join("input file.txt"), "oxel\n   \nCLR\n").unwrap();

    let without_framework = oxel(project, &["check", "main.oxel"]);
    assert!(!without_framework.status.success());
    assert!(String::from_utf8_lossy(&without_framework.stderr).contains("System.IO.File.Exists"));

    assert_eq!(
        success(oxel(project, &["check", "--framework", "main.oxel"])),
        ""
    );
    let artifact = success(oxel(project, &["compile", "--framework", "main.oxel"]));
    let artifact = project.join(artifact.trim());
    assert!(artifact.is_file());
    assert!(!artifact.with_file_name("System.Runtime.dll").exists());
    assert!(!artifact.with_file_name("System.Collections.dll").exists());
    let config: serde_json::Value =
        serde_json::from_slice(&fs::read(artifact.with_extension("runtimeconfig.json")).unwrap())
            .unwrap();
    let sdk = success(
        Command::new("dotnet")
            .current_dir(project)
            .arg("--version")
            .output()
            .unwrap(),
    );
    let sdk = sdk.trim().split('.').take(2).collect::<Vec<_>>().join(".");
    assert_eq!(config["runtimeOptions"]["tfm"], format!("net{sdk}"));
    let direct = Command::new("dotnet")
        .current_dir(project)
        .arg(artifact)
        .arg("input file.txt")
        .output()
        .unwrap();
    assert_eq!(
        success(direct).lines().collect::<Vec<_>>(),
        ["input file.txt", "2"]
    );

    fs::write(
        project.join("oxel.toml"),
        "[package]\nname = 'lines'\nsources = ['main.oxel']\nframework = true",
    )
    .unwrap();
    let run = oxel(project, &["run", "--", "input file.txt"]);
    assert_eq!(
        success(run).lines().collect::<Vec<_>>(),
        ["input file.txt", "2"]
    );
}

#[cfg(unix)]
#[test]
fn framework_checks_probe_the_manifest_sdk_and_fall_back_to_restore() {
    use std::{env, os::unix::fs::PermissionsExt};

    let folder = tempfile::tempdir().unwrap();
    let outside = folder.path();
    let project = outside.join("project");
    fs::create_dir(&project).unwrap();
    fs::write(project.join("main.oxel"), EXAMPLE).unwrap();
    fs::write(
        project.join("oxel.toml"),
        "[package]\nname = 'lines'\nsources = ['main.oxel']\nframework = true",
    )
    .unwrap();
    let version = success(
        Command::new("dotnet")
            .current_dir(&project)
            .arg("--version")
            .output()
            .unwrap(),
    );
    let settings = |version: &str| {
        serde_json::json!({"sdk": {"version": version, "rollForward": "disable"}}).to_string()
    };
    fs::write(project.join("global.json"), settings(version.trim())).unwrap();
    // A check launched from here must resolve the SDK relative to the manifest.
    fs::write(outside.join("global.json"), settings("99.0.100")).unwrap();

    let shims = outside.join("shims");
    fs::create_dir(&shims).unwrap();
    let shim = shims.join("dotnet");
    fs::write(
        &shim,
        r#"#!/bin/sh
printf '%s\n' "$*" >> "$OXEL_TEST_SDK_LOG"
if [ "$OXEL_TEST_SKIP_PROBE" = 1 ]; then
    for arg do
        if [ "$arg" = '-p:SkipResolvePackageAssets=true' ]; then exit 1; fi
    done
fi
PATH="$OXEL_TEST_SDK_PATH"
export PATH
exec dotnet "$@"
"#,
    )
    .unwrap();
    fs::set_permissions(shim, fs::Permissions::from_mode(0o755)).unwrap();
    let path = env::var_os("PATH").unwrap();
    let instrumented =
        env::join_paths(std::iter::once(shims).chain(env::split_paths(&path))).unwrap();
    let log = outside.join("dotnet.log");
    let check = |skip_probe: bool| {
        fs::write(&log, "").unwrap();
        let result = Command::new(env!("CARGO_BIN_EXE_oxel"))
            .current_dir(outside)
            .args(["check", "--manifest-path", "project/oxel.toml"])
            .env("PATH", &instrumented)
            .env("OXEL_TEST_SDK_PATH", &path)
            .env("OXEL_TEST_SDK_LOG", &log)
            .env("OXEL_TEST_SKIP_PROBE", if skip_probe { "1" } else { "0" })
            .output()
            .unwrap();
        fs::read_to_string(&log).map(|log| (result, log)).unwrap()
    };
    let (result, calls) = check(false);
    assert_eq!(success(result), "");
    let calls: Vec<_> = calls.lines().collect();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert!(calls[0].contains("-p:SkipResolvePackageAssets=true"));
    assert!(!calls[0].contains("-restore"));

    let (result, calls) = check(true);
    assert_eq!(success(result), "");
    let calls: Vec<_> = calls.lines().collect();
    assert_eq!(calls.len(), 3, "{calls:?}");
    assert_eq!(calls[1], "--version");
    assert!(calls[2].contains("-restore"));

    fs::write(project.join("global.json"), settings("99.0.100")).unwrap();
    let (result, _) = check(false);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("E0010"));
}

#[test]
#[ignore = "requires the .NET SDK Native AOT and platform linker toolchain"]
fn framework_program_publishes_a_native_executable() {
    let folder = tempfile::tempdir().unwrap();
    let project = folder.path();
    fs::write(project.join("main.oxel"), EXAMPLE).unwrap();
    fs::write(project.join("input.txt"), "oxel\n\n").unwrap();
    let executable = success(oxel(project, &["publish", "main.oxel", "--framework"]));
    let run = Command::new(project.join(executable.trim()))
        .current_dir(project)
        .env("PATH", "")
        .env_remove("DOTNET_ROOT")
        .arg("input.txt")
        .output()
        .unwrap();
    assert_eq!(success(run).lines().collect::<Vec<_>>(), ["input.txt", "1"]);
}

#[test]
fn clr_aliases_call_instance_methods_with_reference_or_value_receivers() {
    let folder = tempfile::tempdir().unwrap();
    let project = folder.path();
    fs::write(
        project.join("main.oxel"),
        r#"
        fn report(n: i32) {
            System.Console.WriteLine(n.ToString("D3"));
        }
        fn main() {
            let text: string? = " hello ";
            if text != null {
                System.Console.WriteLine(text.Trim().Substring(1, 3));
            }
            System.Console.WriteLine("abc".Contains('b'));
            report(System.Int32.Parse("30"));
            System.Console.WriteLine((2 + 4).CompareTo(6));
            System.Console.WriteLine('Z'.ToString());
            System.Console.WriteLine(true.ToString());
            let boxed: object = 42;
            System.Console.WriteLine(boxed.ToString());
            let mut value = 10;
            System.Console.WriteLine(value.CompareTo({ value = 40; 20 }) < 0);
            System.Console.WriteLine(value);
            System.Console.WriteLine((42).GetType().Name);
        }
        "#,
    )
    .unwrap();

    let result = success(oxel(project, &["run", "main.oxel", "--framework"]));
    assert_eq!(
        result.lines().collect::<Vec<_>>(),
        [
            "ell", "True", "030", "0", "Z", "True", "42", "True", "40", "Int32"
        ]
    );

    for (source, expected) in [
        (
            "fn main() { let value: string? = null; value.Trim(); }",
            "E0311",
        ),
        ("fn main() { (1).CompareTo(); }", "E0205"),
        (
            "fn accept(value: System.Span<i32>) {} fn main() {}",
            "E0202",
        ),
    ] {
        fs::write(project.join("main.oxel"), source).unwrap();
        let result = oxel(project, &["check", "main.oxel", "--framework"]);
        assert!(!result.status.success());
        let diagnostic = String::from_utf8(result.stderr).unwrap();
        assert!(diagnostic.contains(expected), "{source}: {diagnostic}");
    }
}

#[test]
fn clr_enum_values_keep_their_type_across_calls_arrays_and_boxing() {
    let folder = tempfile::tempdir().unwrap();
    let project = folder.path();
    fs::write(project.join("main.oxel"), ENUM_EXAMPLE).unwrap();
    let result = success(oxel(project, &["run", "main.oxel", "--framework"]));
    assert_eq!(
        result.lines().collect::<Vec<_>>(),
        ["True", "True", "OrdinalIgnoreCase"]
    );

    fs::write(
        project.join("main.oxel"),
        r#"
        fn main() {
            let expected = System.StringComparison.Ordinal;
            let values = new System.StringComparison[1];
            values[0] = expected;
            let list = new System.Collections.Generic.List<System.StringComparison>();
            list.Add(values[0]);
            let value = list[0];
            let boxed: System.Enum = value;
            System.Console.WriteLine((boxed as System.StringComparison) == expected);
            System.Console.WriteLine(boxed is System.StringComparison);
            System.Console.WriteLine(boxed is i32);
            System.Console.WriteLine(System.Enum.IsDefined::<System.StringComparison>(value));
        }
    "#,
    )
    .unwrap();
    let result = success(oxel(project, &["run", "main.oxel", "--framework"]));
    assert_eq!(
        result.lines().collect::<Vec<_>>(),
        ["True", "True", "False", "True"]
    );

    fs::write(
        project.join("main.oxel"),
        r#"
        fn main() {
            let mode: System.StringComparison = 4;
        }
    "#,
    )
    .unwrap();
    let result = oxel(project, &["check", "main.oxel", "--framework"]);
    assert!(!result.status.success());
    assert!(
        String::from_utf8(result.stderr)
            .unwrap()
            .contains("expected System.StringComparison, found System.Int32")
    );
}

#[test]
fn nullable_values_use_clr_storage_and_lazy_fallbacks() {
    let folder = tempfile::tempdir().unwrap();
    let project = folder.path();
    fs::write(
        project.join("main.oxel"),
        include_str!("../examples/nullable_values.oxel"),
    )
    .unwrap();
    let result = success(oxel(project, &["run", "main.oxel", "--framework"]));
    assert_eq!(result.lines().collect::<Vec<_>>(), ["30", "-1"]);

    let source = r#"
        fn box_nullable<T>(value: T) -> object? { value }
        fn main() {
            let some = if true { 30 } else { null };
            let none = new System.Nullable<i32>();
            let mut evals = 0;
            let present = { evals += 1; some } ?? { evals += 1; 9 };
            System.Console.WriteLine(present);
            System.Console.WriteLine(evals);
            System.Console.WriteLine(none ?? 20);
            System.Console.WriteLine(none.HasValue);
            System.Console.WriteLine(some as i32);
            let list = new System.Collections.Generic.List<i32?>();
            list.Add(null);
            list.Add(10);
            System.Console.WriteLine(list[0] ?? 25);
            System.Console.WriteLine(list[1].Value);
            let boxed = box_nullable(none);
            System.Console.WriteLine((boxed as i32?) == null);
            let values = new i32?[2];
            values[1] = 40;
            System.Console.WriteLine(values[0] == null);
            System.Console.WriteLine(values[1] ?? 0);
        }
    "#;
    fs::write(project.join("main.oxel"), source).unwrap();
    let result = success(oxel(project, &["run", "main.oxel", "--framework"]));
    assert_eq!(
        result.lines().collect::<Vec<_>>(),
        [
            "30", "1", "20", "False", "30", "25", "10", "True", "True", "40"
        ]
    );

    for (source, code) in [
        (
            "fn main() { let value: i32? = null; let boxed: object = value; }",
            "E0301",
        ),
        (
            "fn f<T: struct>(value: T) {} fn main() { let n: i32? = null; f(n); }",
            "E0212",
        ),
        (
            "fn main() { let n: System.Nullable<string> = null; }",
            "E0212",
        ),
        (
            "fn main() { let n: System.Nullable<i32>? = null; }",
            "E0311",
        ),
    ] {
        fs::write(project.join("main.oxel"), source).unwrap();
        let result = oxel(project, &["check", "main.oxel", "--framework"]);
        assert!(!result.status.success());
        let error = String::from_utf8(result.stderr).unwrap();
        assert!(error.contains(code), "{source}: {error}");
    }
    fs::write(
        project.join("main.oxel"),
        r#"
        fn forward(ignored: i32, boxed: object) {}
        fn uses_object<T>(value: T) { forward(1, value); }
        fn main() {
            let some: i32? = 30;
            uses_object(some);
            System.Console.WriteLine("present");
            let none: i32? = null;
            uses_object(none);
        }
    "#,
    )
    .unwrap();
    let result = oxel(project, &["run", "main.oxel", "--framework"]);
    assert!(!result.status.success());
    assert_eq!(String::from_utf8(result.stdout).unwrap().trim(), "present");
    assert!(
        String::from_utf8(result.stderr)
            .unwrap()
            .contains("System.InvalidOperationException")
    );
}

#[test]
fn unsigned_aliases_resolve_unsigned_framework_signatures_and_literals() {
    let folder = tempfile::tempdir().unwrap();
    let project = folder.path();
    fs::write(
        project.join("main.oxel"),
        r#"
        const MAX = System.UInt64.MaxValue;
        fn main() {
            let data = System.BitConverter.GetBytes(MAX);
            System.Console.WriteLine(System.BitConverter.ToUInt64(data, 0));
            System.Console.WriteLine(System.Math.Max(0, 18446744073709551615));
            System.Console.WriteLine(System.Math.Max(0, 0xffff_ffff_ffff_ffff));
            System.Console.WriteLine(System.UInt32.MaxValue);
            let half: System.UInt16 = System.UInt16.Parse("65535");
            System.Console.WriteLine(half);
        }
    "#,
    )
    .unwrap();
    let result = success(oxel(project, &["run", "main.oxel", "--framework"]));
    assert_eq!(
        result.lines().collect::<Vec<_>>(),
        [
            "18446744073709551615",
            "18446744073709551615",
            "18446744073709551615",
            "4294967295",
            "65535"
        ]
    );
}

#[test]
fn generic_clr_constraints_cannot_export_internal_type_arguments() {
    let folder = tempfile::tempdir().unwrap();
    let project = folder.path();
    for declaration in [
        "fn api<T: System.Collections.Generic.IComparer<Hidden>>() {}",
        "struct Api<T: System.Collections.Generic.IComparer<Hidden>> {}",
    ] {
        let source = format!("internal struct Hidden {{}}\n{declaration}\nfn main() {{}}");
        fs::write(project.join("main.oxel"), &source).unwrap();
        let output = oxel(project, &["check", "--framework", "main.oxel"]);
        assert!(!output.status.success());
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(error.contains("E0342"), "{error}");
        assert!(error.contains("internal type 'Hidden'"), "{error}");
    }
}

#[test]
fn clr_typed_catches_restore_pending_operands_and_match_in_order() {
    let folder = tempfile::tempdir().unwrap();
    let project = folder.path();
    fs::write(
        project.join("sample.oxel"),
        include_str!("../examples/catch.oxel"),
    )
    .unwrap();
    let sample = success(oxel(project, &["run", "--framework", "sample.oxel"]));
    assert_eq!(
        sample.lines().collect::<Vec<_>>(),
        ["15", "30", "OverflowException", "31"]
    );

    let source = r#"
        struct Record { value: i32 }

        fn main() {
            let record = Record { value: try { System.Int32.Parse("bad") }
                catch (_: System.FormatException) { 30 } };
            let array = [try { System.Int32.Parse("bad") }
                catch (_: System.FormatException) { record.value }];
            let index = array[try { System.Int32.Parse("bad") }
                catch (_: System.FormatException) { 0 }];
            let mut slot = 7;
            let old = System.Threading.Interlocked.Exchange(
                ref slot,
                try { System.Int32.Parse("bad") } catch (_: System.FormatException) { index },
            );
            System.Console.WriteLine(old);
            System.Console.WriteLine(slot);
            System.Console.WriteLine($"caught {try { throw new System.Exception() } catch (_: System.Exception) { "yes" }}");
            let nested = try {
                try { throw new System.InvalidOperationException() }
                catch (_: System.FormatException) { 0 }
            } catch (_: System.InvalidOperationException) {
                try { System.Int32.Parse("bad") }
                catch (_: System.FormatException) { 42 }
            };
            System.Console.WriteLine(nested);

            let mut chosen: i32;
            try { chosen = System.Int32.Parse("bad"); }
            catch (_: System.FormatException) { chosen = 12; }
            System.Console.WriteLine(chosen);

            let mut visits = 0;
            try {
                for i in 0..=3 {
                    if i == 1 { continue; }
                    if i == 3 { break; }
                    visits += i + 1;
                }
            } catch (_: System.Exception) {}
            System.Console.WriteLine(visits);

            let mut captured = 1;
            System.Array.Find([3]) |item| {
                captured = try { System.Int32.Parse("bad") }
                catch (_: System.FormatException) { captured + item };
                true
            };
            System.Console.WriteLine(captured);

            let input: Option<i32> = Option.None;
            let recovered = try {
                let fallback: Option<i32> = try { input? };
                fallback ?? 11
            } catch (_: System.Exception) { 0 };
            System.Console.WriteLine(recovered);
        }
    "#;
    fs::write(project.join("main.oxel"), source).unwrap();
    let result = success(oxel(project, &["run", "--framework", "main.oxel"]));
    assert_eq!(
        result.lines().collect::<Vec<_>>(),
        ["7", "30", "caught yes", "42", "12", "4", "4", "11"]
    );
}

#[test]
fn clr_exception_handlers_check_reachability_and_definite_assignment() {
    let folder = tempfile::tempdir().unwrap();
    let project = folder.path();
    for (body, code, message) in [
        ("try {} catch (_: string) {}", "E0344", "exception class"),
        (
            "try {} catch (_: System.Exception) {} catch (_: System.FormatException) {}",
            "E0344",
            "earlier catch",
        ),
        (
            "let n: i32; try { n = 1; throw new System.Exception() } catch (_: System.Exception) { n = 2; }",
            "E0322",
            "already be assigned",
        ),
        (
            "let n: i32; try { n = System.Int32.Parse(\"bad\"); } catch (_: System.Exception) { System.Console.WriteLine(n); }",
            "E0321",
            "unassigned",
        ),
    ] {
        let source = format!("fn main() {{ {body} }}");
        fs::write(project.join("main.oxel"), &source).unwrap();
        let output = oxel(project, &["check", "--framework", "main.oxel"]);
        assert!(!output.status.success(), "{source}");
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(
            error.contains(code) && error.contains(message),
            "{source}: {error}"
        );
    }
    let errors = oxel::check("fn main() { try {} catch (_: System.Exception) {} }").unwrap_err();
    assert_eq!(errors[0].code, "E0344");
}

#[test]
fn disposable_using_cleans_up_on_every_clr_control_transfer() {
    let folder = tempfile::tempdir().unwrap();
    let project = folder.path();
    fs::write(
        project.join("sample.oxel"),
        include_str!("../examples/using.oxel"),
    )
    .unwrap();
    assert_eq!(
        success(oxel(project, &["run", "--framework", "sample.oxel"])),
        "oxel\ndisposed\n"
    );

    let source = r#"
        fn disposed(reader: System.IO.StringReader) -> bool {
            try { reader.Peek(); false }
            catch (_: System.ObjectDisposedException) { true }
        }

        fn early(reader: System.IO.StringReader) -> i32 {
            using resource = reader {
                try { throw new System.FormatException() }
                catch (_: System.FormatException) { return resource.Read(); }
            }
        }

        fn propagate(reader: System.IO.StringReader) -> Option<string> {
            using resource = reader {
                Option.Some(resource.ReadLine()?)
            }
        }

        fn add(left: i32, right: i32) -> i32 { left + right }

        fn main() {
            let normal = new System.IO.StringReader("normal");
            let mut acquired = 0;
            let length = 1 + using resource = {
                acquired += 1;
                normal
            } {
                (resource.ReadLine() ?? "").Length
            };
            System.Console.WriteLine(length);
            System.Console.WriteLine(acquired);
            System.Console.WriteLine(disposed(normal));

            let returning = new System.IO.StringReader("A");
            System.Console.WriteLine(early(returning));
            System.Console.WriteLine(disposed(returning));

            let empty = new System.IO.StringReader("");
            System.Console.WriteLine(propagate(empty) ?? "empty");
            System.Console.WriteLine(disposed(empty));

            let raising = new System.IO.StringReader("open");
            try {
                using resource = raising { throw new System.FormatException(); }
            } catch (_: System.FormatException) {
                System.Console.WriteLine(disposed(raising));
            }

            let outer = new System.IO.StringReader("outer");
            let inner = new System.IO.StringReader("inner");
            using outer_input = outer {
                using inner_input = inner {}
                System.Console.WriteLine(disposed(inner));
                System.Console.WriteLine(disposed(outer_input));
            }
            System.Console.WriteLine(disposed(outer));

            let breaking = new System.IO.StringReader("break");
            let sum = 10 + {
                while true {
                    using resource = breaking { break; }
                }
                20
            };
            System.Console.WriteLine(sum);
            System.Console.WriteLine(disposed(breaking));

            let continuing = new System.IO.StringReader("continue");
            let mut visits = 0;
            let result = 10 + {
                for i in 0..=2 {
                    using resource = continuing {
                        if i == 1 { continue; }
                        visits += i + 1;
                    }
                }
                visits
            };
            System.Console.WriteLine(result);
            System.Console.WriteLine(disposed(continuing));

            let attempt_reader = new System.IO.StringReader("try");
            let missing: Option<i32> = Option.None;
            let sum = add(12, (try { using resource = attempt_reader { missing? } }) ?? 18);
            System.Console.WriteLine(sum);
            System.Console.WriteLine(disposed(attempt_reader));
        }
    "#;
    fs::write(project.join("main.oxel"), source).unwrap();
    let output = success(oxel(project, &["run", "--framework", "main.oxel"]));
    assert_eq!(
        output.lines().collect::<Vec<_>>(),
        [
            "7", "1", "True", "65", "True", "empty", "True", "True", "True", "False", "True", "30",
            "True", "14", "True", "30", "True",
        ]
    );
}

#[test]
fn using_checks_disposable_types_and_limits_the_binding_scope() {
    let folder = tempfile::tempdir().unwrap();
    let project = folder.path();
    for (source, code) in [
        ("fn main() { using resource = 1 {} }", "E0345"),
        (
            "fn main() { let reader: System.IO.StringReader? = null; using resource = reader {} }",
            "E0345",
        ),
        (
            "fn main() { using resource = new System.IO.StringReader(\"\") {} System.Console.WriteLine(resource); }",
            "E0203",
        ),
    ] {
        fs::write(project.join("main.oxel"), source).unwrap();
        let output = oxel(project, &["check", "--framework", "main.oxel"]);
        assert!(!output.status.success(), "{source}");
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(error.contains(code), "{source}: {error}");
    }
    assert_eq!(
        oxel::check("fn main() { using x = 1 {} }").unwrap_err()[0].code,
        "E0345"
    );
}
