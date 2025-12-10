;; Harneet Programming Language - Indentation Rules for Helix Editor
;; Tree-sitter indentation queries matching grammar.js

;; ----------------------
;; Indent triggers
;; ----------------------

;; Block-like constructs that increase indentation
[
  (block)
  (function_declaration)
  (method_declaration)
  (anonymous_function)
  (if_statement)
  (for_statement)
  (for_in_statement)
  (switch_statement)
  (case_clause)
  (default_clause)
  (match_expression)
  (match_arm)
  (struct_type)
  (interface_type)
  (enum_declaration)
  (array_literal)
  (map_literal)
  (struct_literal)
  (typed_array_literal)
] @indent

;; ----------------------
;; Outdent triggers
;; ----------------------

;; Closing braces decrease indentation
[
  "}"
  "]"
  ")"
] @outdent

;; ----------------------
;; Special cases
;; ----------------------

;; Case/default in switch - outdent the keyword
(case_clause "case" @outdent)
(default_clause "default" @outdent)

;; Match arms
(match_arm "=>" @indent)

;; Else on same line as closing brace
"else" @extend

;; ----------------------
;; Multi-line constructs
;; ----------------------

;; Parameter lists
(parameter_list
  "(" @indent
  ")" @outdent)

;; Argument lists
(argument_list
  "(" @indent
  ")" @outdent)

;; Tuple types
(tuple_type
  "(" @indent
  ")" @outdent)