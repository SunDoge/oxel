use lsp_types::{
    Position, Range, SemanticToken, SemanticTokenModifier, SemanticTokenType, SemanticTokens,
    SemanticTokensFullOptions, SemanticTokensLegend, SemanticTokensOptions,
    SemanticTokensServerCapabilities,
};
use oxel::{Occurrence, OccurrenceKind, Span, Symbol, SymbolKind};

use super::document::Document;

const TYPES: &[SemanticTokenType] = &[
    SemanticTokenType::FUNCTION,
    SemanticTokenType::METHOD,
    SemanticTokenType::VARIABLE,
    SemanticTokenType::PARAMETER,
    SemanticTokenType::TYPE,
    SemanticTokenType::TYPE_PARAMETER,
    SemanticTokenType::STRUCT,
    SemanticTokenType::ENUM,
    SemanticTokenType::ENUM_MEMBER,
    SemanticTokenType::CLASS,
    SemanticTokenType::INTERFACE,
    SemanticTokenType::PROPERTY,
    SemanticTokenType::OPERATOR,
    SemanticTokenType::EVENT,
];

const MODIFIERS: &[SemanticTokenModifier] = &[
    SemanticTokenModifier::DECLARATION,
    SemanticTokenModifier::READONLY,
    SemanticTokenModifier::DEFAULT_LIBRARY,
];
const DECLARATION: u32 = 1 << 0;
const READONLY: u32 = 1 << 1;
const DEFAULT_LIBRARY: u32 = 1 << 2;

pub(super) fn capabilities() -> SemanticTokensServerCapabilities {
    SemanticTokensOptions {
        legend: SemanticTokensLegend {
            token_types: TYPES.to_vec(),
            token_modifiers: MODIFIERS.to_vec(),
        },
        full: Some(SemanticTokensFullOptions::Bool(true)),
        range: Some(true),
        ..Default::default()
    }
    .into()
}

struct Token {
    span: Span,
    kind: SemanticTokenType,
    modifiers: u32,
}

impl Token {
    fn from_occurrence(text: &str, item: &Occurrence) -> Option<Self> {
        let (kind, modifiers) = match item.kind {
            OccurrenceKind::Function => (SemanticTokenType::FUNCTION, 0),
            OccurrenceKind::Method => (SemanticTokenType::METHOD, 0),
            OccurrenceKind::Constant => (SemanticTokenType::VARIABLE, READONLY),
            OccurrenceKind::Parameter => (SemanticTokenType::PARAMETER, READONLY),
            OccurrenceKind::Local { mutable } => (
                SemanticTokenType::VARIABLE,
                if mutable { 0 } else { READONLY },
            ),
            OccurrenceKind::BuiltinType => (SemanticTokenType::TYPE, DEFAULT_LIBRARY),
            OccurrenceKind::TypeParameter => (SemanticTokenType::TYPE_PARAMETER, 0),
            OccurrenceKind::Struct => (SemanticTokenType::STRUCT, 0),
            OccurrenceKind::Enum => (SemanticTokenType::ENUM, 0),
            OccurrenceKind::EnumMember => (SemanticTokenType::ENUM_MEMBER, 0),
            OccurrenceKind::Class => (SemanticTokenType::CLASS, 0),
            OccurrenceKind::Interface => (SemanticTokenType::INTERFACE, 0),
            OccurrenceKind::Field { readonly } => (
                SemanticTokenType::PROPERTY,
                if readonly { READONLY } else { 0 },
            ),
            OccurrenceKind::Property => (SemanticTokenType::PROPERTY, 0),
            OccurrenceKind::Event => (SemanticTokenType::EVENT, 0),
            OccurrenceKind::Operator => (SemanticTokenType::OPERATOR, 0),
            OccurrenceKind::Constructor | OccurrenceKind::ShorthandField => return None,
        };
        let mut span = item.span;
        if item.kind != OccurrenceKind::Operator {
            // Hover can cover the entire qualified name. Color its final identifier.
            let length = text
                .get(span.start..span.end)?
                .bytes()
                .rev()
                .take_while(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
                .count();
            if length == 0 {
                return None;
            }
            span.start = span.end - length;
        }
        let declaration = if item.is_declaration { DECLARATION } else { 0 };
        Some(Self {
            span,
            kind,
            modifiers: modifiers | declaration,
        })
    }

    fn add_outline(symbol: &Symbol, tokens: &mut Vec<Self>) {
        let (kind, modifiers) = match symbol.kind {
            SymbolKind::Function => (SemanticTokenType::FUNCTION, 0),
            SymbolKind::Constant => (SemanticTokenType::VARIABLE, READONLY),
            SymbolKind::Struct => (SemanticTokenType::STRUCT, 0),
            SymbolKind::Enum => (SemanticTokenType::ENUM, 0),
            SymbolKind::EnumMember => (SemanticTokenType::ENUM_MEMBER, 0),
            SymbolKind::Field => (SemanticTokenType::PROPERTY, READONLY),
        };
        tokens.push(Self {
            span: symbol.name_span,
            kind,
            modifiers: modifiers | DECLARATION,
        });
        for child in &symbol.children {
            Self::add_outline(child, tokens);
        }
    }
}

pub(super) fn collect(document: &Document, range: Option<Range>) -> SemanticTokens {
    let mut data = Vec::new();
    if range.is_some_and(|range| range.start >= range.end) {
        return SemanticTokens {
            result_id: None,
            data,
        };
    }
    let analysis = &document.analysis;
    let mut tokens: Vec<_> = analysis
        .occurrences
        .iter()
        .filter_map(|item| Token::from_occurrence(&document.text, item))
        .collect();
    // The parser can still recover these declarations when a syntax error
    // prevents semantic analysis of a partially edited document.
    for symbol in &analysis.symbols {
        Token::add_outline(symbol, &mut tokens);
    }
    tokens.sort_by_key(|token| (token.span.start, token.span.end));
    let mut previous = Position::default();
    let mut previous_end = (0, Position::default());
    for token in tokens {
        if token.span.start < previous_end.0 {
            continue;
        }
        let position = document.range_after(token.span, previous_end.0, previous_end.1);
        if position.start.line != position.end.line || position.start == position.end {
            continue;
        }
        previous_end = (token.span.end, position.end);
        if range.is_some_and(|range| position.start >= range.end || position.end <= range.start) {
            continue;
        }
        let delta_line = position.start.line - previous.line;
        let delta_start = if delta_line == 0 {
            position.start.character - previous.character
        } else {
            position.start.character
        };
        let token_type = TYPES
            .iter()
            .position(|kind| *kind == token.kind)
            .expect("semantic token type is in the legend") as u32;
        data.push(SemanticToken {
            delta_line,
            delta_start,
            length: position.end.character - position.start.character,
            token_type,
            token_modifiers_bitset: token.modifiers,
        });
        previous = position.start;
    }
    SemanticTokens {
        result_id: None,
        data,
    }
}
