use lsp_types::{
    CodeAction, CodeActionKind, CodeActionOptions, CodeActionParams, CodeActionProviderCapability,
    CodeActionResponse, Range, TextEdit,
};

use super::Session;

pub(super) fn capability() -> CodeActionProviderCapability {
    CodeActionProviderCapability::Options(CodeActionOptions {
        code_action_kinds: Some(vec![CodeActionKind::QUICKFIX]),
        ..Default::default()
    })
}

impl Session<'_> {
    pub(super) fn code_actions(&self, params: CodeActionParams) -> CodeActionResponse {
        if params.range.start > params.range.end
            || params.context.only.as_ref().is_some_and(|kinds| {
                !kinds
                    .iter()
                    .any(|kind| *kind == CodeActionKind::EMPTY || *kind == CodeActionKind::QUICKFIX)
            })
        {
            return Vec::new();
        }
        let uri = params.text_document.uri;
        let Some(document) = self.documents.get(uri.as_str()) else {
            return Vec::new();
        };
        // Open buffers are versioned. A closed file may have changed since
        // the module was last read, so check it before proposing byte edits.
        if document.version.is_none()
            && !self
                .workspace
                .iter()
                .any(|(file, path)| file == &uri && document.matches_file(path))
        {
            return Vec::new();
        }
        document
            .analysis
            .diagnostics
            .iter()
            .filter_map(|error| {
                let fix = error.fix()?;
                let diagnostic = self.diagnostic(&uri, document, error);
                if !intersects(params.range, diagnostic.range) {
                    return None;
                }
                let edit = TextEdit {
                    range: document.range(fix.span),
                    new_text: fix.replacement.to_owned(),
                };
                Some(
                    CodeAction {
                        title: fix.title.to_owned(),
                        kind: Some(CodeActionKind::QUICKFIX),
                        diagnostics: Some(vec![diagnostic]),
                        edit: Some(self.workspace_edit([(
                            uri.clone(),
                            document.version,
                            vec![edit],
                        )])),
                        ..Default::default()
                    }
                    .into(),
                )
            })
            .collect()
    }
}

fn intersects(selection: Range, diagnostic: Range) -> bool {
    if selection.start == selection.end || diagnostic.start == diagnostic.end {
        selection.start <= diagnostic.end && diagnostic.start <= selection.end
    } else {
        selection.start < diagnostic.end && diagnostic.start < selection.end
    }
}
