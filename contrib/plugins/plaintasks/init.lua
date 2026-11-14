-- plaintasks: task lists like PlainTasks (https://github.com/aziz/PlainTasks), Sublime Text's
-- to-do list package, for the `.todo`, `.tasks`, `.taskpaper` and `TODO` files (the
-- `plaintasks` language).
--
--   Errands:
--    ☐ buy milk @today
--    ✔ call the bank @done (24-11-13 18:00)
--    ✘ fix the bike @cancelled (24-11-13 18:05)
--      a note under a task
--   --- ✄ -----------------------
--
-- Keys (in task lists only; see `keys` and `menu_key` below to change them; the first four
-- in insert mode too), and the same in the `<space>t` menu:
--   C-ret   new task (on the next line, or the line becomes one)      menu n
--   C-S-ret new task with @created(date)                              menu N
--   C-d     toggle done                                               menu d
--   A-c     toggle cancelled                                          menu c
--   C-S-a   archive the done and cancelled tasks                      menu a
--   C-S-o   archive the subtree after the cursor to name_archive.todo menu A
--   C-r     go to a project                                           menu p
--   C-S-r   go to a tag                                               menu t
--           the tasks due                                             menu u
--   A-o     open the file link of the line                            menu o
--   C-S-u   open the URL under the cursor                             menu O
--   F4      add @due()                                                menu D
--   F5, F7  sort the project's tasks by due date and priority (F7 descending)  menu s, S
--           recalculate the time spent (@lasted, @wasted)             menu r
--           copy the statistics                                       menu y
-- Tab (insert mode): `t`, `c`, `h`, `l`, `s`, `tg`, `d`, `cr` become @today, @critical,
-- @high, @low, @started, @toggle, @due( ), @created; after @started, @toggle and @created
-- Tab inserts the date; in @due(...) it expands a short date (`+2d`, `5`, `2-3 10:`); `--`
-- becomes a separator; on a done or cancelled task's tag it works out the time spent again.
--
-- Settings (`config` of its `[[plugins.plugins]]` entry): see `config` below. It needs the
-- "edit", "commands" and "keymaps" permissions, "windows" to open links, "files" to archive
-- to a file.

local syntax = require("plaintasks.syntax")
local dates = require("plaintasks.dates")
local tasks = require("plaintasks.tasks")
local links = require("plaintasks.links")

local config = {
  -- the languages of the buffers that are task lists
  languages = { "plaintasks" },
  -- bullets of new, done and cancelled tasks (others are recognized too: "-", "[ ]", "+",
  -- "[x]", "x", "[-]", ...)
  open_tasks_bullet = "☐",
  done_tasks_bullet = "✔",
  cancelled_tasks_bullet = "✘",
  -- spaces before the bullet of a task that is not indented, and after a bullet
  before_tasks_bullet_margin = 1,
  tasks_bullet_space = " ",
  -- TaskPaper's format: "-" bullets, "@done(date)"
  taskpaper_compatible = false,
  -- the stamps of done and cancelled tasks: " @done (24-11-13 18:00)"
  date_format = "(%y-%m-%d %H:%M)",
  before_date_space = " ",
  done_tag = true,          -- false: no @done/@cancelled, only the date
  done_date = true,         -- false: no date
  -- "new task" on a project line turns it into a task (instead of adding one under it)
  header_to_task = false,
  -- the archive
  archive_name = "Archive:",
  project_tag = true,       -- archived tasks get "@project(Work / Errands)" (false: before)
  new_on_top = true,        -- the latest archived first
  archive_org_filemask = "{dir}{sep}{base}_archive{ext}",
  -- the time spent: "1.50" instead of "1:30"
  decimal_minutes = false,
  -- due dates
  highlight_past_due = true,
  highlight_due_soon = 24,  -- hours: a due date that close is "soon" (0: never)
  show_remain_due = true,   -- "2 days, 3:00 remaining" or "1:30 overdue" after the line
  due_remain_format = "{time} remaining",
  due_overdue_format = "{time} overdue",
  due_preview = true,       -- what a short @due(...) at the cursor stands for
  -- gutter signs ("": none)
  icon_past_due = "●",
  icon_due_soon = "•",
  icon_misformatted = "",
  icon_critical = "",
  icon_high = "",
  icon_low = "",
  icon_today = "",
  -- statistics in the status line ("" for none); see tasks.stats
  stats_format = "$n/$a done ($percent%) $progress Last task @done $last",
  stats_ignore_archive = false,
  bar_full = "■",
  bar_empty = "□",
  replace_stats_chars = {}, -- { { "■", "=" }, ... } for "copy statistics"
  -- keys (in normal, select and insert mode): an action's key, a list of them, or false
  keys = {},
  menu_key = "<space>t",    -- false: no menu
  tab = true,               -- Tab's expansions in insert mode
  tab_fallback = "smart_tab", -- what Tab does otherwise
  -- styles of the parts, over the defaults below (written like in a theme)
  styles = {},
}

local DEFAULT_KEYS = {
  new = "C-ret",
  new_with_date = "C-S-ret",
  complete = "C-d",
  cancel = "A-c",
  archive = "C-S-a",
  archive_to_file = "C-S-o",
  goto_project = "C-r",
  goto_tag = "C-S-r",
  open_link = "A-o",
  open_url = "C-S-u",
  add_due = "F4",
  sort = "F5",
  sort_descending = "F7",
}

local user = helix.get_config() or {}
for key, value in pairs(user) do
  if key ~= "keys" and key ~= "styles" then
    config[key] = value
  end
end
for action, key in pairs(DEFAULT_KEYS) do
  config.keys[action] = key
end
for action, key in pairs(user.keys or {}) do
  config.keys[action] = key
end
if type(config.languages) == "string" then
  config.languages = { config.languages }
end
if config.taskpaper_compatible then
  config.open_tasks_bullet = "-"
  config.done_tasks_bullet = "-"
  config.cancelled_tasks_bullet = "-"
  config.before_date_space = ""
  config.done_tag = true
end
syntax.setup(config)

-- The styles of the parts of a task list (theme keys of their own, set from these).
local STYLES = {
  ["plaintasks.project"] = { style = "markup.heading", modifiers = { "bold" } },
  ["plaintasks.note"] = { style = "comment" },
  ["plaintasks.bullet"] = { style = "markup.list" },
  ["plaintasks.done"] = { style = "comment" },
  ["plaintasks.done.bullet"] = { style = "diff.plus", modifiers = { "bold" } },
  ["plaintasks.cancelled"] = { style = "comment", modifiers = { "italic", "crossed_out" } },
  ["plaintasks.cancelled.bullet"] = { style = "diff.minus" },
  ["plaintasks.tag"] = { style = "attribute", modifiers = { "bold" } },
  ["plaintasks.today"] = { style = "warning", modifiers = { "reversed", "bold" } },
  ["plaintasks.critical"] = { style = "error", modifiers = { "reversed", "bold" } },
  ["plaintasks.high"] = { style = "warning", modifiers = { "bold" } },
  ["plaintasks.low"] = { style = "hint", modifiers = { "bold" } },
  ["plaintasks.due.past"] = { style = "error", modifiers = { "bold", "underlined" } },
  ["plaintasks.due.soon"] = { style = "warning", modifiers = { "underlined" } },
  ["plaintasks.due.invalid"] = { style = "hint", underline = { style = "curl" } },
  ["plaintasks.due.remain"] = { style = "ui.virtual" },
  ["plaintasks.due.overdue"] = { style = "error" },
  ["plaintasks.separator"] = { style = "comment" },
  ["plaintasks.url"] = { style = "markup.link.url" },
  ["plaintasks.bold"] = { style = "markup.bold" },
  ["plaintasks.italic"] = { style = "markup.italic" },
}

for scope, style in pairs(STYLES) do
  local own = (user.styles or {})[scope:gsub("^plaintasks%.", "")]
  pcall(helix.ui.set_style, scope, own or style)
end

local SPECIAL_TAGS = { today = true, critical = true, high = true, low = true }

-- The time of now: os.time(), or `config.now` ("2016-12-31 23:00": the tests' clock).
local function now()
  if type(config.now) == "string" then
    local y, mo, d, h, mi = config.now:match("^(%d+)-(%d+)-(%d+) (%d+):(%d+)$")
    if y then
      return os.time({ year = tonumber(y), month = tonumber(mo), day = tonumber(d), hour = tonumber(h), min = tonumber(mi), sec = 0 })
    end
  end
  return os.time()
end

-- Whether `buffer` is a task list.
local function is_task_list(buffer)
  if not buffer then
    return false
  end
  local ok, language = pcall(buffer.get_language, buffer)
  if not ok or not language then
    return false
  end
  for _, name in ipairs(config.languages) do
    if name == language then
      return true
    end
  end
  return false
end

local function notify(message, level)
  helix.ui.notify("PlainTasks: " .. message, level)
end

-- The text of `buffer`, parsed, kept until it changes: { text, lines, infos, starts }
-- (`starts[i]`: the char index line i starts at).
local parsed = {}

local function parse(buffer)
  local id = buffer:id()
  local text = buffer:get_text()
  local cached = parsed[id]
  if cached and cached.text == text then
    return cached
  end
  local lines, infos = tasks.parse(text)
  local starts, at = {}, 0
  local crlf = text:find("\r\n", 1, true) ~= nil
  for i, line in ipairs(lines) do
    starts[i] = at
    at = at + (utf8.len(line) or #line) + (crlf and 2 or 1)
  end
  cached = { text = text, lines = lines, infos = infos, starts = starts, crlf = crlf }
  parsed[id] = cached
  return cached
end

-- The highlights of the parts of a task list: { { from =, to =, style = } } (char indices), in
-- three layers (the later ones over the earlier ones): the lines, their parts, the due dates.
local function highlights(p, dues)
  local base, parts, due_layer = {}, {}, {}
  local function add(list, i, from_byte, to_byte, style)
    local line = p.lines[i]
    local from = p.starts[i] + syntax.char_col(line, from_byte)
    local to = p.starts[i] + syntax.char_col(line, to_byte + 1)
    if to > from then
      table.insert(list, { from = from, to = to, style = style })
    end
  end
  for i, line in ipairs(p.lines) do
    local info = p.infos[i]
    local kind = info.kind
    local text_at = #info.indent + 1
    if kind == "project" then
      add(base, i, text_at, #line, "plaintasks.project")
    elseif kind == "note" then
      add(base, i, text_at, #line, "plaintasks.note")
    elseif kind == "separator" or kind == "archive" then
      add(base, i, text_at, #line, "plaintasks.separator")
    elseif kind == "done" or kind == "cancelled" then
      add(base, i, info.text_at, #line, "plaintasks." .. kind)
    end
    if info.bullet then
      local style = kind == "pending" and "plaintasks.bullet" or ("plaintasks." .. kind .. ".bullet")
      add(parts, i, text_at, text_at + #info.bullet - 1, style)
    end
    if kind ~= "done" and kind ~= "cancelled" and kind ~= "separator" and kind ~= "archive" then
      for _, tag in ipairs(syntax.tags(line)) do
        local style = SPECIAL_TAGS[tag.name] and ("plaintasks." .. tag.name) or "plaintasks.tag"
        add(parts, i, tag.start, tag.finish, style)
      end
      -- *italic*, _italic_, **bold**, __bold__, and URLs
      for start, mark, finish in line:gmatch("()([%*_][%*_]?)%S.-%S?%2()") do
        local before = start == 1 and " " or line:sub(start - 1, start - 1)
        if before:match("%s") and (#mark == 2 or mark == "*" or mark == "_") then
          add(parts, i, start, finish - 1, #mark == 2 and "plaintasks.bold" or "plaintasks.italic")
        end
      end
      local position = 1
      while true do
        local start, finish = line:find("<[%w%+%-%.]+:[^%s>]+>", position)
        if not start then
          start, finish = line:find("%a[%w%+%-%.]*://[^%s\"'<>]+", position)
        end
        if not start then
          break
        end
        add(parts, i, start, finish, "plaintasks.url")
        position = finish + 1
      end
    end
  end
  if config.highlight_past_due then
    for _, due in ipairs(dues) do
      local style = ({ past = "plaintasks.due.past", soon = "plaintasks.due.soon", invalid = "plaintasks.due.invalid" })[due.state]
      if style then
        add(due_layer, due.line, due.start, due.finish, style)
      end
    end
  end
  local all = {}
  for _, layer in ipairs({ base, parts, due_layer }) do
    for _, highlight in ipairs(layer) do
      table.insert(all, highlight)
    end
  end
  return all
end

-- The gutter signs: due dates past, soon or invalid, and the special tags.
local function signs(p, dues)
  local found, taken = {}, {}
  local function sign(line, icon, style)
    if icon and icon ~= "" and not taken[line] then
      taken[line] = true
      table.insert(found, { line = line, text = icon, style = style })
    end
  end
  for _, due in ipairs(dues) do
    if due.state == "past" then
      sign(due.line, config.icon_past_due, "plaintasks.due.past")
    elseif due.state == "soon" then
      sign(due.line, config.icon_due_soon, "plaintasks.due.soon")
    elseif due.state == "invalid" then
      sign(due.line, config.icon_misformatted, "plaintasks.due.invalid")
    end
  end
  for i, line in ipairs(p.lines) do
    if p.infos[i].kind == "pending" then
      for _, name in ipairs({ "critical", "high", "low", "today" }) do
        if syntax.has_tag(line, name) then
          sign(i, config["icon_" .. name], "plaintasks." .. name)
        end
      end
    end
  end
  return found
end

-- The lines of `buffer` with a match of the regex `regex` (for {{...}} in `stats_format`).
local function lines_matching(buffer)
  return function(regex)
    local ok, matches = pcall(buffer.find_all, buffer, regex)
    if not ok then
      return {}
    end
    local found, seen = {}, {}
    for _, match in ipairs(matches) do
      local line = buffer:char_to_line(match.from) + 1
      if not seen[line] then
        seen[line] = true
        table.insert(found, line)
      end
    end
    return found
  end
end

local stats_shown = {} -- document id -> the statistics shown for it

-- The statistics of `buffer`'s task list.
local function stats_of(buffer, p)
  return tasks.stats(p.lines, p.infos, config, lines_matching(buffer))
end

-- The remaining or overdue times after the lines with a due date, and what the short date
-- at the cursor stands for (the focused buffer).
local function annotations(buffer, p, dues)
  local found = {}
  local by_line = {}
  local function add(line, text, style)
    local entry = by_line[line]
    if entry then
      entry.text = entry.text .. "  " .. text
      return
    end
    local last = p.lines[line]
    entry = {
      char_idx = p.starts[line] + (utf8.len(last) or #last),
      text = " " .. text,
      style = style,
    }
    by_line[line] = entry
    table.insert(found, entry)
  end
  if config.show_remain_due then
    for _, due in ipairs(dues) do
      if due.seconds then
        local time = dates.format_delta(math.abs(due.seconds), config.decimal_minutes)
        if time == "" then
          time = "a little bit"
        end
        if due.seconds <= 0 then
          add(due.line, (config.due_overdue_format:gsub("{time}", time)), "plaintasks.due.overdue")
        else
          add(due.line, (config.due_remain_format:gsub("{time}", time)), "plaintasks.due.remain")
        end
      end
    end
  end
  if config.due_preview then
    -- the cursor in a @due(...) not written with `date_format`: the date it stands for
    local cursor = helix.editor.get_cursor()
    local line_index = cursor.row + 1
    local line = p.lines[line_index]
    if line then
      local byte = utf8.offset(line, cursor.col + 1) or (#line + 1)
      for _, tag in ipairs(syntax.tags(line)) do
        if tag.name == "due" and tag.value and byte >= tag.start and byte <= tag.finish + 1 then
          local full = dates.strptime("(" .. tag.value .. ")", config.date_format)
          if not full then
            local now_date = dates.date_of(now())
            local date, err = dates.due(tag.value, config.date_format, now_date)
            if date then
              add(line_index, "→ " .. dates.format(dates.time(date), config.date_format), "plaintasks.due.remain")
            else
              add(line_index, "✗ " .. tostring(err), "plaintasks.due.invalid")
            end
          end
        end
      end
    end
  end
  return found
end

-- Draws the parts of `buffer` (a task list): highlights, signs, statistics, and for the
-- focused one the due times.
-- The due dates of a parsed task list, worked out again each minute (for the cursor's moves).
local function dues_of(p)
  local time = now()
  local minute = time // 60
  if p.dues_minute ~= minute then
    p.dues = tasks.dues(p.lines, p.infos, config, time)
    p.dues_minute = minute
  end
  return p.dues
end

local function refresh(buffer)
  if not is_task_list(buffer) then
    return
  end
  local p = parse(buffer)
  local dues = dues_of(p)
  buffer:set_highlights(highlights(p, dues))
  buffer:set_signs(signs(p, dues))
  local id = buffer:id()
  if config.stats_format ~= "" then
    local text = stats_of(buffer, p)
    if stats_shown[id] ~= text then
      stats_shown[id] = text
      helix.statusline.set("stats-" .. id, { text = text, buffer = buffer, position = "right" })
    end
  end
  local focused = helix.buffer.get_current()
  if focused and focused:id() == id then
    buffer:set_annotations(annotations(buffer, p, dues))
  end
end

-- Removes what the plugin draws for `buffer` (no longer a task list).
local function clear(buffer, id)
  id = id or buffer:id()
  parsed[id] = nil
  if stats_shown[id] then
    helix.statusline.remove("stats-" .. id)
    stats_shown[id] = nil
  end
  if buffer then
    pcall(buffer.set_highlights, buffer, {})
    pcall(buffer.set_signs, buffer, {})
    pcall(buffer.clear_annotations, buffer)
  end
end

local function refresh_all()
  for _, buffer in ipairs(helix.buffer.list()) do
    if is_task_list(buffer) then
      refresh(buffer)
    end
  end
end

-- Editing ------------------------------------------------------------------------------------

local HUGE = 1000000000

-- The line ending of `buffer`.
local function newline(p)
  return p.crlf and "\r\n" or "\n"
end

-- Replaces lines `first` to `last` (1-based) of `buffer` with `new_lines` (none: removes
-- them), in one change.
local function replace_lines(buffer, p, first, last, new_lines)
  if #new_lines > 0 then
    buffer:replace(first, 0, last, HUGE, table.concat(new_lines, newline(p)))
  elseif last < #p.lines then
    buffer:delete(first, 0, last + 1, 0)
  elseif first > 1 then
    buffer:delete(first - 1, HUGE, last, HUGE)
  else
    buffer:replace(first, 0, last, HUGE, "")
  end
end

-- The lines (1-based, in order) the selections of the focused window are on, and the
-- selections as { line, col } of their anchor and head.
local function selected_lines(buffer)
  local selections = helix.editor.get_selections()
  local seen, lines, places = {}, {}, {}
  for _, selection in ipairs(selections) do
    local from = math.min(selection.anchor, selection.head)
    local to = math.max(selection.anchor, selection.head)
    local first = buffer:char_to_line(from) + 1
    local last = buffer:char_to_line(math.max(from, to - 1)) + 1
    for line = first, last do
      if not seen[line] then
        seen[line] = true
        table.insert(lines, line)
      end
    end
    local function place(char)
      local row = buffer:char_to_line(char)
      return { line = row + 1, col = char - buffer:line_to_char(row) }
    end
    table.insert(places, { anchor = place(selection.anchor), head = place(selection.head) })
  end
  table.sort(lines)
  return lines, places
end

-- The char index of `place` ({ line, col }) in `buffer`, the column clamped to its line.
local function char_of(buffer, place)
  local count = buffer:line_count()
  local line = math.max(1, math.min(place.line, count))
  local start = buffer:line_to_char(line - 1)
  local text = buffer:get_line(line):gsub("\r?\n$", "")
  return start + math.min(place.col, utf8.len(text) or #text)
end

-- Puts the selections back where they were (by line and column).
local function restore(buffer, places)
  local selections = {}
  for _, place in ipairs(places) do
    table.insert(selections, { anchor = char_of(buffer, place.anchor), head = char_of(buffer, place.head) })
  end
  if #selections > 0 then
    helix.editor.set_selections(selections)
  end
end

-- The focused buffer if it is a task list (else a message).
local function task_list()
  local buffer = helix.buffer.get_current()
  if not is_task_list(buffer) then
    notify("not a task list (" .. table.concat(config.languages, ", ") .. ")", "warning")
    return nil
  end
  return buffer
end

-- Toggles the selected lines done or cancelled.
local function toggle(action)
  local buffer = task_list()
  if not buffer then
    return
  end
  local p = parse(buffer)
  local lines, places = selected_lines(buffer)
  local selected = {}
  for _, line in ipairs(lines) do
    selected[line] = true
  end
  local first, last = lines[1], lines[#lines]
  local new_lines, changed, message = {}, false, nil
  local time = now()
  for i = first, last do
    local new
    if selected[i] and p.infos[i] then
      local why
      new, why = tasks.toggle(p.lines, p.infos, i, action, config, time)
      message = message or why
    end
    changed = changed or (new ~= nil and new ~= p.lines[i])
    table.insert(new_lines, new or p.lines[i])
  end
  if message then
    notify(message)
  end
  if changed then
    replace_lines(buffer, p, first, last, new_lines)
    restore(buffer, places)
  end
end

-- A new task after each selected line, or the line made a task; the cursors at their ends.
local function new_task(with_date)
  local buffer = task_list()
  if not buffer then
    return
  end
  local p = parse(buffer)
  local lines = selected_lines(buffer)
  local first, last = lines[1], lines[#lines]
  local new_lines, cursors = {}, {}
  local selected = {}
  for _, line in ipairs(lines) do
    selected[line] = true
  end
  local suffix = with_date and (" @created" .. dates.format(now(), config.date_format)) or ""
  for i = first, math.max(last, first) do
    local line = p.lines[i] or ""
    if selected[i] then
      local info = p.infos[i] or syntax.classify(line)
      local result, col = tasks.new_task(line, info, config)
      for k, new in ipairs(result) do
        if k == #result then
          table.insert(new_lines, new .. suffix)
          table.insert(cursors, { line = first + #new_lines - 1, col = col })
        else
          table.insert(new_lines, new)
        end
      end
    else
      table.insert(new_lines, line)
    end
  end
  if #p.lines == 0 then
    -- an empty buffer
    buffer:replace(1, 0, 1, HUGE, table.concat(new_lines, newline(p)))
  else
    replace_lines(buffer, p, first, last, new_lines)
  end
  local selections = {}
  for _, cursor in ipairs(cursors) do
    local char = char_of(buffer, cursor)
    table.insert(selections, { anchor = char, head = char })
  end
  helix.editor.set_selections(selections)
  if helix.editor.mode() ~= "insert" then
    helix.editor.insert_mode()
  end
end

-- Archives the done and cancelled tasks.
local function archive()
  local buffer = task_list()
  if not buffer then
    return
  end
  local p = parse(buffer)
  local _, places = selected_lines(buffer)
  local new_lines, moved = tasks.archive(p.lines, config)
  if moved == 0 then
    notify("Nothing to archive")
    return
  end
  replace_lines(buffer, p, 1, #p.lines, new_lines)
  restore(buffer, places)
  notify(string.format("%d task%s archived", moved, moved == 1 and "" or "s"))
end

-- The open buffer of `path`, if any.
local function buffer_of(path)
  for _, buffer in ipairs(helix.buffer.list()) do
    if buffer:get_path() == path then
      return buffer
    end
  end
  return nil
end

local closing_after_save = {} -- document ids of archive files opened to append to

-- Moves the subtree at the cursor (its line and the lines under it) to the archive file next
-- to the task list (`archive_org_filemask`).
local function archive_to_file()
  local buffer = task_list()
  if not buffer then
    return
  end
  local path = buffer:get_path()
  if not path then
    notify("the task list has no file: write it first", "error")
    return
  end
  local p = parse(buffer)
  local row = helix.editor.get_cursor().row + 1
  local first, last = tasks.subtree(p.infos, row)
  if not first then
    notify("Could not find a tree to archive: nothing is under this line", "error")
    return
  end
  local subtree = {}
  for i = first, last do
    table.insert(subtree, p.lines[i])
  end
  local target_path = tasks.archive_path(path, config)
  local target = buffer_of(target_path)
  local opened = false
  if not target then
    local ok, result = pcall(helix.editor.open, target_path)
    if not ok or not result then
      notify("cannot open " .. target_path .. ": " .. tostring(result), "error")
      return
    end
    target, opened = result, true
  end
  local entry = table.concat(tasks.archive_entry(subtree, config, now()), newline(p))
  local existing = target:get_text()
  if existing:match("^%s*$") then
    -- a new file (Helix's empty buffer has a line ending)
    existing = ""
  end
  local separator = (existing == "" or existing:sub(-1) == "\n") and "" or newline(p)
  target:set_text(existing .. separator .. entry .. newline(p))
  if opened then
    closing_after_save[target:id()] = true
  end
  local ok, err = pcall(target.save, target)
  if not ok then
    notify("cannot write " .. target_path .. ": " .. tostring(err), "error")
    return
  end
  replace_lines(buffer, p, first, last, {})
  notify("archived to " .. target_path)
end

-- Goes to line `line` (1-based), column `col` (0-based) of the focused window.
local function go_to(line, col)
  helix.editor.set_cursor(line - 1, col or 0)
  helix.editor.run("align_view_center")
end

local function goto_project()
  local buffer = task_list()
  if not buffer then
    return
  end
  local p = parse(buffer)
  local items = {}
  for _, project in ipairs(tasks.projects(p.infos)) do
    table.insert(items, { text = project.name, buffer = buffer, line = project.line, data = project })
  end
  if #items == 0 then
    notify("no projects")
    return
  end
  helix.ui.picker({
    items = items,
    title = "Projects",
    prompt = "Project:",
    on_select = function(item)
      go_to(item.data.line, #p.infos[item.data.line].indent)
    end,
  })
end

local function goto_tag()
  local buffer = task_list()
  if not buffer then
    return
  end
  local p = parse(buffer)
  local items = {}
  for _, tag in ipairs(tasks.tag_list(p.lines, p.infos)) do
    local line = p.lines[tag.line]
    table.insert(items, {
      columns = { tag.name, string.format("%d: %s", tag.line, (line:gsub("^%s+", ""))) },
      buffer = buffer,
      line = tag.line,
      data = { line = tag.line, col = syntax.char_col(line, tag.start) },
    })
  end
  if #items == 0 then
    notify("no tags")
    return
  end
  helix.ui.picker({
    items = items,
    title = "Tags",
    prompt = "Tag:",
    columns = { "Tag", "Task" },
    on_select = function(item)
      go_to(item.data.line, item.data.col)
    end,
  })
end

-- The tasks with a due date, soonest first, overdue ones first of all.
local function due_tasks()
  local buffer = task_list()
  if not buffer then
    return
  end
  local p = parse(buffer)
  local dues = {}
  for _, due in ipairs(dues_of(p)) do
    table.insert(dues, due)
  end
  table.sort(dues, function(a, b)
    if (a.seconds == nil) ~= (b.seconds == nil) then
      return a.seconds ~= nil
    end
    if a.seconds and a.seconds ~= b.seconds then
      return a.seconds < b.seconds
    end
    return a.line < b.line
  end)
  local items = {}
  for _, due in ipairs(dues) do
    local when
    if not due.seconds then
      when = "invalid date"
    else
      local time = dates.format_delta(math.abs(due.seconds), config.decimal_minutes)
      if time == "" then
        time = "a little bit"
      end
      when = (due.seconds <= 0 and config.due_overdue_format or config.due_remain_format):gsub("{time}", time)
    end
    table.insert(items, {
      columns = { when, (p.lines[due.line]:gsub("^%s+", "")) },
      buffer = buffer,
      line = due.line,
      data = { line = due.line, col = syntax.char_col(p.lines[due.line], due.start) },
    })
  end
  if #items == 0 then
    notify("no due dates")
    return
  end
  helix.ui.picker({
    items = items,
    title = "Due",
    prompt = "Task:",
    columns = { "Due", "Task" },
    on_select = function(item)
      go_to(item.data.line, item.data.col)
    end,
  })
end

-- Opens the file the line's link points to, at its line, symbol or text.
local function open_link()
  local buffer = task_list()
  if not buffer then
    return
  end
  local row = helix.editor.get_cursor().row + 1
  local line = parse(buffer).lines[row] or ""
  local link = links.file_link(line)
  if not link then
    notify("Line does not contain a valid link to file")
    return
  end
  local found
  for _, candidate in ipairs(links.candidates(link.path, buffer:get_path(), helix.editor.get_cwd())) do
    if helix.fs.exists(candidate) then
      found = candidate
      break
    end
  end
  if not found then
    notify("File was not found: " .. link.path, "error")
    return
  end
  if helix.fs.is_dir(found) then
    if helix.has_permission("explorer") then
      helix.explorer.reveal(found)
    else
      notify(found .. " is a directory", "warning")
    end
    return
  end
  local target = helix.editor.open(found, "replace")
  if link.line then
    go_to(link.line, math.max((link.column or 1) - 1, 0))
  elseif link.symbol or link.text then
    local pattern
    if link.symbol then
      pattern = "\\b" .. link.symbol:gsub("[^%w_]", "") .. "\\b"
    else
      pattern = link.text:gsub("[%^%$%(%)%%%.%[%]%*%+%-%?%{%}%|\\]", "\\%0")
    end
    local ok, matches = pcall(target.find_all, target, pattern, { case_insensitive = false, max = 1 })
    if ok and matches[1] then
      local at = matches[1].from
      local match_row = target:char_to_line(at)
      go_to(match_row + 1, at - target:line_to_char(match_row))
    else
      notify("not found in " .. found .. ": " .. (link.symbol or link.text), "warning")
    end
  end
end

-- Opens the URL under the cursor with the system's opener.
local function open_url()
  local buffer = task_list()
  if not buffer then
    return
  end
  local cursor = helix.editor.get_cursor()
  local line = parse(buffer).lines[cursor.row + 1] or ""
  local byte = utf8.offset(line, cursor.col + 1) or (#line + 1)
  local url = links.url_at(line, byte)
  if not url then
    notify("Looks like there is nothing to open")
    return
  end
  helix.editor.execute_command("open-url", { url })
end

-- Adds @due() to the selected tasks and projects without one, the cursor in the first.
local function add_due()
  local buffer = task_list()
  if not buffer then
    return
  end
  local p = parse(buffer)
  local lines = selected_lines(buffer)
  local first, last = lines[1], lines[#lines]
  local new_lines, cursor = {}, nil
  local selected = {}
  for _, line in ipairs(lines) do
    selected[line] = true
  end
  for i = first, last do
    local line = p.lines[i]
    local info = p.infos[i]
    if selected[i] and (syntax.is_task(info.kind) or info.kind == "project") and not syntax.has_tag(line, "due") then
      line = line:gsub("%s+$", "") .. " @due()"
      cursor = cursor or { line = i, col = (utf8.len(line) or #line) - 1 }
    end
    table.insert(new_lines, line)
  end
  if not cursor then
    return
  end
  replace_lines(buffer, p, first, last, new_lines)
  local char = char_of(buffer, cursor)
  helix.editor.set_selections({ { anchor = char, head = char } })
  if helix.editor.mode() ~= "insert" then
    helix.editor.insert_mode()
  end
end

-- Sorts the tasks of the selected projects by due date and priority.
local function sort(descending)
  local buffer = task_list()
  if not buffer then
    return
  end
  local p = parse(buffer)
  local lines, places = selected_lines(buffer)
  local result = p.lines
  local sorted = 0
  -- from the last project: the lines of the earlier ones keep their places
  for k = #lines, 1, -1 do
    local index = lines[k]
    local infos = {}
    for i, line in ipairs(result) do
      infos[i] = syntax.classify(line)
    end
    local new = tasks.sort_project(result, infos, index, descending)
    if new then
      result = new
      sorted = sorted + 1
    end
  end
  if sorted == 0 then
    notify("put the cursor on a project line to sort its tasks")
    return
  end
  replace_lines(buffer, p, 1, #p.lines, result)
  restore(buffer, places)
end

local function copy_stats()
  local buffer = task_list()
  if not buffer then
    return
  end
  local text = stats_of(buffer, parse(buffer))
  for _, pair in ipairs(config.replace_stats_chars) do
    text = text:gsub(pair[1]:gsub("[%^%$%(%)%%%.%[%]%*%+%-%?]", "%%%0"), (pair[2]:gsub("%%", "%%%%")))
  end
  helix.editor.set_register("+", { text })
  notify("statistics copied: " .. text)
end

-- Works out again the time spent on the selected done and cancelled tasks.
local function recalculate()
  local buffer = task_list()
  if not buffer then
    return false
  end
  local p = parse(buffer)
  local lines, places = selected_lines(buffer)
  local first, last = lines[1], lines[#lines]
  local new_lines, changed = {}, false
  local selected = {}
  for _, line in ipairs(lines) do
    selected[line] = true
  end
  for i = first, last do
    local new = selected[i] and tasks.recalculate(p.lines, p.infos, i, config) or nil
    changed = changed or (new ~= nil and new ~= p.lines[i])
    table.insert(new_lines, new or p.lines[i])
  end
  if changed then
    replace_lines(buffer, p, first, last, new_lines)
    restore(buffer, places)
  end
  return changed
end

local function new_document()
  local buffer = helix.buffer.create("", config.languages[1])
  helix.window.get_current():set_buffer(buffer)
end

-- Tab in insert mode -----------------------------------------------------------------------

local TRIGGERS = {
  t = "@today", c = "@critical", h = "@high", l = "@low",
  s = "@started", tg = "@toggle", cr = "@created", d = "@due( )",
}

-- Replaces bytes `from` to `to` of line `row` (1-based) with `text`, the cursor `cursor_back`
-- characters before its end.
local function replace_in_line(buffer, row, line, from, to, text, cursor_back)
  local start_col = syntax.char_col(line, from)
  local end_col = syntax.char_col(line, to + 1)
  buffer:replace(row, start_col, row, end_col, text)
  local col = start_col + (utf8.len(text) or #text) - (cursor_back or 0)
  local char = buffer:line_to_char(row - 1) + col
  helix.editor.set_selections({ { anchor = char, head = char } })
end

-- What Tab does at the cursor of a task list in insert mode; whether it did something.
local function tab()
  local buffer = helix.buffer.get_current()
  if not is_task_list(buffer) then
    return false
  end
  local cursor = helix.editor.get_cursor()
  local row = cursor.row + 1
  local p = parse(buffer)
  local line = p.lines[row] or ""
  local byte = utf8.offset(line, cursor.col + 1) or (#line + 1)
  local before = line:sub(1, byte - 1)
  local after = line:sub(byte)
  local time = now()
  -- in @due(...): the short date expanded
  local open = before:match(".*()@due%(")
  if open and not before:sub(open):find(")", 1, true) and after:match("^[^)]*%)") then
    local value_start = open + 5
    local close = byte + #after:match("^[^)]*")
    local value = line:sub(value_start, close - 1)
    if value:match("^%s*$") then
      value = "0"
    end
    local created_value = line:match("@created(%b())")
    local created = created_value and dates.parse(created_value, config.date_format, dates.date_of(time))
    local date, err = dates.due(value, config.date_format, dates.date_of(time), created)
    if not date then
      notify(tostring(err), "error")
      return true
    end
    local text = dates.format(dates.time(date), config.date_format)
    replace_in_line(buffer, row, line, value_start - 1, close, text)
    return true
  end
  -- after @started, @toggle or @created: the date
  if before:match("@started$") or before:match("@toggle$") or before:match("@created$") then
    local text = dates.format(time, config.date_format)
    replace_in_line(buffer, row, line, byte, byte - 1, text)
    return true
  end
  -- a trigger: its tag
  local word_start, word = before:match("()(%S+)$")
  if word and (word_start == 1 or before:sub(word_start - 1, word_start - 1):match("%s")) then
    local tag = TRIGGERS[word]
    if tag then
      replace_in_line(buffer, row, line, word_start, byte - 1, tag, tag == "@due( )" and 1 or 0)
      return true
    end
    if word == "--" then
      replace_in_line(buffer, row, line, word_start, byte - 1, "--- ✄ -----------------------")
      return true
    end
  end
  -- on a tag of a done or cancelled task: the time spent worked out again
  local info = p.infos[row]
  if info and (info.kind == "done" or info.kind == "cancelled") then
    for _, found in ipairs(syntax.tags(line)) do
      if byte >= found.start and byte <= found.finish + 1 then
        return recalculate()
      end
    end
  end
  return false
end

-- Commands and keys --------------------------------------------------------------------------

local ACTIONS = {
  { "new", "tasks-new", "New task", function() new_task(false) end, "n" },
  { "new_with_date", "tasks-new-with-date", "New task with @created", function() new_task(true) end, "N" },
  { "complete", "tasks-complete", "Toggle done", function() toggle("done") end, "d" },
  { "cancel", "tasks-cancel", "Toggle cancelled", function() toggle("cancelled") end, "c" },
  { "archive", "tasks-archive", "Archive done tasks", archive, "a" },
  { "archive_to_file", "tasks-archive-to-file", "Archive subtree to file", archive_to_file, "A" },
  { "goto_project", "tasks-goto-project", "Go to project", goto_project, "p" },
  { "goto_tag", "tasks-goto-tag", "Go to tag", goto_tag, "t" },
  { "due", "tasks-due", "Due tasks", due_tasks, "u" },
  { "open_link", "tasks-open-link", "Open file link", open_link, "o" },
  { "open_url", "tasks-open-url", "Open URL", open_url, "O" },
  { "add_due", "tasks-add-due", "Add @due()", add_due, "D" },
  { "sort", "tasks-sort", "Sort by due date and priority", function() sort(false) end, "s" },
  { "sort_descending", "tasks-sort-descending", "Sort by due date and priority, descending", function() sort(true) end, "S" },
  { "recalculate", "tasks-recalculate", "Recalculate time spent", recalculate, "r" },
  { "copy_stats", "tasks-copy-stats", "Copy statistics", copy_stats, "y" },
}

-- Runs `fn`, reporting its error (a missing permission, say) instead of failing silently.
local function guarded(fn)
  return function(...)
    local ok, err = pcall(fn, ...)
    if not ok then
      notify(tostring(err), "error")
    end
  end
end

if helix.has_permission("commands") then
  for _, action in ipairs(ACTIONS) do
    helix.register_command({ name = action[2], doc = "PlainTasks: " .. action[3], handler = guarded(action[4]) })
  end
  helix.register_command({ name = "tasks-new-document", doc = "PlainTasks: a new task list", handler = guarded(new_document) })
end

-- The actions whose keys work in insert mode too (while typing a task); the others keep
-- insert mode's keys (C-r inserts a register there).
local WHILE_TYPING = { new = true, new_with_date = true, complete = true, cancel = true }

if helix.has_permission("keymaps") then
  for _, action in ipairs(ACTIONS) do
    local modes = WHILE_TYPING[action[1]] and { "normal", "select", "insert" } or { "normal", "select" }
    local keys = config.keys[action[1]]
    if type(keys) == "string" then
      keys = { keys }
    end
    for _, key in ipairs(type(keys) == "table" and keys or {}) do
      helix.keymap.set(modes, key, guarded(action[4]), { desc = action[3], language = config.languages })
    end
    if config.menu_key and config.menu_key ~= "" then
      helix.keymap.set({ "normal", "select" }, config.menu_key .. action[5], guarded(action[4]), {
        desc = action[3],
        language = config.languages,
        menu = "Tasks",
      })
    end
  end
  if config.tab and helix.has_permission("commands") then
    helix.keymap.set("insert", "tab", guarded(function()
      if not tab() then
        helix.editor.run(config.tab_fallback)
      end
    end), { desc = "PlainTasks: expand a tag or a date", language = config.languages })
  end
end

-- Events -----------------------------------------------------------------------------------

local function refresh_by_id(id)
  local buffer = id and helix.buffer.get_by_id(id)
  if buffer then
    if is_task_list(buffer) then
      refresh(buffer)
    else
      clear(buffer)
    end
  end
end

helix.on("init", refresh_all)
helix.on("ready", refresh_all)
helix.on("buffer_open", function(event) refresh_by_id(event.document_id) end)
helix.on("buffer_changed", function(event) refresh_by_id(event.document_id) end)
helix.on("language_change", function(event) refresh_by_id(event.document_id) end)
helix.on("selection_change", function(event)
  local buffer = helix.buffer.get_by_id(event.document_id)
  if is_task_list(buffer) then
    local p = parse(buffer)
    buffer:set_annotations(annotations(buffer, p, dues_of(p)))
  end
end)
helix.on("window_focus", function(event) refresh_by_id(event.document_id) end)
helix.on("buffer_post_save", function(event)
  if closing_after_save[event.document_id] then
    closing_after_save[event.document_id] = nil
    local buffer = helix.buffer.get_by_id(event.document_id)
    if buffer and helix.has_permission("windows") then
      pcall(buffer.close, buffer)
    end
  end
end)
helix.on("buffer_save_failed", function(event)
  if closing_after_save[event.document_id] then
    closing_after_save[event.document_id] = nil
    notify("the archive file was not written (" .. tostring(event.error) .. "): undo brings the subtree back", "error")
  end
end)
helix.on("buffer_close", function(event)
  clear(nil, event.document_id)
end)

-- the due dates move closer: drawn again every minute
helix.every(60000, refresh_all)
