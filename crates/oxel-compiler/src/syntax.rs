use crate::{ArgumentMode, CallableSignature, Diagnostic, DiagnosticFix, Span};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Fn,
    Comptime,
    Async,
    Await,
    Extern,
    New,
    As,
    Is,
    Null,
    Question,
    Coalesce,
    Struct,
    Enum,
    Match,
    Const,
    Let,
    Mut,
    Return,
    Throw,
    Try,
    Catch,
    Using,
    If,
    Else,
    While,
    For,
    Break,
    Continue,
    True,
    False,
    Name,
    Integer,
    FloatLiteral,
    StringLiteral,
    CharLiteral,
    InterpolatedStart,
    InterpolatedText,
    InterpolatedEnd,
    HoleStart,
    HoleEnd,
    LeftParen,
    RightParen,
    LeftBracket,
    RightBracket,
    Hash,
    LeftBrace,
    RightBrace,
    Colon,
    DoubleColon,
    Semicolon,
    Dot,
    DotDot,
    DotDotEqual,
    Comma,
    Arrow,
    FatArrow,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Bang,
    Assign,
    CompoundAssign(Binary),
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    AndAnd,
    OrOr,
    Ampersand,
    Pipe,
    Caret,
    Tilde,
    Invalid,
    Eof,
}

use Kind::*;

#[derive(Clone, Copy)]
struct Token {
    kind: Kind,
    span: Span,
}

enum InterpolationMode {
    Text,
    Hole(usize),
}

struct Interpolation {
    start: usize,
    mode: InterpolationMode,
}

fn lex(source: &str, base: usize) -> (Vec<Token>, Vec<Diagnostic>) {
    let span = |start, end| Span {
        start: base + start,
        end: base + end,
    };
    let bytes = source.as_bytes();
    let mut pos = 0;
    let mut tokens = Vec::new();
    let mut errors = Vec::new();
    let mut interpolations: Vec<Interpolation> = Vec::new();
    while let Some(&byte) = bytes.get(pos) {
        if let Some(interpolation) = interpolations.last_mut()
            && matches!(interpolation.mode, InterpolationMode::Text)
        {
            let start = pos;
            pos = scan_interpolated_text(bytes, pos);
            let kind = if pos != start {
                InterpolatedText
            } else {
                match byte {
                    b'"' => {
                        pos += 1;
                        interpolations.pop();
                        InterpolatedEnd
                    }
                    b'{' => {
                        pos += 1;
                        interpolation.mode = InterpolationMode::Hole(0);
                        HoleStart
                    }
                    b'}' => {
                        pos += 1;
                        errors.push(Diagnostic::new(
                            "E0108",
                            span(start, pos),
                            "escape a literal '}' as '}}'",
                        ));
                        Invalid
                    }
                    _ => {
                        errors.push(Diagnostic::new(
                            "E0108",
                            span(interpolation.start, pos),
                            "unterminated interpolated string",
                        ));
                        interpolations.pop();
                        Invalid
                    }
                }
            };
            tokens.push(Token {
                kind,
                span: span(start, pos),
            });
            continue;
        }
        let start = pos;
        pos += 1;
        let kind = match byte {
            b if b.is_ascii_whitespace() => continue,
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => {
                while bytes
                    .get(pos)
                    .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
                {
                    pos += 1;
                }
                identifier_kind(&source[start..pos])
            }
            b'0'..=b'9' => match scan_number(source, start, &mut pos) {
                Ok(kind) => kind,
                Err(message) => {
                    errors.push(Diagnostic::new("E0106", span(start, pos), message));
                    Invalid
                }
            },
            b'/' if bytes.get(pos) == Some(&b'/') => {
                while bytes.get(pos).is_some_and(|&b| b != b'\n') {
                    pos += 1;
                }
                continue;
            }
            b'$' if bytes.get(pos) == Some(&b'"') => {
                pos += 1;
                interpolations.push(Interpolation {
                    start,
                    mode: InterpolationMode::Text,
                });
                InterpolatedStart
            }
            b'"' | b'\'' => {
                let (end, closed) = scan_quoted(bytes, pos, byte);
                let character = byte == b'\'';
                pos = end;
                if closed {
                    if character {
                        CharLiteral
                    } else {
                        StringLiteral
                    }
                } else {
                    errors.push(Diagnostic::new(
                        "E0104",
                        span(start, end),
                        if character {
                            "unterminated character literal"
                        } else {
                            "unterminated string literal"
                        },
                    ));
                    Invalid
                }
            }
            b'(' => LeftParen,
            b')' => RightParen,
            b'[' => LeftBracket,
            b']' => RightBracket,
            b'#' => Hash,
            b'{' => {
                if let Some(Interpolation {
                    mode: InterpolationMode::Hole(depth),
                    ..
                }) = interpolations.last_mut()
                {
                    *depth += 1;
                }
                LeftBrace
            }
            b'}' => match interpolations.last_mut() {
                Some(Interpolation { mode, .. }) => match mode {
                    InterpolationMode::Hole(0) => {
                        *mode = InterpolationMode::Text;
                        HoleEnd
                    }
                    InterpolationMode::Hole(depth) => {
                        *depth -= 1;
                        RightBrace
                    }
                    InterpolationMode::Text => unreachable!("text is scanned separately"),
                },
                None => RightBrace,
            },
            b':' if bytes.get(pos) == Some(&b':') => {
                pos += 1;
                DoubleColon
            }
            b':' => Colon,
            b';' => Semicolon,
            b'.' if bytes.get(pos) == Some(&b'.') => {
                pos += 1;
                if bytes.get(pos) == Some(&b'=') {
                    pos += 1;
                    DotDotEqual
                } else {
                    DotDot
                }
            }
            b'.' => Dot,
            b',' => Comma,
            b'?' if bytes.get(pos) == Some(&b'?') => {
                pos += 1;
                Coalesce
            }
            b'?' => Question,
            b'&' if bytes.get(pos) == Some(&b'&') => {
                pos += 1;
                AndAnd
            }
            b'|' if bytes.get(pos) == Some(&b'|') => {
                pos += 1;
                OrOr
            }
            b'~' => Tilde,
            b'+' | b'-' | b'*' | b'/' | b'%' | b'!' | b'=' | b'<' | b'>' | b'&' | b'|' | b'^' => {
                let next = bytes.get(pos).copied();
                let (kind, paired) = match (byte, next) {
                    (b'+', Some(b'=')) => (CompoundAssign(Binary::Add), true),
                    (b'-', Some(b'=')) => (CompoundAssign(Binary::Sub), true),
                    (b'*', Some(b'=')) => (CompoundAssign(Binary::Mul), true),
                    (b'/', Some(b'=')) => (CompoundAssign(Binary::Div), true),
                    (b'%', Some(b'=')) => (CompoundAssign(Binary::Rem), true),
                    (b'&', Some(b'=')) => (CompoundAssign(Binary::BitAnd), true),
                    (b'|', Some(b'=')) => (CompoundAssign(Binary::BitOr), true),
                    (b'^', Some(b'=')) => (CompoundAssign(Binary::BitXor), true),
                    (b'-', Some(b'>')) => (Arrow, true),
                    (b'!', Some(b'=')) => (NotEqual, true),
                    (b'=', Some(b'=')) => (Equal, true),
                    (b'=', Some(b'>')) => (FatArrow, true),
                    (b'<', Some(b'=')) => (LessEqual, true),
                    (b'>', Some(b'=')) => (GreaterEqual, true),
                    (b'+', _) => (Plus, false),
                    (b'*', _) => (Star, false),
                    (b'/', _) => (Slash, false),
                    (b'%', _) => (Percent, false),
                    (b'-', _) => (Minus, false),
                    (b'!', _) => (Bang, false),
                    (b'=', _) => (Assign, false),
                    (b'<', _) => (Less, false),
                    (b'&', _) => (Ampersand, false),
                    (b'|', _) => (Pipe, false),
                    (b'^', _) => (Caret, false),
                    _ => (Greater, false),
                };
                pos += usize::from(paired);
                kind
            }
            _ => {
                let character = source[start..]
                    .chars()
                    .next()
                    .expect("character at lexer cursor");
                pos = start + character.len_utf8();
                errors.push(Diagnostic::new(
                    "E0101",
                    span(start, pos),
                    format!("unexpected character {character:?}"),
                ));
                Invalid
            }
        };
        tokens.push(Token {
            kind,
            span: span(start, pos),
        });
    }
    if let Some(interpolation) = interpolations.last() {
        let message = match interpolation.mode {
            InterpolationMode::Text => "unterminated interpolated string",
            InterpolationMode::Hole(_) => "unterminated interpolation expression",
        };
        errors.push(Diagnostic::new(
            "E0108",
            span(interpolation.start, pos),
            message,
        ));
    }
    tokens.push(Token {
        kind: Eof,
        span: span(pos, pos),
    });
    (tokens, errors)
}

pub(crate) fn integer_parts(text: &str) -> (u32, &str) {
    let radix = match text.as_bytes() {
        [b'0', b'b' | b'B', ..] => 2,
        [b'0', b'o' | b'O', ..] => 8,
        [b'0', b'x' | b'X', ..] => 16,
        _ => return (10, text),
    };
    (radix, &text[2..])
}

fn scan_number(source: &str, start: usize, pos: &mut usize) -> Result<Kind, &'static str> {
    const SEPARATOR: &str = "numeric separators must appear between digits";
    let (radix, _) = integer_parts(&source[start..]);
    let bytes = source.as_bytes();
    if radix != 10 {
        let first_digit = start + 2;
        *pos = first_digit;
        while bytes
            .get(*pos)
            .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
        {
            *pos += 1;
        }
        let digits = &bytes[first_digit..*pos];
        if digits.is_empty() {
            return Err("expected digits after the integer radix prefix");
        }
        if digits
            .iter()
            .any(|&digit| digit != b'_' && !char::from(digit).is_digit(radix))
        {
            return Err(match radix {
                2 => "invalid digit in binary integer",
                8 => "invalid digit in octal integer",
                _ => "invalid digit in hexadecimal integer",
            });
        }
        return if digits.split(|&digit| digit == b'_').any(<[u8]>::is_empty) {
            Err(SEPARATOR)
        } else {
            Ok(Integer)
        };
    }

    *pos = start;
    let mut valid = scan_decimal_digits(bytes, pos);
    let mut kind = Integer;
    // Keep the dot separate for member access and range expressions.
    if bytes.get(*pos) == Some(&b'.') && bytes.get(*pos + 1).is_some_and(u8::is_ascii_digit) {
        *pos += 1;
        valid &= scan_decimal_digits(bytes, pos);
        kind = FloatLiteral;
    }
    if matches!(bytes.get(*pos), Some(b'e' | b'E')) {
        *pos += 1;
        if matches!(bytes.get(*pos), Some(b'+' | b'-')) {
            *pos += 1;
        }
        let exponent = *pos;
        valid &= scan_decimal_digits(bytes, pos);
        if *pos == exponent {
            return Err("expected decimal exponent digits");
        }
        kind = FloatLiteral;
    }
    valid.then_some(kind).ok_or(SEPARATOR)
}

fn scan_decimal_digits(bytes: &[u8], pos: &mut usize) -> bool {
    let mut valid = true;
    loop {
        let start = *pos;
        while bytes.get(*pos).is_some_and(u8::is_ascii_digit) {
            *pos += 1;
        }
        valid &= *pos > start;
        if bytes.get(*pos) != Some(&b'_') {
            return valid;
        }
        *pos += 1;
    }
}

// Scan bytes to find the closing ASCII quote. The parser decodes escapes
// after taking the token, so malformed escapes have their own source spans.
fn scan_quoted(bytes: &[u8], mut pos: usize, quote: u8) -> (usize, bool) {
    while let Some(&byte) = bytes.get(pos) {
        match byte {
            byte if byte == quote => return (pos + 1, true),
            b'\n' | b'\r' => break,
            b'\\'
                if bytes
                    .get(pos + 1)
                    .is_some_and(|&next| next == quote || next == b'\\') =>
            {
                pos += 2
            }
            _ => pos += 1,
        }
    }
    (pos, false)
}

fn scan_interpolated_text(bytes: &[u8], mut pos: usize) -> usize {
    while let Some(&byte) = bytes.get(pos) {
        match byte {
            b'{' | b'}' if bytes.get(pos + 1) == Some(&byte) => pos += 2,
            b'{' | b'}' | b'"' | b'\n' | b'\r' => break,
            b'\\' if matches!(bytes.get(pos + 1), Some(b'\\' | b'"')) => pos += 2,
            _ => pos += 1,
        }
    }
    pos
}

fn decode_string(body: &str, start: usize, interpolated: bool) -> Result<Box<str>, Diagnostic> {
    let mut output = String::with_capacity(body.len());
    let mut characters = body.char_indices().peekable();
    while let Some((index, character)) = characters.next() {
        if interpolated && matches!(character, '{' | '}') {
            characters.next_if(|&(_, next)| next == character);
        }
        if character != '\\' {
            output.push(character);
            continue;
        }
        let escape = characters.next().map(|(_, value)| value);
        let value = match escape {
            Some('\\') => '\\',
            Some('"') => '"',
            Some('n') => '\n',
            Some('r') => '\r',
            Some('t') => '\t',
            Some('0') => '\0',
            _ => {
                let start = start + index;
                let end = start + 1 + escape.map_or(0, char::len_utf8);
                return Err(Diagnostic::new(
                    "E0105",
                    Span { start, end },
                    "invalid string escape",
                ));
            }
        };
        output.push(value);
    }
    Ok(output.into_boxed_str())
}

fn decode_char(body: &str, span: Span) -> Parsed<u16> {
    let invalid = || {
        Diagnostic::new(
            "E0109",
            span,
            "character literal must contain one UTF-16 code unit",
        )
    };
    if let Some(hex) = body
        .strip_prefix(r"\u{")
        .and_then(|text| text.strip_suffix('}'))
    {
        if hex.is_empty() || hex.len() > 4 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(invalid());
        }
        return u16::from_str_radix(hex, 16).map_err(|_| invalid());
    }
    if body == "\\'" {
        return Ok(u16::from(b'\''));
    }
    let decoded = decode_string(body, span.start + 1, false)?;
    let mut units = decoded.encode_utf16();
    units
        .next()
        .filter(|_| units.next().is_none())
        .ok_or_else(invalid)
}

pub(crate) struct Path<'src> {
    pub segments: Vec<(&'src str, Span)>,
    pub type_arguments: Vec<Path<'src>>,
    pub suffixes: Vec<TypeSuffix>,
    pub span: Span,
}

#[derive(Clone, Copy)]
pub(crate) enum TypeSuffix {
    Nullable(Span),
    Array(Span),
}

impl<'src> Path<'src> {
    fn single(name: &'src str, span: Span) -> Self {
        Self {
            segments: vec![(name, span)],
            span,
            type_arguments: Vec::new(),
            suffixes: Vec::new(),
        }
    }

    pub fn head_span(&self) -> Span {
        Span {
            start: self.span.start,
            end: self.segments.last().expect("nonempty path").1.end,
        }
    }

    pub fn qualified_name(&self) -> String {
        self.segments
            .iter()
            .map(|(name, _)| *name)
            .collect::<Vec<_>>()
            .join(".")
    }

    fn signature_label(&self) -> String {
        let mut label = self.qualified_name();
        if !self.type_arguments.is_empty() {
            let args = self
                .type_arguments
                .iter()
                .map(Self::signature_label)
                .collect::<Vec<_>>();
            label.push('<');
            label.push_str(&args.join(", "));
            label.push('>');
        }
        for suffix in &self.suffixes {
            label.push_str(match suffix {
                TypeSuffix::Array(_) => "[]",
                TypeSuffix::Nullable(_) => "?",
            });
        }
        label
    }
}

#[derive(Default)]
pub(crate) struct File<'src> {
    pub scope: FileScope<'src>,
    pub functions: Vec<Function<'src>>,
    pub types: Vec<TypeDefinition<'src>>,
    pub constants: Vec<ConstDefinition<'src>>,
}

#[derive(Default)]
pub(crate) struct FileScope<'src> {
    pub start: usize,
    pub namespace: Option<Path<'src>>,
    pub imports: Vec<Import<'src>>,
}

pub(crate) struct Import<'src> {
    pub path: Path<'src>,
    pub alias: (&'src str, Span),
}

#[derive(Default)]
pub(crate) struct Module<'ast, 'src> {
    pub scopes: Vec<&'ast FileScope<'src>>,
    pub functions: Vec<&'ast Function<'src>>,
    pub types: Vec<&'ast TypeDefinition<'src>>,
    pub constants: Vec<&'ast ConstDefinition<'src>>,
}

impl<'ast, 'src> Module<'ast, 'src> {
    pub fn new(files: impl IntoIterator<Item = &'ast File<'src>>) -> Self {
        let mut module = Self::default();
        for file in files {
            module.scopes.push(&file.scope);
            module.functions.extend(&file.functions);
            module.types.extend(&file.types);
            module.constants.extend(&file.constants);
        }
        module
    }
}

type TypedName<'src> = (&'src str, Span, Path<'src>);
type Initializer<'src> = (&'src str, Span, Expr<'src>);

pub(crate) struct ConstDefinition<'src> {
    pub name: &'src str,
    pub name_span: Span,
    pub declaration_span: Span,
    pub annotation: Option<Path<'src>>,
    pub value: Expr<'src>,
}

pub(crate) struct TypeDefinition<'src> {
    pub internal: bool,
    pub type_bounds: Vec<TypeBound<'src>>,
    pub type_parameters: Vec<(&'src str, Span)>,
    pub name: &'src str,
    pub span: Span,
    pub declaration_span: Span,
    pub shape: TypeShape<'src>,
}

pub(crate) enum TypeShape<'src> {
    Struct(Vec<TypedName<'src>>),
    Union(Vec<Variant<'src>>),
}

pub(crate) struct Variant<'src> {
    pub name: &'src str,
    pub span: Span,
    pub declaration_span: Span,
    pub payload: Vec<Path<'src>>,
}

pub(crate) struct MatchArm<'src> {
    pub pattern: Pattern<'src>,
    pub guard: Option<Expr<'src>>,
    pub value: Expr<'src>,
    pub arrow_end: usize,
}

pub(crate) struct Pattern<'src> {
    pub span: Span,
    pub kind: PatternKind<'src>,
}

pub(crate) enum PatternKind<'src> {
    Boolean(bool),
    Integer {
        digits: &'src str,
        negative: bool,
    },
    Character(u16),
    String(Box<str>),
    Name(Path<'src>, Option<Vec<Pattern<'src>>>),
    Struct {
        path: Path<'src>,
        fields: Vec<(&'src str, Span, Pattern<'src>)>,
        rest: bool,
    },
    Or(Vec<Pattern<'src>>),
}

pub(crate) struct Function<'src> {
    pub async_fn: bool,
    pub internal: bool,
    pub comptime: bool,
    pub test: Option<TestAttribute>,
    pub name: &'src str,
    pub name_span: Span,
    pub declaration_span: Span,
    pub type_parameters: Vec<(&'src str, Span)>,
    pub type_bounds: Vec<TypeBound<'src>>,
    pub parameters: Vec<FunctionParameter<'src>>,
    pub result: Option<Path<'src>>,
    pub body: FunctionBody<'src>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum TestAttribute {
    Enabled,
    Ignored,
}

impl Function<'_> {
    pub fn matching_test(&self, filter: &str) -> Option<TestAttribute> {
        self.test.filter(|_| self.name.contains(filter))
    }
}

pub(crate) struct FunctionParameter<'src> {
    pub name: &'src str,
    pub span: Span,
    pub annotation: Path<'src>,
    pub passing: ParameterPassing,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParameterPassing {
    Value,
    In,
    Ref,
    Out,
    Utf8,
}

impl ParameterPassing {
    pub fn mode(self) -> ArgumentMode {
        match self {
            Self::Value | Self::Utf8 => ArgumentMode::Value,
            Self::In => ArgumentMode::In,
            Self::Ref => ArgumentMode::Ref,
            Self::Out => ArgumentMode::Out,
        }
    }

    pub fn prefix(self) -> &'static str {
        match self {
            Self::Value => "",
            Self::In => "in ",
            Self::Ref => "ref ",
            Self::Out => "out ",
            Self::Utf8 => "utf8 ",
        }
    }
}

pub(crate) enum FunctionBody<'src> {
    Managed(Block<'src>),
    Native(NativeImport),
}

impl<'src> FunctionBody<'src> {
    pub fn managed(&self) -> Option<&Block<'src>> {
        match self {
            Self::Managed(block) => Some(block),
            Self::Native(_) => None,
        }
    }
}

#[derive(Clone)]
pub(crate) struct NativeImport {
    pub library: Box<str>,
    pub symbol: Box<str>,
}

struct FunctionHeader<'src> {
    async_fn: bool,
    native: bool,
    internal: bool,
    comptime: bool,
    name: &'src str,
    name_span: Span,
    start: usize,
    type_parameters: Vec<(&'src str, Span)>,
    type_bounds: Vec<TypeBound<'src>>,
    parameters: Vec<FunctionParameter<'src>>,
    result: Option<Path<'src>>,
}

pub(crate) fn function_prefix(
    internal: bool,
    comptime: bool,
    native: bool,
    async_fn: bool,
) -> &'static str {
    if async_fn {
        return if internal {
            "internal async fn"
        } else {
            "async fn"
        };
    }
    match (internal, comptime, native) {
        (true, _, true) => "internal extern \"C\" fn",
        (false, _, true) => "extern \"C\" fn",
        (true, true, false) => "internal comptime fn",
        (true, false, false) => "internal fn",
        (false, true, false) => "comptime fn",
        (false, false, false) => "fn",
    }
}

pub(crate) struct TypeBound<'src> {
    pub parameter: usize,
    pub constraint: Bound<'src>,
    pub span: Span,
}

pub(crate) enum Bound<'src> {
    Type(Path<'src>),
    Reference,
    Value,
    Constructor,
}

pub(crate) struct Block<'src> {
    pub statements: Vec<Statement<'src>>,
    pub tail: Option<Box<Expr<'src>>>,
    pub span: Span,
    depth: usize,
}

impl Block<'_> {
    pub fn interior(&self) -> Span {
        Span {
            start: self.span.start + 1,
            end: self.span.end - 1,
        }
    }
}

pub(crate) enum Statement<'src> {
    Let {
        name: &'src str,
        name_span: Span,
        mutable: bool,
        annotation: Option<Path<'src>>,
        value: Option<Expr<'src>>,
        end: usize,
    },
    LetPattern(Box<LetPattern<'src>>),
    Assign {
        operator: Option<Binary>,
        target: Expr<'src>,
        value: Expr<'src>,
    },
    Return {
        span: Span,
        value: Option<Expr<'src>>,
    },
    While {
        condition: Expr<'src>,
        body: Block<'src>,
    },
    WhileLet(Box<WhileLet<'src>>),
    For(Box<ForLoop<'src>>),
    LoopControl {
        kind: LoopControl,
        span: Span,
    },
    Expression(Expr<'src>),
}

pub(crate) struct LetPattern<'src> {
    pub pattern: Pattern<'src>,
    pub annotation: Option<Path<'src>>,
    pub value: Expr<'src>,
    pub fallback: Option<Block<'src>>,
    pub end: usize,
}

pub(crate) struct WhileLet<'src> {
    pub pattern: Pattern<'src>,
    pub value: Expr<'src>,
    pub body: Block<'src>,
}

pub(crate) struct ForLoop<'src> {
    pub binding: (&'src str, Span),
    pub source: Expr<'src>,
    pub range: Option<RangeEnd<'src>>,
    pub body: Block<'src>,
}

pub(crate) struct RangeEnd<'src> {
    pub end: Expr<'src>,
    pub inclusive: bool,
}

pub(crate) struct Expr<'src> {
    pub kind: Expression<'src>,
    pub span: Span,
    depth: usize,
}

impl Expr<'_> {
    pub(crate) fn has_null_tail(&self) -> bool {
        let mut expr = self;
        while let Expression::Block(block) = &expr.kind {
            let Some(tail) = block.tail.as_deref() else {
                return false;
            };
            expr = tail;
        }
        matches!(expr.kind, Expression::Null)
    }

    pub(crate) fn is_numeric_literal(&self) -> bool {
        matches!(self.kind, Expression::Int { .. } | Expression::Float { .. })
    }
}

pub(crate) struct Using<'src> {
    pub name: &'src str,
    pub name_span: Span,
    pub value: Expr<'src>,
    pub body: Block<'src>,
}

pub(crate) struct CatchArm<'src> {
    pub binding: TypedName<'src>,
    pub body: Block<'src>,
}

pub(crate) enum Expression<'src> {
    Await(Box<Expr<'src>>, Span),
    Int {
        digits: &'src str,
        negative: bool,
    },
    Float {
        digits: &'src str,
        negative: bool,
    },
    Bool(bool),
    Null,
    String(Box<str>),
    Char(u16),
    Interpolated(Vec<Expr<'src>>),
    Name(Path<'src>),
    ByRef(ArgumentMode, &'src str, Span),
    Construct {
        path: Path<'src>,
        fields: Vec<Initializer<'src>>,
        base: Option<Box<Expr<'src>>>,
    },
    Field(Box<Expr<'src>>, &'src str, Span),
    Index {
        receiver: Box<Expr<'src>>,
        arguments: Vec<Expr<'src>>,
        span: Span,
    },
    New(Path<'src>, Vec<Expr<'src>>),
    NewArray(Path<'src>, Box<Expr<'src>>),
    Array(Vec<Expr<'src>>),
    MemberCall {
        receiver: Box<Expr<'src>>,
        method: &'src str,
        span: Span,
        type_arguments: Vec<Path<'src>>,
        arguments: Vec<Expr<'src>>,
    },
    StaticMember {
        owner: Path<'src>,
        member: (&'src str, Span),
        call: Option<Box<CallArguments<'src>>>,
    },
    Call {
        path: Path<'src>,
        type_arguments: Vec<Path<'src>>,
        arguments: Vec<Expr<'src>>,
    },
    Invoke {
        callee: Box<Expr<'src>>,
        arguments: Vec<Expr<'src>>,
    },
    Propagate(Box<Expr<'src>>, Span),
    Throw(Box<Expr<'src>>),
    Try(Block<'src>),
    TryCatch(Block<'src>, Vec<CatchArm<'src>>),
    Using(Box<Using<'src>>),
    Lambda(Box<Lambda<'src>>),
    Unary(Unary, Box<Expr<'src>>),
    Binary(Binary, Box<Expr<'src>>, Box<Expr<'src>>),
    Cast(Box<Expr<'src>>, Path<'src>),
    OptionalCast(Box<Expr<'src>>, Path<'src>),
    TypeTest(Box<Expr<'src>>, Path<'src>),
    Block(Block<'src>),
    Match(Box<Expr<'src>>, Vec<MatchArm<'src>>),
    IfLet(Box<IfLet<'src>>),
    If {
        condition: Box<Expr<'src>>,
        yes: Block<'src>,
        no: Option<Box<Expr<'src>>>,
    },
}

pub(crate) struct IfLet<'src> {
    pub pattern: Pattern<'src>,
    pub value: Expr<'src>,
    pub yes: Block<'src>,
    pub no: Option<Box<Expr<'src>>>,
}

pub(crate) struct CallArguments<'src> {
    pub type_arguments: Vec<Path<'src>>,
    pub arguments: Vec<Expr<'src>>,
}

pub(crate) struct Lambda<'src> {
    pub async_fn: bool,
    pub parameters: Vec<(&'src str, Span)>,
    pub body: Block<'src>,
}

impl Expr<'_> {
    pub fn argument_mode(&self) -> ArgumentMode {
        match self.kind {
            Expression::ByRef(mode, ..) => mode,
            _ => ArgumentMode::Value,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum LoopControl {
    Break,
    Continue,
}

#[derive(Clone, Copy)]
pub(crate) enum Unary {
    Negate,
    Not,
    Complement,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Binary {
    Coalesce,
    And,
    Or,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl Binary {
    pub(crate) fn bitwise(self) -> bool {
        matches!(self, Self::BitAnd | Self::BitOr | Self::BitXor)
    }

    pub(crate) fn shift(self) -> bool {
        matches!(self, Self::Shl | Self::Shr)
    }
}

type Parsed<T> = Result<T, Diagnostic>;
pub(crate) const MAX_DEPTH: usize = 96;

pub(crate) fn parse(source: &str) -> Result<File<'_>, Vec<Diagnostic>> {
    let (file, errors) = parse_recovering(source);
    if errors.is_empty() {
        Ok(file)
    } else {
        Err(errors)
    }
}

pub(crate) fn parse_recovering(source: &str) -> (File<'_>, Vec<Diagnostic>) {
    parse_at(source, 0)
}

pub(crate) fn parse_at(source: &str, base: usize) -> (File<'_>, Vec<Diagnostic>) {
    parse_with_cursor(source, base, None).0
}

/// A call whose parenthesized argument list contains the cursor.
#[derive(Debug, PartialEq, Eq)]
pub struct CallSite {
    /// Span used by static call and constructor occurrences.
    pub callee: Span,
    /// Span of the member identifier when the callee is a dotted path.
    pub member: Span,
    /// The name of a call written as a bare or dotted path.
    pub named_path: Option<String>,
    pub argument: usize,
}

pub fn call_site(source: &str, cursor: usize, declaration: Option<Span>) -> Option<CallSite> {
    let (source, base) = declaration_text(source, declaration)?;
    parse_with_cursor(source, base, Some(cursor)).1
}

/// Reads a source signature even if the function body is incomplete.
/// A known declaration range limits how much source needs to be lexed.
pub fn declared_function_signature(
    source: &str,
    name: &str,
    declaration: Option<Span>,
) -> Option<CallableSignature> {
    let (source, base) = declaration_text(source, declaration)?;
    let mut parser = Parser {
        source,
        base,
        tokens: lex(source, base).0,
        pos: 0,
        cursor: None,
        call_site: None,
    };
    let index = parser.tokens.windows(2).position(|pair| {
        pair[0].kind == Fn && pair[1].kind == Name && parser.text(pair[1]) == name
    })?;
    parser.pos = match &parser.tokens[..index] {
        [.., external, abi] if external.kind == Extern && abi.kind == StringLiteral => index - 2,
        [.., modifier] if matches!(modifier.kind, Comptime | Async) => index - 1,
        _ => index,
    };
    if let Some(previous) = parser.pos.checked_sub(1)
        && parser.is_internal(parser.tokens[previous])
    {
        parser.pos = previous;
    }
    let internal = parser.take_internal();
    let header = parser.function_header(internal).ok()?;
    let prefix = function_prefix(
        header.internal,
        header.comptime,
        header.native,
        header.async_fn,
    );
    let mut label = format!("{prefix} {name}");
    if !header.type_parameters.is_empty() {
        let parameters = header
            .type_parameters
            .iter()
            .map(|&(name, _)| name)
            .collect::<Vec<_>>();
        label.push('<');
        label.push_str(&parameters.join(", "));
        label.push('>');
    }
    let result = header
        .result
        .as_ref()
        .map_or_else(|| "void".into(), Path::signature_label);
    Some(CallableSignature::new(
        label,
        header.parameters.iter().map(|parameter| {
            format!(
                "{}{}: {}",
                parameter.passing.prefix(),
                parameter.name,
                parameter.annotation.signature_label()
            )
        }),
        &result,
        false,
    ))
}

fn declaration_text(source: &str, declaration: Option<Span>) -> Option<(&str, usize)> {
    match declaration {
        Some(span) => Some((source.get(span.start..span.end)?, span.start)),
        None => Some((source, 0)),
    }
}

fn parse_with_cursor(
    source: &str,
    base: usize,
    cursor: Option<usize>,
) -> ((File<'_>, Vec<Diagnostic>), Option<CallSite>) {
    let (tokens, mut errors) = lex(source, base);
    let mut parser = Parser {
        source,
        base,
        tokens,
        pos: 0,
        cursor,
        call_site: None,
    };
    let mut file = File::default();
    file.scope.start = base;
    while parser.peek() != Eof {
        if cursor.is_some_and(|offset| offset < parser.current().span.start) {
            break;
        }
        let internal = parser.take_internal();
        let declaration = match parser.peek() {
            Fn | Comptime | Async | Extern => parser
                .function(internal)
                .map(|function| file.functions.push(function)),
            Struct | Enum => parser
                .type_definition(internal)
                .map(|definition| file.types.push(definition)),
            _ if internal.is_some() => {
                Err(parser
                    .expected("'fn', 'comptime', 'extern', 'struct', or 'enum' after 'internal'"))
            }
            Name if parser.text(parser.current()) == "namespace" => (|| {
                let keyword = parser.bump();
                if file.scope.namespace.is_some()
                    || !file.scope.imports.is_empty()
                    || !file.functions.is_empty()
                    || !file.types.is_empty()
                    || !file.constants.is_empty()
                {
                    return Err(Diagnostic::new(
                        "E0110",
                        keyword.span,
                        "a namespace must appear once, before imports and declarations",
                    ));
                }
                let path = parser.path()?;
                parser.semicolon()?;
                file.scope.namespace = Some(path);
                Ok(())
            })(),
            Name if parser.text(parser.current()) == "use" => (|| {
                let keyword = parser.bump();
                if !file.functions.is_empty()
                    || !file.types.is_empty()
                    || !file.constants.is_empty()
                {
                    return Err(Diagnostic::new(
                        "E0110",
                        keyword.span,
                        "imports must appear before declarations",
                    ));
                }
                file.scope.imports.extend(parser.imports()?);
                Ok(())
            })(),
            Hash => parser
                .test_function()
                .map(|function| file.functions.push(function)),
            Const => parser
                .constant()
                .map(|definition| file.constants.push(definition)),
            _ => Err(parser.expected("'fn', 'comptime', 'extern', 'struct', 'enum', or 'const'")),
        };
        match declaration {
            Ok(()) => {}
            Err(error) => {
                if error.code != "E0102" || parser.peek() != Invalid {
                    errors.push(error);
                }
                while parser.peek() != Eof && !parser.starts_declaration() {
                    parser.bump();
                }
            }
        }
    }
    errors.sort_by_key(|error| error.span.start);
    ((file, errors), parser.call_site)
}

struct Parser<'src> {
    source: &'src str,
    base: usize,
    tokens: Vec<Token>,
    pos: usize,
    cursor: Option<usize>,
    call_site: Option<CallSite>,
}

impl<'src> Parser<'src> {
    // Keep angle brackets separate for nested type arguments. Adjacent pairs
    // become shift operators only when the expression parser looks for one.
    fn shift_operator(&self) -> Option<(Binary, bool)> {
        let first = self.current();
        let (op, plain, compound) = match first.kind {
            Less => (Binary::Shl, Less, LessEqual),
            Greater => (Binary::Shr, Greater, GreaterEqual),
            _ => return None,
        };
        let second = self.tokens.get(self.pos + 1)?;
        (first.span.end == second.span.start && (second.kind == plain || second.kind == compound))
            .then_some((op, second.kind == compound))
    }

    fn take_assignment(&mut self) -> Option<Option<Binary>> {
        match self.peek() {
            Assign => {
                self.bump();
                Some(None)
            }
            CompoundAssign(op) => {
                self.bump();
                Some(Some(op))
            }
            _ => {
                let (op, true) = self.shift_operator()? else {
                    return None;
                };
                self.bump();
                self.bump();
                Some(Some(op))
            }
        }
    }

    fn current(&self) -> Token {
        self.tokens[self.pos]
    }

    fn peek(&self) -> Kind {
        self.current().kind
    }

    fn bump(&mut self) -> Token {
        let token = self.current();
        if token.kind != Eof {
            self.pos += 1;
        }
        token
    }

    fn take(&mut self, kind: Kind) -> Option<Token> {
        (self.peek() == kind).then(|| self.bump())
    }

    fn expect(&mut self, kind: Kind, description: &str) -> Parsed<Token> {
        self.take(kind).ok_or_else(|| self.expected(description))
    }

    fn is_internal(&self, token: Token) -> bool {
        token.kind == Name && self.text(token) == "internal"
    }

    fn take_internal(&mut self) -> Option<Token> {
        self.is_internal(self.current()).then(|| self.bump())
    }

    fn starts_declaration(&self) -> bool {
        matches!(
            self.peek(),
            Fn | Comptime | Async | Extern | Struct | Enum | Const | Hash
        ) || (self.peek() == Name
            && matches!(self.text(self.current()), "internal" | "namespace" | "use"))
    }

    fn semicolon(&mut self) -> Parsed<Token> {
        self.take(Semicolon).ok_or_else(|| {
            let error = self.expected("';'");
            if self.starts_statement()
                || matches!(self.peek(), RightBrace | Eof)
                || self.starts_declaration()
            {
                self.suggest_semicolon(error)
            } else {
                error
            }
        })
    }

    fn starts_statement(&self) -> bool {
        matches!(
            self.peek(),
            Let | Return
                | Throw
                | Try
                | Using
                | While
                | For
                | Break
                | Continue
                | If
                | Match
                | Name
                | LeftBrace
                | New
        )
    }

    fn suggest_semicolon(&self, error: Diagnostic) -> Diagnostic {
        let end = self.tokens[self.pos - 1].span.end;
        error.with_fix(DiagnosticFix::insert("Insert missing semicolon", end, ";"))
    }

    fn expected(&self, description: &str) -> Diagnostic {
        let token = self.current();
        if token.kind == Pipe {
            return Diagnostic::new(
                "E0101",
                token.span,
                format!("expected {description}, found '|'"),
            );
        }
        let found = if token.kind == Eof {
            "end of file".into()
        } else {
            format!("{:?}", self.text(token))
        };
        Diagnostic::new(
            "E0102",
            token.span,
            format!("expected {description}, found {found}"),
        )
    }

    fn text(&self, token: Token) -> &'src str {
        &self.source[token.span.start - self.base..token.span.end - self.base]
    }

    fn name(&mut self) -> Parsed<(&'src str, Span)> {
        let token = self.expect(Name, "an identifier")?;
        Ok((self.text(token), token.span))
    }

    fn path(&mut self) -> Parsed<Path<'src>> {
        let (first, mut span) = self.name()?;
        let mut segments = vec![(first, span)];
        while (self.peek() == Dot
            && self
                .tokens
                .get(self.pos + 1)
                .is_none_or(|token| token.kind != Await))
            || (self.peek() == DoubleColon
                && self
                    .tokens
                    .get(self.pos + 1)
                    .is_some_and(|token| token.kind == Name))
        {
            self.bump();
            let (name, part) = self.name()?;
            segments.push((name, part));
            span.end = part.end;
        }
        Ok(Path {
            segments,
            span,
            type_arguments: Vec::new(),
            suffixes: Vec::new(),
        })
    }

    fn imports(&mut self) -> Parsed<Vec<Import<'src>>> {
        let prefix = self.path()?;
        let imports = if self.take(DoubleColon).is_some() {
            self.expect(LeftBrace, "'{' after import prefix")?;
            if self.peek() == RightBrace {
                return Err(self.expected("an import name"));
            }
            self.list(RightBrace, "'}'", |parser| {
                let path = parser.path()?;
                let mut import = parser.import_alias(path)?;
                let mut segments = prefix.segments.clone();
                segments.append(&mut import.path.segments);
                import.path.segments = segments;
                Ok(import)
            })?
        } else {
            vec![self.import_alias(prefix)?]
        };
        self.semicolon()?;
        Ok(imports)
    }

    fn import_alias(&mut self, path: Path<'src>) -> Parsed<Import<'src>> {
        let alias = if self.take(As).is_some() {
            self.name()?
        } else {
            *path.segments.last().expect("nonempty import path")
        };
        Ok(Import { path, alias })
    }

    fn type_path(&mut self, depth: usize) -> Parsed<Path<'src>> {
        self.nesting(depth)?;
        let mut path = self.path()?;
        if self.take(Less).is_some() {
            let (arguments, end) = self.type_arguments(depth + 1)?;
            path.type_arguments = arguments;
            path.span.end = end;
        }
        self.type_suffixes(path, depth)
    }

    fn conversion_type(&mut self, depth: usize) -> Parsed<Path<'src>> {
        let mut path = self.path()?;
        // After a conversion, a bare '<' can start a comparison.
        if self.peek() == Less && self.shift_operator().is_none() && self.has_type_argument_list() {
            self.bump();
            let (arguments, end) = self.type_arguments(depth + 1)?;
            path.type_arguments = arguments;
            path.span.end = end;
        }
        self.type_suffixes(path, depth)
    }

    fn type_suffixes(&mut self, mut path: Path<'src>, depth: usize) -> Parsed<Path<'src>> {
        loop {
            let suffix = match self.peek() {
                Question if !matches!(path.suffixes.last(), Some(TypeSuffix::Nullable(_))) => {
                    let span = self.bump().span;
                    path.span.end = span.end;
                    TypeSuffix::Nullable(span)
                }
                LeftBracket
                    if self
                        .tokens
                        .get(self.pos + 1)
                        .is_some_and(|token| token.kind == RightBracket) =>
                {
                    let start = self.bump().span.start;
                    let end = self.bump().span.end;
                    path.span.end = end;
                    TypeSuffix::Array(Span { start, end })
                }
                _ => break,
            };
            self.nesting(depth + path.suffixes.len() + 1)?;
            path.suffixes.push(suffix);
        }
        Ok(path)
    }

    fn has_type_argument_list(&self) -> bool {
        let mut depth = 0;
        let mut tokens = self.tokens[self.pos..].iter();
        while let Some(token) = tokens.next() {
            match token.kind {
                Less => depth += 1,
                Greater | GreaterEqual => {
                    depth -= 1;
                    if depth == 0 {
                        return true;
                    }
                }
                LeftBracket
                    if tokens
                        .next()
                        .is_some_and(|token| token.kind == RightBracket) => {}
                Name | Dot | Comma | Question => {}
                _ => return false,
            }
        }
        false
    }

    fn type_arguments(&mut self, depth: usize) -> Parsed<(Vec<Path<'src>>, usize)> {
        let mut items = vec![self.type_path(depth)?];
        while self.take(Comma).is_some() && !matches!(self.peek(), Greater | GreaterEqual) {
            items.push(self.type_path(depth)?);
        }
        if self.peek() == GreaterEqual {
            // In a type annotation, `>=` can end the type and start an assignment.
            let token = &mut self.tokens[self.pos];
            token.kind = Assign;
            token.span.start += 1;
            return Ok((items, token.span.start));
        }
        Ok((items, self.expect(Greater, "'>'")?.span.end))
    }

    fn type_parameters(
        &mut self,
        bounds: &mut Vec<TypeBound<'src>>,
    ) -> Parsed<Vec<(&'src str, Span)>> {
        if self.take(Less).is_none() {
            return Ok(Vec::new());
        }
        if self.peek() == Greater {
            return Err(self.expected("a type parameter"));
        }
        let mut parameter = 0;
        self.list(Greater, "'>'", |parser| {
            let name = parser.name()?;
            if parser.take(Colon).is_some() {
                loop {
                    let start = parser.current().span.start;
                    let constraint = match parser.peek() {
                        Name if parser.text(parser.current()) == "class"
                            && matches!(
                                parser.tokens.get(parser.pos + 1).map(|token| token.kind),
                                Some(Plus | Greater | Comma)
                            ) =>
                        {
                            parser.bump();
                            Bound::Reference
                        }
                        Struct => {
                            parser.bump();
                            Bound::Value
                        }
                        New => {
                            parser.bump();
                            parser.expect(LeftParen, "'('")?;
                            parser.expect(RightParen, "')'")?;
                            Bound::Constructor
                        }
                        _ => Bound::Type(parser.type_path(0)?),
                    };
                    let end = parser.tokens[parser.pos - 1].span.end;
                    bounds.push(TypeBound {
                        parameter,
                        constraint,
                        span: Span { start, end },
                    });
                    if parser.take(Plus).is_none() {
                        break;
                    }
                }
            }
            parameter += 1;
            Ok(name)
        })
    }

    fn typed_name(&mut self) -> Parsed<TypedName<'src>> {
        let (name, span) = self.name()?;
        self.expect(Colon, "':'")?;
        Ok((name, span, self.type_path(0)?))
    }

    fn constant(&mut self) -> Parsed<ConstDefinition<'src>> {
        let start = self.expect(Const, "'const'")?.span.start;
        let (name, name_span) = self.name()?;
        let annotation = self.take(Colon).map(|_| self.type_path(0)).transpose()?;
        self.expect(Assign, "'='")?;
        let value = self.expression(0, 0)?;
        let end = self.semicolon()?.span.end;
        Ok(ConstDefinition {
            name,
            name_span,
            declaration_span: Span { start, end },
            annotation,
            value,
        })
    }

    fn type_definition(&mut self, internal: Option<Token>) -> Parsed<TypeDefinition<'src>> {
        let keyword = self.bump();
        let start = internal.unwrap_or(keyword).span.start;
        let (name, span) = self.name()?;
        let mut type_bounds = Vec::new();
        let type_parameters = self.type_parameters(&mut type_bounds)?;
        self.expect(LeftBrace, "'{'")?;
        let shape = if keyword.kind == Struct {
            TypeShape::Struct(self.list(RightBrace, "'}'", Self::typed_name)?)
        } else {
            TypeShape::Union(self.list(RightBrace, "'}'", |parser| {
                let (name, span) = parser.name()?;
                let payload = if parser.take(LeftParen).is_some() {
                    parser.list(RightParen, "')'", |parser| parser.type_path(0))?
                } else {
                    Vec::new()
                };
                let end = parser.tokens[parser.pos - 1].span.end;
                let declaration_span = Span {
                    start: span.start,
                    end,
                };
                Ok(Variant {
                    name,
                    span,
                    declaration_span,
                    payload,
                })
            })?)
        };
        let end = self.tokens[self.pos - 1].span.end;
        Ok(TypeDefinition {
            internal: internal.is_some(),
            name,
            span,
            declaration_span: Span { start, end },
            shape,
            type_parameters,
            type_bounds,
        })
    }

    fn function_header(&mut self, internal: Option<Token>) -> Parsed<FunctionHeader<'src>> {
        let modifier = self
            .take(Comptime)
            .or_else(|| self.take(Extern))
            .or_else(|| self.take(Async));
        let native = modifier.is_some_and(|token| token.kind == Extern);
        if native {
            let abi = self.current().span;
            if &*self.import_string("a native calling convention string")? != "C" {
                return Err(Diagnostic::new(
                    "E0343",
                    abi,
                    "only the C calling convention is supported",
                ));
            }
        }
        let keyword = self.expect(Fn, "'fn'")?;
        let start = internal.or(modifier).unwrap_or(keyword).span.start;
        let (name, name_span) = self.name()?;
        let mut type_bounds = Vec::new();
        let type_parameters = self.type_parameters(&mut type_bounds)?;
        self.expect(LeftParen, "'('")?;
        let parameters = self.list(RightParen, "')'", |parser| {
            let passing = if native {
                parser
                    .take_parameter_prefix(true)
                    .map_or(ParameterPassing::Value, |(passing, _)| passing)
            } else {
                ParameterPassing::Value
            };
            let (name, span, annotation) = parser.typed_name()?;
            Ok(FunctionParameter {
                name,
                span,
                annotation,
                passing,
            })
        })?;
        let result = self.take(Arrow).map(|_| self.type_path(0)).transpose()?;
        Ok(FunctionHeader {
            async_fn: modifier.is_some_and(|token| token.kind == Async),
            native,
            internal: internal.is_some(),
            comptime: modifier.is_some_and(|token| token.kind == Comptime),
            name,
            name_span,
            start,
            type_parameters,
            type_bounds,
            parameters,
            result,
        })
    }

    fn function(&mut self, internal: Option<Token>) -> Parsed<Function<'src>> {
        let header = self.function_header(internal)?;
        let body = if header.native {
            self.expect(Assign, "'=' before a native library name")?;
            let library = self.import_string("a native library name")?;
            let symbol = if self.take(As).is_some() {
                self.import_string("a native symbol name")?
            } else {
                header.name.into()
            };
            self.semicolon()?;
            FunctionBody::Native(NativeImport { library, symbol })
        } else {
            FunctionBody::Managed(self.block(0)?)
        };
        Ok(Function {
            async_fn: header.async_fn,
            internal: header.internal,
            comptime: header.comptime,
            test: None,
            name: header.name,
            name_span: header.name_span,
            declaration_span: Span {
                start: header.start,
                end: self.tokens[self.pos - 1].span.end,
            },
            type_parameters: header.type_parameters,
            type_bounds: header.type_bounds,
            parameters: header.parameters,
            result: header.result,
            body,
        })
    }

    fn test_function(&mut self) -> Parsed<Function<'src>> {
        let start = self.current().span.start;
        let mut test = false;
        let mut ignore = None;
        while self.take(Hash).is_some() {
            self.expect(LeftBracket, "'[' after '#'")?;
            let (attribute, span) = self.name()?;
            let duplicate = match attribute {
                "test" => std::mem::replace(&mut test, true),
                "ignore" => ignore.replace(span).is_some(),
                _ => {
                    return Err(Diagnostic::new(
                        "E0346",
                        span,
                        format!("unknown attribute '{attribute}'"),
                    ));
                }
            };
            if duplicate {
                return Err(Diagnostic::new(
                    "E0346",
                    span,
                    format!("duplicate attribute '#[{attribute}]'"),
                ));
            }
            self.expect(RightBracket, "']' after the attribute name")?;
        }
        if !test && let Some(span) = ignore {
            return Err(Diagnostic::new(
                "E0346",
                span,
                "'#[ignore]' requires '#[test]'",
            ));
        }
        if self.peek() != Fn {
            return Err(self.expected("'fn' after '#[test]'"));
        }
        let mut function = self.function(None)?;
        function.test = Some(if ignore.is_some() {
            TestAttribute::Ignored
        } else {
            TestAttribute::Enabled
        });
        function.internal = true;
        function.declaration_span.start = start;
        Ok(function)
    }

    fn import_string(&mut self, description: &str) -> Parsed<Box<str>> {
        let token = self.expect(StringLiteral, description)?;
        let text = self.text(token);
        let value = decode_string(&text[1..text.len() - 1], token.span.start + 1, false)?;
        if value.is_empty() || value.contains('\0') {
            return Err(Diagnostic::new(
                "E0343",
                token.span,
                "native import strings must be nonempty and contain no NUL",
            ));
        }
        Ok(value)
    }

    fn list<T>(
        &mut self,
        end: Kind,
        description: &str,
        mut item: impl FnMut(&mut Self) -> Parsed<T>,
    ) -> Parsed<Vec<T>> {
        let mut items = Vec::new();
        while self.peek() != end {
            items.push(item(self)?);
            if self.take(Comma).is_none() {
                break;
            }
        }
        self.expect(end, description)?;
        Ok(items)
    }

    fn nesting(&self, depth: usize) -> Parsed<()> {
        if depth > MAX_DEPTH {
            Err(Diagnostic::new(
                "E0103",
                self.current().span,
                "syntax nesting limit exceeded",
            ))
        } else {
            Ok(())
        }
    }

    fn for_loop(&mut self, depth: usize) -> Parsed<Statement<'src>> {
        self.bump();
        let binding = self.name()?;
        if self.peek() != Name || self.text(self.current()) != "in" {
            return Err(self.expected("'in'"));
        }
        self.bump();
        let source = self.expression_inner(0, depth, false)?;
        let range = match self.peek() {
            op @ (DotDot | DotDotEqual) => {
                self.bump();
                Some(RangeEnd {
                    end: self.expression_inner(0, depth, false)?,
                    inclusive: op == DotDotEqual,
                })
            }
            _ => None,
        };
        let body = self.block(depth)?;
        self.take(Semicolon);
        Ok(Statement::For(Box::new(ForLoop {
            binding,
            source,
            range,
            body,
        })))
    }

    fn let_statement(&mut self, depth: usize) -> Parsed<Statement<'src>> {
        self.bump();
        let mutable = self.take(Mut).is_some();
        let is_name = self.peek() == Name
            && !self
                .tokens
                .get(self.pos + 1)
                .is_some_and(|token| matches!(token.kind, Dot | LeftParen | LeftBrace | Pipe));
        let (binding, pattern) = if mutable || is_name {
            (Some(self.name()?), None)
        } else {
            (None, Some(self.pattern(depth + 1)?))
        };
        let annotation = self.take(Colon).map(|_| self.type_path(0)).transpose()?;
        let value = self
            .take(Assign)
            .map(|_| self.expression(0, depth + 1))
            .transpose()?;
        if self.peek() == Else || pattern.is_some() {
            let value = value.ok_or_else(|| self.expected("'=' and a pattern initializer"))?;
            let keyword = self.take(Else);
            if let Some(keyword) = keyword.filter(|_| mutable) {
                return Err(Diagnostic::new(
                    "E0102",
                    keyword.span,
                    "let-else bindings are immutable",
                ));
            }
            let pattern = pattern.unwrap_or_else(|| {
                let (name, span) = binding.expect("simple name pattern");
                Pattern {
                    kind: PatternKind::Name(Path::single(name, span), None),
                    span,
                }
            });
            let fallback = keyword.map(|_| self.block(depth + 1)).transpose()?;
            let end = self.semicolon()?.span.end;
            Ok(Statement::LetPattern(Box::new(LetPattern {
                pattern,
                annotation,
                value,
                fallback,
                end,
            })))
        } else {
            let (name, name_span) = binding.expect("simple binding");
            let end = self.semicolon()?.span.end;
            Ok(Statement::Let {
                name,
                name_span,
                mutable,
                annotation,
                value,
                end,
            })
        }
    }

    fn while_statement(&mut self, depth: usize) -> Parsed<Statement<'src>> {
        self.bump();
        let loop_ = if self.take(Let).is_some() {
            let (pattern, value) = self.pattern_test(depth)?;
            let body = self.block(depth)?;
            Statement::WhileLet(Box::new(WhileLet {
                pattern,
                value,
                body,
            }))
        } else {
            let condition = self.expression_inner(0, depth, false)?;
            let body = self.block(depth)?;
            Statement::While { condition, body }
        };
        self.take(Semicolon);
        Ok(loop_)
    }

    fn block(&mut self, depth: usize) -> Parsed<Block<'src>> {
        self.nesting(depth)?;
        let start = self.expect(LeftBrace, "'{'")?.span.start;
        let mut statements = Vec::new();
        let mut tail = None;
        while self.peek() != RightBrace {
            match self.peek() {
                Let => statements.push(self.let_statement(depth)?),
                Return => {
                    let span = self.bump().span;
                    let value = if matches!(self.peek(), Semicolon | RightBrace) {
                        None
                    } else {
                        Some(self.expression(0, depth + 1)?)
                    };
                    if self.peek() != RightBrace {
                        self.semicolon()?;
                    }
                    statements.push(Statement::Return { span, value });
                }
                While => statements.push(self.while_statement(depth + 1)?),
                For => statements.push(self.for_loop(depth + 2)?),
                Break | Continue => {
                    let token = self.bump();
                    if self.peek() != RightBrace {
                        self.semicolon()?;
                    }
                    let kind = if token.kind == Break {
                        LoopControl::Break
                    } else {
                        LoopControl::Continue
                    };
                    statements.push(Statement::LoopControl {
                        kind,
                        span: token.span,
                    });
                }
                _ => {
                    let expr = self.expression(0, depth + 1)?;
                    if let Some(operator) = self.take_assignment() {
                        let value = self.expression(0, depth + 1)?;
                        self.semicolon()?;
                        statements.push(Statement::Assign {
                            operator,
                            target: expr,
                            value,
                        });
                    } else if self.take(Semicolon).is_some() {
                        statements.push(Statement::Expression(expr));
                    } else if self.peek() == RightBrace {
                        tail = Some(Box::new(expr));
                        break;
                    } else if matches!(
                        expr.kind,
                        Expression::If { .. }
                            | Expression::IfLet(_)
                            | Expression::Block(_)
                            | Expression::Match(..)
                            | Expression::Try(_)
                            | Expression::TryCatch(..)
                            | Expression::Using(_)
                    ) {
                        statements.push(Statement::Expression(expr));
                    } else {
                        let error = self.expected("';' or '}'");
                        return Err(if self.starts_statement() {
                            self.suggest_semicolon(error)
                        } else {
                            error
                        });
                    }
                }
            }
        }
        let end = self.bump().span.end;
        let tree_depth = 1 + statements
            .iter()
            .filter_map(|stmt| match stmt {
                Statement::Let { value, .. } => value.as_ref().map(|value| value.depth),
                Statement::LetPattern(binding) => {
                    let fallback = binding.fallback.as_ref().map_or(0, |block| block.depth);
                    Some(1 + binding.value.depth.max(fallback))
                }
                Statement::Expression(value) => Some(value.depth),
                Statement::Assign { target, value, .. } => Some(target.depth.max(value.depth)),
                Statement::Return { value, .. } => value.as_ref().map(|value| value.depth),
                Statement::While { condition, body } => Some(1 + condition.depth.max(body.depth)),
                Statement::WhileLet(binding) => {
                    Some(1 + binding.value.depth.max(binding.body.depth))
                }
                Statement::For(loop_) => {
                    let end_depth = loop_.range.as_ref().map_or(0, |range| range.end.depth);
                    Some(1 + loop_.source.depth.max(end_depth).max(loop_.body.depth))
                }
                Statement::LoopControl { .. } => None,
            })
            .chain(tail.iter().map(|expr| expr.depth))
            .max()
            .unwrap_or_default();
        self.nesting(depth + tree_depth)?;
        Ok(Block {
            statements,
            tail,
            span: Span { start, end },
            depth: tree_depth,
        })
    }

    fn expression(&mut self, min_binding: u8, depth: usize) -> Parsed<Expr<'src>> {
        self.expression_inner(min_binding, depth, true)
    }

    fn expression_inner(
        &mut self,
        min_binding: u8,
        depth: usize,
        allow_literal: bool,
    ) -> Parsed<Expr<'src>> {
        self.nesting(depth)?;
        let left = self.atom(depth, allow_literal)?;
        let mut left = self.postfix_expression(left, depth, allow_literal)?;
        loop {
            let operator_kind = self.peek();
            let type_binding = match operator_kind {
                As => Some(11),
                Is => Some(4),
                _ => None,
            };
            if let Some(binding) = type_binding {
                if binding < min_binding {
                    break;
                }
                let operator = self.bump();
                let optional = operator_kind == As
                    && self.peek() == Question
                    && self.current().span.start == operator.span.end;
                if optional {
                    self.bump();
                }
                let target = self.conversion_type(depth + 1)?;
                let tree_depth = left.depth + 1;
                self.nesting(depth + tree_depth)?;
                let span = Span {
                    start: left.span.start,
                    end: target.span.end,
                };
                let kind = if optional {
                    Expression::OptionalCast(Box::new(left), target)
                } else if operator_kind == As {
                    Expression::Cast(Box::new(left), target)
                } else {
                    Expression::TypeTest(Box::new(left), target)
                };
                left = Expr {
                    kind,
                    span,
                    depth: tree_depth,
                };
                continue;
            }
            let (operator, binding, tokens) = match self.shift_operator() {
                Some((op, false)) => (op, 8, 2),
                Some((_, true)) => break,
                None => match binary(operator_kind) {
                    Some((op, binding)) => (op, binding, 1),
                    None => break,
                },
            };
            if binding < min_binding {
                break;
            }
            self.pos += tokens;
            let next_binding = if operator_kind == Coalesce {
                binding
            } else {
                binding + 1
            };
            let right = self.expression_inner(next_binding, depth + 1, allow_literal)?;
            let tree_depth = 1 + left.depth.max(right.depth);
            self.nesting(depth + tree_depth)?;
            let span = Span {
                start: left.span.start,
                end: right.span.end,
            };
            left = Expr {
                kind: Expression::Binary(operator, Box::new(left), Box::new(right)),
                span,
                depth: tree_depth,
            };
        }
        Ok(left)
    }

    fn postfix_expression(
        &mut self,
        mut left: Expr<'src>,
        depth: usize,
        allow_literal: bool,
    ) -> Parsed<Expr<'src>> {
        loop {
            if let Some(question) = self.take(Question) {
                let tree_depth = left.depth + 1;
                self.nesting(depth + tree_depth)?;
                let span = Span {
                    start: left.span.start,
                    end: question.span.end,
                };
                left = Expr {
                    kind: Expression::Propagate(Box::new(left), question.span),
                    span,
                    depth: tree_depth,
                };
                continue;
            }
            if let Some(open) = self.take(LeftBracket) {
                left = self.index_expression(left, open.span, depth)?;
                continue;
            }
            if self.take(LeftParen).is_some() {
                let arguments = self.call_arguments(depth + 1, allow_literal, left.span, None)?;
                let tree_depth = 1 + arguments
                    .iter()
                    .map(|arg| arg.depth)
                    .fold(left.depth, usize::max);
                self.nesting(depth + tree_depth)?;
                let span = Span {
                    start: left.span.start,
                    end: self.tokens[self.pos - 1].span.end,
                };
                left = Expr {
                    kind: Expression::Invoke {
                        callee: Box::new(left),
                        arguments,
                    },
                    span,
                    depth: tree_depth,
                };
                continue;
            }
            if self.take(Dot).is_none() {
                break;
            }
            if let Some(await_keyword) = self.take(Await) {
                let tree_depth = left.depth + 1;
                self.nesting(depth + tree_depth)?;
                let span = Span {
                    start: left.span.start,
                    end: await_keyword.span.end,
                };
                left = Expr {
                    kind: Expression::Await(Box::new(left), await_keyword.span),
                    span,
                    depth: tree_depth,
                };
                continue;
            }
            let (field, span) = self.name()?;
            let start = left.span.start;
            let type_arguments = self
                .call_type_arguments(depth + 1)?
                .map_or_else(Vec::new, |(types, _)| types);
            let (kind, tree_depth, end) = if !type_arguments.is_empty() || self.peek() == LeftParen
            {
                self.expect(LeftParen, "'('")?;
                let args = self.call_arguments(depth + 1, allow_literal, span, None)?;
                let tree_depth = 1 + left
                    .depth
                    .max(args.iter().map(|arg| arg.depth).max().unwrap_or(0));
                (
                    Expression::MemberCall {
                        receiver: Box::new(left),
                        method: field,
                        span,
                        type_arguments,
                        arguments: args,
                    },
                    tree_depth,
                    self.tokens[self.pos - 1].span.end,
                )
            } else {
                let depth = left.depth + 1;
                (
                    Expression::Field(Box::new(left), field, span),
                    depth,
                    span.end,
                )
            };
            self.nesting(depth + tree_depth)?;
            left = Expr {
                kind,
                span: Span { start, end },
                depth: tree_depth,
            };
        }
        Ok(left)
    }

    fn index_expression(
        &mut self,
        left: Expr<'src>,
        open: Span,
        depth: usize,
    ) -> Parsed<Expr<'src>> {
        if self.peek() == RightBracket {
            return Err(self.expected("an index expression"));
        }
        let arguments = self.list(RightBracket, "']'", |parser| {
            parser.expression(0, depth + 2)
        })?;
        let end = self.tokens[self.pos - 1].span.end;
        let tree_depth = 1 + arguments
            .iter()
            .map(|arg| arg.depth)
            .fold(left.depth, usize::max);
        self.nesting(depth + tree_depth)?;
        let span = Span {
            start: left.span.start,
            end,
        };
        Ok(Expr {
            kind: Expression::Index {
                receiver: Box::new(left),
                arguments,
                span: Span {
                    start: open.start,
                    end,
                },
            },
            span,
            depth: tree_depth,
        })
    }

    fn numeric_literal(&mut self, negative: bool) -> Expression<'src> {
        let token = self.bump();
        let digits = self.text(token);
        if token.kind == Integer {
            Expression::Int { digits, negative }
        } else {
            Expression::Float { digits, negative }
        }
    }

    fn match_expression(&mut self, depth: usize) -> Parsed<Expression<'src>> {
        self.nesting(depth)?;
        self.bump();
        let value = Box::new(self.expression_inner(0, depth + 1, false)?);
        self.expect(LeftBrace, "'{'")?;
        let arms = self.list(RightBrace, "'}'", |parser| {
            let pattern = parser.pattern(depth + 1)?;
            let guard = parser
                .take(If)
                .map(|_| parser.expression(0, depth + 1))
                .transpose()?;
            let arrow_end = parser.expect(FatArrow, "'=>'")?.span.end;
            let value = parser.expression(0, depth + 1)?;
            Ok(MatchArm {
                pattern,
                guard,
                value,
                arrow_end,
            })
        })?;
        Ok(Expression::Match(value, arms))
    }

    fn pattern(&mut self, depth: usize) -> Parsed<Pattern<'src>> {
        let first = self.pattern_atom(depth)?;
        if self.take(Pipe).is_none() {
            return Ok(first);
        }
        let start = first.span.start;
        let mut alternatives = vec![first];
        loop {
            let next = self.pattern_atom(depth)?;
            let end = next.span.end;
            alternatives.push(next);
            if self.take(Pipe).is_none() {
                return Ok(Pattern {
                    span: Span { start, end },
                    kind: PatternKind::Or(alternatives),
                });
            }
        }
    }

    fn pattern_atom(&mut self, depth: usize) -> Parsed<Pattern<'src>> {
        self.nesting(depth)?;
        let start = self.current().span.start;
        let kind = match self.peek() {
            LeftParen => {
                self.bump();
                let pattern = self.pattern(depth + 1)?;
                self.expect(RightParen, "')'")?;
                pattern.kind
            }
            True | False => PatternKind::Boolean(self.bump().kind == True),
            Integer | Minus => {
                let negative = self.take(Minus).is_some();
                let token = self.expect(Integer, "an integer literal")?;
                PatternKind::Integer {
                    digits: self.text(token),
                    negative,
                }
            }
            StringLiteral | CharLiteral => match self.quoted_literal()? {
                Expression::String(value) => PatternKind::String(value),
                Expression::Char(value) => PatternKind::Character(value),
                _ => unreachable!("quoted pattern literal"),
            },
            _ => {
                let path = self.path()?;
                if self.take(LeftBrace).is_some() {
                    let mut fields = Vec::new();
                    let mut rest = false;
                    while self.peek() != RightBrace {
                        if self.take(DotDot).is_some() {
                            rest = true;
                            self.take(Comma);
                            break;
                        }
                        let (name, span) = self.name()?;
                        let child = if self.take(Colon).is_some() {
                            self.pattern(depth + 2)?
                        } else {
                            Pattern {
                                kind: PatternKind::Name(Path::single(name, span), None),
                                span,
                            }
                        };
                        fields.push((name, span, child));
                        if self.take(Comma).is_none() {
                            break;
                        }
                    }
                    self.expect(RightBrace, "'}'")?;
                    PatternKind::Struct { path, fields, rest }
                } else if self.take(LeftParen).is_some() {
                    let children =
                        self.list(RightParen, "')'", |parser| parser.pattern(depth + 2))?;
                    PatternKind::Name(path, Some(children))
                } else {
                    PatternKind::Name(path, None)
                }
            }
        };
        let end = self.tokens[self.pos - 1].span.end;
        Ok(Pattern {
            span: Span { start, end },
            kind,
        })
    }

    fn call_type_arguments(&mut self, depth: usize) -> Parsed<Option<(Vec<Path<'src>>, usize)>> {
        if self.take(DoubleColon).is_none() {
            return Ok(None);
        }
        self.expect(Less, "'<'")?;
        self.type_arguments(depth).map(Some)
    }

    fn call_arguments(
        &mut self,
        depth: usize,
        allow_literal: bool,
        callee: Span,
        named_path: Option<&Path<'src>>,
    ) -> Parsed<Vec<Expr<'src>>> {
        let open = self.tokens[self.pos - 1].span;
        let cursor = self.cursor.filter(|&cursor| cursor >= open.end);
        let mut argument = 0;
        let arguments = self.list(RightParen, "')'", |parser| {
            // A nested argument keeps the call and its delimited list on the stack.
            let value = parser.call_argument(depth + 1)?;
            let next = parser.current();
            if next.kind == Comma && cursor.is_some_and(|offset| next.span.end <= offset) {
                argument += 1;
            }
            Ok(value)
        });
        if let Some(cursor) = cursor {
            let end = if arguments.is_ok() {
                self.tokens[self.pos - 1].span.start
            } else {
                self.current().span.start
            };
            if cursor <= end
                && self
                    .call_site
                    .as_ref()
                    .is_none_or(|site| site.callee.start < callee.start)
            {
                self.call_site = Some(CallSite {
                    callee,
                    member: named_path
                        .and_then(|path| path.segments.last())
                        .map_or(callee, |&(_, span)| span),
                    named_path: named_path.map(Path::qualified_name),
                    argument,
                });
            }
        }
        let mut arguments = arguments?;
        let async_lambda = self.peek() == Async
            && self.tokens.get(self.pos + 1).is_some_and(|token| {
                matches!(token.kind, Pipe | OrOr) || (allow_literal && token.kind == LeftBrace)
            });
        if async_lambda || self.peek() == Pipe || (allow_literal && self.peek() == LeftBrace) {
            arguments.push(self.lambda_expression(depth)?);
        }
        Ok(arguments)
    }

    fn take_parameter_prefix(&mut self, native: bool) -> Option<(ParameterPassing, Token)> {
        let token = self.current();
        if token.kind != Name || self.tokens.get(self.pos + 1)?.kind != Name {
            return None;
        }
        let passing = match self.text(token) {
            "in" if !native => ParameterPassing::In,
            "ref" => ParameterPassing::Ref,
            "out" => ParameterPassing::Out,
            "utf8" if native => ParameterPassing::Utf8,
            _ => return None,
        };
        Some((passing, self.bump()))
    }

    fn call_argument(&mut self, depth: usize) -> Parsed<Expr<'src>> {
        let reference = if let Some(ampersand) = self.take(Ampersand) {
            let mode = if self.take(Mut).is_some() {
                ArgumentMode::Ref
            } else {
                ArgumentMode::In
            };
            Some((mode, ampersand))
        } else {
            self.take_parameter_prefix(false)
                .map(|(passing, token)| (passing.mode(), token))
        };
        if let Some((mode, token)) = reference {
            let (name, span) = self.name()?;
            return Ok(Expr {
                kind: Expression::ByRef(mode, name, span),
                span: Span {
                    start: token.span.start,
                    end: span.end,
                },
                depth: 0,
            });
        }
        self.expression(0, depth)
    }

    fn lambda_expression(&mut self, depth: usize) -> Parsed<Expr<'src>> {
        // A lambda body is checked while both the enclosing method and the
        // delegate call are active, so charge more than an ordinary block.
        const LAMBDA_BODY_DEPTH: usize = 6;
        let start = self.current().span.start;
        let async_fn = self.take(Async).is_some();
        let parameters = if self.take(Pipe).is_some() {
            self.list(Pipe, "'|'", Self::name)?
        } else {
            self.take(OrOr);
            Vec::new()
        };
        let body = self.block(depth + LAMBDA_BODY_DEPTH)?;
        let span = Span {
            start,
            end: body.span.end,
        };
        let tree_depth = body.depth + LAMBDA_BODY_DEPTH;
        self.nesting(depth + tree_depth)?;
        Ok(Expr {
            kind: Expression::Lambda(Box::new(Lambda {
                async_fn,
                parameters,
                body,
            })),
            span,
            depth: tree_depth,
        })
    }

    fn named_expression(&mut self, depth: usize, allow_literal: bool) -> Parsed<Expression<'src>> {
        let mut path = self.path()?;
        let (type_arguments, end) = self
            .call_type_arguments(depth)?
            .unwrap_or_else(|| (Vec::new(), path.span.end));
        Ok(if allow_literal && self.take(LeftBrace).is_some() {
            let mut fields = Vec::new();
            let mut base = None;
            while self.peek() != RightBrace {
                if self.take(DotDot).is_some() {
                    base = Some(Box::new(self.expression(0, depth)?));
                    self.take(Comma);
                    break;
                }
                let (name, span) = self.name()?;
                let value = if self.take(Colon).is_some() {
                    self.expression(0, depth)?
                } else {
                    Expr {
                        kind: Expression::Name(Path::single(name, span)),
                        span,
                        depth: 1,
                    }
                };
                fields.push((name, span, value));
                if self.take(Comma).is_none() {
                    break;
                }
            }
            self.expect(RightBrace, "'}'")?;
            path.type_arguments = type_arguments;
            path.span.end = end;
            Expression::Construct { path, fields, base }
        } else if !type_arguments.is_empty() && self.take(Dot).is_some() {
            path.type_arguments = type_arguments;
            path.span.end = end;
            return self.static_member(path, depth, allow_literal);
        } else if !type_arguments.is_empty() || self.peek() == LeftParen {
            self.expect(LeftParen, "'('")?;
            let arguments = self.call_arguments(depth, allow_literal, path.span, Some(&path))?;
            Expression::Call {
                path,
                type_arguments,
                arguments,
            }
        } else {
            Expression::Name(path)
        })
    }

    fn static_member(
        &mut self,
        owner: Path<'src>,
        depth: usize,
        allow_literal: bool,
    ) -> Parsed<Expression<'src>> {
        let member = self.name()?;
        let type_arguments = self
            .call_type_arguments(depth)?
            .map_or_else(Vec::new, |(types, _)| types);
        let call = if !type_arguments.is_empty() || self.peek() == LeftParen {
            self.expect(LeftParen, "'('")?;
            Some(Box::new(CallArguments {
                type_arguments,
                arguments: self.call_arguments(depth, allow_literal, member.1, None)?,
            }))
        } else {
            None
        };
        Ok(Expression::StaticMember {
            owner,
            member,
            call,
        })
    }

    fn interpolated_string(&mut self, depth: usize) -> Parsed<Expression<'src>> {
        self.bump();
        let mut parts = Vec::new();
        loop {
            if let Some(text) = self.take(InterpolatedText) {
                parts.push(Expr {
                    kind: Expression::String(decode_string(
                        self.text(text),
                        text.span.start,
                        true,
                    )?),
                    span: text.span,
                    depth: 0,
                });
            } else if self.take(HoleStart).is_some() {
                parts.push(self.expression(0, depth)?);
                self.expect(HoleEnd, "'}' to close the interpolation expression")?;
            } else {
                self.expect(InterpolatedEnd, "the end of the interpolated string")?;
                return Ok(Expression::Interpolated(parts));
            }
        }
    }

    fn if_expression(&mut self, depth: usize, allow_literal: bool) -> Parsed<Expression<'src>> {
        self.bump();
        if self.take(Let).is_some() {
            return self.if_let_expression(depth, allow_literal);
        }
        let condition = Box::new(self.expression_inner(0, depth + 1, false)?);
        let (yes, no) = self.if_branches(depth, allow_literal)?;
        Ok(Expression::If { condition, yes, no })
    }

    fn if_let_expression(&mut self, depth: usize, allow_literal: bool) -> Parsed<Expression<'src>> {
        let (pattern, value) = self.pattern_test(depth + 1)?;
        let (yes, no) = self.if_branches(depth, allow_literal)?;
        Ok(Expression::IfLet(Box::new(IfLet {
            pattern,
            value,
            yes,
            no,
        })))
    }

    fn pattern_test(&mut self, depth: usize) -> Parsed<(Pattern<'src>, Expr<'src>)> {
        let pattern = self.pattern(depth)?;
        self.expect(Assign, "'='")?;
        let value = self.expression_inner(0, depth, false)?;
        Ok((pattern, value))
    }

    fn if_branches(
        &mut self,
        depth: usize,
        allow_literal: bool,
    ) -> Parsed<(Block<'src>, Option<Box<Expr<'src>>>)> {
        let yes = self.block(depth + 1)?;
        let no = if self.take(Else).is_some() {
            self.nesting(depth + 1)?;
            if !matches!(self.peek(), If | LeftBrace) {
                return Err(self.expected("'if' or '{'"));
            }
            Some(Box::new(self.atom(depth + 1, allow_literal)?))
        } else {
            None
        };
        Ok((yes, no))
    }

    fn array_literal(&mut self, depth: usize) -> Parsed<Expression<'src>> {
        self.bump();
        let elements = self.list(RightBracket, "']'", |parser| parser.expression(0, depth))?;
        Ok(Expression::Array(elements))
    }

    fn new_expression(
        &mut self,
        path: Path<'src>,
        depth: usize,
        allow_literal: bool,
    ) -> Parsed<Expression<'src>> {
        if self.take(LeftBracket).is_some() {
            let length = self.expression(0, depth + 1)?;
            self.expect(RightBracket, "']'")?;
            Ok(Expression::NewArray(path, Box::new(length)))
        } else {
            self.expect(LeftParen, "'('")?;
            let arguments = self.call_arguments(depth, allow_literal, path.span, None)?;
            Ok(Expression::New(path, arguments))
        }
    }

    fn quoted_literal(&mut self) -> Parsed<Expression<'src>> {
        let token = self.bump();
        let text = self.text(token);
        let body = &text[1..text.len() - 1];
        if token.kind == CharLiteral {
            decode_char(body, token.span).map(Expression::Char)
        } else {
            decode_string(body, token.span.start + 1, false).map(Expression::String)
        }
    }

    fn block_atom(&mut self, kind: Kind, depth: usize) -> Parsed<Expression<'src>> {
        if kind == Try {
            self.bump();
            let body = self.block(depth)?;
            let mut catches = Vec::new();
            while self.take(Catch).is_some() {
                self.expect(LeftParen, "'(' after 'catch'")?;
                let binding = self.typed_name()?;
                self.expect(RightParen, "')' after a catch binding")?;
                let body = self.block(depth)?;
                catches.push(CatchArm { binding, body });
            }
            if catches.is_empty() {
                Ok(Expression::Try(body))
            } else {
                Ok(Expression::TryCatch(body, catches))
            }
        } else {
            self.block(depth).map(Expression::Block)
        }
    }

    fn using_expression(&mut self, depth: usize) -> Parsed<Expression<'src>> {
        self.bump();
        let (name, name_span) = self.name()?;
        self.expect(Assign, "'=' after a using binding")?;
        let value = self.expression_inner(0, depth + 1, false)?;
        let body = self.block(depth + 1)?;
        Ok(Expression::Using(Box::new(Using {
            name,
            name_span,
            value,
            body,
        })))
    }

    fn atom(&mut self, depth: usize, allow_literal: bool) -> Parsed<Expr<'src>> {
        let token = self.current();
        if matches!(token.kind, Pipe | OrOr | Async) {
            return self.lambda_expression(depth);
        }
        let kind = match token.kind {
            Integer | FloatLiteral => self.numeric_literal(false),
            True | False => Expression::Bool(self.bump().kind == True),
            Null => {
                self.bump();
                Expression::Null
            }
            StringLiteral | CharLiteral => self.quoted_literal()?,
            InterpolatedStart => self.interpolated_string(depth + 2)?,
            Name => self.named_expression(depth + 1, allow_literal)?,
            New => {
                self.bump();
                let path = self.type_path(depth + 1)?;
                self.new_expression(path, depth + 1, allow_literal)?
            }
            LeftParen => {
                self.bump();
                let mut expression = self.expression(0, depth + 1)?;
                let closing = self.expect(RightParen, "')'")?;
                expression.span = Span {
                    start: token.span.start,
                    end: closing.span.end,
                };
                return Ok(expression);
            }
            LeftBracket => self.array_literal(depth + 2)?,
            LeftBrace | Try => self.block_atom(token.kind, depth + 1)?,
            Using => self.using_expression(depth + 1)?,
            If => self.if_expression(depth, allow_literal)?,
            Match => self.match_expression(depth + 1)?,
            Minus | Bang | Tilde | Throw => {
                self.bump();
                if token.kind == Minus && matches!(self.peek(), Integer | FloatLiteral) {
                    self.numeric_literal(true)
                } else {
                    let binding = if token.kind == Throw { 0 } else { 12 };
                    let operand =
                        Box::new(self.expression_inner(binding, depth + 1, allow_literal)?);
                    match token.kind {
                        Throw => Expression::Throw(operand),
                        Minus => Expression::Unary(Unary::Negate, operand),
                        Tilde => Expression::Unary(Unary::Complement, operand),
                        _ => Expression::Unary(Unary::Not, operand),
                    }
                }
            }
            _ => return Err(self.expected("an expression")),
        };
        let end = self.tokens[self.pos - 1].span.end;
        let tree_depth = atom_depth(&kind);
        self.nesting(depth + tree_depth)?;
        Ok(Expr {
            kind,
            span: Span {
                start: token.span.start,
                end,
            },
            depth: tree_depth,
        })
    }
}

fn atom_depth(kind: &Expression<'_>) -> usize {
    1 + match kind {
        Expression::Name(path) => path.segments.len() - 1,
        Expression::Construct { fields, base, .. } => fields
            .iter()
            .map(|(_, _, value)| value.depth)
            .chain(base.iter().map(|base| base.depth))
            .max()
            .unwrap_or_default(),
        Expression::Call {
            path,
            arguments: args,
            ..
        }
        | Expression::New(path, args) => path
            .segments
            .len()
            .saturating_sub(1)
            .max(args.iter().map(|arg| arg.depth).max().unwrap_or(0)),
        Expression::NewArray(path, length) => {
            path.segments.len().saturating_sub(1).max(length.depth)
        }
        Expression::StaticMember { owner, call, .. } => call
            .iter()
            .flat_map(|call| &call.arguments)
            .map(|arg| arg.depth)
            .fold(owner.segments.len(), usize::max),
        Expression::Unary(_, expr) | Expression::Throw(expr) => expr.depth,
        Expression::Interpolated(parts) | Expression::Array(parts) => {
            parts.iter().map(|part| part.depth).max().unwrap_or(0)
        }
        Expression::Block(block) | Expression::Try(block) => block.depth,
        Expression::Using(resource) => resource.value.depth.max(resource.body.depth),
        Expression::TryCatch(body, catches) => catches
            .iter()
            .map(|arm| arm.body.depth)
            .fold(body.depth, usize::max),
        Expression::Match(value, arms) => arms
            .iter()
            .map(|arm| {
                arm.value
                    .depth
                    .max(arm.guard.as_ref().map_or(0, |guard| guard.depth))
            })
            .fold(value.depth, usize::max),
        Expression::If { condition, yes, no } => condition
            .depth
            .max(yes.depth)
            .max(no.as_ref().map_or(0, |expr| expr.depth)),
        Expression::IfLet(binding) => binding
            .value
            .depth
            .max(binding.yes.depth)
            .max(binding.no.as_ref().map_or(0, |expr| expr.depth)),
        _ => 0,
    }
}

fn identifier_kind(text: &str) -> Kind {
    use Kind::*;
    match text {
        "fn" => Fn,
        "comptime" => Comptime,
        "async" => Async,
        "await" => Await,
        "extern" => Extern,
        "new" => New,
        "as" => As,
        "is" => Is,
        "null" => Null,
        "struct" => Struct,
        "enum" => Enum,
        "match" => Match,
        "const" => Const,
        "let" => Let,
        "mut" => Mut,
        "return" => Return,
        "throw" => Throw,
        "try" => Try,
        "catch" => Catch,
        "using" => Using,
        "if" => If,
        "else" => Else,
        "while" => While,
        "for" => For,
        "break" => Break,
        "continue" => Continue,
        "true" => True,
        "false" => False,
        _ => Name,
    }
}

/// Returns whether the entire string lexes as a single oxel identifier.
pub fn valid_identifier(text: &str) -> bool {
    let mut bytes = text.bytes();
    matches!(bytes.next(), Some(b'a'..=b'z' | b'A'..=b'Z' | b'_'))
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        && identifier_kind(text) == Kind::Name
}

fn binary(kind: Kind) -> Option<(Binary, u8)> {
    Some(match kind {
        Coalesce => (Binary::Coalesce, 0),
        OrOr => (Binary::Or, 1),
        AndAnd => (Binary::And, 2),
        Equal => (Binary::Eq, 3),
        NotEqual => (Binary::Ne, 3),
        Less => (Binary::Lt, 4),
        LessEqual => (Binary::Le, 4),
        Greater => (Binary::Gt, 4),
        GreaterEqual => (Binary::Ge, 4),
        Pipe => (Binary::BitOr, 5),
        Caret => (Binary::BitXor, 6),
        Ampersand => (Binary::BitAnd, 7),
        Plus => (Binary::Add, 9),
        Minus => (Binary::Sub, 9),
        Star => (Binary::Mul, 10),
        Slash => (Binary::Div, 10),
        Percent => (Binary::Rem, 10),
        _ => return None,
    })
}
