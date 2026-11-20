use lsp_types::{
    ParameterInformation, ParameterLabel, SignatureHelp, SignatureHelpOptions,
    SignatureInformation, TextDocumentPositionParams,
};
use oxel::{
    CallableSignature, Span, SymbolKind, call_site, clr_function_signatures,
    declared_function_signature,
};

use super::{Document, Session};

pub(super) fn capabilities() -> SignatureHelpOptions {
    SignatureHelpOptions {
        trigger_characters: Some(vec!["(".into(), ",".into()]),
        ..Default::default()
    }
}

impl Session<'_> {
    pub(super) fn signature_help(
        &self,
        point: TextDocumentPositionParams,
    ) -> Option<SignatureHelp> {
        let uri = point.text_document.uri;
        let document = self.documents.get(uri.as_str())?;
        let offset = document.offset(point.position)?;
        let symbols = &document.analysis.symbols;
        let declaration = symbols[..symbols.partition_point(|item| item.span.start <= offset)]
            .last()
            .filter(|item| offset < item.span.end)
            .map(|item| item.span);
        let site = call_site(&document.text, offset, declaration)?;
        let selected = [site.member, site.callee].into_iter().find_map(|span| {
            let occurrences = &document.analysis.occurrences;
            let start = occurrences.partition_point(|item| item.span.start < span.start);
            occurrences[start..]
                .iter()
                .take_while(|item| item.span.start == span.start)
                .filter(|item| item.span == span)
                .find_map(|item| {
                    let parameters = item.call_parameters.clone()?;
                    Some(CallableSignature {
                        label: item.detail.clone(),
                        parameters,
                    })
                })
        });
        let signatures = match (selected, site.named_path.as_deref()) {
            (Some(signature), _) if !signature.parameters.ranges.is_empty() => vec![signature],
            (selected, Some(name)) if name.contains('.') => {
                let mut signatures = clr_function_signatures(name, self.references);
                if signatures.is_empty() {
                    signatures.extend(selected);
                }
                signatures
            }
            (None, Some(name)) => {
                let declaration = if self.is_project_file(&uri) {
                    self.workspace.iter().find_map(|(file, _)| {
                        find_declaration(self.documents.get(file.as_str())?, name)
                    })
                } else {
                    find_declaration(document, name)
                };
                let signature = declaration
                    .and_then(|(source, span)| {
                        declared_function_signature(source, name, Some(span))
                    })
                    .or_else(|| declared_function_signature(&document.text, name, None));
                vec![signature?]
            }
            (selected, _) => selected.into_iter().collect(),
        };
        let active = signatures
            .iter()
            .position(|signature| {
                signature.parameters.variadic || site.argument < signature.parameters.ranges.len()
            })
            .unwrap_or(0);
        let parameters = &signatures.get(active)?.parameters;
        let parameter = if parameters.variadic {
            site.argument.min(parameters.ranges.len().saturating_sub(1))
        } else {
            site.argument
        };
        let active_parameter = (parameter < parameters.ranges.len()).then_some(parameter as u32);
        Some(SignatureHelp {
            signatures: signatures
                .into_iter()
                .map(|signature| signature_information(signature, self.signature_label_offsets))
                .collect(),
            active_signature: Some(active as u32),
            active_parameter,
        })
    }
}

fn find_declaration<'a>(document: &'a Document, name: &str) -> Option<(&'a str, Span)> {
    let symbol = document
        .analysis
        .symbols
        .iter()
        .find(|item| item.kind == SymbolKind::Function && item.name == name)?;
    Some((&document.text, symbol.span))
}

fn signature_information(signature: CallableSignature, offsets: bool) -> SignatureInformation {
    let parameters = signature
        .parameters
        .ranges
        .iter()
        .map(|span| {
            let parameter = &signature.label[span.start..span.end];
            let label = if offsets {
                let start = signature.label[..span.start].encode_utf16().count();
                let end = start + parameter.encode_utf16().count();
                ParameterLabel::LabelOffsets([start as u32, end as u32])
            } else {
                ParameterLabel::Simple(parameter.into())
            };
            ParameterInformation {
                label,
                documentation: None,
            }
        })
        .collect();
    SignatureInformation {
        label: signature.label,
        documentation: None,
        parameters: Some(parameters),
        active_parameter: None,
    }
}
