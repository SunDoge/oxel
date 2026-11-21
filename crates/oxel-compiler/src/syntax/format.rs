//! Whitespace formatting over the compiler's tokens and parser layout hints.

use std::collections::{HashMap, HashSet};

use super::{Kind, Kind::*, lex, parse_with_cursor};
use crate::Diagnostic;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum Role {
    #[default]
    Ordinary,
    Block,
    Import,
    AngleOpen,
    AngleClose,
    ShiftEnd,
    Prefix,
    LambdaOpen,
    LambdaClose,
    OptionalCast,
    ForIn,
}

#[derive(Default)]
pub(super) struct Layout {
    pub roles: HashMap<usize, Role>,
    pub lines: HashSet<usize>,
}

/// Formats a syntactically valid Oxel source file without resolving its names or types.
/// Comments and literal contents, including interpolated strings, are kept verbatim.
pub fn format_source(source: &str) -> Result<String, Vec<Diagnostic>> {
    let mut layout = Layout::default();
    let ((_, errors), _) = parse_with_cursor(source, 0, None, Some(&mut layout));
    if !errors.is_empty() {
        return Err(errors);
    }

    let pieces = pieces(source, &layout);
    let groups = groups(&pieces);
    Ok(render(&pieces, &groups, source.len()))
}

#[derive(Clone, Copy)]
struct Piece<'a> {
    text: &'a str,
    // A comment occupies a gap in the lexer token stream.
    kind: Option<Kind>,
    role: Role,
    line_start: bool,
    newlines_before: usize,
}

fn pieces<'a>(source: &'a str, layout: &Layout) -> Vec<Piece<'a>> {
    // Read the original spans because the parser may split tokens such as '>='.
    let (tokens, _) = lex(source, 0);
    let mut pieces = Vec::with_capacity(tokens.len());
    let mut tokens = tokens.into_iter();
    let mut previous_end = 0;
    while let Some(token) = tokens.next() {
        let start = token.span.start;
        let mut end = token.span.end;
        let mut kind = token.kind;
        // Keep complete interpolated strings opaque, including nested interpolations.
        if kind == InterpolatedStart {
            let mut depth = 1;
            for token in tokens.by_ref() {
                match token.kind {
                    InterpolatedStart => depth += 1,
                    InterpolatedEnd => {
                        depth -= 1;
                        if depth == 0 {
                            end = token.span.end;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            kind = StringLiteral;
        }

        let mut gap = &source[previous_end..start];
        let mut newlines_before = 0;
        while let Some(comment) = gap.find("//") {
            newlines_before += newline_count(&gap[..comment]);
            let line = &gap[comment..];
            let (text, remainder) = line.split_at(line.find('\n').unwrap_or(line.len()));
            pieces.push(Piece {
                text: text.strip_suffix('\r').unwrap_or(text),
                kind: None,
                role: Role::Ordinary,
                line_start: false,
                newlines_before,
            });
            newlines_before = 0;
            gap = remainder;
        }
        newlines_before += newline_count(gap);

        if kind == Eof {
            break;
        }
        let role = layout.roles.get(&start).copied().unwrap_or_default();
        // In a type followed by an initializer, the parser splits the lexer's '>='.
        if role == Role::AngleClose && kind == GreaterEqual {
            pieces.push(Piece {
                text: &source[start..start + 1],
                kind: Some(Greater),
                role,
                line_start: false,
                newlines_before,
            });
            pieces.push(Piece {
                text: &source[start + 1..end],
                kind: Some(Assign),
                role: Role::Ordinary,
                line_start: false,
                newlines_before: 0,
            });
        } else {
            pieces.push(Piece {
                text: &source[start..end],
                kind: Some(kind),
                role,
                line_start: layout.lines.contains(&start),
                newlines_before,
            });
        }
        previous_end = end;
    }
    pieces
}

fn newline_count(text: &str) -> usize {
    text.bytes().filter(|&byte| byte == b'\n').count()
}

impl Piece<'_> {
    fn opens_group(self) -> bool {
        matches!(self.kind, Some(LeftParen | LeftBracket | LeftBrace))
            || matches!(self.role, Role::AngleOpen | Role::LambdaOpen)
    }

    fn closes_group(self) -> bool {
        matches!(self.kind, Some(RightParen | RightBracket | RightBrace))
            || matches!(self.role, Role::AngleClose | Role::LambdaClose)
    }
}

#[derive(Clone, Copy, Default)]
struct Group {
    multiline: bool,
    import: bool,
    empty: bool,
}

fn groups(pieces: &[Piece<'_>]) -> Vec<Group> {
    let mut groups = vec![Group::default(); pieces.len()];
    let mut stack: Vec<usize> = Vec::new();
    for (index, piece) in pieces.iter().enumerate() {
        if piece.newlines_before > 0
            && let Some(&open) = stack.last()
        {
            // A line break inside a child group does not expand its enclosing list.
            groups[open].multiline = true;
        }
        if piece.opens_group() {
            stack.push(index);
            groups[index].multiline = piece.role == Role::Block;
            groups[index].import = piece.role == Role::Import;
        } else if piece.closes_group()
            && let Some(open) = stack.pop()
        {
            let empty = open + 1 == index;
            groups[open].empty = empty;
            groups[open].multiline &= !empty;
        }
    }
    groups
}

fn render(pieces: &[Piece<'_>], groups: &[Group], capacity: usize) -> String {
    let mut result = String::with_capacity(capacity);
    let mut stack: Vec<Group> = Vec::new();
    let mut indent = 0;
    for (index, &piece) in pieces.iter().enumerate() {
        let closing_group = if piece.closes_group() {
            stack.pop()
        } else {
            None
        };
        if closing_group.is_some_and(|group| group.multiline) {
            indent -= 1;
        }
        if index > 0 {
            let previous = pieces[index - 1];
            let multiline = stack.last().is_some_and(|group| group.multiline);
            let after_opener = previous.opens_group() && groups[index - 1].multiline;
            let structural_break = piece.line_start
                || closing_group.is_some_and(|group| group.multiline)
                || (piece.kind.is_some()
                    && (after_opener || (multiline && previous.kind == Some(Comma))));
            let joins = closing_group.is_some_and(|group| group.empty)
                || (piece.kind.is_some()
                    && previous.kind.is_some()
                    && joins_previous(previous, piece));
            if previous.kind.is_none() || structural_break || (!joins && piece.newlines_before > 0)
            {
                result.push('\n');
                if piece.newlines_before > 1 {
                    result.push('\n');
                }
                let continuation = usize::from(piece.kind == Some(Dot));
                for _ in 0..indent + continuation {
                    result.push_str("    ");
                }
            } else if !closing_group.is_some_and(|group| group.import && previous.kind.is_some())
                && !tight_space(previous, piece)
            {
                result.push(' ');
            }
        }
        result.push_str(piece.text);
        if piece.opens_group() {
            let group = groups[index];
            indent += usize::from(group.multiline);
            stack.push(group);
        }
    }
    if !pieces.is_empty() {
        result.push('\n');
    }
    result
}

fn joins_previous(left: Piece<'_>, right: Piece<'_>) -> bool {
    // Preserve manual continuation lines, except at punctuation that attaches
    // to its neighbor. A comment always takes precedence over these joins.
    matches!(right.kind, Some(Else | Catch | Semicolon | Comma))
        || (right.kind == Some(LeftBrace) && right.role == Role::Block)
        || right.role == Role::ShiftEnd
        || right.role == Role::OptionalCast
        || (left.role == Role::Prefix && right.text == "mut")
}

fn tight_space(left: Piece<'_>, right: Piece<'_>) -> bool {
    let (Some(left_kind), Some(right_kind)) = (left.kind, right.kind) else {
        return false;
    };
    if left_kind == Question && right_kind == Question
        || left.role == Role::LambdaOpen && right.role == Role::LambdaClose
    {
        // These pairs would lex as a single operator if joined.
        return false;
    }
    if matches!(right.role, Role::ShiftEnd | Role::OptionalCast)
        || matches!(
            right_kind,
            RightParen | RightBracket | Comma | Semicolon | Colon
        )
        || matches!(left_kind, LeftParen | LeftBracket | Hash)
        || right.role == Role::LambdaClose
        || left.role == Role::LambdaOpen
    {
        return true;
    }
    if matches!(right_kind, Dot | DoubleColon) || matches!(left_kind, Dot | DoubleColon) {
        return true;
    }
    if matches!(left.role, Role::Prefix | Role::AngleOpen)
        || matches!(right.role, Role::AngleOpen | Role::AngleClose)
    {
        return true;
    }
    if matches!((left_kind, right_kind), (LeftBrace, RightBrace)) {
        return true;
    }
    if left_kind == LeftBrace && left.role == Role::Import {
        return true;
    }
    if right_kind == Question {
        return true;
    }
    if matches!(right_kind, DotDot | DotDotEqual) || matches!(left_kind, DotDot | DotDotEqual) {
        return !(matches!(left_kind, Comma | LeftBrace) || right_kind == RightBrace);
    }
    if matches!(right_kind, LeftParen | LeftBracket) {
        return left.role != Role::ForIn
            && (matches!(
                left_kind,
                Name | New
                    | RightParen
                    | RightBracket
                    | RightBrace
                    | Question
                    | Await
                    | Integer
                    | FloatLiteral
                    | StringLiteral
                    | CharLiteral
                    | True
                    | False
                    | Null
            ) || left.role == Role::AngleClose);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_syntax_where_whitespace_affects_tokenization() {
        let source = concat!(
            "use A :: { B,C as D };\n",
            "fn test < T : new ( ) > ( ){let mut x=0x1_2;let v: Outer < Inner < T >>=value;",
            "x<<=2;x=x>>1;let less=x<5;let y=foo :: < T >( & mut x,&x);",
            "let cast=y as? Inner < T >; let f=| left,right |{left|right};",
            "let empty=| |{~ -x}; let propagated=call() ? ?;",
            "for i in(0)..=2{foo(i);}}",
        );
        let expected = concat!(
            "use A::{B, C as D};\n",
            "fn test<T: new()>() {\n",
            "    let mut x = 0x1_2;\n",
            "    let v: Outer<Inner<T>> = value;\n",
            "    x <<= 2;\n",
            "    x = x >> 1;\n",
            "    let less = x < 5;\n",
            "    let y = foo::<T>(&mut x, &x);\n",
            "    let cast = y as? Inner<T>;\n",
            "    let f = |left, right| {\n",
            "        left | right\n",
            "    };\n",
            "    let empty = | | {\n",
            "        ~-x\n",
            "    };\n",
            "    let propagated = call()? ?;\n",
            "    for i in (0)..=2 {\n",
            "        foo(i);\n",
            "    }\n",
            "}\n",
        );
        assert_eq!(formatted(source), expected);
    }

    #[test]
    fn preserves_comments_literal_contents_and_intentional_multiline_groups() {
        let source = concat!(
            " /// 文档\nfn main(){\n foo( // inline  \n",
            r###"  [ 1,2 ,
3],
$"escaped {{x}} {   foo($"nested {   x  }") }",
"// text", '\n'
 ); // τέλος
 // detached
}
"###,
        );
        let expected = concat!(
            "/// 文档\n",
            "fn main() {\n",
            "    foo( // inline  \n",
            "        [\n",
            "            1,\n",
            "            2,\n",
            "            3\n",
            "        ],\n",
            "        $\"escaped {{x}} {   foo($\"nested {   x  }\") }\",\n",
            "        \"// text\",\n",
            "        '\\n'\n",
            "    ); // τέλος\n",
            "    // detached\n",
            "}\n",
        );
        assert_eq!(formatted(source), expected);
    }

    fn formatted(source: &str) -> String {
        let result = format_source(source).unwrap();
        assert_eq!(format_source(&result).unwrap(), result);
        result
    }
}
