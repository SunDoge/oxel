# Command-line guide

## Quick start

Use Rust 1.91 or later. Running programs and the CLI benchmark requires `dotnet` on `PATH` with a .NET 10 or newer runtime. Framework and NuGet projects, Native AOT publishing, and the integration suite require a .NET 10 or newer SDK.

```sh
cargo build --locked
cargo run -- check examples/hello.oxel
cargo run -- compile examples/hello.oxel
dotnet target/oxel/hello.dll
cargo run -- run examples/hello.oxel
```

Both Hello World commands print `30`. [The integer example](../examples/integers.oxel) shows small integer widths and wrapping. [The floating-point example](../examples/floats.oxel) uses both floating-point widths and a constant. [The control-flow example](../examples/control_flow.oxel) demonstrates shadowing, reassignment, branches and an `i64` call. [The string example](../examples/strings.oxel) demonstrates Unicode literals, concatenation and value equality. [The struct example](../examples/structs.oxel) demonstrates nested CLR value types. [The loop example](../examples/loops.oxel) uses `while`, `break` and `continue`. To use `oxel` directly, run `cargo install --path crates/oxel-cli --locked`.

- `oxel check` checks syntax, names, types and the entry point.
- `oxel compile` writes a DLL and its `.runtimeconfig.json` to `target/oxel/` under the current directory, using the input's file stem. It prints the DLL path. Inputs with the same stem compiled from the same working directory share the output path.
- `oxel run` compiles into a temporary directory, runs the DLL with `dotnet exec` and forwards the program's output and exit status. The original working directory is preserved.

Use `fn main(args: string[]) -> i32` to receive command-line arguments and return an exit code. The parameter and return type are independently optional. CLR supplies an empty array when the program receives no arguments. With `oxel run`, put compiler options and source paths before `--` and program arguments after it. [Argument example](../examples/arguments.oxel) · [.NET entry points](https://learn.microsoft.com/en-us/dotnet/csharp/programming-guide/main-and-command-args/)

An entry point with no return type returns `void` and exits with status 0 on normal completion. An entry point returning the module's `Result<T, E>` can use `?`. `Ok(i32)` supplies the exit code and other successful payloads give status 0. `Err(error)` writes the error to standard error and gives status 1. [Result entry point example](../examples/result_main.oxel)

With `--framework`, an asynchronous entry point is a `fn main` that returns a CLR `System.Threading.Tasks.Task` or `ValueTask`. Both support the non-generic form and generic results of `i32` or `Result<T, E>`. Use .NET task APIs in its body. The generated CLR entry point waits for the task and uses the same exit conventions for the completed value. A faulted task propagates its original exception. Run `oxel run --framework examples/async_main.oxel -- ready` to try the asynchronous example. [Asynchronous entry point example](../examples/async_main.oxel)

With `--framework`, write `async fn fetch() -> string` for a function that returns a CLR `Task<string>`. An omitted result type produces a `Task`. Inside an async function, `.await` accepts a CLR `Task`, `Task<T>`, `ValueTask` or `ValueTask<T>` and yields its result. For example, `fetch().await.Trim()` waits for the string and calls `Trim` on it. An incomplete task suspends the caller and schedules a continuation through the CLR awaiter. Exceptions fault the returned task and cancellation sets its canceled state. The same exit rules apply to `async fn main`. [Runnable `.await` example](../examples/await.oxel)

Generic async functions support type inference and constraints. For example, `async fn after<T>(gate: Task, value: T) -> T` returns a CLR `Task<T>`. A struct field initializer can await a task. [Runnable generic async example](../examples/generic_async.oxel)

Use `.await` inside `try` and `catch` blocks. An exception raised when an await resumes can reach a surrounding catch, and the catch can await another task before continuing. Scoped `using` and loops over disposable CLR enumerators preserve their resources across suspension and dispose them when control exits the scope. Span parameters and locals in async bodies require additional lifetime analysis. A managed address cannot remain live across an await.

Write `async |value| { ... }` or `async { ... }` to create an async lambda with a known CLR delegate type. Both forms also work as trailing lambdas. A delegate returning `Task<T>` or `ValueTask<T>` expects a body that yields `T`. A delegate returning `Task`, `ValueTask`, or `void` expects a void body. For void delegates such as event handlers, .NET's async-void builder handles uncaught exceptions. [Runnable async catch and lambda example](../examples/async_catch.oxel)

```sh
oxel run examples/arguments.oxel -- "two words" --flag
```

With plain source files or local DLL references, the runtime configuration targets .NET 10 and allows roll-forward to the next installed major version. Framework references and NuGet dependencies use the selected SDK's target framework. Parsing, semantic analysis and assembly generation run in Rust. [Microsoft .NET version selection](https://learn.microsoft.com/en-us/dotnet/core/versions/selection)

## Formatting

Run `oxel fmt` in a directory with `oxel.toml` to format the project's sources and tests. You can also pass a manifest path or one or more source files.

```sh
oxel fmt
oxel fmt src/main.oxel src/routes.oxel
oxel fmt --check --manifest-path examples/project/oxel.toml
```

`--check` is read-only and exits with a nonzero status if any file needs formatting. Formatting uses four-space indentation and preserves comments, literal contents, and intentional line breaks in lists. It uses the compiler's syntax parser and works without .NET or a NuGet restore. In-place writes start only after every input parses successfully. Zed uses the same formatter via `oxel lsp`. Set `"format_on_save": "on"` under the `Oxel` language settings to format on save.

## Unit tests

Mark a parameterless, non-generic function that returns `void` with `#[test]`. Use `assert(condition)` or `assert(condition, "message")`. A false condition raises a CLR exception with the failure message. The optional message expression runs only on failure.

```rust
#[test]
fn adds_positive_numbers() {
    assert(10 + 20 == 30);
}
```

Run `oxel test examples/unit_tests.oxel`, or run `oxel test` in a directory with `oxel.toml`. Tests share the module with production functions and can call internal helpers. The runner executes tests in source order on CoreCLR, reports exceptions per test, and exits with status 1 if any fail. A test run can use a library or a module without `main`. Normal builds omit test functions. [Runnable unit tests](../examples/unit_tests.oxel)

With `--framework`, use `#[test] async fn name() { ... }` for a test that awaits CLR tasks. The runner waits for the test to finish before reporting the result or starting the next test. A faulted or canceled task is reported as a test failure.

Use `oxel test --filter adds examples/unit_tests.oxel` to run tests whose names contain `adds`. Matching is case-sensitive and the compiler checks the whole test module before running the selected tests.

Add `#[ignore]` alongside `#[test]` to skip a test by default, for example when it requires a service running locally. Skipped tests still undergo type checking. Use `oxel test --include-ignored` to run them as well. Combine both flags to limit execution by name.

Use `oxel test --list` to check the test module and print one line per test in source order. The output is `name: test` or `name: test (ignored)`. The listing includes ignored tests and accepts `--filter`. Pure Oxel modules can list tests without a .NET SDK or runtime. Rust tools can use `oxel::discover_test_project` to get the test names, source spans and ignore flags.

Use `oxel check --tests examples/unit_tests.oxel` to check a test module without generating or running an assembly. It accepts modules without `main` and includes `test_sources` for manifest projects. Pure Oxel modules can use this command without a .NET SDK or runtime.

For editor diagnostics and navigation between tests and library code, start the language server with `oxel lsp --tests --manifest-path path/to/oxel.toml`.

## Diagnostics for tools

Use `oxel check --diagnostic-format json main.oxel` to consume compiler diagnostics from a script or agent. The compiler writes one JSON object per line to standard error and returns a nonzero status when it reports errors. The same option is available for `compile`, `run` and `publish`. Program output and artifact paths still go to standard output. SDK and application messages may appear separately on standard error when running or publishing.

Each diagnostic includes `file`, `code`, `severity`, `message`, and a `range` with zero-based UTF-8 byte offsets and an exclusive end. `line` and `column` are one-based, with columns counted in Unicode scalar values. Type mismatches also provide `expected` and `actual` with the resolved type names. Rust consumers can use `Diagnostic::type_names()` for constructed types, whose internal IDs are scoped to one analysis.

For a wrong argument type or count in an Oxel function call, the compiler can also point to the function declaration. JSON includes this note as a `related` object with its own `file`, `range`, `line`, `column`, and `message`. The language server sends the same location to clients that support related diagnostic information.

When the compiler can suggest a source edit, the JSON diagnostic also has a `fix` with a title, a byte range in the same file, and replacement text. Apply an insertion at an empty range and a deletion when the replacement is empty.

## Native AOT publishing

```sh
oxel publish examples/hello.oxel
./target/oxel/native/hello
```

`publish` builds for the SDK's host runtime and prints the executable path under `target/oxel/native/`. On Windows the executable ends in `.exe`. Publishing requires a .NET 10 or newer SDK and the SDK's platform-specific Native AOT dependencies, such as a linker and development libraries on Linux. The executable can run without an installed .NET runtime on a compatible system. [Microsoft Native AOT prerequisites](https://learn.microsoft.com/en-us/dotnet/core/deploying/native-aot/)

When the SDK publishes companion runtime files, oxel places the executable and its companions in a content-addressed directory under `target/oxel/native/`. Keep that directory together when deploying the app.

The SDK consumes oxel's managed assembly directly. Use repeatable `--reference` flags for managed implementation DLLs. Referenced code must support Native AOT and trimming. SDK failures go to standard error so scripts can read the executable path from standard output.

Repeated publishing can reuse SDK intermediates under `target/oxel/aot-cache/`. For standalone projects, a source edit can reuse the verified SDK restore if the SDK package markers and restore files still match. Unchanged managed assemblies and publish settings can also reuse the verified native output directly for projects without NuGet package dependencies. NuGet-backed projects still run the SDK's restore and publish targets. Delete `target/oxel/aot-cache/` to force a fresh native build.
