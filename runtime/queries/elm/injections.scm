; Parse glsl where defined

((glsl_content) @injection.content
 (#set! injection.language "glsl"))

([(line_comment) (block_comment)] @injection.content
 (#set! injection.language "comment"))
