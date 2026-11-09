; Task tags in comments (`TODO:`, `FIXME(user):`, …), used by the task-tag
; pickers (`todos`, `workspace-todos`). The keywords are the same as in
; `highlights.scm`; see the "Comment annotations" section in the theme docs
; for the full list.
;
; `@comment.tag` captures the keyword (or a bare colonization-less keyword,
; parsed as a standalone "text" token, just like in the highlighting), and
; `@comment.user` captures the optional `(user)` between keyword and colon.
; The pickers derive the severity key for a keyword themselves, so no
; severity-specific captures are needed here.

((tag (name) @comment.tag (user)? @comment.user)
 (#any-of? @comment.tag
  "BROKEN" "BUG" "COMPLIANCE" "DEBUG" "FAIL" "FAILED" "FIX" "FIXIT" "FIXME" "INVARIANT" "ISSUE" "PANIC" "SAFETY" "SECURITY" "XXX"
  "HACK" "WORKAROUND" "TEMP" "TEST" "UNTESTED" "WARN" "WARNING"
  "INFO" "NOTE" "NOTES" "OPTIMISE" "OPTIMIZE" "PERF" "PERFORMANCE" "ASK" "QUESTION" "REFACTOR" "REVIEW" "CR" "PR" "TO-DO" "TODO" "UNDONE"
  "DONE" "HINT" "BOOKMARK" "MARK" "MARKER" "PASSED" "MOCK" "STUB" "TIP"))

("text" @comment.tag
 (#any-of? @comment.tag
  "BROKEN" "BUG" "COMPLIANCE" "DEBUG" "FAIL" "FAILED" "FIX" "FIXIT" "FIXME" "INVARIANT" "ISSUE" "PANIC" "SAFETY" "SECURITY" "XXX"
  "HACK" "WORKAROUND" "TEMP" "TEST" "UNTESTED" "WARN" "WARNING"
  "INFO" "NOTE" "NOTES" "OPTIMISE" "OPTIMIZE" "PERF" "PERFORMANCE" "ASK" "QUESTION" "REFACTOR" "REVIEW" "CR" "PR" "TO-DO" "TODO" "UNDONE"
  "DONE" "HINT" "BOOKMARK" "MARK" "MARKER" "PASSED" "MOCK" "STUB" "TIP"))
