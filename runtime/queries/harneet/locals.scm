;; Harneet Programming Language - Local Scope Queries for Helix Editor
;; Tree-sitter locals queries for semantic highlighting and navigation

;; Scopes
(block) @local.scope
(function_declaration) @local.scope
(anonymous_function) @local.scope
(arrow_function) @local.scope

;; Definitions
(variable_declaration name: (identifier) @local.definition)
(const_declaration name: (identifier) @local.definition)
(function_declaration name: (identifier) @local.definition)
(parameter name: (identifier) @local.definition)

;; References
(identifier) @local.reference