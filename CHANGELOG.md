# Changelog

## 0.1.0

- The Rust compiler emits managed CLR executables and libraries. Use `oxel check`, `oxel compile` and `oxel run` for CoreCLR development, and `oxel publish` with the .NET SDK for Native AOT deployment.
- The language includes CLR-compatible primitive types and generics, immutable bindings, value types with field updates and destructuring, discriminated unions, exhaustive matching, `Option` and `Result` propagation, typed CLR exception handlers, and `using` for scoped resources.
- CLR metadata resolution supports framework and NuGet references, local assemblies, collections, delegates and events. The compiler evaluates constants and supported `comptime fn` bodies in Rust.
- Annotate unit tests with `#[test]`. Use `oxel test` to execute them, `--filter` to select tests by name, and `oxel check --tests` to check a module. Put separate test files under `test_sources` in `oxel.toml`.
- The language server shares the compiler's incremental analysis and supports diagnostics, completion, signature help, navigation, rename and semantic highlighting. Use `oxel lsp --tests` to include the manifest's test files.

See the [README](README.md) for installation and runnable examples.
