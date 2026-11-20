use std::{
    io::BufReader,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{Receiver, channel},
    thread,
    time::Duration,
};

use lsp_server::{Message, Notification, Request, Response};
use serde_json::{Value, json};

#[derive(Debug, PartialEq, Eq)]
struct Highlight {
    line: u32,
    column: u32,
    length: u32,
    kind: String,
    modifiers: Vec<String>,
}

struct Client {
    child: Child,
    input: ChildStdin,
    output: Receiver<Message>,
    next_id: i32,
    semantic_legend: Value,
}

impl Client {
    fn start(hierarchical_symbols: bool, reference: Option<&std::path::Path>) -> Self {
        Self::start_project(hierarchical_symbols, reference, &[])
    }

    fn start_project(
        hierarchical_symbols: bool,
        reference: Option<&std::path::Path>,
        sources: &[&std::path::Path],
    ) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_oxel"));
        command.arg("lsp");
        if let Some(path) = reference {
            command.arg("--reference").arg(path);
        }
        for path in sources {
            command.arg("--source").arg(path);
        }
        Self::launch(command, hierarchical_symbols, false)
    }

    fn launch(command: Command, hierarchical_symbols: bool, versioned_edits: bool) -> Self {
        Self::launch_with_watched_files(command, hierarchical_symbols, versioned_edits, Value::Null)
    }

    fn launch_with_watched_files(
        mut command: Command,
        hierarchical_symbols: bool,
        versioned_edits: bool,
        watched_files: Value,
    ) -> Self {
        command
            .env("PATH", "")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped());
        let mut child = command.spawn().unwrap();
        let input = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (sender, output) = channel();
        thread::spawn(move || {
            let mut stdout = BufReader::new(stdout);
            while let Some(message) = Message::read(&mut stdout).unwrap() {
                if sender.send(message).is_err() {
                    break;
                }
            }
        });
        let mut client = Self {
            child,
            input,
            output,
            next_id: 0,
            semantic_legend: Value::Null,
        };
        let result = client.request(
            "initialize",
            json!({
                "processId": null,
                "capabilities": {
                    "workspace": {
                        "workspaceEdit": {"documentChanges": versioned_edits},
                        "didChangeWatchedFiles": watched_files,
                    },
                    "textDocument": {
                        "publishDiagnostics": {"relatedInformation": true},
                        "signatureHelp": {"signatureInformation": {"parameterInformation": {
                            "labelOffsetSupport": true
                        }}},
                        "documentSymbol": {"hierarchicalDocumentSymbolSupport": hierarchical_symbols},
                        "codeAction": {"codeActionLiteralSupport": {"codeActionKind": {
                            "valueSet": ["quickfix"]
                        }}},
                        "semanticTokens": {
                            "requests": {"full": true, "range": true},
                            "formats": ["relative"],
                            "tokenTypes": ["function", "method", "variable", "parameter", "type",
                                "typeParameter", "struct", "enum", "enumMember", "class", "interface",
                                "property", "operator"],
                            "tokenModifiers": ["declaration", "readonly", "defaultLibrary"]
                        }
                    }
                }
            }),
        );
        assert_eq!(result["serverInfo"]["name"], "oxel");
        assert_eq!(result["capabilities"]["positionEncoding"], "utf-16");
        assert_eq!(result["capabilities"]["textDocumentSync"]["change"], 2);
        assert_eq!(
            result["capabilities"]["signatureHelpProvider"]["triggerCharacters"],
            json!(["(", ","])
        );
        assert_eq!(
            result["capabilities"]["renameProvider"]["prepareProvider"],
            true
        );
        assert_eq!(
            result["capabilities"]["codeActionProvider"]["codeActionKinds"],
            json!(["quickfix"])
        );
        let tokens = &result["capabilities"]["semanticTokensProvider"];
        assert_eq!(tokens["full"], true);
        assert_eq!(tokens["range"], true);
        client.semantic_legend = tokens["legend"].clone();
        client.notify("initialized", json!({}));
        client
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let response = self.response(method, params);
        assert!(response.error.is_none(), "{:?}", response.error);
        response.result.unwrap_or(Value::Null)
    }

    fn response(&mut self, method: &str, params: Value) -> Response {
        self.next_id += 1;
        let id = self.next_id.into();
        let request = Request::new(id, method.into(), params);
        let id = request.id.clone();
        Message::from(request).write(&mut self.input).unwrap();
        match self.output.recv_timeout(Duration::from_secs(10)).unwrap() {
            Message::Response(response) => {
                assert_eq!(response.id, id);
                response
            }
            message => panic!("expected response to {method}, got {message:?}"),
        }
    }

    fn highlights(&mut self, uri: &str, range: Option<Value>) -> Vec<Highlight> {
        let mut params = json!({"textDocument": {"uri": uri}});
        let method = if let Some(range) = range {
            params["range"] = range;
            "textDocument/semanticTokens/range"
        } else {
            "textDocument/semanticTokens/full"
        };
        let result = self.request(method, params);
        let data: lsp_types::SemanticTokens = serde_json::from_value(result).unwrap();
        let mut line = 0;
        let mut column = 0;
        let mut previous_end = 0;
        data.data
            .into_iter()
            .map(|token| {
                if token.delta_line == 0 {
                    column += token.delta_start;
                    assert!(column >= previous_end, "overlapping semantic tokens");
                } else {
                    line += token.delta_line;
                    column = token.delta_start;
                }
                previous_end = column + token.length;
                Highlight {
                    line,
                    column,
                    length: token.length,
                    kind: self.semantic_legend["tokenTypes"][token.token_type as usize]
                        .as_str()
                        .unwrap()
                        .to_owned(),
                    modifiers: self.semantic_legend["tokenModifiers"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .enumerate()
                        .filter(|(index, _)| token.token_modifiers_bitset & (1 << index) != 0)
                        .map(|(_, modifier)| modifier.as_str().unwrap().to_owned())
                        .collect(),
                }
            })
            .collect()
    }

    fn signature_help(&mut self, uri: &str, text: &str, prefix: &str) -> Value {
        let offset = text.find(prefix).unwrap() + prefix.len();
        let leading = &text[..offset];
        let line = leading.bytes().filter(|&byte| byte == b'\n').count();
        let character = leading.rsplit('\n').next().unwrap().encode_utf16().count();
        self.request(
            "textDocument/signatureHelp",
            json!({
                "textDocument": {"uri": uri},
                "position": {"line": line, "character": character}
            }),
        )
    }

    fn notify(&mut self, method: &str, params: Value) {
        Message::from(Notification::new(method.into(), params))
            .write(&mut self.input)
            .unwrap();
    }

    fn diagnostics(&self) -> Value {
        match self.output.recv_timeout(Duration::from_secs(10)).unwrap() {
            Message::Notification(event) => {
                assert_eq!(event.method, "textDocument/publishDiagnostics");
                event.params
            }
            message => panic!("expected diagnostics, got {message:?}"),
        }
    }

    fn open(&mut self, uri: &str, text: &str) {
        self.notify(
            "textDocument/didOpen",
            json!({
                "textDocument": { "uri": uri, "text": text, "version": 1, "languageId": "oxel" }
            }),
        );
    }

    fn change(&mut self, uri: &str, version: i32, content_changes: Value) {
        self.notify(
            "textDocument/didChange",
            json!({
                "textDocument": { "uri": uri, "version": version },
                "contentChanges": content_changes,
            }),
        );
    }

    fn finish(&mut self) {
        assert_eq!(self.request("shutdown", Value::Null), Value::Null);
        self.notify("exit", Value::Null);
        assert!(self.child.wait().unwrap().success());
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn signature_help_follows_nested_calls_and_recovers_incomplete_cross_file_arguments() {
    let temp = tempfile::tempdir().unwrap();
    let main = temp.path().join("main.oxel");
    let helpers = temp.path().join("helpers.oxel");
    let text = "fn main() { show(\"🌍, )\", pick::<i32>([1, 2], 3), 0); }";
    std::fs::write(&main, text).unwrap();
    std::fs::write(
        &helpers,
        r#"
        fn pick<T>(values: T[], fallback: T) -> T { fallback }
        fn show(message: string, value: i32, mode: i32) {}
    "#,
    )
    .unwrap();
    let mut client = Client::start_project(true, None, &[&main, &helpers]);
    let uri = url::Url::from_file_path(main).unwrap();
    let uri = uri.as_str();
    client.open(uri, text);
    assert_eq!(client.diagnostics()["diagnostics"], json!([]));

    let inside = client.signature_help(uri, text, "pick::<i32>([1, 2], ");
    assert_eq!(inside["activeParameter"], 1);
    let signature = &inside["signatures"][0];
    assert_eq!(
        signature["label"],
        "fn pick<T>(values: T[], fallback: T) -> T"
    );
    assert_eq!(
        signature["parameters"],
        json!([{"label": [11, 22]}, {"label": [24, 35]}])
    );

    let outside = client.signature_help(uri, text, "2], 3), ");
    assert_eq!(
        outside["signatures"][0]["label"],
        "fn show(message: string, value: i32, mode: i32) -> void"
    );
    assert_eq!(outside["activeParameter"], 2);
    assert_eq!(client.signature_help(uri, text, "3), 0)"), Value::Null);

    let incomplete = "fn main() { show(\"🌍\", ";
    client.change(uri, 2, json!([{"text": incomplete}]));
    assert!(
        !client.diagnostics()["diagnostics"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let result = client.signature_help(uri, incomplete, incomplete);
    assert_eq!(
        result["signatures"][0]["label"],
        outside["signatures"][0]["label"]
    );
    assert_eq!(result["activeParameter"], 1);

    let incomplete = "fn repeat(first: i32, next: i32) { repeat(first, ";
    client.change(uri, 3, json!([{"text": incomplete}]));
    client.diagnostics();
    let result = client.signature_help(uri, incomplete, incomplete);
    assert_eq!(
        result["signatures"][0]["label"],
        "fn repeat(first: i32, next: i32) -> void"
    );
    assert_eq!(result["activeParameter"], 1);
    client.finish();
}

#[test]
fn signature_help_uses_resolved_and_incomplete_clr_static_calls() {
    let temp = tempfile::tempdir().unwrap();
    let reference = temp.path().join("Bridge.dll");
    let bytes = oxel::compile(
        r#"
        fn main() {}
        fn pair(first: i32, second: i32) -> i32 { first + second }
        extern "C" fn exchange(ref current: i32, out previous: i32) = "sample";
        "#,
        "Bridge",
    )
    .unwrap();
    std::fs::write(&reference, bytes).unwrap();
    let mut client = Client::start(true, Some(&reference));
    let uri = "file:///virtual/signature.oxel";
    let text = "fn main() { System.Console.WriteLine(Program.pair(10, 20)); }";
    client.open(uri, text);
    assert_eq!(client.diagnostics()["diagnostics"], json!([]));
    let result = client.signature_help(uri, text, "Program.pair(10, ");
    assert_eq!(result["activeParameter"], 1);
    assert_eq!(
        result["signatures"][0],
        json!({
            "label": "Program.pair(System.Int32, System.Int32) -> System.Int32",
            "parameters": [{"label": [13, 25]}, {"label": [27, 39]}]
        })
    );
    let text = "fn main() { Program.pair(10, }";
    client.change(uri, 2, json!([{"text": text}]));
    client.diagnostics();
    assert_eq!(
        client.signature_help(uri, text, "Program.pair(10, "),
        result
    );
    let text = "fn main() { System.Console.WriteLine(); }";
    client.change(uri, 3, json!([{"text": text}]));
    assert_eq!(client.diagnostics()["diagnostics"], json!([]));
    let result = client.signature_help(uri, text, "System.Console.WriteLine(");
    assert_eq!(result["activeParameter"], 0);
    assert_eq!(result["signatures"].as_array().unwrap().len(), 2);

    let text = "fn main() { let mut n = 1; let prev: i32; Program.exchange(&mut n, out prev); }";
    client.change(uri, 4, json!([{"text": text}]));
    assert_eq!(client.diagnostics()["diagnostics"], json!([]));
    let result = client.signature_help(uri, text, "Program.exchange(&mut n, ");
    assert_eq!(result["activeParameter"], 1);
    assert_eq!(
        result["signatures"][0],
        json!({
            "label": "Program.exchange(&mut System.Int32, out System.Int32) -> System.Void",
            "parameters": [{"label": [17, 34]}, {"label": [36, 52]}]
        })
    );
    let incomplete = "fn main() { let mut n = 1; Program.exchange(&mut n, ";
    client.change(uri, 5, json!([{"text": incomplete}]));
    client.diagnostics();
    assert_eq!(client.signature_help(uri, incomplete, incomplete), result);
    client.finish();
}

#[test]
fn stdio_server_tracks_utf16_edits_and_caches_document_symbols() {
    let mut client = Client::start(true, None);
    let uri = "file:///virtual/types.oxel";
    // A document can contain only declarations. Reuse the compiler type checker
    // to report the mismatch after the UTF-16 surrogate pair in this line.
    client.open(uri, "struct User { name: string }\r\nfn greet() { let _: string = \"🌍\"; let x: i32 = false; }");
    let diagnostics = client.diagnostics();
    assert_eq!(diagnostics["version"], 1);
    assert_eq!(diagnostics["diagnostics"][0]["code"], "E0301");
    let range = json!({"start": {"line": 1, "character": 48}, "end": {"line": 1, "character": 53}});
    assert_eq!(diagnostics["diagnostics"][0]["range"], range);

    // An invalid second edit cannot commit the valid first edit.
    client.change(uri, 2, json!([
        {"range": range, "text": "invalid"},
        {"range": { "start": {"line": 1, "character": 30}, "end": {"line": 1, "character": 31} }, "text": "x"}
    ]));
    let outline = client.request(
        "textDocument/documentSymbol",
        json!({"textDocument": {"uri": uri}}),
    );
    assert_eq!(outline[1]["name"], "greet");

    client.change(uri, 2, json!([{"range": range, "text": "30"}]));
    let diagnostics = client.diagnostics();
    assert_eq!(diagnostics["version"], 2);
    assert_eq!(diagnostics["diagnostics"], json!([]));

    client.change(uri, 1, json!([{"text": "broken stale document"}]));
    let symbols = client.request(
        "textDocument/documentSymbol",
        json!({"textDocument": {"uri": uri}}),
    );
    assert_eq!(symbols[0]["name"], "User");
    assert_eq!(symbols[0]["children"][0]["name"], "name");
    assert_eq!(
        symbols[0]["selectionRange"]["start"],
        json!({"line": 0, "character": 7})
    );
    assert_eq!(symbols[1]["name"], "greet");

    client.change(
        uri,
        3,
        json!([{"text": "struct User { name: string }\nfn incomplete("}]),
    );
    assert_eq!(client.diagnostics()["diagnostics"][0]["code"], "E0102");
    let symbols = client.request(
        "textDocument/documentSymbol",
        json!({"textDocument": {"uri": uri}}),
    );
    assert_eq!(symbols.as_array().unwrap().len(), 1);
    assert_eq!(symbols[0]["name"], "User");

    let completion = client.request(
        "textDocument/completion",
        json!({
            "textDocument": {"uri": uri}, "position": {"line": 1, "character": 14}
        }),
    );
    let items = completion.as_array().unwrap();
    assert!(items.iter().any(|item| item["label"] == "User"));
    assert!(items.iter().any(|item| item["label"] == "while"));

    client.notify(
        "textDocument/didClose",
        json!({"textDocument": {"uri": uri}}),
    );
    assert_eq!(client.diagnostics()["diagnostics"], json!([]));
    assert_eq!(
        client.request(
            "textDocument/documentSymbol",
            json!({"textDocument": {"uri": uri}})
        ),
        Value::Null
    );
    client.finish();
}

#[test]
fn stdio_server_resolves_configured_clr_references() {
    let folder = tempfile::tempdir().unwrap();
    let reference = folder.path().join("Values.dll");
    fs_write_reference(&reference);
    let mut client = Client::start(false, Some(&reference));
    let uri = "file:///virtual/call.oxel";
    let source = "struct Item { value: i32 }\nfn use_value() -> i32 { Program.value() }";
    client.open(uri, source);
    assert_eq!(client.diagnostics()["diagnostics"], json!([]));
    let symbols = client.request(
        "textDocument/documentSymbol",
        json!({"textDocument": {"uri": uri}}),
    );
    assert_eq!(symbols[1]["name"], "value");
    assert_eq!(symbols[1]["containerName"], "Item");
    assert_eq!(symbols[1]["location"]["uri"], uri);
    let hover = client.request(
        "textDocument/hover",
        json!({
            "textDocument": {"uri": uri}, "position": {"line": 1, "character": 32}
        }),
    );
    assert_eq!(
        hover["contents"]["value"],
        "Program.value() -> System.Int32"
    );
    let call = client
        .highlights(uri, None)
        .into_iter()
        .find(|token| token.kind == "method")
        .unwrap();
    let line = source.lines().nth(1).unwrap();
    assert_eq!(
        (call.line, call.column, call.length),
        (1, line.find("value() }").unwrap() as u32, 5)
    );
    client.finish();
}

fn fs_write_reference(path: &std::path::Path) {
    let bytes = oxel::compile("fn main() {} fn value() -> i32 { 30 }", "Values").unwrap();
    std::fs::write(path, bytes).unwrap();
}

#[test]
fn stdio_hover_and_definition_use_compiler_bindings() {
    let mut client = Client::start(true, None);
    let uri = "file:///virtual/navigation.oxel";
    let text = "struct User { age: i32 }\nfn count(user: User) -> i32 { user.age }\nfn test() { let x = 1; { let x = true; x; } x; count(User { age: 2 }); }";
    client.open(uri, text);
    assert_eq!(client.diagnostics()["diagnostics"], json!([]));

    let point = |line, character| {
        json!({
            "textDocument": {"uri": uri}, "position": {"line": line, "character": character}
        })
    };
    let request = client.request("textDocument/hover", point(1, 36));
    assert_eq!(request["contents"]["value"], "age: System.Int32");
    let request = client.request("textDocument/definition", point(1, 36));
    assert_eq!(
        request["range"]["start"],
        json!({"line": 0, "character": 14})
    );
    let request = client.request("textDocument/definition", point(1, 16));
    assert_eq!(
        request["range"]["start"],
        json!({"line": 0, "character": 7})
    );
    let request = client.request("textDocument/hover", point(2, 39));
    assert_eq!(request["contents"]["value"], "x: System.Boolean");
    let request = client.request("textDocument/definition", point(2, 39));
    assert_eq!(
        request["range"]["start"],
        json!({"line": 2, "character": 29})
    );
    let request = client.request("textDocument/hover", point(2, 44));
    assert_eq!(request["contents"]["value"], "x: System.Int32");
    let request = client.request("textDocument/definition", point(2, 49));
    assert_eq!(
        request["range"]["start"],
        json!({"line": 1, "character": 3})
    );
    client.finish();
}

#[test]
fn project_server_updates_dependents_and_navigates_to_closed_files() {
    let folder = tempfile::tempdir().unwrap();
    let main = folder.path().join("main.oxel");
    let helper = folder.path().join("other source.oxel");
    let main_source = "fn main() { let value: i32 = count(); }";
    let helper_source = "fn count() -> i32 { 30 }";
    std::fs::write(&main, main_source).unwrap();
    std::fs::write(&helper, helper_source).unwrap();
    let main_uri = url::Url::from_file_path(&main).unwrap();
    let helper_uri = url::Url::from_file_path(&helper).unwrap();
    let mut client = Client::start_project(true, None, &[&main, &helper]);
    client.open(main_uri.as_str(), main_source);
    assert_eq!(client.diagnostics()["diagnostics"], json!([]));

    let definition = client.request(
        "textDocument/definition",
        json!({
            "textDocument": {"uri": main_uri.as_str()}, "position": {"line": 0, "character": 30}
        }),
    );
    assert_eq!(definition["uri"], helper_uri.as_str());
    assert_eq!(
        definition["range"]["start"],
        json!({"line": 0, "character": 3})
    );

    let completion = client.request(
        "textDocument/completion",
        json!({
            "textDocument": {"uri": main_uri.as_str()}, "position": {"line": 0, "character": 30}
        }),
    );
    assert!(
        completion
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["label"] == "count")
    );

    let references = client.request(
        "textDocument/references",
        json!({
            "textDocument": {"uri": main_uri.as_str()},
            "position": {"line": 0, "character": 30},
            "context": {"includeDeclaration": true}
        }),
    );
    let uris: Vec<_> = references
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value["uri"].as_str().unwrap())
        .collect();
    assert_eq!(uris, [main_uri.as_str(), helper_uri.as_str()]);

    let opened_uri = helper_uri
        .as_str()
        .replace("other%20source", "%6Fther%20source");
    client.open(&opened_uri, "fn count() -> bool { true }");
    let dependent = client.diagnostics();
    assert_eq!(dependent["uri"], main_uri.as_str());
    assert_eq!(dependent["version"], 1);
    assert_eq!(dependent["diagnostics"][0]["code"], "E0301");
    assert_eq!(
        dependent["diagnostics"][0]["data"],
        json!({"expected": "System.Int32", "actual": "System.Boolean"})
    );
    assert_eq!(client.diagnostics()["diagnostics"], json!([]));

    client.notify(
        "textDocument/didClose",
        json!({"textDocument": {"uri": opened_uri}}),
    );
    assert_eq!(client.diagnostics()["uri"], opened_uri);
    let dependent = client.diagnostics();
    assert_eq!(dependent["uri"], main_uri.as_str());
    assert_eq!(dependent["diagnostics"], json!([]));
    client.finish();
}

#[test]
fn argument_diagnostics_link_to_the_parameter_declaration_file() {
    let folder = tempfile::tempdir().unwrap();
    let main = folder.path().join("main.oxel");
    let api = folder.path().join("api.oxel");
    let caller = "fn main() { accept(false); }";
    let callee = "fn accept(value: i32) {}";
    std::fs::write(&main, caller).unwrap();
    std::fs::write(&api, callee).unwrap();
    let main_uri = url::Url::from_file_path(&main).unwrap();
    let api_uri = url::Url::from_file_path(&api).unwrap();
    let mut client = Client::start_project(true, None, &[&main, &api]);
    let expected = |line| {
        json!([{
            "location": {"uri": api_uri.as_str(), "range": {
                "start": {"line": line, "character": 3}, "end": {"line": line, "character": 9}
            }},
            "message": "parameter 'value' is declared by 'accept'"
        }])
    };
    client.open(main_uri.as_str(), caller);
    let diagnostic = client.diagnostics()["diagnostics"][0].clone();
    assert_eq!(diagnostic["code"], "E0301");
    assert_eq!(diagnostic["relatedInformation"], expected(0));

    client.open(api_uri.as_str(), &format!("// moved 🌍\n{callee}"));
    let refreshed = client.diagnostics();
    assert_eq!(refreshed["uri"], main_uri.as_str());
    assert_eq!(
        refreshed["diagnostics"][0]["relatedInformation"],
        expected(1)
    );
    assert_eq!(client.diagnostics()["diagnostics"], json!([]));
    client.finish();
}

#[test]
fn watched_closed_files_refresh_typed_dependents_and_preserve_open_buffers() {
    let folder = tempfile::tempdir().unwrap();
    let main = folder.path().join("main.oxel");
    let types = folder.path().join("shape definitions.oxel");
    let main_source = "fn read(holder: Holder) -> i32 { holder.cell.value }\nfn main() {}";
    let type_source = "struct Cell { value: i32 }\nstruct Holder { cell: Cell }";
    std::fs::write(&main, main_source).unwrap();
    std::fs::write(&types, type_source).unwrap();
    let main_uri = url::Url::from_file_path(&main).unwrap();
    let types_uri = url::Url::from_file_path(&types).unwrap();
    let base_uri = url::Url::from_directory_path(folder.path()).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_oxel"));
    command
        .arg("lsp")
        .arg("--source")
        .arg(&main)
        .arg("--source")
        .arg(&types);
    let mut client = Client::launch_with_watched_files(
        command,
        true,
        false,
        json!({"dynamicRegistration": true, "relativePatternSupport": true}),
    );
    let registration = client.output.recv_timeout(Duration::from_secs(10)).unwrap();
    let Message::Request(registration) = registration else {
        panic!("expected a watch registration request, got {registration:?}");
    };
    assert_eq!(registration.method, "client/registerCapability");
    let options = &registration.params["registrations"][0];
    assert_eq!(options["method"], "workspace/didChangeWatchedFiles");
    let watchers = options["registerOptions"]["watchers"].as_array().unwrap();
    for name in ["main.oxel", "shape definitions.oxel"] {
        assert!(watchers.contains(&json!({
            "globPattern": {"baseUri": base_uri.as_str(), "pattern": name}
        })));
    }
    std::fs::write(&types, format!("// before watching\n{type_source}")).unwrap();
    Message::Response(Response::new_ok(registration.id, Value::Null))
        .write(&mut client.input)
        .unwrap();

    client.open(main_uri.as_str(), main_source);
    assert_eq!(client.diagnostics()["diagnostics"], json!([]));
    let position = json!({
        "textDocument": {"uri": main_uri.as_str()},
        "position": {"line": 0, "character": main_source.find("value").unwrap()}
    });
    let definition = client.request("textDocument/definition", position.clone());
    assert_eq!(
        definition["range"]["start"],
        json!({"line": 1, "character": 14})
    );
    let changed_source = format!("// updated on disk\n{}", type_source.replace("i32", "bool"));
    std::fs::write(&types, changed_source).unwrap();
    let watched_uri = types_uri.as_str().replace("shape%20", "%73hape%20");
    client.notify(
        "workspace/didChangeWatchedFiles",
        json!({"changes": [{"uri": watched_uri, "type": 2}, {"uri": types_uri.as_str(), "type": 2}]}),
    );
    let changed = client.diagnostics();
    assert_eq!(changed["uri"], main_uri.as_str());
    assert_eq!(changed["version"], 1);
    assert_eq!(changed["diagnostics"][0]["code"], "E0301");
    assert_eq!(
        changed["diagnostics"][0]["data"]["actual"],
        "System.Boolean"
    );
    let definition = client.request("textDocument/definition", position.clone());
    assert_eq!(definition["uri"], types_uri.as_str());
    assert_eq!(
        definition["range"]["start"],
        json!({"line": 1, "character": 14})
    );

    client.open(types_uri.as_str(), type_source);
    assert_eq!(client.diagnostics()["diagnostics"], json!([]));
    assert_eq!(client.diagnostics()["diagnostics"], json!([]));
    client.notify(
        "workspace/didChangeWatchedFiles",
        json!({"changes": [{"uri": types_uri.as_str(), "type": 2}]}),
    );
    let hover = client.request("textDocument/hover", position.clone());
    assert_eq!(hover["contents"]["value"], "value: System.Int32");
    client.notify(
        "textDocument/didClose",
        json!({"textDocument": {"uri": types_uri.as_str()}}),
    );
    assert_eq!(client.diagnostics()["uri"], types_uri.as_str());
    assert_eq!(client.diagnostics()["diagnostics"][0]["code"], "E0301");

    std::fs::remove_file(&types).unwrap();
    client.notify(
        "workspace/didChangeWatchedFiles",
        json!({"changes": [{"uri": types_uri.as_str(), "type": 3}]}),
    );
    assert_eq!(client.diagnostics()["diagnostics"][0]["code"], "E0202");
    std::fs::write(&types, type_source).unwrap();
    client.notify(
        "workspace/didChangeWatchedFiles",
        json!({"changes": [{"uri": types_uri.as_str(), "type": 1}]}),
    );
    assert_eq!(client.diagnostics()["diagnostics"], json!([]));
    let definition = client.request("textDocument/definition", position);
    assert_eq!(
        definition["range"]["start"],
        json!({"line": 0, "character": 14})
    );
    client.finish();
}

#[test]
fn guard_uses_the_same_pattern_binding_as_its_match_arm() {
    let mut client = Client::start(true, None);
    let uri = "file:///virtual/match-guard.oxel";
    let source = "enum Input { A(i32), B(i32), Empty } fn f(input: Input) -> i32 { match input { A(value) | B(value) if value > 0 => value, _ => 0 } }";
    client.open(uri, source);
    assert_eq!(client.diagnostics()["diagnostics"], json!([]));
    let points = source
        .match_indices("value")
        .map(|(offset, _)| offset)
        .collect::<Vec<_>>();
    let point = |offset| {
        json!({
            "textDocument": {"uri": uri}, "position": {"line": 0, "character": offset}
        })
    };
    let options = client.request("textDocument/completion", point(points[2]));
    assert!(
        options
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["label"] == "value")
    );
    for &offset in &points[1..] {
        let result = client.request("textDocument/definition", point(offset));
        assert_eq!(
            result["range"]["start"],
            json!({"line": 0, "character": points[0]})
        );
    }
    let mut params = point(points[1]);
    params["newName"] = json!("number");
    let renamed = client.request("textDocument/rename", params);
    let edits = renamed["changes"][uri].as_array().unwrap();
    let columns = edits
        .iter()
        .map(|edit| edit["range"]["start"]["character"].as_u64().unwrap() as usize)
        .collect::<Vec<_>>();
    assert_eq!(columns, points);
    assert!(edits.iter().all(|edit| edit["newText"] == "number"));
    client.finish();
}

#[test]
fn let_else_bindings_open_after_the_fallback_block() {
    let mut client = Client::start(true, None);
    let uri = "file:///virtual/let-else.oxel";
    let source = [
        "fn inspect(input: Option<i32>) {",
        "    let Some(number) = input else {",
        "        return;",
        "    };",
        "    System.Console.WriteLine(number);",
        "}",
    ];
    client.open(uri, &source.join("\n"));
    assert_eq!(client.diagnostics()["diagnostics"], json!([]));
    let point = |line: usize, character| {
        json!({
            "textDocument": {"uri": uri}, "position": {"line": line, "character": character}
        })
    };
    let has_number = |response: Value| {
        response
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["label"] == "number")
    };
    assert!(!has_number(
        client.request("textDocument/completion", point(2, 8))
    ));
    let use_site = point(4, source[4].find("number").unwrap());
    assert!(has_number(
        client.request("textDocument/completion", use_site.clone())
    ));
    let definition = client.request("textDocument/definition", use_site);
    assert_eq!(
        definition["range"]["start"],
        json!({"line": 1, "character": source[1].find("number").unwrap()})
    );
    client.finish();
}

#[test]
fn completion_uses_visible_bindings_and_rebases_cached_scopes() {
    let mut client = Client::start(true, None);
    let uri = "file:///completion.oxel";
    let source = r#"enum Item { Some(i32), None }
const value = 10;
fn sample(input: Item) {
    let value = 20;
    {
        let value = value > 0;
        let marker = "😀"; value;
    }
    for index in 0..1 {
        index;
    }
    match input {
        Item.Some(payload) => payload,
        Item.None => value,
    };
    missing;
}
"#;
    client.notify(
        "textDocument/didOpen",
        json!({"textDocument": {"uri": uri, "languageId": "oxel", "version": 1, "text": source}}),
    );
    let diagnostics = client.diagnostics();
    assert_eq!(diagnostics["diagnostics"].as_array().unwrap().len(), 1);
    assert_eq!(diagnostics["diagnostics"][0]["range"]["start"]["line"], 15);

    let mut complete = |line, character| {
        let result = client.request(
            "textDocument/completion",
            json!({
                "textDocument": {"uri": uri},
                "position": {"line": line, "character": character},
            }),
        );
        result
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|item| {
                Some((
                    item["label"].as_str()?.to_owned(),
                    item["detail"].as_str()?.to_owned(),
                ))
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        complete(5, 20),
        [
            ("value".into(), "value: System.Int32".into()),
            ("input".into(), "input: Item".into()),
        ]
    );
    let inner_column = "        let marker = \"😀\"; ".encode_utf16().count();
    let inner = complete(6, inner_column);
    assert_eq!(
        inner,
        [
            ("marker".into(), "marker: System.String".into()),
            ("value".into(), "value: System.Boolean".into()),
            ("input".into(), "input: Item".into()),
        ]
    );
    assert!(complete(8, 17).iter().all(|(name, _)| name != "index"));
    assert_eq!(complete(9, 8)[0].0, "index");
    assert_eq!(complete(12, 29)[0].0, "payload");
    assert_eq!(complete(13, 21)[0].0, "value");
    assert_eq!(complete(14, 4)[0].1, "value: System.Int32");

    client.change(
        uri,
        2,
        json!([{"text": format!("// move declarations\n{source}")}]),
    );
    assert_eq!(
        client.diagnostics()["diagnostics"][0]["range"]["start"]["line"],
        16
    );
    let moved = client.request(
        "textDocument/completion",
        json!({"textDocument": {"uri": uri}, "position": {"line": 7, "character": inner_column}}),
    );
    assert_eq!(moved[0]["label"], "marker");
    assert_eq!(moved[1]["detail"], "value: System.Boolean");
    client.finish();
}

#[test]
fn references_distinguish_shadowed_bindings_and_respect_include_declaration() {
    let mut client = Client::start(true, None);
    let uri = "file:///virtual/references.oxel";
    let text = "fn main() { let x = 1; { let x = true; x; } x; }";
    client.open(uri, text);
    assert_eq!(client.diagnostics()["diagnostics"], json!([]));

    let points: Vec<_> = text.match_indices('x').map(|(offset, _)| offset).collect();
    let params = |point, include| {
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": 0, "character": point},
            "context": {"includeDeclaration": include}
        })
    };
    let outer = client.request("textDocument/references", params(points[0], true));
    let columns: Vec<_> = outer
        .as_array()
        .unwrap()
        .iter()
        .map(|position| position["range"]["start"]["character"].as_u64().unwrap() as usize)
        .collect();
    assert_eq!(columns, [points[0], points[3]]);
    let inner = client.request("textDocument/references", params(points[2], false));
    assert_eq!(inner.as_array().unwrap().len(), 1);
    assert_eq!(inner[0]["range"]["start"]["character"], points[2]);
    client.finish();
}

#[test]
fn references_to_structs_include_field_types_and_construction() {
    let mut client = Client::start(true, None);
    let uri = "file:///virtual/record-types.oxel";
    let text = "struct Node {}\nstruct Holder { item: Node }\nfn make() -> Node { Node {} }";
    client.open(uri, text);
    assert_eq!(client.diagnostics()["diagnostics"], json!([]));
    let references = client.request(
        "textDocument/references",
        json!({
            "textDocument": {"uri": uri}, "position": {"line": 0, "character": 8},
            "context": {"includeDeclaration": false}
        }),
    );
    let references: Vec<_> = references
        .as_array()
        .unwrap()
        .iter()
        .map(|location| location["range"]["start"].clone())
        .collect();
    assert_eq!(
        references,
        [
            json!({"line": 1, "character": 22}),
            json!({"line": 2, "character": 13}),
            json!({"line": 2, "character": 20}),
        ],
    );
    client.finish();
}

#[test]
fn enum_variants_and_payload_bindings_are_navigable() {
    let mut client = Client::start(true, None);
    let uri = "file:///virtual/enum.oxel";
    let uri_param = json!({ "textDocument": { "uri": uri } });
    let source = "enum Choice { Empty, Count(i32) } enum Envelope { Wrap(Choice) }\nfn read(value: Envelope) -> i32 {\n    match value { Wrap(Choice.Count(n)) => n, Wrap(Choice.Empty) => 0 }\n}";
    client.open(uri, source);
    assert_eq!(client.diagnostics()["diagnostics"], json!([]));
    let symbols = client.request("textDocument/documentSymbol", uri_param);
    assert_eq!(symbols[0]["name"], "Choice");
    assert_eq!(symbols[0]["kind"], 10); // Enum
    assert_eq!(symbols[0]["children"][1]["name"], "Count");
    assert_eq!(symbols[0]["children"][1]["kind"], 22); // EnumMember

    let line = source.lines().nth(2).unwrap();
    let point = |character| json!({ "textDocument": { "uri": uri }, "position": { "line": 2, "character": character } });
    let variant = client.request(
        "textDocument/definition",
        point(line.find("Count").unwrap()),
    );
    assert_eq!(
        variant["range"]["start"],
        json!({ "line": 0, "character": source.find("Count").unwrap() })
    );
    let use_offset = line.find("=> n").unwrap() + 3;
    let hover = client.request("textDocument/hover", point(use_offset));
    assert_eq!(hover["contents"]["value"], "n: System.Int32");
    let local = client.request("textDocument/definition", point(use_offset));
    assert_eq!(
        local["range"]["start"],
        json!({ "line": 2, "character": line.find("(n)").unwrap() + 1 })
    );
    client.finish();
}

#[test]
fn propagation_operator_hover_reports_the_success_type() {
    let mut client = Client::start(true, None);
    let uri = "file:///virtual/result.oxel";
    let line =
        "fn next(value: Result<i32, string>) -> Result<i32, string> { Result.Ok(value? + 1) }";
    client.open(
        uri,
        &format!("enum Result<T, E> {{ Ok(T), Err(E) }}\n{line}"),
    );
    assert_eq!(client.diagnostics()["diagnostics"], json!([]));
    let hover = client.request(
        "textDocument/hover",
        json!({
            "textDocument": { "uri": uri },
            "position": { "line": 1, "character": line.find('?').unwrap() }
        }),
    );
    assert_eq!(
        hover["contents"]["value"],
        "Propagate Err or yield System.Int32"
    );
    client.finish();
}

#[test]
fn manifest_loads_sources_and_clr_references_for_lsp() {
    let folder = tempfile::tempdir().unwrap();
    let project = folder.path().join("project");
    std::fs::create_dir(&project).unwrap();
    let main = project.join("main.oxel");
    let helper = project.join("helper.oxel");
    let source = "fn main() { System.Console.WriteLine(value()); }";
    std::fs::write(&main, source).unwrap();
    std::fs::write(&helper, "fn value() -> i32 { Program.answer() }").unwrap();
    let bytes = oxel::compile("fn main() {} fn answer() -> i32 { 30 }", "External").unwrap();
    std::fs::write(project.join("deps.dll"), bytes).unwrap();
    std::fs::write(
        project.join("oxel.toml"),
        "[package]\nname = 'demo'\nsources = ['main.oxel', 'helper.oxel']\nreferences = ['deps.dll']",
    ).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_oxel"));
    command
        .current_dir(folder.path())
        .args(["lsp", "--manifest-path", "project/oxel.toml"]);
    let mut client = Client::launch(command, true, false);
    let main_uri = url::Url::from_file_path(&main).unwrap();
    let helper_uri = url::Url::from_file_path(&helper).unwrap();
    client.open(main_uri.as_str(), source);
    assert_eq!(client.diagnostics()["diagnostics"], json!([]));
    let definition = client.request(
        "textDocument/definition",
        json!({
            "textDocument": {"uri": main_uri.as_str()},
            "position": {"line": 0, "character": source.find("value").unwrap()}
        }),
    );
    assert_eq!(definition["uri"], helper_uri.as_str());
    let helper_source = std::fs::read_to_string(&helper).unwrap();
    client.open(helper_uri.as_str(), &helper_source);
    // Opening the second document republishes diagnostics for both open inputs.
    for _ in 0..2 {
        assert_eq!(client.diagnostics()["diagnostics"], json!([]));
    }
    client.finish();
}

#[test]
fn lsp_test_mode_resolves_manifest_test_sources_against_the_library() {
    let folder = tempfile::tempdir().unwrap();
    let project = folder.path();
    let library = project.join("math.oxel");
    let tests = project.join("math_tests.oxel");
    let text = "#[test] fn checks_sum() { assert(add(10, 20) == 30); }";
    std::fs::write(&library, "internal fn add(a: i32, b: i32) -> i32 { a + b }").unwrap();
    std::fs::write(&tests, text).unwrap();
    std::fs::write(
        project.join("oxel.toml"),
        "[package]\nname = 'Samples.Math'\nkind = 'library'\nsources = ['math.oxel']\ntest_sources = ['math_tests.oxel']",
    )
    .unwrap();
    let tests_uri = url::Url::from_file_path(&tests).unwrap();
    let library_uri = url::Url::from_file_path(&library).unwrap();

    for include_tests in [false, true] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_oxel"));
        command
            .current_dir(project)
            .args(["lsp", "--manifest-path", "oxel.toml"]);
        if include_tests {
            command.arg("--tests");
        }
        let mut client = Client::launch(command, true, false);
        client.open(tests_uri.as_str(), text);
        let diagnostics = client.diagnostics();
        if include_tests {
            assert_eq!(diagnostics["diagnostics"], json!([]));
            let definition = client.request(
                "textDocument/definition",
                json!({
                    "textDocument": {"uri": tests_uri.as_str()},
                    "position": {"line": 0, "character": text.find("add").unwrap()},
                }),
            );
            assert_eq!(definition["uri"], library_uri.as_str());
            client.change(
                tests_uri.as_str(),
                2,
                json!([{"text": text.replace("20", "true")}]),
            );
            assert_eq!(client.diagnostics()["diagnostics"][0]["code"], "E0301");
        } else {
            assert_eq!(diagnostics["diagnostics"][0]["code"], "E0203");
        }
        client.finish();
    }
}

#[test]
fn rename_preserves_shadowing_and_rejects_changed_bindings() {
    let mut client = Client::start(true, None);
    let uri = "file:///virtual/rename.oxel";
    let text = r#"fn main() { "🌍"; let x = 1; { let y = 2; x; y; } { let x = 3; x; } x; System.Console.WriteLine(x); }"#;
    client.open(uri, text);
    assert_eq!(client.diagnostics()["diagnostics"], json!([]));

    let column = |byte| text[..byte].encode_utf16().count();
    let xs: Vec<_> = text
        .match_indices('x')
        .map(|(byte, _)| column(byte))
        .collect();
    let point = |character| {
        json!({
            "textDocument": {"uri": uri}, "position": {"line": 0, "character": character}
        })
    };
    let rename = |character, new_name| {
        let mut params = point(character);
        params["newName"] = json!(new_name);
        params
    };
    let prepared = client.request("textDocument/prepareRename", point(xs[1]));
    assert_eq!(prepared["placeholder"], "x");
    assert_eq!(prepared["range"]["start"]["character"], xs[1]);
    let edited = client.request("textDocument/rename", rename(xs[1], "longer_name"));
    let edits = edited["changes"][uri].as_array().unwrap();
    let columns: Vec<_> = edits
        .iter()
        .map(|edit| edit["range"]["start"]["character"].clone())
        .collect();
    assert_eq!(
        columns,
        [xs[0], xs[1], xs[4], xs[5]].map(|column| json!(column))
    );
    assert!(edits.iter().all(|edit| edit["newText"] == "longer_name"));

    let capture = client
        .response("textDocument/rename", rename(xs[0], "y"))
        .error
        .unwrap();
    assert_eq!(capture.code, lsp_server::ErrorCode::RequestFailed as i32);
    assert!(capture.message.contains("resolves to"));
    let keyword = client
        .response("textDocument/rename", rename(xs[0], "for"))
        .error
        .unwrap();
    assert_eq!(keyword.code, lsp_server::ErrorCode::InvalidParams as i32);
    assert_eq!(
        client.request("textDocument/prepareRename", point(3)),
        Value::Null
    );
    assert_eq!(
        client.request(
            "textDocument/prepareRename",
            point(column(text.find("WriteLine").unwrap()))
        ),
        Value::Null
    );
    assert_eq!(
        client.request("textDocument/rename", rename(xs[1], "x")),
        json!({"changes": {}})
    );

    client.change(
        uri,
        2,
        json!([{"text": "fn spare() {} fn broken() { missing; }"}]),
    );
    assert_ne!(client.diagnostics()["diagnostics"], json!([]));
    assert!(
        client
            .response("textDocument/rename", rename(3, "replacement"))
            .error
            .is_some()
    );
    client.finish();
}

#[test]
fn project_rename_uses_unsaved_buffers_and_versions_across_closed_files() {
    let folder = tempfile::tempdir().unwrap();
    let main = folder.path().join("main.oxel");
    let helper = folder.path().join("other source.oxel");
    let main_source = "fn main() { read_total(); }";
    let helper_source = "fn read_total() -> i32 { 30 }";
    std::fs::write(&main, main_source).unwrap();
    std::fs::write(&helper, helper_source).unwrap();
    let main_uri = url::Url::from_file_path(&main).unwrap();
    let helper_uri = url::Url::from_file_path(&helper).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_oxel"));
    command
        .arg("lsp")
        .arg("--source")
        .arg(&main)
        .arg("--source")
        .arg(&helper);
    let mut client = Client::launch(command, true, true);
    let unsaved = "fn main() { read_total(); read_total(); }";
    client.open(main_uri.as_str(), unsaved);
    assert_eq!(client.diagnostics()["diagnostics"], json!([]));
    let params = json!({
        "textDocument": {"uri":main_uri.as_str()},
        "position":{"line":0,"character":unsaved.rfind("read_total").unwrap()},
        "newName":"sum"
    });
    let result = client.request("textDocument/rename", params.clone());
    let documents = result["documentChanges"].as_array().unwrap();
    assert_eq!(documents.len(), 2);
    assert_eq!(
        documents[0]["textDocument"],
        json!({"uri":main_uri.as_str(),"version":1})
    );
    assert_eq!(
        documents[1]["textDocument"],
        json!({"uri":helper_uri.as_str(),"version":null})
    );
    let apply = |original: &str, document: &Value| {
        let mut text = original.to_owned();
        let edits = document["edits"].as_array().unwrap();
        for edit in edits.iter().rev() {
            let start = edit["range"]["start"]["character"].as_u64().unwrap() as usize;
            let end = edit["range"]["end"]["character"].as_u64().unwrap() as usize;
            text.replace_range(start..end, edit["newText"].as_str().unwrap());
        }
        text
    };
    let new_main = apply(unsaved, &documents[0]);
    let new_helper = apply(helper_source, &documents[1]);
    assert_eq!(new_main, "fn main() { sum(); sum(); }");
    assert_eq!(new_helper, "fn sum() -> i32 { 30 }");
    oxel::check_project(&[&new_main, &new_helper], &oxel::References::default()).unwrap();
    assert_eq!(std::fs::read_to_string(main).unwrap(), main_source);
    assert_eq!(std::fs::read_to_string(&helper).unwrap(), helper_source);

    std::fs::write(&helper, format!("// edited outside LSP\n{helper_source}")).unwrap();
    let modified = client
        .response("textDocument/rename", params)
        .error
        .unwrap();
    assert_eq!(modified.code, lsp_server::ErrorCode::ContentModified as i32);
    client.finish();
}

#[test]
fn rename_expands_struct_shorthand_and_preserves_the_other_binding() {
    let mut client = Client::start(true, None);
    let uri = "file:///virtual/struct-shorthand.oxel";
    let lines = [
        "struct Panel { title: string }",
        "fn make(title: string) -> Panel { Panel { title } }",
        "fn read(panel: Panel) -> string { let Panel { title } = panel; title }",
    ];
    client.open(uri, &lines.join("\n"));
    assert_eq!(client.diagnostics()["diagnostics"], json!([]));

    for (line, new_name, expected) in [
        (
            0,
            "caption",
            vec!["caption", "caption: title", "caption: title"],
        ),
        (1, "name", vec!["name", "title: name"]),
        (2, "label", vec!["title: label", "label"]),
    ] {
        let result = client.request(
            "textDocument/rename",
            json!({
                "textDocument": {"uri": uri},
                "position": {"line": line, "character": lines[line].find("title").unwrap()},
                "newName": new_name,
            }),
        );
        let edits = result["changes"][uri]
            .as_array()
            .unwrap_or_else(|| panic!("{result}"));
        let replacements: Vec<_> = edits
            .iter()
            .map(|edit| edit["newText"].as_str().unwrap())
            .collect();
        assert_eq!(replacements, expected);
    }
    client.finish();
}

#[test]
fn rename_follows_constructed_source_types_and_enum_patterns() {
    let mut client = Client::start(true, None);
    let uri = "file:///virtual/generic-rename.oxel";
    let source = concat!(
        "struct Box<T> { value: T }\n",
        "enum Opt<T> { Some(T), None }\n",
        "fn wrap<T>(value: T) -> Box<Opt<T>> { Box { value: Opt.Some(value) } }\n",
        "fn read(item: Box<Opt<i32>>) -> i32 { match item.value { Some(x) => x, None => 0 } }",
    );
    client.open(uri, source);
    assert_eq!(client.diagnostics()["diagnostics"], json!([]));
    let rename = |line, character, new_name| {
        json!({
            "textDocument":{"uri":uri}, "position":{"line":line,"character":character},"newName":new_name
        })
    };
    for (line, text, old_name, new_name, count) in [
        (0, source.lines().next().unwrap(), "Box", "Container", 4),
        (1, source.lines().nth(1).unwrap(), "Some", "Present", 3),
        (2, source.lines().nth(2).unwrap(), "T", "ItemType", 3),
    ] {
        let params = rename(line, text.find(old_name).unwrap(), new_name);
        let result = client.request("textDocument/rename", params);
        let edits = result["changes"][uri].as_array().unwrap();
        assert_eq!(edits.len(), count, "{old_name}: {result}");
    }
    client.finish();
}

#[test]
fn semantic_tokens_classify_resolved_symbols_and_use_utf16_positions() {
    let mut client = Client::start(true, None);
    let uri = "file:///virtual/highlight.oxel";
    let lines = [
        "const LIMIT: i32 = 2;",
        "struct Box<T> { value: T }",
        "enum Option<T> { Some(T), None }",
        "fn wrap<T>(arg: T) -> Box<Option<T>> { Box { value: Option.Some(arg) } }",
        r#"fn main() { "🌍"; let item = wrap(LIMIT); let mut count = 0; count += 1; System.Console.WriteLine(count); }"#,
    ];
    client.open(uri, &lines.join("\r\n"));
    assert_eq!(client.diagnostics()["diagnostics"], json!([]));

    let tokens = client.highlights(uri, None);
    let expect = |line: usize, name: &str, occurrence: usize, kind: &str, modifiers: &[&str]| {
        let (byte, _) = lines[line].match_indices(name).nth(occurrence).unwrap();
        let column = lines[line][..byte].encode_utf16().count() as u32;
        let expected = Highlight {
            line: line as u32,
            column,
            length: name.len() as u32,
            kind: kind.to_owned(),
            modifiers: modifiers.iter().map(|name| (*name).to_owned()).collect(),
        };
        assert!(
            tokens.contains(&expected),
            "missing {expected:?} from {tokens:?}"
        );
    };
    expect(0, "LIMIT", 0, "variable", &["declaration", "readonly"]);
    expect(0, "i32", 0, "type", &["defaultLibrary"]);
    expect(1, "Box", 0, "struct", &["declaration"]);
    expect(1, "value", 0, "property", &["declaration", "readonly"]);
    expect(1, "T", 0, "typeParameter", &["declaration"]);
    expect(1, "T", 1, "typeParameter", &[]);
    expect(2, "Option", 0, "enum", &["declaration"]);
    expect(2, "Some", 0, "enumMember", &["declaration"]);
    expect(3, "wrap", 0, "function", &["declaration"]);
    expect(3, "arg", 0, "parameter", &["declaration", "readonly"]);
    expect(3, "arg", 1, "parameter", &["readonly"]);
    expect(3, "Box", 0, "struct", &[]);
    expect(3, "Box", 1, "struct", &[]);
    expect(3, "Some", 0, "enumMember", &[]);
    expect(4, "item", 0, "variable", &["declaration", "readonly"]);
    expect(4, "wrap", 0, "function", &[]);
    expect(4, "LIMIT", 0, "variable", &["readonly"]);
    expect(4, "count", 0, "variable", &["declaration"]);
    expect(4, "count", 1, "variable", &[]);
    expect(4, "WriteLine", 0, "method", &[]);

    let method = tokens.iter().find(|token| token.kind == "method").unwrap();
    let middle = method.column + 1;
    assert_eq!(
        client.highlights(
            uri,
            Some(json!({
                "start": {"line": 4, "character": middle},
                "end": {"line": 4, "character": middle + 1}
            }))
        ),
        [Highlight {
            line: method.line,
            column: method.column,
            length: method.length,
            kind: method.kind.clone(),
            modifiers: method.modifiers.clone(),
        }]
    );
    client.finish();
}

#[test]
fn semantic_tokens_follow_cross_file_edits_and_keep_recovered_declarations() {
    let folder = tempfile::tempdir().unwrap();
    let first = folder.path().join("entry.oxel");
    let second = folder.path().join("shape.oxel");
    let entry = "fn keep(value: Shape) -> Shape { value }";
    let shape = "enum Shape { Empty }";
    std::fs::write(&first, entry).unwrap();
    std::fs::write(&second, shape).unwrap();
    let first_uri = url::Url::from_file_path(&first).unwrap();
    let second_uri = url::Url::from_file_path(&second).unwrap();
    let mut client = Client::start_project(true, None, &[&first, &second]);
    client.open(first_uri.as_str(), entry);
    assert_eq!(client.diagnostics()["diagnostics"], json!([]));
    assert_eq!(
        client
            .highlights(first_uri.as_str(), None)
            .iter()
            .filter(|token| token.kind == "enum")
            .count(),
        2
    );

    client.open(second_uri.as_str(), "struct Shape { value: i32 }");
    for _ in 0..2 {
        assert_eq!(client.diagnostics()["diagnostics"], json!([]));
    }
    let tokens = client.highlights(first_uri.as_str(), None);
    assert_eq!(
        tokens.iter().filter(|token| token.kind == "struct").count(),
        2
    );
    assert!(!tokens.iter().any(|token| token.kind == "enum"));

    client.change(
        second_uri.as_str(),
        2,
        json!([{
            "text": "struct Shape { value: i32 }\nfn broken("
        }]),
    );
    for _ in 0..2 {
        client.diagnostics();
    }
    assert_eq!(
        client.highlights(first_uri.as_str(), None),
        [Highlight {
            line: 0,
            column: 3,
            length: 4,
            kind: "function".to_owned(),
            modifiers: vec!["declaration".to_owned()],
        }]
    );
    client.finish();
}

#[test]
fn quick_fixes_use_current_diagnostics_and_return_versioned_utf16_edits() {
    let mut command = Command::new(env!("CARGO_BIN_EXE_oxel"));
    command.arg("lsp");
    let mut client = Client::launch(command, true, true);
    let uri = "file:///virtual/fixes.oxel";
    let original = r#"fn main() { "🌍"; let count = 1; count += 2; }"#;
    client.open(uri, original);
    let diagnostic = client.diagnostics()["diagnostics"][0].clone();
    assert_eq!(diagnostic["code"], "E0204");
    let mut params = json!({
        "textDocument": {"uri": uri},
        "range": diagnostic["range"],
        "context": {"diagnostics": [diagnostic], "only": ["quickfix"]}
    });
    let response = client.request("textDocument/codeAction", params.clone());
    let actions = response.as_array().unwrap();
    assert_eq!(actions.len(), 1);
    let action = &actions[0];
    assert_eq!(action["kind"], "quickfix");
    assert_eq!(action["diagnostics"], json!([diagnostic]));
    let offset = original.find("count").unwrap();
    let position = json!({"line": 0, "character": original[..offset].encode_utf16().count()});
    assert_eq!(
        action["edit"]["documentChanges"],
        json!([{
            "textDocument": {"uri": uri, "version": 1},
            "edits": [{"range": {"start": position, "end": position}, "newText": "mut "}]
        }])
    );
    let mut filtered = params.clone();
    filtered["context"]["only"] = json!(["refactor"]);
    assert_eq!(
        client.request("textDocument/codeAction", filtered),
        json!([])
    );

    let mut unrelated = params.clone();
    unrelated["range"] = json!({
        "start": {"line": 0, "character": 3},
        "end": {"line": 0, "character": 4}
    });
    assert_eq!(
        client.request("textDocument/codeAction", unrelated),
        json!([])
    );

    let mut fixed = original.to_owned();
    fixed.insert_str(
        offset,
        action["edit"]["documentChanges"][0]["edits"][0]["newText"]
            .as_str()
            .unwrap(),
    );
    client.change(uri, 2, json!([{"text": fixed}]));
    assert_eq!(client.diagnostics()["diagnostics"], json!([]));
    assert_eq!(
        client.request("textDocument/codeAction", params.clone()),
        json!([])
    );

    let bad = "fn main() { let n = 1\r\n}";
    client.change(uri, 3, json!([{"text": bad}]));
    let missing = client.diagnostics()["diagnostics"][0].clone();
    assert_eq!(missing["code"], "E0102");
    params["range"] = missing["range"].clone();
    params["context"]["diagnostics"] = json!([missing]);
    let action = &client.request("textDocument/codeAction", params)[0];
    let edit = &action["edit"]["documentChanges"][0];
    assert_eq!(edit["textDocument"]["version"], 3);
    assert_eq!(edit["edits"][0]["newText"], ";");
    assert_eq!(
        edit["edits"][0]["range"]["start"],
        json!({
            "line": 0, "character": bad.find('\r').unwrap()
        })
    );
    client.finish();
}

#[test]
fn quick_fixes_for_a_closed_file_require_the_current_disk_snapshot() {
    let folder = tempfile::tempdir().unwrap();
    let source = folder.path().join("main.oxel");
    std::fs::write(&source, "fn main() { let flag: bool? = false; }").unwrap();
    let uri = url::Url::from_file_path(&source).unwrap();
    let mut client = Client::start_project(true, None, &[&source]);
    let params = json!({
        "textDocument": {"uri": uri.as_str()},
        "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 37}},
        "context": {"diagnostics": []}
    });
    let response = client.request("textDocument/codeAction", params.clone());
    assert_eq!(response[0]["kind"], "quickfix");
    let edit = &response[0]["edit"]["changes"][uri.as_str()][0];
    assert_eq!(edit["newText"], "");
    assert_eq!(
        edit["range"],
        json!({
            "start": {"line": 0, "character": 26}, "end": {"line": 0, "character": 27}
        })
    );

    std::fs::write(source, "fn main() {}").unwrap();
    assert_eq!(client.request("textDocument/codeAction", params), json!([]));
    client.finish();
}

#[test]
fn library_lsp_treats_main_as_an_ordinary_function() {
    let folder = tempfile::tempdir().unwrap();
    let file = folder.path().join("api.oxel");
    let source = "fn main(flag: bool) -> bool { flag }";
    std::fs::write(&file, source).unwrap();
    std::fs::write(
        folder.path().join("oxel.toml"),
        "[package]\nname = 'Library'\nkind = 'library'\nsources = ['api.oxel']",
    )
    .unwrap();
    let uri = url::Url::from_file_path(&file).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_oxel"));
    command
        .current_dir(folder.path())
        .args(["lsp", "--manifest-path", "oxel.toml"]);
    let mut client = Client::launch(command, true, false);
    client.open(uri.as_str(), source);
    assert_eq!(client.diagnostics()["diagnostics"], json!([]));
    let parameter_rename = client.request(
        "textDocument/rename",
        json!({
            "textDocument": {"uri": uri.as_str()},
            "position": {"line": 0, "character": source.find("flag").unwrap()},
            "newName": "state"
        }),
    );
    let edits = parameter_rename["changes"][uri.as_str()]
        .as_array()
        .unwrap();
    assert_eq!(edits.len(), 2);
    assert!(edits.iter().all(|edit| edit["newText"] == "state"));

    let rename = client.request(
        "textDocument/rename",
        json!({
            "textDocument": {"uri": uri.as_str()},
            "position": {"line": 0, "character": 3},
            "newName": "enabled"
        }),
    );
    assert_eq!(rename["changes"][uri.as_str()][0]["newText"], "enabled");
    client.finish();
}
