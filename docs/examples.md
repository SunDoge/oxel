# Application examples

## ONNX Runtime inference example

The [ONNX inference example](../examples/onnx_inference) calls the CPU `Microsoft.ML.OnnxRuntime` NuGet package directly from Oxel. The checked-in model computes `2 * input + 1` for a three-element `f32` tensor. The script reuses one inference session and run-options object across two inputs. It passes a preallocated output array to ONNX Runtime and uses scoped `using` blocks for the session and tensors. [ONNX Runtime C# documentation](https://onnxruntime.ai/docs/get-started/with-csharp.html)

Use a .NET 10 SDK and runtime with the checked-in lock file.

```sh
oxel run --locked --manifest-path examples/onnx_inference/oxel.toml -- examples/onnx_inference/affine.onnx
dotnet run --project examples/onnx_inference/reference -- examples/onnx_inference/affine.onnx
```

Both programs print `3`, `5`, `7`, `-3`, `1`, and `9` on separate lines. The opt-in integration test compares their output, builds the Oxel inference function as a library, and runs it from a C# project that installs a locally packed NuGet package.

```sh
cargo test --locked --test onnx -- --ignored
```

Regenerate the fixture with `uv run --no-project --python 3.12 --with onnx==1.19.1 python examples/onnx_inference/generate_model.py`.

## HTTP server example

The [HTTP server](../examples/http_server.oxel) uses the .NET SDK's framework references and serves one request at a time on the IPv4 loopback address. Start it in one terminal and send requests from another.

```sh
oxel run --framework examples/http_server.oxel -- 8080
curl http://127.0.0.1:8080/health
curl 'http://127.0.0.1:8080/add?left=10&right=20'
```

The endpoints return `ok` and `30`. The server returns HTTP 400 for missing or invalid operands, 404 for an unknown path, and 405 with an `Allow` header for methods other than GET. Stop it with Ctrl+C. Use `oxel publish --framework examples/http_server.oxel` to build a native executable that takes the same port argument. On Windows, HTTP.sys may require a URL reservation for the selected prefix. [.NET HttpListener](https://learn.microsoft.com/en-us/dotnet/api/system.net.httplistener.getcontext) · [Windows URL reservations](https://learn.microsoft.com/en-us/windows/win32/http/add-urlacl)

The [ASP.NET Core example](../examples/aspnet_api) uses Kestrel and Minimal APIs on CoreCLR with the .NET 10 or newer ASP.NET Core shared runtime.

```console
oxel run --manifest-path examples/aspnet_api/oxel.toml -- --urls http://127.0.0.1:8080
curl http://127.0.0.1:8080/health
curl http://127.0.0.1:8080/add/10/20
curl -H 'Content-Type: application/json' -d '[10,20]' http://127.0.0.1:8080/sum
```

The add route binds the two named path parameters and passes the request cancellation token to the asynchronous handler. The sum route binds a JSON array from the request body. Both return `{"sum":30}` for the inputs above. Use non-capturing route lambdas with Minimal API's reflection-based binding. [ASP.NET Core Minimal APIs](https://learn.microsoft.com/en-us/aspnet/core/fundamentals/apis?view=aspnetcore-10.0)

## SQLite API example

The [SQLite service](../examples/aspnet_sqlite) uses a NuGet dependency, parameterized commands, and scoped disposal for its database connections. Kestrel binds the request cancellation token, which the handlers check before starting database work. The example uses SQLite's synchronous APIs. [Microsoft.Data.Sqlite async behavior](https://learn.microsoft.com/en-us/dotnet/standard/data/sqlite/async)

```sh
OXEL_SQLITE_PATH=items.db oxel run --locked --manifest-path examples/aspnet_sqlite/oxel.toml -- --urls http://127.0.0.1:8080
curl -X POST http://127.0.0.1:8080/items/1/30
curl http://127.0.0.1:8080/items/1
```

Both requests return `30`. Restart the service and repeat the GET request to read the same value from disk. Run the integration test with `cargo test --locked --test aspnet aspnet_sqlite -- --ignored`.

## Avalonia desktop example

The [Avalonia counter](../examples/avalonia_counter/main.oxel) uses an immediate-style view with trailing lambdas for the layout and click actions. A click updates the counter state and reruns the view. The [reconciler](../examples/avalonia_counter/immediate.oxel) reuses Avalonia controls by key within each parent, patches changed properties, and inserts, moves, or removes children when the structure changes. Avalonia provides input handling and draws the controls.

```sh
oxel run --manifest-path examples/avalonia_counter/oxel.toml
oxel publish --manifest-path examples/avalonia_counter/oxel.toml
oxel test --manifest-path examples/avalonia_counter/oxel.toml
```

Run the app in a graphical desktop session. The tests exercise the keyed diff and dispatch Avalonia click events without opening a window.

The manifest uses the Avalonia Desktop and Fluent Theme packages. To run the Native AOT build, launch the executable printed by `publish` from its bundle. Keep the bundle together when moving it to a compatible machine. [Code-only Avalonia UI](https://docs.avaloniaui.net/docs/fundamentals/coded-ui)

## Plico desktop experiment

The [Plico counter view](../examples/plico_counter/view.oxel) uses the same immediate-style view and trailing lambdas with a Rust window host. The Oxel host holds the counter state and passes activated button keys to the view. It submits a command array and a UTF-8 buffer together. The Rust host copies the accepted batch and declares its keyed nodes through Plico, which handles reconciliation, layout, input, and drawing. Plico's desktop host provides the winit event loop and wgpu renderer.

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

## Hot reload on CoreCLR

After building the Plico host and generating its bindings, start the development host.

```sh
oxel dev --manifest-path examples/plico_counter/dev.toml \
  --reload-manifest examples/plico_counter/view.toml
```

Raise the count, then edit the label or increment logic in [view.oxel](../examples/plico_counter/view.oxel) and save. The view updates in the existing window and keeps the count. Compiler diagnostics appear in the terminal while the last usable view stays active. Closing the window or pressing Ctrl+C stops the development session.

`oxel dev` watches the sources in a `kind = "library"` reload manifest. It retains parsed files, resolved references, and checked function bodies in memory. A body-only edit rechecks changed functions. Changes to imports, types, signatures, or compile-time inputs invalidate the module's checked bodies. Each successful build emits a complete replacement assembly.

The host receives `OXEL_RELOAD_DIRECTORY`. Its `current` file names the latest generation number, and `<generation>.dll` contains that assembly. The [example loader](../examples/hot_reload/loader.oxel) binds a public function to a CLR delegate in a collectible load context. The Plico window wakes when the generation changes, and the [development host](../examples/plico_counter/dev.oxel) switches delegates on the UI thread after accepting the new view's batch. Keep persistent state and shared dependencies in the host. Restart the session to pick up host, manifest, dependency or native library changes. Use the regular Plico manifest for Native AOT publishing.
