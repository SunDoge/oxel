(namespace_declaration "namespace" @context name: (path) @name) @item
(function_declaration "fn" @context name: (identifier) @name) @item
(constant_declaration "const" @context name: (identifier) @name) @item
(struct_declaration "struct" @context name: (identifier) @name) @item
(enum_declaration "enum" @context name: (identifier) @name) @item
(interface_declaration "interface" @context name: (identifier) @name) @item
(variant name: (identifier) @name) @item
(implementation "impl" @context interface: (type) @name "for" @context target: (type) @name) @item
