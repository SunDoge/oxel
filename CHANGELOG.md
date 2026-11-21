# Changelog

## [Unreleased]

## [0.1.0](https://github.com/SunDoge/oxel/releases/tag/v0.1.0) - 2026-11-21

### Added

- replace coalescing with explicit fallback calls
- run ONNX Runtime inference from Oxel
- add async disposal, source debugging, and a SQLite example
- add the Oxel source formatter
- support Oxel in Zed
- add CLR-native source interfaces
- add CoreCLR view hot reload
- support ASP.NET Core framework references
- support generic async functions and async tests
- add Plico desktop counter experiment
- bind Rust buffers and native callbacks
- generate Oxel bindings from Rust C ABI exports
- drive the Avalonia counter with an immediate view
- support async catch blocks and lambdas
- add source namespaces and explicit use imports
- implement async functions and postfix await
- support CLR Span and ReadOnlySpan views
- show Rust-style CLR references in signature help
- add Rust-style managed reference arguments
- forward Oxel parameters to readonly CLR references
- support readonly CLR references in virtual calls
- support readonly CLR in arguments
- support ValueTask entry points
- support Result and Task entry points
- list tests without starting CoreCLR
- support lambdas in generic functions
- require .NET 10 or newer
- allow direct calls to CLR delegates
- add optional runtime casts
- support ignored unit tests
- include dedicated test sources in language server sessions
- check test modules without executing them
- support dedicated test sources in manifests
- filter unit tests by name
- run Oxel unit tests on CoreCLR
- support shorthand struct fields and bindings
- bootstrap Oxel compiler and CLR toolchain

### Fixed

- point to mismatched CLR argument modifiers
- honor CLR interfaces on builtin aliases
- validate editor renames using the project assembly kind

### Other

- move detailed guides out of the README
- publish CLI when the release PR is merged
- open release PRs automatically
- reuse SDK restore after standalone AOT edits
- reuse Native AOT publish stages and outputs
- reduce JIT script startup overhead
- reuse NuGet outputs with analyzer-only targets
- reuse NuGet dependencies after source edits
- reuse locked NuGet run bundles
- avoid temporary arrays in short string interpolation
- add a verified CLI release workflow
- reuse checked bodies during hot reload
- skip SDK packaging for framework-only execution
- lock down explicit CLR argument passing modes
- add a manual Git-only release workflow
- package the Apache license text with each crate
- use release-plz in Git-only mode
- license Oxel under Apache-2.0
- verify emitted assemblies with ILVerify
- manage workspace releases with release-plz
- move the CLI into the oxel-cli workspace crate

### Highlights

- The Rust compiler emits managed CLR executables and libraries for .NET 10 or newer. Use `oxel check`, `oxel compile` and `oxel run` for CoreCLR development, and `oxel publish` with the .NET SDK for Native AOT deployment.
- Reuse SDK intermediates for Native AOT publishing and verified native output on unchanged publishes without NuGet package dependencies.
- Skip SDK restore after standalone Native AOT source edits when the cached restore still validates.
- Entry points can return `Result<T, E>` or a CLR `Task` or `ValueTask`. The launcher waits for asynchronous completion and maps the result to the process exit status.
- Declare `async fn` to return a CLR task. Postfix `.await` suspends on incomplete `Task` and `ValueTask` values and supports chaining member calls on the awaited result.
- Await tasks inside `try` and `catch` blocks. Use `async` lambdas with CLR delegates that return `Task`, `ValueTask`, or `void`.
- Generate Oxel P/Invoke declarations from Rust C ABI exports with `oxel bindgen`, including C-layout structs and pointers annotated for `ref`, `out`, or UTF-8 strings.
- Pass Rust batch buffers as Oxel arrays with automatic element counts, and bind Cdecl callbacks with explicit lifetime handles for retained callbacks.
- Run an experimental Plico counter that sends keyed UI batches from Oxel to a Rust winit and wgpu desktop host.
- Build and run framework-only projects directly from an installed targeting pack and reuse content-addressed native libraries during repeated runs.
- Use generic `async fn` declarations, await inside struct field initializers, and run `#[test] async fn` tests through the normal test runner.
- Organize source files with explicit `namespace` declarations and import individual names or aliases with `use`. Library namespaces map to CLR namespaces.
- Use explicit `unwrap_or` and `unwrap_or_else` calls for fallbacks on `Option`, `Result`, and nullable CLR values. The old `??` syntax now reports a migration diagnostic.
- The language includes CLR-compatible primitive types and generics, immutable bindings, value types with field updates and destructuring, discriminated unions, exhaustive matching, `Option` and `Result` propagation, typed CLR exception handlers, and `using` for scoped resources.
- Use `as?` to check a CLR object's runtime type and get an `Option`. A null reference or mismatched type becomes `None`.
- CLR metadata resolution supports framework and NuGet references, local assemblies, collections, delegates and events. The compiler evaluates constants and supported `comptime fn` bodies in Rust.
- Pass managed references to CLR APIs with `&value`, `&mut value` and `out value`. The call-site spelling selects the passing mode before overloads are compared by type. Read-only `in` and `ref readonly` parameters accept initialized locals and Oxel parameters, including in calls through interfaces, virtual methods and delegates.
- Use CLR `Span<T>` and `ReadOnlySpan<T>` to index zero-copy views and pass them to supported .NET methods.
- Create CLR delegates with lambdas in ordinary or generic functions, pass them as callbacks and call them directly with `callback(value)`.
- Lower short string interpolations to fixed-arity CLR concatenation calls without a temporary string array.
- Reuse verified executable bundles for unchanged NuGet scripts with a lock file, and reuse compatible SDK references and runtime files after source edits, including projects whose package targets only filter Roslyn analyzers.
- Speed up JIT runs of NuGet projects with native SDK resolution and fewer repeated reads of cached assemblies and runtime assets.
- Annotate unit tests with `#[test]`. Use `oxel test` to execute them, `--list` to discover them, `--filter` to select tests by name, and `oxel check --tests` to check a module. Add `#[ignore]` to skip an individual test by default and use `--include-ignored` to run skipped tests. Put separate test files under `test_sources` in `oxel.toml`.
- Format project sources and tests with `oxel fmt`. Use `oxel fmt --check` to check formatting without modifying files.
- Use the standard `Result<T, E>` without declaring a result enum in each module. Existing module definitions still resolve locally.
- Use scoped `using` and disposable CLR enumerators inside async functions.
- Call ONNX Runtime directly from Oxel and use CLR APIs that accept read-only collection interfaces or unmanaged scalar type arguments. The inference example compares C# results and tests local NuGet installation.
- Run the SQLite Minimal API example to verify NuGet packaging and persistence across restarts.
- Build with `--debug` to embed source line information for CoreCLR stack traces and Zed breakpoints.
- The language server formats open documents with the same formatter as `oxel fmt`.
- The language server shares the compiler's incremental analysis and supports diagnostics, completion, signature help, navigation, rename and semantic highlighting. Use `oxel lsp --tests` to include the manifest's test files.

See the [README](README.md) for installation and runnable examples.
