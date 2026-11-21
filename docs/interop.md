# CLR and native interoperability

## CLR references

### .NET framework references

Use `--framework` to call supported APIs from the .NET framework selected by the installed SDK. The SDK resolves its `Microsoft.NETCore.App` targeting pack, and oxel reads its metadata in Rust. The output uses the corresponding target framework and loads the implementations provided by .NET at runtime. [Reference assemblies](https://learn.microsoft.com/en-us/dotnet/standard/assembly/reference-assemblies)

```sh
oxel run --framework examples/file_lines.oxel -- README.md
```

The option also works with `check`, `compile`, `publish`, and `lsp`. Set `framework = true` under `[package]` in `oxel.toml` to enable it for a project. Set the SDK version with `global.json` in the working directory, or in the manifest directory for manifest builds. Local CLR references and NuGet packages can be used alongside the framework APIs. [File line-counting example](../examples/file_lines.oxel)

Set `framework_references = ["Microsoft.AspNetCore.App"]` under `[package]` to use ASP.NET Core APIs. This also enables `Microsoft.NETCore.App`, which can be listed explicitly as an alternative to `framework = true`. Framework-dependent executables require the shared runtimes declared in their `.runtimeconfig.json`. [ASP.NET Core example](../examples/aspnet_api/oxel.toml) · [.NET framework references](https://learn.microsoft.com/en-us/dotnet/core/tools/dependencies)

For projects without NuGet dependencies, `check` and LSP can load framework references directly from the SDK's targeting-pack manifests when a single stable SDK is installed. In that setup, `compile`, `run`, and `test` also stage managed outputs directly when there are no local CLR references. They use the installed pack's target framework, minimum runtime version, and application dependency manifest. Projects with `global.json`, multiple SDKs, or custom SDK settings use the SDK path. [SDK selection](https://learn.microsoft.com/en-us/dotnet/core/versions/selection)

Direct framework execution reuses native library snapshots by content from `target/oxel/native-cache`. Each run stages its DLL beside the matching native assets, using hard links when available. A change to a native library creates a new snapshot.

With framework references enabled, aliases can call the public CLR instance methods whose signatures oxel supports. For example, `" oxel ".Trim()` returns `"oxel"` and `(30).ToString("D3")` returns `"030"`. Numeric, Boolean and character methods receive a managed address. String and object calls use reference dispatch. Nullable receivers must be narrowed before a call. [CLR instance calls](https://learn.microsoft.com/dotnet/api/system.reflection.emit.opcodes.call)

Aliases also use the interfaces declared by their CLR types. An `i32` satisfies `T: System.IFormattable`, and assigning it to an interface boxes it. Oxel can infer `T` from the implemented `System.IComparable<T>` interface when a call receives an `i32` or `string`. [Runnable example](../examples/alias_interfaces.oxel)

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

Top-level public CLR enums with signed or unsigned 8-, 16-, 32-, or 64-bit storage keep their CLR type in signatures, fields, arrays, generic arguments and boxes. Use qualified members such as `System.StringComparison.OrdinalIgnoreCase` and compare values of the same enum with `==` or `!=`. CLR base and interface conversions box the value. [Enum comparison example](../examples/enum_interop.oxel) · [ECMA-335 enum semantics](https://ecma-international.org/wp-content/uploads/ECMA-335_6th_edition_june_2012.pdf)

Use `as` to convert explicitly between a CLR enum and a numeric type, `char`, or another CLR enum. Oxel checks the conversion against each enum's underlying integer type. Any in-range value is allowed, including flag combinations without a named member. A direct cast to a `u64`-backed enum accepts literals up to `u64`'s maximum. These conversions work in constant evaluation, where overflow produces a diagnostic. [CLR flags example](../examples/bitwise.oxel) · [Enumeration conversion rules](https://learn.microsoft.com/en-us/dotnet/csharp/language-reference/language-specification/conversions#1033-explicit-enumeration-conversions)

Public, top-level CLR structs support public constructors and members, by-value parameters and returns, fields, boxing, and generic arguments. Generic structs such as `System.Collections.Generic.KeyValuePair<string, i32>` preserve CLR type arguments and constraints. Use a `let mut` local as the receiver to persist mutations or assign a field, property, or indexer. With a concrete CLR struct receiver, calls through immutable bindings and temporary values operate on a copy. Build arrays of imported structs from explicit values such as `[start, finish]` so each element has an initializer. Ref-like structs remain outside this subset. [Date range example](../examples/date_ranges.oxel) · [Key-value pair example](../examples/key_value_pairs.oxel) · [CLR struct value semantics](https://learn.microsoft.com/en-us/dotnet/csharp/language-reference/language-specification/structs)

Property access covers properties with supported types and ordinary public getters or setters. Imported CLR default indexed properties use `value[index]` or `value[first, second]`. Index argument types select an overload. Reads require a public getter, writes require a public setter, and compound assignments need both. A compound assignment evaluates the receiver and each index once, then reads the old value, evaluates the right-hand side, and writes the result. Instance indexers work through constructed generic classes and interface constraints. Ref-returning and init-only accessors are outside the supported subset. [ECMA-335 Property and MethodSemantics tables](https://ecma-international.org/wp-content/uploads/ECMA-335_6th_edition_june_2012.pdf)

The [indexer example](../examples/interop/indexers.oxel) prints `30`.

```sh
dotnet build examples/interop/Bridge.csproj -o target/interop
oxel run examples/interop/indexers.oxel --reference target/interop/Bridge.dll
```

Constructor, static method and static property lookup use the named declaring type. Imported public static generic methods on non-generic types can infer type arguments from their arguments or accept explicit arguments such as `Namespace.Type.Identity::<i32>(30)`. When otherwise tied overloads have identical instantiated parameter types, the non-generic overload takes precedence. Imported method signatures can use supported concrete types, method type parameters and constructed CLR reference types. For example, a call to C# `Copy<T>(Cell<T> value)` can infer `T` from an argument of type `Cell<i32>`.

Calls to CLR methods and constructors can omit trailing positional parameters marked optional in metadata when their defaults are supported CLI constants. The compiler emits the defaults at the call site, including primitive and enum values, strings, null references and zero-initialized value types. Among otherwise tied overloads, a call with no omitted parameters takes precedence. Pass an argument explicitly for defaults stored only in custom attributes. For parameters with caller-info attributes, oxel uses the declared default constant. [Runnable example](../examples/optional_arguments.oxel) · [C# optional arguments](https://learn.microsoft.com/en-us/dotnet/csharp/programming-guide/classes-and-structs/named-and-optional-arguments)

For CLR methods and constructors declared with `params T[]`, pass a compatible array directly or pass zero or more trailing values. Oxel evaluates the trailing expressions from left to right and builds a CLR vector for the call. Generic methods can infer the element type from the supplied values. When there are no values and no other arguments to infer from, specify the type argument explicitly. A compatible explicit array keeps its identity, and a fixed-arity overload wins when its argument types otherwise tie with an expanded overload. [Runnable example](../examples/parameter_arrays.oxel) · [CLR parameter arrays](https://learn.microsoft.com/en-us/dotnet/api/system.paramarrayattribute)

The call-site spelling determines the passing mode. Oxel only considers CLR overloads whose parameter modes match the call before it compares parameter types and conversions. For example, `f(x)` requires a by-value overload, so calling a method that only has an `in` parameter requires `f(&x)`.

| Call | CLR parameter | Requirement |
| --- | --- | --- |
| `f(x)` | By value | An expression of a compatible type |
| `f(&x)` | `in` or `ref readonly` | An initialized local, function parameter, or lambda parameter |
| `f(&mut x)` | `ref` | An initialized, mutable local |
| `f(out x)` | `out` | A mutable local or a delayed `let` that the call initializes |

When passing a CLR object binding with `&mut`, both the local and the CLR `ref` parameter must be nullable. An `out` local becomes definitely assigned on normal return, even when the method returns `false`. To write it again, declare it with `let mut`. [TryParse and Interlocked example](../examples/byref_arguments.oxel) · [C# reference parameters](https://learn.microsoft.com/en-us/dotnet/csharp/language-reference/keywords/method-parameters)

Managed-reference arguments work with imported methods, constructors, and delegate calls, including calls through interfaces and virtual dispatch. The storage type must match the CLR parameter type, even if a null check has narrowed the type for ordinary reads. `&value` and `&mut value` are call-site forms that follow CLR aliasing rules. The spellings `in value` and `ref value` also work.

### Span views

With `--framework`, use `System.Span<T>` for a writable view and `System.ReadOnlySpan<T>` for a read-only view. Create a view with a CLR constructor or `System.MemoryExtensions.AsSpan`, then use `.Slice(start, length)` for a subview and `[index]` to access an element. Views share the underlying storage, and mutations through a writable view are visible through other views of that storage. Pass a view by value to an Oxel or CLR function that accepts its type. [Runnable span example](../examples/slices.oxel) · [.NET span guidelines](https://learn.microsoft.com/en-us/dotnet/standard/memory-and-spans/memory-t-usage-guidelines)

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

Here `OnClick` is a CLR method accepting an `EventHandler`. The lambda captures the immutable `button` binding, so the callback can still update the referenced object after the registration function returns. A nested lambda can capture parameters and bindings from any enclosing scope. A lambda inside a generic function can use the enclosing type parameters and their CLR constraints. For example, `fn remember<T>(value: T) -> System.Func<T> { || { value } }` returns a delegate that captures `value`. [Runnable threading example](../examples/trailing_lambda.oxel) · [Generic lambda example](../examples/generic_lambdas.oxel) · [CLR delegate binding](https://learn.microsoft.com/en-us/dotnet/api/system.delegate.createdelegate)

Capturing a `let mut` binding gives the declaring scope and all its callbacks shared storage for that variable. Initialize it before creating a callback that captures it. Each execution of the declaration creates its own storage, including declarations inside loops. Captured mutable locals also support CLR `ref` and `out` arguments. [Mutable captures example](../examples/mutable_captures.oxel)

Constructed delegates such as `System.Action<i32>` also work. For a generic CLR method such as `System.Array.ForEach`, the ordinary arguments can determine the method type arguments before oxel checks the lambda. Supply explicit method type arguments when the ordinary arguments leave them open. [Generic callback example](../examples/generic_callbacks.oxel) · [CLR generic parameter flags](https://learn.microsoft.com/en-us/dotnet/api/system.reflection.genericparameterattributes)

Call a CLR delegate with `callback(value)` or `make_callback()(value)`. Use `(button.Callback)(value)` for a delegate stored in a field or property, and `(callback)(value)` when a named function shares the binding's name. Oxel checks the arguments against the delegate's `Invoke` method. Check a nullable delegate before calling it. Try `oxel run --framework examples/delegate_calls.oxel`. [Runnable example](../examples/delegate_calls.oxel)

### CLR events

Use `+=` to subscribe a delegate to a public CLR event and `-=` to unsubscribe it. A lambda can appear directly on the right-hand side, or be stored in a binding annotated with its delegate type. Keep the delegate in a binding when you need to unsubscribe it later.

```rust
let on_click: System.EventHandler = |sender, _| {
    System.Console.WriteLine("clicked");
};
button.Click += on_click;
button.Click -= on_click;
```

Write `|| { ... }` for a zero-parameter lambda expression. Event operations work through instance, interface and static members, including inherited events and events on constructed CLR types. [Observable collection example](../examples/events.oxel) · [CLR event accessors](https://learn.microsoft.com/en-us/dotnet/api/system.reflection.methodsemanticsattributes)

oxel follows CLR type forwarders when resolving supported top-level types in imported signatures, generic method bounds and inheritance. Provide the forwarding assemblies and the assembly containing the definition through `--reference`, the manifest, or the resolved NuGet dependencies. The compiler checks each assembly identity and resolves the chain from metadata. CLR multi-module assemblies remain future work. [.NET type forwarding](https://learn.microsoft.com/en-us/dotnet/standard/assembly/type-forwarding)

### Imported generic CLR types

Imported public generic classes and interfaces can be named as `Namespace.Type<Arguments>` in oxel signatures, locals and fields. Invoke constructors with `new Namespace.Type<Arguments>(values)`. Constructors, methods and properties can use constructed reference types in their signatures, including nested types and signatures that use the declaring type's parameters. The compiler substitutes those parameters during type checking and emits member references with the original CLR member signature. [ECMA-335 generic member signatures](https://ecma-international.org/wp-content/uploads/ECMA-335_6th_edition_june_2012.pdf)

```sh
dotnet build examples/interop/Bridge.csproj -o target/interop
oxel run examples/interop/generic_classes.oxel --reference target/interop/Bridge.dll
```

Generic instance methods support inference from value arguments or explicit arguments such as `cell.SetAndReturn::<string>(60, "hello")`. A method declared on a generic class can use both its declaring type's parameters and its own method parameters. The compiler also supports generic virtual and interface calls through imported receivers and supported named bounds. [ECMA-335 generic methods](https://ecma-international.org/wp-content/uploads/ECMA-335_6th_edition_june_2012.pdf)

CLR signatures can annotate a type parameter use as `T?`. With a concrete reference argument such as `string`, Oxel applies the annotation as `string?`. With a concrete value argument such as `i32`, the annotated use has type `i32`. C# emits `System.Nullable<T>` as a separate generic type for value-constrained parameters. Run `oxel run --framework examples/annotated_clr_generics.oxel` to see both outcomes with `System.Array.Find`. Forwarding an open parameter into an annotated CLR position and declaring nullable open reference parameters in Oxel are future work. [Array.Find example](../examples/annotated_clr_generics.oxel) · [C# generic nullability](https://learn.microsoft.com/en-us/dotnet/csharp/language-reference/builtin-types/nullable-reference-types)

Inherited members and implicit conversions follow constructed generic bases and interface chains. For example, `TextCell : Cell<string>` can supply the type argument in `Copy<T>(Cell<T> value)`, and a `Cell<string>` can convert to the `IReadCell<string>` interface it implements. Calls through interface receivers resolve inherited methods and properties. Named non-generic bounds can also give an oxel type parameter access to a constructed ancestor. [ECMA-335 generic inheritance and binding](https://ecma-international.org/wp-content/uploads/ECMA-335_6th_edition_june_2012.pdf)

Imported generic interfaces and delegates honor CLR covariance (`out`) and contravariance (`in`). In the [generic classes example](../examples/interop/generic_classes.oxel), a `Cell<string>` implements `IReadCell<string>` and can be passed as `IReadCell<object>` because `IReadCell<out T>` is covariant. Contravariance lets an `IWriteCell<object>` be passed as `IWriteCell<string>` when the interface declares `in T`. Value type arguments and invariant parameters require identical types. [CLR generic variance](https://learn.microsoft.com/en-us/dotnet/standard/generics/covariance-and-contravariance)

Variance also respects nullable annotations. For example, `IReadCell<string>` can widen to `IReadCell<object?>`. The reverse nullability conversion is rejected. Mutable invariant types such as `List<string>` and `List<string?>` require matching element annotations, so a writable collection preserves its nullability guarantees.

Use `Namespace.Cell::<i32>.From(30)` to call a static method on a constructed generic class, and `Namespace.CellDefaults::<i32>.Initial` to read or assign a static property. The declaring type and a generic method take separate argument lists, as in `CellDefaults::<i64>.Choose::<bool>(30, true)`. Method type arguments can also be inferred. Each closed CLR type has its own static storage. [Static members on generic types](https://learn.microsoft.com/en-us/dotnet/csharp/language-reference/keywords/static)

Imported generic types and methods support the CLR constraints for reference types (`class`), non-nullable value types (`struct`), public parameterless construction (`new()`), and imported base classes or interfaces. A constraint can name a constructed type, such as `ICompare<T>`, or another type parameter, such as `where T : U`. The compiler substitutes both declaring-type and method type arguments before checking the constraint. An oxel function can forward a type parameter when its own declared bounds imply the imported constraint. Oxel checks imported reference-type constraints as non-nullable. Imported C# `unmanaged` constraints accept Oxel scalar types, native integers, and CLR enums. Verifying arbitrary struct layouts and handling `notnull` and `class?` are future work. [CLR generic parameter constraints](https://learn.microsoft.com/en-us/dotnet/csharp/programming-guide/generics/constraints-on-type-parameters)

The [interop example](../examples/interop/main.oxel) builds a C# library and uses its constructors, properties, overloaded instance methods and interface dispatch from oxel.

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

Run the [native interop example](../examples/native_interop/main.oxel) on Linux with a C compiler.

```sh
cd examples/native_interop
mkdir -p target
cc -shared -fPIC -o target/liboxel_math.so math.c
oxel run
oxel compile
binary=$(oxel publish)
"$binary"
```

The example's `[package]` sets `native_libraries = ["target/liboxel_math.so"]`. Paths resolve from the manifest directory. `compile`, `run` and `publish` copy the listed files beside the managed assembly or native executable under their original filenames. Each filename must be unique in the output directory. `check` can run before the C library is built. [Example manifest](../examples/native_interop/oxel.toml)

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

For `utf8` and `utf8_nullable`, use `*const u8`, `*const core::ffi::c_char` or `*const std::ffi::c_char`. The Rust function must finish using any of these annotated pointers before it returns. Build and run the [Rust bindgen example](../examples/rust_bindgen/abi.rs) from the repository root on Linux.

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

The [batch example](../examples/rust_bindgen/batch/main.oxel) lets Rust mutate an Oxel array and invokes a retained callback after a forced .NET collection.

```sh
mkdir -p examples/rust_bindgen/batch/target
rustc --edition 2024 --crate-type cdylib examples/rust_bindgen/batch/abi.rs \
  -o examples/rust_bindgen/batch/target/liboxel_rust_batch.so
oxel bindgen examples/rust_bindgen/batch/abi.rs --library oxel_rust_batch \
  -o examples/rust_bindgen/batch/target/batch.ffi.oxel
oxel run --manifest-path examples/rust_bindgen/batch/oxel.toml
```
