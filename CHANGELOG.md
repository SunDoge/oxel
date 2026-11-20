# Changelog

## 0.1.0

- The Rust compiler emits managed CLR executables and libraries for .NET 10 or newer. Use `oxel check`, `oxel compile` and `oxel run` for CoreCLR development, and `oxel publish` with the .NET SDK for Native AOT deployment.
- Entry points can return `Result<T, E>` or a CLR `Task` or `ValueTask`. The launcher waits for asynchronous completion and maps the result to the process exit status.
- The language includes CLR-compatible primitive types and generics, immutable bindings, value types with field updates and destructuring, discriminated unions, exhaustive matching, `Option` and `Result` propagation, typed CLR exception handlers, and `using` for scoped resources.
- Use `as?` to check a CLR object's runtime type and get an `Option`. A null reference or mismatched type becomes `None`.
- CLR metadata resolution supports framework and NuGet references, local assemblies, collections, delegates and events. The compiler evaluates constants and supported `comptime fn` bodies in Rust.
- Pass managed references to CLR APIs with `&value`, `&mut value` and `out value`. The call-site spelling selects the passing mode before overloads are compared by type. Read-only `in` and `ref readonly` parameters accept initialized locals and Oxel parameters, including in calls through interfaces, virtual methods and delegates.
- Create CLR delegates with lambdas in ordinary or generic functions, pass them as callbacks and call them directly with `callback(value)`.
- Annotate unit tests with `#[test]`. Use `oxel test` to execute them, `--list` to discover them, `--filter` to select tests by name, and `oxel check --tests` to check a module. Add `#[ignore]` to skip an individual test by default and use `--include-ignored` to run skipped tests. Put separate test files under `test_sources` in `oxel.toml`.
- The language server shares the compiler's incremental analysis and supports diagnostics, completion, signature help, navigation, rename and semantic highlighting. Use `oxel lsp --tests` to include the manifest's test files.

See the [README](README.md) for installation and runnable examples.
