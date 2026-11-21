# Compiler development

## Workspace layout

The [`oxel-compiler`](../crates/oxel-compiler/Cargo.toml) crate owns parsing, type checking, incremental analysis, CLR metadata and CIL emission. The [`oxel-cli`](../crates/oxel-cli/Cargo.toml) crate owns the `oxel` command, LSP transport and .NET SDK integration. The CLI library re-exports the compiler API as `oxel::compile`, `oxel::check` and the related project APIs. To work on the compiler library independently, run `cargo test -p oxel-compiler --locked`.

| Component | Responsibility |
|---|---|
| [syntax](../crates/oxel-compiler/src/syntax.rs) | Lexer, recursive-descent declarations and statements, Pratt expressions, spans and recovery at function boundaries |
| [Project](../crates/oxel-compiler/src/project.rs) | Source indexing and multi-file compilation |
| [Incremental analysis](../crates/oxel-compiler/src/analysis/incremental.rs) | In-memory cache for syntax trees and function-body checks |
| [HIR](../crates/oxel-compiler/src/hir.rs) | Declaration collection, lexical resolution, type checking, value-type layout validation and typed function bodies |
| [Built-in types](../crates/oxel-compiler/src/hir/builtin_types.rs) | Shared mapping for source aliases, CLR names and signature encodings |
| [CLR references](../crates/oxel-compiler/src/clr.rs) | Managed PE metadata, assembly identity and supported external signatures |
| [Constant evaluation](../crates/oxel-compiler/src/hir/constants.rs) | Dependency ordering and typed evaluation in Rust |
| [Compile-time evaluation](../crates/oxel-compiler/src/hir/constants.rs) | Constant dependencies and typed HIR interpretation |
| [CIL](../crates/oxel-compiler/src/cil/mod.rs) | Instruction emission, evaluation stack sizing and branch fixups |
| [Assembly](../crates/oxel-compiler/src/cil/assembly.rs) | CLR metadata, stable module identity, method bodies and the managed PE entry point |
| [Compiler API](../crates/oxel-cli/src/lib.rs) | `oxel::check`, `oxel::compile`, editor snapshots and shared diagnostics |
| [Language server](../crates/oxel-cli/src/lsp/mod.rs) | stdio protocol, document snapshots and editor requests |
| [CLI](../crates/oxel-cli/src/main.rs) | CLI dispatch, file I/O, artifact placement and CoreCLR launch |
| [Arguments](../crates/oxel-cli/src/arguments.rs) | Typed usage-rs commands, help and diagnostic formatting |
| [Manifest](../crates/oxel-cli/src/manifest.rs) | Spanned TOML parsing and project-relative input paths |
| [SDK](../crates/oxel-cli/src/sdk.rs) | Framework and NuGet references, runtime packaging and typed SNAFU errors |
| [Native AOT](../crates/oxel-cli/src/aot.rs) | Native publishing and executable placement |

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

The [Release workflow](../.github/workflows/release.yml) opens or updates a release PR on pushes to `main`. Merge the release PR to test the workspace, build and verify the Linux x86-64 CLI bundle, and publish it with its SHA-256 checksum on GitHub Releases. Enable **Allow GitHub Actions to create and approve pull requests** under **Settings → Actions → General** so the workflow's `GITHUB_TOKEN` can create the PR. [Release-plz GitHub setup](https://release-plz.dev/docs/github/quickstart)

To create and test the same bundle locally, use Linux x86-64 with Rust 1.91.0, the .NET 10 runtime, and `jq`.

```sh
RUSTUP_TOOLCHAIN=1.91.0 bash scripts/package-cli.sh
```

The archive and checksum are written to `target/preview`. Pass an output directory as the first argument to choose another location.

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

The benchmark measures in-process check and compile for the example, a generated 500-function source, and larger numeric and generic workloads. Run it with `OXEL_JOBS=1` and `OXEL_JOBS=4` in separate processes to compare the same workloads. It also compares fresh analysis of 20 files with cached analysis after a function-body, signature, or constant-value edit. A 21-file workload measures formatting edits and changes to a field type. A separate workload edits a runtime function that uses an expensive comptime constant and then edits an unrelated type. CLI measurements include fresh compiler process startup with warm filesystem caches. The CoreCLR measurement runs the existing DLL and includes runtime startup, initial JIT and execution. The opt-in Native AOT benchmark reports first publish, unchanged republish and republish after a source edit, plus executable size and startup time.

The 20-file workload also compares full compilation and incremental emission after editing one method. To measure from save until the changed program prints its result, run the edit-loop benchmark on Linux or macOS with Python, Go, and the .NET 10 SDK.

```sh
cargo build --release --locked -p oxel-cli
python3 scripts/benchmark-edit.py
```

The script generates 1,024 equivalent functions across 32 files, warms up each workflow, and reports the median and p90 over 11 distinct edits by default. It verifies that `oxel dev` and `dotnet watch` keep their application process alive. `oxel run` and `go run` rebuild and launch an executable on each edit. Pass `--previous-oxel /path/to/oxel` to include an older release binary with the `dev` command.

The opt-in framework benchmark times `check --framework` on the file line-counting example, including SDK discovery and CLR metadata loading. The opt-in Go comparison uses equivalent sources with 1,024 functions that perform integer arithmetic and branching. It launches each compiler directly with one worker and includes writing its output. Oxel emits a CLR assembly and runtime configuration. Go emits an object file using the compiler behind `go tool compile`. The comparison covers this single-file workload with warm filesystem caches.

Use `OXEL_BENCH_INTERPOLATION=1 cargo bench --bench compiler --locked` to compare short string interpolation in Oxel and C# under the same CoreCLR. Set `OXEL_BENCH_PREVIOUS_CLI=/path/to/old/oxel` to include a previous Oxel compiler. The benchmark checks the result and reports median time and managed bytes allocated per interpolation over seven warmed-up processes per compiler. Oxel uses fixed-arity CLR `String.Concat` overloads for up to four parts, removing the temporary string array.

The C# comparison uses the same 1,024-function workload and launches the installed SDK's Roslyn compiler directly with optimizations and deterministic emission enabled. Both compilations use one worker and include compiler process startup and writing the assembly, with warm filesystem caches. The Roslyn measurement includes its managed host startup. SDK reference resolution happens during setup. The execution comparison uses a shared runtime configuration with tiered compilation disabled for both languages. It times launching equivalent console programs, including their initial JIT and output. For steady-state execution, the [Oxel kernel](../crates/oxel-cli/benches/managed/kernel.oxel) and [C# kernel](../crates/oxel-cli/benches/managed/kernel.cs) each warm up eight times, then use `Stopwatch` to measure 32 million integer loop iterations inside the process. The benchmark checks that both kernels produce the same checksum and reports the median of five fresh processes per language. [Roslyn compiler options](https://learn.microsoft.com/en-us/dotnet/csharp/language-reference/compiler-options/code-generation) · [CoreCLR tiered compilation](https://learn.microsoft.com/en-us/dotnet/core/runtime-config/compilation)
