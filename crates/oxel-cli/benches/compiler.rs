mod managed;

use std::{
    fmt::Write,
    hint::black_box,
    path::Path,
    process::Command,
    sync::Arc,
    time::{Duration, Instant},
};

fn main() {
    if std::env::var_os("OXEL_BENCH_INTERPOLATION").is_some() {
        let directory = tempfile::tempdir().unwrap();
        managed::compare_interpolation(env!("CARGO_BIN_EXE_oxel"), directory.path());
        return;
    }
    let hello = include_str!("../../../examples/hello.oxel");
    let mut generated = String::new();
    for index in 0..500 {
        writeln!(
            generated,
            "fn f{index}(x: i32) -> i32 {{ if x > 0 {{ x + {index} }} else {{ -x }} }}"
        )
        .unwrap();
    }
    generated.push_str("fn main() { System.Console.WriteLine(f499(3)); }");
    for (label, source) in [("hello", hello), ("500 functions", generated.as_str())] {
        measure(&format!("{label} / check"), || {
            oxel::check(black_box(source)).unwrap()
        });
        measure(&format!("{label} / compile"), || {
            black_box(oxel::compile(black_box(source), "benchmark").unwrap());
        });
    }
    for (label, generic) in [("1,024 numeric", false), ("1,024 generic", true)] {
        let source = body_workload(generic);
        measure(&format!("{label} / check"), || {
            oxel::check(black_box(&source)).unwrap()
        });
        measure(&format!("{label} / compile"), || {
            black_box(oxel::compile(black_box(&source), "benchmark").unwrap());
        });
    }
    measure_project_edits();
    measure_comptime_edits();
    let Some(binary) = option_env!("CARGO_BIN_EXE_oxel") else {
        println!("CLI timings unavailable in this benchmark build");
        return;
    };
    let folder = tempfile::tempdir().unwrap();
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/hello.oxel");
    let cli = |action: &str| {
        let mut command = Command::new(binary);
        command.current_dir(folder.path()).arg(action);
        if action != "--help" {
            command.arg(&source);
        }
        completed(&mut command);
    };
    for (label, action) in [
        ("CLI startup / help", "--help"),
        ("CLI hello / check", "check"),
        ("CLI hello / compile", "compile"),
        ("CLI hello / run", "run"),
    ] {
        measure(label, || cli(action));
    }
    measure("CoreCLR hello / exec", || {
        completed(
            Command::new("dotnet")
                .current_dir(folder.path())
                .args(["exec", "target/oxel/hello.dll"]),
        );
    });
    if std::env::var_os("OXEL_BENCH_FRAMEWORK").is_some() {
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/file_lines.oxel");
        measure("CLI framework / check", || {
            completed(
                Command::new(binary)
                    .current_dir(folder.path())
                    .args(["check", "--framework"])
                    .arg(&source),
            );
        });
    }
    if std::env::var_os("OXEL_BENCH_GO").is_some() {
        measure_go_compile(binary, folder.path());
    }
    if std::env::var_os("OXEL_BENCH_CSHARP").is_some() {
        managed::compare_csharp(binary, folder.path());
    }
    if std::env::var_os("OXEL_BENCH_NATIVE").is_some() {
        let source = folder.path().join("native.oxel");
        std::fs::write(&source, hello).unwrap();
        let publish = || {
            completed(
                Command::new(binary)
                    .current_dir(folder.path())
                    .args(["publish", "native.oxel"]),
            );
        };
        let start = Instant::now();
        publish();
        println!(
            "{:<25} {:>10.2?} (one publish)",
            "Native AOT hello / first publish",
            start.elapsed()
        );
        measure("Native AOT hello / unchanged", publish);
        std::fs::write(&source, hello.replace("add(10, 20)", "add(10, 35)")).unwrap();
        let start = Instant::now();
        publish();
        println!(
            "{:<25} {:>10.2?} (one publish)",
            "Native AOT hello / edited",
            start.elapsed()
        );
        let executable = folder
            .path()
            .join("target/oxel/native")
            .join(format!("native{}", std::env::consts::EXE_SUFFIX));
        assert_eq!(
            String::from_utf8(output(&mut Command::new(&executable)))
                .unwrap()
                .trim(),
            "45"
        );
        println!(
            "{:<25} {:>10} bytes",
            "Native AOT hello / size",
            executable.metadata().unwrap().len()
        );
        measure("Native AOT hello / exec", || {
            completed(Command::new(&executable).current_dir(folder.path()));
        });
    }
}

fn measure_go_compile(oxel: &str, directory: &Path) {
    let go_tool_dir = output(Command::new("go").args(["env", "GOTOOLDIR"]));
    let go_tool_dir = std::str::from_utf8(&go_tool_dir).unwrap().trim();
    let go_compile =
        Path::new(go_tool_dir).join(format!("compile{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(directory.join("numeric.oxel"), body_workload(false)).unwrap();
    std::fs::write(directory.join("numeric.go"), go_numeric_workload()).unwrap();
    measure("1,024 numeric / oxel CLI", || {
        completed(
            Command::new(oxel)
                .current_dir(directory)
                .env("OXEL_JOBS", "1")
                .args(["compile", "numeric.oxel"]),
        );
    });
    measure("1,024 numeric / Go compiler", || {
        completed(
            Command::new(&go_compile)
                .current_dir(directory)
                .env("GOMAXPROCS", "1")
                .args(["-c=1", "-o", "numeric.o", "numeric.go"]),
        );
    });
}

fn go_numeric_workload() -> String {
    let mut source = String::from("package main\n");
    for index in 0..WORKLOAD_FUNCTIONS {
        writeln!(source, "func work{index}(x int32) int32 {{ n := x").unwrap();
        for step in 0..BODY_STEPS {
            writeln!(source, "  if n > {step} {{ n += x * 2 }} else {{ n -= x }}").unwrap();
        }
        source.push_str("  return n\n}\n");
    }
    source.push_str("func main() {}\n");
    source
}

const WORKLOAD_FUNCTIONS: usize = 1024;
const BODY_STEPS: usize = 24;

fn body_workload(generic: bool) -> String {
    let mut source = String::new();
    if generic {
        source.push_str("struct Cell<T> { value: T }\n");
    }
    for index in 0..WORKLOAD_FUNCTIONS {
        writeln!(source, "fn work{index}(x: i32) -> i32 {{ let mut n = x;").unwrap();
        for step in 0..BODY_STEPS {
            if generic {
                writeln!(
                    source,
                    "  let cell{step} = Cell::<i32> {{ value: x + {step} }}; n += cell{step}.value;"
                )
                .unwrap();
            } else {
                writeln!(
                    source,
                    "  if n > {step} {{ n += x * 2; }} else {{ n -= x; }}"
                )
                .unwrap();
            }
        }
        writeln!(
            source,
            "  n
}}"
        )
        .unwrap();
    }
    source.push_str("fn main() {}");
    source
}

fn measure_project_edits() {
    let references = oxel::References::default();
    let mut sources: Vec<_> = (0..20).map(|file| {
        let mut source = String::new();
        for function in 0..50 {
            writeln!(source, "fn f{file}_{function}(x: i32) -> i32 {{ if x > 0 {{ x + {function} }} else {{ -x }} }}").unwrap();
        }
        Arc::<str>::from(source)
    }).collect();
    let alternatives = [
        Arc::<str>::from(format!(
            "{}fn main() {{ System.Console.WriteLine(f19_49(1)); }}",
            sources[0]
        )),
        Arc::<str>::from(format!(
            "{}fn main() {{ System.Console.WriteLine(f19_49(12)); }}",
            sources[0]
        )),
    ];
    let mut next = 0;
    measure("20 files / cold inspect", || {
        next ^= 1;
        sources[0] = Arc::clone(&alternatives[next]);
        let files: Vec<_> = sources.iter().map(Arc::as_ref).collect();
        black_box(oxel::inspect_project(&files, &references));
    });
    let mut analyzer = oxel::ProjectAnalyzer::default();
    black_box(analyzer.inspect(&sources, &references));
    measure("20 files / one edit", || {
        next ^= 1;
        sources[0] = Arc::clone(&alternatives[next]);
        black_box(analyzer.inspect(&sources, &references));
        assert_eq!(analyzer.parsed_files(), 1);
        assert_eq!(analyzer.checked_functions(), 1);
    });
    measure("20 files / full compile", || {
        next ^= 1;
        sources[0] = Arc::clone(&alternatives[next]);
        let files: Vec<_> = sources.iter().map(Arc::as_ref).collect();
        black_box(oxel::compile_project(&files, "benchmark", &references).unwrap());
    });
    let mut compiler = oxel::ProjectCompiler::new(oxel::AssemblyKind::Executable, &references);
    black_box(compiler.compile(&sources, "benchmark").unwrap());
    measure("20 files / incremental IL", || {
        next ^= 1;
        sources[0] = Arc::clone(&alternatives[next]);
        black_box(compiler.compile(&sources, "benchmark").unwrap());
        assert_eq!(compiler.parsed_files(), 1);
        assert_eq!(compiler.checked_functions(), 1);
    });
    black_box(analyzer.inspect(&sources, &references));
    let signatures = [
        Arc::clone(&sources[19]),
        Arc::<str>::from(sources[19].replace(
            "fn f19_49(x: i32) -> i32 { if x > 0 { x + 49 } else { -x } }",
            "fn f19_49(x: i32) -> bool { x > 0 }",
        )),
    ];
    let mut next = 0;
    measure("20 files / signature edit", || {
        next ^= 1;
        sources[19] = Arc::clone(&signatures[next]);
        black_box(analyzer.inspect(&sources, &references));
        assert_eq!(analyzer.parsed_files(), 1);
        assert_eq!(analyzer.checked_functions(), 2);
    });
    let constants = [1, 2].map(|value| {
        Arc::<str>::from(format!(
            "{}const OFFSET = {value}; fn read_offset() -> i32 {{ OFFSET }}",
            sources[1],
        ))
    });
    sources[1] = Arc::clone(&constants[0]);
    black_box(analyzer.inspect(&sources, &references));
    let mut next = 0;
    measure("20 files / constant edit", || {
        next ^= 1;
        sources[1] = Arc::clone(&constants[next]);
        black_box(analyzer.inspect(&sources, &references));
        assert_eq!(analyzer.parsed_files(), 1);
        assert_eq!(analyzer.checked_functions(), 1);
    });
    let layouts = [
        Arc::<str>::from("struct Point { x: i32, y: i32 }"),
        Arc::<str>::from("struct Point {\n    x: i32, // horizontal\n    y: i32,\n}"),
    ];
    sources[0] = Arc::from(format!("{}fn read(p: Point) -> i32 {{ p.x }}", sources[0]));
    sources.push(Arc::clone(&layouts[0]));
    black_box(analyzer.inspect(&sources, &references));
    let mut next = 0;
    measure("21 files / type reformat", || {
        next ^= 1;
        *sources.last_mut().unwrap() = Arc::clone(&layouts[next]);
        black_box(analyzer.inspect(&sources, &references));
        assert_eq!(analyzer.parsed_files(), 1);
        assert_eq!(analyzer.checked_functions(), 0);
    });
    let shapes = [
        Arc::<str>::from("struct Point { x: i32, y: i32 }"),
        Arc::<str>::from("struct Point { x: i32, y: bool }"),
    ];
    *sources.last_mut().unwrap() = Arc::clone(&shapes[0]);
    black_box(analyzer.inspect(&sources, &references));
    let mut next = 0;
    measure("21 files / type field edit", || {
        next ^= 1;
        *sources.last_mut().unwrap() = Arc::clone(&shapes[next]);
        black_box(analyzer.inspect(&sources, &references));
        assert_eq!(analyzer.parsed_files(), 1);
        assert_eq!(analyzer.checked_functions(), 1);
    });
}

fn measure_comptime_edits() {
    let references = oxel::References::default();
    let mut analyzer = oxel::ProjectAnalyzer::default();
    let constants = Arc::<str>::from(concat!(
        "comptime fn compute(n: i32) -> i32 { let mut sum = 0; ",
        "for i in 0..8000 { sum += i + n; } sum } ",
        "const EXPENSIVE = compute(3); ",
    ));
    let runtime = [
        Arc::<str>::from("fn main() { System.Console.WriteLine(EXPENSIVE + 1); }"),
        Arc::<str>::from("fn main() { System.Console.WriteLine(EXPENSIVE + 2); }"),
    ];
    let mut sources = [constants, Arc::clone(&runtime[0])];
    let initial = analyzer.inspect(&sources, &references);
    assert!(initial.iter().all(|file| file.diagnostics.is_empty()));
    assert_eq!(analyzer.evaluated_constants(), 1);
    let mut next = 0;
    measure("comptime / runtime edit", || {
        next ^= 1;
        sources[1] = Arc::clone(&runtime[next]);
        black_box(analyzer.inspect(&sources, &references));
        assert_eq!(analyzer.checked_functions(), 1);
        assert_eq!(analyzer.evaluated_constants(), 0);
    });
    let types = [
        Arc::<str>::from("struct Unrelated { value: i32 }"),
        Arc::<str>::from("struct Unrelated { value: bool }"),
    ];
    let [constants, runtime] = sources;
    let mut sources = [constants, runtime, Arc::clone(&types[0])];
    black_box(analyzer.inspect(&sources, &references));
    let mut next = 0;
    measure("comptime / unrelated type", || {
        next ^= 1;
        sources[2] = Arc::clone(&types[next]);
        black_box(analyzer.inspect(&sources, &references));
        assert_eq!(analyzer.parsed_files(), 1);
        assert_eq!(analyzer.evaluated_constants(), 0);
    });
}

fn completed(command: &mut Command) {
    output(command);
}

fn output(command: &mut Command) -> Vec<u8> {
    let output = command.output().expect("launch benchmark process");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn measure(label: &str, mut operation: impl FnMut()) {
    operation();
    let start = Instant::now();
    let mut samples = 0;
    while samples < 5 || start.elapsed() < Duration::from_millis(500) {
        operation();
        samples += 1;
    }
    let mean = start.elapsed() / samples;
    println!("{label:<25} {mean:>10.2?} per operation ({samples} samples)");
}
