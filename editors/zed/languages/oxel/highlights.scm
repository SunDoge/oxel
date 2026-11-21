(line_comment) @comment
(doc_comment) @comment.doc

[(string) (character) (interpolated_string)] @string
(interpolation) @embedded
(escape) @string.escape
(number) @number
(boolean) @boolean
(null) @constant.builtin

[
  "namespace" "use" "const" "internal" "comptime" "async" "extern"
  "fn" "struct" "enum" "interface" "impl" "for" "as" "new"
  (keyword) (passing_mode)
] @keyword

(primitive_type) @type.builtin
(type name: (path (identifier) @type))
(type_parameter name: (identifier) @type.parameter)
(namespace_declaration name: (path (identifier) @namespace))

(function_declaration name: (identifier) @function)
(parameter name: (identifier) @variable.parameter)
(struct_declaration name: (identifier) @type)
(enum_declaration name: (identifier) @enum)
(interface_declaration name: (identifier) @type)
(field_declaration name: (identifier) @property)
(variant name: (identifier) @constructor)
(constant_declaration name: (identifier) @constant)
(attribute name: (identifier) @attribute)

(operator) @operator
["=" "->" "+" "?"] @operator
["(" ")" "[" "]" "{" "}"] @punctuation.bracket
["," ";" ":" "." "::"] @punctuation.delimiter
