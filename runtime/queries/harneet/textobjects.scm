;; Harneet Programming Language - Text Objects

;; Functions
(function_declaration) @function.around
(function_declaration body: (block) @function.inside)
(anonymous_function) @function.around
(anonymous_function body: (block) @function.inside)
(arrow_function) @function.around

;; Types/Classes
(type_declaration) @class.around
(enum_declaration) @class.around

;; Comments
(comment) @comment.around

;; Parameters
(parameter_list) @parameter.around
(parameter) @parameter.inside
(argument_list) @parameter.around

;; Control Flow
(if_statement) @conditional.around
(if_statement consequence: (block) @conditional.inside)
(switch_statement) @conditional.around
(match_expression) @conditional.around

;; Loops
(for_statement) @loop.around
(for_statement body: (block) @loop.inside)
(for_in_statement) @loop.around
(for_in_statement body: (block) @loop.inside)

;; Blocks
(block) @block.around
