use std::{
    fs,
    process::{Command, Output},
};

fn successful(output: Output) -> String {
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn resolves_and_executes_static_overloads_from_a_csharp_assembly() {
    let folder = tempfile::tempdir().unwrap();
    let cwd = folder.path();
    let sdk = successful(Command::new("dotnet").arg("--version").output().unwrap());
    let major = sdk.split('.').next().unwrap().parse::<u32>().unwrap();
    let records_source = format!(
        "{}\nfn preserve<T>(value: T) -> T {{ value }}\n         struct Container<T> {{ value: T }}\n         fn pack<T>(value: T) -> Container<T> {{ Container {{ value: value }} }}\n
         fn optional_text(value: string?) -> string? {{ value }}
         enum Maybe<T> {{ Some(T), None }}
         fn enum_value() -> Maybe<i32> {{ Maybe.Some(30) }}
         fn unwrap(value: Maybe<i32>) -> i32 {{ 1 + match value {{ Some(x) => x, _ => 0 }} }}
         enum Envelope {{ Wrap(Maybe<i32>) }}
         fn unwrap_nested(value: Maybe<i32>) -> i32 {{
             1 + match Envelope.Wrap(value) {{ Wrap(Some(x)) => x, Wrap(Maybe.None) => 0 }}
         }}",
        include_str!("../../../examples/structs.oxel")
    );
    let records = oxel::compile(&records_source, "OxelRecords").unwrap();
    let records_path = cwd.join("OxelRecords.dll");
    fs::write(&records_path, &records).unwrap();
    fs::write(
        cwd.join("Bridge.csproj"),
        format!(
            r#"
        <Project Sdk="Microsoft.NET.Sdk">
          <PropertyGroup>
            <TargetFramework>net{major}.0</TargetFramework>
            <AssemblyVersion>3.2.1.0</AssemblyVersion>
          </PropertyGroup>
          <ItemGroup>
            <Reference Include="OxelRecords">
              <HintPath>OxelRecords.dll</HintPath>
            </Reference>
          </ItemGroup>
        </Project>
    "#
        ),
    )
    .unwrap();
    fs::write(
        cwd.join("Api.cs"),
        r#"
        namespace Samples.Interop {
        public static class Numbers {
            public static int Pick(int value) => value + 1;
            public static long Pick(long value) => value + 2;
            public static long Pair(bool first, long second) => first ? second + 3 : second;
            public static int Pair(int first, int second) => first + second;
            public static long Tie(int first, long second) => first + second;
            public static long Tie(long first, int second) => first + second;
            public static bool Flip(bool value) => !value;
            public static void Write() => System.Console.WriteLine(30);
            public static decimal Unsupported(decimal value) => value;
            public static string Pick(string value) => "[" + value + "]";
            public static string Duplicate(string value) => new string(value.ToCharArray());
            public static T Generic<T>(T value) => value;
            public static T Repeat<T>(T first, T second) => first;
            public static object Repack(object value) => value;
            public static object NullObject() => null!;
            public static string ObjectOverload(object _) => "object";
            public static string ObjectOverload(int _) => "int32";
            public static string Kind<T>(T _) => typeof(T).Name;
            public static int Width(float _) => 32;
            public static int Width(double _) => 64;
            public static int FloatBits(bool _, float value) => System.BitConverter.SingleToInt32Bits(value);
            public static int FloatBits(int _, int value) => value;
            public static float AddSingle(float a, float b) => a + b;
            public static double AddDouble(double a, double b) => a + b;
            public static short PickSmall(sbyte signed, byte unsigned, short bias)
                => (short)(signed + unsigned + bias);
            public static int Small(bool _, sbyte value) => value;
            public static int Small(int _, int value) => value;
            public static string Select(int value) => "concrete";
            public static string Select<T>(T value) => "generic";
            public static int GenericTie<T>(T first, int second) => second;
            public static int GenericTie<T>(int first, T second) => first;
            public static T NeedsArguments<T>() => default!;
            public static U Generic<T, U>(T first, U second) => second;
            public static T Constrained<T>(T value) where T : struct => value;
            public static T Comparable<T>(T value) where T : System.IComparable => value;
            private static int Hidden() => -1;
        }
        }
        public static class GlobalNumbers {
            public static int Value() => 42;
            public static string ReadOxelRecords() {
                Profile profile = Program.profile("Bob", 32);
                User user = Program.birthday(new User { name = profile.user.name, age = profile.user.age });
                if (Program.optional_text(null) != null || Program.optional_text("text") != "text")
                    throw new System.Exception("expected the CLR string signature");
                Container<User> packed = Program.pack<User>(user);
                user = Program.preserve<User>(packed.value);
                if (!packed.GetType().IsValueType) throw new System.Exception("expected a CLR value type");
                if (Program.unwrap(Program.enum_value()) != 31)
                    throw new System.Exception("expected CLR union round trip");
                try {
                    Program.unwrap(default(Maybe<int>));
                    throw new System.Exception("expected default CLR value to be rejected");
                } catch (System.InvalidOperationException) {}
                if (Program.unwrap_nested(Program.enum_value()) != 31)
                    throw new System.Exception("expected CLR nested union round trip");
                try {
                    Program.unwrap_nested(default(Maybe<int>));
                    throw new System.Exception("expected invalid nested tag to be rejected");
                } catch (System.InvalidOperationException) {}
                return user.name + ":" + user.age + ":" + typeof(User).IsValueType;
            }
        }
    "#,
    )
    .unwrap();
    successful(
        Command::new("dotnet")
            .current_dir(cwd)
            .args(["build", "--nologo", "--verbosity", "quiet", "-o", "lib"])
            .output()
            .unwrap(),
    );
    let original = cwd.join("lib/Bridge.dll");
    let reference = cwd.join("renamed input.dll");
    fs::rename(original, &reference).unwrap();
    let bytes = fs::read(&reference).unwrap();
    let mut references = oxel::References::default();
    assert_eq!(references.add(&bytes).unwrap(), "Bridge");
    assert_eq!(references.add(&records).unwrap(), "OxelRecords");
    assert!(
        references
            .add(&bytes)
            .unwrap_err()
            .contains("duplicate reference")
    );
    for (expression, expected) in [
        ("Samples.Interop.Numbers.Pick(true)", "E0205"),
        ("Samples.Interop.Numbers.Pick()", "E0205"),
        ("Samples.Interop.Numbers.Tie(1, 2)", "E0208"),
        ("Samples.Interop.Numbers.Hidden()", "E0203"),
        ("Samples.Interop.Numbers.NeedsArguments()", "E0205"),
        ("Samples.Interop.Numbers.Repeat(1, true)", "E0205"),
        ("Samples.Interop.Numbers.GenericTie(1, 1)", "E0208"),
        (
            "Samples.Interop.Numbers.Constrained::<string>(\"x\")",
            "E0212",
        ),
        ("Samples.Interop.Numbers.Comparable::<i32>(1)", "E0203"),
        ("Samples.Interop.Numbers.Generic::<i32>(true)", "E0301"),
        ("Samples.Interop.Numbers.Unsupported(1)", "E0203"),
        ("GlobalNumbers.Missing()", "E0203"),
    ] {
        let source = format!("fn main() {{ {expression}; }}");
        assert_eq!(
            oxel::check_with_references(&source, &references).unwrap_err()[0].code,
            expected
        );
    }
    let source = "const VALUE = 10; fn main() { Samples.Interop.Numbers.Pair(true, VALUE); }";
    assert_eq!(
        oxel::check_with_references(source, &references).unwrap_err()[0].code,
        "E0205"
    );
    let source = r#"
        struct LocalBox<T> { value: T }
        fn roundtrip<T>(value: T) -> T {
            Samples.Interop.Numbers.Generic(value)
        }
        fn report_kind<T>(value: T) -> T {
            System.Console.WriteLine(Samples.Interop.Numbers.Kind(value));
            value
        }
        fn main() {
            let long: i64 = 10;
            System.Console.WriteLine(Samples.Interop.Numbers.Pick(10));
            System.Console.WriteLine(Samples.Interop.Numbers.Pick(long));
            System.Console.WriteLine(Samples.Interop.Numbers.Pick(3000000000));
            System.Console.WriteLine(Samples.Interop.Numbers.Pair(true, 10));
            System.Console.WriteLine(Samples.Interop.Numbers.Flip(false));
            Samples.Interop.Numbers.Write();
            System.Console.WriteLine(GlobalNumbers.Value());
            System.Console.WriteLine(GlobalNumbers.ReadOxelRecords());
            System.Console.WriteLine(Program.extra());
            System.Console.WriteLine(Samples.Interop.Numbers.Pick("界 🌍"));
            let text = "a\0b";
            System.Console.WriteLine(Samples.Interop.Numbers.Duplicate(text) == text);
            System.Console.WriteLine(Samples.Interop.Numbers.Duplicate(text) != "ab");
            System.Console.WriteLine(roundtrip::<i32>(30));
            System.Console.WriteLine(Samples.Interop.Numbers.Generic::<bool, string>(true, "CLR"));
            System.Console.WriteLine(Program.preserve::<i64>(3000000000));
            let box = Samples.Interop.Numbers.Generic::<LocalBox<i32>>(LocalBox { value: 30 });
            System.Console.WriteLine(box.value);
            System.Console.WriteLine(Samples.Interop.Numbers.Repeat(3000000000, long));
            System.Console.WriteLine(Samples.Interop.Numbers.Select(1));
            System.Console.WriteLine(Samples.Interop.Numbers.Select(false));
            System.Console.WriteLine(Samples.Interop.Numbers.Generic(true, "inferred"));
            let single: f32 = 1.0;
            System.Console.WriteLine(Samples.Interop.Numbers.Width(single));
            System.Console.WriteLine(Samples.Interop.Numbers.Width(1.0));
            System.Console.WriteLine(Samples.Interop.Numbers.FloatBits(true, 1.0000000596046448));
            System.Console.WriteLine(Samples.Interop.Numbers.Repeat(1.0000000596046448, single) > single);
            System.Console.WriteLine(Samples.Interop.Numbers.AddSingle(1.5, 0.5) == 2.0);
            System.Console.WriteLine(Samples.Interop.Numbers.AddDouble(1.8, 0.2) == 2.0);
            System.Console.WriteLine(Samples.Interop.Numbers.PickSmall(-128, 255, -100));
            System.Console.WriteLine(Samples.Interop.Numbers.Small(true, -128));
            let byte: u8 = 1;
            System.Console.WriteLine(Samples.Interop.Numbers.Repeat(255, byte));
            let payload = Samples.Interop.Numbers.Repack(300);
            System.Console.WriteLine(payload as i32);
            System.Console.WriteLine(Samples.Interop.Numbers.ObjectOverload(300));
            System.Console.WriteLine(Samples.Interop.Numbers.ObjectOverload(true));
            let missing: object? = Samples.Interop.Numbers.NullObject();
            System.Console.WriteLine(missing is object);
            System.Console.WriteLine((missing ?? 30) as i32);
            let reported: object = report_kind(30);
            System.Console.WriteLine(reported as i32);
        }
    "#;
    let helper = oxel::compile("fn main() {} fn extra() -> i32 { 7 }", "Helper").unwrap();
    references.add(&helper).unwrap();
    let helper_path = cwd.join("Helper.dll");
    fs::write(&helper_path, &helper).unwrap();
    fs::write(cwd.join("user.oxel"), source).unwrap();
    let cli = |action| {
        Command::new(env!("CARGO_BIN_EXE_oxel"))
            .current_dir(cwd)
            .args([action, "user.oxel", "--reference"])
            .arg(&reference)
            .arg("--reference")
            .arg(&helper_path)
            .arg("--reference")
            .arg(&records_path)
            .output()
            .unwrap()
    };
    successful(cli("check"));
    assert!(!cwd.join("target").exists());
    assert_output(cli("run"));
    let assembly = cwd.join(successful(cli("compile")).trim());
    assert!(assembly.with_file_name("Bridge.dll").is_file());
    assert!(assembly.with_file_name("Helper.dll").is_file());
    let mut reversed = oxel::References::default();
    reversed.add(&helper).unwrap();
    reversed.add(&bytes).unwrap();
    reversed.add(&records).unwrap();
    assert_eq!(
        oxel::compile_with_references(source, "user", &references).unwrap(),
        oxel::compile_with_references(source, "user", &reversed).unwrap(),
    );
    assert_eq!(
        fs::read(&assembly).unwrap(),
        oxel::compile_with_references(source, "user", &references).unwrap()
    );
    assert_output(Command::new("dotnet").arg(assembly).output().unwrap());
}

fn assert_output(output: Output) {
    assert_eq!(
        successful(output).lines().collect::<Vec<_>>(),
        [
            "11",
            "12",
            "3000000002",
            "13",
            "True",
            "30",
            "42",
            "Bob:33:True",
            "7",
            "[界 🌍]",
            "True",
            "True",
            "30",
            "CLR",
            "3000000000",
            "30",
            "3000000000",
            "concrete",
            "generic",
            "inferred",
            "32",
            "64",
            "1065353217",
            "True",
            "True",
            "True",
            "27",
            "-128",
            "255",
            "300",
            "int32",
            "object",
            "False",
            "30",
            "Int32",
            "30",
        ]
    );
}

#[test]
fn reports_invalid_reference_at_its_filename() {
    let folder = tempfile::tempdir().unwrap();
    fs::write(folder.path().join("bad.dll"), b"not a CLR image").unwrap();
    fs::write(folder.path().join("example.oxel"), "fn main() {}").unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_oxel"))
        .current_dir(folder.path())
        .args(["check", "example.oxel", "--reference", "bad.dll"])
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("bad.dll:1:1: error[E0006]"));
    assert!(!folder.path().join("target").exists());
}

fn build_counter_bridge(cwd: &std::path::Path) -> u32 {
    let sdk = successful(Command::new("dotnet").arg("--version").output().unwrap());
    let major = sdk.split('.').next().unwrap().parse::<u32>().unwrap();
    fs::write(
        cwd.join("Bridge.csproj"),
        include_str!("../../../examples/interop/Bridge.csproj")
            .replace("net10.0", &format!("net{major}.0")),
    )
    .unwrap();
    fs::write(
        cwd.join("Counter.cs"),
        include_str!("../../../examples/interop/Counter.cs"),
    )
    .unwrap();
    successful(
        Command::new("dotnet")
            .current_dir(cwd)
            .args(["build", "--nologo", "--verbosity", "quiet", "-o", "lib"])
            .output()
            .unwrap(),
    );
    major
}

#[test]
fn constructs_clr_objects_and_dispatches_instance_calls() {
    let folder = tempfile::tempdir().unwrap();
    let cwd = folder.path();
    let major = build_counter_bridge(cwd);
    let bridge = cwd.join("lib/Bridge.dll");
    let bridge_bytes = fs::read(&bridge).unwrap();
    let mut references = oxel::References::default();
    references.add(&bridge_bytes).unwrap();
    let factory = oxel::compile_with_references(
        r#"
        fn main() {}
        fn transform(value: Samples.Interop.Counter) -> Samples.Interop.Counter {
            value.Copy()
        }
        "#,
        "Factory",
        &references,
    )
    .unwrap();
    let factory_path = cwd.join("lib/Factory.dll");
    fs::write(&factory_path, &factory).unwrap();
    references.add(&factory).unwrap();

    for (body, expected) in [
        (
            "let value: Samples.Interop.Counter? = null; value.Read();",
            "E0311",
        ),
        (
            "let value: Samples.Interop.Counter? = null; value.Value = 1;",
            "E0311",
        ),
        (
            "let a = new Samples.Interop.Counter(1); a.Value = false;",
            "E0301",
        ),
        (
            "let a = new Samples.Interop.Counter(1); a.Snapshot = 2;",
            "E0308",
        ),
        (
            "let a = new Samples.Interop.Counter(1); a.Initial = 2;",
            "E0308",
        ),
        ("let a = new Samples.Interop.Counter(1); a.Item;", "E0305"),
        (
            "Samples.Interop.CounterLabel.Tracked = new Samples.Interop.Counter(1);",
            "E0308",
        ),
        ("Samples.Interop.CounterLabel.Last;", "E0308"),
        (
            "let a = new Samples.Interop.Counter(1); a.Missing = 1;",
            "E0305",
        ),
        (
            "let a = new Samples.Interop.Counter(1); a = new Samples.Interop.Counter(2);",
            "E0204",
        ),
        ("1 + 2 = 0;", "E0204"),
        ("new Samples.Interop.Counter(true);", "E0301"),
        ("new Samples.Interop.Counter();", "E0205"),
        ("new Samples.Interop.ICounter();", "E0203"),
        ("new Samples.Interop.Missing();", "E0202"),
        ("new Samples.Interop.Counter(1).Hidden();", "E0203"),
        ("new Samples.Interop.Counter(1).Add();", "E0205"),
        ("new Samples.Interop.DoubleCounter(1).Hidden();", "E0203"),
        ("Samples.Interop.Counter.Read();", "E0203"),
        ("let a = true; a.Read();", "E0203"),
        (
            "let a: Samples.Interop.Counter = new Samples.Interop.FixedCounter();",
            "E0301",
        ),
        (
            "let a: Samples.Interop.ICounter = new Samples.Interop.Counter(1); let b: Samples.Interop.Counter = a;",
            "E0301",
        ),
        (
            "Samples.Interop.CounterLabel.Choose(new Samples.Interop.Counter(1));",
            "E0208",
        ),
        (
            "new Samples.Interop.Counter(1) == new Samples.Interop.Counter(1);",
            "E0302",
        ),
    ] {
        let program = format!("fn main() {{ {body} }}");
        let errors = oxel::check_with_references(&program, &references).unwrap_err();
        assert_eq!(errors[0].code, expected, "{body}: {errors:?}");
    }

    let foreign = cwd.join("foreign");
    fs::create_dir(&foreign).unwrap();
    fs::write(
        foreign.join("Foreign.csproj"),
        format!(
            r#"
        <Project Sdk="Microsoft.NET.Sdk">
          <PropertyGroup><TargetFramework>net{major}.0</TargetFramework></PropertyGroup>
          <ItemGroup>
            <Reference Include="Bridge"><HintPath>../lib/Bridge.dll</HintPath></Reference>
          </ItemGroup>
        </Project>
        "#
        ),
    )
    .unwrap();
    fs::write(
        foreign.join("ForeignCounter.cs"),
        r#"
        namespace Samples.Interop;
        public sealed class ForeignCounter(int value) : Counter(value)
        {
            public Counter Inner { get; set; } = new Counter(value);
        }
        "#,
    )
    .unwrap();
    successful(
        Command::new("dotnet")
            .current_dir(&foreign)
            .args(["build", "--nologo", "--verbosity", "quiet", "-o", "../lib"])
            .output()
            .unwrap(),
    );
    let foreign_bytes = fs::read(cwd.join("lib/Foreign.dll")).unwrap();
    references.add(&foreign_bytes).unwrap();
    let source = include_str!("../../../examples/interop/main.oxel")
        .replace("counter.Copy().Read()", "Program.transform(counter).Read()")
        .replace(
            "new Samples.Interop.Counter(30).Read()",
            r#"{
                let foreign = new Samples.Interop.ForeignCounter(20);
                foreign.Inner = new Samples.Interop.Counter(30);
                foreign.Value = foreign.Inner.Value;
                let mut view: Samples.Interop.IReadable = counter;
                view = foreign;
                let a = if true { foreign } else { counter };
                let b = if false { counter } else { a };
                let optional: Samples.Interop.ForeignCounter? = foreign;
                if optional != null { optional.Add(0); }
                let erased: object = foreign;
                let restored = erased as Samples.Interop.ForeignCounter;
                let base: Samples.Interop.Counter = restored;
                let derived = base as Samples.Interop.ForeignCounter;
                view.Read() + derived.Read()
            }"#,
        );
    let source = source.replace(
        "System.Console.WriteLine(Samples.Interop.CounterLabel.Reads);",
        r#"System.Console.WriteLine(Samples.Interop.CounterLabel.Reads);
        Samples.Interop.CounterLabel.Last = 23;
        System.Console.WriteLine(Samples.Interop.CounterLabel.ReadLast());
        (doubled.Copy()).Value = 8;"#,
    );
    let mut reversed = oxel::References::default();
    reversed.add(&factory).unwrap();
    reversed.add(&foreign_bytes).unwrap();
    assert!(oxel::check_with_references(&source, &reversed).is_err());
    reversed.add(&bridge_bytes).unwrap();
    let compiled = oxel::compile_with_references(&source, "main", &references).unwrap();
    assert_eq!(
        compiled,
        oxel::compile_with_references(&source, "main", &reversed).unwrap(),
    );
    fs::write(cwd.join("main.oxel"), source).unwrap();
    let args = [
        "main.oxel",
        "--reference",
        "lib/Factory.dll",
        "--reference",
        "lib/Foreign.dll",
        "--reference",
        "lib/Bridge.dll",
    ];
    let run = |action| {
        Command::new(env!("CARGO_BIN_EXE_oxel"))
            .current_dir(cwd)
            .arg(action)
            .args(args)
            .output()
            .unwrap()
    };
    successful(run("check"));
    let expected = [
        "30",
        "30",
        "3000000030",
        "30",
        "30",
        "30",
        "class",
        "counter",
        "60",
        "11",
        "12",
        "23",
    ];
    assert_eq!(successful(run("run")).lines().collect::<Vec<_>>(), expected);
    let output = successful(run("compile"));
    let assembly = cwd.join(output.trim());
    assert_eq!(fs::read(&assembly).unwrap(), compiled);
    let output = Command::new("dotnet").arg(assembly).output().unwrap();
    assert_eq!(successful(output).lines().collect::<Vec<_>>(), expected);
}

#[test]
fn enforces_generic_bounds_and_dispatches_for_clr_structs_and_classes() {
    let folder = tempfile::tempdir().unwrap();
    let cwd = folder.path();
    let major = build_counter_bridge(cwd);
    let bridge = fs::read(cwd.join("lib/Bridge.dll")).unwrap();
    let mut references = oxel::References::default();
    references.add(&bridge).unwrap();

    for (snippet, expected) in [
        ("fn f<T: i32>(x: T) {} fn main() {}", "E0211"),
        (
            "fn f<T: Samples.Interop.IReadable + Samples.Interop.IReadable>(x: T) {} fn main() {}",
            "E0211",
        ),
        (
            "fn f<T: Samples.Interop.Counter + Samples.Interop.FixedCounter>(x: T) {} fn main() {}",
            "E0211",
        ),
        (
            "fn f<T: Samples.Interop.IReadable>(x: T) {} fn main() { f(1); }",
            "E0212",
        ),
        (
            "fn f<T: Samples.Interop.IReadable>(x: T) {} fn main() { f::<bool>(true); }",
            "E0212",
        ),
        (
            "fn f<T>(x: T) { Samples.Interop.CounterLabel.ReadBound(x); } fn main() {}",
            "E0212",
        ),
        (
            "fn f<T: Samples.Interop.IReadable>(x: T) { Samples.Interop.CounterLabel.Keep(x); } fn main() {}",
            "E0212",
        ),
        (
            "fn main() { Samples.Interop.CounterLabel.Keep::<Samples.Interop.FixedCounter>(new Samples.Interop.FixedCounter()); }",
            "E0212",
        ),
    ] {
        let errors = oxel::check_with_references(snippet, &references).expect_err(snippet);
        assert_eq!(errors[0].code, expected, "{snippet}: {errors:?}");
    }

    let mut source = include_str!("../../../examples/interop/constraints.oxel").replace(
        "System.Console.WriteLine(second.Value);",
        r#"System.Console.WriteLine(second.Value);
    System.Console.WriteLine(Samples.Interop.CounterLabel.Rank(first, 1));
    System.Console.WriteLine(Samples.Interop.CounterLabel.Rank(false, 1));"#,
    );
    source.push_str(
        r#"
        fn optional_readable<T: Samples.Interop.IReadable>(value: T) -> Samples.Interop.IReadable? {
            value
        }
        fn add_to_property<T: Samples.Interop.IWritable>(value: T, replacement: T) -> i32 {
            let mut local = value;
            local.Value += { local = replacement; 9 };
            local.Value
        }
        "#,
    );
    let app = oxel::compile_with_references(&source, "Bounds", &references).unwrap();
    fs::write(cwd.join("lib/Bounds.dll"), &app).unwrap();
    fs::write(cwd.join("bounds.oxel"), &source).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_oxel"))
        .current_dir(cwd)
        .args(["run", "bounds.oxel", "--reference", "lib/Bridge.dll"])
        .output()
        .unwrap();
    assert_eq!(
        successful(output).lines().collect::<Vec<_>>(),
        ["30", "30", "42", "10", "constrained", "open"]
    );

    let mut reversed = oxel::References::default();
    reversed.add(&app).unwrap();
    let downstream = "fn main() { Program.read(new Samples.Interop.Counter(7)); }";
    assert!(oxel::check_with_references(downstream, &reversed).is_err());
    reversed.add(&bridge).unwrap();
    oxel::check_with_references(downstream, &reversed).unwrap();
    let errors =
        oxel::check_with_references("fn main() { Program.read(7); }", &reversed).unwrap_err();
    assert_eq!(errors[0].code, "E0212");

    let consumer = cwd.join("consumer");
    fs::create_dir(&consumer).unwrap();
    fs::write(
        consumer.join("Consumer.csproj"),
        format!(
            r#"
        <Project Sdk="Microsoft.NET.Sdk">
          <PropertyGroup>
            <TargetFramework>net{major}.0</TargetFramework>
            <OutputType>Exe</OutputType>
          </PropertyGroup>
          <ItemGroup>
            <Reference Include="Bounds"><HintPath>../lib/Bounds.dll</HintPath></Reference>
            <Reference Include="Bridge"><HintPath>../lib/Bridge.dll</HintPath></Reference>
          </ItemGroup>
        </Project>
        "#
        ),
    )
    .unwrap();
    fs::write(
        consumer.join("Harness.cs"),
        r#"
        using Samples.Interop;
        public struct ValueCounter : IReadable, IWritable {
            public int Value { get; set; }
            public int Read() => Value;
        }
        public static class Harness {
            public static void Main() {
                var value = new ValueCounter { Value = 3 };
                System.Console.WriteLine(Program.edit(value, 70));
                System.Console.WriteLine(Program.edit_local(value, 13));
                System.Console.WriteLine(Program.as_readable(value).Value);
                System.Console.WriteLine(Program.read(new Counter(9)));
                System.Console.WriteLine(Program.optional_readable(value).Value);
                System.Console.WriteLine(Program.add_to_property(value, new ValueCounter { Value = 8 }));
                var first = new Counter(3);
                var second = new Counter(8);
                System.Console.WriteLine(Program.add_to_property(first, second));
                System.Console.WriteLine(first.Value);
            }
        }
        "#,
    )
    .unwrap();
    let result = Command::new("dotnet")
        .current_dir(consumer)
        .args(["run", "--verbosity", "quiet"])
        .output()
        .unwrap();
    assert_eq!(
        successful(result).lines().collect::<Vec<_>>(),
        ["70", "13", "3", "9", "3", "12", "8", "12"]
    );
}

#[test]
fn exchanges_nullable_signature_metadata_with_csharp() {
    let folder = tempfile::tempdir().unwrap();
    let cwd = folder.path();
    let major = successful(Command::new("dotnet").arg("--version").output().unwrap())
        .split('.')
        .next()
        .unwrap()
        .parse::<u32>()
        .unwrap();
    fs::write(
        cwd.join("Annotations.csproj"),
        format!(
            r#"
        <Project Sdk="Microsoft.NET.Sdk">
          <PropertyGroup><TargetFramework>net{major}.0</TargetFramework></PropertyGroup>
        </Project>
        "#
        ),
    )
    .unwrap();
    fs::write(
        cwd.join("Annotations.cs"),
        r#"
        #nullable enable
        namespace Boundary;

        public class Reader {
            public string? Status { get; set; }
            public virtual string? Read(string? value) => value;
            public virtual Packet<string?> ReadPacket() => new(null);
        }

        public sealed class RequiredReader : Reader {
            public override string Read(string? value) => value ?? "ready";
            public override Packet<string> ReadPacket() => new("checked override");
        }

        public struct Packet<T> {
            public T Value;
            public Packet(T value) => Value = value;
        }

        public class RefCell<T> where T : class { }

        public class OptionalCell<T> {
            public T? Field = default;
            public T? Value => Field;
            public T?[] Values() => new[] { Field };
        }

        public static class Api {
            public static string? Mix(int number, string? value, bool selected, string fallback)
                => selected ? value : fallback;
            public static string Required(string value) => value;
            public static T Identity<T>(T value) => value;
            public static Packet<string?> Empty() => new(null);
            public static Packet<string?> Wrap(string? value) => new(value);
            public static Reader? Optional(bool present) => present ? new Reader() : null;
            public static T? Annotated<T>(T? value) => value;
            public static T Pick<T>(T? first, T second) => second;
            public static T? Select<T>(T value, int tag) => value;
            public static T Select<T>(T value, object tag) => value;
            public static string Receive(string? value) => value ?? "missing";
            public static string Receive(object value) => "boxed";
            public static string Collect(params string?[] values) => values[0] ?? "fallback";
        }
        "#,
    )
    .unwrap();
    successful(
        Command::new("dotnet")
            .current_dir(cwd)
            .args(["build", "--nologo", "--verbosity", "quiet", "-o", "lib"])
            .output()
            .unwrap(),
    );
    let external = fs::read(cwd.join("lib/Annotations.dll")).unwrap();
    let mut references = oxel::References::default();
    references.add(&external).unwrap();
    for (source, code) in [
        (
            r#"fn main() { let x: string = Boundary.Api.Mix(0, null, true, "fallback"); }"#,
            "E0301",
        ),
        ("fn main() { Boundary.Api.Required(null); }", "E0301"),
        (
            "fn main() { Boundary.Api.Optional(false).Read(null); }",
            "E0311",
        ),
        (
            "fn main() { let item = new Boundary.Reader(); let x: string = item.Status; }",
            "E0301",
        ),
        (
            r#"fn main() { let value: string = Boundary.Api.Annotated("x"); }"#,
            "E0301",
        ),
        (
            "fn main() { Boundary.Api.Annotated::<i32>(null); }",
            "E0301",
        ),
        (
            "fn forward<T>(value: T) { Boundary.Api.Annotated(value); } fn main() {}",
            "E0317",
        ),
        (
            "fn main() { let value: string = new Boundary.OptionalCell<string>().Field; }",
            "E0301",
        ),
        (
            "fn main() { let text: string = Boundary.Api.Empty().Value; }",
            "E0301",
        ),
        (
            "fn main() { let packet: Boundary.Packet<string> = Boundary.Api.Empty(); }",
            "E0301",
        ),
        (
            "fn main() { let item: string = Boundary.Api.Identity::<string?>(null); }",
            "E0301",
        ),
        ("fn main() { new Boundary.RefCell<string?>(); }", "E0212"),
        (
            "fn keep<T: class>(value: T) -> T { value } fn main() { keep::<string?>(null); }",
            "E0212",
        ),
        (
            "fn main() { let value: Option<string> = Option.None; Boundary.Api.Required(value); }",
            "E0301",
        ),
        (
            "fn main() { let value: Option<string?> = Option.Some(null); let raw: string? = value; }",
            "E0301",
        ),
    ] {
        let errors = oxel::check_with_references(source, &references).expect_err(source);
        assert_eq!(errors[0].code, code, "{source}: {errors:?}");
    }

    for initializer in [
        r#"Boundary.Api.Annotated("value")"#,
        "new Boundary.OptionalCell<string>().Field",
        "new Boundary.OptionalCell<string>().Value",
    ] {
        let source =
            format!("fn main() {{ let value = {initializer}; let required: string = value; }}");
        let errors = oxel::check_with_references(&source, &references).unwrap_err();
        assert_eq!(
            errors[0].type_names(),
            Some(("System.String", "Option<System.String>"))
        );
        let source = format!(
            "fn main() {{ let value: string? = {initializer}; if value != null {{ Boundary.Api.Required(value); }} }}"
        );
        oxel::check_with_references(&source, &references).unwrap();
    }

    let source = r#"
        struct Cell<T> { value: T }
        struct Text { optional: string?, required: string }
        enum Option<T> { Some(T), None }
        fn echo(value: string?, required: string) -> string? { value }
        fn wrap(cell: Cell<string>) -> Cell<string> { cell }
        fn wrap_nullable(cell: Cell<string?>) -> Cell<string?> { cell }
        fn cross(values: Boundary.Packet<string?>[]) -> Boundary.Packet<string?>[] { values }
        fn identity<T>(value: T) -> T { value }
        fn forward<T>(value: T) -> T { Boundary.Api.Select::<T>(value, 0) }
        fn recover(value: Option<string?>) -> Option<string?> { try { value? } }
        fn main() {
            System.Console.WriteLine(Boundary.Api.Mix(0, null, true, "fallback") ?? "missing");
            let item = new Boundary.Reader();
            item.Status = null;
            System.Console.WriteLine(item.Status ?? "unset");
            let reader = Boundary.Api.Optional(false) ?? new Boundary.RequiredReader();
            System.Console.WriteLine(reader.Read(null) ?? "fallback");
            let refined: string = Boundary.Api.Identity(new Boundary.RequiredReader().Read(null));
            System.Console.WriteLine(refined);
            let empty = Boundary.Api.Empty();
            let value = Boundary.Api.Identity(empty.Value) ?? "imported";
            System.Console.WriteLine(value);
            let wrapped = wrap_nullable(Cell::<string?> { value: null });
            System.Console.WriteLine(identity(wrapped.value) ?? "inferred");
            System.Console.WriteLine(identity::<string?>(null) ?? "explicit");
            let result = recover(Option.Some::<string?>(null));
            match result {
                Some(text) => System.Console.WriteLine(text ?? "some null"),
                None => System.Console.WriteLine("none"),
            }
            System.Console.WriteLine(cross([Boundary.Api.Wrap(null)])[0].Value ?? "nested");
            let required: string = new Boundary.RequiredReader().ReadPacket().Value;
            System.Console.WriteLine(required);
            let annotated = Boundary.Api.Annotated::<string>(null);
            System.Console.WriteLine(annotated ?? "annotated");
            let number: i32 = Boundary.Api.Annotated(30);
            System.Console.WriteLine(number);
            let text: string? = null;
            let picked: string = Boundary.Api.Pick(text, "picked");
            System.Console.WriteLine(picked);
            System.Console.WriteLine(forward("fallback overload"));
            let cell = new Boundary.OptionalCell<string>();
            cell.Field = "field";
            System.Console.WriteLine(cell.Field ?? "empty");
            cell.Field = null;
            System.Console.WriteLine(cell.Value ?? "property");
            System.Console.WriteLine(cell.Values()[0] ?? "element");
            let zero: i32 = new Boundary.OptionalCell<i32>().Value;
            System.Console.WriteLine(zero);
        }
    "#;
    let artifact = oxel::compile_with_references(source, "NullableExports", &references).unwrap();
    fs::write(cwd.join("lib/NullableExports.dll"), &artifact).unwrap();
    fs::write(cwd.join("nullable.oxel"), source).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_oxel"))
        .current_dir(cwd)
        .args(["run", "nullable.oxel", "--reference", "lib/Annotations.dll"])
        .output()
        .unwrap();
    assert_eq!(
        successful(output).lines().collect::<Vec<_>>(),
        [
            "missing",
            "unset",
            "ready",
            "ready",
            "imported",
            "inferred",
            "explicit",
            "some null",
            "nested",
            "checked override",
            "annotated",
            "30",
            "picked",
            "fallback overload",
            "field",
            "property",
            "element",
            "0",
        ]
    );

    let option_source = r#"
        fn main() {
            let value: Option<string> = Option.Some("bridged");
            let missing: Option<string> = Option.None;
            System.Console.WriteLine(Boundary.Api.Receive(value));
            System.Console.WriteLine(Boundary.Api.Receive(missing));
            System.Console.WriteLine(Boundary.Api.Collect(missing, value));
            System.Console.WriteLine((missing as string?) == null);
        }
    "#;
    fs::write(cwd.join("option.oxel"), option_source).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_oxel"))
        .current_dir(cwd)
        .args(["run", "option.oxel", "--reference", "lib/Annotations.dll"])
        .output()
        .unwrap();
    assert_eq!(
        successful(output).lines().collect::<Vec<_>>(),
        ["bridged", "missing", "fallback", "True"]
    );

    references.add(&artifact).unwrap();
    oxel::check_with_references(
        r#"fn main() { let result: string? = Program.echo(null, "required"); }"#,
        &references,
    )
    .unwrap();
    let errors = oxel::check_with_references(
        r#"fn main() { let result: string = Program.echo(null, "required"); }"#,
        &references,
    )
    .unwrap_err();
    assert_eq!(
        errors[0].type_names(),
        Some(("System.String", "System.String?"))
    );

    let consumer = cwd.join("consumer");
    fs::create_dir(&consumer).unwrap();
    fs::write(
        consumer.join("Consumer.csproj"),
        format!(r#"
        <Project Sdk="Microsoft.NET.Sdk">
          <PropertyGroup>
            <TargetFramework>net{major}.0</TargetFramework>
            <OutputType>Exe</OutputType>
            <Nullable>enable</Nullable>
            <TreatWarningsAsErrors>true</TreatWarningsAsErrors>
          </PropertyGroup>
          <ItemGroup>
            <Reference Include="NullableExports"><HintPath>../lib/NullableExports.dll</HintPath></Reference>
            <Reference Include="Annotations"><HintPath>../lib/Annotations.dll</HintPath></Reference>
          </ItemGroup>
        </Project>
        "#),
    ).unwrap();
    fs::write(
        consumer.join("Harness.cs"),
        r#"
        using System;
        using System.Reflection;
        using Boundary;
        public static class Harness {
            public static void Main() {
                string? result = Program.echo(null, "text");
                Console.WriteLine(result is null);
                var ctx = new NullabilityInfoContext();
                var method = typeof(Program).GetMethod("echo")!;
                var args = method.GetParameters();
                Console.WriteLine(ctx.Create(args[0]).WriteState);
                Console.WriteLine(ctx.Create(args[1]).WriteState);
                Console.WriteLine(ctx.Create(method.ReturnParameter).ReadState);
                Console.WriteLine(ctx.Create(typeof(Text).GetField("optional")!).ReadState);
                Console.WriteLine(ctx.Create(typeof(Text).GetField("required")!).ReadState);
                var nested = typeof(Program).GetMethod("wrap")!;
                Console.WriteLine(ctx.Create(nested.ReturnParameter).GenericTypeArguments[0].ReadState);
                var optional = typeof(Program).GetMethod("wrap_nullable")!;
                Console.WriteLine(ctx.Create(optional.ReturnParameter).GenericTypeArguments[0].ReadState);
                var cross = typeof(Program).GetMethod("cross")!;
                var element = ctx.Create(cross.ReturnParameter).ElementType!;
                Console.WriteLine(element.GenericTypeArguments[0].ReadState);
                var packets = Program.cross(new Packet<string?>[] { new(null) });
                Console.WriteLine(packets[0].Value is null);
            }
        }
        "#,
    ).unwrap();
    let result = Command::new("dotnet")
        .current_dir(&consumer)
        .args(["run", "--verbosity", "quiet"])
        .output()
        .unwrap();
    assert_eq!(
        successful(result).lines().collect::<Vec<_>>(),
        [
            "True", "Nullable", "NotNull", "Nullable", "Nullable", "NotNull", "NotNull",
            "Nullable", "Nullable", "True"
        ]
    );
    fs::write(
        consumer.join("Harness.cs"),
        r#"
        public static class Harness {
            public static void Main() {
                string required = Program.echo(null, null);
                System.Console.WriteLine(required);
            }
        }
        "#,
    )
    .unwrap();
    let output = Command::new("dotnet")
        .current_dir(&consumer)
        .args(["build", "--nologo", "--verbosity", "quiet"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let diagnostics = String::from_utf8_lossy(&output.stdout);
    assert!(diagnostics.contains("CS8600"), "{diagnostics}");
    assert!(diagnostics.contains("CS8625"), "{diagnostics}");
}

#[test]
fn constructs_imported_generic_classes_and_calls_specialized_members() {
    let folder = tempfile::tempdir().unwrap();
    let cwd = folder.path();
    let sdk = successful(Command::new("dotnet").arg("--version").output().unwrap());
    let major = sdk.split('.').next().unwrap();
    fs::write(cwd.join("Generic.csproj"), format!(
        r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFramework>net{major}.0</TargetFramework></PropertyGroup></Project>"#
    )).unwrap();
    fs::write(
        cwd.join("Types.cs"),
        r#"
        #nullable enable
        namespace Fixture;
        public class Base { public int Offset() => 1; }
        public class Box<T> : Base {
            public Box(T value) { Value = value; }
            public T Value { get; set; }
            public virtual T Exchange(T value) { T old = Value; Value = value; return old; }
            public Box<T> Copy(Box<T> other) => new(other.Value);
            public Pair<T, string> WithLabel(string label) => new(Value, label);
            public Pair<T, U> Tag<U>(T value, U label) => new(value, label);
            public Pair<T, U> With<U>(U label) => new(Value, label);
            public string Select(int value) => "concrete";
            public string Select<U>(U value) => "generic";
            public U Restricted<U>(U value) where U : Base => value;
            public U Take<U>(Box<U> value) => value.Value;
            public Box<T>? Next { get; set; }
        }
        public class IntegerBox : Box<int> {
            public IntegerBox(int value) : base(value) {}
            public override int Exchange(int value) => base.Exchange(value) + 1;
        }
        public class Holder<T> : Box<Pair<T, string>> {
            public Holder(T value) : base(new Pair<T, string>(value, "tag")) {}
        }
        public class NullableBox : Box<string?> { public NullableBox() : base(null) {} }
        public interface ISlot<T> {
            T Value { get; set; }
            T Read();
            Pair<T, U> With<U>(U item);
        }
        public interface ILeft<T> : ISlot<T> {}
        public interface IRight<T> : ISlot<T> {}
        public interface IDiamond : ILeft<int>, IRight<int> {}
        public sealed class Slot : IDiamond {
            int ISlot<int>.Value { get; set; } = 29;
            int ISlot<int>.Read() => ((ISlot<int>)this).Value;
            Pair<int, U> ISlot<int>.With<U>(U item) => new(((ISlot<int>)this).Value, item);
        }
        public struct SlotValue : IDiamond {
            public SlotValue(int value) { Value = value; }
            public int Value { get; set; }
            public int Read() => Value;
            public Pair<int, U> With<U>(U item) => new(Value, item);
        }
        public interface IAmbiguous : ILeft<int>, ILeft<string> {}
        public static class Factory {
            public static Box<int> Make(int value) => new(value);
            public static int Read(Box<int> box) => box.Value;
            public static Box<T> Identity<T>(Box<T> value) => value;
            public static Box<T> Replace<T>(Box<T> value, T replacement) => new(replacement);
            public static Pair<Box<T>, string> Nested<T>(Box<T> value) => new(value, "nested");
            public static Box<string>? None() => null;
            public static Box<string?> NullableArgument() => new(null);
        }
        public class Envelope {
            public Envelope(Box<string> value) => Item = value;
            public Box<string> Item { get; }
        }
        public class Pair<A, B> {
            public Pair(A left, B right) { Left = left; Right = right; }
            public A Left { get; }
            public B Right { get; }
        }
        public class Same { public Same() {} public int Read() => 20; }
        public class Same<T> { public Same(T value) { Value = value; } public T Value { get; } }
        public class Restricted<T> where T : class { public Restricted(T value) {} }
        public interface IValue<T> { T Read(); }
    "#,
    )
    .unwrap();
    successful(
        Command::new("dotnet")
            .current_dir(cwd)
            .args(["build", "--nologo", "-v", "quiet", "-o", "lib"])
            .output()
            .unwrap(),
    );
    let path = cwd.join("lib/Generic.dll");
    let mut references = oxel::References::default();
    references.add(&fs::read(&path).unwrap()).unwrap();

    // Rename and find references need source type arguments even under CLR owners.
    let generic = "fn keep<T>(value: Fixture.Box<T>?) -> Fixture.Box<T>? { value }";
    let analysis = oxel::inspect_document(generic, &references);
    assert!(
        analysis.diagnostics.is_empty(),
        "{:?}",
        analysis.diagnostics
    );
    let declaration = generic.find("<T>").unwrap() + 1;
    let linked: Vec<_> = analysis
        .occurrences
        .iter()
        .filter(|item| {
            item.definition
                .is_some_and(|span| span.start == declaration)
        })
        .map(|item| item.span.start)
        .collect();
    assert_eq!(
        linked,
        generic
            .match_indices("<T>")
            .skip(1)
            .map(|(offset, _)| offset + 1)
            .collect::<Vec<_>>(),
    );

    let source = r#"
        struct Wrapped<T> { inner: Fixture.Box<T> }

        fn unpack<T>(value: Fixture.Box<T>) -> T { value.Value }
        fn wrap<T>(value: T) -> Wrapped<T> {
            Wrapped { inner: new Fixture.Box<T>(value) }
        }
        fn offset(value: Fixture.Base) -> i32 { value.Offset() }
        fn widen<T: Fixture.IDiamond>(value: T) -> Fixture.ISlot<i32> { value }
        fn apply<A, B>(box: Fixture.Box<B>, label: A) -> Fixture.Pair<B, A> {
            box.Tag::<A>(box.Value, label)
        }
        fn touch<T: Fixture.IDiamond>(view: T) -> i32 {
            view.With::<i32>(1).Left + view.With(1).Right
        }
        fn increment<T: Fixture.IDiamond>(value: T) -> i32 {
            value.Value = value.Value + 1;
            value.Read()
        }
        fn main() {
            let a = new Fixture.Box<i32>(20);
            a.Value = a.Value + 9;
            System.Console.WriteLine(a.Exchange(80) + offset(a));
            let b = new Fixture.Box<string>("before");
            System.Console.WriteLine(b.Exchange("after"));
            let item: object = b;
            System.Console.WriteLine((item as Fixture.Box<string>).Value);
            System.Console.WriteLine(item is Fixture.Box<i32>);
            let nullable: Fixture.Box<i32>? = null;
            let fallback = (nullable ?? a).Value;
            System.Console.WriteLine(fallback);
            let nested = new Fixture.Pair<Fixture.Box<string>, i64>(b, 3000000000);
            System.Console.WriteLine(nested.Left.Value);
            System.Console.WriteLine(nested.Right);
            System.Console.WriteLine(unpack(wrap(true).inner));
            System.Console.WriteLine(new Fixture.Same().Read() + new Fixture.Same<i32>(10).Value);
            let made = Fixture.Factory.Make(30);
            System.Console.WriteLine(Fixture.Factory.Read(made));
            a.Next = made.Copy(made);
            System.Console.WriteLine((a.Next ?? a).Value);
            System.Console.WriteLine(made.WithLabel("label").Right);
            let inferred = Fixture.Factory.Identity(made);
            System.Console.WriteLine(inferred.Value);
            let nested_factory = Fixture.Factory.Nested(Fixture.Factory.Identity::<string>(b));
            System.Console.WriteLine(nested_factory.Left.Value + nested_factory.Right);
            let long = new Fixture.Box<i64>(3);
            System.Console.WriteLine(Fixture.Factory.Replace(long, 3000000000).Value);
            System.Console.WriteLine((Fixture.Factory.None() ?? b).Value);
            System.Console.WriteLine(new Fixture.Envelope(b).Item.Value);
            let derived = new Fixture.IntegerBox(29);
            System.Console.WriteLine(derived.Exchange(20));
            System.Console.WriteLine(Fixture.Factory.Identity(derived).Value);
            let holder = new Fixture.Holder<i32>(30);
            System.Console.WriteLine(holder.Value.Left);
            let view: Fixture.IDiamond = new Fixture.Slot();
            view.Value = 30;
            let widened: Fixture.ISlot<i32> = view;
            let root = widen(widened as Fixture.IDiamond);
            System.Console.WriteLine(root.Read());
            System.Console.WriteLine(increment(new Fixture.Slot()));
            System.Console.WriteLine(derived.With(false).Right);
            System.Console.WriteLine(apply(b, 30).Right);
            System.Console.WriteLine((new Fixture.Box<i32>(30)).With::<string>("explicit").Right);
            System.Console.WriteLine(a.Select(10));
            System.Console.WriteLine(a.Select::<bool>(true));
            System.Console.WriteLine(a.Restricted(new Fixture.Base()).Offset());
            System.Console.WriteLine(a.Take(b));
            System.Console.WriteLine(root.With(30).Right);
            System.Console.WriteLine(touch(new Fixture.Slot()));
            System.Console.WriteLine(Fixture.Factory.NullableArgument().Value ?? "empty");
        }
    "#;
    fs::write(cwd.join("main.oxel"), source).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_oxel"))
        .current_dir(cwd)
        .args(["run", "main.oxel", "--reference", "lib/Generic.dll"])
        .output()
        .unwrap();
    assert_eq!(
        successful(result).lines().collect::<Vec<_>>(),
        [
            "30",
            "before",
            "after",
            "False",
            "80",
            "after",
            "3000000000",
            "True",
            "30",
            "30",
            "30",
            "label",
            "30",
            "afternested",
            "3000000000",
            "after",
            "after",
            "30",
            "20",
            "30",
            "30",
            "30",
            "False",
            "30",
            "explicit",
            "concrete",
            "generic",
            "1",
            "after",
            "30",
            "30",
            "empty"
        ]
    );
    assert!(
        oxel::check_with_references(
            "fn inspect(value: Fixture.IValue<i32>) -> i32 { value.Read() } fn main() {}",
            &references,
        )
        .is_ok()
    );

    // C# can use the generic signatures emitted by oxel.
    let managed = oxel::compile_project(&[source], "CompiledConsumer", &references).unwrap();
    let consumer = cwd.join("consumer");
    fs::create_dir(&consumer).unwrap();
    fs::write(consumer.join("CompiledConsumer.dll"), managed).unwrap();
    fs::write(consumer.join("Consumer.csproj"), format!(r#"
        <Project Sdk="Microsoft.NET.Sdk">
          <PropertyGroup><TargetFramework>net{major}.0</TargetFramework><OutputType>Exe</OutputType></PropertyGroup>
          <ItemGroup>
            <Reference Include="Generic"><HintPath>../lib/Generic.dll</HintPath></Reference>
            <Reference Include="CompiledConsumer"><HintPath>CompiledConsumer.dll</HintPath></Reference>
          </ItemGroup>
        </Project>
    "#)).unwrap();
    fs::write(
        consumer.join("Caller.cs"),
        r#"
        public static class Across {
            public static Fixture.Box<int> Make() => new(30);
        }
        public class Outside : Fixture.Box<int> {
            public Outside(int value) : base(value) {}
        }
        public static class Caller {
            public static void Main() {
                var boxed = Program.wrap<int>(30).inner;
                System.Console.WriteLine(Program.unpack<int>(boxed));
                System.Console.WriteLine(Program.increment(new Fixture.SlotValue(29)));
                System.Console.WriteLine(Program.widen(new Fixture.SlotValue(30)).Read());
                System.Console.WriteLine(Program.touch(new Fixture.SlotValue(29)));
            }
        }
    "#,
    )
    .unwrap();
    let result = Command::new("dotnet")
        .current_dir(&consumer)
        .args(["run", "--project", "Consumer.csproj"])
        .output()
        .unwrap();
    assert_eq!(successful(result).trim(), "30\n30\n30\n30");

    let later = consumer.join(format!("bin/Debug/net{major}.0/Consumer.dll"));
    let mut reversed = oxel::References::default();
    reversed.add(&fs::read(&later).unwrap()).unwrap();
    reversed.add(&fs::read(&path).unwrap()).unwrap();
    let cross_source = "fn main() { let value: Fixture.Box<i32> = Across.Make(); System.Console.WriteLine(value.Value + new Outside(0).Value); }";
    assert!(oxel::check_with_references(cross_source, &reversed).is_ok());
    fs::write(cwd.join("across.oxel"), cross_source).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_oxel"))
        .current_dir(cwd)
        .args(["run", "across.oxel", "--reference"])
        .arg(later)
        .arg("--reference")
        .arg(path)
        .output()
        .unwrap();
    assert_eq!(successful(result).trim(), "30");

    for (source, code) in [
        ("fn main() { new Fixture.Box(1); }", "E0202"),
        ("fn main() { new Fixture.Box<i32, i32>(1); }", "E0202"),
        ("fn main() { new Fixture.Box<i32>(true); }", "E0301"),
        (
            "fn main() { let x = new Fixture.Box<i32>(1); x.Value = false; }",
            "E0301",
        ),
        ("fn main() { new Fixture.Restricted<i32>(1); }", "E0212"),
        (
            "fn main() { let x: Fixture.Box<i32>? = null; x.Value; }",
            "E0311",
        ),
        (
            "fn main() { let x: Fixture.Box<i32> = new Fixture.Box<string>(\"a\"); }",
            "E0301",
        ),
        (
            r#"fn main() { Fixture.Factory.Read(new Fixture.Box<string>("wrong")); }"#,
            "E0301",
        ),
        ("fn main() { Fixture.Factory.None().Value; }", "E0311"),
        (
            "fn main() { let value: string = Fixture.Factory.NullableArgument().Value; }",
            "E0301",
        ),
        (
            "fn f(s: Fixture.IAmbiguous) { s.Read(); } fn main() {}",
            "E0208",
        ),
        (
            "fn f(s: Fixture.IAmbiguous) { s.Value; } fn main() {}",
            "E0208",
        ),
        (
            "fn f(s: Fixture.IDiamond) -> Fixture.ISlot<string> { s } fn main() {}",
            "E0301",
        ),
        (
            "fn f(s: Fixture.NullableBox) -> Fixture.Box<string> { s } fn main() {}",
            "E0301",
        ),
        (
            "fn main() { new Fixture.Box<i32>(1).With::<i32, bool>(true); }",
            "E0205",
        ),
        (
            "fn main() { new Fixture.Box<i32>(1).Restricted(0); }",
            "E0212",
        ),
        (
            "fn main() { (new Fixture.Box<i32>(1)).Tag::<bool>(false, true); }",
            "E0301",
        ),
    ] {
        let errors = oxel::check_with_references(source, &references).unwrap_err();
        assert_eq!(errors[0].code, code, "{source}: {errors:?}");
    }
}

#[test]
fn checks_and_executes_clr_variant_reference_conversions() {
    let folder = tempfile::tempdir().unwrap();
    let cwd = folder.path();
    let sdk = successful(Command::new("dotnet").arg("--version").output().unwrap());
    let major = sdk.split('.').next().unwrap();
    fs::write(
        cwd.join("Variance.csproj"),
        format!(r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFramework>net{major}.0</TargetFramework></PropertyGroup></Project>"#),
    ).unwrap();
    fs::write(cwd.join("Types.cs"), r#"
        #nullable enable
        namespace Variant;
        public interface IRead<out T> { T Read(); }
        public interface IWrite<in T> { void Write(T value); }
        public interface IConvert<in T, out U, V> { U Convert(T value, V marker); }
        public interface IBoth : IRead<string>, IRead<object> {}
        public delegate T ReadDelegate<out T>();
        public delegate void WriteDelegate<in T>(T value);

        public class Base { public int Number() => 30; }
        public class Child : Base { }
        public class Source<T> : IRead<T> {
            private readonly T value;
            public Source(T value) => this.value = value;
            public T Read() => value;
        }
        public sealed class TextSource : Source<string> {
            public TextSource() : base("text") { }
        }
        public sealed class Printer<T> : IWrite<T> {
            public void Write(T value) => System.Console.WriteLine(value);
        }
        public sealed class Converter : IConvert<Base, Child, int> {
            public Child Convert(Base value, int marker) => new();
        }
        public static class Factory {
            public static ReadDelegate<string> Reader() => () => "delegate result";
            public static WriteDelegate<object> Writer() => value => System.Console.WriteLine(value);
            public static string Pick(IRead<object> value) => "base";
            public static string Pick(IRead<string> value) => "specific";
        }
    "#).unwrap();
    successful(
        Command::new("dotnet")
            .current_dir(cwd)
            .args(["build", "--nologo", "-v", "quiet", "-o", "lib"])
            .output()
            .unwrap(),
    );
    let reference = cwd.join("lib/Variance.dll");
    let mut references = oxel::References::default();
    references.add(&fs::read(&reference).unwrap()).unwrap();
    let src = r#"
        fn as_base(value: Variant.IRead<Variant.Child>) -> Variant.IRead<Variant.Base> { value }
        fn choose(flag: bool, value: Variant.IRead<string>) -> Variant.IRead<object> {
            if flag { value } else { new Variant.Source<object>(17) }
        }
        fn main() {
            let text = new Variant.TextSource();
            let widened: Variant.IRead<object> = text;
            System.Console.WriteLine(widened.Read());
            let writer: Variant.IWrite<string> = new Variant.Printer<object>();
            writer.Write("sink");
            let child: Variant.IRead<Variant.Child> = new Variant.Source<Variant.Child>(new Variant.Child());
            System.Console.WriteLine(as_base(child).Read().Number());
            let convert: Variant.IConvert<Variant.Child, Variant.Base, i32> = new Variant.Converter();
            System.Console.WriteLine(convert.Convert(new Variant.Child(), 0).Number());
            let nested = new Variant.Source<Variant.IRead<string>>(text);
            let outer: Variant.IRead<Variant.IRead<object>> = nested;
            System.Console.WriteLine(outer.Read().Read());
            let read: Variant.ReadDelegate<object> = Variant.Factory.Reader();
            System.Console.WriteLine(read.Invoke());
            let write: Variant.WriteDelegate<string> = Variant.Factory.Writer();
            write.Invoke("delegate input");
            System.Console.WriteLine(Variant.Factory.Pick(text));
            System.Console.WriteLine(choose(false, text).Read());
            let nullable: Variant.IRead<string>? = null;
            let nullable_wide: Variant.IRead<object>? = nullable;
            System.Console.WriteLine((nullable_wide ?? widened).Read());
            let boxed: object = text;
            System.Console.WriteLine((boxed as Variant.IRead<object>).Read());
            let allows_null: Variant.IRead<object?> = text;
            System.Console.WriteLine(allows_null.Read() ?? "empty");
            let writes_nullable = new Variant.Printer<object?>();
            let sink: Variant.IWrite<string> = writes_nullable;
            sink.Write("nullable sink");
        }
    "#;
    fs::write(cwd.join("main.oxel"), src).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_oxel"))
        .current_dir(cwd)
        .args(["run", "main.oxel", "--reference", "lib/Variance.dll"])
        .output()
        .unwrap();
    assert_eq!(
        successful(output).lines().collect::<Vec<_>>(),
        [
            "text",
            "sink",
            "30",
            "30",
            "text",
            "delegate result",
            "delegate input",
            "specific",
            "17",
            "text",
            "text",
            "text",
            "nullable sink"
        ]
    );

    for (function, code) in [
        (
            "fn f(p: Variant.IRead<i32>) -> Variant.IRead<object> { p }",
            "E0301",
        ),
        (
            "fn f(p: Variant.IWrite<object>) -> Variant.IWrite<i32> { p }",
            "E0301",
        ),
        (
            "fn f(p: Variant.IRead<object>) -> Variant.IRead<string> { p }",
            "E0301",
        ),
        (
            "fn f(p: Variant.IWrite<string>) -> Variant.IWrite<object> { p }",
            "E0301",
        ),
        (
            "fn f(p: Variant.Source<string>) -> Variant.Source<object> { p }",
            "E0301",
        ),
        (
            "fn f(p: Variant.Converter) -> Variant.IConvert<Variant.Child, Variant.Base, i64> { p }",
            "E0301",
        ),
        (
            "fn f(p: Variant.IRead<Variant.IRead<i32>>) -> Variant.IRead<Variant.IRead<object>> { p }",
            "E0301",
        ),
        (
            "fn f(p: Variant.IRead<string>?) -> Variant.IRead<object> { p }",
            "E0301",
        ),
        (
            "fn f(p: Variant.IRead<string?>) -> Variant.IRead<object> { p }",
            "E0301",
        ),
        (
            "fn f(p: Variant.IWrite<object>) -> Variant.IWrite<string?> { p }",
            "E0301",
        ),
        (
            "fn f(p: Variant.Source<string>) -> Variant.Source<string?> { p }",
            "E0301",
        ),
        ("fn f(p: Variant.IBoth) { p.Read(); }", "E0208"),
    ] {
        let source = format!("{function} fn main() {{}}");
        let errors = oxel::check_with_references(&source, &references).unwrap_err();
        assert_eq!(errors[0].code, code, "{source}: {errors:?}");
    }
}

#[test]
fn calls_static_members_on_constructed_clr_classes() {
    let folder = tempfile::tempdir().unwrap();
    let cwd = folder.path();
    let sdk = successful(Command::new("dotnet").arg("--version").output().unwrap());
    let major = sdk.split('.').next().unwrap();
    fs::write(cwd.join("Generic.csproj"), format!(
        r#"<Project Sdk="Microsoft.NET.Sdk"><PropertyGroup><TargetFramework>net{major}.0</TargetFramework></PropertyGroup></Project>"#
    )).unwrap();
    fs::write(
        cwd.join("Cell.cs"),
        include_str!("../../../examples/interop/Cell.cs"),
    )
    .unwrap();
    successful(
        Command::new("dotnet")
            .current_dir(cwd)
            .args(["build", "--nologo", "-v", "quiet", "-o", "lib"])
            .output()
            .unwrap(),
    );

    let source = format!(
        "{}\n{}",
        include_str!("../../../examples/interop/generic_classes.oxel").replacen(
            "fn main()",
            "fn example()",
            1,
        ),
        r#"
        fn main() {
            example();
            System.Console.WriteLine(Samples.Interop.CellDefaults::<string>.Initial.Value);
            let inferred = Samples.Interop.CellDefaults::<i64>.Choose(32, "inferred");
            System.Console.WriteLine(inferred);
            System.Console.WriteLine(Samples.Interop.CellDefaults::<i64>.Initial.Value);
        }
        "#,
    );
    fs::write(cwd.join("main.oxel"), &source).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_oxel"))
        .current_dir(cwd)
        .args(["run", "main.oxel", "--reference", "lib/Generic.dll"])
        .output()
        .unwrap();
    assert_eq!(
        successful(result).lines().collect::<Vec<_>>(),
        ["30", "hello", "3000000000", "hello", "inferred", "32"]
    );

    let mut references = oxel::References::default();
    references
        .add(&fs::read(cwd.join("lib/Generic.dll")).unwrap())
        .unwrap();
    for (program, code) in [
        (
            r#"fn main() { Samples.Interop.Cell::<i32>.From("wrong"); }"#,
            "E0301",
        ),
        (
            r#"fn main() { Samples.Interop.Cell::<i32>.Value = 30; }"#,
            "E0305",
        ),
        (
            r#"fn main() { Samples.Interop.Cell::<i32>.Read(); }"#,
            "E0203",
        ),
        (
            r#"fn main() { Samples.Interop.CellDefaults::<i32>.Initial = new Samples.Interop.Cell<string>("wrong"); }"#,
            "E0301",
        ),
        (
            r#"fn main() { Samples.Interop.CellDefaults::<i32, i64>.Initial; }"#,
            "E0202",
        ),
        (
            r#"fn main() { let value: Samples.Interop.CellDefaults<i32>; }"#,
            "E0202",
        ),
        (
            r#"fn main() { new Samples.Interop.CellDefaults<i32>(); }"#,
            "E0202",
        ),
    ] {
        let errors = oxel::check_with_references(program, &references).unwrap_err();
        assert_eq!(errors[0].code, code, "{program}: {errors:?}");
    }
}

#[test]
fn value_task_entry_waits_for_a_pending_clr_source() {
    let folder = tempfile::tempdir().unwrap();
    let project = folder.path();
    let sdk = successful(Command::new("dotnet").arg("--version").output().unwrap());
    let reference = build_reference_fixture(
        &project.join("fixture"),
        sdk.split('.').next().unwrap(),
        "TaskBridge",
        "1.0.0.0",
        r#"
        using System;
        using System.Threading;
        using System.Threading.Tasks;
        using System.Threading.Tasks.Sources;

        public sealed class TaskBridge : IValueTaskSource<int> {
            private ManualResetValueTaskSourceCore<int> source;

            public static ValueTask<int> Start() {
                var pending = new TaskBridge();
                return new ValueTask<int>(pending, pending.source.Version);
            }

            public int GetResult(short token) => source.GetResult(token);
            public ValueTaskSourceStatus GetStatus(short token) => source.GetStatus(token);

            public void OnCompleted(Action<object?> continuation, object? state,
                                    short token, ValueTaskSourceOnCompletedFlags flags) {
                source.OnCompleted(continuation, state, token, flags);
                ThreadPool.QueueUserWorkItem(_ => source.SetResult(42));
            }
        }
        "#,
        None,
    );
    fs::write(
        project.join("main.oxel"),
        "fn main() -> System.Threading.Tasks.ValueTask<i32> { TaskBridge.Start() }",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_oxel"))
        .current_dir(project)
        .args(["run", "--framework", "main.oxel", "--reference"])
        .arg(reference)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(42), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
}

fn build_reference_fixture(
    directory: &std::path::Path,
    sdk_major: &str,
    name: &str,
    version: &str,
    source: &str,
    dependency: Option<(&str, &std::path::Path)>,
) -> std::path::PathBuf {
    fs::create_dir(directory).unwrap();
    let reference = dependency.map_or_else(String::new, |(name, path)| {
        format!(
            r#"<ItemGroup><Reference Include="{name}"><HintPath>{}</HintPath></Reference></ItemGroup>"#,
            path.display(),
        )
    });
    fs::write(
        directory.join("Fixture.csproj"),
        format!(
            r#"<Project Sdk="Microsoft.NET.Sdk">
                <PropertyGroup>
                    <TargetFramework>net{sdk_major}.0</TargetFramework>
                    <AssemblyName>{name}</AssemblyName>
                    <AssemblyVersion>{version}</AssemblyVersion>
                    <Nullable>enable</Nullable>
                </PropertyGroup>
                {reference}
            </Project>"#
        ),
    )
    .unwrap();
    fs::write(directory.join("Types.cs"), source).unwrap();
    successful(
        Command::new("dotnet")
            .current_dir(directory)
            .args(["build", "--nologo", "-v", "quiet", "-o", "lib"])
            .output()
            .unwrap(),
    );
    directory.join("lib").join(format!("{name}.dll"))
}

#[test]
fn comptime_type_edits_only_recompute_dependent_initializers() {
    use std::sync::Arc;

    let folder = tempfile::tempdir().unwrap();
    let sdk = successful(Command::new("dotnet").arg("--version").output().unwrap());
    let assembly = build_reference_fixture(
        &folder.path().join("fixture"),
        sdk.split('.').next().unwrap(),
        "ConstantsBridge",
        "1.0.0.0",
        "public static class Shell { public const int Ready = 30; }",
        None,
    );
    let mut references = oxel::References::default();
    references.add(&fs::read(assembly).unwrap()).unwrap();
    let declarations = concat!(
        "struct Shell { inner: Payload } ",
        "struct Payload { item: i32 } ",
        "struct Unrelated { spare: i32 }",
    );
    let program = concat!(
        "comptime fn first(n: i32) -> i32 { if n == 0 { 30 } else { second(n - 1) } }\n",
        "comptime fn second(n: i32) -> i32 { if false { Shell.Ready; } first(n) }\n",
        "const TRACKED = first(2); const DERIVED = TRACKED + 1; const INDEPENDENT = 20; const DIRECT = Shell.Ready;\n",
        "fn main() { System.Console.WriteLine(DERIVED + INDEPENDENT); }",
    );
    let mut analyzer = oxel::ProjectAnalyzer::default();
    let mut sources: [Arc<str>; 2] = [Arc::from(declarations), Arc::from(program)];
    let initial = analyzer.inspect(&sources, &references);
    assert!(
        initial.iter().all(|file| file.diagnostics.is_empty()),
        "{initial:?}"
    );
    assert_eq!(analyzer.evaluated_constants(), 4);

    for (field, evaluations) in [("spare", 0), ("item", 2)] {
        sources[0] =
            Arc::from(sources[0].replace(&format!("{field}: i32"), &format!("{field}: bool")));
        let edited = analyzer.inspect(&sources, &references);
        assert_eq!(
            edited,
            oxel::inspect_project(&[&sources[0], program], &references)
        );
        assert!(edited.iter().all(|file| file.diagnostics.is_empty()));
        assert_eq!(analyzer.evaluated_constants(), evaluations);
        assert_eq!(analyzer.checked_functions(), 0);
    }

    sources[0] = Arc::from("enum Shell { Ready }");
    let shadowed = analyzer.inspect(&sources, &references);
    assert_eq!(
        shadowed,
        oxel::inspect_project(&[&sources[0], program], &references)
    );
    assert!(shadowed.iter().all(|file| file.diagnostics.is_empty()));
    assert!(
        shadowed[1]
            .occurrences
            .iter()
            .any(|item| item.detail == "const DIRECT: Shell")
    );
    assert_eq!(analyzer.evaluated_constants(), 2);
}

#[test]
fn enum_bitwise_operations_and_casts_preserve_backing_widths() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path();
    let sdk = successful(Command::new("dotnet").arg("--version").output().unwrap());
    let reference = build_reference_fixture(
        &project.join("fixture"),
        sdk.split('.').next().unwrap(),
        "FlagWidths",
        "1.0.0.0",
        r#"
        namespace Flags;
        [System.Flags] public enum Tiny : byte { None = 0, Read = 1, Write = 128 }
        [System.Flags] public enum Wide : ulong { None = 0, Low = 1, High = 1UL << 63 }
        public enum Signed : sbyte { Negative = -128 }
        public static class Api {
            public static int ReadTiny(Tiny value) => (byte)value;
            public static ulong ReadWide(Wide value) => (ulong)value;
            public static Tiny TinyInput() => Tiny.Read;
            public static Wide WideInput() => Wide.None;
        }
        "#,
        None,
    );
    fs::write(
        project.join("main.oxel"),
        r#"
        comptime fn toggle(value: Flags.Tiny) -> Flags.Tiny { value ^ Flags.Tiny.Write }
        const TINY = toggle(~Flags.Tiny.Read);
        const WIDE = ~(Flags.Wide.High | Flags.Wide.Low);
        const MAX = 0xffff_ffff_ffff_ffff as Flags.Wide;
        const NEGATIVE = (-128 as Flags.Signed) as i64;
        const FRACTION = 128.75 as Flags.Tiny as u16;
        fn main() {
            let mut tiny = ~Flags.Api.TinyInput();
            tiny ^= Flags.Tiny.Write;
            System.Console.WriteLine(tiny == TINY);
            System.Console.WriteLine(Flags.Api.ReadTiny(tiny));
            let wide = ~Flags.Api.WideInput() & ~(Flags.Wide.High | Flags.Wide.Low);
            System.Console.WriteLine(wide == WIDE);
            System.Console.WriteLine(Flags.Api.ReadWide(wide));
            System.Console.WriteLine(MAX as u64);
            System.Console.WriteLine(Flags.Signed.Negative as i64 == NEGATIVE);
            System.Console.WriteLine(Flags.Api.TinyInput() as Flags.Wide == Flags.Wide.Low);
            System.Console.WriteLine(FRACTION == Flags.Tiny.Write as u16);
            System.Console.WriteLine(Flags.Api.TinyInput() as f64 == 1.0);
            let boxed: object = 0x81 as Flags.Tiny;
            System.Console.WriteLine((boxed as Flags.Tiny) as u8);
        }
        "#,
    )
    .unwrap();
    let result = successful(
        Command::new(env!("CARGO_BIN_EXE_oxel"))
            .current_dir(project)
            .args(["run", "main.oxel", "--reference"])
            .arg(&reference)
            .output()
            .unwrap(),
    );
    assert_eq!(
        result.lines().collect::<Vec<_>>(),
        [
            "True",
            "126",
            "True",
            "9223372036854775806",
            "18446744073709551615",
            "True",
            "True",
            "True",
            "True",
            "129"
        ],
    );
    let mut references = oxel::References::default();
    references.add(&fs::read(&reference).unwrap()).unwrap();
    for constant in [
        "const VALUE = 0x100 as Flags.Tiny;",
        "const VALUE = -1 as Flags.Tiny;",
        "const VALUE = 0x100 as Flags.Wide as Flags.Tiny;",
    ] {
        let error =
            &oxel::check_with_references(&format!("{constant} fn main() {{}}"), &references)
                .unwrap_err()[0];
        assert_eq!(error.code, "E0503");
        assert!(error.message.contains("Flags.Tiny"), "{}", error.message);
    }
    let errors = oxel::check_with_references("fn main() { let _: Flags.Tiny = 1; }", &references)
        .unwrap_err();
    assert_eq!(errors[0].code, "E0301");

    fs::write(
        project.join("main.oxel"),
        "fn main() { let value = 256; System.Console.WriteLine(value as Flags.Tiny); }",
    )
    .unwrap();
    let overflow = Command::new(env!("CARGO_BIN_EXE_oxel"))
        .current_dir(project)
        .args(["run", "main.oxel", "--reference"])
        .arg(&reference)
        .output()
        .unwrap();
    assert!(!overflow.status.success());
    assert!(String::from_utf8_lossy(&overflow.stderr).contains("System.OverflowException"));
}

#[test]
fn imports_clr_parameter_arrays_and_packs_only_expanded_arguments() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path();
    let sdk = successful(Command::new("dotnet").arg("--version").output().unwrap());
    let reference = build_reference_fixture(
        &cwd.join("fixture"),
        sdk.split('.').next().unwrap(),
        "VariadicBridge",
        "1.0.0.0",
        r#"
        using System;
        namespace Variadic;

        public class ListSize {
            public ListSize(params int[] values) => Size = values.Length;
            public int Size { get; }
            public int Extra(int seed = 10, params int[] values) => seed + values.Length;
        }

        public static class Api {
            public static int Count<T>(params T[] values) => values.Length;
            public static int CountObjects(params object[] values) => values.Length;
            public static int Sum(params byte[] values) => Array.FindAll(values, x => x == 255).Length;
            public static string Pick(int x, int y) => "fixed";
            public static string Pick(int x, params int[] ys) => "expanded";
            public static int ArrayOnly(int[] values) => values.Length;
            public static int Invoke(params Func<int>[] callbacks) => callbacks[0]();
            public static int Mutate(int prefix, params int[] values) => values[0] += prefix;
        }
        "#,
        None,
    );
    let source = cwd.join("main.oxel");
    let cli = |command| {
        Command::new(env!("CARGO_BIN_EXE_oxel"))
            .current_dir(cwd)
            .args([command, "--framework", "--reference"])
            .arg(&reference)
            .arg(&source)
            .output()
            .unwrap()
    };
    fs::write(
        &source,
        r#"
        fn mark(value: i32) -> i32 {
            System.Console.WriteLine(value);
            value
        }

        fn main() {
            let values = [10, 20];
            System.Console.WriteLine(Variadic.Api.Count(values));
            System.Console.WriteLine(Variadic.Api.Count(1, 2, 3));
            System.Console.WriteLine(Variadic.Api.Count::<i32>());
            System.Console.WriteLine(Variadic.Api.CountObjects(1, "value"));
            System.Console.WriteLine(Variadic.Api.CountObjects([1, "value", false]));
            System.Console.WriteLine(Variadic.Api.Sum(255));
            System.Console.WriteLine(Variadic.Api.Sum([]));
            System.Console.WriteLine(Variadic.Api.Pick(1, 2));
            System.Console.WriteLine(Variadic.Api.Pick(1, 2, 3));
            let size = new Variadic.ListSize(1, 2, 3);
            System.Console.WriteLine(size.Size);
            System.Console.WriteLine(size.Extra());
            System.Console.WriteLine(size.Extra(20, 5));
            System.Console.WriteLine(Variadic.Api.Mutate(5, values));
            System.Console.WriteLine(values[0]);
            System.Console.WriteLine(Variadic.Api.Mutate(mark(1), mark(2), mark(3)));
            let result = Variadic.Api.Invoke() { 42 };
            System.Console.WriteLine(result);
            System.Console.WriteLine(System.String.Join(",", "a", "b", "c"));
        }
        "#,
    )
    .unwrap();
    assert_eq!(
        successful(cli("run")).lines().collect::<Vec<_>>(),
        [
            "2", "3", "0", "2", "3", "1", "0", "fixed", "expanded", "3", "10", "21", "15", "15",
            "1", "2", "3", "3", "42", "a,b,c",
        ]
    );
    for call in [
        "Variadic.Api.ArrayOnly(1, 2)",
        "Variadic.Api.Sum(true)",
        "Variadic.Api.Sum(256)",
        "Variadic.Api.Count()",
    ] {
        fs::write(&source, format!("fn main() {{ {call}; }}")).unwrap();
        let output = cli("check");
        assert!(!output.status.success(), "{call}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("error[E0205]") || stderr.contains("error[E0303]"),
            "{call}: {stderr}"
        );
    }
}

#[test]
fn imports_and_emits_optional_clr_arguments_for_methods_and_constructors() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path();
    let sdk = successful(Command::new("dotnet").arg("--version").output().unwrap());
    let reference = build_reference_fixture(
        &cwd.join("fixture"),
        sdk.split('.').next().unwrap(),
        "OptionalBridge",
        "1.0.0.0",
        r#"
        using System;
        using System.Runtime.InteropServices;
        namespace Defaults;

        public enum Mode : ulong { Enabled = ulong.MaxValue }

        public struct Marker {
            public int Score;
            public Marker() => Score = 99;
        }

        public class Label {
            private readonly string text;
            public Label(string text = "oxel 🌍") => this.text = text;
            public string Read(bool enabled = true, char suffix = '!') => enabled ? text + suffix : "-";
        }

        public static class Api {
            public static double Real(float a = 1.25F, double b = 2.75) => a + b;
            public static bool Enum(Mode mode = Mode.Enabled) => mode == Mode.Enabled;
            public static int Zero(Marker marker = default, int? number = null) => marker.Score + (number ?? 10);
            public static bool Null(string? value = null) => value is null;
            public static T Second<T>(T first, T second = default!) => second;
            public static int Box([Optional, DefaultParameterValue(9)] object value) => (int)value;
            public static string Pick(int x) => "exact";
            public static string Pick(int x, int y = 2) => "padded";
            public static string Prefer(object x) => "object";
            public static string Prefer(string x, int y = 2) => "string";
            public static int Ambiguous(int x, int y = 1) => 0;
            public static int Ambiguous(int x, long y = 2) => 0;
            public static int Missing(int x, [Optional] object? value) => x;
        }
        "#,
        None,
    );
    let source = cwd.join("main.oxel");
    fs::write(
        &source,
        r#"
        fn generic<T>(value: T) -> T { Defaults.Api.Second(value) }

        fn main() {
            let label = new Defaults.Label();
            System.Console.WriteLine(label.Read());
            System.Console.WriteLine(label.Read(false));
            System.Console.WriteLine(Defaults.Api.Real());
            System.Console.WriteLine(Defaults.Api.Enum());
            System.Console.WriteLine(Defaults.Api.Zero());
            System.Console.WriteLine(Defaults.Api.Null());
            System.Console.WriteLine(generic(123));
            System.Console.WriteLine(Defaults.Api.Box());
            System.Console.WriteLine(Defaults.Api.Pick(30));
            System.Console.WriteLine(Defaults.Api.Pick(30, 20));
            System.Console.WriteLine(Defaults.Api.Prefer("specific"));
            System.Console.WriteLine(Defaults.Api.Missing(3, null));
            System.Console.WriteLine("a.b".Split('.').Length);
        }
        "#,
    )
    .unwrap();
    let cli = |command| {
        Command::new(env!("CARGO_BIN_EXE_oxel"))
            .current_dir(cwd)
            .args([command, "--framework", "--reference"])
            .arg(&reference)
            .arg(&source)
            .output()
            .unwrap()
    };
    assert_eq!(
        successful(cli("run")).lines().collect::<Vec<_>>(),
        [
            "oxel 🌍!",
            "-",
            "4",
            "True",
            "10",
            "True",
            "0",
            "9",
            "exact",
            "padded",
            "string",
            "3",
            "2"
        ]
    );

    for (call, code) in [
        ("Defaults.Api.Pick()", "E0205"),
        ("Defaults.Api.Ambiguous(1)", "E0208"),
        ("Defaults.Api.Missing(1)", "E0205"),
        ("Defaults.Api.Second()", "E0205"),
    ] {
        fs::write(&source, format!("fn main() {{ {call}; }}")).unwrap();
        let result = cli("check");
        assert!(!result.status.success(), "{call}");
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(stderr.contains(code), "{call}: {stderr}");
    }
}

#[test]
fn clr_managed_addresses_preserve_types_and_assignment_order() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path();
    let sdk = successful(Command::new("dotnet").arg("--version").output().unwrap());
    let reference = build_reference_fixture(
        &cwd.join("fixture"),
        sdk.split('.').next().unwrap(),
        "AddressBridge",
        "1.0.0.0",
        r#"
        namespace ByRef;
        public interface IRead {
            int Read(in int value, int offset, in int scale);
            int ReadLocation(ref readonly int value);
        }
        public class Base {
            public int Pick(int value) => value;
            public virtual int Read(in int value, int offset, in int scale) => 0;
        }
        public class Reader : Base, IRead {
            public Reader(out int code) => code = 10;
            public int Pick(ref int value) => ++value;
            public override int Read(in int value, int offset, in int scale) => (value + offset) * scale;
            public int ReadLocation(ref readonly int value) => value + 3;
        }
        public delegate int ReadDelegate(in int value);
        public delegate int Transform(int value);
        public struct Cursor {
            private int value;
            public Cursor(int value) => this.value = value;
            public void Read(out int number, int offset = 1) => number = value + offset;
        }
        public static class Api {
            public static void Copy<T>(T value, out T output) => output = value;
            public static bool Next(out string? output) { output = null; return false; }
            public static void Clear(ref string? value) => value = null;
            public static void Set(out int value, int input = 10) => value = input;
            public static void Both(out int first, out int second) { first = 1; second = 2; }
            public static int ReadOnly(in int input) => input + 1;
            public static int Pick(int value) => value;
            public static int Pick(in int value) => value + 1;
            public static int Length(in string text) => text.Length;
            public static int MaybeLength(in string? text) => text?.Length ?? 0;
            public static T Echo<T>(in T value) => value;
            public static int Apply(int value, Transform transform) => transform(value);
            public static int Location(ref readonly int value) => value + 2;
            public static ReadDelegate GetDelegate() => ReadOnly;
            public static void InputFlag([System.Runtime.InteropServices.In] ref int value) => value++;
        }
        "#,
        None,
    );
    let source = cwd.join("main.oxel");
    let cli = |command, code: &str| {
        fs::write(&source, code).unwrap();
        Command::new(env!("CARGO_BIN_EXE_oxel"))
            .current_dir(cwd)
            .args([command, "--reference"])
            .arg(&reference)
            .arg(&source)
            .output()
            .unwrap()
    };
    let mut symbols = oxel::References::default();
    symbols.add(&fs::read(&reference).unwrap()).unwrap();
    assert_eq!(
        oxel::clr_function_signatures("ByRef.Api.ReadOnly", &symbols)[0].label,
        "ByRef.Api.ReadOnly(&System.Int32) -> System.Int32"
    );
    let output = cli(
        "run",
        r#"
        fn forward(number: i32, word: string?) -> i32 {
            if word != null {
                ByRef.Api.ReadOnly(&number) + ByRef.Api.MaybeLength(&word)
            } else {
                ByRef.Api.MaybeLength(in word)
            }
        }

        fn echo<T>(value: T) -> T {
            ByRef.Api.Echo(&value)
        }

        fn main() {
            let code: i32;
            let reader = new ByRef.Reader(out code);
            let mut count = code;
            System.Console.WriteLine(reader.Pick(30));
            System.Console.WriteLine(reader.Pick(&mut count));
            System.Console.WriteLine(count);

            let cursor = new ByRef.Cursor(29);
            let number: i32;
            cursor.Read(out number);
            System.Console.WriteLine(number);

            let text: string?;
            ByRef.Api.Copy("yes", out text);
            System.Console.WriteLine(text ?? "no");
            let mut mutable_text: string? = text;
            ByRef.Api.Clear(ref mutable_text);
            System.Console.WriteLine(mutable_text ?? "cleared");
            let empty: string?;
            ByRef.Api.Next(out empty);
            System.Console.WriteLine(empty ?? "empty");

            System.Console.WriteLine(ByRef.Api.ReadOnly(in number));
            System.Console.WriteLine(ByRef.Api.Pick(count & 15));
            System.Console.WriteLine(ByRef.Api.Pick(&count));
            let word = "hello";
            System.Console.WriteLine(ByRef.Api.Length(in word));
            System.Console.WriteLine(ByRef.Api.Location(in number));

            let base_reader: ByRef.Base = reader;
            let interface_reader: ByRef.IRead = reader;
            let scale = 2;
            System.Console.WriteLine(base_reader.Read(&number, 2, &scale));
            System.Console.WriteLine(interface_reader.Read(in number, 3, in scale));
            System.Console.WriteLine(interface_reader.ReadLocation(in number));
            let callback = ByRef.Api.GetDelegate();
            System.Console.WriteLine(callback(&number));
            System.Console.WriteLine(forward(number, word));
            System.Console.WriteLine(echo(number));
            System.Console.WriteLine(ByRef.Api.Apply(number) |value| { ByRef.Api.ReadOnly(in value) });
        }
        "#,
    );
    assert_eq!(
        successful(output).lines().collect::<Vec<_>>(),
        [
            "30", "11", "11", "30", "yes", "cleared", "empty", "31", "11", "12", "5", "32", "64",
            "66", "33", "31", "36", "30", "31"
        ]
    );

    for (body, code) in [
        ("let x: i32; ByRef.Api.Set(x);", "E0213"),
        ("let x: i64; ByRef.Api.Set(out x);", "E0205"),
        ("let x: i32; ByRef.Api.Set(out x, x);", "E0321"),
        ("let x: i32; ByRef.Api.Both(out x, out x);", "E0322"),
        (
            "let x: i32; ByRef.Api.Set(out x); ByRef.Api.Set(out x);",
            "E0322",
        ),
        ("let x = 1; ByRef.Api.Set(out x);", "E0204"),
        ("let x: string; ByRef.Api.Next(out x);", "E0213"),
        ("let mut x = 1; ByRef.Api.ReadOnly(ref x);", "E0213"),
        ("let x = 1; ByRef.Api.ReadOnly(x);", "E0213"),
        ("let x: i32; ByRef.Api.ReadOnly(&x);", "E0321"),
        ("let x: i64 = 1; ByRef.Api.ReadOnly(&x);", "E0205"),
        ("let x: string? = null; ByRef.Api.Clear(&mut x);", "E0204"),
        ("let mut x = 1; ByRef.Api.ReadOnly(&mut x);", "E0213"),
        ("let x = 1; ByRef.Api.Set(&mut x);", "E0213"),
        ("let x: string? = null; ByRef.Api.Length(in x);", "E0205"),
        ("let mut x = 1; ByRef.Api.Location(ref x);", "E0213"),
        ("let x = 1; ByRef.Api.InputFlag(in x);", "E0203"),
        (
            "let code: i32; let reader = new ByRef.Reader(out code); let mut x: i32; reader.Pick(ref x);",
            "E0321",
        ),
    ] {
        let result = cli("check", &format!("fn main() {{ {body} }}"));
        assert!(!result.status.success(), "{body}");
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(stderr.contains(code), "{body}: {stderr}");
    }

    for (call, code) in [
        ("ByRef.Api.Set(out number)", "E0213"),
        ("ByRef.Api.Clear(ref word)", "E0213"),
        ("if word != null { ByRef.Api.Length(in word); }", "E0205"),
    ] {
        let result = cli(
            "check",
            &format!("fn forward(number: i32, word: string?) {{ {call}; }} fn main() {{}}"),
        );
        assert!(!result.status.success(), "{call}");
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(stderr.contains(code), "{call}: {stderr}");
    }
}

#[test]
fn trailing_lambdas_use_clr_delegates_and_capture_live_object_references() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path();
    let sdk = successful(Command::new("dotnet").arg("--version").output().unwrap());
    let reference = build_reference_fixture(
        &cwd.join("fixture"),
        sdk.split('.').next().unwrap(),
        "CallbackBridge",
        "1.0.0.0",
        r#"
        using System;
        namespace Callbacks;

        public delegate int Transform(int value);
        public delegate void Fill(out int value);

        public struct State {
            public int Count;
            public State(int count) => Count = count;
            public void Advance() => Count++;
        }

        public interface ISignal {
            event EventHandler? Raised;
        }
        public class Signal : ISignal {
            public event EventHandler? Raised;
            public void Raise() => Raised?.Invoke(this, EventArgs.Empty);
        }
        public sealed class DerivedSignal : Signal {}
        public static class Bus<T> {
            public static event Action<T>? Raised;
            public static void Raise(T value) => Raised?.Invoke(value);
        }

        public static class Dispatch {
            public static Fill Writer() => (out int value) => value = 30;

            public static void Run<T>(T value, Action<T> callback)
        #if NET9_0_OR_GREATER
                where T : allows ref struct
        #endif
                => callback(value);
        }

        public sealed class Button {
            private EventHandler? click;
            private Transform? transform;
            public int Count { get; set; }

            public void OnClick(EventHandler handler) => click = handler;
            public void OnClick(Action handler) => click = (_, _) => handler();
            public void OnTransform(Transform handler) => transform = handler;
            public void Click() => click?.Invoke(this, EventArgs.Empty);
            public int Apply(int value) => transform!(value);
        }
        "#,
        None,
    );
    let source = cwd.join("callbacks.oxel");
    let cli = |command, code: &str| {
        fs::write(&source, code).unwrap();
        Command::new(env!("CARGO_BIN_EXE_oxel"))
            .current_dir(cwd)
            .args([command, "--framework", "--reference"])
            .arg(&reference)
            .arg(&source)
            .output()
            .unwrap()
    };
    let program = r#"
        fn create(step: i32) -> Callbacks.Button {
            let button = new Callbacks.Button();
            button.OnClick() |sender, _| {
                if sender != null {
                    button.Count += step;
                }
            };
            button.OnTransform() |input| {
                input + button.Count
            };
            button
        }

        fn main() {
            let button = create(10);
            button.Click();
            button.Click();
            Callbacks.Dispatch.Run(10) |value| {
                System.Console.WriteLine(button.Apply(value));
            };
            let counter = new Callbacks.DerivedSignal();
            let signal: Callbacks.ISignal = counter;
            let handler: System.EventHandler = |_, _| { button.Count += 5; };
            signal.Raised += handler;
            counter.Raise();
            counter.Raised -= handler;
            counter.Raise();
            Callbacks.Bus::<i32>.Raised += |value| {
                System.Console.WriteLine(button.Apply(value));
            };
            Callbacks.Dispatch.Run(2) |scale| {
                let offset = 3;
                Callbacks.Bus::<i32>.Raised += |value| {
                    System.Console.WriteLine(button.Apply(value) + scale + offset);
                };
            };
            Callbacks.Bus::<i32>.Raise(10);

            let mut state = new Callbacks.State(10);
            let update: System.Action = || {
                state.Count += 7;
                state.Advance();
            };
            state.Advance();
            update.Invoke();
            System.Console.WriteLine(state.Count);

            let mut number: i32;
            System.Int32.TryParse("10", out number);
            Callbacks.Dispatch.Run(20) |delta| { number += delta; };
            System.Console.WriteLine(number);

            let output: i32;
            let fill = Callbacks.Dispatch.Writer();
            fill(out output);
            System.Console.WriteLine(output);
        }
        "#;
    assert_eq!(
        successful(cli("run", program)).lines().collect::<Vec<_>>(),
        ["30", "35", "40", "19", "30", "30"]
    );
    let first = successful(cli("compile", program));
    let bytes = fs::read(cwd.join(first.trim())).unwrap();
    let second = successful(cli("compile", program));
    assert_eq!(bytes, fs::read(cwd.join(second.trim())).unwrap());

    let mut references = oxel::References::default();
    references.add(&fs::read(&reference).unwrap()).unwrap();
    let editor_source = "fn apply(callback: Callbacks.Transform) -> i32 { callback(20) }";
    let analysis = &oxel::inspect_project(&[editor_source], &references)[0];
    assert!(
        analysis.diagnostics.is_empty(),
        "{:?}",
        analysis.diagnostics
    );
    let call = analysis
        .occurrences
        .iter()
        .find(|item| item.span.start == editor_source.rfind("callback").unwrap())
        .unwrap();
    assert_eq!(call.kind, oxel::OccurrenceKind::Parameter);
    assert_eq!(
        call.definition.unwrap().start,
        editor_source.find("callback").unwrap()
    );
    assert_eq!(call.call_parameters.as_ref().unwrap().ranges.len(), 1);

    for (body, code, message) in [
        ("button.OnClick() |first| {};", "E0340", "parameter count"),
        (
            "let signal = new Callbacks.DerivedSignal(); signal.Raised = null;",
            "E0341",
            "requires '+=' or '-='",
        ),
        (
            "let signal = new Callbacks.DerivedSignal(); let callback = signal.Raised;",
            "E0341",
            "requires '+=' or '-='",
        ),
        (
            "let mut x: i32; button.OnClick() { System.Console.WriteLine(x); };",
            "E0321",
            "may be unassigned",
        ),
        (
            "let x: i32; button.OnClick() { System.Console.WriteLine(x); };",
            "E0321",
            "may be unassigned",
        ),
        (
            "let x: i32; button.OnClick() { button.OnClick() { System.Console.WriteLine(x); }; };",
            "E0321",
            "may be unassigned",
        ),
    ] {
        let result = cli(
            "check",
            &format!("fn main() {{ let button = new Callbacks.Button(); {body} }}"),
        );
        assert!(!result.status.success(), "{body}");
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(
            stderr.contains(code) && stderr.contains(message),
            "{body}: {stderr}"
        );
    }
}

#[test]
fn follows_clr_type_forwarders_for_existing_clients() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path();
    let sdk = successful(Command::new("dotnet").arg("--version").output().unwrap());
    let major = sdk.split('.').next().unwrap();
    let build = |folder: &str, name, source, dependency| {
        build_reference_fixture(
            &cwd.join(folder),
            major,
            name,
            "1.0.0.0",
            source,
            dependency,
        )
    };

    let types = r#"
        namespace Moved;
        public enum Wide : ulong { Zero, Top = 0xFEDCBA9876543210UL }
        public enum Signed : sbyte { Zero, Negative = -128 }
        public class Item {
            public int Value { get; }
            public Item(int value) => Value = value;
        }
        public class Box<T> {
            public T Value { get; }
            public Box(T value) => Value = value;
        }
    "#;
    let forwarders = r#"
        using System.Runtime.CompilerServices;
        [assembly: TypeForwardedTo(typeof(Moved.Item))]
        [assembly: TypeForwardedTo(typeof(Moved.Box<>))]
        [assembly: TypeForwardedTo(typeof(Moved.Wide))]
        [assembly: TypeForwardedTo(typeof(Moved.Signed))]
    "#;
    let old_facade = build("old_f", "Facade", types, None);
    let old_relay = build("old_r", "Relay", types, None);
    let implementation = build("impl", "Implementation", types, None);
    let facade = build("facade", "Facade", forwarders, Some(("Relay", &old_relay)));
    let relay = build(
        "relay",
        "Relay",
        forwarders,
        Some(("Implementation", &implementation)),
    );
    let cycle = build("cycle", "Relay", forwarders, Some(("Facade", &old_facade)));
    let client = build(
        "client",
        "Clients",
        r#"
        using Moved;
        namespace Clients;
        public class Derived : Box<Item> {
            public Derived(int value) : base(new Item(value)) {}
        }
        public static class Api {
            public static Box<Item> Wrap(Item item) => new(item);
            public static Box<T> Copy<T>(Box<T> box) => new(box.Value);
            public static T Keep<T>(T value) where T : Item => value;
            public static Wide Echo(Wide value) => value;
            public static bool IsTop(Wide value) => value == Wide.Top;
            public const Wide Favorite = Wide.Top;
            public static Signed Negative() => Signed.Negative;
        }
        "#,
        Some(("Facade", &old_facade)),
    );

    let source = r#"
        fn main() {
            let item = Clients.Api.Keep(new Moved.Item(30));
            let wrapped: Moved.Box<Moved.Item> = Clients.Api.Wrap(item);
            System.Console.WriteLine(wrapped.Value.Value);
            let copied = Clients.Api.Copy(new Clients.Derived(40));
            System.Console.WriteLine(copied.Value.Value);
            let wide: Moved.Wide = Clients.Api.Echo(Clients.Api.Favorite);
            System.Console.WriteLine(Clients.Api.IsTop(wide));
            System.Console.WriteLine(wide);
            System.Console.WriteLine(Clients.Api.Negative() == Moved.Signed.Negative);
        }
    "#;
    fs::write(cwd.join("main.oxel"), source).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_oxel"));
    command.current_dir(cwd).args(["run", "main.oxel"]);
    for path in [&client, &facade, &relay, &implementation] {
        command.arg("--reference").arg(path);
    }
    assert_eq!(
        successful(command.output().unwrap())
            .lines()
            .collect::<Vec<_>>(),
        ["30", "40", "True", "Top", "True"]
    );

    let load = |paths: &[&std::path::PathBuf]| {
        let mut references = oxel::References::default();
        for path in paths {
            references.add(&fs::read(path).unwrap()).unwrap();
        }
        references
    };
    let reverse = load(&[&implementation, &relay, &facade, &client]);
    oxel::check_with_references(source, &reverse).unwrap();

    let wrong = build_reference_fixture(
        &cwd.join("wrong"),
        major,
        "Implementation",
        "2.0.0.0",
        types,
        None,
    );
    let invalid_references: [(&str, &[_]); 3] = [
        ("cycle", &[&client, &facade, &cycle, &implementation]),
        ("wrong version", &[&client, &facade, &relay, &wrong]),
        ("missing facade", &[&client, &implementation, &relay]),
    ];
    for (case, paths) in invalid_references {
        let references = load(paths);
        let errors = oxel::check_with_references(source, &references).unwrap_err();
        assert_eq!(errors[0].code, "E0203", "{case}: {errors:?}");
        assert!(
            errors[0].message.contains("Clients.Api.Keep"),
            "{case}: {errors:?}"
        );
    }
}

#[test]
fn reads_and_writes_public_clr_fields() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path();
    let sdk = successful(Command::new("dotnet").arg("--version").output().unwrap());
    let major = sdk.split('.').next().unwrap();
    let models = build_reference_fixture(
        &cwd.join("models"),
        major,
        "Models",
        "1.0.0.0",
        r#"
        namespace Fields;
        public class Slot<T> {
            public T Entry;
            public Slot(T entry) => Entry = entry;
        }
        "#,
        None,
    );
    let bridge = build_reference_fixture(
        &cwd.join("bridge"),
        major,
        "Bridge",
        "1.0.0.0",
        r#"
        namespace Fields;
        public class Base {
            public int Count = 10;
            private int stored = 4;
            public int Traced {
                get { Registry.Sequence = Registry.Sequence * 10 + 3; return stored; }
                set { Registry.Sequence = Registry.Sequence * 10 + 4; stored = value; }
            }
            public int SetterOnly { set => stored = value; }
            public int Stored() => stored;
            public readonly int Stamp = 7;
            public string Name = "base";
            public string? Label;
            public int Across = 8;
            public int Modified = 6;
            public int Shadow => 9;
            private int Hidden = -1;
            public int Internal() => Hidden;
        }
        public class Derived : Base {
            public new int Across => 3;
            public new int Shadow = 2;
            public new volatile int Modified;
        }
        public static class Registry {
            public static readonly Derived Current = new();
            public static int Sequence;
            public static Slot<int> Slot = new(41);
            public const int Literal = 6;
            public const sbyte Signed = -8;
            public const byte Unsigned = 250;
            public const short Small = -1234;
            public const long Large = 3000000000;
            public const float Single = 1.5f;
            public const double Double = 2.25;
            public const bool Enabled = true;
            public const string Text = "\u4f60\u597d\U0001f680";
            public const string Empty = "";
            public const string? Missing = null;
            public const object? MissingObject = null;
            public const char Letter = '界';
            public const decimal Unsupported = 1.0m;
            public static char EchoChar(char value) => value;
            public static int Property => 9;
            public static volatile int Signal;
            public static Derived Receiver() { Sequence = Sequence * 10 + 1; return Current; }
            public static int Value() { Sequence = Sequence * 10 + 2; return 30; }
            public static int ReplaceCount() { Current.Count = 99; return Value(); }
        }
        public static class Store<T> {
            public static T Value = default!;
            public static volatile string? State;
            public const int Limit = 20;
        }
        "#,
        Some(("Models", &models)),
    );
    let source = r#"
        const LIMIT = Fields.Store::<string>.Limit;
        const ADDED = double_literal() + LIMIT;
        const BANNER = Fields.Registry.Text + Fields.Registry.Empty;
        const LETTER = Fields.Registry.Letter;
        comptime fn double_literal() -> i32 {
            Fields.Registry.Literal * 2
        }
        fn check_types(a: i8, b: u8, c: i16, large: i64, single: f32, wide: f64) -> bool {
            a == -8 && b == 250 && c == -1234 && large == 3000000000
                && single == 1.5 && wide == 2.25
        }
        fn change<T: Fields.Base>(value: T) -> i32 {
            value.Count = value.Count + 2;
            value.Count
        }
        fn main() {
            Fields.Registry.Receiver().Count = Fields.Registry.Value();
            System.Console.WriteLine(Fields.Registry.Sequence);
            let current = Fields.Registry.Current;
            System.Console.WriteLine(change(current));
            System.Console.WriteLine(current.Stamp);
            System.Console.WriteLine(current.Across);
            current.Shadow = 5;
            System.Console.WriteLine(current.Shadow);
            let base: Fields.Base = current;
            System.Console.WriteLine(base.Shadow);
            current.Label = null;
            System.Console.WriteLine(current.Label ?? current.Name);
            current.Label = "set";
            System.Console.WriteLine(current.Label ?? current.Name);

            Fields.Registry.Slot.Entry = 42;
            System.Console.WriteLine(Fields.Registry.Slot.Entry);
            Fields.Registry.Slot = new Fields.Slot<i32>(52);
            System.Console.WriteLine(Fields.Registry.Slot.Entry);
            Fields.Store::<i64>.Value = 3000000000;
            Fields.Store::<string>.Value = "text";
            System.Console.WriteLine(Fields.Store::<i64>.Value);
            System.Console.WriteLine(Fields.Store::<string>.Value);
            current.Modified = 70;
            Fields.Registry.Signal = current.Modified + base.Modified;
            System.Console.WriteLine(Fields.Registry.Signal);
            Fields.Store::<string>.State = "visible";
            System.Console.WriteLine(Fields.Store::<string>.State ?? "missing");
            System.Console.WriteLine(Fields.Registry.Missing ?? BANNER);
            System.Console.WriteLine(Fields.Registry.MissingObject == null);
            Fields.Registry.Sequence = 0;
            Fields.Registry.Receiver().Count += Fields.Registry.ReplaceCount();
            System.Console.WriteLine(Fields.Registry.Sequence);
            System.Console.WriteLine(current.Count);
            Fields.Registry.Sequence = 0;
            Fields.Registry.Receiver().Traced *= Fields.Registry.Value();
            System.Console.WriteLine(Fields.Registry.Sequence);
            System.Console.WriteLine(current.Stored());
            Fields.Store::<i64>.Value += 30;
            System.Console.WriteLine(Fields.Store::<i64>.Value);
            System.Console.WriteLine(Fields.Registry.EchoChar(LETTER));
            current.Name += " field";
            System.Console.WriteLine(current.Name);
            if Fields.Registry.Enabled && check_types(
                Fields.Registry.Signed, Fields.Registry.Unsigned, Fields.Registry.Small,
                Fields.Registry.Large, Fields.Registry.Single, Fields.Registry.Double
            ) {
                System.Console.WriteLine(ADDED);
            }
        }
    "#;
    fs::write(cwd.join("main.oxel"), source).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_oxel"))
        .current_dir(cwd)
        .args(["run", "main.oxel", "--reference"])
        .arg(&bridge)
        .arg("--reference")
        .arg(&models)
        .output()
        .unwrap();
    assert_eq!(
        successful(result).lines().collect::<Vec<_>>(),
        [
            "12",
            "32",
            "7",
            "3",
            "5",
            "9",
            "base",
            "set",
            "42",
            "52",
            "3000000000",
            "text",
            "76",
            "visible",
            "你好🚀",
            "True",
            "12",
            "62",
            "1324",
            "120",
            "3000000030",
            "界",
            "base field",
            "32"
        ],
    );

    let mut references = oxel::References::default();
    for path in [&bridge, &models] {
        references.add(&fs::read(path).unwrap()).unwrap();
    }
    let assembly = oxel::compile_with_references(source, "FieldsTest", &references).unwrap();
    for field_opcode in [0x7b, 0x7d, 0x7e, 0x80] {
        assert!(
            assembly
                .windows(3)
                .any(|bytes| bytes == [0xfe, 0x13, field_opcode]),
            "missing volatile prefix before opcode {field_opcode:#x}"
        );
    }
    for (source, code) in [
        ("fn main() { new Fields.Base().Stamp = 1; }", "E0204"),
        ("fn main() { new Fields.Base().Stamp += 1; }", "E0204"),
        ("fn main() { new Fields.Base().SetterOnly += 1; }", "E0308"),
        ("fn main() { new Fields.Derived().Across += 1; }", "E0308"),
        ("fn main() { let b: Fields.Base; b.Count += 1; }", "E0321"),
        (
            "fn main() { Fields.Registry.Current = new Fields.Derived(); }",
            "E0204",
        ),
        ("fn main() { new Fields.Base().Name = null; }", "E0301"),
        ("fn main() { new Fields.Derived().Across = 1; }", "E0308"),
        ("fn main() { new Fields.Base().Hidden; }", "E0305"),
        ("fn main() { Fields.Registry.Literal = 1; }", "E0204"),
        ("fn main() { Fields.Registry.Unsupported; }", "E0305"),
        ("const X = Fields.Registry.Signal; fn main() {}", "E0501"),
        ("const X = Fields.Registry.Property; fn main() {}", "E0501"),
        (
            "const X = Fields.Store::<string>.State; fn main() {}",
            "E0501",
        ),
        (
            "fn main() { let b: Fields.Base? = null; b.Count = 1; }",
            "E0311",
        ),
        ("fn main() { let b: Fields.Base; b.Count = 1; }", "E0321"),
    ] {
        let errors = oxel::check_with_references(source, &references).unwrap_err();
        assert_eq!(errors[0].code, code, "{source}: {errors:?}");
    }
}

#[test]
fn validates_clr_generic_constraints_across_languages() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path();
    let sdk = successful(Command::new("dotnet").arg("--version").output().unwrap());
    let major = sdk.split('.').next().unwrap();
    let contract = build_reference_fixture(
        &cwd.join("contract"),
        major,
        "Contract",
        "1.0.0.0",
        r#"
        namespace Bound;
        public interface ITagged { int Tag(); }
        public interface ICompare<T> { int Compare(T other); }
        public abstract class Parent { public int Offset() => 5; }
        public class Good : Parent, ITagged, ICompare<Good> {
            public int Tag() => 25;
            public int Compare(Good other) => other.Tag() + 5;
        }
        public sealed class NoDefault : Parent, ITagged {
            public NoDefault(int value) {}
            public int Tag() => 3;
        }
        public sealed class NoTag : Parent {}
        "#,
        None,
    );
    let bridge = build_reference_fixture(
        &cwd.join("bridge"),
        major,
        "Bridge",
        "1.0.0.0",
        r#"
        namespace Bound;
        public interface IRef<out T> where T : class { T Read(); }
        public class Ref<T>(T value) : IRef<T> where T : class { public T Read() => value; }
        public class Val<T>(T value) where T : struct {
            public T Value = value;
            public Val<U> Replace<U>(U replacement) where U : struct => new(replacement);
        }
        public class Both<T>(T value) where T : Parent, ITagged {
            public T Value = value;
            public int Total() => Value.Tag() + Value.Offset();
        }
        public class Derived() : Both<Good>(new Good()) {}
        public static class Maker<T> where T : new() {
            public static T Make() => new T();
            public static U Choose<U>(T first, U second) where U : class => second;
        }
        public class Constructible<T> where T : new() {}
        public struct ConstructedValue {
            public int Number;
            public ConstructedValue() { Number = 31; }
        }
        public static class Api {
            public static Both<Good> Wrap() => new Derived();
            public static int Tag<T>(Both<T> value) where T : Parent, ITagged => value.Total();
            public static Ref<T> KeepRef<T>(T value) where T : class => new(value);
            public static T Create<T>() where T : new() => new T();
            public static Val<T> KeepValue<T>(T value) where T : struct => new(value);
            public static string Pick<T>(T value) where T : struct => "value";
            public static string Pick(object value) => "object";
            public static T Unmanaged<T>(T value) where T : unmanaged => value;
        }
        public class Raw<T> where T : unmanaged {}
        public class Dependent<T, U>(T value) where T : U { public U Value => value; }
        public class CompareHolder<T>(T value) where T : ICompare<T> {
            public int Compare() => value.Compare(value);
        }
        public interface ISelf<T> where T : ISelf<T> { int Number(); }
        public sealed class Self : ISelf<Self> { public int Number() => 30; }
        public class Recursive<T>(T value) where T : ISelf<T> {
            public int Number() => value.Number();
        }
        public static class Relay<T> {
            public static T Upcast<U>(U value) where U : T => value;
        }
        public static class Templates {
            public static int Compare<T>(T value) where T : ICompare<T> => value.Compare(value);
            public static T Upcast<U, T>(U value) where U : T => value;
            public static object Read<T>(T value) where T : IRef<object> => value.Read();
            public static int ReadFixed<T>(T value) where T : ICompare<Good> => value.Compare(new Good());
        }
        "#,
        Some(("Contract", &contract)),
    );
    let source = r#"
        struct Record<T> { value: T }
        struct Wrapper { field: Bound.Val<Record<i32>> }
        struct Layer<T: Bound.ICompare<T>> { item: Package<T> }
        struct Package<T: Bound.ICompare<T>> { value: T }
        struct RefPackage<T: Bound.Parent> { value: Bound.Ref<T> }
        enum Checked<T: Bound.ISelf<T>> { None, Some(T) }
        struct UpcastBox<U, T: U> { value: T }
        struct ValueBox<T: struct> { value: T }
        enum ReferenceOption<T: class> { None, Some(T) }
        fn from_base<T: Bound.Parent>(value: T) -> Bound.IRef<T> {
            Bound.Api.KeepRef(value)
        }
        fn from_both<T: Bound.Parent + Bound.ITagged>(value: T) -> Bound.Both<T> {
            new Bound.Both<T>(value)
        }
        fn value_wrapper<T>(value: T) -> Wrapper {
            Wrapper { field: new Bound.Val<Record<i32>>(Record { value: 30 }) }
        }
        fn fixed<T: Bound.Good>(value: T) -> i32 { Bound.Templates.ReadFixed(value) }
        fn compare<T: Bound.ICompare<T>>(value: T) -> i32 { value.Compare(value) }
        fn call_compare<T: Bound.ICompare<T>>(value: T) -> i32 {
            compare(value) + Bound.Templates.Compare(value)
        }
        fn as_comparable<T: Bound.ICompare<T>>(value: T) -> Bound.ICompare<T> { value }
        fn compare_pair<U, T: Bound.ICompare<U>>(value: T, other: U) -> i32 {
            value.Compare(other)
        }
        fn recursive<T: Bound.ISelf<T>>(value: T) -> i32 { value.Number() }
        fn member<T: Bound.Val<i32>>(value: T) -> i32 { value.Value }
        fn lift<U, T: U>(value: T) -> U { value }
        fn chain<U: Bound.Parent, V: U, T: V>(value: T) -> i32 {
            lift::<U, T>(value).Offset() + value.Offset()
        }
        fn inherited<U: Bound.ICompare<U>, T: U>(value: T, other: U) -> i32 {
            value.Compare(other)
        }
        fn from_box<U, T: U>(boxed: UpcastBox<U, T>) -> U { boxed.value }
        fn fresh<T: new()>() -> T { new T() }
        fn zero<T: struct + new()>() -> ValueBox<T> { ValueBox { value: new T() } }
        fn keep_reference<T: class>(value: T) -> Bound.Ref<T> { Bound.Api.KeepRef(value) }
        fn reference<T: class>(value: ReferenceOption<T>) -> object {
            match value { Some(item) => item, None => "missing" }
        }
        fn via_imported_factory<T: new()>() -> T { Bound.Api.Create::<T>() }
        fn read_package<T: Bound.ICompare<T>>(package: Package<T>) -> i32 {
            compare(package.value)
        }
        fn make_layer<T: Bound.ICompare<T>>(value: T) -> Layer<T> {
            Layer { item: Package { value: value } }
        }
        fn read_checked<T: Bound.ISelf<T>>(value: Checked<T>) -> i32 {
            match value { Some(item) => recursive(item), None => 0 }
        }
        fn main() {
            let good = Bound.Maker::<Bound.Good>.Make();
            let covariant: Bound.IRef<object> = from_base(good);
            System.Console.WriteLine((covariant.Read() as Bound.Good).Tag());
            System.Console.WriteLine(Bound.Api.Tag(from_both(good)));
            System.Console.WriteLine(Bound.Api.Tag(new Bound.Derived()));
            System.Console.WriteLine(Bound.Api.Wrap().Value.Tag());
            System.Console.WriteLine(new Bound.Val<i32>(30).Value);
            System.Console.WriteLine(value_wrapper("x").field.Value.value);
            System.Console.WriteLine(Bound.Maker::<i64>.Make());
            System.Console.WriteLine(Bound.Api.Create::<Bound.Good>().Tag());
            System.Console.WriteLine(Bound.Api.KeepValue(40).Replace::<i64>(3000000000).Value);
            System.Console.WriteLine(Bound.Api.KeepRef("text").Read());
            System.Console.WriteLine(Bound.Maker::<i32>.Choose(0, "from generic owner"));
            System.Console.WriteLine(Bound.Api.Pick(30));
            System.Console.WriteLine(Bound.Api.Pick("fallback"));
            System.Console.WriteLine(new Bound.Dependent<i32, object>(30).Value);
            System.Console.WriteLine(Bound.Templates.Upcast::<i32, i32>(30));
            System.Console.WriteLine(Bound.Relay::<Bound.Parent>.Upcast(good).Offset());
            System.Console.WriteLine(new Bound.CompareHolder<Bound.Good>(good).Compare());
            System.Console.WriteLine(Bound.Templates.Compare(good));
            System.Console.WriteLine(new Bound.Recursive<Bound.Self>(new Bound.Self()).Number());
            System.Console.WriteLine(Bound.Templates.Read(new Bound.Ref<string>("covariant")));
            System.Console.WriteLine(fixed(good));
            System.Console.WriteLine(call_compare(good));
            System.Console.WriteLine(as_comparable(good).Compare(good));
            System.Console.WriteLine(recursive(new Bound.Self()));
            System.Console.WriteLine(member(new Bound.Val<i32>(30)));
            System.Console.WriteLine(compare_pair(good, good));
            System.Console.WriteLine(read_package(make_layer(good).item));
            System.Console.WriteLine(read_package(Package::<Bound.Good> { value: good }));
            System.Console.WriteLine(read_checked(Checked.Some::<Bound.Self>(new Bound.Self())));
            let reference: RefPackage<Bound.Good> = RefPackage { value: Bound.Api.KeepRef(good) };
            System.Console.WriteLine(reference.value.Read().Tag());
            System.Console.WriteLine(lift::<i32, i32>(30));
            System.Console.WriteLine(lift::<object, i32>(30));
            System.Console.WriteLine(chain::<Bound.Parent, Bound.Good, Bound.Good>(good));
            System.Console.WriteLine(inherited::<Bound.Good, Bound.Good>(good, good));
            System.Console.WriteLine(from_box(UpcastBox::<i32, i32> { value: 30 }));
            System.Console.WriteLine(fresh::<Bound.Good>().Tag());
            System.Console.WriteLine(zero::<i64>().value);
            System.Console.WriteLine(zero::<Bound.ConstructedValue>().value.Number);
            System.Console.WriteLine(keep_reference("reference").Read());
            System.Console.WriteLine(reference(ReferenceOption.Some::<string>("present")));
            System.Console.WriteLine(via_imported_factory::<Bound.Good>().Tag());
        }
    "#;
    fs::write(cwd.join("main.oxel"), source).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_oxel"))
        .current_dir(cwd)
        .args(["run", "main.oxel", "--reference"])
        .arg(&bridge)
        .arg("--reference")
        .arg(&contract)
        .output()
        .unwrap();
    assert_eq!(
        successful(result).lines().collect::<Vec<_>>(),
        [
            "25",
            "30",
            "30",
            "25",
            "30",
            "30",
            "0",
            "25",
            "3000000000",
            "text",
            "from generic owner",
            "value",
            "object",
            "30",
            "30",
            "5",
            "30",
            "30",
            "30",
            "covariant",
            "30",
            "60",
            "30",
            "30",
            "30",
            "30",
            "30",
            "30",
            "30",
            "25",
            "30",
            "30",
            "10",
            "30",
            "30",
            "25",
            "0",
            "31",
            "reference",
            "present",
            "25",
        ],
    );

    let mut references = oxel::References::default();
    references.add(&fs::read(&bridge).unwrap()).unwrap();
    for source in [
        "fn main() { let x: Bound.Both<string>; }",
        "fn main() { let x: Bound.CompareHolder<string>; }",
    ] {
        let missing = oxel::check_with_references(source, &references).unwrap_err();
        assert_eq!(missing[0].code, "E0211");
    }
    references.add(&fs::read(&contract).unwrap()).unwrap();
    for (invalid, code, bound) in [
        ("fn main() { let x: Bound.Ref<i32>; }", "E0212", "class"),
        (
            "fn main() { new Bound.Val<string>(\"x\"); }",
            "E0212",
            "struct",
        ),
        (
            "fn main() { let x: Bound.Both<Bound.NoTag>; }",
            "E0212",
            "Bound.ITagged",
        ),
        (
            "fn main() { let x: Bound.Both<string>; }",
            "E0212",
            "Bound.Parent",
        ),
        (
            "fn main() { Bound.Maker::<Bound.NoDefault>.Make(); }",
            "E0212",
            "new()",
        ),
        (
            "fn main() { Bound.Maker::<Bound.Parent>.Make(); }",
            "E0212",
            "new()",
        ),
        (
            "fn main() { Bound.Maker::<string>.Make(); }",
            "E0212",
            "new()",
        ),
        (
            "fn f<T: Bound.ITagged>(x: Bound.Ref<T>) {} fn main() {}",
            "E0212",
            "class",
        ),
        (
            "fn f<T: Bound.Parent>(x: Bound.Both<T>) {} fn main() {}",
            "E0212",
            "Bound.ITagged",
        ),
        (
            "struct S { field: Bound.Ref<S> } fn main() {}",
            "E0212",
            "class",
        ),
        (
            "fn main() { let x: Bound.Raw<i32>; }",
            "E0211",
            "unsupported",
        ),
        (
            "fn main() { let x: Bound.Dependent<string, i32>; }",
            "E0212",
            "System.Int32",
        ),
        (
            "fn main() { let x: Bound.CompareHolder<Bound.NoTag>; }",
            "E0212",
            "Bound.ICompare<Bound.NoTag>",
        ),
        (
            "fn main() { Bound.Templates.Compare(new Bound.NoTag()); }",
            "E0212",
            "Bound.ICompare<Bound.NoTag>",
        ),
        (
            "fn main() { Bound.Relay::<Bound.Parent>.Upcast(1); }",
            "E0212",
            "Bound.Parent",
        ),
        (
            "struct Bad { field: Bound.CompareHolder<Bad> } fn main() {}",
            "E0212",
            "Bound.ICompare<Bad>",
        ),
        (
            "fn compare<T: Bound.ICompare<T>>(value: T) {} fn main() { compare(1); }",
            "E0212",
            "Bound.ICompare<System.Int32>",
        ),
        (
            "fn bad<T: Bound.Val<string>>(value: T) {} fn main() {}",
            "E0212",
            "struct",
        ),
        (
            "fn bad<T: Bound.ICompare<T> + Bound.ICompare<T>>(value: T) {} fn main() {}",
            "E0211",
            "duplicate",
        ),
        (
            "struct Outer<T> { item: Package<T> } struct Package<T: Bound.ICompare<T>> { value: T } fn main() {}",
            "E0212",
            "Bound.ICompare<T>",
        ),
        (
            "struct Package<T: Bound.ICompare<T>> { value: T } fn main() { let value: Package<i32>; }",
            "E0212",
            "Bound.ICompare<System.Int32>",
        ),
        (
            "struct Package<T: Bound.ICompare<T>> { value: T } fn main() { Package::<i32> { value: 1 }; }",
            "E0212",
            "Bound.ICompare<System.Int32>",
        ),
        (
            "enum Checked<T: Bound.ISelf<T>> { None, Some(T) } fn main() { Checked.Some::<i32>(1); }",
            "E0212",
            "Bound.ISelf<System.Int32>",
        ),
        ("fn f<T: T>(value: T) {} fn main() {}", "E0211", "cyclic"),
        (
            "struct Bad<T: U, U: V, V: T> { value: T } fn main() {}",
            "E0211",
            "cyclic",
        ),
        (
            "fn f<U: Bound.Parent, T: U + Bound.Ref<string>>(value: T) {} fn main() {}",
            "E0211",
            "incompatible class bounds",
        ),
        (
            "fn f<U, T: U>(value: T) -> U { value } fn main() { f::<i32, string>(\"x\"); }",
            "E0212",
            "System.Int32",
        ),
        (
            "struct UpcastBox<U, T: U> { value: T } fn main() { UpcastBox::<i32, string> { value: \"x\" }; }",
            "E0212",
            "System.Int32",
        ),
        (
            "fn f<T: class>(value: T) {} fn main() { f(1); }",
            "E0212",
            "class",
        ),
        (
            "struct ValueBox<T: struct> { value: T } fn main() { let invalid: ValueBox<string>; }",
            "E0212",
            "struct",
        ),
        (
            "fn f<T: class + struct>(value: T) {} fn main() {}",
            "E0211",
            "incompatible",
        ),
        (
            "fn f<T: struct + class + new()>() {} fn main() {}",
            "E0211",
            "incompatible",
        ),
        (
            "fn f<T: new()>() -> T { new T() } fn main() { f::<Bound.NoDefault>(); }",
            "E0212",
            "new()",
        ),
        (
            "fn f<T>() -> T { new T() } fn main() {}",
            "E0212",
            "construction requires",
        ),
        (
            "fn f<T: new()>() -> T { new T(1) } fn main() {}",
            "E0205",
            "no arguments",
        ),
        (
            "fn f<U: new(), T: U>() -> T { new T() } fn main() {}",
            "E0212",
            "construction requires",
        ),
        (
            "fn f<U: struct, T: U>() {} fn main() {}",
            "E0211",
            "value type bounds",
        ),
        ("fn main() { Bound.Api.KeepRef(1); }", "E0212", "class"),
        (
            "fn main() { Bound.Api.KeepValue::<string>(\"x\"); }",
            "E0212",
            "struct",
        ),
        (
            "fn main() { Bound.Api.Create::<Bound.NoDefault>(); }",
            "E0212",
            "new()",
        ),
        (
            "struct Record { name: string } fn main() { Bound.Api.Create::<Record>(); }",
            "E0212",
            "new()",
        ),
        (
            "struct Holder { value: Bound.Constructible<Record> } struct Record { name: string } fn main() {}",
            "E0212",
            "new()",
        ),
        (
            "fn f<T: Bound.ITagged>(x: T) { Bound.Api.KeepRef(x); } fn main() {}",
            "E0212",
            "class",
        ),
        (
            "fn main() { new Bound.Val<i32>(1).Replace(\"x\"); }",
            "E0212",
            "struct",
        ),
        (
            "fn main() { Bound.Maker::<i32>.Choose(0, 1); }",
            "E0212",
            "class",
        ),
        (
            "fn main() { Bound.Api.Unmanaged(1); }",
            "E0203",
            "Unmanaged",
        ),
    ] {
        let errors = oxel::check_with_references(invalid, &references).unwrap_err();
        assert_eq!(errors[0].code, code, "{invalid}: {errors:?}");
        assert!(errors[0].message.contains(bound), "{invalid}: {errors:?}");
        assert!(
            errors[0].span.start < errors[0].span.end,
            "{invalid}: {errors:?}"
        );
        if invalid.contains("KeepValue::<string>") {
            assert_eq!(&invalid[errors[0].span.start..errors[0].span.end], "string");
        }
    }

    let assembly = oxel::compile_with_references(source, "SourceBounds", &references).unwrap();
    fs::write(cwd.join("SourceBounds.dll"), &assembly).unwrap();
    let mut downstream = oxel::References::default();
    downstream.add(&assembly).unwrap();
    downstream.add(&fs::read(&bridge).unwrap()).unwrap();
    downstream.add(&fs::read(&contract).unwrap()).unwrap();
    oxel::check_with_references(
        "fn main() { Program.compare(new Bound.Good()); Program.lift::<object, i32>(7); Program.fresh::<Bound.Good>(); }",
        &downstream,
    )
    .unwrap();
    let errors =
        oxel::check_with_references("fn main() { Program.compare(1); }", &downstream).unwrap_err();
    assert_eq!(errors[0].code, "E0212");

    let consumer = cwd.join("consumer");
    fs::create_dir(&consumer).unwrap();
    fs::write(
        consumer.join("Consumer.csproj"),
        format!(
            r#"<Project Sdk="Microsoft.NET.Sdk">
                <PropertyGroup><OutputType>Exe</OutputType><TargetFramework>net{major}.0</TargetFramework></PropertyGroup>
                <ItemGroup>
                    <Reference Include="SourceBounds"><HintPath>../SourceBounds.dll</HintPath></Reference>
                    <Reference Include="Contract"><HintPath>../contract/lib/Contract.dll</HintPath></Reference>
                    <Reference Include="Bridge"><HintPath>../bridge/lib/Bridge.dll</HintPath></Reference>
                </ItemGroup>
            </Project>"#
        ),
    )
    .unwrap();
    fs::write(
        consumer.join("Harness.cs"),
        r#"
        using System;
        using Bound;
        public struct Comparable : ICompare<Comparable> {
            public int Compare(Comparable other) => 31;
        }
        public struct Initialized {
            public int Value;
            public Initialized() => Value = 41;
        }
        public static class Harness {
            public static void Main() {
                var arg = new Comparable();
                Console.WriteLine(Program.call_compare(arg));
                Console.WriteLine(Program.as_comparable(arg).Compare(arg));
                Console.WriteLine(Program.recursive(new Self()));
                Console.WriteLine(Program.read_package(new Package<Comparable> { value = arg }));
                var parameter = typeof(Package<>).GetGenericArguments()[0];
                var constraint = parameter.GetGenericParameterConstraints()[0];
                Console.WriteLine(constraint.GetGenericTypeDefinition() == typeof(ICompare<>)
                    && constraint.GenericTypeArguments[0] == parameter);
                Console.WriteLine(Program.inherited<Comparable, Comparable>(arg, arg));
                Console.WriteLine(Program.from_box(new UpcastBox<object, int> { value = 7 }));
                var parameters = typeof(UpcastBox<,>).GetGenericArguments();
                Console.WriteLine(parameters[1].GetGenericParameterConstraints()[0] == parameters[0]);
                Console.WriteLine(Program.fresh<Good>().Tag());
                Console.WriteLine(Program.zero<Initialized>().value.Value);
                var valueParameter = typeof(ValueBox<>).GetGenericArguments()[0];
                Console.WriteLine(valueParameter.GenericParameterAttributes.HasFlag(
                    System.Reflection.GenericParameterAttributes.NotNullableValueTypeConstraint));
            }
        }
        "#,
    )
    .unwrap();
    let output = Command::new("dotnet")
        .current_dir(&consumer)
        .args(["run", "--verbosity", "quiet"])
        .output()
        .unwrap();
    assert_eq!(
        successful(output).lines().collect::<Vec<_>>(),
        [
            "62", "31", "30", "31", "True", "31", "7", "True", "25", "41", "True"
        ]
    );
}

#[test]
fn formats_clr_interpolation_parts_in_evaluation_order() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path();
    let sdk = successful(Command::new("dotnet").arg("--version").output().unwrap());
    let major = sdk.split('.').next().unwrap();
    let bridge = build_reference_fixture(
        &cwd.join("bridge"),
        major,
        "Bridge",
        "1.0.0.0",
        r#"
        public sealed class Probe(int digit) {
            public static int Trace;
            public static Probe Create(int digit) {
                Trace = Trace * 10 + digit;
                return new(digit);
            }
            public override string ToString() {
                Trace = Trace * 10 + (digit + 1);
                return "tag";
            }
            public static string UnexpectedNull() => null!;
        }
        "#,
        None,
    );
    fs::write(
        cwd.join("main.oxel"),
        r#"
        fn main() {
            System.Console.WriteLine($"{Probe.Create(1)}|{Probe.Create(3)}|{Probe.Trace}");
            let missing = $"{Probe.UnexpectedNull()}";
            System.Console.WriteLine(missing is string && missing == "");
        }
        "#,
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_oxel"))
        .current_dir(cwd)
        .args(["run", "main.oxel", "--reference"])
        .arg(bridge)
        .output()
        .unwrap();
    assert_eq!(
        successful(output).lines().collect::<Vec<_>>(),
        ["tag|tag|1234", "True"],
    );
}

#[test]
fn reads_and_writes_clr_default_indexers() {
    let folder = tempfile::tempdir().unwrap();
    let cwd = folder.path();
    let sdk = successful(Command::new("dotnet").arg("--version").output().unwrap());
    let major = sdk.split('.').next().unwrap();
    let bridge = build_reference_fixture(
        &cwd.join("bridge"),
        major,
        "Indexers",
        "1.0.0.0",
        r#"
        using System.Runtime.CompilerServices;
        namespace Indexers;
        public interface ICounter {
            int this[int index] { get; set; }
        }
        public interface IAnotherCounter {
            int this[int index] { get; }
        }
        public interface IAmbiguousCounter : ICounter, IAnotherCounter {}
        public class Counter : ICounter {
            public int Value;
            public Counter(int value) => Value = value;
            public virtual int this[int index] { get => Value + index; set => Value = value; }
            public int this[bool flag] => flag ? 9 : 0;
            public int this[long index] { set => Value = value + 1; }
        }
        public class Child() : Counter(1) {
            public override int this[int index] { get => Value + index + 1; set => Value = value + 1; }
        }
        public struct ValueCounter : ICounter {
            public int Value;
            public int this[int index] { get => Value + index; set => Value = value; }
        }
        public class Named<T>(T value) {
            private T stored = value;
            [IndexerName("Element")]
            public T this[int index] { get => stored; set => stored = value; }
        }
        public class NullableIndex {
            public string? this[string? key] { get => key; set {} }
        }
        public class Traced {
            public int Value = 10;
            public int this[int first, int second] {
                get { Trace.Mark(4); return Value; }
                set { Trace.Mark(6); Value = value; }
            }
        }
        public static class Trace {
            public static int Sequence;
            public static Traced Item = new();
            public static void Mark(int digit) { Sequence = Sequence * 10 + digit; }
            public static Traced Receiver() { Mark(1); return Item; }
            public static int First() { Mark(2); return 0; }
            public static int Second() { Mark(3); return 1; }
            public static int Right() { Mark(5); return 20; }
        }
        "#,
        None,
    );
    let source = r#"
        fn adjust<T: Indexers.ICounter>(value: T, replacement: T) -> i32 {
            let mut copy = value;
            copy[{ copy = replacement; 0 }] += 5;
            copy[0]
        }
        fn early_index(box: Indexers.Traced) -> i32 {
            box[{ return 1; }, 0] += 99;
            0
        }
        fn early_rhs(box: Indexers.Traced) -> i32 {
            box[0, 0] += { return 2; };
            0
        }
        fn main() {
            let child = new Indexers.Child();
            child[0] = 10;
            System.Console.WriteLine(child[0]);
            System.Console.WriteLine(child[true]);
            let wide: i64 = 0;
            child[wide] = 20;
            System.Console.WriteLine(child[0]);

            let record = new Indexers.Named<string>("before");
            record[0] += " after";
            System.Console.WriteLine(record[0]);

            let nullable = new Indexers.NullableIndex();
            nullable[null] = null;
            System.Console.WriteLine(nullable[null] ?? "fallback");

            Indexers.Trace.Receiver()[Indexers.Trace.First(), Indexers.Trace.Second()] += Indexers.Trace.Right();
            System.Console.WriteLine(Indexers.Trace.Sequence);
            System.Console.WriteLine(Indexers.Trace.Item.Value);
            System.Console.WriteLine(early_index(Indexers.Trace.Item));
            System.Console.WriteLine(early_rhs(Indexers.Trace.Item));

            let first = new Indexers.Counter(3);
            let second = new Indexers.Counter(20);
            System.Console.WriteLine(adjust(first, second));
            System.Console.WriteLine(first.Value);
        }
    "#;
    fs::write(cwd.join("main.oxel"), source).unwrap();
    let run = Command::new(env!("CARGO_BIN_EXE_oxel"))
        .current_dir(cwd)
        .args(["run", "main.oxel", "--reference"])
        .arg(&bridge)
        .output()
        .unwrap();
    assert_eq!(
        successful(run).lines().collect::<Vec<_>>(),
        [
            "12",
            "9",
            "22",
            "before after",
            "fallback",
            "123456",
            "30",
            "1",
            "2",
            "20",
            "8"
        ],
    );
    let mut references = oxel::References::default();
    references.add(&fs::read(&bridge).unwrap()).unwrap();
    for (source, code) in [
        (
            "fn read(value: Indexers.IAmbiguousCounter) -> i32 { value[0] } fn main() {}",
            "E0208",
        ),
        ("fn main() { new Indexers.Counter(1)[true] += 1; }", "E0308"),
        ("fn main() { new Indexers.Counter(1)[true] = 1; }", "E0308"),
        (
            "fn main() { let i: i64 = 1; new Indexers.Counter(1)[i]; }",
            "E0308",
        ),
        ("fn main() { new Indexers.Counter(1)[0, 1]; }", "E0205"),
        ("fn main() { new Indexers.Counter(1)[\"x\"]; }", "E0205"),
        ("fn main() { new Indexers.Counter(1)[0] = true; }", "E0301"),
        (
            "fn main() { let x: Indexers.Counter? = null; x[0]; }",
            "E0311",
        ),
        (
            "fn main() { let x: i32; new Indexers.Counter(1)[x] += 1; }",
            "E0321",
        ),
        ("fn main() { new Indexers.Counter(1)[]; }", "E0102"),
        ("fn main() { let x = 1; x[0]; }", "E0305"),
        ("fn main() { new Indexers.Named<i32>(1).Element; }", "E0305"),
    ] {
        let errors = oxel::check_with_references(source, &references).unwrap_err();
        assert_eq!(errors[0].code, code, "{source}: {errors:?}");
    }

    let assembly = oxel::compile_with_references(source, "IndexApp", &references).unwrap();
    fs::write(cwd.join("IndexApp.dll"), assembly).unwrap();
    let harness = cwd.join("harness");
    fs::create_dir(&harness).unwrap();
    fs::write(
        harness.join("Harness.csproj"),
        format!(
            r#"<Project Sdk="Microsoft.NET.Sdk">
            <PropertyGroup>
                <TargetFramework>net{major}.0</TargetFramework>
                <OutputType>Exe</OutputType>
            </PropertyGroup>
            <ItemGroup>
                <Reference Include="Indexers"><HintPath>{}</HintPath></Reference>
                <Reference Include="IndexApp"><HintPath>../IndexApp.dll</HintPath></Reference>
            </ItemGroup>
        </Project>"#,
            bridge.display()
        ),
    )
    .unwrap();
    fs::write(
        harness.join("Harness.cs"),
        r#"public static class Harness {
            public static void Main() {
                var first = new Indexers.ValueCounter { Value = 3 };
                var second = new Indexers.ValueCounter { Value = 20 };
                System.Console.WriteLine(Program.adjust(first, second));
            }
        }"#,
    )
    .unwrap();
    let harness = Command::new("dotnet")
        .current_dir(harness)
        .args(["run", "--verbosity", "quiet"])
        .output()
        .unwrap();
    assert_eq!(successful(harness).trim(), "25");
}

#[test]
fn compiler_queries_read_imported_field_names_without_loading_clr() {
    let folder = tempfile::tempdir().unwrap();
    let cwd = folder.path();
    let sdk = successful(Command::new("dotnet").arg("--version").output().unwrap());
    let assembly = build_reference_fixture(
        &cwd.join("model"),
        sdk.split('.').next().unwrap(),
        "ReflectionModel",
        "1.0.0.0",
        r#"
        namespace Model;
        public class Base { public int Inherited; }
        public class Box<T> : Base {
            public T Zulu = default!;
            public static int StaticField;
            private int hidden = 0;
            public int Value => hidden;
            public readonly int Alpha;
        }
        public class Partial { public decimal Amount; }
        "#,
        None,
    );
    fs::write(
        cwd.join("main.oxel"),
        r#"
        const TYPE = compiler.type_name::<Model.Box<i32>>();
        const FIRST = compiler.field_names::<Model.Box<i32>>()[0];
        const SECOND = compiler.field_names::<Model.Box<i32>>()[1];
        const COUNT = compiler.field_names::<Model.Box<i32>?>().Length;
        comptime fn first_field<T>() -> compiler.FieldInfo {
            compiler.fields_of::<T>()[0]
        }
        const ZULU: compiler.FieldInfo = first_field::<Model.Box<string>>();
        const ALPHA = compiler.fields_of::<Model.Box<string>>()[1];
        const PARTIAL_FIELD = compiler.field_names::<Model.Partial>()[0];
        fn main() {
            System.Console.WriteLine(TYPE);
            System.Console.WriteLine(FIRST);
            System.Console.WriteLine(SECOND);
            System.Console.WriteLine(COUNT);
            System.Console.WriteLine(ZULU.type_name);
            System.Console.WriteLine(ZULU.readonly);
            System.Console.WriteLine(ALPHA.type_name);
            System.Console.WriteLine(ALPHA.readonly);
            System.Console.WriteLine(PARTIAL_FIELD);
        }
        "#,
    )
    .unwrap();
    successful(
        Command::new(env!("CARGO_BIN_EXE_oxel"))
            .current_dir(cwd)
            .env("PATH", "")
            .args(["check", "main.oxel", "--reference"])
            .arg(&assembly)
            .output()
            .unwrap(),
    );
    let output = Command::new(env!("CARGO_BIN_EXE_oxel"))
        .current_dir(cwd)
        .args(["run", "main.oxel", "--reference"])
        .arg(&assembly)
        .output()
        .unwrap();
    assert_eq!(
        successful(output).lines().collect::<Vec<_>>(),
        [
            "Model.Box<System.Int32>",
            "Zulu",
            "Alpha",
            "2",
            "System.String",
            "False",
            "System.Int32",
            "True",
            "Amount"
        ]
    );
    let mut references = oxel::References::default();
    references.add(&fs::read(assembly).unwrap()).unwrap();
    let source = "const FIELD = compiler.fields_of::<Model.Partial>()[0]; fn main() {}";
    let errors = oxel::check_with_references(source, &references).unwrap_err();
    assert_eq!(errors[0].code, "E0501");
    assert!(errors[0].message.contains("Amount"));
}

#[test]
fn clr_argument_contracts_refine_branches_and_normal_returns() {
    let folder = tempfile::tempdir().unwrap();
    let cwd = folder.path();
    let sdk = successful(Command::new("dotnet").arg("--version").output().unwrap());
    let bridge = build_reference_fixture(
        &cwd.join("bridge"),
        sdk.split('.').next().unwrap(),
        "BranchContracts",
        "1.0.0.0",
        r#"
        using System.Diagnostics.CodeAnalysis;
        namespace Contracts;

        public static class Branch {
            public static bool Present([NotNullWhen(true)] string? text) => text != null;
            public static bool Missing([NotNullWhen(false)] string? text) => text == null;

            public static void Require([NotNull] object? value) {
                if (value == null) throw new System.ArgumentNullException(nameof(value));
            }

            public static void Read([NotNull] out string? value) => value = "output";

            public static bool Check([NotNull] string? value) {
                Require(value);
                return false;
            }

            public static void Reject([DoesNotReturnIf(true)] bool invalid) {
                if (invalid) throw new System.ArgumentException();
            }

            public static bool TryRead(bool found, [NotNullWhen(true)] out string? value) {
                value = found ? "found" : null;
                return found;
            }

            public static bool TryReadGeneric<T>(
                bool found, T input, [MaybeNullWhen(false)] out T value) {
                value = found ? input : default!;
                return found;
            }
        }

        public class Reader {
            public bool TryRead(bool found, [NotNullWhen(true)] out string? value) {
                return Branch.TryRead(found, out value);
            }
        }

        public struct Inspector {
            public Inspector() {}
            public bool Ready([NotNullWhen(true)] string? text) => text != null;
            public void Require([NotNull] string? text) => Branch.Require(text);
            public bool Accept([DoesNotReturnIf(false)] bool valid) {
                Branch.Reject(!valid);
                return false;
            }
        }
        "#,
        None,
    );
    let source = r#"
        fn show(text: string) { System.Console.WriteLine(text); }

        fn by_value(text: string?) {
            if Contracts.Branch.Present(text) && text != "" { show(text); }
            if Contracts.Branch.Missing(text) == false { show(text); }
            if Contracts.Branch.Missing(text) || text == "" {} else { show(text); }
            let inspector = new Contracts.Inspector();
            if inspector.Ready(text) { show(text); }
            while Contracts.Branch.Present(text) { show(text); break; }
            match true {
                _ if Contracts.Branch.Present(text) => show(text),
                _ => {},
            };
        }

        fn read(found: bool) -> string {
            let text: string?;
            if !Contracts.Branch.TryRead(found, out text) { return "early missing"; }
            text
        }

        fn after_return(first: string?, second: string?, third: string?, fourth: string?) {
            Contracts.Branch.Require(first);
            show(first);
            let output: string?;
            Contracts.Branch.Read(out output);
            show(output);
            let inspector = new Contracts.Inspector();
            inspector.Require(second);
            show(second);
            if Contracts.Branch.Check(third) { show(third); } else { show(third); }
            show(third);
            show({ Contracts.Branch.Require(fourth); "nested call" });
            show(fourth);
        }

        fn after_guard(first: string?, second: string?, third: string?) {
            Contracts.Branch.Reject(first == null || second == null);
            show(first);
            show(second);
            let inspector = new Contracts.Inspector();
            if inspector.Accept(third != null) || third != "" { show(third); }
            show(third);
        }

        fn main() {
            by_value("value");
            by_value(null);
            let text: string?;
            let reader = new Contracts.Reader();
            if reader.TryRead(true, out text) && text != "" { show(text); }
            let missing: string?;
            if !Contracts.Branch.TryRead(false, out missing) {
                System.Console.WriteLine(missing ?? "absent");
            }
            let generic: string?;
            if Contracts.Branch.TryReadGeneric(true, "generic", out generic) { show(generic); }
            show(read(true));
            show(read(false));

            let mut found = true;
            while {
                let loop_text: string?;
                if Contracts.Branch.TryRead(found, out loop_text) { show(loop_text); }
                found
            } {
                found = false;
            }
            after_return("first", "second", "third", "fourth");
            after_guard("guard one", "guard two", "guard three");
        }
    "#;
    fs::write(cwd.join("main.oxel"), source).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_oxel"))
        .current_dir(cwd)
        .args(["run", "main.oxel", "--reference"])
        .arg(&bridge)
        .output()
        .unwrap();
    assert_eq!(
        successful(output).lines().collect::<Vec<_>>(),
        [
            "value",
            "value",
            "value",
            "value",
            "value",
            "value",
            "found",
            "absent",
            "generic",
            "found",
            "early missing",
            "found",
            "first",
            "output",
            "second",
            "third",
            "third",
            "nested call",
            "fourth",
            "guard one",
            "guard two",
            "guard three",
            "guard three",
        ]
    );
    let mut references = oxel::References::default();
    references.add(&fs::read(&bridge).unwrap()).unwrap();
    for source in [
        "fn f(x: string?) -> string { if Contracts.Branch.Missing(x) { x } else { \"absent\" } } fn main() {}",
        "fn f(x: string?) -> string { if Contracts.Branch.Present(x) || true { x } else { \"absent\" } } fn main() {}",
        "fn main() { let x: string?; if Contracts.Branch.TryRead(false, out x) {} else { let nonnull: string = x; } }",
        "fn main() { let mut x: string?; if Contracts.Branch.TryRead(true, out x) { x = null; let nonnull: string = x; } }",
        "fn main() { let x: string?; if Contracts.Branch.TryReadGeneric::<string?>(true, null, out x) { let nonnull: string = x; } }",
        "fn main() { let x: string?; Contracts.Branch.TryRead(true, out x); let nonnull: string = x; }",
        "fn f(x: string?) -> string { if false { Contracts.Branch.Require(x); } x } fn main() {}",
        "fn main() { let mut x: string? = \"hi\"; Contracts.Branch.Require(x); x = null; let nonnull: string = x; }",
        "fn f(x: string?, y: string?) -> string { Contracts.Branch.Reject(x == null && y == null); x } fn main() {}",
        "fn f(x: string?) -> string { Contracts.Branch.Reject(x != null); x } fn main() {}",
    ] {
        let errors = oxel::check_with_references(source, &references).unwrap_err();
        assert_eq!(errors[0].code, "E0301", "{source}: {errors:?}");
    }
}

#[test]
fn clr_conditional_return_nullability_uses_the_actual_argument() {
    let folder = tempfile::tempdir().unwrap();
    let cwd = folder.path();
    let sdk = successful(Command::new("dotnet").arg("--version").output().unwrap());
    let bridge = build_reference_fixture(
        &cwd.join("bridge"),
        sdk.split('.').next().unwrap(),
        "ConditionalContracts",
        "1.0.0.0",
        r#"
        using System.Diagnostics.CodeAnalysis;
        namespace Contracts;

        public delegate void Callback();

        public static class Conditional {
            [return: NotNullIfNotNull(nameof(value))]
            public static string? Echo(string? value) => value;

            public static string? Echo(object? value) => null;

            [return: NotNullIfNotNull(nameof(value))]
            public static T EchoGeneric<T>(T value) => value;

            [return: NotNullIfNotNull(nameof(value))]
            public static string? Box(object? value) => value?.ToString();

            [return: NotNullIfNotNull(nameof(left)), NotNullIfNotNull(nameof(right))]
            public static string? Either(string? left, string? right) => left ?? right;

            [return: NotNullIfNotNull(nameof(value))]
            public static string? Default(string? value = "default") => value;

            [return: NotNullIfNotNull(nameof(value))]
            public static string? Missing(string? value = null) => value;

            [return: NotNullIfNotNull(nameof(values))]
            public static string? Pack(params string?[]? values) => values?.Length.ToString();

            [return: NotNullIfNotNull(nameof(action))]
            public static string? FromDelegate(Callback? action) => action is null ? null : "delegate";

            [return: NotNullIfNotNull("missingName")]
            public static string? Incorrect(string? value) => null;
        }

        public class Table {
            [NotNullIfNotNull("key")]
            public string? this[string? key] => key;
        }
        "#,
        None,
    );
    let source = r#"
        fn display(value: Option<string>) -> string { value ?? "absent" }
        fn refined(value: string?) -> string {
            if value != null { Contracts.Conditional.Echo(value) } else { "fallback" }
        }
        fn main() {
            let value = "value";
            let present: string = Contracts.Conditional.Echo(value);
            let generic: string = Contracts.Conditional.EchoGeneric::<string?>(value);
            let boxed: string = Contracts.Conditional.Box(42);
            let either: string = Contracts.Conditional.Either(null, value);
            let first: string = Contracts.Conditional.Either(value, null);
            let default_value: string = Contracts.Conditional.Default();
            let packed: string = Contracts.Conditional.Pack(null, null);
            let callback: string = Contracts.Conditional.FromDelegate() {};
            let table = new Contracts.Table();
            let indexed: string = table[value];
            let absent_option: Option<string> = Option.None;
            let object_value: object = "object";
            let from_other_overload = Contracts.Conditional.Echo(object_value);
            let from_option = Contracts.Conditional.Echo(absent_option);
            let absent = Contracts.Conditional.Missing();
            let absent_index = table[null];
            System.Console.WriteLine(present);
            System.Console.WriteLine(generic);
            System.Console.WriteLine(boxed);
            System.Console.WriteLine(either);
            System.Console.WriteLine(first);
            System.Console.WriteLine(default_value);
            System.Console.WriteLine(packed);
            System.Console.WriteLine(callback);
            System.Console.WriteLine(indexed);
            System.Console.WriteLine(refined("refined"));
            System.Console.WriteLine(display(from_other_overload));
            System.Console.WriteLine(display(from_option));
            System.Console.WriteLine(display(absent));
            System.Console.WriteLine(display(absent_index));
        }
    "#;
    fs::write(cwd.join("main.oxel"), source).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_oxel"))
        .current_dir(cwd)
        .args(["run", "main.oxel", "--reference"])
        .arg(&bridge)
        .output()
        .unwrap();
    assert_eq!(
        successful(output).lines().collect::<Vec<_>>(),
        [
            "value", "value", "42", "value", "value", "default", "2", "delegate", "value",
            "refined", "absent", "absent", "absent", "absent"
        ]
    );
    let mut references = oxel::References::default();
    references.add(&fs::read(&bridge).unwrap()).unwrap();
    for source in [
        "fn main() { let x: string = Contracts.Conditional.Echo(null); }",
        "fn f(input: string?) -> string { Contracts.Conditional.Echo(input) } fn main() {}",
        "fn main() { let x: string = Contracts.Conditional.Missing(); }",
        "fn main() { let x: string = Contracts.Conditional.Pack(null); }",
        "fn main() { let x: string = Contracts.Conditional.Incorrect(\"present\"); }",
        "fn main() { let t = new Contracts.Table(); let x: string = t[null]; }",
    ] {
        let errors = oxel::check_with_references(source, &references).unwrap_err();
        assert_eq!(errors[0].code, "E0301", "{source}: {errors:?}");
    }
}

#[test]
fn clr_input_nullability_contracts_control_calls_and_writes() {
    let folder = tempfile::tempdir().unwrap();
    let cwd = folder.path();
    let sdk = successful(Command::new("dotnet").arg("--version").output().unwrap());
    let bridge = build_reference_fixture(
        &cwd.join("bridge"),
        sdk.split('.').next().unwrap(),
        "InputContracts",
        "1.0.0.0",
        r#"
        using System.Diagnostics.CodeAnalysis;
        namespace Contracts;

        public static class Inputs {
            public static string Label([AllowNull] string text) => text ?? "label";
            public static string Required([DisallowNull] string? text) => text!;
            public static string Generic<T>([AllowNull] T value) => value?.ToString() ?? "generic";
            public static string GenericRequired<T>([DisallowNull] T value) => value!.ToString()!;
            public static int Count([AllowNull] params string[] items) => items?.Length ?? -1;
            public static int FlexibleCount<T>([DisallowNull] params T[]? items) => items!.Length;
            public static string Select([DisallowNull] string? value) => "string";
            public static string Select(object? value) => "object";
        }

        public class Packet<T> {
            [AllowNull]
            public T Field = default!;

            [DisallowNull]
            public string? RequiredField;

            [AllowNull]
            public string Label {
                get => _label;
                set => _label = value ?? "property";
            }
            private string _label = "initial";

            public string? Required {
                get => RequiredField;
                [param: DisallowNull]
                set => RequiredField = value!;
            }

            [AllowNull]
            public string this[int index] {
                get => Label;
                set => Label = value;
            }
        }
        "#,
        None,
    );
    let source = r#"
        fn display(value: Option<string>) -> string { value ?? "absent" }
        fn forward<T>(value: T) -> string { Contracts.Inputs.Generic::<T>(value) }
        fn main() {
            let absent: Option<string> = Option.None;
            System.Console.WriteLine(Contracts.Inputs.Label(absent));
            System.Console.WriteLine(Contracts.Inputs.Required("required"));
            System.Console.WriteLine(Contracts.Inputs.Generic::<string>(null));
            System.Console.WriteLine(Contracts.Inputs.GenericRequired::<string?>("generic required"));
            System.Console.WriteLine(forward(30));
            System.Console.WriteLine(Contracts.Inputs.Count(null));
            System.Console.WriteLine(Contracts.Inputs.Count("one", "two"));
            System.Console.WriteLine(Contracts.Inputs.FlexibleCount::<string?>(null, null));
            System.Console.WriteLine(Contracts.Inputs.Select(null));
            System.Console.WriteLine(Contracts.Inputs.Select("value"));
            let packet = new Contracts.Packet<string>();
            packet.Field = null;
            let from_field = packet.Field;
            let unassigned = packet.Required;
            System.Console.WriteLine(display(from_field));
            System.Console.WriteLine(display(unassigned));
            packet.Required = "packet";
            packet.Label = null;
            let label: string = packet.Label;
            System.Console.WriteLine(label);
            packet[0] = "indexed";
            let indexed: string = packet[0];
            System.Console.WriteLine(indexed);
            packet[0] = null;
            let reset: string = packet[0];
            System.Console.WriteLine(reset);
        }
    "#;
    fs::write(cwd.join("main.oxel"), source).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_oxel"))
        .current_dir(cwd)
        .args(["run", "main.oxel", "--reference"])
        .arg(&bridge)
        .output()
        .unwrap();
    assert_eq!(
        successful(output).lines().collect::<Vec<_>>(),
        [
            "label",
            "required",
            "generic",
            "generic required",
            "30",
            "-1",
            "2",
            "2",
            "object",
            "string",
            "absent",
            "absent",
            "property",
            "indexed",
            "property",
        ]
    );
    let mut references = oxel::References::default();
    references.add(&fs::read(&bridge).unwrap()).unwrap();
    for source in [
        "fn main() { Contracts.Inputs.Required(null); }",
        "fn main() { let x: string? = null; Contracts.Inputs.GenericRequired(x); }",
        "fn main() { Contracts.Inputs.GenericRequired::<string?>(null); }",
        "fn main() { Contracts.Inputs.Count(null, null); }",
        "fn main() { let p = new Contracts.Packet<string>(); p.RequiredField = null; }",
        "fn main() { let p = new Contracts.Packet<string>(); p.Required = null; }",
        "fn main() { let p = new Contracts.Packet<string>(); let x: string = p.Field; }",
    ] {
        let errors = oxel::check_with_references(source, &references).unwrap_err();
        assert!(
            matches!(errors[0].code, "E0301" | "E0205"),
            "{source}: {errors:?}"
        );
    }
}

#[test]
fn clr_read_nullability_contracts_apply_after_generic_substitution() {
    let folder = tempfile::tempdir().unwrap();
    let cwd = folder.path();
    let sdk = successful(Command::new("dotnet").arg("--version").output().unwrap());
    let bridge = build_reference_fixture(
        &cwd.join("bridge"),
        sdk.split('.').next().unwrap(),
        "ReadContracts",
        "1.0.0.0",
        r#"
        using System.Diagnostics.CodeAnalysis;
        namespace Contracts;

        public struct Number<T> {
            public T Value;
        }

        public static class Lookup {
            [return: MaybeNull]
            public static T Missing<T>() => default;

            [return: NotNull]
            public static T Present<T>(T value) => value!;

            [return: MaybeNull]
            public static Number<int> Number() => new Number<int> { Value = 30 };

            [return: MaybeNull]
            public static T DefaultNumber<T>() where T : struct => default;

            [return: MaybeNull]
            public static string?[] MaybeArray() => null;

            [return: NotNull]
            public static string?[]? PresentArray() => new string?[] { null };
        }

        public class Packet<T> {
            [MaybeNull]
            public T Field = default!;

            [MaybeNull]
            public T Optional { get; set; } = default!;

            [NotNull]
            public string? Required {
                get => "property";
                set {}
            }

            public T this[int index] {
                [return: MaybeNull]
                get => default!;
                set {}
            }
        }

        public class Readonly {
            [NotNull]
            public readonly string? Value = "readonly";
        }
        "#,
        None,
    );
    let source = r#"
        fn unwrap(value: Option<string>) -> string { value ?? "missing" }
        fn default_number<T: struct>() -> T { Contracts.Lookup.DefaultNumber::<T>() }
        fn main() {
            let packet = new Contracts.Packet<string>();
            packet.Optional = "present";
            packet.Required = null;
            packet[0] = "accepted";
            let from_field = packet.Field;
            let from_property = packet.Optional;
            let from_indexer = packet[0];
            let from_method = Contracts.Lookup.Missing::<string>();
            System.Console.WriteLine(unwrap(from_field));
            System.Console.WriteLine(unwrap(from_property));
            System.Console.WriteLine(unwrap(from_indexer));
            System.Console.WriteLine(unwrap(from_method));
            let present: string = Contracts.Lookup.Present::<string?>("value");
            let property: string = packet.Required;
            let readonly: string = (new Contracts.Readonly()).Value;
            System.Console.WriteLine(present);
            System.Console.WriteLine(property);
            System.Console.WriteLine(readonly);
            let numeric: i32 = Contracts.Lookup.Missing::<i32>() + default_number::<i32>();
            let number = Contracts.Lookup.Number();
            System.Console.WriteLine(numeric + number.Value);
            let array: Option<string?[]> = Contracts.Lookup.MaybeArray();
            System.Console.WriteLine((array ?? [null])[0] ?? "array absent");
            let present_array: string?[] = Contracts.Lookup.PresentArray();
            System.Console.WriteLine(present_array[0] ?? "element absent");
        }
    "#;
    fs::write(cwd.join("main.oxel"), source).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_oxel"))
        .current_dir(cwd)
        .args(["run", "main.oxel", "--reference"])
        .arg(&bridge)
        .output()
        .unwrap();
    assert_eq!(
        successful(output).lines().collect::<Vec<_>>(),
        [
            "missing",
            "present",
            "missing",
            "missing",
            "value",
            "property",
            "readonly",
            "30",
            "array absent",
            "element absent"
        ]
    );
    let mut references = oxel::References::default();
    references.add(&fs::read(&bridge).unwrap()).unwrap();
    for source in [
        "fn main() { let x: string = Contracts.Lookup.Missing::<string>(); }",
        "fn main() { let p = new Contracts.Packet<string>(); p.Field = null; }",
        "fn main() { let p = new Contracts.Packet<string>(); p.Optional = null; }",
        "fn main() { let p = new Contracts.Packet<string>(); p[0] = null; }",
        "fn main() { let x: string[] = Contracts.Lookup.PresentArray(); }",
    ] {
        let errors = oxel::check_with_references(source, &references).unwrap_err();
        assert_eq!(errors[0].code, "E0301", "{source}: {errors:?}");
    }
}

#[test]
fn exchanges_clr_vectors_and_nullable_annotations_with_csharp() {
    let folder = tempfile::tempdir().unwrap();
    let cwd = folder.path();
    let sdk = successful(Command::new("dotnet").arg("--version").output().unwrap());
    let major = sdk.split('.').next().unwrap();
    let bridge = build_reference_fixture(
        &cwd.join("bridge"),
        major,
        "Arrays",
        "1.0.0.0",
        r#"
        namespace Arrays;
        public static class Vectors {
            public static T[] Echo<T>(T[] values) => values;
            public static int Append(int[] values) => values[0] += 20;
            public static string?[]? Maybe() => new string?[] { null };
            public static string[] Words() => new[] { "CLR" };
            public static int[][] Grid() => new[] { new[] { 1 }, new[] { 2, 3 } };
        }
        public class Packet {
            public int[] Data { get; set; } = new int[1];
            public int[] Field = new int[1];
        }
        "#,
        None,
    );
    let source = r#"
        fn reply(values: string?[]?) -> string?[]? { values }
        fn first<T>(values: T[]) -> T { values[0] }
        fn main() {
            let packet = new Arrays.Packet();
            packet.Data = [10];
            packet.Field[0] = 42;
            System.Console.WriteLine(Arrays.Vectors.Append(packet.Data));
            System.Console.WriteLine(packet.Data[0]);
            System.Console.WriteLine(packet.Field[0]);
            System.Console.WriteLine(first(Arrays.Vectors.Echo([4, 8])));
            System.Console.WriteLine(Arrays.Vectors.Words()[0]);
            System.Console.WriteLine(Arrays.Vectors.Grid()[1][1]);
            System.Console.WriteLine((Arrays.Vectors.Maybe() ?? [null])[0] ?? "empty");
        }
    "#;
    fs::write(cwd.join("main.oxel"), source).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_oxel"))
        .current_dir(cwd)
        .args(["run", "main.oxel", "--reference"])
        .arg(&bridge)
        .output()
        .unwrap();
    assert_eq!(
        successful(output).lines().collect::<Vec<_>>(),
        ["30", "30", "42", "4", "CLR", "3", "empty"]
    );
    let mut references = oxel::References::default();
    references.add(&fs::read(&bridge).unwrap()).unwrap();
    for (source, code) in [
        ("fn main() { Arrays.Vectors.Maybe()[0]; }", "E0311"),
        (
            "fn main() { let words: string[] = Arrays.Vectors.Maybe() ?? [null]; }",
            "E0301",
        ),
        ("fn main() { Arrays.Vectors.Words()[0] = null; }", "E0301"),
    ] {
        let errors = oxel::check_with_references(source, &references).unwrap_err();
        assert_eq!(errors[0].code, code, "{source}: {errors:?}");
    }
    let assembly = oxel::compile_with_references(source, "VectorApp", &references).unwrap();
    fs::write(cwd.join("VectorApp.dll"), assembly).unwrap();
    let consumer = cwd.join("consumer");
    fs::create_dir(&consumer).unwrap();
    fs::write(
        consumer.join("Consumer.csproj"),
        format!(
            r#"<Project Sdk="Microsoft.NET.Sdk">
            <PropertyGroup><TargetFramework>net{major}.0</TargetFramework>
                <OutputType>Exe</OutputType><Nullable>enable</Nullable></PropertyGroup>
            <ItemGroup>
                <Reference Include="VectorApp"><HintPath>../VectorApp.dll</HintPath></Reference>
            </ItemGroup>
        </Project>"#
        ),
    )
    .unwrap();
    fs::write(
        consumer.join("Consumer.cs"),
        r#"
        using System;
        using System.Reflection;
        public static class Consumer {
            public static void Main() {
                Console.WriteLine(Program.first(new[] { 30 }));
                var method = typeof(Program).GetMethod("reply")!;
                var ctx = new NullabilityInfoContext();
                var result = ctx.Create(method.ReturnParameter);
                var parameter = ctx.Create(method.GetParameters()[0]);
                Console.WriteLine($"{result.ReadState}/{result.ElementType!.ReadState}");
                Console.WriteLine($"{parameter.ReadState}/{parameter.ElementType!.ReadState}");
            }
        }
    "#,
    )
    .unwrap();
    let result = Command::new("dotnet")
        .current_dir(consumer)
        .args(["run", "--verbosity", "quiet"])
        .output()
        .unwrap();
    assert_eq!(
        successful(result).lines().collect::<Vec<_>>(),
        ["30", "Nullable/Nullable", "Nullable/Nullable"]
    );
}

#[test]
fn clr_structs_preserve_copy_mutation_and_generic_dispatch() {
    let folder = tempfile::tempdir().unwrap();
    let cwd = folder.path();
    let sdk = successful(Command::new("dotnet").arg("--version").output().unwrap());
    let fixture = build_reference_fixture(
        &cwd.join("values"),
        sdk.split('.').next().unwrap(),
        "Values",
        "1.0.0.0",
        r#"
        namespace Interop;
        public interface ICounter {
            int Current { get; }
            void Advance();
        }
        public struct Counter : ICounter {
            public int Field;
            public Counter(int value) => Field = value;
            public int Current { get => Field; set => Field = value; }
            public int this[int offset] {
                get => Field + offset;
                set => Field = value - offset;
            }
            public void Advance() => Field++;
            public static Counter Echo(Counter value) => value;
        }
        public struct Cell<T> where T : struct {
            public T Stored;
            public T this[int index] { get => Stored; set => Stored = value; }
            public Cell(T item) => Stored = item;
            public Cell<T> Copy() => this;
            public static Cell<T> From(T value) => new(value);
        }
        public struct Slot<T> {
            public T Item;
            public Slot(T item) => Item = item;
        }
        public ref struct Restricted {
            public int Field;
        }
        public static class Api {
            public static Restricted Hidden() => new Restricted();
            public static T Unwrap<T>(Cell<T> value) where T : struct => value.Stored;
            public static Slot<string> Text() => new("present");
            public static Slot<string?> NullableText() => new(null);
        }
        "#,
        None,
    );
    let source = r#"
        fn observe<T: Interop.ICounter>(input: T) -> i32 {
            input.Advance();
            input.Current
        }

        fn advance<T: Interop.ICounter>(input: T) -> T {
            let mut value = input;
            value.Advance();
            value
        }

        fn duplicate<T: struct>(cell: Interop.Cell<T>) -> Interop.Cell<T> {
            cell.Copy()
        }

        fn main() {
            let initial = Interop.Counter.Echo(new Interop.Counter(10));
            initial.Advance();
            System.Console.WriteLine(initial.Field);
            System.Console.WriteLine(observe(initial));

            let mut copy = advance(initial);
            copy.Field += 2;
            copy.Current += 3;
            let mut times = 0;
            copy[{ times += 1; 2 }] += 4;
            System.Console.WriteLine(times);
            System.Console.WriteLine(copy.Field);
            System.Console.WriteLine(copy[5]);
            System.Console.WriteLine(initial.Field);

            let boxed: Interop.ICounter = copy;
            boxed.Advance();
            System.Console.WriteLine(boxed.Current);
            System.Console.WriteLine(copy.Current);
            let unboxed = boxed as Interop.Counter;
            System.Console.WriteLine(unboxed.Field);

            let mut cell = Interop.Cell::<i32>.From(10);
            cell.Stored += 10;
            cell[0] += 10;
            let original = new Interop.Cell<i32>(8);
            let copy = duplicate(cell);
            cell.Stored = 90;
            System.Console.WriteLine(Interop.Api.Unwrap(copy));
            let boxed_cell: object = original;
            System.Console.WriteLine((boxed_cell as Interop.Cell<i32>).Stored);
            System.Console.WriteLine(Interop.Api.Text().Item);
            System.Console.WriteLine(Interop.Api.NullableText().Item ?? "empty");
        }
    "#;
    fs::write(cwd.join("main.oxel"), source).unwrap();
    let result = successful(
        Command::new(env!("CARGO_BIN_EXE_oxel"))
            .current_dir(cwd)
            .args(["run", "main.oxel", "--reference"])
            .arg(&fixture)
            .output()
            .unwrap(),
    );
    assert_eq!(
        result.lines().collect::<Vec<_>>(),
        [
            "10", "11", "1", "20", "25", "10", "21", "20", "21", "30", "8", "present", "empty"
        ]
    );
    let mut references = oxel::References::default();
    references.add(&fs::read(fixture).unwrap()).unwrap();
    oxel::check_with_references(
        "struct Linked { children: Interop.Slot<Linked[]> } fn main() {}",
        &references,
    )
    .unwrap();
    for (source, code) in [
        (
            "struct Infinite { next: Interop.Slot<Infinite> } fn main() {}",
            "E0304",
        ),
        (
            "fn main() { let value = new Interop.Counter(1); value.Field = 2; }",
            "E0204",
        ),
        (
            "fn main() { let value = new Interop.Counter(1); value.Current = 2; }",
            "E0204",
        ),
        (
            "fn main() { let value = new Interop.Counter(1); value[0] += 1; }",
            "E0204",
        ),
        (
            "fn main() { let values = new Interop.Counter[2]; }",
            "E0314",
        ),
        ("fn f(value: Interop.Restricted) {} fn main() {}", "E0202"),
        ("fn main() { Interop.Api.Hidden(); }", "E0203"),
        (
            "fn main() { let text: string = Interop.Api.NullableText().Item; }",
            "E0301",
        ),
        (
            "fn main() { new Interop.Cell<string>(\"invalid\"); }",
            "E0212",
        ),
    ] {
        let error = oxel::check_with_references(source, &references).unwrap_err();
        assert_eq!(error[0].code, code, "{source}: {error:?}");
    }
}

#[test]
fn nullable_values_cross_csharp_generic_signatures() {
    let folder = tempfile::tempdir().unwrap();
    let cwd = folder.path();
    let sdk = successful(Command::new("dotnet").arg("--version").output().unwrap());
    let bridge = build_reference_fixture(
        &cwd.join("bridge"),
        sdk.split('.').next().unwrap(),
        "Bridge",
        "1.0.0.0",
        r#"
        using System.Collections.Generic;
        namespace Optional;
        public static class Api {
            public static long? Read(bool yes) => yes ? 3000000000L : null;
            public static KeyValuePair<long?, string> Describe(long? value) => new(value, "result");
            public static string? ReadLabel(long? value, string? label) => value.HasValue ? label : null;
        }
        "#,
        None,
    );
    let source = r#"
        fn expose(pair: System.Collections.Generic.KeyValuePair<i64?, string>) -> i64? { pair.Key }
        fn main() {
            let result = Optional.Api.Describe(Optional.Api.Read(true));
            System.Console.WriteLine(result.Value);
            System.Console.WriteLine(expose(result) ?? -1);
            System.Console.WriteLine(Optional.Api.ReadLabel(null, null) ?? "empty");
        }
    "#;
    fs::write(cwd.join("main.oxel"), source).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_oxel"))
        .current_dir(cwd)
        .args(["run", "--framework", "main.oxel", "--reference"])
        .arg(bridge)
        .output()
        .unwrap();
    assert_eq!(
        successful(result).lines().collect::<Vec<_>>(),
        ["result", "3000000000", "empty"]
    );
}

#[test]
fn using_dispatches_explicit_dispose_and_unwinds_resources_once_in_reverse_order() {
    let folder = tempfile::tempdir().unwrap();
    let cwd = folder.path();
    let sdk = successful(Command::new("dotnet").arg("--version").output().unwrap());
    let library = build_reference_fixture(
        &cwd.join("fixture"),
        sdk.split('.').next().unwrap(),
        "Resources",
        "1.0.0.0",
        r#"
        using System;
        namespace Fixture;
        public static class Resources {
            public static string Log = "";
            public static Lease Open(string name) => new Lease(name);
            public static Lease Fail() => throw new InvalidOperationException();
            public static Token MakeToken() => new Token();
        }
        public class Lease : IDisposable {
            readonly string name;
            public Lease(string name) => this.name = name;
            public void Dispose() => Resources.Log += "wrong";
            void IDisposable.Dispose() => Resources.Log += name;
        }
        public struct Token : IDisposable {
            void IDisposable.Dispose() => Resources.Log += "s";
        }
        "#,
        None,
    );
    fs::write(
        cwd.join("main.oxel"),
        r#"
        fn consume<T: System.IDisposable>(value: T) {
            using resource = value { return; }
        }

        fn main() {
            using outer = Fixture.Resources.Open("a") {
                using inner = Fixture.Resources.Open("b") {}
            }
            consume(Fixture.Resources.MakeToken());
            try {
                using prior = Fixture.Resources.Open("c") {
                    using unavailable = Fixture.Resources.Fail() {}
                }
            } catch (_: System.InvalidOperationException) {}
            System.Console.WriteLine(Fixture.Resources.Log);
        }
        "#,
    )
    .unwrap();
    let result = successful(
        Command::new(env!("CARGO_BIN_EXE_oxel"))
            .current_dir(cwd)
            .args(["run", "--framework", "main.oxel", "--reference"])
            .arg(library)
            .output()
            .unwrap(),
    );
    assert_eq!(result.trim(), "basc");
}

#[test]
fn collection_loops_use_typed_interfaces_and_dispose_on_exit() {
    let folder = tempfile::tempdir().unwrap();
    let cwd = folder.path();
    let sdk = successful(Command::new("dotnet").arg("--version").output().unwrap());
    let library = build_reference_fixture(
        &cwd.join("fixture"),
        sdk.split('.').next().unwrap(),
        "Sequences",
        "1.0.0.0",
        r#"
        #nullable enable
        using System;
        using System.Collections;
        using System.Collections.Generic;
        namespace Fixture;
        public interface IAmbiguous : IEnumerable<int>, IEnumerable<string> {}
        public struct Sequence : IEnumerable<int> {
            public static string Log = "";
            public static Sequence Open() { Log += "s"; return new Sequence(); }
            IEnumerator<int> IEnumerable<int>.GetEnumerator() {
                Log += "g";
                return new Cursor();
            }
            IEnumerator IEnumerable.GetEnumerator() => throw new Exception("wrong interface");
            struct Cursor : IEnumerator<int> {
                int position;
                int IEnumerator<int>.Current => position * 10;
                object IEnumerator.Current => throw new Exception("wrong Current");
                bool IEnumerator.MoveNext() => ++position <= 3;
                void IEnumerator.Reset() => throw new NotSupportedException();
                void IDisposable.Dispose() => Log += "d";
            }
        }
        "#,
        None,
    );
    let check = |command| {
        Command::new(env!("CARGO_BIN_EXE_oxel"))
            .current_dir(cwd)
            .args([command, "--framework", "main.oxel", "--reference"])
            .arg(&library)
            .output()
            .unwrap()
    };
    fs::write(
        cwd.join("main.oxel"),
        r#"
        fn first<T, S: System.Collections.Generic.IEnumerable<T>>(source: S, fallback: T) -> T {
            for value in source { return value; }
            fallback
        }

        fn propagate(input: Option<i32>) -> Option<i32> {
            for value in Fixture.Sequence.Open() {
                return Option.Some(value + input?);
            }
            Option.None
        }

        fn main() {
            let mut total = 0;
            for value in Fixture.Sequence.Open() { total += value; }
            System.Console.WriteLine(total);
            for value in Fixture.Sequence.Open() {
                if value == 10 { continue; }
                System.Console.WriteLine(value);
                break;
            }
            System.Console.WriteLine(first::<i32, Fixture.Sequence>(Fixture.Sequence.Open(), -1));
            System.Console.WriteLine(propagate(Option.None) ?? -1);
            try {
                for value in Fixture.Sequence.Open() { throw new System.FormatException(); }
            } catch (_: System.FormatException) {}
            System.Console.WriteLine(Fixture.Sequence.Log);

            let text = new System.Collections.Generic.List<string?>();
            text.Add(null);
            text.Add("text");
            for item in text { System.Console.WriteLine(item ?? "empty"); }
        }
        "#,
    )
    .unwrap();
    assert_eq!(
        successful(check("run")),
        "60\n20\n10\n-1\nsgdsgdsgdsgdsgd\nempty\ntext\n"
    );

    for (parameter, expected) in [
        ("Fixture.IAmbiguous", "ambiguous CLR element type"),
        (
            "System.Collections.Generic.IEnumerable<i32>?",
            "non-nullable",
        ),
    ] {
        let source =
            format!("fn visit(source: {parameter}) {{ for value in source {{}} }} fn main() {{}}");
        fs::write(cwd.join("main.oxel"), source).unwrap();
        let result = check("check");
        assert!(!result.status.success());
        let error = String::from_utf8(result.stderr).unwrap();
        assert!(
            error.contains("E0315") && error.contains(expected),
            "{error}"
        );
    }
}
