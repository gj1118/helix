; Harneet Programming Language - Syntax Highlighting
; Based on Go's highlighting style

; Identifiers

(identifier) @variable

(package_declaration name: (identifier) @namespace)

(const_declaration
  name: (identifier) @constant)

(struct_field
  name: (identifier) @variable.other.member)

(struct_field_value
  name: (identifier) @variable.other.member)

(parameter name: (identifier) @variable.parameter)

(enum_variant name: (identifier) @constant)

; Function calls

(call_expression
  function: (expression (identifier) @function))

(call_expression
  function: (expression
    (member_expression
      property: (identifier) @function.method)))

; Types

(simple_type) @type.builtin

(type_declaration name: (identifier) @type)
(enum_declaration name: (identifier) @type)
(struct_literal type: (identifier) @type)

; Function definitions

(function_declaration
  name: (identifier) @function)

; Arrow function variable assignments (var double = x => ...)
(variable_declaration
  name: (identifier) @function
  value: (expression (arrow_function)))

(variable_declaration
  name: (identifier) @function
  value: (expression (anonymous_function)))

; Operators

[
  "-"
  "-="
  ":="
  "!"
  "!="
  "*"
  "*="
  "/"
  "/="
  "&"
  "%"
  "%="
  "^"
  "+"
  "+="
  "<"
  "<<"
  "<="
  "="
  "=="
  ">"
  ">="
  ">>"
  "|"
  "**"
  "=>"
  "and"
  "or"
  "not"
] @operator

; Keywords

[
  "type"
] @keyword

[
  "defer"
  "go"
] @keyword.control

[
  "if"
  "else"
  "switch"
  "case"
  "default"
  "match"
] @keyword.control.conditional

[
  "for"
  "in"
] @keyword.control.repeat

[
  "import"
  "package"
  "as"
] @keyword.control.import

[
  "return"
] @keyword.control.return

; Break and continue are statement nodes
(break_statement) @keyword.control.return
(continue_statement) @keyword.control.return

[
  "function"
] @keyword.function

[
  "var"
  "interface"
  "map"
  "struct"
  "enum"
] @keyword.storage.type

[
  "const"
] @keyword.storage.modifier

; Delimiters

[
  ":"
  "."
  ","
  ";"
] @punctuation.delimiter

[
  "("
  ")"
  "["
  "]"
  "{"
  "}"
] @punctuation.bracket

; Literals

(string) @string

(rune) @constant.character

(number) @constant.numeric.integer

(float) @constant.numeric.float

(boolean) @constant.builtin.boolean

(none) @constant.builtin

(blank_identifier) @variable.builtin

; Comments

(comment) @comment

; Imports

(import_spec path: (identifier) @namespace)
(import_spec path: (string) @string)
(import_spec alias: (identifier) @namespace)

; Member access

(member_expression property: (identifier) @variable.other.member)

; Error

(ERROR) @error
