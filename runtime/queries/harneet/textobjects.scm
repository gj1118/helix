;; Harneet Programming Language - Text Objects for Helix Editor
;; Tree-sitter text object queries matching grammar.js

;; ----------------------
;; Functions
;; ----------------------

;; Function declarations
(function_declaration) @function.around
(function_declaration body: (block) @function.inside)

;; Method declarations
(method_declaration) @function.around
(method_declaration body: (block) @function.inside)

;; Anonymous functions
(anonymous_function) @function.around
(anonymous_function body: (block) @function.inside)

;; Arrow functions
(arrow_function) @function.around

;; Function calls
(call_expression) @function.around
(call_expression arguments: (argument_list) @function.inside)

;; ----------------------
;; Types/Classes
;; ----------------------

;; Type declarations (struct, interface)
(type_declaration) @class.around
(type_declaration (struct_type) @class.inside)
(type_declaration (interface_type) @class.inside)

;; Enum declarations
(enum_declaration) @class.around

;; ----------------------
;; Comments
;; ----------------------

(comment) @comment.around

;; ----------------------
;; Parameters and Arguments
;; ----------------------

;; Parameters
(parameter_list) @parameter.around
(parameter) @parameter.inside

;; Arguments
(argument_list) @parameter.around

;; ----------------------
;; Control Flow
;; ----------------------

;; If statements
(if_statement) @conditional.around
(if_statement consequence: (block) @conditional.inside)

;; Switch statements
(switch_statement) @conditional.around
(case_clause) @conditional.inside
(default_clause) @conditional.inside

;; Match expressions
(match_expression) @conditional.around
(match_arm) @conditional.inside

;; ----------------------
;; Loops
;; ----------------------

;; For loops
(for_statement) @loop.around
(for_statement body: (block) @loop.inside)

;; For-in loops
(for_in_statement) @loop.around
(for_in_statement body: (block) @loop.inside)

;; ----------------------
;; Blocks
;; ----------------------

(block) @block.around

;; ----------------------
;; Entries (for maps, structs, etc.)
;; ----------------------

(map_entry) @entry.around
(struct_field) @entry.around
(struct_field_value) @entry.around
(enum_variant) @entry.around