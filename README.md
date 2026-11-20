# oxel

oxel is a Rust compiler for a small, statically typed CLR-native language. It compiles a module of one or more `.oxel` files to a managed .NET DLL with a CoreCLR entry point.

## Quick start

Use Rust 1.91 or later. Running programs and the CLI benchmark requires `dotnet` on `PATH` with a .NET 10 or newer runtime. Framework and NuGet projects, Native AOT publishing, and the integration suite require a .NET 10 or newer SDK.

```sh
cargo build --locked
cargo run -- check examples/hello.oxel
cargo run -- compile examples/hello.oxel
dotnet target/oxel/hello.dll
cargo run -- run examples/hello.oxel
```

Both Hello World commands print `30`. [The integer example](examples/integers.oxel) shows small integer widths and wrapping. [The floating-point example](examples/floats.oxel) uses both floating-point widths and a constant. [The control-flow example](examples/control_flow.oxel) demonstrates shadowing, reassignment, branches and an `i64` call. [The string example](examples/strings.oxel) demonstrates Unicode literals, concatenation and value equality. [The struct example](examples/structs.oxel) demonstrates nested CLR value types. [The loop example](examples/loops.oxel) uses `while`, `break` and `continue`. To use `oxel` directly, run `cargo install --path crates/oxel-cli --locked`.

- `oxel check` checks syntax, names, types and the entry point.
- `oxel compile` writes a DLL and its `.runtimeconfig.json` to `target/oxel/` under the current directory, using the input's file stem. It prints the DLL path. Inputs with the same stem compiled from the same working directory share the output path.
- `oxel run` compiles into a temporary directory, runs the DLL with `dotnet exec` and forwards the program's output and exit status. The original working directory is preserved.

Use `fn main(args: string[]) -> i32` to receive command-line arguments and return an exit code. The parameter and return type are independently optional. CLR supplies an empty array when the program receives no arguments. With `oxel run`, put compiler options and source paths before `--` and program arguments after it. [Argument example](examples/arguments.oxel) · [.NET entry points](https://learn.microsoft.com/en-us/dotnet/csharp/programming-guide/main-and-command-args/)

An entry point with no return type returns `void` and exits with status 0 on normal completion. An entry point returning the module's `Result<T, E>` can use `?`. `Ok(i32)` supplies the exit code and other successful payloads give status 0. `Err(error)` writes the error to standard error and gives status 1. [Result entry point example](examples/result_main.oxel)

With `--framework`, an asynchronous entry point is a `fn main` that returns a CLR `System.Threading.Tasks.Task` or `ValueTask`. Both support the non-generic form and generic results of `i32` or `Result<T, E>`. Use .NET task APIs in its body. The generated CLR entry point waits for the task and uses the same exit conventions for the completed value. A faulted task propagates its original exception. Run `oxel run --framework examples/async_main.oxel -- ready` to try the asynchronous example. [Asynchronous entry point example](examples/async_main.oxel)

With `--framework`, write `async fn fetch() -> string` for a function that returns a CLR `Task<string>`. An omitted result type produces a `Task`. Inside an async function, `.await` accepts a CLR `Task`, `Task<T>`, `ValueTask` or `ValueTask<T>` and yields its result. For example, `fetch().await.Trim()` waits for the string and calls `Trim` on it. An incomplete task suspends the caller and schedules a continuation through the CLR awaiter. Exceptions fault the returned task and cancellation sets its canceled state. The same exit rules apply to `async fn main`. [Runnable `.await` example](examples/await.oxel)

Use `.await` inside `try` and `catch` blocks. An exception raised when an await resumes can reach a surrounding catch, and the catch can await another task before continuing. Generic named async functions, `#[test]` on async functions, `using`, and loops over disposable CLR enumerators inside async bodies require additional lowering. Span parameters and locals also require additional lifetime analysis. A managed address cannot remain live across an await.

Write `async |value| { ... }` or `async { ... }` to create an async lambda with a known CLR delegate type. Both forms also work as trailing lambdas. A delegate returning `Task<T>` or `ValueTask<T>` expects a body that yields `T`. A delegate returning `Task`, `ValueTask`, or `void` expects a void body. For void delegates such as event handlers, .NET's async-void builder handles uncaught exceptions. [Runnable async catch and lambda example](examples/async_catch.oxel)

```sh
oxel run examples/arguments.oxel -- "two words" --flag
```

With plain source files or local DLL references, the runtime configuration targets .NET 10 and allows roll-forward to the next installed major version. Framework references and NuGet dependencies use the selected SDK's target framework. Parsing, semantic analysis and assembly generation run in Rust. [Microsoft .NET version selection](https://learn.microsoft.com/en-us/dotnet/core/versions/selection)

### HTTP server example

The [HTTP server](examples/http_server.oxel) uses the .NET SDK's framework references and serves one request at a time on the IPv4 loopback address. Start it in one terminal and send requests from another.

```sh
oxel run --framework examples/http_server.oxel -- 8080
curl http://127.0.0.1:8080/health
curl 'http://127.0.0.1:8080/add?left=10&right=20'
```

The endpoints return `ok` and `30`. The server returns HTTP 400 for missing or invalid operands, 404 for an unknown path, and 405 with an `Allow` header for methods other than GET. Stop it with Ctrl+C. Use `oxel publish --framework examples/http_server.oxel` to build a native executable that takes the same port argument. On Windows, HTTP.sys may require a URL reservation for the selected prefix. [.NET HttpListener](https://learn.microsoft.com/en-us/dotnet/api/system.net.httplistener.getcontext) · [Windows URL reservations](https://learn.microsoft.com/en-us/windows/win32/http/add-urlacl)

### Avalonia desktop example

The [Avalonia counter](examples/avalonia_counter/main.oxel) uses an immediate-style view with trailing lambdas for the layout and click actions. A click updates the counter state and reruns the view. The [reconciler](examples/avalonia_counter/immediate.oxel) reuses Avalonia controls by key within each parent, patches changed properties, and inserts, moves, or removes children when the structure changes. Avalonia provides input handling and draws the controls.

```sh
oxel run --manifest-path examples/avalonia_counter/oxel.toml
oxel publish --manifest-path examples/avalonia_counter/oxel.toml
oxel test --manifest-path examples/avalonia_counter/oxel.toml
```

Run the app in a graphical desktop session. The tests exercise the keyed diff and dispatch Avalonia click events without opening a window.

The manifest uses the Avalonia Desktop and Fluent Theme packages. To run the Native AOT build, launch the executable printed by `publish` from its bundle. Keep the bundle together when moving it to a compatible machine. [Code-only Avalonia UI](https://docs.avaloniaui.net/docs/fundamentals/coded-ui)

### Plico desktop experiment

The [Plico counter](examples/plico_counter/main.oxel) uses the same immediate-style view and trailing lambdas with a Rust window host. Oxel holds the counter state and receives a batch of activated button keys. It submits a command array and a UTF-8 buffer together. The Rust host copies the accepted batch and declares its keyed nodes through Plico, which handles reconciliation, layout, input, and drawing. Plico's desktop host provides the winit event loop and wgpu renderer.

Build the pinned Rust host and generate its bindings before running the example. The manifest packages the Linux shared library. The Rust host is an independent Cargo workspace under the example.

```sh
cargo build --locked --manifest-path examples/plico_counter/native/Cargo.toml
mkdir -p examples/plico_counter/target
oxel bindgen examples/plico_counter/native/src/lib.rs --library oxel_plico_counter \
  -o examples/plico_counter/target/plico.ffi.oxel
oxel run --manifest-path examples/plico_counter/oxel.toml
oxel publish --manifest-path examples/plico_counter/oxel.toml
```

Run the window in a Linux graphical session with working wgpu drivers and the system libraries needed by Plico for text. Use Tab to focus a button and Space to activate it. The host keeps the callback alive until the window closes. It invokes Oxel for the initial view and after activations while Plico can repaint the current batch independently. The experiment packages a desktop executable. Mobile runtime and application packaging are separate integration work.

## Multi-file modules

```sh
oxel check examples/project/main.oxel examples/project/geometry.oxel
oxel run examples/project/main.oxel examples/project/geometry.oxel
```

Pass the source files as positional arguments to `check`, `compile`, `run` or `publish`. The first source filename supplies the default executable name. Each file is parsed separately, and diagnostics show positions in the originating file.

Place `namespace Geometry;` at the top of a file to put its declarations in a named scope. Files with the same namespace can reference each other's declarations by their short names. Files without a namespace remain in the root scope. An executable has one `main` across its namespaces.

Use explicit paths such as `Geometry::area()` or add file-local imports before your declarations. Imports can select individual names, group names from a common prefix, or assign aliases. They work with Oxel declarations and CLR references. Paths in `use` start at the root of the project or the referenced CLR namespace.

```rust
use Geometry::{Point, area as measure};
use System::Console::WriteLine as print;
```

Run `oxel run --manifest-path examples/namespaces/oxel.toml` for an example. [Namespace example](examples/namespaces/main.oxel)

## Unit tests

Mark a parameterless, non-generic function that returns `void` with `#[test]`. Use `assert(condition)` or `assert(condition, "message")`. A false condition raises a CLR exception with the failure message. The optional message expression runs only on failure.

```rust
#[test]
fn adds_positive_numbers() {
    assert(10 + 20 == 30);
}
```

Run `oxel test examples/unit_tests.oxel`, or run `oxel test` in a directory with `oxel.toml`. Tests share the module with production functions and can call internal helpers. The runner executes tests in source order on CoreCLR, reports exceptions per test, and exits with status 1 if any fail. A test run can use a library or a module without `main`. Normal builds omit test functions. [Runnable unit tests](examples/unit_tests.oxel)

Use `oxel test --filter adds examples/unit_tests.oxel` to run tests whose names contain `adds`. Matching is case-sensitive and the compiler checks the whole test module before running the selected tests.

Add `#[ignore]` alongside `#[test]` to skip a test by default, for example when it requires a service running locally. Skipped tests still undergo type checking. Use `oxel test --include-ignored` to run them as well. Combine both flags to limit execution by name.

Use `oxel test --list` to check the test module and print one line per test in source order. The output is `name: test` or `name: test (ignored)`. The listing includes ignored tests and accepts `--filter`. Pure Oxel modules can list tests without a .NET SDK or runtime. Rust tools can use `oxel::discover_test_project` to get the test names, source spans and ignore flags.

Use `oxel check --tests examples/unit_tests.oxel` to check a test module without generating or running an assembly. It accepts modules without `main` and includes `test_sources` for manifest projects. Pure Oxel modules can use this command without a .NET SDK or runtime.

For editor diagnostics and navigation between tests and library code, start the language server with `oxel lsp --tests --manifest-path path/to/oxel.toml`.

## Project manifest

```toml
[package]
name = "geometry-demo"
sources = ["main.oxel", "geometry.oxel"]
# test_sources = ["tests/geometry.oxel"]
# references = ["lib/GeometrySupport.dll"]
```

Use `oxel.toml` to name a package and list its source files and optional local CLR assemblies. The compiler reads source files in the listed order and resolves relative paths from the manifest directory. Additional `--reference` paths resolve from the calling directory. Each executable package has one `main` function.

`oxel test`, `oxel check --tests` and `oxel lsp --tests` append the optional `test_sources` list to `sources`, in manifest order. Both lists use paths relative to the manifest, and each file may appear only once. Files listed under `test_sources` can declare test helpers that are available to the test module.

```sh
oxel check --manifest-path examples/project/oxel.toml
oxel compile --manifest-path examples/project/oxel.toml
oxel run --manifest-path examples/project/oxel.toml
oxel publish --manifest-path examples/project/oxel.toml
oxel lsp --manifest-path examples/project/oxel.toml
```

Run `check`, `compile`, `run` or `publish` without source arguments from the manifest directory to use its `oxel.toml`. Compiled and published artifacts use the package name and go under that project’s `target/oxel/`. A published package selects the .NET SDK from the project directory, and `run` preserves the caller’s working directory. The language server reads the manifest at startup. Explicit source files are also supported as a separate invocation. [Example manifest](examples/project/oxel.toml)

### Managed libraries

Set `kind = "library"` under `[package]` to build a managed CLR library. The package name becomes its CLR namespace and must use dot-separated Oxel identifiers, such as `Samples.Math`. Oxel exports runtime functions on `Samples.Math.Program` and declared types under `Samples.Math` by default. A library can have a function named `main` with any supported signature.

A source namespace appends to the package namespace. For example, `namespace Geometry;` in `Samples.Math` emits types under `Samples.Math.Geometry` and functions on `Samples.Math.Geometry.Program`.

```sh
oxel check --manifest-path examples/math_library/oxel.toml
oxel compile --manifest-path examples/math_library/oxel.toml
oxel test --manifest-path examples/math_library/oxel.toml
```

The [example manifest](examples/math_library/oxel.toml) writes `target/oxel/lib/Samples.Math.dll` inside the project directory. Add that DLL as a .NET assembly reference and call `Samples.Math.Program.add(10, 20)` from C# or Oxel. For an Oxel consumer, pass the path using `--reference`. With SDK or NuGet references, use the path printed by `compile` and provide the library's runtime dependencies to the consuming project. The executable host selects the runtime and can be published through Native AOT.

Use `internal fn`, `internal struct` or `internal enum` for implementation details shared within the assembly. The math example exposes `add` while keeping its operands, operation enum and evaluation helper internal. Public parameters, return types, fields, enum payloads and generic constraints must use public types. The compiler checks nested type arguments and array elements too.

### NuGet packages

Add a `[nuget]` table to the manifest with exact direct package versions.

```toml
[nuget]
"Newtonsoft.Json" = "13.0.3"
```

```sh
oxel run --manifest-path examples/nuget/oxel.toml
```

Projects with NuGet dependencies use the .NET SDK selected from the manifest directory and its `NuGet.Config` settings. The SDK restores transitive dependencies and selects compile-time and runtime assets for its target framework. The compiler reads compile-time assemblies as CLR metadata. `compile` and `run` package SDK-selected implementation assemblies, `.deps.json` and runtime configuration alongside the oxel DLL. `publish` selects the host Native AOT target and uses the restored SDK graph. Package APIs are subject to oxel's CLR interoperability support and Native AOT constraints. [NuGet PackageReference](https://learn.microsoft.com/en-us/nuget/consume-packages/package-references-in-project-files)

NuGet restore runs for commands using a manifest with package declarations, including `check` and LSP startup. The SDK reuses its package cache. Transitive versions are resolved at restore time when no lock file is selected. Local assembly references and single-file compilation continue to work without a .NET SDK. [Runnable NuGet example](examples/nuget/oxel.toml)

### NuGet lock files

```sh
oxel restore --manifest-path examples/nuget/oxel.toml
oxel check --locked --manifest-path examples/nuget/oxel.toml
```

`restore` writes the SDK's package graph beside the manifest and prints the lock file path. For example, `oxel.toml` targeting .NET 10 produces `oxel.toml.nuget.net10.0.lock.json`. The file records direct and transitive package versions and package content hashes. Commit it with the manifest to reuse the resolved graph. The SDK target framework follows the selected SDK, which can be pinned with `global.json`. [NuGet lock files](https://learn.microsoft.com/en-us/nuget/consume-packages/package-references-in-project-files#locking-dependencies)

Build, run, check, publish and language server commands enforce an existing lock file for their target. Pass `--locked` to require that file for a project with NuGet dependencies. Use `oxel restore` to refresh the graph after changing dependencies, or `oxel restore --locked` to verify it without compiling. A failed refresh leaves the previous lock file in place.

Use `oxel restore --native` to generate the separate Native AOT lock file for the SDK's host runtime identifier. For example, `oxel.toml.nuget.net10.0.linux-x64.aot.lock.json` is used by `oxel publish --locked` on that host and target framework.

## Language

```rust
fn add(a: i32, b: i32) -> i32 {
    a + b
}

fn main() {
    let mut result = add(10, 20);
    if result < 0 {
        result = -result;
    }
    System.Console.WriteLine(result);
}
```

- A program has one zero-argument, void `main`. Functions can call each other in any source order. Parameters require annotations. The omitted result type is `void`.
- `bool`, `char`, `string`, `object` and `void` map to `System.Boolean`, `System.Char`, `System.String`, `System.Object` and `System.Void`. Signed integers `i8`, `i16`, `i32` and `i64` map to `System.SByte`, `System.Int16`, `System.Int32` and `System.Int64`. Unsigned integers `u8`, `u16`, `u32` and `u64` map to `System.Byte`, `System.UInt16`, `System.UInt32` and `System.UInt64`. `f32` and `f64` map to `System.Single` and `System.Double`. Fully qualified CLR names are valid annotations. `void` is a result type. [Unsigned integer example](examples/unsigned_integers.oxel)
- Integer literals default to `i32` and take another integer type from a typed context when they fit. Use decimal digits or the prefixes `0x`, `0o`, and `0b` for hexadecimal, octal, and binary. Uppercase prefixes are also accepted. Insert `_` between digits for readability, as in `0b1111_0000`, `1_000`, or `12.5_0e-0_1`. Integer arithmetic wraps at the declared width. Integer division and remainder truncate toward zero, and division by zero raises an error. The CLR also checks signed division overflow at the evaluation width. Small integers evaluate on the `i32` stack and are narrowed after each arithmetic operation. Unary `-` requires a signed integer or floating-point operand.
- Decimal floating-point literals such as `3.0`, `5e-2` and `1.25E+3` default to `f64`. A typed context can give a literal type `f32`, which parses and computes at single precision. Literals that overflow their target width are rejected. Floating-point division by zero follows IEEE 754. NaN is unequal to itself, and all ordering comparisons with NaN are false. Numeric operations use equal-width operands from the same family. [CLR floating-point comparisons](https://learn.microsoft.com/en-us/dotnet/api/system.reflection.emit.opcodes.clt_un?view=net-6.0)
- Locals have block scope. An initialized `let` can infer its type. A declaration such as `let result: i32;` gives the type and delays initialization. The compiler checks that every reachable read follows an assignment. An immutable binding can be initialized once on each path, and `let mut` permits subsequent writes. A new binding can shadow an earlier one.
- Update an initialized mutable binding with `+=`, `-=`, `*=`, `/=`, `%=`, `&=`, `|=`, `^=`, `<<=` or `>>=`. The operation uses the same operand types and rules as the corresponding binary operator. Strings support `+=` for concatenation. Compound assignments are statements and also work in constant evaluation.
- Use `&`, `|` and `^` on two integers of the same type, two booleans, or two values of the same CLR enum. Both operands are evaluated. Use `~` to complement an integer or CLR enum at its declared width. Bitwise operators bind more tightly than comparisons, with `&` binding before `^`, then `|`. These operators also work in constants and `comptime fn`. [CLR flags example](examples/bitwise.oxel) · [CLR enumeration operators](https://learn.microsoft.com/en-us/dotnet/csharp/language-reference/language-specification/expressions)
- Use `<<` and `>>` with an integer value and an `i32` count. The result keeps the value's type. Oxel masks the count by the declared bit width minus one, so `1 << 8` in a `u8` context yields `1`. Right shift extends the sign for signed integers and fills with zeroes for unsigned integers. Shifts bind more tightly than `&` and less tightly than addition.
- A block can yield its last expression by omitting the semicolon. `return` exits the containing function. An `if` condition is boolean. An `if` that yields a value requires an `else` unless evaluation leaves the function.
- A `while` loop evaluates a boolean condition before each iteration. Its body has its own scope. `break` exits the innermost enclosing loop body, and `continue` reevaluates that loop's condition. In the condition of a nested loop, either keyword targets the enclosing loop body.
- `for i in start..end { ... }` visits an ascending integer range, excluding the end. Use `..=` to include the end, even if it is the largest value of the counter's type. The bounds must have the same integer type and evaluate once, from left to right, before the body. The loop binding is immutable and local to the body. A reversed range is empty, and `continue` advances to the next value. [Range and comptime factorial example](examples/ranges.oxel)
- `for value in values { ... }` visits the elements of a non-nullable CLR vector in index order, or the UTF-16 `char` values of a non-nullable string. The source expression evaluates once before the loop, and each element is read when its iteration begins. The loop binding is immutable and local to the body. Array and string iteration also work in `comptime fn`. [Array example](examples/arrays.oxel)
- With `--framework`, `for value in values { ... }` also accepts a non-nullable CLR `IEnumerable<T>`, including collection types that implement it and type parameters constrained to it. The source is evaluated once and elements are read lazily in enumeration order. The enumerator is disposed on exhaustion and on early exit. If the source implements multiple distinct `IEnumerable<T>` interfaces, assign it to a binding of the desired interface type before iterating. Run `oxel run --framework examples/collections.oxel` to iterate over a list and a LINQ sequence. [Collection example](examples/collections.oxel) · [CLR enumeration interfaces](https://learn.microsoft.com/en-us/dotnet/csharp/language-reference/language-specification/statements#13952-synchronous-foreach)
- `&&` and `||` short-circuit. They bind more loosely than comparisons, and `&&` binds more tightly than `||`.
- Operators include `+`, `-`, `*`, `/`, `%`, unary `-` and `!`, and `==`, `!=`, `<`, `<=`, `>` and `>=`. Equality supports booleans, characters, equal-width numbers and strings. Ordering supports characters and equal-width numbers.
- String literals use double quotes, accept Unicode and support `\"`, `\\`, `\n`, `\r`, `\t` and `\0`. Use `+` to concatenate two strings. String equality is ordinal and case-sensitive. Strings returned by CLR methods retain CLR null behavior. [System.String](https://learn.microsoft.com/en-us/dotnet/api/system.string)
- Character literals such as `'A'`, `'界'`, and `'\u{D83D}'` contain one UTF-16 code unit. Characters support equality, ordering and checked casts to and from integers. For strings, `.Length` counts UTF-16 code units and `text[index]` reads a `char` at an `i32` index. Both operations work in constant evaluation. A supplementary character occupies two indices. [Character example](examples/characters.oxel) · [System.Char](https://learn.microsoft.com/en-us/dotnet/api/system.char?view=net-10.0)
- Use `$"Hello, {user.name}"` to insert an expression into a string. Write `{{` or `}}` for literal braces. Interpolation evaluates and formats each expression once, from left to right. Runtime values use CLR string conversion, and null contributes empty text. Constant evaluation supports string-valued expressions in interpolation. See the [interpolation example](examples/interpolation.oxel). [CLR string concatenation](https://learn.microsoft.com/en-us/dotnet/api/system.string.concat)
- `System.Console.WriteLine` accepts zero arguments or one value. Small integers use the CLR `Int32` overload. Other values use their matching primitive overload or the `Object` overload. Use `//` for line comments.

Invalid programs fail with a diagnostic such as `bad.oxel:2:27: error[E0301]: expected System.Int32, found System.Boolean`. Library diagnostics carry byte ranges and type information for mismatches. The CLI displays one-based line and column numbers.

## Definite assignment

```rust
fn choose(first: bool) -> i32 {
    let result: i32;
    if first {
        result = 10;
    } else {
        result = 20;
    }
    result
}
```

The check follows execution order through blocks, branches, loops, early returns, short-circuit expressions and loop exits. A path that exits the function does not need to initialize a local for later reads. An immutable local declared inside a loop can initialize anew on each iteration. A write to an immutable local declared outside the loop is rejected when a later iteration could write it again. [Runnable example](examples/assignment.oxel)

## Diagnostics for tools

Use `oxel check --diagnostic-format json main.oxel` to consume compiler diagnostics from a script or agent. The compiler writes one JSON object per line to standard error and returns a nonzero status when it reports errors. The same option is available for `compile`, `run` and `publish`. Program output and artifact paths still go to standard output. SDK and application messages may appear separately on standard error when running or publishing.

Each diagnostic includes `file`, `code`, `severity`, `message`, and a `range` with zero-based UTF-8 byte offsets and an exclusive end. `line` and `column` are one-based, with columns counted in Unicode scalar values. Type mismatches also provide `expected` and `actual` with the resolved type names. Rust consumers can use `Diagnostic::type_names()` for constructed types, whose internal IDs are scoped to one analysis.

For a wrong argument type or count in an Oxel function call, the compiler can also point to the function declaration. JSON includes this note as a `related` object with its own `file`, `range`, `line`, `column`, and `message`. The language server sends the same location to clients that support related diagnostic information.

When the compiler can suggest a source edit, the JSON diagnostic also has a `fix` with a title, a byte range in the same file, and replacement text. Apply an insertion at an empty range and a deletion when the replacement is empty.

## CLR objects

```rust
fn main() {
    let value: object = 30;
    if value is i32 {
        System.Console.WriteLine(value as i32);
    }
}
```

Values can convert implicitly to `object` in annotations, assignments, calls and returns. Value types are boxed. References keep their object identity. The `is` operator checks a value’s runtime type and yields `false` for a null reference returned by CLR code.

Use `as` for explicit boxing, checked casts between related reference types, or unboxing from `object`. A failed cast raises the standard CLR exception. Parenthesize the cast before accessing a member, as in `(value as Namespace.Type).Member()`. The [objects example](examples/objects.oxel) also boxes a generic struct and restores its concrete type. [CLR boxing and unboxing](https://learn.microsoft.com/en-us/dotnet/csharp/programming-guide/types/boxing-and-unboxing)

Use `value as? T` to get a standard `Option<T>` from a CLR runtime type check. It evaluates the input once and produces `None` for null or a type mismatch. The target must be non-nullable. CLR reference and interface matches preserve object identity, and a boxed value must match its runtime type exactly. For example, `29 as? i64` produces `None`. Use `(value as? T)?` to propagate failure. [Runnable example](examples/optional_casts.oxel)

`as` also converts between numeric types. Conversions to integers truncate fractional values toward zero, then check the target range. An out-of-range result, NaN, or infinity raises `System.OverflowException` at runtime and produces a diagnostic in constant evaluation. Conversions to floating-point types round to the destination precision. An integer literal directly before a numeric cast can use the full `i64` range. A direct cast to `u64` also accepts literals up to `18446744073709551615`. See the [numeric casts example](examples/numeric_casts.oxel). [CLR numeric conversion behavior](https://learn.microsoft.com/en-us/dotnet/csharp/language-reference/builtin-types/numeric-conversions#explicit-numeric-conversions)

## Nullable references

Reference annotations such as `string`, `object` and imported CLR class names reject source `null` by default. Add `?` when a binding, parameter, result or field can hold null. A non-null reference can widen to its nullable type. Use `??` to supply a fallback. It evaluates the left operand once and evaluates the right operand only when needed. The fallback must fit the underlying reference type, and a nullable fallback keeps the result nullable.

Comparisons with `null` refine immutable locals and parameters in `if` and `while` branches and through short-circuit boolean expressions. Mutable bindings require an explicit fallback or a checked immutable copy. A cast from a nullable reference to another reference keeps the destination nullable. [Runnable example](examples/nullable.oxel)

An early exit can establish a non-null type for the following statements. After `if text == null { return; }`, an immutable `text` can be used as `string`. The same rule applies to CLR predicates with conditional nullability contracts. When both branches can continue, a binding must be non-null on both paths to keep the refined type.

The compiler uses the same CLR type signature for a reference and its nullable form. It emits Roslyn-compatible annotations on public parameters, returns and struct fields, including concrete reference arguments in generic struct signatures. It reads nullable annotations and inherited declaration context from imported method and property signatures. References without annotations retain their prior behavior. Explicitly annotate results from those APIs when they can return null. [Roslyn nullable metadata](https://github.com/dotnet/roslyn/blob/main/docs/features/nullable-metadata.md)

Use concrete nullable arguments in generic types, such as `Cell<string?>` and `System.Collections.Generic.List<string?>`. Substituting a nullable argument preserves its nullability in values read from fields, methods and indexers. The compiler reads and emits these annotations in nested CLR signatures. Run the example with `oxel run --framework examples/nullable_generics.oxel`. [Runnable example](examples/nullable_generics.oxel)

Oxel imports `MaybeNull` and `NotNull` on CLR method returns, properties, indexers and field reads. These contracts affect the outermost reference, including a concrete reference substituted for a generic parameter. A `MaybeNull` result can infer `Option<T>` in a direct `let` binding. [C# nullability analysis attributes](https://learn.microsoft.com/en-us/dotnet/csharp/language-reference/attributes/nullable-analysis)

`AllowNull` and `DisallowNull` govern by-value reference arguments and reference writes to CLR properties, indexers and fields. For example, an `AllowNull` property can accept `null` in its setter and still return a non-null reference from its getter. A mutable CLR field that accepts `null` is also read as nullable because a write directly changes its storage.

`NotNullIfNotNull` can refine a CLR method or indexer result to a non-null reference when the named by-value argument is known to be non-null. Otherwise the result retains its normal nullability and can still map to `Option<T>`. [C# conditional postconditions](https://learn.microsoft.com/en-us/dotnet/csharp/language-reference/attributes/nullable-analysis#conditional-postconditions-notnullwhen-maybenullwhen-and-notnullifnotnull)

With framework references, `System.IO.Path.GetFileName("src/main.oxel")` can be assigned directly to `string`. Passing a nullable path to the same API preserves its nullable result.

`NotNullWhen` can refine an immutable by-value binding or an immutable `out` local on the specified boolean branch. For example, the true branch of `!System.String.IsNullOrEmpty(text)` can use `text` as a `string`. A `MaybeNullWhen` contract can also preserve a declared non-null `out` reference on the opposite branch, as with `Dictionary<string, string>.TryGetValue`. Refinement works in `if`, `while`, short-circuit expressions and match guards. Mutable locals require an explicit check on an immutable copy or a fallback. [C# conditional postconditions](https://learn.microsoft.com/en-us/dotnet/csharp/language-reference/attributes/nullable-analysis#conditional-postconditions-notnullwhen-maybenullwhen-and-notnullifnotnull)

A CLR parameter marked `NotNull` refines an immutable argument or an immutable `out` local after the call returns normally. With framework references, call `System.ArgumentNullException.ThrowIfNull(text);` to check a nullable argument and then use `text` as a non-null reference. A catch handler that allows execution to continue after a failed check leaves the argument nullable. [CLR unconditional postconditions](https://learn.microsoft.com/en-us/dotnet/csharp/language-reference/attributes/nullable-analysis#unconditional-postconditions-maybenull-and-notnull)

Oxel also reads `DoesNotReturnIf` on CLR boolean parameters. For example, if `Reject` has `[DoesNotReturnIf(true)]`, a normal return from `Reject(text == null)` establishes that an immutable `text` is non-null. Short-circuit checks such as `Reject(first == null || second == null)` can establish both facts at once. [CLR guard contracts](https://learn.microsoft.com/en-us/dotnet/csharp/language-reference/attributes/nullable-analysis#stop-nullable-analysis-when-called-method-throws)

## Nullable values

With `--framework`, a suffix on a non-nullable value type such as `i32?` constructs the CLR type `System.Nullable<i32>`. Use `null` for an empty value and a value of the underlying type for a present value. This applies to CLR enums, structs and generic type parameters constrained to `struct`. Use `value != null`, `value.HasValue`, or `value ?? fallback` to handle absence. An explicit unwrap with `as` or `.Value` throws if the value is empty. An empty nullable value boxes to `null`, so a direct boxing conversion requires `object?`. [Nullable value example](examples/nullable_values.oxel) · [CLR nullable value types](https://learn.microsoft.com/en-us/dotnet/csharp/language-reference/builtin-types/nullable-value-types)

```sh
oxel run --framework examples/nullable_values.oxel
```

## Arrays

`T[]` is a CLR zero-based vector. Use `[10, 20]` to initialize its elements, or `new i32[count]` to allocate zero-initialized storage. A typed context supplies the element type for a literal, including an empty literal such as `let empty: i32[] = [];`. Otherwise the first element supplies the type. Use `.Length` for the length and an `i32` index for reads and writes. A compound update such as `values[i] += 2` evaluates the array and index once.

Nullability is independent at each level. `string?[]` has nullable elements and `string[]?` is a nullable array with non-nullable elements. Use a literal for non-nullable reference elements, or `new string?[count]` for storage initialized to null. Length allocation requires an element type with a valid zero value. Jagged vectors use nested array types such as `i32[][]`.

The standard `Option<T>` treats zero-initialized CLR storage as `None`, including `new Option<string>[count]` and arrays of Oxel structs whose fields all have valid zero values. A `Some` payload is read only after the option's tag identifies a present value. This behavior also applies in `comptime fn`. [Option array example](examples/option_arrays.oxel)

Supported CLR signatures can pass and return these arrays, including generic `T[]` signatures. Array element types are invariant in oxel. Rectangular arrays are outside this subset. The [array example](examples/arrays.oxel) prints `30`. [CLR vector allocation](https://learn.microsoft.com/dotnet/api/system.reflection.emit.opcodes.newarr)

## Structs

```rust
struct User {
    name: string,
    age: i32,
}

fn birthday(user: User) -> User {
    User { name: user.name, age: user.age + 1 }
}
```

A struct is a named CLR value type with public instance fields. It can be stored in a local, passed to a function, returned, or embedded as a field of another struct. Assignment and argument passing copy the value. Types can be declared in any order. By-value cycles are rejected because they would require an infinite layout.

A literal must initialize each field exactly once. Initializers evaluate in source order and can be written in any order. Read fields with `value.field`. To change a value in a mutable binding, construct a replacement and reassign the binding. When using a struct literal directly in an `if` condition, parenthesize it to distinguish the literal from the condition's block.

Emitted structs and functions that use them are accessible to other .NET languages. Struct fields can also hold reference types imported from CLR assemblies.

Use `Window { title: "Settings", ..original }` to construct a copy with selected fields replaced. Explicit field expressions evaluate first in source order, followed by the base expression, exactly once each. The base must have the same struct type and goes last. For generic structs, write the type arguments explicitly or supply them through the expected type, as with an ordinary struct literal. Struct update also works in compile-time code. [Nested struct update example](examples/struct_update.oxel)

Use `let Window { title: name, .. } = window;` to bind a field from a struct. A field pattern can itself bind a value, test a literal, or destructure a nested struct or enum. List every field or use a final `..` to ignore the remaining fields. Generic arguments come from the scrutinee type. An ordinary `let` pattern must cover every value of its type. Use `let … else`, `if let`, `while let` or `match` for patterns that can fail. [Nested struct example](examples/struct_update.oxel)

Use `title` as shorthand for `title: title` in both literals and patterns. At that token, editor navigation selects the variable. Renaming either the field or variable expands the shorthand when needed so the other name keeps its meaning.

### Generic structs

```rust
struct Container<T> {
    value: T,
}

fn wrap<T>(value: T) -> Container<T> {
    Container { value: value }
}

fn main() {
    let n = Container::<i32> { value: 30 };
    System.Console.WriteLine(n.value);
}
```

Use `Container<T>` in type annotations and `Container::<T> { ... }` in literals. A literal can omit its type arguments when its expected type already names the same struct and supplies the arguments. Generic structs can be nested and retain CLR value semantics. Layout checking follows the type arguments that contribute to field layout and rejects infinite by-value recursion.

The compiler emits public CLR generic value types, which C# can use as `Container<int>`. The [generic structs example](examples/generic_structs.oxel) combines nested structs, field access and generic functions. [ECMA-335 generic types](https://ecma-international.org/wp-content/uploads/ECMA-335_6th_edition_june_2012.pdf)

## Discriminated unions and match

```rust
enum Result<T, E> {
    Ok(T),
    Err(E),
}

fn number(result: Result<i32, string>) -> i32 {
    match result {
        Result.Ok(value) => value,
        Result.Err(_) => 0,
    }
}

fn main() {
    let result: Result<i32, string> = Result.Ok(30);
    System.Console.WriteLine(number(result));
}
```

A variant can carry zero or more positional values. Construct a unit variant with `Type.Variant` and pass payloads with `Type.Variant(values)`. Generic variant construction takes its arguments from the expected enum type or from an explicit call such as `Result.Ok::<i32, string>(30)`.

`match` evaluates its input once. Arms use either a qualified variant name or a name from the input's enum. Payload bindings are immutable and scoped to their arm. Use `_` to discard one payload or as the final arm to cover the remaining variants. The compiler checks exhaustiveness, binding counts and compatible result types. Match arms participate in definite assignment and can return from the surrounding function.

In runtime functions, use `let Some(value) = input else { return; };` to destructure a refutable pattern. The initializer evaluates once and the immutable bindings are available after the statement. The `else` block sees the enclosing scope and must exit the current path, for example through `return`, `throw`, `break` or `continue`. Boolean and nested enum patterns use the same rules as `match`. An optional annotation after the pattern supplies the initializer type. [Let-else example](examples/let_else.oxel)

Use `if let Some(value) = input { ... }` to run a block only when the pattern matches. Add `else` to handle failure or to return a value from the conditional. Use `while let Some(value) = next() { ... }` to reevaluate the input before each iteration and stop when the pattern fails. Bindings are scoped to the successful branch or loop body. Both forms can match nullable CLR values directly. [Pattern conditions example](examples/pattern_conditions.oxel)

Patterns can inspect nested variants or boolean payloads. For example, a `Result<Maybe<i32>, string>` can distinguish `Ok(Some(value))`, `Ok(Maybe.None)`, and `Err(_)`. Inside a payload pattern, a bare name binds the value. Use `Maybe.None` or `None()` to match a nested unit variant. A standalone `bool` can also be matched with `true` and `false` arms. Match tests its arms in source order and runs the first successful arm. The compiler reports both uncovered cases and arms fully covered by earlier patterns.

Add a boolean guard after a pattern, as in `Some(value) if value > 0 => value`. The guard sees the pattern bindings and evaluates once after the pattern matches. A false guard continues to the next arm. Exhaustiveness is checked using the unguarded arms. [Match guard example](examples/match_guards.oxel)

Integer, character and string literals are patterns too. Use `200`, `-1`, `'界'` or `Some("cached")` in a match, or use a literal with `if let`, `while let` and `let … else`. Integer literals must fit the input type and equivalent spellings such as `0xa` and `10` cover the same value. Text matching is ordinal and case-sensitive. Use `_` for values outside the listed literals, or list every value of a finite type such as `u8`. [Literal pattern example](examples/literal_patterns.oxel)

Dense matches on characters and integers up to 32 bits can use a CIL jump table. Values in gaps and outside the table follow the default branch. [CIL switch instruction](https://learn.microsoft.com/en-us/dotnet/api/system.reflection.emit.opcodes.switch?view=net-10.0)

Use `|` to share a branch between patterns, as in `200 | 201 => "success"` or `Some("start" | "stop")`. Alternatives must bind the same names with the same types. The first matching alternative supplies the bindings. Its arm's guard runs once, and a false guard continues with the next arm. Alternatives also work in `if let`, `while let` and `let … else`. [Pattern alternatives example](examples/pattern_alternatives.oxel)

The compiler emits tagged CLR value types, including generic CLR value types for generic enums. Their storage layout may change. A default value supplied by CLR code is `None` for the standard `Option<T>`. For a user-defined enum, its default value has no valid tag, and matching it raises `System.InvalidOperationException`. By-value recursive payloads are rejected. [Runnable example](examples/unions.oxel)

## Error propagation

```rust
enum Result<T, E> { Ok(T), Err(E) }

fn next(input: Result<i32, string>) -> Result<i32, string> {
    let value = input?;
    Result.Ok(value + 1)
}
```

The postfix `?` evaluates its operand once. It yields the `Ok` payload or immediately returns an `Err` from the enclosing function. Use it with the module's `Result<T, E>` enum, whose variants must be `Ok(T)` and `Err(E)`. Variant order and type parameter names are unrestricted.

The two `Result` types may have different success types. The input error must be assignable to the function's error type, including ordinary CLR reference conversions and boxing to `object`. The operand determines its own generic arguments, so use a typed input or supply explicit arguments to an otherwise ambiguous call. [Runnable example](examples/errors.oxel)

`try { ... }` gives `?` a local propagation target. For `Result<T, E>`, a successful tail expression becomes `Ok`. On `Err`, the nearest enclosing try block produces the error result and the enclosing expression continues. The first `?` with a typed operand determines the error type unless the surrounding expression supplies an expected `Result` type. A try block requires a `?` and a success value, or an explicit success type if the body diverges. Loop control is available in loops declared within the block. Use `return` in the enclosing function after handling the block's result. [Result example](examples/try_blocks.oxel)

The standard `Option<T>` enum provides `Option.Some(value)` and `Option.None`. Supply the element type through a typed context or use an explicit constructor such as `Option.Some::<i32>(42)`. The `?` operator yields the `Some` payload or returns `None` from the enclosing function or nearest try block. An Option try block wraps its successful tail in `Some`. Each try block uses one enum family, inferred from its operands or supplied by its expected type. Nest try blocks to use both families in a single expression. Existing modules can still declare their own `Option`. [Option example](examples/option_try.oxel)

A direct CLR method, property, field or indexer read in a `let` initializer infers `Option<T>` when the result is nullable. A nullable reference or `System.Nullable<T>` also converts to the standard `Option` when an argument or result expects it. An absent CLR value becomes `None`, and a present value becomes `Some(value)`. Use `Option<T>` in Oxel signatures and an explicit annotation such as `let name: string? = ...` to select the underlying CLR type. The postfix `?` can propagate `None` directly from a nullable CLR call, including `(System.Array.Find(names) |name| { name == target })?`. The input expression is evaluated once. Use `match` to handle both cases or `option ?? fallback` to supply a value on `None`. Use `oxel run --framework examples/clr_option.oxel` to try these forms. [CLR Option example](examples/clr_option.oxel)

In the other direction, an expected CLR nullable type with the same underlying type accepts a standard `Option<T>`. For example, pass an `Option<string>` directly to a `string?` argument or write it to an element of `List<string?>`. A value of `None` produces a null reference or an empty `System.Nullable<T>`. A value of `Some(0)` produces a populated nullable integer containing zero. Use an explicit cast such as `choice as string?` to select the CLR type at an overloaded call. An exact nullable conversion takes precedence over boxing the option when overloads require the same number of conversions. Managed `ref` and `out` parameters require a local of the CLR storage type. [CLR nullable values](https://learn.microsoft.com/en-us/dotnet/api/system.nullable-1?view=net-10.0)

Use `throw value` with `--framework` to raise a CLR exception. The value must be assignable to non-nullable `System.Exception`. A throw ends the current control-flow path, so another branch can supply the value of an enclosing expression. [Runnable example](examples/exceptions.oxel) · [CIL throw instruction](https://learn.microsoft.com/en-us/dotnet/api/system.reflection.emit.opcodes.throw?view=net-10.0)

```sh
oxel run --framework examples/exceptions.oxel
oxel run --framework examples/exceptions.oxel -- disable
```

Use `try { ... } catch (error: System.Exception) { ... }` with `--framework` to handle CLR exceptions. Catch clauses are checked in source order and can name derived exception classes. Use `_` to ignore the exception. The body and handlers can produce a value, including inside a larger expression. An unmatched exception continues unwinding. This form uses CLR exception regions. The existing `try { ... }` form without a catch clause provides a propagation target for `Option` and `Result`. [Runnable example](examples/catch.oxel) · [CLR exception handling](https://learn.microsoft.com/en-us/dotnet/standard/exceptions/)

`return`, `break`, `continue` and `?` can cross CLR exception regions. A propagation block keeps its existing rules for control transfers.

### Scoped resources

Use `using name = resource { ... }` with `--framework` to acquire a non-null `System.IDisposable` resource once and bind it for the duration of the block. The block can produce a value. Oxel calls `IDisposable.Dispose` in a CLR finally handler when control leaves the block, including by an exception, `return`, `?`, `break` or `continue`. Nested bindings dispose in reverse acquisition order. The resource type can be a CLR class, a CLR struct or a generic type constrained to `IDisposable`. [Runnable example](examples/using.oxel) · [CLR using semantics](https://learn.microsoft.com/en-us/dotnet/csharp/language-reference/statements/using)

## Generic functions

```rust
fn identity<T>(value: T) -> T {
    let copy: T = value;
    copy
}

fn main() {
    System.Console.WriteLine(identity(30));
}
```

Declare one or more type parameters on a function. A call can infer them from its arguments, including the arguments of a constructed type such as `Container<i32>`. An expected result type can fill in parameters that the arguments leave open. Repeated evidence must agree, and decimal literals can take their width from other arguments or the result context. Use `::<...>` at a call site to supply all type arguments explicitly.

Type parameters can appear in function signatures and local annotations, and can be forwarded to other generic calls. An unconstrained parameter can be stored, passed or returned as that same type. Operations such as arithmetic still require a supported concrete operand type.

oxel emits CLR generic methods. The emitted definitions can also be called from C# as `Program.identity<int>(30)`. The [generics example](examples/generics.oxel) instantiates the same method with a struct, an integer and a string. [ECMA-335 generic signatures and method instantiations](https://ecma-international.org/wp-content/uploads/ECMA-335_6th_edition_june_2012.pdf)

### Generic constraints

```rust
fn read<T: Samples.Interop.IReadable>(value: T) -> i32 {
    value.Read()
}
```

A method type parameter can name one imported CLR base class and any number of imported interfaces, separated by `+`. The compiler checks these bounds for inferred and explicit calls, including calls that forward a type parameter. Bound members are available inside the method, and a value of the type parameter can be passed as one of its bound types. Class-bound members take precedence over interface-bound members with the same name.

Bounds can name constructed types and refer to the function's type parameters. For example, `fn compare<T: Library.ICompare<T>>(first: T, second: T) -> i32 { first.Compare(second) }` calls the imported interface through its CLR generic constraint. The CLR signature preserves the constructed bound for C# and other callers.

Generic `struct` and `enum` declarations use the same bound syntax. A bound is available when checking fields or variant payloads that use a constrained type. For example, the [generic interop example](examples/interop/generic_classes.oxel) declares `CellView<T: Samples.Interop.IReadCell<i64>>`. C# can instantiate the emitted `CellView<T>` with a type that implements that interface.

Type parameters can constrain one another. For example, `fn widen<U, T: U>(value: T) -> U { value }` requires a type argument for `T` that can convert to `U`. Members available through `U`'s bounds are also available on `T`. The compiler rejects cycles and incompatible class bounds in these dependencies. [CLR generic constraints](https://learn.microsoft.com/en-us/dotnet/csharp/language-reference/language-specification/classes#1525-type-parameter-constraints)

Use `T: class` to require a reference type and `T: struct` for a non-nullable value type. Use `T: new()` to call `new T()`. Combine the requirements with `T: struct + new()`. Oxel checks that an Oxel-defined type can safely be zero-initialized before accepting it for a `new()` constraint, including explicit `new()` constraints on imported APIs. A CLR value type with an explicit parameterless constructor runs that constructor through the CLR's generic constructor helper. CLR callers and imported APIs continue to follow CLR metadata constraints. [CLR generic parameter constraints](https://learn.microsoft.com/en-us/dotnet/csharp/programming-guide/generics/constraints-on-type-parameters) · [CLR generic construction](https://learn.microsoft.com/en-us/dotnet/api/system.activator.createinstance)

The compiler writes CLR generic constraint metadata and uses constrained instance dispatch. C# callers can instantiate an interface-constrained oxel method with a value type that implements the interface. [ECMA-335 generic parameter constraints](https://ecma-international.org/wp-content/uploads/ECMA-335_6th_edition_june_2012.pdf)

Build the [interop library](examples/interop/Bridge.csproj) to run the [constraints example](examples/interop/constraints.oxel).

```sh
dotnet build examples/interop/Bridge.csproj -o target/interop
oxel run examples/interop/constraints.oxel --reference target/interop/Bridge.dll
```

## Compile-time constants

```rust
const TOTAL = if ENABLED { STEP * 3 } else { 0 };
const STEP: i32 = 10;
const ENABLED = STEP > 0;
```

Module-level `const` declarations accept an optional type annotation. They evaluate during `check` and `compile` in Rust, and compiled code uses the resulting values directly. Constants can refer to declarations in any source order, including declarations in other files of the same module.

Initializers support boolean, integer, floating-point and string literals, other constants, unary and binary operators, and value-producing blocks and `if` expressions. Both branches are type checked and contribute to dependency analysis. Only the selected branch runs. Boolean operators short-circuit.

Integer addition, subtraction, multiplication and negation wrap at the type's width, as they do at runtime. Integer division and remainder by zero, signed overflow at the CLR evaluation width, and dependency cycles produce compile-time errors. Floating-point folding retains the operation width, permits infinity and NaN, and uses a canonical NaN when emitting a folded value. [CLR division](https://learn.microsoft.com/en-us/dotnet/api/system.reflection.emit.opcodes.div) · [CLR remainder](https://learn.microsoft.com/en-us/dotnet/api/system.reflection.emit.opcodes.rem)

[Runnable constants example](examples/constants.oxel)

A `string?` constant can hold a string or `null`. Use `const LABEL: string? = null;` to give an absent value its type. Compile-time code can compare nullable strings with `null`, choose a lazy fallback with `??`, and convert between `string?` and `Option<string>` through a typed binding or return. A null interpolated part contributes empty text, as it does at runtime.

### Comptime functions

```rust
const VALUE = factorial(10);

comptime fn factorial(n: i32) -> i32 {
    let mut result = 1;
    for factor in 1..=n {
        result *= factor;
    }
    result
}
```

The Rust compiler type checks and interprets `comptime fn` bodies. They are available in constant initializers and other comptime functions. Signatures can use primitives and Oxel structs and enums whose fields also use supported value types. Generic comptime functions accept inferred or explicit type arguments. For example, `comptime fn identity<T>(value: T) -> T { value }` works with both `identity(42)` and `identity::<string>("oxel")`. Parameters, returns and locals must use supported value types after substitution. Comptime functions can read module constants, use local variables, branch, loop, and return early. Comptime methods exist only during compilation.

Module-level `const` can hold primitives, structs and enums returned from a comptime function. Code can access its fields, match it, or copy it into a local. The assembly constructs the value from its precomputed fields at the use site. Shared parts of a nested constant are constructed once per use.

Compile-time code can create non-null arrays with supported element types, use `.Length`, read and write elements, and pass arrays between comptime functions. Assigning or passing an array shares its storage, as at runtime. An initializer can use arrays while computing a constant. The resulting module-level `const` must contain only immutable values, so it cannot retain a mutable array, including through a struct or enum payload.

Compile-time `match` supports booleans, integer, character and string literals, and enum variants with payloads. It supports alternatives and guards and uses the same exhaustiveness rules as runtime matching. It evaluates only the matching arms' guards and the selected arm. These patterns also work in `if let`, `while let` and `let … else` inside comptime functions. Use `?` to propagate `Option` or `Result` failures, and `try { ... }` to capture them locally.

Use `compiler.type_name::<T>()` for the resolved display name of a type, such as `System.Int32`. Use `compiler.field_names::<T>()` for an Oxel struct's fields in declaration order, or the public instance fields declared by an imported CLR type in metadata order. Use `compiler.variant_names::<T>()` for an Oxel enum's variants in declaration order. The name lists are `string[]` values that you can iterate over in a comptime function. These queries read compiler information and imported metadata during constant evaluation. When a query depends on a generic parameter, it uses the concrete type for that call.

`compiler.fields_of::<T>()` returns the same fields as records of type `compiler.FieldInfo`. Each record has `name: string`, `type_name: string`, and `readonly: bool` fields. The type name uses the concrete generic arguments of `T`. Oxel fields are read-only in source, and imported CLR fields report their metadata's read-only flag. The structured query requires each field's declared type to be available in the compiler. Use `field_names` to read names from metadata when a field type is unavailable. A comptime function can iterate over the records, or a module constant can hold one record for runtime use.

The compiler tracks dependencies through calls, including across files, and rejects cycles involving a constant declaration. Recursive functions are allowed. Evaluation has step and nesting limits and detects repeated active calls with the same type arguments and numeric, boolean or null values when those calls are tracked. Within one initializer it can reuse completed calls with comparable arguments and results. [Runnable comptime example](examples/comptime.oxel)

## CLR references

### .NET framework references

Use `--framework` to call supported APIs from the .NET framework selected by the installed SDK. The SDK resolves its `Microsoft.NETCore.App` targeting pack, and oxel reads its metadata in Rust. The output uses the corresponding target framework and loads the implementations provided by .NET at runtime. [Reference assemblies](https://learn.microsoft.com/en-us/dotnet/standard/assembly/reference-assemblies)

```sh
oxel run --framework examples/file_lines.oxel -- README.md
```

The option also works with `check`, `compile`, `publish`, and `lsp`. Set `framework = true` under `[package]` in `oxel.toml` to enable it for a project. Set the SDK version with `global.json` in the working directory, or in the manifest directory for manifest builds. Local CLR references and NuGet packages can be used alongside the framework APIs. [File line-counting example](examples/file_lines.oxel)

For projects without NuGet dependencies, `check` and LSP can load framework references directly from the SDK's targeting-pack manifests when a single stable SDK is installed. In that setup, `compile`, `run`, and `test` also stage managed outputs directly when there are no local CLR references. They use the installed pack's target framework, minimum runtime version, and application dependency manifest. Projects with `global.json`, multiple SDKs, or custom SDK settings use the SDK path. [SDK selection](https://learn.microsoft.com/en-us/dotnet/core/versions/selection)

Direct framework execution reuses native library snapshots by content from `target/oxel/native-cache`. Each run stages its DLL beside the matching native assets, using hard links when available. A change to a native library creates a new snapshot.

With framework references enabled, aliases can call the public CLR instance methods whose signatures oxel supports. For example, `" oxel ".Trim()` returns `"oxel"` and `(30).ToString("D3")` returns `"030"`. Numeric, Boolean and character methods receive a managed address. String and object calls use reference dispatch. Nullable receivers must be narrowed before a call. [CLR instance calls](https://learn.microsoft.com/dotnet/api/system.reflection.emit.opcodes.call)

Aliases also use the interfaces declared by their CLR types. An `i32` satisfies `T: System.IFormattable`, and assigning it to an interface boxes it. Oxel can infer `T` from the implemented `System.IComparable<T>` interface when a call receives an `i32` or `string`. [Runnable example](examples/alias_interfaces.oxel)

### Local assemblies

Pass an implementation DLL with `--reference` to check, compile or run code that uses its public methods and reference types. Repeat the flag for each DLL, including any additional DLLs the application needs at runtime.

```sh
oxel check app.oxel --reference lib/Bridge.dll
oxel compile app.oxel --reference lib/Bridge.dll
oxel run app.oxel --reference lib/Bridge.dll
```

For example, a public static `Add(int, int)` on a class `Calculator` in namespace `Company.Tools` can be called as `Company.Tools.Calculator.Add(10, 20)`. Overload resolution prefers fewer conversions and can use a fitting integer literal at another integer width or parse a decimal literal at single precision. When several overloads require the same number of conversions, one can win if its parameter types are at least as specific in every position and more specific in at least one. An unresolved tie produces an ambiguity diagnostic.

Public, top-level, non-generic CLR classes and interfaces can appear in annotations, locals, function signatures and struct fields. Use `new Namespace.Class(args)` to invoke a public constructor, and `receiver.Method(args)` to call a public instance method on the receiver's type or an imported base type. An interface receiver can also call methods from its imported parent interfaces. Calls use CLR virtual and interface dispatch. A local or constant at the start of a dotted call supplies a value receiver. Otherwise, the dotted path is resolved as a static method name.

Supported signatures use `bool`, the numeric aliases, `string`, `object` or imported reference types, plus `void` for results. When a signature names a type from another assembly, pass that assembly as an additional reference with the identity recorded in the signature. The compiler uses the same referenced assemblies to follow base classes and interface implementations. An imported reference can convert implicitly to an imported base class or interface in a binding, assignment, argument, return or field initializer. If an `if` expression has no expected type, one branch's type can serve as the result when the other is assignable to it. An explicit annotation can supply a common interface for unrelated branch types. [ECMA-335 assignment compatibility](https://ecma-international.org/wp-content/uploads/ECMA-335_6th_edition_june_2012.pdf)

Read and assign public CLR fields and properties with `value.Member` and `value.Member = expression;`. Use `Namespace.Type.Member` for static members, or `Namespace.Type::<i32>.Member` on a constructed generic type. Mutating a CLR object through a reference works with immutable bindings. An assignment evaluates the receiver before the assigned value. A compound assignment evaluates the receiver once, reads the current field or property value, evaluates the right-hand side, and then writes back the result. A property needs both a supported getter and a supported setter for a compound assignment. Readonly fields can be read. Field access recognizes CLR nullable reference annotations and preserves the memory ordering of `volatile` reads and writes. Other custom field modifiers are outside the supported subset. [CLR field attributes](https://learn.microsoft.com/en-us/dotnet/api/system.reflection.fieldattributes) [CLR volatile field marker](https://learn.microsoft.com/en-us/dotnet/api/system.runtime.compilerservices.isvolatile)

Public CLR literal fields inline supported primitive and enum values, Unicode strings and null references. For example, `const LIMIT = Namespace.Settings.MaxRetries;` can read a CLR integer constant without loading the assembly into the compiler. `const` and `comptime fn` can use supported non-null values from these fields. [ECMA-335 Constant table](https://ecma-international.org/wp-content/uploads/ECMA-335_6th_edition_june_2012.pdf)

Top-level public CLR enums with signed or unsigned 8-, 16-, 32-, or 64-bit storage keep their CLR type in signatures, fields, arrays, generic arguments and boxes. Use qualified members such as `System.StringComparison.OrdinalIgnoreCase` and compare values of the same enum with `==` or `!=`. CLR base and interface conversions box the value. [Enum comparison example](examples/enum_interop.oxel) · [ECMA-335 enum semantics](https://ecma-international.org/wp-content/uploads/ECMA-335_6th_edition_june_2012.pdf)

Use `as` to convert explicitly between a CLR enum and a numeric type, `char`, or another CLR enum. Oxel checks the conversion against each enum's underlying integer type. Any in-range value is allowed, including flag combinations without a named member. A direct cast to a `u64`-backed enum accepts literals up to `u64`'s maximum. These conversions work in constant evaluation, where overflow produces a diagnostic. [CLR flags example](examples/bitwise.oxel) · [Enumeration conversion rules](https://learn.microsoft.com/en-us/dotnet/csharp/language-reference/language-specification/conversions#1033-explicit-enumeration-conversions)

Public, top-level CLR structs support public constructors and members, by-value parameters and returns, fields, boxing, and generic arguments. Generic structs such as `System.Collections.Generic.KeyValuePair<string, i32>` preserve CLR type arguments and constraints. Use a `let mut` local as the receiver to persist mutations or assign a field, property, or indexer. With a concrete CLR struct receiver, calls through immutable bindings and temporary values operate on a copy. Build arrays of imported structs from explicit values such as `[start, finish]` so each element has an initializer. Ref-like structs remain outside this subset. [Date range example](examples/date_ranges.oxel) · [Key-value pair example](examples/key_value_pairs.oxel) · [CLR struct value semantics](https://learn.microsoft.com/en-us/dotnet/csharp/language-reference/language-specification/structs)

Property access covers properties with supported types and ordinary public getters or setters. Imported CLR default indexed properties use `value[index]` or `value[first, second]`. Index argument types select an overload. Reads require a public getter, writes require a public setter, and compound assignments need both. A compound assignment evaluates the receiver and each index once, then reads the old value, evaluates the right-hand side, and writes the result. Instance indexers work through constructed generic classes and interface constraints. Ref-returning and init-only accessors are outside the supported subset. [ECMA-335 Property and MethodSemantics tables](https://ecma-international.org/wp-content/uploads/ECMA-335_6th_edition_june_2012.pdf)

The [indexer example](examples/interop/indexers.oxel) prints `30`.

```sh
dotnet build examples/interop/Bridge.csproj -o target/interop
oxel run examples/interop/indexers.oxel --reference target/interop/Bridge.dll
```

Constructor, static method and static property lookup use the named declaring type. Imported public static generic methods on non-generic types can infer type arguments from their arguments or accept explicit arguments such as `Namespace.Type.Identity::<i32>(30)`. When otherwise tied overloads have identical instantiated parameter types, the non-generic overload takes precedence. Imported method signatures can use supported concrete types, method type parameters and constructed CLR reference types. For example, a call to C# `Copy<T>(Cell<T> value)` can infer `T` from an argument of type `Cell<i32>`.

Calls to CLR methods and constructors can omit trailing positional parameters marked optional in metadata when their defaults are supported CLI constants. The compiler emits the defaults at the call site, including primitive and enum values, strings, null references and zero-initialized value types. Among otherwise tied overloads, a call with no omitted parameters takes precedence. Pass an argument explicitly for defaults stored only in custom attributes. For parameters with caller-info attributes, oxel uses the declared default constant. [Runnable example](examples/optional_arguments.oxel) · [C# optional arguments](https://learn.microsoft.com/en-us/dotnet/csharp/programming-guide/classes-and-structs/named-and-optional-arguments)

For CLR methods and constructors declared with `params T[]`, pass a compatible array directly or pass zero or more trailing values. Oxel evaluates the trailing expressions from left to right and builds a CLR vector for the call. Generic methods can infer the element type from the supplied values. When there are no values and no other arguments to infer from, specify the type argument explicitly. A compatible explicit array keeps its identity, and a fixed-arity overload wins when its argument types otherwise tie with an expanded overload. [Runnable example](examples/parameter_arrays.oxel) · [CLR parameter arrays](https://learn.microsoft.com/en-us/dotnet/api/system.paramarrayattribute)

The call-site spelling determines the passing mode. Oxel only considers CLR overloads whose parameter modes match the call before it compares parameter types and conversions. For example, `f(x)` requires a by-value overload, so calling a method that only has an `in` parameter requires `f(&x)`.

| Call | CLR parameter | Requirement |
| --- | --- | --- |
| `f(x)` | By value | An expression of a compatible type |
| `f(&x)` | `in` or `ref readonly` | An initialized local, function parameter, or lambda parameter |
| `f(&mut x)` | `ref` | An initialized, mutable local |
| `f(out x)` | `out` | A mutable local or a delayed `let` that the call initializes |

When passing a CLR object binding with `&mut`, both the local and the CLR `ref` parameter must be nullable. An `out` local becomes definitely assigned on normal return, even when the method returns `false`. To write it again, declare it with `let mut`. [TryParse and Interlocked example](examples/byref_arguments.oxel) · [C# reference parameters](https://learn.microsoft.com/en-us/dotnet/csharp/language-reference/keywords/method-parameters)

Managed-reference arguments work with imported methods, constructors, and delegate calls, including calls through interfaces and virtual dispatch. The storage type must match the CLR parameter type, even if a null check has narrowed the type for ordinary reads. `&value` and `&mut value` are call-site forms that follow CLR aliasing rules. The spellings `in value` and `ref value` also work.

### Span views

With `--framework`, use `System.Span<T>` for a writable view and `System.ReadOnlySpan<T>` for a read-only view. Create a view with a CLR constructor or `System.MemoryExtensions.AsSpan`, then use `.Slice(start, length)` for a subview and `[index]` to access an element. Views share the underlying storage, and mutations through a writable view are visible through other views of that storage. Pass a view by value to an Oxel or CLR function that accepts its type. [Runnable span example](examples/slices.oxel) · [.NET span guidelines](https://learn.microsoft.com/en-us/dotnet/standard/memory-and-spans/memory-t-usage-guidelines)

Span values are stack-only. Oxel supports them as locals and by-value parameters. Heap storage, boxing, closure capture, and use as generic arguments require other types. Returning a span from an Oxel function or obtaining one from a CLR call with by-reference parameters requires escape analysis and is currently unavailable.

### Trailing lambdas

Append `{ ... }` to a call to pass a zero-parameter lambda as its last argument. For a delegate with parameters, write `|sender, args| { ... }` after the call and use `_` to discard an unused parameter. The CLR delegate's `Invoke` signature supplies the parameter and return types. A block returning a value uses a tail expression.

```rust
button.OnClick() |sender, _| {
    if sender != null {
        button.Count += 1;
    }
};
```

Here `OnClick` is a CLR method accepting an `EventHandler`. The lambda captures the immutable `button` binding, so the callback can still update the referenced object after the registration function returns. A nested lambda can capture parameters and bindings from any enclosing scope. A lambda inside a generic function can use the enclosing type parameters and their CLR constraints. For example, `fn remember<T>(value: T) -> System.Func<T> { || { value } }` returns a delegate that captures `value`. [Runnable threading example](examples/trailing_lambda.oxel) · [Generic lambda example](examples/generic_lambdas.oxel) · [CLR delegate binding](https://learn.microsoft.com/en-us/dotnet/api/system.delegate.createdelegate)

Capturing a `let mut` binding gives the declaring scope and all its callbacks shared storage for that variable. Initialize it before creating a callback that captures it. Each execution of the declaration creates its own storage, including declarations inside loops. Captured mutable locals also support CLR `ref` and `out` arguments. [Mutable captures example](examples/mutable_captures.oxel)

Constructed delegates such as `System.Action<i32>` also work. For a generic CLR method such as `System.Array.ForEach`, the ordinary arguments can determine the method type arguments before oxel checks the lambda. Supply explicit method type arguments when the ordinary arguments leave them open. [Generic callback example](examples/generic_callbacks.oxel) · [CLR generic parameter flags](https://learn.microsoft.com/en-us/dotnet/api/system.reflection.genericparameterattributes)

Call a CLR delegate with `callback(value)` or `make_callback()(value)`. Use `(button.Callback)(value)` for a delegate stored in a field or property, and `(callback)(value)` when a named function shares the binding's name. Oxel checks the arguments against the delegate's `Invoke` method. Check a nullable delegate before calling it. Try `oxel run --framework examples/delegate_calls.oxel`. [Runnable example](examples/delegate_calls.oxel)

### CLR events

Use `+=` to subscribe a delegate to a public CLR event and `-=` to unsubscribe it. A lambda can appear directly on the right-hand side, or be stored in a binding annotated with its delegate type. Keep the delegate in a binding when you need to unsubscribe it later.

```rust
let on_click: System.EventHandler = |sender, _| {
    System.Console.WriteLine("clicked");
};
button.Click += on_click;
button.Click -= on_click;
```

Write `|| { ... }` for a zero-parameter lambda expression. Event operations work through instance, interface and static members, including inherited events and events on constructed CLR types. [Observable collection example](examples/events.oxel) · [CLR event accessors](https://learn.microsoft.com/en-us/dotnet/api/system.reflection.methodsemanticsattributes)

oxel follows CLR type forwarders when resolving supported top-level types in imported signatures, generic method bounds and inheritance. Provide the forwarding assemblies and the assembly containing the definition through `--reference`, the manifest, or the resolved NuGet dependencies. The compiler checks each assembly identity and resolves the chain from metadata. CLR multi-module assemblies remain future work. [.NET type forwarding](https://learn.microsoft.com/en-us/dotnet/standard/assembly/type-forwarding)

### Imported generic CLR types

Imported public generic classes and interfaces can be named as `Namespace.Type<Arguments>` in oxel signatures, locals and fields. Invoke constructors with `new Namespace.Type<Arguments>(values)`. Constructors, methods and properties can use constructed reference types in their signatures, including nested types and signatures that use the declaring type's parameters. The compiler substitutes those parameters during type checking and emits member references with the original CLR member signature. [ECMA-335 generic member signatures](https://ecma-international.org/wp-content/uploads/ECMA-335_6th_edition_june_2012.pdf)

```sh
dotnet build examples/interop/Bridge.csproj -o target/interop
oxel run examples/interop/generic_classes.oxel --reference target/interop/Bridge.dll
```

Generic instance methods support inference from value arguments or explicit arguments such as `cell.SetAndReturn::<string>(60, "hello")`. A method declared on a generic class can use both its declaring type's parameters and its own method parameters. The compiler also supports generic virtual and interface calls through imported receivers and supported named bounds. [ECMA-335 generic methods](https://ecma-international.org/wp-content/uploads/ECMA-335_6th_edition_june_2012.pdf)

CLR signatures can annotate a type parameter use as `T?`. With a concrete reference argument such as `string`, Oxel applies the annotation as `string?`. With a concrete value argument such as `i32`, the annotated use has type `i32`. C# emits `System.Nullable<T>` as a separate generic type for value-constrained parameters. Run `oxel run --framework examples/annotated_clr_generics.oxel` to see both outcomes with `System.Array.Find`. Forwarding an open parameter into an annotated CLR position and declaring nullable open reference parameters in Oxel are future work. [Array.Find example](examples/annotated_clr_generics.oxel) · [C# generic nullability](https://learn.microsoft.com/en-us/dotnet/csharp/language-reference/builtin-types/nullable-reference-types)

Inherited members and implicit conversions follow constructed generic bases and interface chains. For example, `TextCell : Cell<string>` can supply the type argument in `Copy<T>(Cell<T> value)`, and a `Cell<string>` can convert to the `IReadCell<string>` interface it implements. Calls through interface receivers resolve inherited methods and properties. Named non-generic bounds can also give an oxel type parameter access to a constructed ancestor. [ECMA-335 generic inheritance and binding](https://ecma-international.org/wp-content/uploads/ECMA-335_6th_edition_june_2012.pdf)

Imported generic interfaces and delegates honor CLR covariance (`out`) and contravariance (`in`). In the [generic classes example](examples/interop/generic_classes.oxel), a `Cell<string>` implements `IReadCell<string>` and can be passed as `IReadCell<object>` because `IReadCell<out T>` is covariant. Contravariance lets an `IWriteCell<object>` be passed as `IWriteCell<string>` when the interface declares `in T`. Value type arguments and invariant parameters require identical types. [CLR generic variance](https://learn.microsoft.com/en-us/dotnet/standard/generics/covariance-and-contravariance)

Variance also respects nullable annotations. For example, `IReadCell<string>` can widen to `IReadCell<object?>`. The reverse nullability conversion is rejected. Mutable invariant types such as `List<string>` and `List<string?>` require matching element annotations, so a writable collection preserves its nullability guarantees.

Use `Namespace.Cell::<i32>.From(30)` to call a static method on a constructed generic class, and `Namespace.CellDefaults::<i32>.Initial` to read or assign a static property. The declaring type and a generic method take separate argument lists, as in `CellDefaults::<i64>.Choose::<bool>(30, true)`. Method type arguments can also be inferred. Each closed CLR type has its own static storage. [Static members on generic types](https://learn.microsoft.com/en-us/dotnet/csharp/language-reference/keywords/static)

Imported generic types and methods support the CLR constraints for reference types (`class`), non-nullable value types (`struct`), public parameterless construction (`new()`), and imported base classes or interfaces. A constraint can name a constructed type, such as `ICompare<T>`, or another type parameter, such as `where T : U`. The compiler substitutes both declaring-type and method type arguments before checking the constraint. An oxel function can forward a type parameter when its own declared bounds imply the imported constraint. Oxel checks imported reference-type constraints as non-nullable. C#-specific `unmanaged`, `notnull` and `class?` constraint handling is future work. [CLR generic parameter constraints](https://learn.microsoft.com/en-us/dotnet/csharp/programming-guide/generics/constraints-on-type-parameters)

The [interop example](examples/interop/main.oxel) builds a C# library and uses its constructors, properties, overloaded instance methods and interface dispatch from oxel.

```sh
dotnet build examples/interop/Bridge.csproj -o target/interop
oxel run examples/interop/main.oxel --reference target/interop/Bridge.dll
```

With references, `compile` prints the executable DLL path under `target/oxel/linked/` and places the implementation DLLs alongside it using their managed assembly names. Each distinct build has its own directory, so different versions of a dependency can coexist in the build outputs. `run` stages the DLLs together in its temporary directory. [Microsoft .NET default probing](https://learn.microsoft.com/en-us/dotnet/core/dependency-loading/default-probing)

## Native C libraries

Declare a native function with `extern "C" fn`. The string after `=` names the dynamic library. The Oxel function name is the default native symbol. Add `as "symbol_name"` to select a different symbol.

```rust
internal extern "C" fn add(left: i32, right: i32) -> i32 = "oxel_math" as "oxel_add";
```

The compiler emits a CLR P/Invoke declaration using Cdecl and exact symbol spelling. Parameters and results accept fixed-width integers and floating-point types. Functions can also return `void`. Match the C signature exactly, for example `i32` with `int32_t` and `f64` with `double`. The compiler checks Oxel argument types during `check` and `compile`. The runtime loader resolves the actual library and symbol when called. [Native calling conventions](https://learn.microsoft.com/en-us/dotnet/standard/native-interop/calling-conventions) · [Type marshalling](https://learn.microsoft.com/en-us/dotnet/standard/native-interop/type-marshaling)

A nonempty, nongeneric Oxel `struct` can also pass or return by value when every field is a supported number or another eligible struct. The CLR lays out fields in declaration order with the target's default packing. Use the corresponding field types and order with default packing on the C side. Diagnostics for incompatible nested layouts point to both the native signature and the affected field. [Sequential layout](https://learn.microsoft.com/en-us/dotnet/api/system.runtime.interopservices.structlayoutattribute) · [Native interoperability type rules](https://learn.microsoft.com/en-us/dotnet/standard/native-interop/best-practices)

Declare `ref` or `out` parameters to call C functions that take pointers to these scalar or struct types. For example, `extern "C" fn step(ref count: i32, out result: i32) = "math";` matches C parameters `int32_t *count` and `int32_t *result`. Call it with `step(&mut count, out result)`. The `&mut` argument must be an assigned, mutable local. An `out` argument may initialize a local and becomes definitely assigned after the call completes. The native function must write each `out` parameter on every normal return. The pointer is valid for the duration of the call. [Marshalling by-reference parameters](https://learn.microsoft.com/en-us/dotnet/standard/native-interop/best-practices)

Use `utf8 text: string` or `utf8 text: string?` for a by-value C `const char *` input. For example, `extern "C" fn bytes(utf8 text: string?) -> i64 = "math";` asks the CLR to marshal text as a null-terminated UTF‑8 buffer. It converts a null reference to a null pointer, so an `Option<string>` can be passed directly to the nullable form. C code reads up to the first NUL and may use the buffer during the call. [UTF‑8 string marshalling](https://learn.microsoft.com/en-us/dotnet/framework/interop/default-marshalling-for-strings)

Run the [native interop example](examples/native_interop/main.oxel) on Linux with a C compiler.

```sh
cd examples/native_interop
mkdir -p target
cc -shared -fPIC -o target/liboxel_math.so math.c
oxel run
oxel compile
binary=$(oxel publish)
"$binary"
```

The example's `[package]` sets `native_libraries = ["target/liboxel_math.so"]`. Paths resolve from the manifest directory. `compile`, `run` and `publish` copy the listed files beside the managed assembly or native executable under their original filenames. Each filename must be unique in the output directory. `check` can run before the C library is built. [Example manifest](examples/native_interop/oxel.toml)

Both executions print the same results for numeric values, structs, pointer updates and UTF‑8 string byte counts. The platform loader finds the packaged library beside the output. Keep the output directory together when deploying the program. `native_libraries` accepts prebuilt files for the target platform. [Default probing](https://learn.microsoft.com/en-us/dotnet/core/dependency-loading/default-probing) · [Native AOT library search](https://learn.microsoft.com/en-us/dotnet/core/compatibility/interop/10.0/native-library-search)

### Generate bindings from Rust

Run `oxel bindgen path/to/abi.rs --library my_library -o path/to/library.ffi.oxel` to generate Oxel source from a single Rust ABI file. The library name is the name that CoreCLR or Native AOT resolves when the generated code makes a call. Omit `-o` to write the bindings to standard output. Add the generated file to the application's `sources` and package the Rust `cdylib` with `native_libraries`.

Use top-level `pub extern "C"` functions with `#[unsafe(no_mangle)]` or `#[unsafe(export_name = "...")]`. Fixed-width numbers, native-sized numbers (`isize` as `nint`, `usize` as `nuint`), and nonempty, nongeneric `#[repr(C)]` structs with named ABI fields in the same file map to Oxel types. Use explicit Rust imports and keep the ABI type declarations in that file. Supported raw pointer fields are emitted as `nint`. Conditional declarations, macro-generated exports, packed or custom-aligned structs, and general type aliases require a separate ABI declaration.

Use Rust doc attributes on an exported function to describe how Oxel should pass a pointer or name the binding. These attributes compile with ordinary Rust. A doc comment such as `/// oxel: out result` has the same effect.

| Rust attribute | Generated binding |
| --- | --- |
| `#[doc = "oxel: ref value"]` | `ref` for a `*mut T` that points to initialized storage |
| `#[doc = "oxel: out result"]` | `out` for a `*mut T` that the Rust function initializes before returning |
| `#[doc = "oxel: utf8 text"]` | `utf8 text: string` for a NUL-terminated read-only string |
| `#[doc = "oxel: utf8_nullable text"]` | `utf8 text: string?`, where null becomes a null pointer |
| `#[doc = "oxel: buffer data count"]` | Pass a `T[]` as `*const T` followed by its element count |
| `#[doc = "oxel: buffer_mut data count"]` | Pass a writable `T[]` as `*mut T` followed by its element count |
| `#[doc = "oxel: address context"]` | Pass a thin raw pointer as `nint` |
| `#[doc = "oxel: callback handler"]` | Pass a Cdecl callback that Rust uses during this call |
| `#[doc = "oxel: callback_retained handler"]` | Pass a Cdecl callback and receive an `out` root to keep alive until Rust unregisters it |
| `#[doc = "oxel: rename name"]` | Choose an Oxel function name while preserving the Rust symbol |
| `#[doc = "oxel: skip"]` | Omit the export |

For `utf8` and `utf8_nullable`, use `*const u8`, `*const core::ffi::c_char` or `*const std::ffi::c_char`. The Rust function must finish using any of these annotated pointers before it returns. Build and run the [Rust bindgen example](examples/rust_bindgen/abi.rs) from the repository root on Linux.

```sh
mkdir -p examples/rust_bindgen/target
rustc --edition 2024 --crate-type cdylib examples/rust_bindgen/abi.rs \
  -o examples/rust_bindgen/target/liboxel_rust_math.so
oxel bindgen examples/rust_bindgen/abi.rs --library oxel_rust_math \
  -o examples/rust_bindgen/target/math.ffi.oxel
oxel run --manifest-path examples/rust_bindgen/oxel.toml
```

#### Rust buffers and callbacks

The Rust count must immediately follow its pointer and use `usize`, `u32`, or `u64`. The generator selects the corresponding Oxel declaration modifier, such as `buffer`, `buffer32`, or `buffer64`. Oxel passes the array's element count automatically. The mutable forms also expose writes to the caller. Rust must use these buffers only during the call and leave them untouched for the read-only form. An empty buffer has count zero and must never be dereferenced.

Callbacks use `System.Action` or `System.Func`, so projects using them need framework references. The generator supports Cdecl function-pointer parameters, same-file aliases for them, and `Option<...>` for nullable callbacks. Within a callback signature, thin addresses are represented as `nint`. Use a supported ABI pointee type or a qualified `core::ffi::c_void` or `std::ffi::c_void` for an opaque address. Callbacks must handle exceptions before returning across the native boundary. Calling function pointers returned by Rust is outside this interface.

A retained callback adds a managed `out` argument to the Oxel declaration. Keep this root reachable until the native side has stopped calling the function pointer. In a local scope, perform unregistration before `System.GC.KeepAlive(root)`. Use a rooted field or `GCHandle` for longer-lived registrations. [Native delegate lifetime](https://learn.microsoft.com/en-us/dotnet/api/system.runtime.interopservices.marshal.getfunctionpointerfordelegate)

The [batch example](examples/rust_bindgen/batch/main.oxel) lets Rust mutate an Oxel array and invokes a retained callback after a forced .NET collection.

```sh
mkdir -p examples/rust_bindgen/batch/target
rustc --edition 2024 --crate-type cdylib examples/rust_bindgen/batch/abi.rs \
  -o examples/rust_bindgen/batch/target/liboxel_rust_batch.so
oxel bindgen examples/rust_bindgen/batch/abi.rs --library oxel_rust_batch \
  -o examples/rust_bindgen/batch/target/batch.ffi.oxel
oxel run --manifest-path examples/rust_bindgen/batch/oxel.toml
```

## Native AOT publishing

```sh
oxel publish examples/hello.oxel
./target/oxel/native/hello
```

`publish` builds for the SDK's host runtime and prints the executable path under `target/oxel/native/`. On Windows the executable ends in `.exe`. Publishing requires a .NET 10 or newer SDK and the SDK's platform-specific Native AOT dependencies, such as a linker and development libraries on Linux. The executable can run without an installed .NET runtime on a compatible system. [Microsoft Native AOT prerequisites](https://learn.microsoft.com/en-us/dotnet/core/deploying/native-aot/)

When the SDK publishes companion runtime files, oxel places the executable and its companions in a content-addressed directory under `target/oxel/native/`. Keep that directory together when deploying the app.

The SDK consumes oxel's managed assembly directly. Use repeatable `--reference` flags for managed implementation DLLs. Referenced code must support Native AOT and trimming. SDK failures go to standard error so scripts can read the executable path from standard output.

## Editor support

Configure an editor's language client to start `oxel lsp` over stdio and associate it with `.oxel` files. Add `--reference path/to/Library.dll` to the server arguments for each managed dependency used in the editor session. The language server runs in Rust and can start without .NET. [Language Server Protocol](https://microsoft.github.io/language-server-protocol/)

The server provides live compiler diagnostics, an outline of functions, structs, enums and their members, hover information, jump to definition, find references, and completion. Pass `--manifest-path path/to/oxel.toml` or add one `--source path/to/file.oxel` argument per module input to enable cross-file analysis. For example, use `oxel lsp --source examples/project/main.oxel --source examples/project/geometry.oxel` to navigate between those two files. The configured module uses unsaved buffers for open files and disk snapshots for closed files. When the client supports file watching, the server registers the configured sources and refreshes the module after a closed file is changed, deleted, or recreated on disk. Open buffers remain authoritative until closed. Hover includes signatures for resolved CLR static calls. Find references uses compiler bindings, so it distinguishes shadowed names and can search the configured module, including closed files. Documents can be checked without a `main` declaration while editing.

Completion offers in-scope parameters, local variables, loop bindings, and match payload bindings with their types. It also offers top-level declarations and keywords. When syntax errors prevent semantic analysis, completion uses the declarations recovered by the parser and the keyword list. [LSP completion](https://github.com/microsoft/language-server-protocol/blob/gh-pages/_specifications/lsp/3.17/language/completion.md)

Signature help opens on `(` and `,` and highlights the parameter at the cursor. Calls to Oxel functions use their declarations, including across configured source files, and incomplete CLR static calls use available metadata signatures. Resolved CLR calls show the selected overload. For a CLR parameter array, additional arguments highlight the final array parameter. [LSP signature help](https://github.com/microsoft/language-server-protocol/blob/gh-pages/_specifications/lsp/3.17/language/signatureHelp.md)

Rename a source symbol from its declaration or any resolved use. The server proposes edits across the configured module, including unsaved buffers and closed files. It checks the edited program for errors and verifies that references still resolve to the same declarations. Rename requires a module without errors. The `main` entry point and imported CLR symbols are excluded. Clients that support versioned workspace edits receive the version of each open document. [LSP rename and workspace edits](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_rename)

Quick fixes offer edits for selected compiler diagnostics. They can insert a missing semicolon, make an eligible `let` binding mutable, or remove an unsupported `?` suffix on a primitive value type. The editor receives the proposed edit and applies it when selected. [LSP code actions](https://github.com/microsoft/language-server-protocol/blob/gh-pages/_specifications/lsp/3.17/language/codeAction.md)

Semantic highlighting supports whole-document and range requests. It distinguishes definitions and references for oxel and CLR symbols, including generic type parameters, and marks immutable bindings and fields as read-only. Highlighted declarations recovered by the parser remain available while a syntax error prevents full analysis. [LSP semantic tokens](https://github.com/microsoft/language-server-protocol/blob/gh-pages/_specifications/lsp/3.17/language/semanticTokens.md)

The server applies versioned UTF-16 text edits and retains the latest analysis for repeated requests. An edit reanalyzes the affected standalone document or configured module and updates diagnostics for its open files. The module reuses the syntax trees of unchanged sources while their reserved span ranges remain stable. An unusually large edit can move later ranges and require reparsing those files. An edit confined to one function body rechecks that function and reuses the other bodies' diagnostics and symbol information. Cached locations are updated when declarations move. A changed, added, or removed function signature invalidates bodies that looked up that function, including unresolved calls. Signature formatting rechecks the edited function to refresh its locations. Callers reuse their results because the parsed signature is the same. Runtime bodies track constant lookups, including unknown names. Changes to a constant's evaluated value or type invalidate its consumers, including when the change comes through another constant or a `comptime fn`. Edits that produce the same value can reuse those bodies, with references updated to the current declaration location. Type formatting and file moves that preserve the parsed declarations and their order reuse body results and update definition links. Changes to source type shapes invalidate bodies that use those types, including types reached through fields, enum payloads, or called function signatures. Unresolved type lookups are tracked so adding a declaration refreshes affected diagnostics. Definition links follow the named type and its member as declarations move. Diagnostics containing compiler-local type IDs are refreshed after type declarations change. Changes to CLR references invalidate all body results in the module. The analyzer collects declarations and checks comptime bodies and initializers on each snapshot. It reuses successful initializer results when their source, value dependencies, and looked-up types match. The cache tracks dependencies reached through a `comptime fn`, including mutually recursive calls. Evaluation errors are reported from the current snapshot.

## Workspace layout

The [`oxel-compiler`](crates/oxel-compiler/Cargo.toml) crate owns parsing, type checking, incremental analysis, CLR metadata and CIL emission. The [`oxel-cli`](crates/oxel-cli/Cargo.toml) crate owns the `oxel` command, LSP transport and .NET SDK integration. The CLI library re-exports the compiler API as `oxel::compile`, `oxel::check` and the related project APIs. To work on the compiler library independently, run `cargo test -p oxel-compiler --locked`.

| Component | Responsibility |
|---|---|
| [syntax](crates/oxel-compiler/src/syntax.rs) | Lexer, recursive-descent declarations and statements, Pratt expressions, spans and recovery at function boundaries |
| [Project](crates/oxel-compiler/src/project.rs) | Source indexing and multi-file compilation |
| [Incremental analysis](crates/oxel-compiler/src/analysis/incremental.rs) | In-memory cache for syntax trees and function-body checks |
| [HIR](crates/oxel-compiler/src/hir.rs) | Declaration collection, lexical resolution, type checking, value-type layout validation and typed function bodies |
| [Built-in types](crates/oxel-compiler/src/hir/builtin_types.rs) | Shared mapping for source aliases, CLR names and signature encodings |
| [CLR references](crates/oxel-compiler/src/clr.rs) | Managed PE metadata, assembly identity and supported external signatures |
| [Constant evaluation](crates/oxel-compiler/src/hir/constants.rs) | Dependency ordering and typed evaluation in Rust |
| [Compile-time evaluation](crates/oxel-compiler/src/hir/constants.rs) | Constant dependencies and typed HIR interpretation |
| [CIL](crates/oxel-compiler/src/cil/mod.rs) | Instruction emission, evaluation stack sizing and branch fixups |
| [Assembly](crates/oxel-compiler/src/cil/assembly.rs) | CLR metadata, stable module identity, method bodies and the managed PE entry point |
| [Compiler API](crates/oxel-cli/src/lib.rs) | `oxel::check`, `oxel::compile`, editor snapshots and shared diagnostics |
| [Language server](crates/oxel-cli/src/lsp/mod.rs) | stdio protocol, document snapshots and editor requests |
| [CLI](crates/oxel-cli/src/main.rs) | CLI dispatch, file I/O, artifact placement and CoreCLR launch |
| [Arguments](crates/oxel-cli/src/arguments.rs) | Typed usage-rs commands, help and diagnostic formatting |
| [Manifest](crates/oxel-cli/src/manifest.rs) | Spanned TOML parsing and project-relative input paths |
| [SDK](crates/oxel-cli/src/sdk.rs) | Framework and NuGet references, runtime packaging and typed SNAFU errors |
| [Native AOT](crates/oxel-cli/src/aot.rs) | Native publishing and executable placement |

When extending syntax, lower the new construct to typed HIR before implementing its CIL behavior. CLR metadata tokens are assigned in the backend. The built-in console call uses known framework signatures. External calls and property accessors resolve from the supplied assemblies without loading managed code. The backend encodes imported reference types as assembly-scoped CLR type references and emits `newobj` and `callvirt` for construction and instance calls. [ECMA-335](https://ecma-international.org/publications-and-standards/standards/ecma-335/) Both the CLI and future editor workflows can use the same analysis entry points.

Run `oxel __usage_spec__` to export the CLI's KDL specification for the Usage tool. [usage-rs](https://usage.jdx.dev/rust/) · [SNAFU](https://docs.rs/snafu/)

The backend uses the MIT-licensed `clrmeta` for ECMA-335 tables and heaps and the MIT/Apache-2.0 `object` crate for PE writing. `dotnetdll` was evaluated for its broader assembly and IL support. The project selected permissively licensed core dependencies after reviewing its GPL-3.0-or-later license. The custom CIL encoder covers the MVP opcodes and uses the platform standard. [clrmeta](https://github.com/coconutbird/clrmeta) · [object](https://github.com/gimli-rs/object) · [dotnetdll](https://github.com/nickbclifford/dotnetdll) · [ECMA-335](https://ecma-international.org/publications-and-standards/standards/ecma-335/)

## Parallel checking

The compiler checks independent runtime function bodies in parallel after collecting declarations, resolving signatures, and evaluating constants. Incremental inspection schedules only the bodies that missed the cache. The compiler uses a sequential path for batches under 128 bodies or 64 KiB of combined body text, including most single-function edits. Larger batches use a reusable Rayon thread pool with up to eight workers by default. Set `OXEL_JOBS=1` to force sequential checking or set a higher worker count to compare. The compiler reads this setting when the process first encounters a batch large enough to schedule. [Rayon](https://github.com/rayon-rs/rayon)

CIL and metadata are emitted in declaration order. Constructed type IDs are internal lookups and may be created in different orders without changing assembly bytes.

## Verification and benchmarks

Commits follow Conventional Commits. Run `mise install` for the project tools and make `cog` available on `PATH`. In a fresh clone, run `cog install-hook commit-msg` to validate messages on commit. Use `git commit -m "feat: add a compiler feature"` or `cog commit feat "add a compiler feature"`. Run `cog check` to validate the branch history. [mise](https://mise.jdx.dev/) · [Cocogitto Git hooks](https://docs.cocogitto.io/guide/git_hooks.html)

### Releases

Release-plz runs in Git-only mode. It uses the last `v<version>` tag as the baseline for both Cargo packages and includes compiler changes in the root `CHANGELOG.md`. Each release creates one Git tag and a matching forge release, such as `v0.1.0`. Run `release-plz update` on a clean branch to review version and changelog edits. With the forge and Git credentials configured, run `release-plz release --dry-run` to preview the release. Use `cargo package --workspace --locked` to verify the crate archives separately. [Release-plz Git-only configuration](https://release-plz.dev/docs/config#the-git_only-field)

The GitHub [Release workflow](.github/workflows/release.yml) runs the workspace tests and checks the Linux x86-64 bundle. A manual run builds a downloadable workflow artifact by default. Enable `publish` on the default branch to create the Git-only draft through release-plz, attach the bundle and SHA-256 checksums, and publish the draft.

```sh
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo bench --bench compiler --locked
# With the .NET SDK
OXEL_BENCH_FRAMEWORK=1 cargo bench --bench compiler --locked
OXEL_BENCH_CSHARP=1 cargo bench --bench compiler --locked
# With Go installed
OXEL_BENCH_GO=1 cargo bench --bench compiler --locked
# With the .NET Native AOT toolchain
cargo test --locked --test aot -- --ignored
OXEL_BENCH_NATIVE=1 cargo bench --bench compiler --locked
```

The tests check invalid programs, deterministic image bytes, and actual execution on CoreCLR through both `oxel run` and a separately compiled DLL. To independently verify the emitted CIL, install Microsoft's `dotnet-ilverify` for your .NET runtime and run `cargo test --locked --test cil_verification -- --ignored`. Put `ilverify` on `PATH` or set `OXEL_ILVERIFY` to its executable path. The test uses the installed runtime assemblies and verifies the example DLLs and the synthesized unit test runner. [Microsoft ILVerify](https://www.nuget.org/packages/dotnet-ilverify/)

The benchmark measures in-process check and compile for the example, a generated 500-function source, and larger numeric and generic workloads. Run it with `OXEL_JOBS=1` and `OXEL_JOBS=4` in separate processes to compare the same workloads. It also compares fresh analysis of 20 files with cached analysis after a function-body, signature, or constant-value edit. A 21-file workload measures formatting edits and changes to a field type. A separate workload edits a runtime function that uses an expensive comptime constant and then edits an unrelated type. CLI measurements include fresh compiler process startup with warm filesystem caches. The CoreCLR measurement runs the existing DLL and includes runtime startup, initial JIT and execution. The opt-in Native AOT benchmark reports publish time, executable size and the time to start and run that executable.

The opt-in framework benchmark times `check --framework` on the file line-counting example, including SDK discovery and CLR metadata loading. The opt-in Go comparison uses equivalent sources with 1,024 functions that perform integer arithmetic and branching. It launches each compiler directly with one worker and includes writing its output. Oxel emits a CLR assembly and runtime configuration. Go emits an object file using the compiler behind `go tool compile`. The comparison covers this single-file workload with warm filesystem caches.

## License

Oxel is licensed under the Apache License, Version 2.0. [License text](LICENSE)

The C# comparison uses the same 1,024-function workload and launches the installed SDK's Roslyn compiler directly with optimizations and deterministic emission enabled. Both compilations use one worker and include compiler process startup and writing the assembly, with warm filesystem caches. The Roslyn measurement includes its managed host startup. SDK reference resolution happens during setup. The execution comparison uses a shared runtime configuration with tiered compilation disabled for both languages. It times launching equivalent console programs, including their initial JIT and output. For steady-state execution, the [Oxel kernel](crates/oxel-cli/benches/managed/kernel.oxel) and [C# kernel](crates/oxel-cli/benches/managed/kernel.cs) each warm up eight times, then use `Stopwatch` to measure 32 million integer loop iterations inside the process. The benchmark checks that both kernels produce the same checksum and reports the median of five fresh processes per language. [Roslyn compiler options](https://learn.microsoft.com/en-us/dotnet/csharp/language-reference/compiler-options/code-generation) · [CoreCLR tiered compilation](https://learn.microsoft.com/en-us/dotnet/core/runtime-config/compilation)

Full generic CLR interoperability and a CoreCLR comptime worker are later work.
