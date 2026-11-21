# Editor setup and debugging

## Zed

The [Oxel extension](../editors/zed) recognizes `.oxel` files and provides Tree-sitter highlighting, an outline, bracket matching, and auto-indentation. It starts the existing `oxel lsp` server for diagnostics, navigation, completion, and code actions.

To use the extension from this checkout, put `oxel` on your `PATH` and stage a local extension.

```sh
cargo install --path crates/oxel-cli --locked
python3 editors/zed/prepare-local.py
```

In Zed, run `zed: install dev extension` and select the path printed by the script. The script points Zed at the committed grammar in the local Git checkout. Dev-extension builds use the rustup `wasm32-wasip2` target and the WASI SDK. [Zed dev extensions](https://zed.dev/docs/extensions/developing-extensions)

Open the directory containing `oxel.toml` to use its sources, test sources, and references. Open a standalone `.oxel` file for single-file analysis. To select a checkout binary, set both `path` and `arguments` in Zed settings. Include `lsp` as the first argument, and omit the manifest flags for standalone files. With the default `PATH` lookup, the extension selects the manifest automatically. [Zed language server settings](https://zed.dev/docs/configuring-languages)

```json
{
  "lsp": {
    "oxel-lsp": {
      "binary": {
        "path": "/absolute/path/to/oxel",
        "arguments": ["lsp", "--manifest-path", "path/to/oxel.toml", "--tests"]
      }
    }
  }
}
```

## Source debugging

Use `oxel run --debug` to include Oxel filenames and line numbers in CLR exception traces. Use `oxel compile --debug` for a managed DLL with embedded Portable PDB symbols. A debug build records the absolute paths of its inputs and disables JIT optimizations. It maps emitted IL to source lines and supports line breakpoints. Oxel local-variable names and scopes require additional debug metadata. [Portable PDB format](https://github.com/dotnet/runtime/blob/main/docs/design/specs/PortablePdb-Metadata.md)

With the Oxel Zed extension installed and `netcoredbg` on `PATH`, set a gutter breakpoint and run `debugger: start`. Add a launch configuration to `.zed/debug.json` and use the DLL path printed by `oxel compile --debug`. [Zed debugger](https://zed.dev/docs/debugger) · [NetCoreDbg releases](https://github.com/Samsung/netcoredbg/releases)

```json
[
  {
    "label": "Debug Oxel",
    "adapter": "oxel",
    "request": "launch",
    "program": "/absolute/path/to/target/oxel/hello.dll",
    "cwd": "/absolute/path/to/project"
  }
]
```

To regenerate and test the editor grammar, run `npx --yes tree-sitter-cli@0.25.10 generate` and `npx --yes tree-sitter-cli@0.25.10 test` from `editors/tree-sitter-oxel`.

## Other LSP clients

Configure an editor's language client to start `oxel lsp` over stdio and associate it with `.oxel` files. Add `--reference path/to/Library.dll` to the server arguments for each managed dependency used in the editor session. The language server runs in Rust and can start without .NET. [Language Server Protocol](https://microsoft.github.io/language-server-protocol/)

The server provides live compiler diagnostics, an outline of functions, structs, enums and their members, hover information, jump to definition, find references, and completion. Pass `--manifest-path path/to/oxel.toml` or add one `--source path/to/file.oxel` argument per module input to enable cross-file analysis. For example, use `oxel lsp --source examples/project/main.oxel --source examples/project/geometry.oxel` to navigate between those two files. The configured module uses unsaved buffers for open files and disk snapshots for closed files. When the client supports file watching, the server registers the configured sources and refreshes the module after a closed file is changed, deleted, or recreated on disk. Open buffers remain authoritative until closed. Hover includes signatures for resolved CLR static calls. Find references uses compiler bindings, so it distinguishes shadowed names and can search the configured module, including closed files. Documents can be checked without a `main` declaration while editing.

Completion offers in-scope parameters, local variables, loop bindings, and match payload bindings with their types. It also offers top-level declarations and keywords. When syntax errors prevent semantic analysis, completion uses the declarations recovered by the parser and the keyword list. [LSP completion](https://github.com/microsoft/language-server-protocol/blob/gh-pages/_specifications/lsp/3.17/language/completion.md)

Signature help opens on `(` and `,` and highlights the parameter at the cursor. Calls to Oxel functions use their declarations, including across configured source files, and incomplete CLR static calls use available metadata signatures. Resolved CLR calls show the selected overload. For a CLR parameter array, additional arguments highlight the final array parameter. [LSP signature help](https://github.com/microsoft/language-server-protocol/blob/gh-pages/_specifications/lsp/3.17/language/signatureHelp.md)

Rename a source symbol from its declaration or any resolved use. The server proposes edits across the configured module, including unsaved buffers and closed files. It checks the edited program for errors and verifies that references still resolve to the same declarations. Rename requires a module without errors. The `main` entry point and imported CLR symbols are excluded. Clients that support versioned workspace edits receive the version of each open document. [LSP rename and workspace edits](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_rename)

Quick fixes offer edits for selected compiler diagnostics. They can insert a missing semicolon, make an eligible `let` binding mutable, or remove an unsupported `?` suffix on a primitive value type. The editor receives the proposed edit and applies it when selected. [LSP code actions](https://github.com/microsoft/language-server-protocol/blob/gh-pages/_specifications/lsp/3.17/language/codeAction.md)

Semantic highlighting supports whole-document and range requests. It distinguishes definitions and references for oxel and CLR symbols, including generic type parameters, and marks immutable bindings and fields as read-only. Highlighted declarations recovered by the parser remain available while a syntax error prevents full analysis. [LSP semantic tokens](https://github.com/microsoft/language-server-protocol/blob/gh-pages/_specifications/lsp/3.17/language/semanticTokens.md)

The server applies versioned UTF-16 text edits and retains the latest analysis for repeated requests. An edit reanalyzes the affected standalone document or configured module and updates diagnostics for its open files. The module reuses the syntax trees of unchanged sources while their reserved span ranges remain stable. An unusually large edit can move later ranges and require reparsing those files. An edit confined to one function body rechecks that function and reuses the other bodies' diagnostics and symbol information. Cached locations are updated when declarations move. A changed, added, or removed function signature invalidates bodies that looked up that function, including unresolved calls. Signature formatting rechecks the edited function to refresh its locations. Callers reuse their results because the parsed signature is the same. Runtime bodies track constant lookups, including unknown names. Changes to a constant's evaluated value or type invalidate its consumers, including when the change comes through another constant or a `comptime fn`. Edits that produce the same value can reuse those bodies, with references updated to the current declaration location. Type formatting and file moves that preserve the parsed declarations and their order reuse body results and update definition links. Changes to source type shapes invalidate bodies that use those types, including types reached through fields, enum payloads, or called function signatures. Unresolved type lookups are tracked so adding a declaration refreshes affected diagnostics. Definition links follow the named type and its member as declarations move. Diagnostics containing compiler-local type IDs are refreshed after type declarations change. Changes to CLR references invalidate all body results in the module. The analyzer collects declarations and checks comptime bodies and initializers on each snapshot. It reuses successful initializer results when their source, value dependencies, and looked-up types match. The cache tracks dependencies reached through a `comptime fn`, including mutually recursive calls. Evaluation errors are reported from the current snapshot.
