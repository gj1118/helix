;; Harneet Programming Language - Local Scope Queries

;; Scopes
(block) @local.scope
(function_declaration) @local.scope
(anonymous_function) @local.scope
(arrow_function) @local.scope
(for_statement) @local.scope
(for_in_statement) @local.scope
(if_statement) @local.scope
(switch_statement) @local.scope
(match_expression) @local.scope

;; Definitions
(variable_declaration name: (identifier) @local.definition.var)
(const_declaration name: (identifier) @local.definition.constant)
(function_declaration name: (identifier) @local.definition.function)
(parameter name: (identifier) @local.definition.parameter)
(for_in_statement left: (identifier) @local.definition.var)

;; Type Definitions
(type_declaration name: (identifier) @local.definition.type)
(enum_declaration name: (identifier) @local.definition.type)
(enum_variant name: (identifier) @local.definition.constant)

;; Import Definitions
(import_spec path: (identifier) @local.definition.import)
(import_spec alias: (identifier) @local.definition.import)

;; References
(identifier) @local.reference
