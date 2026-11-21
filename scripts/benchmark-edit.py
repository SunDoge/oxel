#!/usr/bin/env python3
"""Measure source save to changed program output on Linux or macOS."""

import argparse
import os
from pathlib import Path
import queue
import signal
import statistics
import subprocess
import tempfile
import threading
import time

ROOT = Path(__file__).resolve().parents[1]
FILE_COUNT = 32
FUNCTIONS_PER_FILE = 32
STEPS = 16


def workload(directory, language):
    names = []
    for file in range(FILE_COUNT):
        if language == "go":
            lines, extension = ["package main"], "go"
        elif language == "csharp":
            lines, extension = ["partial class Program {"], "cs"
        else:
            lines, extension = [], "oxel"
        for index in range(file * FUNCTIONS_PER_FILE, (file + 1) * FUNCTIONS_PER_FILE):
            if language == "go":
                lines.append(f"func work{index}(n int32) int32 {{")
            elif language == "csharp":
                lines.append(f"static int Work{index}(int n) {{")
            else:
                lines.append(f"fn work{index}(input: i32) -> i32 {{ let mut n = input;")
            for step in range(STEPS):
                if language == "go":
                    lines.append(f"if n > {step} {{ n += 2 }} else {{ n -= 1 }}")
                elif language == "csharp":
                    lines.append(f"if (n > {step}) {{ n += 2; }} else {{ n -= 1; }}")
                else:
                    lines.append(f"if n > {step} {{ n += 2; }} else {{ n -= 1; }};")
            lines.append({"go": "return n\n}", "csharp": "return n;\n}"}.get(language, "n\n}"))
        if language == "csharp":
            lines.append("}")
        name = f"work{file}.{extension}"
        (directory / name).write_text("\n".join(lines) + "\n")
        names.append(name)
    return names


def replace(path, text):
    stage = path.with_suffix(".tmp")
    stage.write_text(text)
    stage.replace(path)


def wait_for_value(output, expected):
    deadline = time.monotonic() + 25
    lines = []
    while time.monotonic() < deadline:
        try:
            line = output.get(timeout=max(0, deadline - time.monotonic()))
        except queue.Empty:
            break
        if line is None:
            break
        prefix = f"VALUE {expected} PID "
        if line.startswith(prefix):
            return int(line[len(prefix):])
        lines.append(line.strip())
    raise RuntimeError(f"expected VALUE {expected}: " + "\n".join(lines[-12:]))


def watched(command, directory, source, source_code, iterations):
    with subprocess.Popen(
        command, cwd=directory, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
        text=True, start_new_session=True,
    ) as child:
        output = queue.Queue()

        def read():
            for line in child.stdout:
                output.put(line)
            output.put(None)

        threading.Thread(target=read, daemon=True).start()
        try:
            process = wait_for_value(output, 1000)
            # Let the SDK finish attaching its file watcher after initial app output.
            time.sleep(0.5)
            timings = []
            for iteration in range(1, iterations + 2):
                expected = 1000 + iteration
                start = time.perf_counter()
                source.write_text(source_code(expected))
                assert wait_for_value(output, expected) == process, "the application restarted"
                elapsed = time.perf_counter() - start
                if iteration > 1:
                    timings.append(elapsed)
            return timings
        finally:
            if child.poll() is None:
                os.killpg(child.pid, signal.SIGINT)
                try:
                    child.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    os.killpg(child.pid, signal.SIGKILL)


def measure_oxel(directory, cli, iterations):
    directory.mkdir()
    sources = workload(directory, "oxel")
    files = ", ".join(f'"{name}"' for name in ["hot.oxel", *sources])
    (directory / "reload.toml").write_text(
        f'[package]\nname = "Edit.View"\nkind = "library"\nsources = [{files}]\n'
    )
    (directory / "oxel.toml").write_text(
        '[package]\nname = "edit_host"\nframework = true\nsources = ["main.oxel", "loader.oxel"]\n'
    )
    (directory / "loader.oxel").write_bytes((ROOT / "examples/hot_reload/loader.oxel").read_bytes())
    (directory / "main.oxel").write_text('''
use System::{Console, Environment, Func, InvalidOperationException};
use System::Threading::Thread;

fn main() {
    let directory = Environment.GetEnvironmentVariable("OXEL_RELOAD_DIRECTORY") ?? throw new InvalidOperationException("No reload directory");
    let mut seen = "";
    while true {
        let generation = try_reload_generation(directory) ?? seen;
        if generation != seen {
            let view = load_view::<Func<i32>>(directory, generation, "Edit.View.Program", "read");
            Console.WriteLine($"VALUE {(view.invoke)()} PID {Environment.ProcessId}");
            view.context.Unload();
            seen = generation;
        }
        Thread.Sleep(1);
    }
}
''')
    source = directory / "hot.oxel"

    def code(value):
        return f"fn read() -> i32 {{ work0({value - 32}) }}\n"

    source.write_text(code(1000))
    return watched([cli, "dev", "--reload-manifest", "reload.toml"], directory, source, code, iterations)


def measure_csharp(directory, iterations):
    directory.mkdir()
    workload(directory, "csharp")
    (directory / "Edit.csproj").write_text('''
<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup><OutputType>Exe</OutputType><TargetFramework>net10.0</TargetFramework></PropertyGroup>
</Project>
''')
    (directory / "Program.cs").write_text('''
partial class Program {
    static void Main() {
        int seen = -1;
        while (true) {
            int value = Read();
            if (value != seen) System.Console.WriteLine($"VALUE {value} PID {System.Environment.ProcessId}");
            seen = value;
            System.Threading.Thread.Sleep(1);
        }
    }
}
''')
    source = directory / "Hot.cs"

    def code(value):
        return "partial class Program { static int Read() { return Work0(" + str(value - 32) + "); } }\n"

    source.write_text(code(1000))
    subprocess.run(["dotnet", "build", "--nologo"], cwd=directory, stdout=subprocess.DEVNULL, check=True)
    return watched(["dotnet", "watch", "--non-interactive", "--no-restore", "run"], directory, source, code, iterations)


def measure_go(directory, iterations):
    directory.mkdir()
    workload(directory, "go")
    (directory / "go.mod").write_text("module benchmark/edit\n\ngo 1.22\n")
    return fresh(
        ["go", "run", "."], directory, directory / "main.go",
        lambda value: f'package main\nfunc main() {{ println("VALUE", work0({value - 32})) }}\n',
        iterations,
    )


def measure_oxel_run(directory, cli, iterations):
    directory.mkdir()
    names = workload(directory, "oxel")
    return fresh(
        [cli, "run", "main.oxel", *names], directory, directory / "main.oxel",
        lambda value: f'fn main() {{ System.Console.WriteLine($"VALUE {{work0({value - 32})}}"); }}\n',
        iterations,
    )


def fresh(command, directory, source, source_code, iterations):
    timings = []
    for iteration in range(iterations + 2):
        expected = 1000 + iteration
        start = time.perf_counter()
        replace(source, source_code(expected))
        output = subprocess.check_output(
            command, cwd=directory, stderr=subprocess.STDOUT, text=True, timeout=25,
        )
        assert output.strip() == f"VALUE {expected}", output
        if iteration > 1:
            timings.append(time.perf_counter() - start)
    return timings


def report(label, timings):
    p90 = sorted(timings)[(len(timings) * 9 - 1) // 10]
    print(f"{label:24s} median {statistics.median(timings) * 1000:7.2f} ms    p90 {p90 * 1000:7.2f} ms", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--oxel", type=Path, default=ROOT / "target/release/oxel")
    parser.add_argument("--previous-oxel", type=Path)
    parser.add_argument("--iterations", type=int, default=11)
    args = parser.parse_args()
    if args.iterations < 1:
        parser.error("--iterations must be at least 1")
    with tempfile.TemporaryDirectory(prefix="oxel-bench-edit-") as temp:
        directory = Path(temp)
        if args.previous_oxel:
            report("Oxel dev previous", measure_oxel(directory / "previous", args.previous_oxel.resolve(), args.iterations))
        cli = args.oxel.resolve()
        report("Oxel dev", measure_oxel(directory / "oxel", cli, args.iterations))
        report("Oxel run", measure_oxel_run(directory / "oxel-run", cli, args.iterations))
        report("Go run", measure_go(directory / "go", args.iterations))
        report("C# dotnet watch", measure_csharp(directory / "csharp", args.iterations))


if __name__ == "__main__":
    main()
