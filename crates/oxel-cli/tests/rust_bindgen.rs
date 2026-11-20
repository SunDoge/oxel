use std::{fs, path::Path, process::Command};

const ABI: &str = include_str!("../../../examples/rust_bindgen/abi.rs");
const APP: &str = include_str!("../../../examples/rust_bindgen/main.oxel");

fn oxel(directory: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_oxel"));
    command.current_dir(directory);
    command
}

fn success(command: &mut Command) -> Vec<u8> {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{command:?}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

#[test]
fn generates_reproducible_oxel_bindings_without_rustc_or_dotnet() {
    let folder = tempfile::tempdir().unwrap();
    let root = folder.path();
    let rust = format!(
        r#"{ABI}
#[repr(C)]
struct Pair {{ p: Point, q: Point }}

#[unsafe(no_mangle)]
pub extern "C" fn read_pair(input: Pair) -> i32 {{ input.p.x }}

#[repr(C)]
struct Unused {{ address: *mut u8 }}

/// oxel: skip
#[unsafe(no_mangle)]
pub extern "C" fn raw_pointer() -> *mut u8 {{ std::ptr::null_mut() }}
"#,
    );
    fs::write(root.join("abi.rs"), rust).unwrap();
    fs::write(root.join("main.oxel"), APP).unwrap();
    let generate = || {
        let mut command = oxel(root);
        command
            .env("PATH", "")
            .args(["bindgen", "abi.rs", "--library", "oxel_rust_math"]);
        command
    };

    let bindings = String::from_utf8(success(&mut generate())).unwrap();
    assert!(bindings.contains("struct Point {\n    x: i32,\n    y: i32,"));
    assert!(bindings.contains("fn translate(point: Point, dx: i32, dy: i32) -> Point = \"oxel_rust_math\" as \"oxel_rust_translate\";"));
    assert!(bindings.contains("fn increment(ref value: i32, out previous: i32)"));
    assert!(bindings.contains("fn text_bytes(utf8 text: string?)"));
    assert!(bindings.contains("struct Pair {\n    p: Point,\n    q: Point,"));
    assert!(!bindings.contains("Unused"));
    assert!(!bindings.contains("raw_pointer"));
    success(generate().args(["-o", "bindings.oxel"]));
    assert_eq!(
        fs::read_to_string(root.join("bindings.oxel")).unwrap(),
        bindings
    );
    success(
        oxel(root)
            .env("PATH", "")
            .args(["check", "main.oxel", "bindings.oxel"]),
    );
}

#[test]
fn reports_unsupported_abi_and_preserves_the_previous_output() {
    let folder = tempfile::tempdir().unwrap();
    let root = folder.path();
    let source = root.join("abi.rs");
    let bindings = root.join("bindings.oxel");
    fs::write(&bindings, "existing output").unwrap();

    for (abi, message) in [
        (
            "#[unsafe(no_mangle)]\npub extern \"C\" fn run(ptr: *mut i32) {}",
            "pointer parameter needs an explicit",
        ),
        (
            "#[doc = \"oxel: out missing\"]\n#[unsafe(no_mangle)]\npub extern \"C\" fn run(n: i32) {}",
            "no ABI parameter named 'missing'",
        ),
        (
            "/// oxel: ref ptr\n#[unsafe(no_mangle)]\npub extern \"C\" fn run(ptr: *const i32) {}",
            "require a *mut parameter",
        ),
        (
            "#[repr(C, packed)]\npub struct Point { x: i32 }\n#[unsafe(no_mangle)]\npub extern \"C\" fn run(p: Point) {}",
            "default layout and alignment",
        ),
        (
            "#[cfg_attr(unix, unsafe(no_mangle))]\npub extern \"C\" fn run() {}",
            "conditional Rust ABI declarations",
        ),
        (
            "#[unsafe(no_mangle)]\npub extern \"C\" fn run() -> u128 { 0 }",
            "expected a fixed-width number",
        ),
        (
            "type i32 = u64;\n#[unsafe(no_mangle)]\npub extern \"C\" fn run() -> i32 { 0 }",
            "cannot shadow a fixed-width primitive",
        ),
        (
            "/// oxel: buffer input len\n#[unsafe(no_mangle)]\npub extern \"C\" fn run(input: *const u8, tag: u32, len: u32) {}",
            "immediately follow its pointer",
        ),
        (
            "/// oxel: buffer_mut input len\n#[unsafe(no_mangle)]\npub extern \"C\" fn run(input: *const u8, len: u32) {}",
            "buffer_mut requires *mut",
        ),
        (
            "/// oxel: buffer input len\n#[unsafe(no_mangle)]\npub extern \"C\" fn run(input: *const u8, len: i32) {}",
            "must be usize, u32 or u64",
        ),
        (
            "/// oxel: callback handler\n#[unsafe(no_mangle)]\npub extern \"C\" fn run(handler: fn(i32) -> i32) {}",
            "requires an extern \"C\" function pointer",
        ),
        (
            "/// oxel: address data\n#[unsafe(no_mangle)]\npub extern \"C\" fn run(data: *const [u8]) {}",
            "expected a thin address",
        ),
    ] {
        fs::write(&source, abi).unwrap();
        let result = oxel(root)
            .args([
                "bindgen",
                "abi.rs",
                "--library",
                "test",
                "-o",
                "bindings.oxel",
                "--diagnostic-format",
                "json",
            ])
            .output()
            .unwrap();
        assert!(!result.status.success(), "{abi}");
        assert!(result.stdout.is_empty());
        let error: serde_json::Value = serde_json::from_slice(&result.stderr).unwrap();
        assert_eq!(error["code"], "E0011");
        assert_eq!(error["file"], "abi.rs");
        assert!(error["range"]["end"].as_u64().unwrap() > 0, "{error}");
        assert!(
            error["message"].as_str().unwrap().contains(message),
            "{error}"
        );
        assert_eq!(fs::read_to_string(&bindings).unwrap(), "existing output");
    }

    fs::write(&source, ABI).unwrap();
    let result = oxel(root)
        .args(["bindgen", "abi.rs", "--library", "test", "-o", "./abi.rs"])
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert_eq!(fs::read_to_string(source).unwrap(), ABI);
}

#[test]
fn generates_buffer_widths_and_borrows_the_callback_pointer_layout() {
    let folder = tempfile::tempdir().unwrap();
    let root = folder.path();
    fs::write(
        root.join("abi.rs"),
        r#"
#[repr(C)]
pub struct Event { address: *mut core::ffi::c_void, length: usize }
type Notify = unsafe extern "C" fn(*const Event);
/// oxel: buffer commands length
/// oxel: callback notify
#[unsafe(no_mangle)]
pub extern "C" fn process(commands: *const Event, length: u64, notify: Option<Notify>) {}
"#,
    )
    .unwrap();
    let bindings = String::from_utf8(success(oxel(root).env("PATH", "").args([
        "bindgen",
        "abi.rs",
        "--library",
        "test",
    ])))
    .unwrap();
    assert!(bindings.contains("struct Event {\n    address: nint,\n    length: nuint,"));
    assert!(
        bindings
            .contains("process(buffer64 commands: Event[], callback notify: System.Action<nint>?)")
    );
}

#[cfg(target_os = "linux")]
#[test]
fn calls_rust_with_generated_batch_buffers_and_retained_callbacks() {
    let folder = tempfile::tempdir().unwrap();
    let root = folder.path();
    fs::create_dir(root.join("target")).unwrap();
    for (name, source) in [
        (
            "abi.rs",
            include_str!("../../../examples/rust_bindgen/batch/abi.rs"),
        ),
        (
            "main.oxel",
            include_str!("../../../examples/rust_bindgen/batch/main.oxel"),
        ),
        (
            "oxel.toml",
            include_str!("../../../examples/rust_bindgen/batch/oxel.toml"),
        ),
    ] {
        fs::write(root.join(name), source).unwrap();
    }
    success(Command::new("rustc").current_dir(root).args([
        "--crate-type",
        "cdylib",
        "--edition",
        "2024",
        "abi.rs",
        "-o",
        "target/liboxel_rust_batch.so",
    ]));
    success(oxel(root).args([
        "bindgen",
        "abi.rs",
        "--library",
        "oxel_rust_batch",
        "-o",
        "target/batch.ffi.oxel",
    ]));
    let output = success(oxel(root).env_remove("LD_LIBRARY_PATH").arg("run"));
    let output = String::from_utf8(output).unwrap();
    assert_eq!(
        output.lines().collect::<Vec<_>>(),
        ["30", "15", "0", "65", "-1", "42", "-1"]
    );
}

#[cfg(target_os = "linux")]
#[test]
fn calls_generated_bindings_in_a_rust_cdylib() {
    let folder = tempfile::tempdir().unwrap();
    let root = folder.path();
    fs::create_dir(root.join("target")).unwrap();
    fs::write(root.join("abi.rs"), ABI).unwrap();
    fs::write(root.join("main.oxel"), APP).unwrap();
    fs::write(
        root.join("oxel.toml"),
        include_str!("../../../examples/rust_bindgen/oxel.toml"),
    )
    .unwrap();
    success(Command::new("rustc").current_dir(root).args([
        "--crate-type",
        "cdylib",
        "--edition",
        "2024",
        "abi.rs",
        "-o",
        "target/liboxel_rust_math.so",
    ]));
    success(oxel(root).args([
        "bindgen",
        "abi.rs",
        "--library",
        "oxel_rust_math",
        "-o",
        "target/math.ffi.oxel",
    ]));
    let output = success(oxel(root).env_remove("LD_LIBRARY_PATH").arg("run"));
    let output = String::from_utf8(output).unwrap();
    assert_eq!(
        output.lines().collect::<Vec<_>>(),
        ["30", "29", "42", "6", "-1"]
    );
}
