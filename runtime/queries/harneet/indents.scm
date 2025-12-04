;; Harneet Programming Language - Indentation Rules

;; Indent triggers
[
  (block)
  (function_declaration)
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
] @indent

;; Outdent triggers
["}" "]" ")"] @outdent

;; Extend
"else" @extend
