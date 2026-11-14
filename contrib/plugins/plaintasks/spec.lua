-- Tests of the plaintasks modules (run by `cargo test -p helix-plugin plaintasks`): plain Lua,
-- no editor. Each check asserts the type and the value.

local syntax = require("plaintasks.syntax")
local dates = require("plaintasks.dates")
local tasks = require("plaintasks.tasks")
local links = require("plaintasks.links")

local function show(value)
  if type(value) == "table" then
    local parts = {}
    for k, v in pairs(value) do
      table.insert(parts, tostring(k) .. "=" .. show(v))
    end
    table.sort(parts)
    return "{" .. table.concat(parts, ", ") .. "}"
  end
  return string.format("%q", value)
end

-- Whether `a` and `b` are equal, tables by their contents.
local function same(a, b)
  if type(a) ~= type(b) then
    return false
  end
  if type(a) ~= "table" then
    -- an integer is not a float, even of the same value
    return a == b and math.type(a) == math.type(b)
  end
  for k, v in pairs(a) do
    if not same(v, b[k]) then
      return false
    end
  end
  for k in pairs(b) do
    if a[k] == nil then
      return false
    end
  end
  return true
end

-- Asserts that `actual` has the type and the value of `expected`.
local function check(actual, expected, what)
  assert(type(actual) == type(expected), string.format(
    "%s: a %s, not a %s (%s)", what or "value", type(expected), type(actual), show(actual)))
  assert(same(actual, expected), string.format(
    "%s: %s, not %s", what or "value", show(expected), show(actual)))
end

local cfg = {
  open_tasks_bullet = "☐",
  done_tasks_bullet = "✔",
  cancelled_tasks_bullet = "✘",
  before_tasks_bullet_margin = 1,
  tasks_bullet_space = " ",
  date_format = "(%y-%m-%d %H:%M)",
  before_date_space = " ",
  done_tag = true,
  done_date = true,
  header_to_task = false,
  archive_name = "Archive:",
  project_tag = true,
  new_on_top = true,
  archive_org_filemask = "{dir}{sep}{base}_archive{ext}",
  decimal_minutes = false,
  highlight_due_soon = 24,
  stats_format = "$n/$a done ($percent%) $progress Last task @done $last",
  stats_ignore_archive = false,
  bar_full = "■",
  bar_empty = "□",
}
syntax.setup(cfg)

local NOW = os.time({ year = 2016, month = 12, day = 31, hour = 23, min = 0, sec = 0 })
local now_date = dates.date_of(NOW)

local function date(y, mo, d, h, mi)
  return { year = y, month = mo, day = d, hour = h, min = mi }
end

-- syntax ---------------------------------------------------------------------------------------

local kinds = {
  { "Errands:", "project" },
  { "  Sub project: @today", "project" },
  { "# Heading:", "project" },
  { " ☐ buy milk", "pending" },
  { " - buy milk", "pending" },
  { " [ ] buy milk", "pending" },
  { " ✔ buy milk @done (16-12-31 22:00)", "done" },
  { " + buy milk", "done" },
  { " [x] buy milk", "done" },
  { " - buy milk @done(2016-12-31)", "done" },
  { " ✘ buy milk @cancelled (16-12-31 22:00)", "cancelled" },
  { " x buy milk", "cancelled" },
  { " [-] buy milk", "cancelled" },
  { " - buy milk @cancelled", "cancelled" },
  { "   a note", "note" },
  { "+1 for this", "note" },
  { "--- ✄ -----------------------", "separator" },
  { "------------", "separator" },
  { "＿＿＿＿＿＿", "archive" },
  { "   ", "blank" },
  { "", "blank" },
  { " ☐ ends with a colon:", "pending" },
}
for _, case in ipairs(kinds) do
  check(syntax.classify(case[1]).kind, case[2], case[1])
end

local info = syntax.classify("  ☐  two spaces")
check(info.indent, "  ", "indent")
check(info.bullet, "☐", "bullet")
check(info.text, "two spaces", "text")
check(info.text_at, 3 + #"☐" + 2, "text_at")

local tags = syntax.tags(" ☐ pay @due(16-12-31 10:00) @high a@b @x-y")
check(#tags, 3, "tags")
check(tags[1].name, "due")
check(tags[1].value, "16-12-31 10:00")
check(tags[2].name, "high")
check(tags[3].name, "x-y")
check(syntax.has_tag("task @done (x)", "done"), true)
check(syntax.has_tag("task @doneish", "done"), false)
check(syntax.char_col("☐ ab", #"☐" + 2), 2, "char_col")

check(syntax.lines("a\r\nb\n"), { "a", "b" }, "lines")
check(syntax.lines("a\n\nb"), { "a", "", "b" }, "lines")

local _, infos = tasks.parse("Work:\n  Errands:\n    ☐ milk\n  ☐ report\n")
check(syntax.project_path(infos, 3), { "Work", "Errands" }, "project path")
check(syntax.project_path(infos, 4), { "Work" }, "project path")
check(syntax.block_end(infos, 1), 4, "block end")

-- dates: the cases of PlainTasks's own tests (now: 2016-12-31 23:00) ------------------------

check(dates.format(NOW, cfg.date_format), "(16-12-31 23:00)", "format")
check(dates.strptime("(16-12-31 23:00)", cfg.date_format), date(2016, 12, 31, 23, 0), "strptime")
check(dates.strptime("(16-12-31)", cfg.date_format), nil, "strptime of another format")
check(dates.strptime("(16-02-30 10:00)", cfg.date_format), nil, "February 30th")

local parse_cases = {
  { "1-1", date(2017, 1, 1, 23, 0) },
  { "1 1:", date(2017, 1, 1, 1, 0) },
  { "3", date(2017, 1, 3, 23, 0) },
  { "31", date(2017, 1, 31, 23, 0) },
  { "0", date(2016, 12, 31, 23, 0) },
  { "", date(2016, 12, 31, 23, 0) },
  { "--1", date(2016, 12, 1, 23, 0) },
  { "2-3", date(2017, 2, 3, 23, 0) },
  { "31 10:", date(2017, 1, 31, 10, 0) },
  { "16.1.1 1:1", date(2016, 1, 1, 1, 1) },
  { "1.1.16", date(2001, 1, 16, 23, 0) },
  { "2003-Sep-25", date(2003, 9, 25, 23, 0) },
  { "16-12-31 10:00", date(2016, 12, 31, 10, 0) },
}
for _, case in ipairs(parse_cases) do
  check(dates.parse(case[1], cfg.date_format, now_date), case[2], "parse " .. case[1])
end
local day_first = "(%d-%m-%y %H:%M)"
check(dates.parse("1.1.16", day_first, now_date), date(2016, 1, 1, 23, 0), "day first")
check(dates.parse("4-1-16", day_first, now_date), date(2016, 1, 4, 23, 0), "day first")
check(dates.parse("4-1", day_first, now_date), date(2017, 4, 1, 23, 0), "day first short")
local missing, err = dates.parse("2-30", cfg.date_format, now_date)
check(missing, nil, "February 30th")
check(type(err), "string", "error")

local created = date(2016, 12, 1, 23, 0)
local increase_cases = {
  { "+", date(2017, 1, 1, 23, 0) },
  { "+hey", date(2017, 1, 1, 23, 0) },
  { "+1d", date(2017, 1, 1, 23, 0) },
  { "+w", date(2017, 1, 7, 23, 0) },
  { "+3w", date(2017, 1, 21, 23, 0) },
  { "+33.", date(2017, 1, 2, 8, 0) },
  { "+33.55", date(2017, 1, 2, 8, 55) },
  { "+2:", date(2017, 1, 1, 1, 0) },
  { "+:555", date(2017, 1, 1, 8, 15) },
  { "+2 12:", date(2017, 1, 3, 11, 0) },
  { "+555", date(2018, 7, 9, 23, 0) },
  { "++", date(2016, 12, 2, 23, 0), created },
  { "++4w", date(2016, 12, 29, 23, 0), created },
}
for _, case in ipairs(increase_cases) do
  check(dates.increase(case[1], now_date, case[3]), case[2], "increase " .. case[1])
end

check(dates.format_delta(2 * 86400 + 3 * 3600 + 4 * 60), "2 days, 3:04", "delta")
check(dates.format_delta(86400), "1 day", "delta")
check(dates.format_delta(5 * 60), "0:05", "delta")
check(dates.format_delta(30), "", "delta")
check(dates.format_delta(86400 + 5400, true), "1 day, 1.50", "decimal delta")
check(dates.parse_delta("2 days, 3:04"), 2 * 86400 + 3 * 3600 + 4 * 60, "parse delta")
check(dates.parse_delta("1:30:15"), 5415, "parse delta")
check(dates.parse_delta("1.50"), 5400, "parse delta")
check(dates.parse_delta("a bit"), 0, "parse delta")
check(dates.spent(0, { 3600, 7200 }, 10800), 7200, "spent with a pause")

-- tasks ----------------------------------------------------------------------------------------

local function toggled(text, index, action)
  local lines, list_infos = tasks.parse(text)
  return tasks.toggle(lines, list_infos, index, action, cfg, NOW)
end

check(tasks.stamp("@done", cfg, NOW), " @done (16-12-31 23:00)", "stamp")
check(toggled(" ☐ buy milk", 1, "done"), " ✔ buy milk @done (16-12-31 23:00)", "done")
check(toggled(" ✔ buy milk @done (16-12-31 22:00)", 1, "done"), " ☐ buy milk", "undone")
check(toggled(" ☐ buy milk", 1, "cancelled"), " ✘ buy milk @cancelled (16-12-31 23:00)", "cancelled")
check(toggled(" ✘ buy milk @cancelled (16-12-31 22:00)", 1, "cancelled"), " ☐ buy milk", "uncancelled")
check(toggled(" ✘ buy milk @cancelled (16-12-31 22:00)", 1, "done"), " ✔ buy milk @done (16-12-31 23:00)", "cancelled then done")
local refused, why = toggled(" ✔ buy milk @done (16-12-31 22:00)", 1, "cancelled")
check(refused, nil, "cancel a done task")
check(why, "You cannot cancel what have been done, can you?", "why")
check(toggled(" ☐ line break  ", 1, "done"), " ✔ line break @done (16-12-31 23:00)  ", "double space kept")
check(toggled(" ☐ write @started(16-12-31 20:00)", 1, "done"),
  " ✔ write @started(16-12-31 20:00) @done (16-12-31 23:00) @lasted(3:00)", "lasted")
check(toggled(" ☐ write @started(16-12-31 20:00) @toggle(16-12-31 21:00) @toggle(16-12-31 22:00)", 1, "cancelled"),
  " ✘ write @started(16-12-31 20:00) @toggle(16-12-31 21:00) @toggle(16-12-31 22:00) @cancelled (16-12-31 23:00) @wasted(2:00)",
  "wasted with a pause")
check(toggled(" ☐ quick @started(16-12-31 23:00)", 1, "done"),
  " ✔ quick @started(16-12-31 23:00) @done (16-12-31 23:00) @lasted(a bit)", "a bit")
check(toggled("Work:\n ✔ a @lasted(1:00)\n ✘ b @wasted(0:30)", 1, "done"),
  "✔ Work: @done (16-12-31 23:00) @total(1:30)", "project done")
check(toggled(" ✔ archived @done (16-12-31 22:00) @project(Work)", 1, "done"), " ☐ archived", "undone from the archive")

local _, recalc_infos = tasks.parse(" ✔ write @started(16-12-31 20:00) @done (16-12-31 22:30) @lasted(9:00)")
check(tasks.recalculate({ " ✔ write @started(16-12-31 20:00) @done (16-12-31 22:30) @lasted(9:00)" }, recalc_infos, 1, cfg),
  " ✔ write @started(16-12-31 20:00) @done (16-12-31 22:30) @lasted(2:30)", "recalculated")

local function new_task(line)
  return tasks.new_task(line, syntax.classify(line), cfg)
end
local new_lines, col = new_task(" ☐ buy milk")
check(new_lines, { " ☐ buy milk", " ☐ " }, "new after a task")
check(col, 3, "cursor")
check(new_task("Errands:"), { "Errands:", " ☐ " }, "new under a project")
check(new_task("  a note"), { "  ☐ a note" }, "a note becomes a task")
check(new_task("a note"), { " ☐ a note" }, "a note becomes a task (margin)")
check(new_task(""), { " ☐ " }, "new on a blank line")

local list = {
  "Work:",
  " ☐ report",
  " ✔ mail @done (16-12-30 10:00)",
  "   a note of the mail",
  "  Errands:",
  "   ✘ bike @cancelled (16-12-31 10:00)",
  "",
}
local archived, moved = tasks.archive(list, cfg)
check(moved, 2, "moved")
check(archived, {
  "Work:",
  " ☐ report",
  "  Errands:",
  "",
  "＿＿＿＿＿＿＿＿＿＿＿＿＿＿＿＿＿＿＿",
  "Archive:",
  " ✘ bike @cancelled (16-12-31 10:00) @project(Work / Errands)",
  " ✔ mail @done (16-12-30 10:00) @project(Work)",
  "  a note of the mail",
}, "archive")
-- archiving again: the new ones go in the archive there, by date
local again = { "Home:", " ✔ dishes @done (16-12-31 12:00)" }
for _, line in ipairs(archived) do
  table.insert(again, line)
end
check((tasks.archive(again, cfg)), {
  "Home:",
  "Work:",
  " ☐ report",
  "  Errands:",
  "",
  "＿＿＿＿＿＿＿＿＿＿＿＿＿＿＿＿＿＿＿",
  "Archive:",
  " ✔ dishes @done (16-12-31 12:00) @project(Home)",
  " ✘ bike @cancelled (16-12-31 10:00) @project(Work / Errands)",
  " ✔ mail @done (16-12-30 10:00) @project(Work)",
  "  a note of the mail",
}, "archive again")
local nothing, none = tasks.archive({ "Work:", " ☐ report" }, cfg)
check(none, 0, "nothing to archive")
check(nothing, { "Work:", " ☐ report" }, "nothing to archive")

check(tasks.archive_path("/home/me/list.todo", cfg), "/home/me/list_archive.todo", "archive path")
check(tasks.archive_path("TODO", cfg), "./TODO_archive", "archive path without extension")
local sub_lines, sub_infos = tasks.parse("Work:\n ☐ a\n ☐ b\nHome:\n")
local first, last = tasks.subtree(sub_infos, 1)
check(first, 1, "subtree")
check(last, 3, "subtree")
check(tasks.subtree(sub_infos, 2), nil, "no subtree")
check(tasks.archive_entry({ "Work:", " ☐ a" }, cfg, NOW),
  { "--- ✄ -----------------------", "Archived (16-12-31 23:00):", "Work:", " ☐ a" }, "archive entry")
check(#sub_lines, 4, "lines")

local unsorted = {
  "Work:",
  " ☐ later @due(17-02-01 10:00)",
  " ☐ low @low",
  " ☐ critical @critical",
  "   its note",
  " ☐ sooner @due(17-01-01 10:00)",
  "Home:",
}
local _, unsorted_infos = tasks.parse(table.concat(unsorted, "\n"))
check(tasks.sort_project(unsorted, unsorted_infos, 1, false), {
  "Work:",
  " ☐ sooner @due(17-01-01 10:00)",
  " ☐ later @due(17-02-01 10:00)",
  " ☐ critical @critical",
  "   its note",
  " ☐ low @low",
  "Home:",
}, "sorted")
check(tasks.sort_project(unsorted, unsorted_infos, 2, false), nil, "not a project")

local stats_lines, stats_infos = tasks.parse(
  "Work:\n ☐ a\n ✔ b @done (16-12-30 10:00)\n ✔ c @done (16-12-31 10:00)\n ✘ d @cancelled (16-12-31 11:00)\n")
check(tasks.stats(stats_lines, stats_infos, cfg),
  "3/4 done (75%) ■■■■■■■■□□ Last task @done (16-12-31 10:00)", "stats")
local tag_cfg = setmetatable({ stats_format = "☐$o ✔$d ✘$c {{@x}}" }, { __index = cfg })
check(tasks.stats(stats_lines, stats_infos, tag_cfg, function() return { 2, 3 } end), "☐1 ✔2 ✘1 1/1/0", "stats with a regex")

local due_lines, due_infos = tasks.parse(table.concat({
  " ☐ past @due(16-12-30 10:00)",
  " ☐ soon @due(17-01-01 10:00)",
  " ☐ later @due(17-02-01 10:00)",
  " ☐ invalid @due(2-30)",
  " ✔ done @due(16-12-01 10:00) @done (16-12-02 10:00)",
  " ☐ short @due(+2d)",
}, "\n"))
local dues = tasks.dues(due_lines, due_infos, cfg, NOW)
check(#dues, 5, "dues")
check(dues[1].state, "past")
check(dues[1].seconds, -(86400 + 13 * 3600), "overdue by")
check(dues[2].state, "soon")
check(dues[2].seconds, 11 * 3600, "due in")
check(dues[3].state, "later")
check(dues[4].state, "invalid")
check(dues[4].seconds, nil, "no time for an invalid date")
check(dues[5].state, "later")
check(dues[5].seconds, 2 * 86400, "+2d")

local project_lines, project_infos = tasks.parse("Work:\n  Errands: @today\n   ☐ a @high\n ✔ b @x @done (16-12-31 10:00)\n")
check(tasks.projects(project_infos), {
  { line = 1, name = "Work" },
  { line = 2, name = "Work / Errands" },
}, "projects")
local tag_list = tasks.tag_list(project_lines, project_infos)
check(#tag_list, 2, "tags of open tasks and projects")
check(tag_list[1].name, "@today")
check(tag_list[2].name, "@high")

-- links ----------------------------------------------------------------------------------------

check(links.file_link(" ☐ see ./notes/plan.md:11:8"), { path = "notes/plan.md", line = 11, column = 8 }, "dot link")
check(links.file_link(" ☐ see .\\filename\\"), { path = "filename" }, "backslash link")
check(links.file_link(" ☐ see ./another filename/ soon"), { path = "another filename" }, "a name with spaces")
check(links.file_link(" ☐ see ./src/main.rs>main"), { path = "src/main.rs", symbol = "main" }, "symbol")
check(links.file_link(' ☐ see ./README.md"Install"'), { path = "README.md", text = "Install" }, "text")
check(links.file_link(" ☐ see [the plan](docs/plan.md)"), { path = "docs/plan.md" }, "markdown link")
check(links.file_link(' ☐ see [the plan](docs/plan.md ":3")'), { path = "docs/plan.md", line = 3 }, "markdown link with a line")
check(links.file_link(" ☐ see [[docs/plan.md]]"), { path = "docs/plan.md" }, "wiki link")
check(links.file_link(" ☐ see [[docs/plan.md::12:4]]"), { path = "docs/plan.md", line = 12, column = 4 }, "wiki link with a line")
check(links.file_link(" ☐ see [[file:./x.md::*main]]"), { path = "x.md", symbol = "main" }, "org link with a symbol")
check(links.file_link(" ☐ see [[x.md::some text][desc]]"), { path = "x.md", text = "some text" }, "org link with a text")
check(links.file_link(" ☐ no link here."), nil, "no link")

local url, from, to = links.url_at(" ☐ read https://example.com/a?b=c, then", 15)
check(url, "https://example.com/a?b=c", "url")
check(from, 11, "url start (bytes: ☐ is three)")
check(to, 35, "url end")
check((links.url_at(" ☐ call <skype:nick> now", 12)), "skype:nick", "url in <>")
check((links.url_at(" ☐ www.example.com rocks", 8)), "http://www.example.com", "www")
check((links.url_at(" ☐ nothing here", 6)), nil, "no url")

check(links.candidates("x.md", "/home/me/list.todo", "/work"), { "/home/me/x.md", "/work/x.md" }, "candidates")
check(links.candidates("/abs/x.md", "/home/me/list.todo", "/work"), { "/abs/x.md" }, "absolute")

return "ok"
