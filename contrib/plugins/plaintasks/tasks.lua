-- What the PlainTasks commands do to the lines of a task list: plain functions of the lines
-- (without line endings), the configuration and the time, that give the new lines.
--
-- `cfg` has the settings of `init.lua` (bullets, `date_format`, ...); `now` is a time
-- (seconds, like os.time gives).

local syntax = require("plaintasks.syntax")
local dates = require("plaintasks.dates")

local M = {}

-- The text written after a task done or cancelled at `now`: " @done (24-11-13 18:00)".
function M.stamp(tag, cfg, now)
  local date = cfg.done_date and dates.format(now, cfg.date_format) or ""
  local text = " " .. (cfg.done_tag and tag or "") .. cfg.before_date_space .. date
  return (text:gsub("  +", " "):gsub("%s+$", ""))
end

-- The spaces before a task's bullet when it is not indented: `before_tasks_bullet_margin`.
local function margin(cfg)
  return string.rep(" ", cfg.before_tasks_bullet_margin)
end

-- `text` without trailing spaces, and the two spaces it ended with, if any (PlainTasks keeps
-- them: Markdown's line break).
local function split_double_space(text)
  if text:sub(-2) == "  " then
    return (text:gsub("%s+$", "")), "  "
  end
  return (text:gsub("%s+$", "")), ""
end

-- The new lines for "new task" on `line`, and the column of the cursor (characters) on the
-- last of them.
function M.new_task(line, info, cfg)
  local bullet = cfg.open_tasks_bullet .. cfg.tasks_bullet_space
  local function after(prefix)
    local new = info.indent .. prefix .. bullet
    return { line, new }, utf8.len(new)
  end
  local kind = info.kind
  if syntax.is_task(kind) then
    return after("")
  elseif (kind == "project" and not cfg.header_to_task) or kind == "separator" or kind == "archive" then
    return after(margin(cfg))
  elseif kind == "blank" then
    local new = (info.indent ~= "" and info.indent or margin(cfg)) .. bullet
    return { new }, utf8.len(new)
  end
  -- a note (or a project with `header_to_task`) becomes a task
  local new = (info.indent ~= "" and info.indent or margin(cfg)) .. bullet .. info.text
  return { new }, utf8.len(new)
end

-- Where the ending of a task done or cancelled starts in `text` (after its bullet): its stamp,
-- `@project(...)` of the archive, and the time spent. Nil when it has none.
local function ending_at(text, tag, cfg)
  local cut
  for _, pattern in ipairs({ "%s@" .. tag .. "%f[^%w_]", "@project%(", "@lasted%(", "@wasted%(" }) do
    local at = text:find(pattern)
    if at and (not cut or at < cut) then
      cut = at
    end
  end
  if cut then
    return cut
  end
  -- without the done tag (`done_tag = false`): a date in parentheses at the end
  local before, date = text:match("^(.-)%s(%b())%s*$")
  if before and dates.strptime(date, cfg.date_format) then
    return #before + 1
  end
  return nil
end

-- The text of a done or cancelled task without its ending (see `ending_at`).
local function without_ending(text, tag, cfg)
  local cut = ending_at(text, tag, cfg)
  if cut then
    text = text:sub(1, cut - 1)
  end
  return (text:gsub("%s+$", ""))
end

-- The value of the first `@name(...)` tag of `text`, with its parentheses.
local function tag_value(text, name)
  for _, tag in ipairs(syntax.tags(text)) do
    if tag.name == name and tag.value then
      return "(" .. tag.value .. ")"
    end
  end
  return nil
end

-- " @lasted(2:30)" (or `@wasted` for `tag`): the time spent on the task `text` (from its
-- @started and @toggle tags) until `now`; "" without @started.
local function time_spent(text, tag, cfg, now)
  local started = tag_value(text, "started")
  if not started then
    return ""
  end
  local function time_of(value)
    local date = dates.strptime(value, cfg.date_format)
    return date and dates.time(date)
  end
  local start = time_of(started)
  -- the moment written in the stamp (to the minute, like the stamp)
  local finish = time_of(dates.format(now, cfg.date_format)) or now
  if not start then
    return ""
  end
  local toggles = {}
  for _, t in ipairs(syntax.tags(text)) do
    if t.name == "toggle" and t.value then
      local time = time_of("(" .. t.value .. ")")
      if time then
        table.insert(toggles, time)
      end
    end
  end
  local delta = dates.format_delta(dates.spent(start, toggles, finish), cfg.decimal_minutes)
  if delta == "" then
    delta = cfg.date_format:find("%H", 1, true) and "a bit" or "less than day"
  end
  return " @" .. tag .. "(" .. delta .. ")"
end

-- The time spent on the tasks under project line `index` (their @lasted, @wasted and @total):
-- " @total(...)", or "" when none.
local function project_total(lines, infos, index, cfg)
  local last = syntax.block_end(infos, index)
  local total, any = 0, false
  for i = index + 1, last do
    for _, tag in ipairs(syntax.tags(lines[i])) do
      if (tag.name == "lasted" or tag.name == "wasted" or tag.name == "total") and tag.value then
        total = total + dates.parse_delta(tag.value)
        any = true
      end
    end
  end
  if not any or total == 0 then
    return ""
  end
  return " @total(" .. dates.format_delta(total, cfg.decimal_minutes) .. ")"
end

-- Toggles line `index` done (`action` "done") or cancelled ("cancelled") at `now`: the new
-- line, or nil and a message.
function M.toggle(lines, infos, index, action, cfg, now)
  local line, info = lines[index], infos[index]
  local bullet = action == "done" and cfg.done_tasks_bullet or cfg.cancelled_tasks_bullet
  local spent_tag = action == "done" and "lasted" or "wasted"
  local stamp = M.stamp(action == "done" and "@done" or "@cancelled", cfg, now)
  local kind = info.kind
  if kind == "pending" then
    -- the bullet changes, the stamp and the time spent go at the end
    local after_bullet = line:sub(#info.indent + #info.bullet + 1)
    local text, double = split_double_space(after_bullet)
    return info.indent .. bullet .. text .. stamp .. time_spent(text, spent_tag, cfg, now) .. double
  elseif kind == "project" then
    -- a project becomes a task, its time the total of its tasks'
    local text, double = split_double_space(line:sub(#info.indent + 1))
    local new = info.indent .. bullet .. " " .. text .. stamp .. time_spent(text, spent_tag, cfg, now)
    local total = project_total(lines, infos, index, cfg)
    return new .. total .. double
  elseif kind == action then
    -- back to pending: without its ending
    local after_bullet = line:sub(#info.indent + #info.bullet + 1)
    local _, double = split_double_space(after_bullet)
    return info.indent .. cfg.open_tasks_bullet .. without_ending(after_bullet, action, cfg) .. double
  elseif kind == "cancelled" then
    -- cancelled, then done: the cancel stamp goes
    local after_bullet = line:sub(#info.indent + #info.bullet + 1)
    local text = without_ending(after_bullet, "cancelled", cfg)
    local _, double = split_double_space(after_bullet)
    return info.indent .. bullet .. text .. stamp .. time_spent(text, spent_tag, cfg, now) .. double
  elseif kind == "done" then
    return nil, "You cannot cancel what have been done, can you?"
  end
  return nil
end

-- Line `index`, a task done or cancelled, with its time spent worked out again (after its
-- @started, @toggle or stamp was changed): the new line, or nil.
function M.recalculate(lines, infos, index, cfg)
  local line, info = lines[index], infos[index]
  if info.kind ~= "done" and info.kind ~= "cancelled" then
    return nil
  end
  local tag = info.kind == "done" and "lasted" or "wasted"
  -- the moment it was done or cancelled, from its stamp
  local stamp_value = tag_value(info.text, info.kind) or tag_value(info.text, "done")
  local finished
  if stamp_value then
    local date = dates.strptime(stamp_value, cfg.date_format)
    finished = date and dates.time(date)
  end
  finished = finished or os.time()
  local text, double = split_double_space(line)
  text = text:gsub("[ \t]@[lw]asted%b()", "")
  local spent = time_spent(text, tag, cfg, finished)
  if spent == "" then
    return nil
  end
  return text .. spent .. double
end

-- The lines and infos of `text`.
function M.parse(text)
  local lines = syntax.lines(text)
  local infos = {}
  for i, line in ipairs(lines) do
    infos[i] = syntax.classify(line)
  end
  return lines, infos
end

-- The index of the archive's line ("Archive:"), or nil.
function M.archive_index(lines, cfg)
  for i, line in ipairs(lines) do
    local trimmed = line:gsub("^%s+", "")
    if trimmed:sub(1, #cfg.archive_name) == cfg.archive_name then
      return i
    end
  end
  return nil
end

-- The notes right after line `index` (indexes).
local function notes_after(infos, index)
  local notes = {}
  local i = index + 1
  while infos[i] and infos[i].kind == "note" do
    table.insert(notes, i)
    i = i + 1
  end
  return notes
end

-- Whether `format` allows the archive to be sorted by its stamps (they sort as text).
local function sortable(format)
  return format:find("%%[Yy][-./ ]*%%m[-./ ]*%%d%s*%%H.*%%M") ~= nil
end

-- The archive (after line `archive`) sorted by the tasks' stamps, newest first when
-- `cfg.new_on_top` (with their notes); the tasks without a stamp after them.
local function sort_archive(lines, archive, cfg)
  if not sortable(cfg.date_format) then
    return lines
  end
  local infos = {}
  for i, line in ipairs(lines) do
    infos[i] = syntax.classify(line)
  end
  local dated, rest = {}, {}
  local i = archive + 1
  while i <= #lines do
    local info = infos[i]
    local stamp = (info.kind == "done" or info.kind == "cancelled")
      and (tag_value(info.text, "done") or tag_value(info.text, "cancelled"))
    if stamp then
      local chunk = { key = stamp .. lines[i], index = i, lines = { lines[i] } }
      for _, note in ipairs(notes_after(infos, i)) do
        table.insert(chunk.lines, lines[note])
        i = note
      end
      table.insert(dated, chunk)
    else
      table.insert(rest, lines[i])
    end
    i = i + 1
  end
  table.sort(dated, function(a, b)
    if a.key ~= b.key then
      if cfg.new_on_top then
        return a.key > b.key
      end
      return a.key < b.key
    end
    return a.index < b.index
  end)
  local result = {}
  for j = 1, archive do
    result[j] = lines[j]
  end
  for _, chunk in ipairs(dated) do
    for _, line in ipairs(chunk.lines) do
      table.insert(result, line)
    end
  end
  for _, line in ipairs(rest) do
    table.insert(result, line)
  end
  return result
end

-- The lines with the done and cancelled tasks (and their notes) moved to the archive at the
-- end ("Archive:", made when missing), each with its project: the new lines and how many tasks
-- moved.
function M.archive(lines, cfg)
  local infos = {}
  for i, line in ipairs(lines) do
    infos[i] = syntax.classify(line)
  end
  local archive = M.archive_index(lines, cfg)
  local limit = archive or (#lines + 1)
  local moving, moved = {}, 0
  local archived = {}
  for i = 1, limit - 1 do
    local info = infos[i]
    if (info.kind == "done" or info.kind == "cancelled") and not moving[i] then
      moved = moved + 1
      moving[i] = true
      local project = table.concat(syntax.project_path(infos, i), " / ")
      local line = lines[i]
      local text, double = split_double_space(line:gsub("^%s+", ""))
      if cfg.project_tag then
        table.insert(archived, margin(cfg) .. text .. (project ~= "" and (" @project(" .. project .. ")") or "") .. double)
      else
        local after_bullet = text:sub(#info.bullet + 1)
        local prefix = project ~= "" and (cfg.tasks_bullet_space .. project .. ":") or ""
        table.insert(archived, margin(cfg) .. info.bullet .. prefix .. after_bullet .. double)
      end
      for _, note in ipairs(notes_after(infos, i)) do
        if note < limit then
          moving[note] = true
          table.insert(archived, margin(cfg) .. margin(cfg) .. (lines[note]:gsub("^%s+", "")))
        end
      end
    end
  end
  if moved == 0 then
    return lines, 0
  end
  local result = {}
  for i, line in ipairs(lines) do
    if not moving[i] then
      table.insert(result, line)
    end
    if i == archive then
      for _, new in ipairs(archived) do
        table.insert(result, new)
      end
    end
  end
  local archive_at
  if archive then
    archive_at = M.archive_index(result, cfg)
  else
    -- the archive goes at the end, under a line
    while #result > 0 and result[#result]:match("^%s*$") do
      table.remove(result)
    end
    table.insert(result, "")
    table.insert(result, "＿＿＿＿＿＿＿＿＿＿＿＿＿＿＿＿＿＿＿")
    table.insert(result, cfg.archive_name)
    archive_at = #result
    for _, new in ipairs(archived) do
      table.insert(result, new)
    end
  end
  return sort_archive(result, archive_at, cfg), moved
end

-- The subtree of line `index` (it and the lines under it): the first and last indexes, or nil
-- when nothing is under it.
function M.subtree(infos, index)
  local last = syntax.block_end(infos, index)
  if last == index then
    return nil
  end
  return index, last
end

-- What "archive to file" appends to the archive file: the subtree's lines under a separator
-- and the moment.
function M.archive_entry(subtree_lines, cfg, now)
  local entry = {
    "--- ✄ -----------------------",
    "Archived " .. dates.format(now, cfg.date_format) .. ":",
  }
  for _, line in ipairs(subtree_lines) do
    table.insert(entry, line)
  end
  return entry
end

-- The path of the archive file of `path` ("dir/list.todo": "dir/list_archive.todo"), with
-- `cfg.archive_org_filemask` ({dir}, {sep}, {base}, {ext}).
function M.archive_path(path, cfg)
  local dir, file = path:match("^(.*)[/\\]([^/\\]*)$")
  if not dir then
    dir, file = ".", path
  end
  local base, ext = file:match("^(.+)(%.[^%.]*)$")
  if not base then
    base, ext = file, ""
  end
  local sep = path:find("\\", 1, true) and not path:find("/", 1, true) and "\\" or "/"
  local values = { dir = dir, sep = sep, base = base, ext = ext }
  return (cfg.archive_org_filemask:gsub("{(%w+)}", function(name)
    return values[name] or ("{" .. name .. "}")
  end))
end

-- The priority of a task's text: 1 critical, 2 high, 3 normal, 4 low.
local function priority(text)
  if syntax.has_tag(text, "critical") then
    return 1
  elseif syntax.has_tag(text, "high") then
    return 2
  elseif syntax.has_tag(text, "low") then
    return 4
  end
  return 3
end

-- The lines with the tasks of project line `index` sorted by their @due date, then their
-- priority (`descending`: the other way), each with the lines under it; nil when the line is
-- not a project or has no tasks.
function M.sort_project(lines, infos, index, descending)
  if infos[index].kind ~= "project" then
    return nil
  end
  local last = syntax.block_end(infos, index)
  local chunks, first_task, depth = {}, nil, nil
  for i = index + 1, last do
    local info = infos[i]
    local width = syntax.indent_width(info.indent)
    if (syntax.is_task(info.kind) or info.kind == "project") and (not first_task or width == depth) then
      first_task = first_task or i
      depth = depth or width
      local due = tag_value(info.text, "due")
      table.insert(chunks, {
        due = due and due:sub(2, -2) or "99-01-01 00:00",
        priority = priority(info.text),
        index = #chunks + 1,
        lines = { lines[i] },
      })
    elseif first_task then
      table.insert(chunks[#chunks].lines, lines[i])
    end
  end
  if #chunks == 0 then
    return nil
  end
  table.sort(chunks, function(a, b)
    if a.due ~= b.due then
      if descending then
        return a.due > b.due
      end
      return a.due < b.due
    end
    if a.priority ~= b.priority then
      if descending then
        return a.priority > b.priority
      end
      return a.priority < b.priority
    end
    return a.index < b.index
  end)
  local result = {}
  for i = 1, first_task - 1 do
    table.insert(result, lines[i])
  end
  for _, chunk in ipairs(chunks) do
    -- the blank lines at the end of a task's lines go (like PlainTasks)
    local chunk_lines = chunk.lines
    while #chunk_lines > 1 and chunk_lines[#chunk_lines]:match("^%s*$") do
      table.remove(chunk_lines)
    end
    for _, line in ipairs(chunk_lines) do
      table.insert(result, line)
    end
  end
  for i = last + 1, #lines do
    table.insert(result, lines[i])
  end
  return result
end

-- The counts of the tasks of `infos`: pending, done, cancelled (before the archive when
-- `before`, an index, is given).
function M.counts(infos, before)
  local pending, done, cancelled = 0, 0, 0
  for i, info in ipairs(infos) do
    if before and i >= before then
      break
    end
    if info.kind == "pending" then
      pending = pending + 1
    elseif info.kind == "done" then
      done = done + 1
    elseif info.kind == "cancelled" then
      cancelled = cancelled + 1
    end
  end
  return pending, done, cancelled
end

-- The statistics of the task list for the status line, from `cfg.stats_format`:
--   $o pending, $d done, $c cancelled, $n done and cancelled, $a all, $percent, $progress
--   (a bar of `bar_full` and `bar_empty`), $last (the latest @done stamp); `{{...}}` gives
--   "pending/done/cancelled" of the tasks `matches` finds for the regex `...`.
-- `matches(regex)` gives the indexes of the lines with a match.
function M.stats(lines, infos, cfg, matches)
  local before = cfg.stats_ignore_archive and M.archive_index(lines, cfg) or nil
  local pending, done, cancelled = M.counts(infos, before)
  local all = pending + done + cancelled
  local percent = all > 0 and (done + cancelled) / all * 100 or 0
  local factor = percent < 90 and math.floor(percent / 10 + 0.5) or math.floor(percent / 10)
  local progress = factor > 0 and (string.rep(cfg.bar_full, factor) .. string.rep(cfg.bar_empty, 10 - factor)) or ""
  local stamps = {}
  for _, info in ipairs(infos) do
    if info.kind == "done" then
      local stamp = tag_value(info.text, "done")
      if stamp and dates.strptime(stamp, cfg.date_format) then
        table.insert(stamps, stamp)
      end
    end
  end
  table.sort(stamps, function(a, b) return a > b end)
  local last = stamps[1] or "(UNKNOWN)"
  local message = cfg.stats_format:gsub("{{(.-)}}", function(regex)
    local p, d, c = 0, 0, 0
    for _, index in ipairs(matches and matches(regex) or {}) do
      local kind = infos[index] and infos[index].kind
      if kind == "pending" then
        p = p + 1
      elseif kind == "done" then
        d = d + 1
      elseif kind == "cancelled" then
        c = c + 1
      end
    end
    return string.format("%d/%d/%d", p, d, c)
  end)
  local values = {
    ["$percent"] = tostring(math.floor(percent)),
    ["$progress"] = progress,
    ["$last"] = last,
    ["$o"] = tostring(pending),
    ["$d"] = tostring(done),
    ["$c"] = tostring(cancelled),
    ["$n"] = tostring(done + cancelled),
    ["$a"] = tostring(all),
  }
  -- the longer names first ($percent before $p...)
  for _, name in ipairs({ "$percent", "$progress", "$last", "$o", "$d", "$c", "$n", "$a" }) do
    message = message:gsub(name:gsub("%$", "%%$"), (values[name]:gsub("%%", "%%%%")))
  end
  return message
end

-- The @due tags of the lines that are not done or cancelled, with their state at `now`:
-- { { line, start, finish (bytes, the tag), value, state = "past"|"soon"|"later"|"invalid",
-- seconds (until the date; negative when past) } }.
function M.dues(lines, infos, cfg, now)
  local now_date = dates.date_of(now)
  -- the minute of now (short dates without a time take it)
  local found = {}
  for i, line in ipairs(lines) do
    local info = infos[i]
    if info.kind ~= "done" and info.kind ~= "cancelled" then
      for _, tag in ipairs(syntax.tags(line)) do
        if tag.name == "due" and tag.value then
          local created_value = tag_value(line, "created")
          local created
          if created_value then
            created = dates.parse(created_value, cfg.date_format, now_date)
          end
          local date = dates.due(tag.value, cfg.date_format, now_date, created)
          local entry = { line = i, start = tag.start, finish = tag.finish, value = tag.value }
          if not date then
            entry.state = "invalid"
          else
            local seconds = dates.time(date) - (now - now % 60)
            entry.seconds = seconds
            if seconds <= 0 then
              entry.state = "past"
            elseif cfg.highlight_due_soon > 0 and seconds < cfg.highlight_due_soon * 3600 then
              entry.state = "soon"
            else
              entry.state = "later"
            end
          end
          table.insert(found, entry)
        end
      end
    end
  end
  return found
end

-- The projects of the task list, for the picker: { { line, name ("Work / Errands") } }.
function M.projects(infos)
  local found = {}
  for i, info in ipairs(infos) do
    if info.kind == "project" then
      local path = syntax.project_path(infos, i)
      table.insert(path, syntax.project_name(info))
      table.insert(found, { line = i, name = table.concat(path, " / ") })
    end
  end
  return found
end

-- The tags of the tasks not done or cancelled (and @critical, @high, @low and @today
-- anywhere), for the picker: { { line, start, name, text } }.
function M.tag_list(lines, infos)
  local special = { critical = true, high = true, low = true, today = true }
  local found = {}
  for i, line in ipairs(lines) do
    local info = infos[i]
    local open = info.kind ~= "done" and info.kind ~= "cancelled"
    for _, tag in ipairs(syntax.tags(line)) do
      if open or special[tag.name] then
        table.insert(found, {
          line = i,
          start = tag.start,
          name = line:sub(tag.start, tag.finish),
        })
      end
    end
  end
  return found
end

return M
