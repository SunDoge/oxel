# Oxel

Oxel is a statically typed language for .NET with Rust-inspired syntax. Its Rust compiler emits CIL for CoreCLR and supports Native AOT publishing through the .NET SDK.

## Quick start

Install Rust 1.91 or later and the .NET 10 SDK. From the repository root, run the example:

```sh
cargo run --locked -- run examples/hello.oxel
```

The example prints `30`. To install the CLI locally, run `cargo install --path crates/oxel-cli --locked`.

```rust
fn add(a: i32, b: i32) -> i32 {
    a + b
}

fn main() {
    let result = add(10, 20);
    System.Console.WriteLine(result);
}
```

## Documentation

- [CLI guide](docs/cli.md) covers checking, compiling, running, formatting, testing, diagnostics, and Native AOT.
- [Language reference](docs/language.md) covers types, functions, pattern matching, and compile-time evaluation.
- [Projects and NuGet](docs/projects.md) covers modules, manifests, managed libraries, and dependency locking.
- [CLR and native interop](docs/interop.md) covers .NET assemblies, delegates, P/Invoke, and Rust bindings.
- [Application examples](docs/examples.md) covers ONNX Runtime, web servers, desktop apps, and hot reload.
- [Editor setup](docs/editor.md) covers Zed, LSP, and debugging.
- [Compiler development](docs/development.md) covers architecture, tests, benchmarks, and releases.

## License

[Apache License 2.0](LICENSE)
