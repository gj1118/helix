;; Harneet Programming Language - Syntax Highlighting for Helix Editor

;; Keywords
[
  "var"
  "const"
  "type"
  "enum"
  "function"
  "return"
  "defer"
  "if"
  "else"
  "for"
  "in"
  "switch"
  "case"
  "default"
  "match"
  "import"
  "as"
  "go"
  "do"
  "package"
  "struct"
  "interface"
  "map"
  "and"
  "or"
  "not"
  "receive"
  "send"
] @keyword

;; Statements (for break/continue which are anonymous)
(break_statement) @keyword
(continue_statement) @keyword

;; Types
(simple_type) @type.builtin

;; Literals
(boolean) @constant.builtin
(none) @constant.builtin
(number) @constant.numeric
(float) @constant.numeric
(string) @string
(rune) @string

;; Comments
(comment) @comment

;; Operators
[
  "="
  ":="
  "+="
  "-="
  "*="
  "/="
  "%="
  "+"
  "-"
  "*"
  "/"
  "%"
  "=="
  "!="
  "<"
  ">"
  "<="
  ">="
  "!"
  "=>"
  "|>"
] @operator

;; Punctuation
[
  ";"
  ":"
  ","
  "."
] @punctuation.delimiter

[
  "("
  ")"
  "{"
  "}"
  "["
  "]"
] @punctuation.bracket

;; Functions
(function_declaration name: (identifier) @function)

;; Simple function calls: foo(...)
(call_expression
  function: (expression
    (identifier) @function.builtin))

;; Method calls: module.Func(...)
(call_expression
  function: (expression
    (member_expression
      object: (expression
        (identifier) @namespace)
      property: (identifier) @function.method)))

;; General member access: module.property (fallback for non-call contexts)
(member_expression
  object: (expression
    (identifier) @namespace)
  property: (identifier) @variable.other.member)

;; Variables
(variable_declaration name: (identifier) @variable)
(typed_var_spec name: (identifier) @variable)
(typed_var_spec type: (_) @type)
(multi_var_shared_type name: (identifier) @variable)
(multi_var_shared_type type: (_) @type)
(const_declaration name: (identifier) @constant)
(parameter name: (identifier) @variable.parameter)

;; Typed literals
(typed_slice_literal element_type: (_) @type)
(typed_map_literal key_type: (_) @type)
(typed_map_literal value_type: (_) @type)
(typed_array_literal element_type: (_) @type)

;; User-defined types
(type_declaration name: (identifier) @type)
(struct_literal type: (identifier) @type)
(interface_type) @type
(struct_type) @type

;; Error
(ERROR) @error