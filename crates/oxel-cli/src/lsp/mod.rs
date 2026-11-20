//! The oxel language server speaks LSP over standard input and output.

mod code_actions;
mod document;
mod rename;
mod semantic_tokens;
mod signature_help;
mod watched_files;

use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{self, Path, PathBuf},
    sync::Arc,
};

use lsp_server::{Connection, ErrorCode, Message, Notification, Request, RequestId, Response};
use lsp_types::{
    CodeActionParams, CompletionItem, CompletionItemKind, CompletionOptions, CompletionParams,
    CompletionResponse, Diagnostic, DiagnosticRelatedInformation, DiagnosticSeverity,
    DidChangeTextDocumentParams, DidChangeWatchedFilesParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, DocumentChanges, DocumentSymbol, DocumentSymbolParams,
    DocumentSymbolResponse, GotoDefinitionParams, Hover, HoverContents, HoverParams,
    InitializeParams, InitializeResult, Location, MarkupContent, MarkupKind, NumberOrString, OneOf,
    OptionalVersionedTextDocumentIdentifier, PositionEncodingKind, PublishDiagnosticsParams,
    ReferenceParams, RenameOptions, RenameParams, SemanticTokensParams, SemanticTokensRangeParams,
    ServerCapabilities, ServerInfo, SignatureHelpParams, SymbolInformation, SymbolKind,
    TextDocumentEdit, TextDocumentPositionParams, TextDocumentSyncKind, TextDocumentSyncOptions,
    TextEdit, Uri, WorkspaceEdit,
    notification::{self, Notification as _},
    request::{self, Request as _},
};
use oxel::{AssemblyKind, ProjectAnalyzer, References, Symbol};
use serde_json::{Value, to_value};
use url::Url;

use self::document::Document;

type ServerResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

pub(super) fn run(
    references: &References,
    sources: &[PathBuf],
    kind: AssemblyKind,
) -> ServerResult {
    let mut documents = HashMap::new();
    let mut workspace = Vec::new();
    let mut seen = HashSet::new();
    for path in sources {
        let absolute = path::absolute(path)?;
        let url = Url::from_file_path(&absolute)
            .map_err(|_| format!("invalid source path {}", path.display()))?;
        let uri: Uri = url.as_str().parse()?;
        let resolved = absolute.canonicalize().map_err(|error| {
            format!(
                "could not resolve project source {}: {error}",
                path.display()
            )
        })?;
        if !seen.insert(resolved) {
            return Err(format!("duplicate project source {}", path.display()).into());
        }
        let text = fs::read_to_string(&absolute).map_err(|error| {
            format!("could not read project source {}: {error}", path.display())
        })?;
        documents.insert(uri.as_str().to_owned(), Document::new(text, None));
        workspace.push((uri, absolute));
    }
    let (connection, threads) = Connection::stdio();
    let (id, params) = connection.initialize_start()?;
    let params: InitializeParams = serde_json::from_value(params)?;
    let editor = params.capabilities.text_document.as_ref();
    let hierarchy = editor
        .and_then(|capabilities| capabilities.document_symbol.as_ref())
        .and_then(|capabilities| capabilities.hierarchical_document_symbol_support)
        .unwrap_or(false);
    let code_action_provider = editor
        .and_then(|capabilities| capabilities.code_action.as_ref())
        .and_then(|capabilities| capabilities.code_action_literal_support.as_ref())
        .map(|_| code_actions::capability());
    let diagnostic_related_information = editor
        .and_then(|capabilities| capabilities.publish_diagnostics.as_ref())
        .and_then(|capabilities| capabilities.related_information)
        .unwrap_or(false);
    let signature_label_offsets = editor
        .and_then(|capabilities| capabilities.signature_help.as_ref())
        .and_then(|capabilities| capabilities.signature_information.as_ref())
        .and_then(|capabilities| capabilities.parameter_information.as_ref())
        .and_then(|capabilities| capabilities.label_offset_support)
        .unwrap_or(false);
    let workspace_capabilities = params.capabilities.workspace.as_ref();
    let versioned_edits = workspace_capabilities
        .and_then(|capabilities| capabilities.workspace_edit.as_ref())
        .and_then(|capabilities| capabilities.document_changes)
        .unwrap_or(false);
    let capabilities = ServerCapabilities {
        position_encoding: Some(PositionEncodingKind::UTF16),
        text_document_sync: Some(
            TextDocumentSyncOptions {
                open_close: Some(true),
                change: Some(TextDocumentSyncKind::INCREMENTAL),
                ..Default::default()
            }
            .into(),
        ),
        document_symbol_provider: Some(OneOf::Left(true)),
        definition_provider: Some(OneOf::Left(true)),
        code_action_provider,
        references_provider: Some(OneOf::Left(true)),
        semantic_tokens_provider: Some(semantic_tokens::capabilities()),
        rename_provider: Some(OneOf::Right(RenameOptions {
            prepare_provider: Some(true),
            work_done_progress_options: Default::default(),
        })),
        hover_provider: Some(lsp_types::HoverProviderCapability::Simple(true)),
        signature_help_provider: Some(signature_help::capabilities()),
        completion_provider: Some(CompletionOptions::default()),
        ..Default::default()
    };
    let result = InitializeResult {
        capabilities,
        server_info: Some(ServerInfo {
            name: "oxel".into(),
            version: Some(env!("CARGO_PKG_VERSION").into()),
        }),
    };
    connection.initialize_finish(id, to_value(result)?)?;
    let watch_registration = if let Some(capabilities) =
        workspace_capabilities.and_then(|capabilities| capabilities.did_change_watched_files)
    {
        watched_files::register(
            &connection,
            capabilities,
            workspace.iter().map(|(_, path)| path.as_path()),
        )?
    } else {
        None
    };
    let mut session = Session {
        references,
        documents,
        workspace,
        kind,
        analyzer: ProjectAnalyzer::new(kind),
        standalone: HashMap::new(),
        hierarchy,
        versioned_edits,
        diagnostic_related_information,
        signature_label_offsets,
    };
    session.analyze_project();
    session.serve(&connection, watch_registration)?;
    drop(connection);
    threads.join()?;
    Ok(())
}

struct Session<'a> {
    references: &'a References,
    documents: HashMap<String, Document>,
    workspace: Vec<(Uri, PathBuf)>,
    kind: AssemblyKind,
    analyzer: ProjectAnalyzer,
    standalone: HashMap<String, ProjectAnalyzer>,
    hierarchy: bool,
    versioned_edits: bool,
    diagnostic_related_information: bool,
    signature_label_offsets: bool,
}

impl Session<'_> {
    fn is_project_file(&self, uri: &Uri) -> bool {
        self.workspace.iter().any(|(file, _)| file == uri)
    }

    fn workspace_edit(
        &self,
        edits: impl IntoIterator<Item = (Uri, Option<i32>, Vec<TextEdit>)>,
    ) -> WorkspaceEdit {
        let edits = edits.into_iter();
        if self.versioned_edits {
            WorkspaceEdit {
                document_changes: Some(DocumentChanges::Edits(
                    edits
                        .map(|(uri, version, edits)| TextDocumentEdit {
                            text_document: OptionalVersionedTextDocumentIdentifier { uri, version },
                            edits: edits.into_iter().map(OneOf::Left).collect(),
                        })
                        .collect(),
                )),
                ..Default::default()
            }
        } else {
            WorkspaceEdit {
                changes: Some(edits.map(|(uri, _, edits)| (uri, edits)).collect()),
                ..Default::default()
            }
        }
    }

    fn adopt_project_uri(&mut self, uri: &Uri) {
        if let Some(index) = self.project_file_index(uri)
            && self.workspace[index].0 != *uri
        {
            let (key, _) = &mut self.workspace[index];
            self.documents.remove(key.as_str());
            *key = uri.clone();
        }
    }

    fn project_file_index(&self, uri: &Uri) -> Option<usize> {
        if let Some(index) = self.workspace.iter().position(|(file, _)| file == uri) {
            return Some(index);
        }
        let file = Url::parse(uri.as_str())
            .ok()
            .filter(|url| url.scheme() == "file")
            .and_then(|url| url.to_file_path().ok())?;
        if let Some(index) = self.workspace.iter().position(|(_, path)| *path == file) {
            return Some(index);
        }
        let resolved = file.canonicalize().ok()?;
        self.workspace
            .iter()
            .position(|(_, path)| path.canonicalize().ok().as_ref() == Some(&resolved))
    }

    fn update_closed_files(
        &mut self,
        connection: &Connection,
        params: DidChangeWatchedFilesParams,
    ) -> ServerResult {
        let files: HashSet<_> = params
            .changes
            .iter()
            .filter_map(|event| self.project_file_index(&event.uri))
            .collect();
        self.reload_closed_files(connection, files)
    }

    fn reload_closed_files(
        &mut self,
        connection: &Connection,
        files: impl IntoIterator<Item = usize>,
    ) -> ServerResult {
        let mut changed = false;
        for index in files {
            let (uri, path) = &self.workspace[index];
            let document = self
                .documents
                .get_mut(uri.as_str())
                .expect("project document");
            if document.version.is_none() {
                let text = read_closed_source(path);
                if text != *document.text {
                    *document = Document::new(text, None);
                    changed = true;
                }
            }
        }
        if changed {
            self.refresh_project(connection)?;
        }
        Ok(())
    }

    fn analyze_project(&mut self) {
        let sources: Vec<_> = self
            .workspace
            .iter()
            .map(|(uri, _)| Arc::clone(&self.documents[uri.as_str()].text))
            .collect();
        let analyses = self.analyzer.inspect(&sources, self.references);
        for ((uri, _), analysis) in self.workspace.iter().zip(analyses) {
            self.documents
                .get_mut(uri.as_str())
                .expect("project document")
                .analysis = analysis;
        }
    }

    fn refresh_project(&mut self, connection: &Connection) -> ServerResult {
        self.analyze_project();
        for (file, _) in &self.workspace {
            let document = &self.documents[file.as_str()];
            if document.version.is_some() {
                self.publish(connection, file.clone(), Some(document))?;
            }
        }
        Ok(())
    }

    fn refresh(&mut self, connection: &Connection, uri: &Uri) -> ServerResult {
        if self.is_project_file(uri) {
            self.refresh_project(connection)?;
        } else if let Some(document) = self.documents.get_mut(uri.as_str()) {
            let analyzer = self.standalone.entry(uri.as_str().to_owned()).or_default();
            document.analysis = analyzer
                .inspect(std::slice::from_ref(&document.text), self.references)
                .pop()
                .expect("standalone analysis");
            self.publish(connection, uri.clone(), self.documents.get(uri.as_str()))?;
        }
        Ok(())
    }

    fn serve(
        &mut self,
        connection: &Connection,
        mut watch_registration: Option<RequestId>,
    ) -> ServerResult {
        let mut shutdown = false;
        for message in &connection.receiver {
            match message {
                Message::Notification(event) if event.method == notification::Exit::METHOD => {
                    return if shutdown {
                        Ok(())
                    } else {
                        Err("LSP exit received before shutdown".into())
                    };
                }
                Message::Request(request) => {
                    let response = if shutdown {
                        Response::new_err(
                            request.id,
                            ErrorCode::InvalidRequest as i32,
                            "server is shutting down".into(),
                        )
                    } else if request.method == request::Shutdown::METHOD {
                        shutdown = true;
                        Response::new_ok(request.id, Value::Null)
                    } else {
                        self.request(request)?
                    };
                    connection.sender.send(response.into())?;
                }
                Message::Notification(event) if !shutdown => {
                    self.notification(connection, event)?
                }
                Message::Response(response)
                    if !shutdown && watch_registration.as_ref() == Some(&response.id) =>
                {
                    watch_registration = None;
                    if response.error.is_none() {
                        // Catch edits between the initial disk read and watcher activation.
                        self.reload_closed_files(connection, 0..self.workspace.len())?;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn request(&self, request: Request) -> ServerResult<Response> {
        let result = match request.method.as_str() {
            request::DocumentSymbolRequest::METHOD => {
                let params: DocumentSymbolParams = match serde_json::from_value(request.params) {
                    Ok(params) => params,
                    Err(error) => {
                        return Ok(Response::new_err(
                            request.id,
                            ErrorCode::InvalidParams as i32,
                            error.to_string(),
                        ));
                    }
                };
                to_value(self.symbols(&params.text_document.uri))?
            }
            request::HoverRequest::METHOD => {
                let params: HoverParams = match serde_json::from_value(request.params) {
                    Ok(params) => params,
                    Err(error) => {
                        return Ok(Response::new_err(
                            request.id,
                            ErrorCode::InvalidParams as i32,
                            error.to_string(),
                        ));
                    }
                };
                let point = params.text_document_position_params;
                let hover = self
                    .documents
                    .get(point.text_document.uri.as_str())
                    .and_then(|document| {
                        let item = document.occurrence(point.position)?;
                        Some(Hover {
                            contents: HoverContents::Markup(MarkupContent {
                                kind: MarkupKind::PlainText,
                                value: item.detail.clone(),
                            }),
                            range: Some(document.range(item.span)),
                        })
                    });
                to_value(hover)?
            }
            request::GotoDefinition::METHOD => {
                let params: GotoDefinitionParams = match serde_json::from_value(request.params) {
                    Ok(params) => params,
                    Err(error) => {
                        return Ok(Response::new_err(
                            request.id,
                            ErrorCode::InvalidParams as i32,
                            error.to_string(),
                        ));
                    }
                };
                let point = params.text_document_position_params;
                let uri = point.text_document.uri;
                let definition = self.documents.get(uri.as_str()).and_then(|document| {
                    let item = document.occurrence(point.position)?;
                    let target_uri = if self.is_project_file(&uri) {
                        &self.workspace.get(item.definition_source?)?.0
                    } else {
                        &uri
                    };
                    let target = self.documents.get(target_uri.as_str())?;
                    Some(Location::new(
                        target_uri.clone(),
                        target.range(item.definition?),
                    ))
                });
                to_value(definition)?
            }
            request::SignatureHelpRequest::METHOD => {
                let params: SignatureHelpParams = match serde_json::from_value(request.params) {
                    Ok(params) => params,
                    Err(error) => {
                        return Ok(Response::new_err(
                            request.id,
                            ErrorCode::InvalidParams as i32,
                            error.to_string(),
                        ));
                    }
                };
                to_value(self.signature_help(params.text_document_position_params))?
            }
            request::References::METHOD => {
                let params: ReferenceParams = match serde_json::from_value(request.params) {
                    Ok(params) => params,
                    Err(error) => {
                        return Ok(Response::new_err(
                            request.id,
                            ErrorCode::InvalidParams as i32,
                            error.to_string(),
                        ));
                    }
                };
                to_value(self.find_references(params))?
            }
            request::PrepareRenameRequest::METHOD => {
                let params: TextDocumentPositionParams =
                    match serde_json::from_value(request.params) {
                        Ok(params) => params,
                        Err(error) => {
                            return Ok(Response::new_err(
                                request.id,
                                ErrorCode::InvalidParams as i32,
                                error.to_string(),
                            ));
                        }
                    };
                to_value(self.prepare_rename(&params))?
            }
            request::Rename::METHOD => {
                let params: RenameParams = match serde_json::from_value(request.params) {
                    Ok(params) => params,
                    Err(error) => {
                        return Ok(Response::new_err(
                            request.id,
                            ErrorCode::InvalidParams as i32,
                            error.to_string(),
                        ));
                    }
                };
                match self.rename(&params) {
                    Ok(edits) => to_value(edits)?,
                    Err((code, message)) => {
                        return Ok(Response::new_err(request.id, code as i32, message));
                    }
                }
            }
            request::SemanticTokensFullRequest::METHOD => {
                let params: SemanticTokensParams = match serde_json::from_value(request.params) {
                    Ok(params) => params,
                    Err(error) => {
                        return Ok(Response::new_err(
                            request.id,
                            ErrorCode::InvalidParams as i32,
                            error.to_string(),
                        ));
                    }
                };
                to_value(
                    self.documents
                        .get(params.text_document.uri.as_str())
                        .map(|document| semantic_tokens::collect(document, None)),
                )?
            }
            request::SemanticTokensRangeRequest::METHOD => {
                let params: SemanticTokensRangeParams = match serde_json::from_value(request.params)
                {
                    Ok(params) => params,
                    Err(error) => {
                        return Ok(Response::new_err(
                            request.id,
                            ErrorCode::InvalidParams as i32,
                            error.to_string(),
                        ));
                    }
                };
                to_value(
                    self.documents
                        .get(params.text_document.uri.as_str())
                        .map(|document| semantic_tokens::collect(document, Some(params.range))),
                )?
            }
            request::CodeActionRequest::METHOD => {
                let params: CodeActionParams = match serde_json::from_value(request.params) {
                    Ok(params) => params,
                    Err(error) => {
                        return Ok(Response::new_err(
                            request.id,
                            ErrorCode::InvalidParams as i32,
                            error.to_string(),
                        ));
                    }
                };
                to_value(self.code_actions(params))?
            }
            request::Completion::METHOD => {
                let params: CompletionParams = match serde_json::from_value(request.params) {
                    Ok(params) => params,
                    Err(error) => {
                        return Ok(Response::new_err(
                            request.id,
                            ErrorCode::InvalidParams as i32,
                            error.to_string(),
                        ));
                    }
                };
                to_value(self.complete(&params.text_document_position))?
            }
            _ => {
                return Ok(Response::new_err(
                    request.id,
                    ErrorCode::MethodNotFound as i32,
                    format!("unknown method '{}'", request.method),
                ));
            }
        };
        Ok(Response::new_ok(request.id, result))
    }

    fn notification(&mut self, connection: &Connection, event: Notification) -> ServerResult {
        match event.method.as_str() {
            notification::DidChangeWatchedFiles::METHOD => {
                if let Ok(params) = serde_json::from_value(event.params) {
                    self.update_closed_files(connection, params)?;
                }
            }
            notification::DidOpenTextDocument::METHOD => {
                if let Ok(params) =
                    serde_json::from_value::<DidOpenTextDocumentParams>(event.params)
                {
                    let item = params.text_document;
                    self.adopt_project_uri(&item.uri);
                    self.documents.insert(
                        item.uri.as_str().to_owned(),
                        Document::new(item.text, Some(item.version)),
                    );
                    self.refresh(connection, &item.uri)?;
                }
            }
            notification::DidChangeTextDocument::METHOD => {
                if let Ok(params) =
                    serde_json::from_value::<DidChangeTextDocumentParams>(event.params)
                    && let Some(document) =
                        self.documents.get_mut(params.text_document.uri.as_str())
                    && let Some(changed) =
                        document.change(params.text_document.version, params.content_changes)
                {
                    if changed {
                        self.refresh(connection, &params.text_document.uri)?;
                    } else {
                        let uri = params.text_document.uri;
                        self.publish(connection, uri.clone(), self.documents.get(uri.as_str()))?;
                    }
                }
            }
            notification::DidCloseTextDocument::METHOD => {
                if let Ok(params) =
                    serde_json::from_value::<DidCloseTextDocumentParams>(event.params)
                {
                    let uri = params.text_document.uri;
                    if self
                        .documents
                        .get(uri.as_str())
                        .is_some_and(|document| document.version.is_some())
                    {
                        self.publish(connection, uri.clone(), None)?;
                        if let Some((_, path)) =
                            self.workspace.iter().find(|(file, _)| file == &uri)
                        {
                            let text = read_closed_source(path);
                            self.documents
                                .insert(uri.as_str().to_owned(), Document::new(text, None));
                            self.refresh(connection, &uri)?;
                        } else {
                            self.documents.remove(uri.as_str());
                            self.standalone.remove(uri.as_str());
                        }
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn find_references(&self, params: ReferenceParams) -> Option<Vec<Location>> {
        let symbol = self.symbol_occurrences(&params.text_document_position)?;
        Some(
            symbol
                .matching(params.context.include_declaration)
                .map(|(file, document, item)| {
                    Location::new(file.clone(), document.range(item.span))
                })
                .collect(),
        )
    }

    fn symbols(&self, uri: &Uri) -> Option<DocumentSymbolResponse> {
        let document = self.documents.get(uri.as_str())?;
        let symbols = &document.analysis.symbols;
        Some(if self.hierarchy {
            DocumentSymbolResponse::Nested(
                symbols
                    .iter()
                    .map(|symbol| nested(document, symbol))
                    .collect(),
            )
        } else {
            let mut flat = Vec::new();
            for symbol in symbols {
                flatten(document, uri, symbol, None, &mut flat);
            }
            DocumentSymbolResponse::Flat(flat)
        })
    }

    fn complete(&self, params: &TextDocumentPositionParams) -> Option<CompletionResponse> {
        let uri = &params.text_document.uri;
        let document = self.documents.get(uri.as_str())?;
        let cursor = document.offset(params.position)?;
        let occurrences = &document.analysis.occurrences;
        let preceding = occurrences.partition_point(|item| item.span.start < cursor);
        let mut bindings = HashSet::new();
        let mut items = Vec::new();
        // A later declaration in an active scope shadows an earlier one.
        for occurrence in occurrences[..preceding].iter().rev() {
            let Some(scope) = occurrence.visibility else {
                continue;
            };
            if !(scope.start..=scope.end).contains(&cursor) {
                continue;
            }
            let name = &document.text[occurrence.span.start..occurrence.span.end];
            if bindings.insert(name) {
                items.push(CompletionItem {
                    label: name.into(),
                    kind: Some(CompletionItemKind::VARIABLE),
                    detail: Some(occurrence.detail.clone()),
                    ..Default::default()
                });
            }
        }
        let symbols: Vec<_> = if self.is_project_file(uri) {
            self.workspace
                .iter()
                .flat_map(|(file, _)| &self.documents[file.as_str()].analysis.symbols)
                .collect()
        } else {
            document.analysis.symbols.iter().collect()
        };
        items.extend(
            symbols
                .into_iter()
                .filter(|symbol| {
                    symbol.kind != oxel::SymbolKind::Constant
                        || !bindings.contains(symbol.name.as_str())
                })
                .map(|symbol| {
                    let kind = match symbol.kind {
                        oxel::SymbolKind::Function => CompletionItemKind::FUNCTION,
                        oxel::SymbolKind::Constant => CompletionItemKind::CONSTANT,
                        oxel::SymbolKind::Struct => CompletionItemKind::STRUCT,
                        oxel::SymbolKind::Enum => CompletionItemKind::ENUM,
                        oxel::SymbolKind::EnumMember => CompletionItemKind::ENUM_MEMBER,
                        oxel::SymbolKind::Field => CompletionItemKind::FIELD,
                    };
                    CompletionItem {
                        label: symbol.name.clone(),
                        kind: Some(kind),
                        ..Default::default()
                    }
                }),
        );
        for keyword in [
            "fn", "comptime", "extern", "internal", "struct", "enum", "match", "const", "new",
            "let", "mut", "if", "else", "while", "for", "in", "break", "continue", "return",
            "throw", "try", "catch", "using", "true", "false", "as", "is", "null",
        ] {
            items.push(CompletionItem {
                label: keyword.into(),
                kind: Some(CompletionItemKind::KEYWORD),
                ..Default::default()
            });
        }
        Some(CompletionResponse::Array(items))
    }
}

fn read_closed_source(path: &Path) -> String {
    match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) => {
            if error.kind() != std::io::ErrorKind::NotFound {
                eprintln!("oxel lsp: could not reload {}: {error}", path.display());
            }
            String::new()
        }
    }
}

impl Session<'_> {
    fn diagnostic(&self, uri: &Uri, document: &Document, error: &oxel::Diagnostic) -> Diagnostic {
        let related_information = self.related_information(uri, error).map(|item| vec![item]);
        Diagnostic {
            range: document.range(error.span),
            severity: Some(DiagnosticSeverity::ERROR),
            code: Some(NumberOrString::String(error.code.into())),
            source: Some("oxel".into()),
            message: error.message.clone(),
            related_information,
            data: error.type_names().map(|(expected, actual)| {
                serde_json::json!({
                    "expected": expected,
                    "actual": actual,
                })
            }),
            ..Default::default()
        }
    }

    fn related_information(
        &self,
        uri: &Uri,
        error: &oxel::Diagnostic,
    ) -> Option<DiagnosticRelatedInformation> {
        if !self.diagnostic_related_information {
            return None;
        }
        let related = error.related()?;
        let uri = match related.source {
            Some(index) if self.is_project_file(uri) => &self.workspace.get(index)?.0,
            _ => uri,
        };
        let target = self.documents.get(uri.as_str())?;
        Some(DiagnosticRelatedInformation {
            location: Location::new(uri.clone(), target.range(related.span)),
            message: related.message.clone(),
        })
    }

    fn publish(
        &self,
        connection: &Connection,
        uri: Uri,
        document: Option<&Document>,
    ) -> ServerResult {
        let diagnostics = document.map_or_else(Vec::new, |document| {
            document
                .analysis
                .diagnostics
                .iter()
                .map(|error| self.diagnostic(&uri, document, error))
                .collect()
        });
        let params = PublishDiagnosticsParams::new(
            uri,
            diagnostics,
            document.and_then(|document| document.version),
        );
        connection.sender.send(
            Notification::new(notification::PublishDiagnostics::METHOD.into(), params).into(),
        )?;
        Ok(())
    }
}

fn symbol_kind(symbol: &Symbol) -> SymbolKind {
    match symbol.kind {
        oxel::SymbolKind::Function => SymbolKind::FUNCTION,
        oxel::SymbolKind::Constant => SymbolKind::CONSTANT,
        oxel::SymbolKind::Struct => SymbolKind::STRUCT,
        oxel::SymbolKind::Enum => SymbolKind::ENUM,
        oxel::SymbolKind::EnumMember => SymbolKind::ENUM_MEMBER,
        oxel::SymbolKind::Field => SymbolKind::FIELD,
    }
}

#[allow(deprecated)]
fn nested(document: &Document, symbol: &Symbol) -> DocumentSymbol {
    DocumentSymbol {
        name: symbol.name.clone(),
        detail: None,
        kind: symbol_kind(symbol),
        tags: None,
        deprecated: None,
        range: document.range(symbol.span),
        selection_range: document.range(symbol.name_span),
        children: (!symbol.children.is_empty()).then(|| {
            symbol
                .children
                .iter()
                .map(|child| nested(document, child))
                .collect()
        }),
    }
}

#[allow(deprecated)]
fn flatten(
    document: &Document,
    uri: &Uri,
    symbol: &Symbol,
    container: Option<&str>,
    result: &mut Vec<SymbolInformation>,
) {
    result.push(SymbolInformation {
        name: symbol.name.clone(),
        kind: symbol_kind(symbol),
        tags: None,
        deprecated: None,
        location: Location::new(uri.clone(), document.range(symbol.name_span)),
        container_name: container.map(str::to_owned),
    });
    for child in &symbol.children {
        flatten(document, uri, child, Some(&symbol.name), result);
    }
}
