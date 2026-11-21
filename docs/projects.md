# Projects and NuGet

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

Run `oxel run --manifest-path examples/namespaces/oxel.toml` for an example. [Namespace example](../examples/namespaces/main.oxel)

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

Run `check`, `compile`, `run` or `publish` without source arguments from the manifest directory to use its `oxel.toml`. Compiled and published artifacts use the package name and go under that project’s `target/oxel/`. A published package selects the .NET SDK from the project directory, and `run` preserves the caller’s working directory. The language server reads the manifest at startup. Explicit source files are also supported as a separate invocation. [Example manifest](../examples/project/oxel.toml)

### Managed libraries

Set `kind = "library"` under `[package]` to build a managed CLR library. The package name becomes its CLR namespace and must use dot-separated Oxel identifiers, such as `Samples.Math`. Oxel exports runtime functions on `Samples.Math.Program` and declared types under `Samples.Math` by default. A library can have a function named `main` with any supported signature.

A source namespace appends to the package namespace. For example, `namespace Geometry;` in `Samples.Math` emits types under `Samples.Math.Geometry` and functions on `Samples.Math.Geometry.Program`.

```sh
oxel check --manifest-path examples/math_library/oxel.toml
oxel compile --manifest-path examples/math_library/oxel.toml
oxel test --manifest-path examples/math_library/oxel.toml
```

The [example manifest](../examples/math_library/oxel.toml) writes `target/oxel/lib/Samples.Math.dll` inside the project directory. Add that DLL as a .NET assembly reference and call `Samples.Math.Program.add(10, 20)` from C# or Oxel. For an Oxel consumer, pass the path using `--reference`. With SDK or NuGet references, use the path printed by `compile` and provide the library's runtime dependencies to the consuming project. The executable host selects the runtime and can be published through Native AOT.

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

NuGet projects use the SDK package cache when restoring dependencies, including during `check` and LSP startup. Transitive versions are resolved at restore time when no lock file is selected. Local assembly references and single-file compilation continue to work without a .NET SDK. [Runnable NuGet example](../examples/nuget/oxel.toml)

When the selected SDK's NuGet lock file exists, `oxel run` can reuse a verified executable bundle under `target/oxel/run-cache`. Unchanged builds start the existing DLL. After a source edit, Oxel can compile a new DLL against the cached CLR references and stage it beside the existing runtime assets. Recognized filters that only remove Roslyn analyzers also allow reuse, because the SDK packages an assembly already compiled by Oxel. Package build targets that can transform the application assembly or generate other outputs use the full SDK build. Changes to the manifest, lock, local CLR or native libraries, compiler, SDK selection, build environment or NuGet configuration invalidate the dependency snapshot. Runtime arguments can change between cached runs. Keep cached bundles and their shared assets read-only. Remove `target/oxel/run-cache` when no scripts are running to reclaim cached bundles.

On Linux and macOS, JIT runs of NuGet projects query the SDK through its native host when Oxel can locate the selected installation. The resolver honors `global.json`, and other installations use the CLI's SDK query. During source edits, the compiler checks types from the same reference bytes used to verify the cache. The updated application DLL gets its own digest, and unchanged runtime assets keep their verified digest.

### NuGet lock files

```sh
oxel restore --manifest-path examples/nuget/oxel.toml
oxel check --locked --manifest-path examples/nuget/oxel.toml
```

`restore` writes the SDK's package graph beside the manifest and prints the lock file path. For example, `oxel.toml` targeting .NET 10 produces `oxel.toml.nuget.net10.0.lock.json`. The file records direct and transitive package versions and package content hashes. Commit it with the manifest to reuse the resolved graph. The SDK target framework follows the selected SDK, which can be pinned with `global.json`. [NuGet lock files](https://learn.microsoft.com/en-us/nuget/consume-packages/package-references-in-project-files#locking-dependencies)

Build, run, check, publish and language server commands enforce an existing lock file for their target. Pass `--locked` to require that file for a project with NuGet dependencies. Use `oxel restore` to refresh the graph after changing dependencies, or `oxel restore --locked` to verify it without compiling. A failed refresh leaves the previous lock file in place.

Use `oxel restore --native` to generate the separate Native AOT lock file for the SDK's host runtime identifier. For example, `oxel.toml.nuget.net10.0.linux-x64.aot.lock.json` is used by `oxel publish --locked` on that host and target framework.
