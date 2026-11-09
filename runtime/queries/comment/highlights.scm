; Task tags in comments (`TODO:`, `FIXME:`, …), captured per keyword as
; `<severity>.<keyword>`, e.g. `@info.todo` / `@error.fixme`. Themes can style
; each keyword through its full scope; scopes that a theme does not know fall
; back to the matching severity key (`error`, `warning`, `info`, `hint`), and
; unrecognized tags are plain comment text.
;
; Some tags share a key when they mean the same thing (the aliases are listed
; in the theme docs): `OPTIMIZE`/`OPTIMISE` -> `optimize`, `PERF`/`PERFORMANCE`
; -> `perf`, `MARK`/`MARKER`/`BOOKMARK` -> `mark`, etc.
;
; See the "Comment annotations" section in the theme documentation for the
; full list of recognized keywords.

(tag (name) @ui.text
 (user)? @constant)

; Error level keywords: BROKEN BUG COMPLIANCE DEBUG FAIL FIX FIXME INVARIANT ISSUE PANIC SAFETY SECURITY XXX
((tag (name) @error.broken)
 (#any-of? @error.broken "BROKEN"))
("text" @error.broken
 (#any-of? @error.broken "BROKEN"))

((tag (name) @error.bug)
 (#any-of? @error.bug "BUG"))
("text" @error.bug
 (#any-of? @error.bug "BUG"))

((tag (name) @error.compliance)
 (#any-of? @error.compliance "COMPLIANCE"))
("text" @error.compliance
 (#any-of? @error.compliance "COMPLIANCE"))

((tag (name) @error.debug)
 (#any-of? @error.debug "DEBUG"))
("text" @error.debug
 (#any-of? @error.debug "DEBUG"))

((tag (name) @error.fail)
 (#any-of? @error.fail "FAIL" "FAILED"))
("text" @error.fail
 (#any-of? @error.fail "FAIL" "FAILED"))

((tag (name) @error.fix)
 (#any-of? @error.fix "FIX" "FIXIT"))
("text" @error.fix
 (#any-of? @error.fix "FIX" "FIXIT"))

((tag (name) @error.fixme)
 (#any-of? @error.fixme "FIXME"))
("text" @error.fixme
 (#any-of? @error.fixme "FIXME"))

((tag (name) @error.invariant)
 (#any-of? @error.invariant "INVARIANT"))
("text" @error.invariant
 (#any-of? @error.invariant "INVARIANT"))

((tag (name) @error.issue)
 (#any-of? @error.issue "ISSUE"))
("text" @error.issue
 (#any-of? @error.issue "ISSUE"))

((tag (name) @error.panic)
 (#any-of? @error.panic "PANIC"))
("text" @error.panic
 (#any-of? @error.panic "PANIC"))

((tag (name) @error.safety)
 (#any-of? @error.safety "SAFETY"))
("text" @error.safety
 (#any-of? @error.safety "SAFETY"))

((tag (name) @error.security)
 (#any-of? @error.security "SECURITY"))
("text" @error.security
 (#any-of? @error.security "SECURITY"))

((tag (name) @error.xxx)
 (#any-of? @error.xxx "XXX"))
("text" @error.xxx
 (#any-of? @error.xxx "XXX"))


; Warning level keywords: HACK TEMP TEST UNTESTED WARN
((tag (name) @warning.hack)
 (#any-of? @warning.hack "HACK" "WORKAROUND"))
("text" @warning.hack
 (#any-of? @warning.hack "HACK" "WORKAROUND"))

((tag (name) @warning.temp)
 (#any-of? @warning.temp "TEMP"))
("text" @warning.temp
 (#any-of? @warning.temp "TEMP"))

((tag (name) @warning.test)
 (#any-of? @warning.test "TEST"))
("text" @warning.test
 (#any-of? @warning.test "TEST"))

((tag (name) @warning.untested)
 (#any-of? @warning.untested "UNTESTED"))
("text" @warning.untested
 (#any-of? @warning.untested "UNTESTED"))

((tag (name) @warning.warn)
 (#any-of? @warning.warn "WARN" "WARNING"))
("text" @warning.warn
 (#any-of? @warning.warn "WARN" "WARNING"))


; Info level keywords: INFO NOTE OPTIMIZE PERF QUESTION REFACTOR REVIEW TODO UNDONE
((tag (name) @info.info)
 (#any-of? @info.info "INFO"))
("text" @info.info
 (#any-of? @info.info "INFO"))

((tag (name) @info.note)
 (#any-of? @info.note "NOTE" "NOTES"))
("text" @info.note
 (#any-of? @info.note "NOTE" "NOTES"))

((tag (name) @info.optimize)
 (#any-of? @info.optimize "OPTIMIZE" "OPTIMISE"))
("text" @info.optimize
 (#any-of? @info.optimize "OPTIMIZE" "OPTIMISE"))

((tag (name) @info.perf)
 (#any-of? @info.perf "PERF" "PERFORMANCE"))
("text" @info.perf
 (#any-of? @info.perf "PERF" "PERFORMANCE"))

((tag (name) @info.question)
 (#any-of? @info.question "QUESTION" "ASK"))
("text" @info.question
 (#any-of? @info.question "QUESTION" "ASK"))

((tag (name) @info.refactor)
 (#any-of? @info.refactor "REFACTOR"))
("text" @info.refactor
 (#any-of? @info.refactor "REFACTOR"))

((tag (name) @info.review)
 (#any-of? @info.review "REVIEW" "PR" "CR"))
("text" @info.review
 (#any-of? @info.review "REVIEW" "PR" "CR"))

((tag (name) @info.todo)
 (#any-of? @info.todo "TODO" "TO-DO"))
("text" @info.todo
 (#any-of? @info.todo "TODO" "TO-DO"))

((tag (name) @info.undone)
 (#any-of? @info.undone "UNDONE"))
("text" @info.undone
 (#any-of? @info.undone "UNDONE"))


; Hint level keywords: DONE HINT MARK PASSED STUB TIP
((tag (name) @hint.done)
 (#any-of? @hint.done "DONE"))
("text" @hint.done
 (#any-of? @hint.done "DONE"))

((tag (name) @hint.hint)
 (#any-of? @hint.hint "HINT"))
("text" @hint.hint
 (#any-of? @hint.hint "HINT"))

((tag (name) @hint.mark)
 (#any-of? @hint.mark "MARK" "MARKER" "BOOKMARK"))
("text" @hint.mark
 (#any-of? @hint.mark "MARK" "MARKER" "BOOKMARK"))

((tag (name) @hint.passed)
 (#any-of? @hint.passed "PASSED"))
("text" @hint.passed
 (#any-of? @hint.passed "PASSED"))

((tag (name) @hint.stub)
 (#any-of? @hint.stub "STUB" "MOCK"))
("text" @hint.stub
 (#any-of? @hint.stub "STUB" "MOCK"))

((tag (name) @hint.tip)
 (#any-of? @hint.tip "TIP"))
("text" @hint.tip
 (#any-of? @hint.tip "TIP"))
; Issue number (#123)
("text" @constant.numeric
 (#match? @constant.numeric "^#[0-9]+$"))

; User mention (@user)
("text" @tag
 (#match? @tag "^[@][a-zA-Z0-9_-]+$"))

(uri) @markup.link.url
