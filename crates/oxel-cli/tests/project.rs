use std::{fs, path::Path, process::Command};

use oxel::{References, Span, check_project, compile_project, inspect_project};

const MAIN: &str = include_str!("../../../examples/project/main.oxel");
const GEOMETRY: &str = include_str!("../../../examples/project/geometry.oxel");

#[test]
fn compiles_and_runs_cross_file_calls_and_structs() {
    let folder = tempfile::tempdir().unwrap();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/project");
    let main = root.join("main.oxel");
    let types = root.join("geometry.oxel");
    let cli = |action| {
        Command::new(env!("CARGO_BIN_EXE_oxel"))
            .current_dir(folder.path())
            .arg(action)
            .arg(&main)
            .arg(&types)
            .output()
            .unwrap()
    };
    let checked = cli("check");
    assert!(
        checked.status.success(),
        "{}",
        String::from_utf8_lossy(&checked.stderr)
    );
    assert!(!folder.path().join("target").exists());
    let compiled = cli("compile");
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let artifact = folder
        .path()
        .join(String::from_utf8(compiled.stdout).unwrap().trim());
    assert_eq!(artifact.file_name().unwrap(), "main.dll");
    let references = References::default();
    assert_eq!(
        fs::read(&artifact).unwrap(),
        compile_project(&[MAIN, GEOMETRY], "main", &references).unwrap()
    );
    let result = cli("run");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(String::from_utf8(result.stdout).unwrap().trim(), "30");
}

#[test]
fn preserves_file_boundaries_and_diagnostic_spans() {
    let sources = ["fn unfinished() {", "", "fn main() { let x = ; }"];
    let errors = check_project(&sources, &References::default()).unwrap_err();
    assert_eq!(errors.len(), 2);
    assert_eq!(errors[0].source, Some(0));
    assert_eq!(
        errors[0].diagnostic.span,
        Span {
            start: sources[0].len(),
            end: sources[0].len()
        }
    );
    assert_eq!(errors[1].source, Some(2));
    let sources = ["", "fn main() {}", "fn broken() { let _: i32 = false; }"];
    let errors = check_project(&sources, &References::default()).unwrap_err();
    assert_eq!(errors[0].source, Some(2));
    let span = errors[0].diagnostic.span;
    assert_eq!(&sources[2][span.start..span.end], "false");
    let errors =
        check_project(&["fn main() {}", "fn main() {}"], &References::default()).unwrap_err();
    assert_eq!(
        (errors[0].source, errors[0].diagnostic.code),
        (Some(1), "E0201")
    );
}

#[test]
fn editor_analysis_localizes_definitions_in_other_files() {
    let snapshots = inspect_project(&[MAIN, GEOMETRY], &References::default());
    assert!(
        snapshots
            .iter()
            .all(|document| document.diagnostics.is_empty())
    );
    let call = snapshots[0]
        .occurrences
        .iter()
        .find(|item| &MAIN[item.span.start..item.span.end] == "area")
        .unwrap();
    assert_eq!(call.definition_source, Some(1));
    let declaration = call.definition.unwrap();
    assert_eq!(&GEOMETRY[declaration.start..declaration.end], "area");
}

#[test]
fn cli_reports_the_secondary_source_filename() {
    let folder = tempfile::tempdir().unwrap();
    fs::write(folder.path().join("main.oxel"), "fn main() {}").unwrap();
    fs::write(
        folder.path().join("bad.oxel"),
        "fn broken() {\n    let value: i32 = false;\n}",
    )
    .unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_oxel"))
        .current_dir(folder.path())
        .args(["check", "main.oxel", "bad.oxel"])
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("bad.oxel:2:22: error[E0301]"));
    assert!(!folder.path().join("target").exists());
}

#[test]
fn incremental_inspection_reuses_syntax_and_refreshes_shared_symbols() {
    use std::sync::Arc;

    let references = References::default();
    let mut analyzer = oxel::ProjectAnalyzer::default();
    let main = MAIN.replace(
        "System.Console.WriteLine(area(box));",
        "let _: i32 = area(box);",
    );
    let mut sources: Vec<Arc<str>> = vec![
        Arc::from(main.as_str()),
        Arc::from(GEOMETRY),
        Arc::from("fn idle() {}"),
    ];
    let first = analyzer.inspect(&sources, &references);
    assert_eq!(analyzer.parsed_files(), 3);
    assert!(first.iter().all(|document| document.diagnostics.is_empty()));

    // A freshly allocated but identical buffer has the same syntax.
    sources[0] = Arc::from(main.as_str());
    assert_eq!(analyzer.inspect(&sources, &references), first);
    assert_eq!(analyzer.parsed_files(), 0);

    // Updating the type of a symbol changes diagnostics in a file whose
    // syntax is reused. The edit also shifts ranges inside the edited file.
    sources[1] = Arc::from(
        GEOMETRY
            .replace("-> i32", "-> bool")
            .replace("box.width * box.height", "true"),
    );
    let actual = analyzer.inspect(&sources, &references);
    assert_eq!(analyzer.parsed_files(), 1);
    assert_eq!(actual[0].diagnostics[0].code, "E0301");
    assert_eq!(analyzer.checked_functions(), 2);
    let cold: Vec<_> = sources.iter().map(Arc::as_ref).collect();
    assert_eq!(actual, inspect_project(&cold, &references));

    // Recover from a syntax error without invalidating unrelated files.
    sources[1] = Arc::from("fn broken(");
    let broken = analyzer.inspect(&sources, &references);
    assert_eq!(analyzer.parsed_files(), 1);
    assert_eq!(broken[1].diagnostics[0].code, "E0102");
    assert_eq!(analyzer.checked_functions(), 0);
    assert_eq!(broken[0].symbols[0].name, "main");
    assert_eq!(broken[2].symbols[0].name, "idle");
}

#[test]
fn incremental_inspection_preserves_ranges_after_a_file_outgrows_its_span_range() {
    use std::sync::Arc;

    let references = References::default();
    let mut analyzer = oxel::ProjectAnalyzer::default();
    let mut sources: Vec<Arc<str>> = vec![Arc::from(MAIN), Arc::from(GEOMETRY)];
    analyzer.inspect(&sources, &references);
    sources[0] = Arc::from(format!("//{}\n{MAIN}", "é".repeat(3000)));
    let actual = analyzer.inspect(&sources, &references);
    let cold: Vec<_> = sources.iter().map(Arc::as_ref).collect();
    assert_eq!(actual, inspect_project(&cold, &references));
    assert_eq!(analyzer.inspect(&sources, &references), actual);
    assert_eq!(analyzer.parsed_files(), 0);

    assert_eq!(analyzer.inspect(&[], &references), vec![]);
    assert_eq!(analyzer.parsed_files(), 0);
    assert_eq!(analyzer.inspect(&sources, &references), actual);
    assert_eq!(analyzer.parsed_files(), 2);
}

#[test]
fn body_edits_reuse_other_checks_and_relocate_definitions_and_diagnostics() {
    use std::sync::Arc;

    let references = References::default();
    let mut analyzer = oxel::ProjectAnalyzer::default();
    let mut sources: Vec<Arc<str>> = vec![
        Arc::from(
            "fn before() -> i32 { 1 }\nstruct Value { n: i32 }\nconst LIMIT = 3;\nfn after(value: Value) -> i32 { let n = value.n; n + LIMIT }\nfn bad() { let count = 1; count += 1; }",
        ),
        Arc::from("fn main() { let _: i32 = after(Value { n: before() }); }"),
    ];
    let first = analyzer.inspect(&sources, &references);
    assert_eq!(analyzer.checked_functions(), 4);
    assert_eq!(first[0].diagnostics[0].code, "E0204");
    let fix = first[0].diagnostics[0].fix().unwrap();
    assert_eq!(fix.span.start, sources[0].find("count").unwrap());
    assert_eq!(analyzer.inspect(&sources, &references), first);
    assert_eq!(analyzer.checked_functions(), 0);

    sources[0] =
        Arc::from(sources[0].replace("{ 1 }", "{ // moved following declarations\n1000 }"));
    let changed = analyzer.inspect(&sources, &references);
    assert_eq!(analyzer.checked_functions(), 1);
    assert_eq!(analyzer.parsed_files(), 1);
    assert!(changed[0].diagnostics[0].span.start > first[0].diagnostics[0].span.start);
    let fix = changed[0].diagnostics[0].fix().unwrap();
    assert_eq!(fix.span.start, sources[0].find("count").unwrap());
    assert_eq!(
        changed,
        inspect_project(
            &sources.iter().map(Arc::as_ref).collect::<Vec<_>>(),
            &references
        )
    );

    sources[0] = Arc::from(sources[0].replace("const LIMIT = 3;", "const LIMIT = false;"));
    let changed = analyzer.inspect(&sources, &references);
    assert_eq!(analyzer.checked_functions(), 1);
    assert_eq!(changed[0].diagnostics.len(), 2);
    assert_eq!(
        changed,
        inspect_project(
            &sources.iter().map(Arc::as_ref).collect::<Vec<_>>(),
            &references
        )
    );

    sources[0] = Arc::from(sources[0].replace("n: i32", "n: bool"));
    let changed = analyzer.inspect(&sources, &references);
    assert_eq!(analyzer.checked_functions(), 2);
    assert_eq!(changed[1].diagnostics[0].code, "E0301");
    assert_eq!(
        changed,
        inspect_project(
            &sources.iter().map(Arc::as_ref).collect::<Vec<_>>(),
            &references
        )
    );
}

#[test]
fn type_shape_changes_follow_inferred_and_transitive_types() {
    use std::sync::Arc;

    let references = References::default();
    let mut analyzer = oxel::ProjectAnalyzer::default();
    let functions = concat!(
        "fn load(b: Box) -> Box { b }\n",
        "fn read(b: Box) -> i32 { load(b).payload.n }\n",
        "fn inferred() -> i32 { item().payload.n }\n",
        "fn item() -> Box { Box { payload: Payload { n: 30 } } }\n",
        "fn unaffected(s: Other) -> i32 { s.number }\n",
        "fn missing() { Future { n: 0 }; }\n",
        "fn signal() { Signal.Ready; }\n",
        "fn plain() -> i32 { 30 }",
    );
    let declarations =
        "struct Payload { n: i32 } struct Box { payload: Payload } struct Other { number: i32 }";
    let mut sources: [Arc<str>; 2] = [Arc::from(declarations), Arc::from(functions)];
    let initial = analyzer.inspect(&sources, &references);
    assert_eq!(analyzer.checked_functions(), 8);
    assert_eq!(initial[1].diagnostics.len(), 2);
    assert_eq!(initial[1].diagnostics[0].code, "E0202");

    sources[0] = Arc::from(sources[0].replace("n: i32", "n: bool"));
    let changed = analyzer.inspect(&sources, &references);
    assert_eq!(analyzer.checked_functions(), 4);
    assert_eq!(
        changed,
        inspect_project(&[&sources[0], functions], &references)
    );
    assert_eq!(changed[1].diagnostics.len(), 5);

    sources[0] = Arc::from(format!(
        "struct Future {{ n: i32 }} enum Signal {{ Ready }}\n{}",
        sources[0],
    ));
    let added = analyzer.inspect(&sources, &references);
    assert_eq!(analyzer.checked_functions(), 2);
    assert_eq!(
        added,
        inspect_project(&[&sources[0], functions], &references)
    );
    assert_eq!(added[1].diagnostics.len(), 3);
}

#[test]
fn rechecks_diagnostics_when_source_type_ids_move() {
    use std::sync::Arc;

    let references = References::default();
    let mut analyzer = oxel::ProjectAnalyzer::default();
    let functions = "fn mismatch() -> A { B {} } fn good() -> A { A {} }";
    let mut sources: [Arc<str>; 2] = [Arc::from("struct A {} struct B {}"), Arc::from(functions)];
    let before = analyzer.inspect(&sources, &references);
    let error = &before[1].diagnostics[0];
    assert_eq!(error.type_names(), Some(("A", "B")));

    sources[0] = Arc::from("struct B {} struct A {}");
    let after = analyzer.inspect(&sources, &references);
    assert_eq!(analyzer.checked_functions(), 1);
    assert_eq!(
        after,
        inspect_project(&[&sources[0], functions], &references)
    );
    assert_ne!(after[1].diagnostics[0].expected, error.expected);
}

#[test]
fn body_cache_invalidates_when_clr_references_change() {
    use std::sync::Arc;

    let mut references = References::default();
    let mut analyzer = oxel::ProjectAnalyzer::default();
    let sources = [Arc::<str>::from(
        "fn main() { let n: i32 = Program.value(); }",
    )];
    let first = analyzer.inspect(&sources, &references);
    assert_eq!(first[0].diagnostics[0].code, "E0203");
    assert_eq!(analyzer.checked_functions(), 1);

    let dll = oxel::compile("fn main() {} fn value() -> i32 { 30 }", "Values").unwrap();
    references.add(&dll).unwrap();
    let second = analyzer.inspect(&sources, &references);
    assert!(second[0].diagnostics.is_empty());
    assert_eq!(analyzer.parsed_files(), 0);
    assert_eq!(analyzer.checked_functions(), 1);
}

#[test]
fn signature_edits_recheck_only_callers_that_use_the_signature() {
    use std::sync::Arc;

    let mut analyzer = oxel::ProjectAnalyzer::default();
    let references = References::default();
    let mut sources: Vec<Arc<str>> = vec![
        Arc::from("fn leaf() -> i32 { 30 }\nfn neighbor() -> i32 { 4 }"),
        Arc::from(
            "fn middle() -> i32 { leaf() }\nfn main() { System.Console.WriteLine(middle()); }",
        ),
        Arc::from("fn independent() -> i32 { neighbor() }"),
    ];
    assert!(
        analyzer
            .inspect(&sources, &references)
            .iter()
            .all(|document| document.diagnostics.is_empty())
    );
    assert_eq!(analyzer.checked_functions(), 5);
    sources[0] = Arc::from("fn leaf() -> bool { true }\nfn neighbor() -> i32 { 4 }");
    let actual = analyzer.inspect(&sources, &references);
    assert_eq!(analyzer.checked_functions(), 2);
    assert_eq!(analyzer.parsed_files(), 1);
    assert_eq!(actual[1].diagnostics[0].code, "E0301");
    assert_eq!(
        actual,
        inspect_project(
            &sources.iter().map(Arc::as_ref).collect::<Vec<_>>(),
            &references
        ),
    );
}

#[test]
fn adding_and_removing_functions_invalidates_resolved_and_unresolved_calls() {
    use std::sync::Arc;

    let mut analyzer = oxel::ProjectAnalyzer::default();
    let references = References::default();
    let mut sources: Vec<Arc<str>> = vec![
        Arc::from("fn retained() -> i32 { 30 }"),
        Arc::from("fn main() { missing(); }\nfn other() -> i32 { retained() }"),
    ];
    let first = analyzer.inspect(&sources, &references);
    assert_eq!(first[1].diagnostics[0].code, "E0203");
    assert_eq!(analyzer.checked_functions(), 3);

    sources[0] = Arc::from("fn missing() {}\nfn retained() -> i32 { 30 }");
    let added = analyzer.inspect(&sources, &references);
    assert!(added[1].diagnostics.is_empty());
    assert_eq!(analyzer.checked_functions(), 2);
    assert_eq!(
        added,
        inspect_project(
            &sources.iter().map(Arc::as_ref).collect::<Vec<_>>(),
            &references
        ),
    );

    sources[0] = Arc::from("fn retained() -> i32 { 30 }");
    assert_eq!(analyzer.inspect(&sources, &references), first);
    assert_eq!(analyzer.checked_functions(), 1);
}

#[test]
fn incremental_analysis_tracks_generic_parameters_and_signature_changes() {
    let references = References::default();
    let caller = "fn main() { identity(30); }";
    let declaration = "fn identity<T>(value: T) -> T { let result: T = value; result }";
    let mut analysis = oxel::ProjectAnalyzer::default();
    let mut sources = [
        std::sync::Arc::from(caller),
        std::sync::Arc::from(declaration),
    ];
    let first = analysis.inspect(&sources, &references);
    assert!(first.iter().all(|file| file.diagnostics.is_empty()));
    let parameter = declaration.find("<T>").unwrap() + 1;
    let occurrences = &first[1].occurrences;
    let uses = occurrences.iter().filter(|item| {
        item.definition
            == Some(Span {
                start: parameter,
                end: parameter + 1,
            })
    });
    assert_eq!(uses.count(), 3);

    let updated = "fn identity<T, U>(value: T) -> T { let result: T = value; result }";
    sources[1] = std::sync::Arc::from(updated);
    let second = analysis.inspect(&sources, &references);
    assert_eq!(analysis.checked_functions(), 2);
    assert_eq!(second, inspect_project(&[caller, updated], &references));
    assert_eq!(second[0].diagnostics[0].code, "E0210");
    assert!(second[0].diagnostics[0].message.contains("'U'"));
}

#[test]
fn editor_analysis_resolves_constructed_type_annotations_and_preserves_diagnostics() {
    use std::sync::Arc;

    let references = References::default();
    let input = "fn main() { Box::<string> { value: \"a\" }; }\n                 fn mismatch(arg: Box<i32>) -> Box<bool> { Box::<string> { value: arg.value } }";
    let definition = "struct Box<T> { value: T }";
    let mut analyzer = oxel::ProjectAnalyzer::default();
    let mut sources = [Arc::from(input), Arc::from(definition)];
    let first = analyzer.inspect(&sources, &references);
    let usage = first[1]
        .occurrences
        .iter()
        .find(|item| item.definition.is_some())
        .unwrap();
    let declaration = usage.definition.unwrap();
    assert_eq!(&definition[declaration.start..declaration.end], "T");
    assert_eq!(
        first[0].diagnostics[0].type_names(),
        Some(("System.String", "System.Int32"))
    );
    assert_eq!(analyzer.inspect(&sources, &references), first);
    assert_eq!(analyzer.checked_functions(), 0);

    let edited = "fn main() { Box::<i64> { value: 30 }; }\n                  fn mismatch(arg: Box<i32>) -> Box<bool> { Box::<string> { value: \"text\" } }";
    sources[0] = Arc::from(edited);
    let actual = analyzer.inspect(&sources, &references);
    assert_eq!(actual, inspect_project(&[edited, definition], &references));
    assert_eq!(
        actual[0].diagnostics[0].type_names(),
        Some(("Box<System.Boolean>", "Box<System.String>"))
    );
    assert_eq!(analyzer.inspect(&sources, &references), actual);
    assert_eq!(analyzer.checked_functions(), 0);
}

#[test]
fn type_formatting_and_file_moves_keep_cached_bodies_and_relocate_links() {
    use std::sync::Arc;

    let references = References::default();
    let mut analyzer = oxel::ProjectAnalyzer::default();
    let input = concat!(
        "const INDEX = 0;\n",
        "fn read(p: Pick<i32>) -> i32 { match p { ",
        "Pick.Some(boxed) => boxed.values[INDEX], Pick.Empty => 0, } }",
    );
    let mut sources: [Arc<str>; 3] = [
        Arc::from("struct Box<T: struct> { values: T[] }"),
        Arc::from("enum Pick<T: struct> { Some(Box<T>), Empty }"),
        Arc::from(input),
    ];
    let first = analyzer.inspect(&sources, &references);
    assert!(first.iter().all(|document| document.diagnostics.is_empty()));
    assert_eq!(analyzer.checked_functions(), 1);
    assert_eq!(analyzer.evaluated_constants(), 1);

    sources[0] = Arc::from("");
    sources[1] = Arc::from(concat!(
        "struct Box<T: struct,> {\n    values: T [ ], // payload\n}\n",
        "enum Pick<T: struct> {\n    Some(Box<T>,), Empty(),\n}",
    ));
    let edited = analyzer.inspect(&sources, &references);
    assert_eq!(analyzer.checked_functions(), 0);
    assert_eq!(analyzer.evaluated_constants(), 0);
    assert_eq!(
        edited,
        inspect_project(&["", &sources[1], input], &references)
    );
    assert!(edited[2].occurrences.iter().any(|item| {
        item.definition_source == Some(1)
            && item
                .definition
                .is_some_and(|span| &sources[1][span.start..span.end] == "values")
    }));

    sources[1] = Arc::from(sources[1].replace("values:", "other:"));
    let changed = analyzer.inspect(&sources, &references);
    assert_eq!(analyzer.checked_functions(), 1);
    assert!(!changed[2].diagnostics.is_empty());
    assert_eq!(
        changed,
        inspect_project(&["", &sources[1], input], &references)
    );
}

#[test]
fn cached_call_diagnostics_follow_the_current_declaration_location() {
    use std::sync::Arc;

    let references = References::default();
    let caller = "fn main() { take(false); }";
    let mut files: [Arc<str>; 3] = [
        Arc::from(caller),
        Arc::from("fn take(value: i32) {}"),
        Arc::from(""),
    ];
    let mut analyzer = oxel::ProjectAnalyzer::default();
    let initial = analyzer.inspect(&files, &references);
    let related = initial[0].diagnostics[0].related().unwrap();
    assert_eq!(related.source, Some(1));
    assert_eq!(related.span.start, 3);

    files[1] = Arc::from("");
    files[2] = Arc::from("// moved 🌍\nfn take(value: i32) {}");
    let edited = analyzer.inspect(&files, &references);
    assert_eq!(analyzer.checked_functions(), 0);
    assert_eq!(
        edited,
        inspect_project(&[caller, "", &files[2]], &references)
    );
    let related = edited[0].diagnostics[0].related().unwrap();
    assert_eq!(related.source, Some(2));
    assert_eq!(related.location(&files[2]), (2, 4));
}

#[test]
fn marking_a_function_as_a_test_invalidates_cached_callers() {
    use std::sync::Arc;

    let references = References::default();
    let mut files = [
        Arc::from("fn main() { helper(); }"),
        Arc::from("internal fn helper() {}"),
    ];
    let mut analyzer = oxel::ProjectAnalyzer::default();
    assert!(
        analyzer.inspect(&files, &references)[0]
            .diagnostics
            .is_empty()
    );

    files[1] = Arc::from("#[test] #[ignore] fn helper() {}");
    let marked = analyzer.inspect(&files, &references);
    assert_eq!(marked[0].diagnostics[0].code, "E0346");

    files[1] = Arc::from("internal fn helper() {}");
    assert!(
        analyzer.inspect(&files, &references)[0]
            .diagnostics
            .is_empty()
    );
}

#[test]
fn field_type_edits_recheck_generic_constructor_calls() {
    use std::sync::Arc;

    let references = References::default();
    let mut analyzer = oxel::ProjectAnalyzer::default();
    let caller = "fn make<T: new()>() -> T { new T() } fn main() { make::<Wrapper>(); }";
    let wrapper = "struct Wrapper { record: Record }";
    let mut sources = [
        Arc::from(caller),
        Arc::from(wrapper),
        Arc::from("struct Record { label: string? }"),
    ];
    let initial = analyzer.inspect(&sources, &references);
    assert!(initial.iter().all(|file| file.diagnostics.is_empty()));

    sources[2] = Arc::from("struct Record { label: string }");
    let changed = analyzer.inspect(&sources, &references);
    assert_eq!(changed[0].diagnostics[0].code, "E0212");
    assert_eq!(
        changed,
        inspect_project(&[caller, wrapper, &sources[2]], &references)
    );
}

#[test]
fn signature_formatting_rechecks_only_the_edited_function() {
    use std::sync::Arc;

    let references = References::default();
    let mut analyzer = oxel::ProjectAnalyzer::default();
    let caller = "fn main() { same(30); }";
    let mut sources: [Arc<str>; 2] = [
        Arc::from(caller),
        Arc::from("fn same<T: struct>(value: T) -> T { value }"),
    ];
    let first = analyzer.inspect(&sources, &references);
    assert!(first.iter().all(|document| document.diagnostics.is_empty()));

    sources[1] = Arc::from("fn same<T: struct,>(\n value: T, // argument\n) -> T { value }");
    let changed = analyzer.inspect(&sources, &references);
    assert_eq!(analyzer.checked_functions(), 1);
    assert_eq!(
        changed,
        inspect_project(&[caller, &sources[1]], &references)
    );

    sources[1] = Arc::from(sources[1].replace("value", "number"));
    let renamed = analyzer.inspect(&sources, &references);
    assert_eq!(analyzer.checked_functions(), 2);
    assert_eq!(
        renamed,
        inspect_project(&[caller, &sources[1]], &references)
    );
}

#[test]
fn rechecks_definite_assignment_after_editing_a_branch() {
    use std::sync::Arc;

    let references = References::default();
    let declaration = "fn input() -> bool { false }";
    let before =
        "fn main() { let value: i32; if input() { value = 30; } System.Console.WriteLine(value); }";
    let after = "fn main() { let value: i32; if input() { value = 30; } else { value = 20; } System.Console.WriteLine(value); }";
    let mut analyzer = oxel::ProjectAnalyzer::default();
    let mut sources = [Arc::from(declaration), Arc::from(before)];
    let initial = analyzer.inspect(&sources, &references);
    let diagnostic = &initial[1].diagnostics[0];
    assert_eq!(diagnostic.code, "E0321");
    assert_eq!(diagnostic.span.start, before.rfind("value").unwrap());
    assert_eq!(analyzer.inspect(&sources, &references), initial);
    assert_eq!(analyzer.checked_functions(), 0);

    sources[1] = Arc::from(after);
    let updated = analyzer.inspect(&sources, &references);
    assert!(updated.iter().all(|file| file.diagnostics.is_empty()));
    assert_eq!(analyzer.checked_functions(), 1);
    assert_eq!(updated, inspect_project(&[declaration, after], &references));
}

#[test]
fn recomputed_comptime_values_invalidate_only_their_runtime_consumers() {
    use std::sync::Arc;

    let references = References::default();
    let runtime = concat!(
        "fn changed() -> i32 { DERIVED } fn stable() -> i32 { STABLE } ",
        "fn main() { System.Console.WriteLine(changed() + stable()); }",
    );
    let constants =
        "const VALUE = compute(); const DERIVED = VALUE * 2; const STABLE = VALUE % 15;";
    let before = "comptime fn compute() -> i32 { 30 }";
    let equivalent = "comptime fn compute() -> i32 { 15 * 2 }";
    let after = "comptime fn compute() -> i32 { 60 }";
    let mut analyzer = oxel::ProjectAnalyzer::default();
    let mut sources = [Arc::from(runtime), Arc::from(constants), Arc::from(before)];
    let initial = analyzer.inspect(&sources, &references);
    assert!(initial.iter().all(|file| file.diagnostics.is_empty()));
    assert_eq!(analyzer.checked_functions(), 3);
    assert_eq!(analyzer.evaluated_constants(), 3);
    assert_eq!(analyzer.inspect(&sources, &references), initial);
    assert_eq!(analyzer.evaluated_constants(), 0);

    for (source, checked, evaluated) in [(equivalent, 0, 1), (after, 1, 3)] {
        sources[2] = Arc::from(source);
        let updated = analyzer.inspect(&sources, &references);
        assert_eq!(analyzer.checked_functions(), checked);
        assert_eq!(analyzer.evaluated_constants(), evaluated);
        assert_eq!(
            updated,
            inspect_project(&[runtime, constants, source], &references)
        );
    }

    sources[2] = Arc::from("comptime fn compute() -> i32 { 30 / 0 }");
    let invalid = analyzer.inspect(&sources, &references);
    assert_eq!(invalid[2].diagnostics[0].code, "E0503");
    assert_eq!(analyzer.evaluated_constants(), 1);
    assert_eq!(analyzer.inspect(&sources, &references), invalid);
    assert_eq!(analyzer.evaluated_constants(), 1);
    sources[2] = Arc::from(after);
    assert_eq!(
        analyzer.inspect(&sources, &references),
        inspect_project(&[runtime, constants, after], &references)
    );
    assert_eq!(analyzer.checked_functions(), 0);
    assert_eq!(analyzer.evaluated_constants(), 0);

    sources[2] = Arc::from(before);
    assert_eq!(analyzer.inspect(&sources, &references), initial);
    assert_eq!(analyzer.checked_functions(), 1);
}

#[test]
fn constant_lookups_follow_moves_additions_and_removals() {
    use std::sync::Arc;

    let references = References::default();
    let runtime = concat!(
        "fn read() -> i32 { KEEP } fn missing() -> i32 { ADDED } ",
        "fn assign() { ADDED = 1; } fn shadow(ADDED: i32) -> i32 { ADDED } ",
        "fn main() { System.Console.WriteLine(30); }",
    );
    let mut analyzer = oxel::ProjectAnalyzer::default();
    let mut sources = [Arc::from(runtime), Arc::from(""), Arc::from("")];
    let mut inspect = |first: &str, second: &str, checked| {
        sources[1] = Arc::from(first);
        sources[2] = Arc::from(second);
        let updated = analyzer.inspect(&sources, &references);
        assert_eq!(analyzer.checked_functions(), checked, "{first} | {second}");
        assert_eq!(
            updated,
            inspect_project(&[runtime, first, second], &references)
        );
        updated
    };

    let initial = inspect("const KEEP = 30;", "const OTHER = 1;", 5);
    assert_eq!(initial[0].diagnostics.len(), 2);
    let shifted = "const OTHER = 1; const KEEP = 15 + 15;";
    let moved = inspect("", shifted, 0);
    let occurrence = moved[0]
        .occurrences
        .iter()
        .find(|item| item.definition_source == Some(2))
        .unwrap();
    assert_eq!(
        occurrence.definition.unwrap().start,
        shifted.find("KEEP").unwrap()
    );

    let added = inspect("const ADDED = 3;", shifted, 2);
    assert_eq!(added[0].diagnostics.len(), 1);
    assert_eq!(
        added[0].diagnostics[0].message,
        "constant 'ADDED' is immutable"
    );
    let clash = inspect("const ADDED = 3; const System = false;", shifted, 1);
    assert_eq!(clash[0].diagnostics.len(), 2);
    let removed = inspect("", shifted, 3);
    assert_eq!(removed[0].diagnostics, initial[0].diagnostics);

    let typed = inspect("", "const OTHER = 1; const KEEP: i64 = 30;", 1);
    assert_eq!(
        typed[0].diagnostics[0].type_names(),
        Some(("System.Int32", "System.Int64"))
    );
    let missing = inspect("", "const OTHER = 1;", 1);
    assert_eq!(missing[0].diagnostics[0].code, "E0203");
}

#[test]
fn constant_nullability_changes_invalidate_consumers_with_the_same_text() {
    use std::sync::Arc;

    let references = References::default();
    let runtime = "fn read() -> string { VALUE } fn main() {}";
    let nonnull = r#"const VALUE = "oxel";"#;
    let nullable = r#"const VALUE: string? = "oxel";"#;
    let mut sources = [Arc::from(runtime), Arc::from(nonnull)];
    let mut analyzer = oxel::ProjectAnalyzer::default();
    let first = analyzer.inspect(&sources, &references);
    assert!(first.iter().all(|file| file.diagnostics.is_empty()));

    sources[1] = Arc::from(nullable);
    let result = analyzer.inspect(&sources, &references);
    assert_eq!(result, inspect_project(&[runtime, nullable], &references));
    assert_eq!(result[0].diagnostics[0].code, "E0301");
    assert_eq!(analyzer.checked_functions(), 1);

    sources[1] = Arc::from(nonnull);
    assert_eq!(analyzer.inspect(&sources, &references), first);
    assert_eq!(analyzer.checked_functions(), 1);
}

#[test]
fn constant_evaluation_tracks_recursive_groups_and_cross_file_dependencies() {
    use std::sync::Arc;

    let references = References::default();
    let constants = "const DIVISOR = 1; const VALUE = 30 / even(3); const INDEPENDENT = 1 + 2;";
    let even = "comptime fn even(n: i32) -> i32 { if n == 0 { 1 } else { odd(n - 1) } }";
    let odd = "comptime fn odd(n: i32) -> i32 { match n { 0 => DIVISOR, _ => even(n - 1) } }";
    let runtime = "fn main() { System.Console.WriteLine(VALUE + INDEPENDENT); }";
    let mut analyzer = oxel::ProjectAnalyzer::default();
    let mut sources = [
        Arc::from(constants),
        Arc::from(even),
        Arc::from(odd),
        Arc::from(runtime),
    ];
    let inspect = |analyzer: &mut oxel::ProjectAnalyzer, sources: &[Arc<str>]| {
        let result = analyzer.inspect(sources, &references);
        let cold: Vec<_> = sources.iter().map(Arc::as_ref).collect();
        assert_eq!(result, inspect_project(&cold, &references));
        result
    };
    assert!(
        inspect(&mut analyzer, &sources)
            .iter()
            .all(|file| file.diagnostics.is_empty())
    );
    assert_eq!(analyzer.evaluated_constants(), 3);

    // Changing graph indices and moving sources keeps both results and fresh definition links.
    sources.swap(1, 2);
    assert!(
        inspect(&mut analyzer, &sources)
            .iter()
            .all(|file| file.diagnostics.is_empty())
    );
    assert_eq!(analyzer.evaluated_constants(), 0);
    assert_eq!(analyzer.checked_functions(), 0);

    // The only edit is in an upstream constant read by the other function in the group.
    sources[0] = Arc::from(constants.replace("DIVISOR = 1", "DIVISOR = 0"));
    let invalid = inspect(&mut analyzer, &sources);
    assert_eq!(invalid[0].diagnostics[0].code, "E0503");
    assert_eq!(analyzer.evaluated_constants(), 2);

    sources[0] = Arc::from(constants);
    assert!(
        inspect(&mut analyzer, &sources)
            .iter()
            .all(|file| file.diagnostics.is_empty())
    );
    assert_eq!(analyzer.evaluated_constants(), 1);
    assert_eq!(analyzer.checked_functions(), 0);

    sources[1] = Arc::from(odd.replace("0 => DIVISOR", "0 => DIVISOR - 1"));
    let invalid = inspect(&mut analyzer, &sources);
    assert_eq!(invalid[0].diagnostics[0].code, "E0503");
    assert_eq!(analyzer.evaluated_constants(), 1);
}

#[test]
fn visibility_edits_update_signatures_and_cross_file_diagnostics() {
    use std::sync::Arc;

    let references = References::default();
    let api = "fn read(value: Entry) -> i32 { convert(value) } fn main() {}";
    let public = "struct Entry { value: i32 } fn convert(value: Entry) -> i32 { value.value }";
    let private_helper =
        "struct Entry { value: i32 } internal fn convert(value: Entry) -> i32 { value.value }";
    let private_type = "internal struct Entry { value: i32 } internal fn convert(value: Entry) -> i32 { value.value }";
    let mut sources = [Arc::from(api), Arc::from(public)];
    let mut analyzer = oxel::ProjectAnalyzer::default();
    let first = analyzer.inspect(&sources, &references);
    assert!(first.iter().all(|file| file.diagnostics.is_empty()));

    sources[1] = Arc::from(private_helper);
    let updated = analyzer.inspect(&sources, &references);
    assert_eq!(
        updated,
        inspect_project(&[api, private_helper], &references)
    );
    assert!(
        updated[0]
            .occurrences
            .iter()
            .any(|item| item.detail.starts_with("internal fn convert("))
    );

    sources[1] = Arc::from(private_type);
    let invalid = analyzer.inspect(&sources, &references);
    assert_eq!(invalid, inspect_project(&[api, private_type], &references));
    assert_eq!(invalid[0].diagnostics[0].code, "E0342");
    assert_eq!(invalid[0].diagnostics[0].related().unwrap().source, Some(1));

    sources[1] = Arc::from(public);
    assert_eq!(analyzer.inspect(&sources, &references), first);
}

#[test]
fn compiler_type_queries_invalidate_only_dependents() {
    use std::sync::Arc;

    let references = References::default();
    let layout = "struct Pair { second: i32, first: i32 }";
    let queries = "const FIELD = first_field::<Pair>(); const EXACT = compiler.type_name::<i32>();";
    let function =
        "comptime fn first_field<T>() -> compiler.FieldInfo { compiler.fields_of::<T>()[0] }";
    let runtime =
        "fn found() -> string { FIELD.name } fn stable() -> string { EXACT } fn main() {}";
    let mut sources = [
        Arc::from(layout),
        Arc::from(queries),
        Arc::from(function),
        Arc::from(runtime),
    ];
    let mut analyzer = oxel::ProjectAnalyzer::default();
    let first = analyzer.inspect(&sources, &references);
    assert!(first.iter().all(|document| document.diagnostics.is_empty()));
    assert_eq!(analyzer.evaluated_constants(), 2);
    assert_eq!(analyzer.checked_functions(), 3);
    assert_eq!(analyzer.inspect(&sources, &references), first);
    assert_eq!(analyzer.evaluated_constants(), 0);

    let reordered = "struct Pair { first: i32, second: i32 }";
    sources[0] = Arc::from(reordered);
    let current = analyzer.inspect(&sources, &references);
    assert!(
        current
            .iter()
            .all(|document| document.diagnostics.is_empty())
    );
    assert_eq!(
        current,
        inspect_project(&[reordered, queries, function, runtime], &references)
    );
    assert_eq!(analyzer.evaluated_constants(), 1);
    assert_eq!(analyzer.checked_functions(), 1);
}

#[test]
fn aggregate_constants_survive_type_id_changes_and_recheck_layout_edits() {
    use std::sync::Arc;

    let references = References::default();
    let base = "struct Pair<T> { left: T, right: T }";
    let shifted = "struct Other { flag: bool } struct Pair<T> { left: T, right: T }";
    let reordered = "struct Other { flag: bool } struct Pair<T> { right: T, left: T }";
    let values = "const PAIR: Pair<i32> = Pair { right: 12, left: 30 }; const LEFT = PAIR.left;";
    let runtime = "fn main() { System.Console.WriteLine(PAIR.right + LEFT); }";
    let mut sources = [Arc::from(base), Arc::from(values), Arc::from(runtime)];
    let mut analyzer = oxel::ProjectAnalyzer::default();
    for (types, evaluated) in [(base, 2), (shifted, 0), (reordered, 2)] {
        sources[0] = Arc::from(types);
        let current = analyzer.inspect(&sources, &references);
        assert!(
            current
                .iter()
                .all(|document| document.diagnostics.is_empty())
        );
        assert_eq!(
            current,
            inspect_project(&[types, values, runtime], &references)
        );
        assert_eq!(analyzer.evaluated_constants(), evaluated);
    }
}

#[test]
fn constant_evaluation_uses_current_clr_reference_metadata() {
    use std::sync::Arc;

    // Oxel assemblies expose static functions under Program, which also tests
    // that the cache respects the module-wide metadata fingerprint.
    let source: Arc<str> = Arc::from(
        "const VALUE = compute(); comptime fn compute() -> i32 { 30 } fn main() { System.Console.WriteLine(VALUE); }",
    );
    let mut analyzer = oxel::ProjectAnalyzer::default();
    let mut references = References::default();
    let first = analyzer.inspect(std::slice::from_ref(&source), &references);
    assert!(first[0].diagnostics.is_empty());
    assert_eq!(analyzer.evaluated_constants(), 1);
    assert_eq!(
        first,
        analyzer.inspect(std::slice::from_ref(&source), &references)
    );
    assert_eq!(analyzer.evaluated_constants(), 0);

    let dependency = oxel::compile("fn main() {} fn value() -> i32 { 30 }", "Values").unwrap();
    references.add(&dependency).unwrap();
    assert_eq!(
        first,
        analyzer.inspect(std::slice::from_ref(&source), &references)
    );
    assert_eq!(analyzer.evaluated_constants(), 1);
}
#[test]
fn cached_nullable_matches_follow_changes_to_the_standard_option() {
    use std::sync::Arc;

    let mut analyzer = oxel::ProjectAnalyzer::default();
    let references = oxel::References::default();
    let body: Arc<str> = "fn show(value: string?) -> string { match value { Some(text) => text, None => \"missing\" } }".into();
    let legacy: Arc<str> = "enum Option<T> { Some(T), None }".into();
    assert!(
        analyzer.inspect(&[Arc::clone(&body)], &references)[0]
            .diagnostics
            .is_empty()
    );
    let changed = analyzer.inspect(&[Arc::clone(&body), legacy], &references);
    assert_eq!(changed[0].diagnostics[0].code, "E0331");
    assert!(
        analyzer.inspect(&[body], &references)[0]
            .diagnostics
            .is_empty()
    );
}
