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

const EXAMPLE: &str = include_str!("../../../examples/file_lines.oxel");
const ENUM_EXAMPLE: &str = include_str!("../../../examples/enum_interop.oxel");

#[test]
fn generic_async_functions_infer_and_restore_parameters_after_suspension() {
    let folder = tempfile::tempdir().unwrap();
    let project = folder.path();
    fs::write(
        project.join("main.oxel"),
        include_str!("../../../examples/generic_async.oxel"),
    )
    .unwrap();
    assert_eq!(
        success(oxel(project, &["run", "--framework", "main.oxel"])),
        "oxel\n30\n"
    );
}

#[test]
fn async_tests_wait_report_exceptions_and_keep_running_in_source_order() {
    let folder = tempfile::tempdir().unwrap();
    let project = folder.path();
    let file = project.join("tests.oxel");
    fs::write(
        &file,
        r#"
        use System::Threading::Tasks::Task;

        #[test]
        async fn passes() {
            Task.Delay(10).await;
            System.Console.WriteLine("resumed");
        }

        #[test]
        async fn fails_after_await() {
            Task.Delay(10).await;
            assert(false, "async failure");
        }

        #[test]
        #[ignore]
        async fn skipped() {
            Task.Delay(1).await;
        }

        #[test]
        fn continues() {}
        "#,
    )
    .unwrap();
    let result = oxel(project, &["test", "--framework", "tests.oxel"]);
    assert_eq!(
        result.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let output = String::from_utf8(result.stdout).unwrap();
    assert!(
        output.starts_with("resumed\nok passes\nFAIL fails_after_await\n"),
        "{output}"
    );
    assert!(
        output.contains("System.InvalidOperationException: async failure"),
        "{output}"
    );
    assert!(
        output.ends_with("ignored skipped\nok continues\n3 tests, 1 failed, 1 ignored\n"),
        "{output}"
    );

    assert_eq!(
        success(oxel(
            project,
            &[
                "test",
                "--framework",
                "--include-ignored",
                "--filter",
                "skipped",
                "tests.oxel"
            ]
        )),
        "ok skipped\n1 test, 0 failed\n"
    );
    fs::write(&file, "#[test] async fn invalid() -> i32 { 42 }").unwrap();
    let invalid = oxel(project, &["check", "--framework", "--tests", "tests.oxel"]);
    assert!(!invalid.status.success());
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("E0346"));
}

#[test]
fn async_lambdas_work_in_generic_functions_and_trailing_calls() {
    let folder = tempfile::tempdir().unwrap();
    let project = folder.path();
    let main = project.join("main.oxel");
    fs::write(&main, include_str!("../../../examples/async_catch.oxel")).unwrap();
    assert_eq!(
        success(oxel(project, &["run", "--framework", "main.oxel"]))
            .lines()
            .collect::<Vec<_>>(),
        ["handled", "30", "1"]
    );

    fs::write(
        &main,
        r#"
        use System::{Func, Action};
        use System::Threading::Tasks::{Task, TaskCompletionSource, ValueTask};

        fn factory<T>(value: T) -> Func<Task<T>> {
            async { Task.Delay(1).await; value }
        }

        async fn main() {
            let produced = factory(30);
            System.Console.WriteLine(produced().await);
            let add: Func<i32, ValueTask<i32>> = async |value| { Task.Delay(1).await; value + 1 };
            System.Console.WriteLine(add(30).await);
            let pending = new TaskCompletionSource();
            let completed = new TaskCompletionSource<i32>();
            let notify: Action = async || { pending.Task.await; completed.SetResult(32); };
            notify();
            assert(!completed.Task.IsCompleted);
            pending.SetResult();
            System.Console.WriteLine(completed.Task.await);
            let running = Task.Run() async { Task.Delay(1).await; System.Console.WriteLine(33); };
            running.await;
        }
    "#,
    )
    .unwrap();
    assert_eq!(
        success(oxel(project, &["run", "--framework", "main.oxel"]))
            .lines()
            .collect::<Vec<_>>(),
        ["30", "31", "32", "33"]
    );
    fs::write(
        &main,
        "fn main() { let bad: System.Func<i32> = async { 1 }; }",
    )
    .unwrap();
    let invalid = oxel(project, &["check", "--framework", "main.oxel"]);
    assert!(!invalid.status.success());
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("E0351"));
}

#[test]
fn postfix_await_suspends_and_resumes_without_blocking_the_caller() {
    use std::{
        process::Stdio,
        thread,
        time::{Duration, Instant},
    };

    let folder = tempfile::tempdir().unwrap();
    let project = folder.path();
    let main = project.join("main.oxel");
    fs::write(&main, include_str!("../../../examples/await.oxel")).unwrap();
    let dll = success(oxel(project, &["compile", "--framework", "main.oxel"]));
    let mut process = Command::new("dotnet")
        .arg(dll.trim())
        .current_dir(project)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while process.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            process.kill().unwrap();
            let _ = process.wait();
            panic!("the async example did not finish after completing the task");
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(success(process.wait_with_output().unwrap()).trim(), "30");

    fs::write(&main, r#"
        async fn read(source: System.Threading.Tasks.Task<i32>) -> i32 {
            let value = source.await;
            let mut iterations = 0;
            while iterations < 2 {
                System.Threading.Tasks.Task.Delay(5).await;
                iterations += 1;
            }
            return value + iterations;
        }

        fn check_failures() {
            let source = new System.Threading.Tasks.TaskCompletionSource<i32>();
            let failed: System.Threading.Tasks.Task = read(source.Task);
            source.SetException(new System.InvalidOperationException("preserved"));
            try { failed.GetAwaiter().GetResult(); }
            catch (error: System.InvalidOperationException) { System.Console.WriteLine(error.Message); };

            let source = new System.Threading.Tasks.TaskCompletionSource<i32>();
            let canceled: System.Threading.Tasks.Task = read(source.Task);
            source.SetCanceled();
            try { canceled.GetAwaiter().GetResult(); }
            catch (_: System.OperationCanceledException) { System.Console.WriteLine(canceled.IsCanceled); };
        }

        async fn change_context(slot: System.Threading.AsyncLocal<i32>, gate: System.Threading.Tasks.Task) -> i32 {
            slot.Value = 17;
            gate.await;
            slot.Value
        }

        async fn format_after_suspend(task: System.Threading.Tasks.Task<i32>, prefix: string?) -> string {
            $"{prefix}{12}:{task.await}"
        }

        async fn main() {
            check_failures();
            System.Console.WriteLine(read(System.Threading.Tasks.Task.FromResult(28)).await);
            let slot = new System.Threading.AsyncLocal<i32>();
            slot.Value = 3;
            let gate = new System.Threading.Tasks.TaskCompletionSource();
            let pending = change_context(slot, gate.Task);
            System.Console.WriteLine(slot.Value);
            gate.SetResult();
            System.Console.WriteLine(pending.await);
            let input = new System.Threading.Tasks.TaskCompletionSource<i32>();
            let formatted = format_after_suspend(input.Task, null);
            assert(!formatted.IsCompleted);
            input.SetResult(30);
            System.Console.WriteLine(formatted.await);
        }
    "#).unwrap();
    assert_eq!(
        success(oxel(project, &["run", "--framework", "main.oxel"]))
            .lines()
            .collect::<Vec<_>>(),
        ["preserved", "True", "30", "3", "17", "12:30"]
    );

    for (source, message) in [
        (
            "fn main() { System.Threading.Tasks.Task.Delay(1).await; }",
            "requires an async function",
        ),
        (
            "async fn main() { 1.await; }",
            "requires a non-null CLR Task or ValueTask",
        ),
    ] {
        fs::write(&main, source).unwrap();
        let result = oxel(project, &["check", "--framework", "main.oxel"]);
        assert!(!result.status.success(), "{source}");
        let error = String::from_utf8_lossy(&result.stderr);
        assert!(
            error.contains("E0351") && error.contains(message),
            "{error}"
        );
    }
}

#[test]
fn slices_share_arrays_and_cross_clr_signatures_by_value() {
    let folder = tempfile::tempdir().unwrap();
    let project = folder.path();
    let main = project.join("main.oxel");
    fs::write(&main, include_str!("../../../examples/slices.oxel")).unwrap();
    assert_eq!(
        success(oxel(project, &["run", "--framework", "main.oxel"]))
            .lines()
            .collect::<Vec<_>>(),
        ["30", "30", "True"]
    );

    fs::write(
        &main,
        r#"
        fn bound(value: i32) -> i32 { System.Console.WriteLine(value); value }
        fn main() {
            let data = [9, 8, 7];
            let view = new System.ReadOnlySpan<i32>(data, bound(0), bound(2));
            System.Console.WriteLine(view[1]);
            let empty = view.Slice(2, 0);
            System.Console.WriteLine(empty.Length);
            try { let invalid = view.Slice(2, -1); }
            catch (_: System.ArgumentOutOfRangeException) { System.Console.WriteLine(31); };
            try { let invalid = System.MemoryExtensions.AsSpan(data, 0, 4); }
            catch (_: System.ArgumentOutOfRangeException) { System.Console.WriteLine(32); };
            try { let invalid = view[2]; }
            catch (_: System.IndexOutOfRangeException) { System.Console.WriteLine(33); };
        }
    "#,
    )
    .unwrap();
    assert_eq!(
        success(oxel(project, &["run", "--framework", "main.oxel"]))
            .lines()
            .collect::<Vec<_>>(),
        ["0", "2", "8", "0", "31", "32", "33"]
    );

    for (source, code, message) in [
        (
            "fn main() { let data = [1]; let view = new System.ReadOnlySpan<i32>(data); view[0] = 2; }",
            "E0204",
            "read-only",
        ),
        (
            "struct Storage { view: System.ReadOnlySpan<i32> } fn main() {}",
            "E0350",
            "field type",
        ),
        (
            "fn bad(data: i32[]) -> System.ReadOnlySpan<i32> { new System.ReadOnlySpan<i32>(data) } fn main() {}",
            "E0350",
            "return type",
        ),
        (
            "fn main() { let data = [1]; let boxed: object = new System.ReadOnlySpan<i32>(data); }",
            "E0301",
            "expected System.Object",
        ),
        (
            "fn main() { let data = [1]; System.Console.WriteLine(new System.ReadOnlySpan<i32>(data)); }",
            "E0350",
            "boxed argument",
        ),
        (
            "fn main() { let data = [1]; let values = [new System.ReadOnlySpan<i32>(data)]; }",
            "E0350",
            "array element type",
        ),
        (
            "fn id<T>(value: T) -> T { value } fn main() { let data = [1]; id(new System.ReadOnlySpan<i32>(data)); }",
            "E0350",
            "generic argument",
        ),
        (
            "fn main() { let data = [1]; let view = new System.ReadOnlySpan<i32>(data); System.Array.ConvertAll::<i32, i32>([1]) |x| { view[0] + x }; }",
            "E0350",
            "captured value",
        ),
    ] {
        fs::write(&main, source).unwrap();
        let result = oxel(project, &["check", "--framework", "main.oxel"]);
        assert!(!result.status.success(), "{source}");
        let error = String::from_utf8_lossy(&result.stderr);
        assert!(
            error.contains(code) && error.contains(message),
            "{source}\n{error}"
        );
    }
}

#[test]
fn task_entry_point_awaits_completion_and_maps_the_result() {
    let folder = tempfile::tempdir().unwrap();
    let project = folder.path();
    let main = project.join("main.oxel");
    fs::write(&main, include_str!("../../../examples/async_main.oxel")).unwrap();
    assert_eq!(
        success(oxel(
            project,
            &["run", "--framework", "main.oxel", "--", "ready"]
        ))
        .trim(),
        "ready"
    );
    let failed = oxel(project, &["run", "--framework", "main.oxel"]);
    assert_eq!(failed.status.code(), Some(1));
    assert!(failed.stdout.is_empty());
    assert_eq!(
        String::from_utf8_lossy(&failed.stderr).trim(),
        "supply a command-line argument"
    );

    for (result, body, exit) in [
        ("Task", "System.Threading.Tasks.Task.Delay(10)", 0),
        (
            "Task<i32>",
            "System.Threading.Tasks.Task.FromResult(42)",
            42,
        ),
        (
            "ValueTask",
            "new System.Threading.Tasks.ValueTask(System.Threading.Tasks.Task.Delay(10))",
            0,
        ),
        (
            "ValueTask<i32>",
            "new System.Threading.Tasks.ValueTask<i32>(42)",
            42,
        ),
    ] {
        fs::write(
            &main,
            format!("fn main() -> System.Threading.Tasks.{result} {{ {body} }}"),
        )
        .unwrap();
        let output = oxel(project, &["run", "--framework", "main.oxel"]);
        assert_eq!(output.status.code(), Some(exit), "{output:?}");
    }
    fs::write(
        &main,
        "fn main() -> System.Threading.Tasks.Task<bool> { System.Threading.Tasks.Task.FromResult(true) }",
    )
    .unwrap();
    let invalid = oxel(project, &["check", "--framework", "main.oxel"]);
    assert!(!invalid.status.success());
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("E0206"));

    fs::write(
        &main,
        r#"fn main() -> System.Threading.Tasks.Task {
            System.Threading.Tasks.Task.FromException(new System.InvalidOperationException("task failed"))
        }"#,
    )
    .unwrap();
    let failed = oxel(project, &["run", "--framework", "main.oxel"]);
    assert!(!failed.status.success());
    let error = String::from_utf8_lossy(&failed.stderr);
    assert!(
        error.contains("System.InvalidOperationException: task failed"),
        "{error}"
    );
    assert!(!error.contains("AggregateException"), "{error}");
}

#[test]
fn builtin_aliases_participate_in_clr_interface_conversions() {
    let folder = tempfile::tempdir().unwrap();
    let project = folder.path();
    let source = project.join("main.oxel");
    fs::write(
        &source,
        include_str!("../../../examples/alias_interfaces.oxel"),
    )
    .unwrap();
    assert_eq!(
        success(oxel(project, &["run", "--framework", "main.oxel"])),
        "030\n30\n1\n0\n",
    );

    for (code, diagnostic) in [
        (
            "fn use<T: System.IDisposable>(value: T) {} fn main() { use(30); }",
            "E0212",
        ),
        (
            "fn main() { let value: System.IComparable<object> = 30; }",
            "E0301",
        ),
    ] {
        fs::write(&source, code).unwrap();
        let failure = oxel(project, &["check", "--framework", "main.oxel"]);
        assert!(!failure.status.success());
        let error = String::from_utf8(failure.stderr).unwrap();
        assert!(error.contains(diagnostic), "{error}");
    }
}

#[test]
fn lambdas_preserve_generic_types_constraints_and_captures() {
    let folder = tempfile::tempdir().unwrap();
    let project = folder.path();
    fs::write(
        project.join("main.oxel"),
        include_str!("../../../examples/generic_lambdas.oxel"),
    )
    .unwrap();
    assert_eq!(
        success(oxel(project, &["run", "--framework", "main.oxel"])),
        "10\n30\noxel\n2024\n10\n20\n",
    );
}

#[test]
fn invokes_clr_delegates_through_bindings_and_expressions() {
    let folder = tempfile::tempdir().unwrap();
    let project = folder.path();
    let source = project.join("main.oxel");
    fs::write(
        &source,
        include_str!("../../../examples/delegate_calls.oxel"),
    )
    .unwrap();
    assert_eq!(
        success(oxel(project, &["run", "--framework", "main.oxel"])),
        "30\n30\n30\n",
    );
    fs::write(
        &source,
        r#"
        struct Controls { run: System.Func<i32, i32> }
        fn run(input: i32) -> i32 { 0 }
        fn load() -> System.Func<i32, i32> {
            System.Console.WriteLine("load");
            |input| { input + 10 }
        }
        fn input() -> i32 { System.Console.WriteLine("input"); 20 }
        fn guarded(callback: System.Func<i32, i32>?) -> i32 {
            if callback == null { return 0; }
            callback(20)
        }
        fn main() {
            let run = load();
            let nested: System.Action = || { System.Console.WriteLine((run)(20)); };
            nested();
            System.Console.WriteLine(run(20));
            let controls = Controls { run };
            System.Console.WriteLine((controls.run)(20));
            System.Console.WriteLine(guarded(run));
            System.Console.WriteLine(guarded(null));
            System.Console.WriteLine(load()(input()));
        }
        "#,
    )
    .unwrap();
    assert_eq!(
        success(oxel(project, &["run", "--framework", "main.oxel"])),
        "load\n30\n0\n30\n30\n0\nload\ninput\n30\n",
    );
    for (body, code) in [
        ("let value = 10; value();", "E0205"),
        ("let callback: System.Action? = null; callback();", "E0311"),
        ("let callback: System.Action; callback();", "E0321"),
        (
            "let callback: System.Action = || {}; callback(true);",
            "E0205",
        ),
        (
            "let callback: System.Action = || {}; callback::<i32>();",
            "E0205",
        ),
    ] {
        fs::write(&source, format!("fn main() {{ {body} }}")).unwrap();
        let output = oxel(project, &["check", "--framework", "main.oxel"]);
        assert!(!output.status.success(), "{body}");
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(error.contains(code), "{body}: {error}");
    }
}

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
        include_str!("../../../examples/collections.oxel"),
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
        include_str!("../../../examples/bitwise.oxel"),
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
        include_str!("../../../examples/events.oxel"),
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
        include_str!("../../../examples/generic_callbacks.oxel"),
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
        include_str!("../../../examples/nested_lambdas.oxel"),
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
        include_str!("../../../examples/mutable_captures.oxel"),
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
        include_str!("../../../examples/http_server.oxel"),
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
        include_str!("../../../examples/exceptions.oxel"),
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
        include_str!("../../../examples/date_ranges.oxel"),
    )
    .unwrap();
    let result = success(oxel(project, &["run", "main.oxel", "--framework"]));
    assert_eq!(result.lines().collect::<Vec<_>>(), ["22", "True"]);
}

#[test]
fn clr_framework_generic_structs_keep_constructed_value_signatures() {
    let folder = tempfile::tempdir().unwrap();
    let project = folder.path();
    let source = include_str!("../../../examples/key_value_pairs.oxel");
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
        include_str!("../../../examples/clr_option.oxel"),
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
        include_str!("../../../examples/annotated_clr_generics.oxel"),
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
fn framework_only_builds_use_installed_pack_without_msbuild() {
    use std::{env, os::unix::fs::PermissionsExt};

    let folder = tempfile::tempdir().unwrap();
    let project = folder.path().join("app");
    let sdk = folder.path().join("sdk/10.0.100");
    let pack = folder.path().join("packs/Microsoft.NETCore.App.Ref/10.0.2");
    let tools = folder.path().join("tools");
    for directory in [
        &project,
        &sdk,
        &pack.join("ref/net10.0"),
        &pack.join("data"),
        &tools,
    ] {
        fs::create_dir_all(directory).unwrap();
    }
    fs::write(
        sdk.join("Microsoft.NETCoreSdk.BundledVersions.props"),
        r#"<Project><ItemGroup><KnownFrameworkReference Include="Microsoft.NETCore.App"
            TargetFramework="net10.0" TargetingPackName="Microsoft.NETCore.App.Ref"
            TargetingPackVersion="10.0.2" DefaultRuntimeFrameworkVersion="10.0.1" />
            </ItemGroup></Project>"#,
    )
    .unwrap();
    fs::write(
        pack.join("data/FrameworkList.xml"),
        r#"<FileList FrameworkName="Microsoft.NETCore.App" TargetFrameworkVersion="10.0">
            <File Type="Managed" Path="ref/net10.0/System.Runtime.dll" />
            </FileList>"#,
    )
    .unwrap();
    fs::write(
        pack.join("ref/net10.0/System.Runtime.dll"),
        oxel::compile("fn main() {}", "mock_framework").unwrap(),
    )
    .unwrap();
    fs::write(project.join("main.oxel"), "fn main() {}").unwrap();
    fs::write(
        project.join("oxel.toml"),
        "[package]\nname = 'app'\nsources = ['main.oxel']\nframework = true\nnative_libraries = ['helper.so']",
    ).unwrap();
    let native = project.join("helper.so");
    fs::write(&native, "first").unwrap();
    let shim = tools.join("dotnet");
    fs::write(
        &shim,
        r#"#!/bin/sh
printf '%s\n' "$1" >> "$OXEL_TEST_SDK_LOG"
case "$1" in
    --list-sdks) printf '10.0.100 [%s/sdk]\n' "$OXEL_TEST_SDK_ROOT" ;;
    exec) cat "${2%/*}/helper.so" ;;
    *) exit 1 ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(shim, fs::Permissions::from_mode(0o755)).unwrap();
    let path = env::var_os("PATH").unwrap();
    let path = env::join_paths(std::iter::once(tools).chain(env::split_paths(&path))).unwrap();
    let log = folder.path().join("calls.txt");
    let invoke = |action| {
        success(
            Command::new(env!("CARGO_BIN_EXE_oxel"))
                .current_dir(&project)
                .env("PATH", &path)
                .env("OXEL_TEST_SDK_ROOT", folder.path())
                .env("OXEL_TEST_SDK_LOG", &log)
                .arg(action)
                .output()
                .unwrap(),
        )
    };
    let dll = project.join(invoke("compile").trim());
    let config: serde_json::Value =
        serde_json::from_slice(&fs::read(dll.with_extension("runtimeconfig.json")).unwrap())
            .unwrap();
    assert_eq!(config["runtimeOptions"]["tfm"], "net10.0");
    assert_eq!(config["runtimeOptions"]["framework"]["version"], "10.0.1");
    let deps: serde_json::Value =
        serde_json::from_slice(&fs::read(dll.with_extension("deps.json")).unwrap()).unwrap();
    assert_eq!(
        deps["targets"][".NETCoreApp,Version=v10.0"]["app/1.0.0"]["runtime"]["app.dll"],
        serde_json::json!({})
    );
    assert_eq!(
        fs::read_to_string(dll.with_file_name("helper.so")).unwrap(),
        "first"
    );
    assert_eq!(invoke("run"), "first");
    assert_eq!(invoke("run"), "first");
    let cache = project.join("target/oxel/native-cache");
    assert_eq!(fs::read_dir(&cache).unwrap().count(), 1);
    fs::write(&native, "later").unwrap();
    assert_eq!(invoke("run"), "later");
    assert_eq!(fs::read_dir(&cache).unwrap().count(), 2);
    assert_eq!(
        fs::read_to_string(dll.with_file_name("helper.so")).unwrap(),
        "first"
    );
    assert_eq!(
        fs::read_to_string(log).unwrap().lines().collect::<Vec<_>>(),
        [
            "--list-sdks",
            "--list-sdks",
            "exec",
            "--list-sdks",
            "exec",
            "--list-sdks",
            "exec"
        ]
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
            "fn accept(value: System.Text.Json.Utf8JsonReader) {} fn main() {}",
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
        include_str!("../../../examples/nullable_values.oxel"),
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
            let present = unwrap_or_else({ evals += 1; some }, { evals += 1; 9 });
            System.Console.WriteLine(present);
            System.Console.WriteLine(evals);
            System.Console.WriteLine(unwrap_or(none, 20));
            System.Console.WriteLine(none.HasValue);
            System.Console.WriteLine(some as i32);
            let list = new System.Collections.Generic.List<i32?>();
            list.Add(null);
            list.Add(10);
            System.Console.WriteLine(unwrap_or(list[0], 25));
            System.Console.WriteLine(list[1].Value);
            let boxed = box_nullable(none);
            System.Console.WriteLine((boxed as i32?) == null);
            let values = new i32?[2];
            values[1] = 40;
            System.Console.WriteLine(values[0] == null);
            System.Console.WriteLine(unwrap_or(values[1], 0));
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
        include_str!("../../../examples/catch.oxel"),
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
                unwrap_or(fallback, 11)
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
        include_str!("../../../examples/using.oxel"),
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
                (unwrap_or(resource.ReadLine(), "")).Length
            };
            System.Console.WriteLine(length);
            System.Console.WriteLine(acquired);
            System.Console.WriteLine(disposed(normal));

            let returning = new System.IO.StringReader("A");
            System.Console.WriteLine(early(returning));
            System.Console.WriteLine(disposed(returning));

            let empty = new System.IO.StringReader("");
            System.Console.WriteLine(unwrap_or(propagate(empty), "empty"));
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
            let sum = add(12, unwrap_or((try { using resource = attempt_reader { missing? } }), 18));
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

#[test]
fn async_using_keeps_resources_alive_and_disposes_on_each_exit() {
    let folder = tempfile::tempdir().unwrap();
    let project = folder.path();
    let source = r#"
        use System::IO::StringReader;
        use System::Threading::Tasks::{Task, TaskCompletionSource};

        struct FailingResource {}
        impl System.IDisposable for FailingResource {
            fn Dispose(self: FailingResource) {
                throw new System.FormatException()
            }
        }

        fn closed(reader: StringReader) -> bool {
            try { reader.Peek(); false }
            catch (_: System.ObjectDisposedException) { true }
        }

        async fn read(gate: Task, outer: StringReader, inner: StringReader) -> i32 {
            using a = outer {
                using b = inner {
                    return a.Read() + { gate.await; b.Read() };
                }
            }
        }

        async fn dispose_fails(gate: Task) -> i32 {
            try {
                using resource = (FailingResource {}) {
                    gate.await;
                    return 1;
                }
            } catch (_: System.FormatException) {
                30
            }
        }

        async fn propagate(gate: Task, input: StringReader) -> Option<i32> {
            let missing: Option<i32> = Option.None;
            using reader = input {
                gate.await;
                Option.Some(missing?)
            }
        }

        async fn break_from_using(gate: Task, input: StringReader) {
            while true {
                using reader = input {
                    gate.await;
                    break;
                }
            }
        }

        async fn enumerate(gate: Task) -> i32 {
            let values = new System.Collections.Generic.List<i32>();
            values.Add(12);
            values.Add(18);
            let mut sum = 0;
            for value in values {
                gate.await;
                sum += value;
            }
            sum
        }

        async fn main() {
            let gate = new TaskCompletionSource<i32>();
            let outer = new StringReader("A");
            let inner = new StringReader("B");
            let missing = new StringReader("");
            let breaking = new StringReader("");
            let reading = read(gate.Task, outer, inner);
            let failing = dispose_fails(gate.Task);
            let propagating = propagate(gate.Task, missing);
            let loop_exit = break_from_using(gate.Task, breaking);
            let enumerating = enumerate(gate.Task);
            assert(!reading.IsCompleted);
            assert(!closed(outer) && !closed(inner) && !closed(missing) && !closed(breaking));
            gate.SetResult(0);
            assert(reading.await == 131);
            assert(failing.await == 30);
            assert((unwrap_or(propagating.await, 42)) == 42);
            loop_exit.await;
            assert(enumerating.await == 30);
            assert(closed(outer) && closed(inner) && closed(missing) && closed(breaking));

            let canceled_gate = new TaskCompletionSource<i32>();
            let cancel_outer = new StringReader("A");
            let cancel_inner = new StringReader("B");
            let cancel_read = read(canceled_gate.Task, cancel_outer, cancel_inner);
            canceled_gate.SetCanceled();
            try {
                cancel_read.await;
                assert(false);
            } catch (_: System.OperationCanceledException) {
                assert(cancel_read.IsCanceled);
                assert(closed(cancel_outer) && closed(cancel_inner));
            }
            System.Console.WriteLine("disposed");
        }
    "#;
    fs::write(project.join("main.oxel"), source).unwrap();
    assert_eq!(
        success(oxel(project, &["run", "--framework", "main.oxel"])),
        "disposed\n"
    );
}

#[test]
fn arrays_upcast_to_read_only_clr_collections_without_element_boxing() {
    let folder = tempfile::tempdir().unwrap();
    let project = folder.path();
    let main = project.join("main.oxel");
    fs::write(
        &main,
        r#"
        use System::Collections::Generic::{IEnumerable, IReadOnlyCollection, IReadOnlyList};

        fn combined(nums: IReadOnlyList<i32>, texts: IReadOnlyCollection<object>) -> i32 {
            nums[0] + texts.Count
        }

        fn sum(nums: IEnumerable<i32>) -> i32 {
            let mut total = 0;
            for num in nums {
                total += num;
            }
            total
        }

        fn main() {
            System.Console.WriteLine(combined([28], ["a", "b"]));
            System.Console.WriteLine(sum([10, 20]));
            let texts: IReadOnlyCollection<string?> = ["present"];
            System.Console.WriteLine(texts.Count);
        }
        "#,
    )
    .unwrap();
    assert_eq!(
        success(oxel(project, &["run", "--framework", "main.oxel"])),
        "30\n30\n1\n"
    );

    for (actual, expected, values) in [
        ("i32[]", "object", "[1]"),
        ("string?[]", "string", "[null]"),
        ("string[]?", "string", "null"),
    ] {
        let source = format!(
            "fn main() {{ let source: {actual} = {values}; let result: System.Collections.Generic.IReadOnlyCollection<{expected}> = source; }}"
        );
        fs::write(&main, &source).unwrap();
        let error = oxel(project, &["check", "--framework", "main.oxel"]);
        assert!(!error.status.success(), "{source}");
        let diagnostic = String::from_utf8(error.stderr).unwrap();
        assert!(diagnostic.contains("E0301"), "{source}: {diagnostic}");
    }
}
