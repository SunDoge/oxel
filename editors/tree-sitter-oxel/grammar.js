// Declaration structure and balanced token trees for Oxel editor features.
// The Oxel compiler parses expressions and reports syntax errors.
module.exports = grammar({
  name: "oxel",

  extras: $ => [/\s/, $.line_comment, $.doc_comment],
  word: $ => $.identifier,

  rules: {
    source_file: $ => repeat(choice(
      $.namespace_declaration,
      $.use_declaration,
      $.constant_declaration,
      $.function_declaration,
      $.struct_declaration,
      $.enum_declaration,
      $.interface_declaration,
      $.implementation,
      $.attribute,
    )),

    namespace_declaration: $ => seq("namespace", field("name", $.path), ";"),
    use_declaration: $ => seq("use", repeat1($._token_tree), ";"),
    constant_declaration: $ => seq(
      "const",
      field("name", $.identifier),
      optional(seq(":", field("type", $.type))),
      "=",
      repeat1($._token_tree),
      ";",
    ),

    function_declaration: $ => seq(
      optional("internal"),
      optional(choice("comptime", "async", seq("extern", $.string))),
      "fn",
      field("name", $.identifier),
      optional($.type_parameters),
      field("parameters", $.parameters),
      optional(seq("->", field("result", $.type))),
      choice(
        field("body", $.block),
        seq("=", $.string, optional(seq("as", $.string)), ";"),
        ";",
      ),
    ),
    parameters: $ => seq("(", commaList($.parameter), ")"),
    parameter: $ => seq(
      optional($.passing_mode),
      field("name", choice($.identifier, alias($.passing_mode, $.identifier))),
      ":",
      field("type", $.type),
    ),
    passing_mode: _ => choice(
      "ref", "out", "in", "utf8", "buffer", "buffer32", "buffer64",
      "buffer_mut", "buffer_mut32", "buffer_mut64", "callback",
      "callback_retained", "callback_root",
    ),

    struct_declaration: $ => seq(
      optional("internal"), "struct", field("name", $.identifier),
      optional($.type_parameters), field("body", $.struct_body),
    ),
    struct_body: $ => seq("{", commaList($.field_declaration), "}"),
    field_declaration: $ => seq(
      field("name", $.identifier), ":", field("type", $.type),
    ),

    enum_declaration: $ => seq(
      optional("internal"), "enum", field("name", $.identifier),
      optional($.type_parameters), field("body", $.enum_body),
    ),
    enum_body: $ => seq("{", commaList($.variant), "}"),
    variant: $ => seq(
      field("name", $.identifier),
      optional(seq("(", commaList($.type), ")")),
    ),

    interface_declaration: $ => seq(
      optional("internal"), "interface", field("name", $.identifier),
      optional($.type_parameters), field("body", $.method_body),
    ),
    implementation: $ => seq(
      "impl", optional($.type_parameters), field("interface", $.type),
      "for", field("target", $.type), field("body", $.method_body),
    ),
    method_body: $ => seq("{", repeat($.function_declaration), "}"),

    type_parameters: $ => seq("<", commaList($.type_parameter), ">"),
    type_parameter: $ => seq(
      field("name", $.identifier),
      optional(seq(":", $._bound, repeat(seq("+", $._bound)))),
    ),
    _bound: $ => choice($.type, "struct", seq("new", "(", ")")),
    type_arguments: $ => seq("<", commaList($.type), ">"),
    type: $ => seq(
      field("name", choice($.primitive_type, $.path)),
      optional($.type_arguments),
      repeat(choice("?", seq("[", "]"))),
    ),
    path: $ => seq($.identifier, repeat(seq(choice("::", "."), $.identifier))),
    primitive_type: _ => choice(
      "bool", "char", "i8", "u8", "i16", "u16", "i32", "u32", "i64", "u64",
      "f32", "f64", "nint", "nuint", "string", "object", "void",
    ),
    attribute: $ => seq("#", "[", field("name", $.identifier), repeat($._token_tree), "]"),

    block: $ => seq("{", repeat(choice($._token_tree, ";")), "}"),
    parenthesized: $ => seq("(", repeat(choice($._token_tree, ";")), ")"),
    bracketed: $ => seq("[", repeat(choice($._token_tree, ";")), "]"),
    _token_tree: $ => choice(
      $.block, $.parenthesized, $.bracketed,
      $.identifier, $.primitive_type, $.keyword,
      $.boolean, $.null, $.number, $.string, $.character, $.interpolated_string,
      $.operator, ",", ".", ":", "::",
    ),
    keyword: _ => choice(
      "let", "mut", "return", "if", "else", "for", "while", "break", "continue",
      "match", "try", "catch", "throw", "using", "new", "await", "as", "is",
      "in", "ref", "out", "async", "comptime", "self",
    ),
    boolean: _ => choice("true", "false"),
    null: _ => "null",
    operator: _ => token(choice(
      "..=", "..", "=>", "->", "<<=", ">>=", "<<", ">>",
      "+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=",
      "==", "!=", "<=", ">=", "&&", "||",
      /[+*/%&|^!<>=?~-]/,
    )),
    identifier: _ => /[a-zA-Z_][a-zA-Z_0-9]*/,
    number: _ => token(choice(
      /0[xX][0-9a-fA-F_]+/,
      /0[bB][01_]+/,
      /0[oO][0-7_]+/,
      /[0-9][0-9_]*(\.[0-9][0-9_]*)?([eE][+-]?[0-9][0-9_]*)?/,
    )),
    string: _ => token(/"([^"\\\r\n]|\\[^\r\n])*"/),
    character: _ => token(/'([^'\\\r\n]|\\[^\r\n])+'/),
    interpolated_string: $ => seq(
      '$"', repeat(choice($.interpolated_text, $.escape, $.interpolation)), '"',
    ),
    interpolated_text: _ => token.immediate(/[^"{}\\\r\n]+/),
    escape: _ => token.immediate(choice(/\\[^\r\n]/, "{{", "}}")),
    interpolation: $ => seq("{", repeat($._token_tree), "}"),
    line_comment: _ => token(seq("//", /[^\r\n]*/)),
    doc_comment: _ => token(prec(1, seq("///", /[^\r\n]*/))),
  },
});

function commaList(rule) {
  return optional(seq(rule, repeat(seq(",", rule)), optional(",")));
}
