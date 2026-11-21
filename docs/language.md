# Language reference

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

- An executable has one `main`. See the [entry point examples](cli.md#quick-start) for its supported parameters and result types. Functions can call each other in any source order. Parameters require annotations. The omitted result type is `void`.
- `bool`, `char`, `string`, `object` and `void` map to `System.Boolean`, `System.Char`, `System.String`, `System.Object` and `System.Void`. Signed integers `i8`, `i16`, `i32` and `i64` map to `System.SByte`, `System.Int16`, `System.Int32` and `System.Int64`. Unsigned integers `u8`, `u16`, `u32` and `u64` map to `System.Byte`, `System.UInt16`, `System.UInt32` and `System.UInt64`. `f32` and `f64` map to `System.Single` and `System.Double`. Fully qualified CLR names are valid annotations. `void` is a result type. [Unsigned integer example](../examples/unsigned_integers.oxel)
- Integer literals default to `i32` and take another integer type from a typed context when they fit. Use decimal digits or the prefixes `0x`, `0o`, and `0b` for hexadecimal, octal, and binary. Uppercase prefixes are also accepted. Insert `_` between digits for readability, as in `0b1111_0000`, `1_000`, or `12.5_0e-0_1`. Integer arithmetic wraps at the declared width. Integer division and remainder truncate toward zero, and division by zero raises an error. The CLR also checks signed division overflow at the evaluation width. Small integers evaluate on the `i32` stack and are narrowed after each arithmetic operation. Unary `-` requires a signed integer or floating-point operand.
- Decimal floating-point literals such as `3.0`, `5e-2` and `1.25E+3` default to `f64`. A typed context can give a literal type `f32`, which parses and computes at single precision. Literals that overflow their target width are rejected. Floating-point division by zero follows IEEE 754. NaN is unequal to itself, and all ordering comparisons with NaN are false. Numeric operations use equal-width operands from the same family. [CLR floating-point comparisons](https://learn.microsoft.com/en-us/dotnet/api/system.reflection.emit.opcodes.clt_un?view=net-6.0)
- Locals have block scope. An initialized `let` can infer its type. A declaration such as `let result: i32;` gives the type and delays initialization. The compiler checks that every reachable read follows an assignment. An immutable binding can be initialized once on each path, and `let mut` permits subsequent writes. A new binding can shadow an earlier one.
- Update an initialized mutable binding with `+=`, `-=`, `*=`, `/=`, `%=`, `&=`, `|=`, `^=`, `<<=` or `>>=`. The operation uses the same operand types and rules as the corresponding binary operator. Strings support `+=` for concatenation. Compound assignments are statements and also work in constant evaluation.
- Use `&`, `|` and `^` on two integers of the same type, two booleans, or two values of the same CLR enum. Both operands are evaluated. Use `~` to complement an integer or CLR enum at its declared width. Bitwise operators bind more tightly than comparisons, with `&` binding before `^`, then `|`. These operators also work in constants and `comptime fn`. [CLR flags example](../examples/bitwise.oxel) · [CLR enumeration operators](https://learn.microsoft.com/en-us/dotnet/csharp/language-reference/language-specification/expressions)
- Use `<<` and `>>` with an integer value and an `i32` count. The result keeps the value's type. Oxel masks the count by the declared bit width minus one, so `1 << 8` in a `u8` context yields `1`. Right shift extends the sign for signed integers and fills with zeroes for unsigned integers. Shifts bind more tightly than `&` and less tightly than addition.
- A block can yield its last expression by omitting the semicolon. `return` exits the containing function. An `if` condition is boolean. An `if` that yields a value requires an `else` unless evaluation leaves the function.
- A `while` loop evaluates a boolean condition before each iteration. Its body has its own scope. `break` exits the innermost enclosing loop body, and `continue` reevaluates that loop's condition. In the condition of a nested loop, either keyword targets the enclosing loop body.
- `for i in start..end { ... }` visits an ascending integer range, excluding the end. Use `..=` to include the end, even if it is the largest value of the counter's type. The bounds must have the same integer type and evaluate once, from left to right, before the body. The loop binding is immutable and local to the body. A reversed range is empty, and `continue` advances to the next value. [Range and comptime factorial example](../examples/ranges.oxel)
- `for value in values { ... }` visits the elements of a non-nullable CLR vector in index order, or the UTF-16 `char` values of a non-nullable string. The source expression evaluates once before the loop, and each element is read when its iteration begins. The loop binding is immutable and local to the body. Array and string iteration also work in `comptime fn`. [Array example](../examples/arrays.oxel)
- With `--framework`, `for value in values { ... }` also accepts a non-nullable CLR `IEnumerable<T>`, including collection types that implement it and type parameters constrained to it. The source is evaluated once and elements are read lazily in enumeration order. The enumerator is disposed on exhaustion and on early exit. If the source implements multiple distinct `IEnumerable<T>` interfaces, assign it to a binding of the desired interface type before iterating. Run `oxel run --framework examples/collections.oxel` to iterate over a list and a LINQ sequence. [Collection example](../examples/collections.oxel) · [CLR enumeration interfaces](https://learn.microsoft.com/en-us/dotnet/csharp/language-reference/language-specification/statements#13952-synchronous-foreach)
- `&&` and `||` short-circuit. They bind more loosely than comparisons, and `&&` binds more tightly than `||`.
- Operators include `+`, `-`, `*`, `/`, `%`, unary `-` and `!`, and `==`, `!=`, `<`, `<=`, `>` and `>=`. Equality supports booleans, characters, equal-width numbers and strings. Ordering supports characters and equal-width numbers.
- String literals use double quotes, accept Unicode and support `\"`, `\\`, `\n`, `\r`, `\t` and `\0`. Use `+` to concatenate two strings. String equality is ordinal and case-sensitive. Strings returned by CLR methods retain CLR null behavior. [System.String](https://learn.microsoft.com/en-us/dotnet/api/system.string)
- Character literals such as `'A'`, `'界'`, and `'\u{D83D}'` contain one UTF-16 code unit. Characters support equality, ordering and checked casts to and from integers. For strings, `.Length` counts UTF-16 code units and `text[index]` reads a `char` at an `i32` index. Both operations work in constant evaluation. A supplementary character occupies two indices. [Character example](../examples/characters.oxel) · [System.Char](https://learn.microsoft.com/en-us/dotnet/api/system.char?view=net-10.0)
- Use `$"Hello, {user.name}"` to insert an expression into a string. Write `{{` or `}}` for literal braces. Interpolation evaluates and formats each expression once, from left to right. Runtime values use CLR string conversion, and null contributes empty text. Constant evaluation supports string-valued expressions in interpolation. See the [interpolation example](../examples/interpolation.oxel). [CLR string concatenation](https://learn.microsoft.com/en-us/dotnet/api/system.string.concat)
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

The check follows execution order through blocks, branches, loops, early returns, short-circuit expressions and loop exits. A path that exits the function does not need to initialize a local for later reads. An immutable local declared inside a loop can initialize anew on each iteration. A write to an immutable local declared outside the loop is rejected when a later iteration could write it again. [Runnable example](../examples/assignment.oxel)

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

Use `as` for explicit boxing, checked casts between related reference types, or unboxing from `object`. A failed cast raises the standard CLR exception. Parenthesize the cast before accessing a member, as in `(value as Namespace.Type).Member()`. The [objects example](../examples/objects.oxel) also boxes a generic struct and restores its concrete type. [CLR boxing and unboxing](https://learn.microsoft.com/en-us/dotnet/csharp/programming-guide/types/boxing-and-unboxing)

Use `value as? T` to get a standard `Option<T>` from a CLR runtime type check. It evaluates the input once and produces `None` for null or a type mismatch. The target must be non-nullable. CLR reference and interface matches preserve object identity, and a boxed value must match its runtime type exactly. For example, `29 as? i64` produces `None`. Use `(value as? T)?` to propagate failure. [Runnable example](../examples/optional_casts.oxel)

`as` also converts between numeric types. Conversions to integers truncate fractional values toward zero, then check the target range. An out-of-range result, NaN, or infinity raises `System.OverflowException` at runtime and produces a diagnostic in constant evaluation. Conversions to floating-point types round to the destination precision. An integer literal directly before a numeric cast can use the full `i64` range. A direct cast to `u64` also accepts literals up to `18446744073709551615`. See the [numeric casts example](../examples/numeric_casts.oxel). [CLR numeric conversion behavior](https://learn.microsoft.com/en-us/dotnet/csharp/language-reference/builtin-types/numeric-conversions#explicit-numeric-conversions)

## Nullable references

Reference annotations such as `string`, `object` and imported CLR class names reject source `null` by default. Add `?` when a binding, parameter, result or field can hold null. A non-null reference can widen to its nullable type. Use `unwrap_or(value, fallback)` to supply a fallback. A nullable fallback keeps the result nullable. Use `unwrap_or_else(value, { ... })` to evaluate the fallback only when needed.

Comparisons with `null` refine immutable locals and parameters in `if` and `while` branches and through short-circuit boolean expressions. Mutable bindings require an explicit fallback or a checked immutable copy. A cast from a nullable reference to another reference keeps the destination nullable. [Runnable example](../examples/nullable.oxel)

An early exit can establish a non-null type for the following statements. After `if text == null { return; }`, an immutable `text` can be used as `string`. The same rule applies to CLR predicates with conditional nullability contracts. When both branches can continue, a binding must be non-null on both paths to keep the refined type.

The compiler uses the same CLR type signature for a reference and its nullable form. It emits Roslyn-compatible annotations on public parameters, returns and struct fields, including concrete reference arguments in generic struct signatures. It reads nullable annotations and inherited declaration context from imported method and property signatures. References without annotations retain their prior behavior. Explicitly annotate results from those APIs when they can return null. [Roslyn nullable metadata](https://github.com/dotnet/roslyn/blob/main/docs/features/nullable-metadata.md)

Use concrete nullable arguments in generic types, such as `Cell<string?>` and `System.Collections.Generic.List<string?>`. Substituting a nullable argument preserves its nullability in values read from fields, methods and indexers. The compiler reads and emits these annotations in nested CLR signatures. Run the example with `oxel run --framework examples/nullable_generics.oxel`. [Runnable example](../examples/nullable_generics.oxel)

Oxel imports `MaybeNull` and `NotNull` on CLR method returns, properties, indexers and field reads. These contracts affect the outermost reference, including a concrete reference substituted for a generic parameter. A `MaybeNull` result can infer `Option<T>` in a direct `let` binding. [C# nullability analysis attributes](https://learn.microsoft.com/en-us/dotnet/csharp/language-reference/attributes/nullable-analysis)

`AllowNull` and `DisallowNull` govern by-value reference arguments and reference writes to CLR properties, indexers and fields. For example, an `AllowNull` property can accept `null` in its setter and still return a non-null reference from its getter. A mutable CLR field that accepts `null` is also read as nullable because a write directly changes its storage.

`NotNullIfNotNull` can refine a CLR method or indexer result to a non-null reference when the named by-value argument is known to be non-null. Otherwise the result retains its normal nullability and can still map to `Option<T>`. [C# conditional postconditions](https://learn.microsoft.com/en-us/dotnet/csharp/language-reference/attributes/nullable-analysis#conditional-postconditions-notnullwhen-maybenullwhen-and-notnullifnotnull)

With framework references, `System.IO.Path.GetFileName("src/main.oxel")` can be assigned directly to `string`. Passing a nullable path to the same API preserves its nullable result.

`NotNullWhen` can refine an immutable by-value binding or an immutable `out` local on the specified boolean branch. For example, the true branch of `!System.String.IsNullOrEmpty(text)` can use `text` as a `string`. A `MaybeNullWhen` contract can also preserve a declared non-null `out` reference on the opposite branch, as with `Dictionary<string, string>.TryGetValue`. Refinement works in `if`, `while`, short-circuit expressions and match guards. Mutable locals require an explicit check on an immutable copy or a fallback. [C# conditional postconditions](https://learn.microsoft.com/en-us/dotnet/csharp/language-reference/attributes/nullable-analysis#conditional-postconditions-notnullwhen-maybenullwhen-and-notnullifnotnull)

A CLR parameter marked `NotNull` refines an immutable argument or an immutable `out` local after the call returns normally. With framework references, call `System.ArgumentNullException.ThrowIfNull(text);` to check a nullable argument and then use `text` as a non-null reference. A catch handler that allows execution to continue after a failed check leaves the argument nullable. [CLR unconditional postconditions](https://learn.microsoft.com/en-us/dotnet/csharp/language-reference/attributes/nullable-analysis#unconditional-postconditions-maybenull-and-notnull)

Oxel also reads `DoesNotReturnIf` on CLR boolean parameters. For example, if `Reject` has `[DoesNotReturnIf(true)]`, a normal return from `Reject(text == null)` establishes that an immutable `text` is non-null. Short-circuit checks such as `Reject(first == null || second == null)` can establish both facts at once. [CLR guard contracts](https://learn.microsoft.com/en-us/dotnet/csharp/language-reference/attributes/nullable-analysis#stop-nullable-analysis-when-called-method-throws)

## Nullable values

With `--framework`, a suffix on a non-nullable value type such as `i32?` constructs the CLR type `System.Nullable<i32>`. Use `null` for an empty value and a value of the underlying type for a present value. This applies to CLR enums, structs and generic type parameters constrained to `struct`. Use `value != null`, `value.HasValue`, or `unwrap_or(value, fallback)` to handle absence. An explicit unwrap with `as` or `.Value` throws if the value is empty. An empty nullable value boxes to `null`, so a direct boxing conversion requires `object?`. [Nullable value example](../examples/nullable_values.oxel) · [CLR nullable value types](https://learn.microsoft.com/en-us/dotnet/csharp/language-reference/builtin-types/nullable-value-types)

```sh
oxel run --framework examples/nullable_values.oxel
```

## Arrays

`T[]` is a CLR zero-based vector. Use `[10, 20]` to initialize its elements, or `new i32[count]` to allocate zero-initialized storage. A typed context supplies the element type for a literal, including an empty literal such as `let empty: i32[] = [];`. Otherwise the first element supplies the type. Use `.Length` for the length and an `i32` index for reads and writes. A compound update such as `values[i] += 2` evaluates the array and index once.

Nullability is independent at each level. `string?[]` has nullable elements and `string[]?` is a nullable array with non-nullable elements. Use a literal for non-nullable reference elements, or `new string?[count]` for storage initialized to null. Length allocation requires an element type with a valid zero value. Jagged vectors use nested array types such as `i32[][]`. Arrays implicitly convert to the CLR interfaces `IEnumerable<T>`, `IReadOnlyCollection<T>`, and `IReadOnlyList<T>` with matching element types or compatible reference element types.

The standard `Option<T>` treats zero-initialized CLR storage as `None`, including `new Option<string>[count]` and arrays of Oxel structs whose fields all have valid zero values. A `Some` payload is read only after the option's tag identifies a present value. This behavior also applies in `comptime fn`. [Option array example](../examples/option_arrays.oxel)

Supported CLR signatures can pass and return these arrays, including generic `T[]` signatures. Array element types are invariant in oxel. Rectangular arrays are outside this subset. The [array example](../examples/arrays.oxel) prints `30`. [CLR vector allocation](https://learn.microsoft.com/dotnet/api/system.reflection.emit.opcodes.newarr)

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

Use `Window { title: "Settings", ..original }` to construct a copy with selected fields replaced. Explicit field expressions evaluate first in source order, followed by the base expression, exactly once each. The base must have the same struct type and goes last. For generic structs, write the type arguments explicitly or supply them through the expected type, as with an ordinary struct literal. Struct update also works in compile-time code. [Nested struct update example](../examples/struct_update.oxel)

Use `let Window { title: name, .. } = window;` to bind a field from a struct. A field pattern can itself bind a value, test a literal, or destructure a nested struct or enum. List every field or use a final `..` to ignore the remaining fields. Generic arguments come from the scrutinee type. An ordinary `let` pattern must cover every value of its type. Use `let … else`, `if let`, `while let` or `match` for patterns that can fail. [Nested struct example](../examples/struct_update.oxel)

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

The compiler emits public CLR generic value types, which C# can use as `Container<int>`. The [generic structs example](../examples/generic_structs.oxel) combines nested structs, field access and generic functions. [ECMA-335 generic types](https://ecma-international.org/wp-content/uploads/ECMA-335_6th_edition_june_2012.pdf)

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

In runtime functions, use `let Some(value) = input else { return; };` to destructure a refutable pattern. The initializer evaluates once and the immutable bindings are available after the statement. The `else` block sees the enclosing scope and must exit the current path, for example through `return`, `throw`, `break` or `continue`. Boolean and nested enum patterns use the same rules as `match`. An optional annotation after the pattern supplies the initializer type. [Let-else example](../examples/let_else.oxel)

Use `if let Some(value) = input { ... }` to run a block only when the pattern matches. Add `else` to handle failure or to return a value from the conditional. Use `while let Some(value) = next() { ... }` to reevaluate the input before each iteration and stop when the pattern fails. Bindings are scoped to the successful branch or loop body. Both forms can match nullable CLR values directly. [Pattern conditions example](../examples/pattern_conditions.oxel)

Patterns can inspect nested variants or boolean payloads. For example, a `Result<Maybe<i32>, string>` can distinguish `Ok(Some(value))`, `Ok(Maybe.None)`, and `Err(_)`. Inside a payload pattern, a bare name binds the value. Use `Maybe.None` or `None()` to match a nested unit variant. A standalone `bool` can also be matched with `true` and `false` arms. Match tests its arms in source order and runs the first successful arm. The compiler reports both uncovered cases and arms fully covered by earlier patterns.

Add a boolean guard after a pattern, as in `Some(value) if value > 0 => value`. The guard sees the pattern bindings and evaluates once after the pattern matches. A false guard continues to the next arm. Exhaustiveness is checked using the unguarded arms. [Match guard example](../examples/match_guards.oxel)

Integer, character and string literals are patterns too. Use `200`, `-1`, `'界'` or `Some("cached")` in a match, or use a literal with `if let`, `while let` and `let … else`. Integer literals must fit the input type and equivalent spellings such as `0xa` and `10` cover the same value. Text matching is ordinal and case-sensitive. Use `_` for values outside the listed literals, or list every value of a finite type such as `u8`. [Literal pattern example](../examples/literal_patterns.oxel)

Dense matches on characters and integers up to 32 bits can use a CIL jump table. Values in gaps and outside the table follow the default branch. [CIL switch instruction](https://learn.microsoft.com/en-us/dotnet/api/system.reflection.emit.opcodes.switch?view=net-10.0)

Use `|` to share a branch between patterns, as in `200 | 201 => "success"` or `Some("start" | "stop")`. Alternatives must bind the same names with the same types. The first matching alternative supplies the bindings. Its arm's guard runs once, and a false guard continues with the next arm. Alternatives also work in `if let`, `while let` and `let … else`. [Pattern alternatives example](../examples/pattern_alternatives.oxel)

The compiler emits tagged CLR value types, including generic CLR value types for generic enums. Their storage layout may change. A default value supplied by CLR code is `None` for the standard `Option<T>`. For a user-defined enum, its default value has no valid tag, and matching it raises `System.InvalidOperationException`. By-value recursive payloads are rejected. [Runnable example](../examples/unions.oxel)

## Error propagation

```rust
fn next(input: Result<i32, string>) -> Result<i32, string> {
    let value = input?;
    Result.Ok(value + 1)
}
```

The postfix `?` evaluates its operand once. It yields the `Ok` payload or immediately returns an `Err` from the enclosing function. The standard `Result<T, E>` supplies `Result.Ok(value)` and `Result.Err(error)`. A module may declare its own compatible `Result` enum with `Ok(T)` and `Err(E)` in either order. The compiler emits the standard definition into each assembly that uses it, with each assembly retaining its own CLR type identity.

The two `Result` types may have different success types. The input error must be assignable to the function's error type, including ordinary CLR reference conversions and boxing to `object`. The operand determines its own generic arguments, so use a typed input or supply explicit arguments to an otherwise ambiguous call. [Runnable example](../examples/errors.oxel)

`try { ... }` gives `?` a local propagation target. For `Result<T, E>`, a successful tail expression becomes `Ok`. On `Err`, the nearest enclosing try block produces the error result and the enclosing expression continues. The first `?` with a typed operand determines the error type unless the surrounding expression supplies an expected `Result` type. A try block requires a `?` and a success value, or an explicit success type if the body diverges. Loop control is available in loops declared within the block. Use `return` in the enclosing function after handling the block's result. [Result example](../examples/try_blocks.oxel)

The standard `Option<T>` enum provides `Option.Some(value)` and `Option.None`. Supply the element type through a typed context or use an explicit constructor such as `Option.Some::<i32>(42)`. The `?` operator yields the `Some` payload or returns `None` from the enclosing function or nearest try block. An Option try block wraps its successful tail in `Some`. Each try block uses one enum family, inferred from its operands or supplied by its expected type. Nest try blocks to use both families in a single expression. Existing modules can still declare their own `Option`. [Option example](../examples/option_try.oxel)

A direct CLR method, property, field or indexer read in a `let` initializer infers `Option<T>` when the result is nullable. A nullable reference or `System.Nullable<T>` also converts to the standard `Option` when an argument or result expects it. An absent CLR value becomes `None`, and a present value becomes `Some(value)`. Use `Option<T>` in Oxel signatures and an explicit annotation such as `let name: string? = ...` to select the underlying CLR type. The postfix `?` can propagate `None` directly from a nullable CLR call, including `(System.Array.Find(names) |name| { name == target })?`. The input expression is evaluated once. Use `match` to handle both cases or `option.unwrap_or(fallback)` to supply a value on `None`. Use `oxel run --framework examples/clr_option.oxel` to try these forms. [CLR Option example](../examples/clr_option.oxel)

The standard `Option<T>` and `Result<T, E>` offer `value.unwrap_or(default)` and `value.unwrap_or_else({ ... })`. Both return the success value when present and the fallback on `None` or `Err`. Use `match` to read a `Result`'s error value. The free forms `unwrap_or(value, default)` and `unwrap_or_else(value, { ... })` also accept nullable CLR references and `System.Nullable<T>`. Both forms evaluate the input once. `unwrap_or` evaluates the fallback after the input, including when it already contains a value. `unwrap_or_else` evaluates its fallback expression or block only on absence or error. A fallback block follows the surrounding function's control-flow rules.

Use these calls in place of the previous `??` operator. The postfix `?` continues to propagate failure.

In the other direction, an expected CLR nullable type with the same underlying type accepts a standard `Option<T>`. For example, pass an `Option<string>` directly to a `string?` argument or write it to an element of `List<string?>`. A value of `None` produces a null reference or an empty `System.Nullable<T>`. A value of `Some(0)` produces a populated nullable integer containing zero. Use an explicit cast such as `choice as string?` to select the CLR type at an overloaded call. An exact nullable conversion takes precedence over boxing the option when overloads require the same number of conversions. Managed `ref` and `out` parameters require a local of the CLR storage type. [CLR nullable values](https://learn.microsoft.com/en-us/dotnet/api/system.nullable-1?view=net-10.0)

Use `throw value` with `--framework` to raise a CLR exception. The value must be assignable to non-nullable `System.Exception`. A throw ends the current control-flow path, so another branch can supply the value of an enclosing expression. [Runnable example](../examples/exceptions.oxel) · [CIL throw instruction](https://learn.microsoft.com/en-us/dotnet/api/system.reflection.emit.opcodes.throw?view=net-10.0)

```sh
oxel run --framework examples/exceptions.oxel
oxel run --framework examples/exceptions.oxel -- disable
```

Use `try { ... } catch (error: System.Exception) { ... }` with `--framework` to handle CLR exceptions. Catch clauses are checked in source order and can name derived exception classes. Use `_` to ignore the exception. The body and handlers can produce a value, including inside a larger expression. An unmatched exception continues unwinding. This form uses CLR exception regions. The existing `try { ... }` form without a catch clause provides a propagation target for `Option` and `Result`. [Runnable example](../examples/catch.oxel) · [CLR exception handling](https://learn.microsoft.com/en-us/dotnet/standard/exceptions/)

`return`, `break`, `continue` and `?` can cross CLR exception regions. A propagation block keeps its existing rules for control transfers.

### Scoped resources

Use `using name = resource { ... }` with `--framework` to acquire a non-null `System.IDisposable` resource once and bind it for the duration of the block. The block can produce a value. Oxel calls `IDisposable.Dispose` in a CLR finally handler when control leaves the block, including by an exception, `return`, `?`, `break` or `continue`. Nested bindings dispose in reverse acquisition order. The resource type can be a CLR class, a CLR struct or a generic type constrained to `IDisposable`. [Runnable example](../examples/using.oxel) · [CLR using semantics](https://learn.microsoft.com/en-us/dotnet/csharp/language-reference/statements/using)

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

oxel emits CLR generic methods. The emitted definitions can also be called from C# as `Program.identity<int>(30)`. The [generics example](../examples/generics.oxel) instantiates the same method with a struct, an integer and a string. [ECMA-335 generic signatures and method instantiations](https://ecma-international.org/wp-content/uploads/ECMA-335_6th_edition_june_2012.pdf)

### Generic constraints

```rust
fn read<T: Samples.Interop.IReadable>(value: T) -> i32 {
    value.Read()
}
```

A method type parameter can name one imported CLR base class and any number of interfaces, separated by `+`. The compiler checks these bounds for inferred and explicit calls, including calls that forward a type parameter. Bound members are available inside the method, and a value of the type parameter can be passed as one of its bound types. Class-bound members take precedence over imported interface members with the same name. Use a binding with an explicit interface type to select among competing interface methods.

Bounds can name constructed types and refer to the function's type parameters. For example, `fn compare<T: Library.ICompare<T>>(first: T, second: T) -> i32 { first.Compare(second) }` calls the imported interface through its CLR generic constraint. The CLR signature preserves the constructed bound for C# and other callers.

Generic `struct` and `enum` declarations use the same bound syntax. A bound is available when checking fields or variant payloads that use a constrained type. For example, the [generic interop example](../examples/interop/generic_classes.oxel) declares `CellView<T: Samples.Interop.IReadCell<i64>>`. C# can instantiate the emitted `CellView<T>` with a type that implements that interface.

Type parameters can constrain one another. For example, `fn widen<U, T: U>(value: T) -> U { value }` requires a type argument for `T` that can convert to `U`. Members available through `U`'s bounds are also available on `T`. The compiler rejects cycles and incompatible class bounds in these dependencies. [CLR generic constraints](https://learn.microsoft.com/en-us/dotnet/csharp/language-reference/language-specification/classes#1525-type-parameter-constraints)

Use `T: class` to require a reference type and `T: struct` for a non-nullable value type. Use `T: new()` to call `new T()`. Combine the requirements with `T: struct + new()`. Oxel checks that an Oxel-defined type can safely be zero-initialized before accepting it for a `new()` constraint, including explicit `new()` constraints on imported APIs. A CLR value type with an explicit parameterless constructor runs that constructor through the CLR's generic constructor helper. CLR callers and imported APIs continue to follow CLR metadata constraints. [CLR generic parameter constraints](https://learn.microsoft.com/en-us/dotnet/csharp/programming-guide/generics/constraints-on-type-parameters) · [CLR generic construction](https://learn.microsoft.com/en-us/dotnet/api/system.activator.createinstance)

The compiler writes CLR generic constraint metadata and uses constrained instance dispatch. C# callers can instantiate an interface-constrained oxel method with a value type that implements the interface. [ECMA-335 generic parameter constraints](https://ecma-international.org/wp-content/uploads/ECMA-335_6th_edition_june_2012.pdf)

### Interfaces

Declare a behavioral contract with `interface` and attach it to a source struct with `impl`. Each implementation method has a read-only first parameter `self` of the implementing struct type. The remaining parameters and the result match the interface declaration.

```rust
interface Read<T> {
    fn read() -> T;
}

struct Cell<T> {
    value: T,
}

impl<T> Read<T> for Cell<T> {
    fn read(self: Cell<T>) -> T {
        self.value
    }
}

fn get<T>(reader: Read<T>) -> T {
    reader.read()
}

fn get_number<T: Read<i32>>(reader: T) -> i32 {
    reader.read()
}
```

Use an interface in a function signature, struct field, or generic argument. For an interface with type arguments, Oxel can infer those arguments from a struct's implementation. Passing a struct as an interface boxes it. A call through a generic constraint uses CLR constrained dispatch and keeps a struct value unboxed. With a constrained generic struct, repeat its type bounds on the `impl` block. [Runnable example](../examples/interfaces.oxel) · [CLR struct dispatch](https://learn.microsoft.com/en-us/dotnet/csharp/language-reference/language-specification/structs)

Oxel emits standard CLR interface metadata, so C# can implement an exported interface, call an Oxel implementation through it, and call generic Oxel functions with the same interface constraint. Oxel structs can also implement CLR interfaces with supported methods, such as `System.IDisposable` and `System.IComparable<i32>`. Source interfaces currently declare methods with distinct names. Source interface inheritance, default methods, and method-specific type parameters are outside this subset. Implementing a CLR interface with base interfaces, static abstract members, generic methods, or by-reference parameters is also outside the supported subset. [C# interfaces](https://learn.microsoft.com/en-us/dotnet/csharp/language-reference/language-specification/interfaces)

Build the [interop library](../examples/interop/Bridge.csproj) to run the [constraints example](../examples/interop/constraints.oxel).

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

[Runnable constants example](../examples/constants.oxel)

A `string?` constant can hold a string or `null`. Use `const LABEL: string? = null;` to give an absent value its type. Compile-time code can compare nullable strings with `null`, choose a lazy fallback with `unwrap_or_else`, and convert between `string?` and `Option<string>` through a typed binding or return. A null interpolated part contributes empty text, as it does at runtime.

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

The compiler tracks dependencies through calls, including across files, and rejects cycles involving a constant declaration. Recursive functions are allowed. Evaluation has step and nesting limits and detects repeated active calls with the same type arguments and numeric, boolean or null values when those calls are tracked. Within one initializer it can reuse completed calls with comparable arguments and results. [Runnable comptime example](../examples/comptime.oxel)

Full generic CLR interoperability and a CoreCLR comptime worker are later work.
