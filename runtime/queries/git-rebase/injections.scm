(((command) @attribute
  (message)? @injection.content)
 (#match? @attribute "^(x|exec)$")
 (#set! injection.language "bash")
)

((comment) @injection.content
 (#set! injection.language "comment"))
