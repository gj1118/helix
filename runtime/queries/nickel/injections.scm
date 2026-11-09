(annot_atom doc: (static_string)
            @injection.content
            (#set! injection.language "markdown"))

((comment) @injection.content
 (#set! injection.language "comment"))
