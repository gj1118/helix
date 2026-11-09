; Parse JSDoc annotations in comments

((comment) @injection.content
 (#set! injection.language "jsdoc")
 (#match? @injection.content "^/\\*+"))

; Task tags in single line comments

((comment) @injection.content
 (#set! injection.language "comment")
 (#match? @injection.content "^//"))
