-- The PlainTasks format: what each line of a task list is, and where its parts are.
--
-- A line is one of:
--   project    a line ending with a colon, tags allowed after it ("Errands:", "Work: @today")
--   pending    a task to do: an open bullet then a space ("☐ buy milk", "- buy milk", "[ ] x")
--   done       a done task ("✔ buy milk @done (24-11-13 18:00)", "[x] ...", "- ... @done")
--   cancelled  a cancelled task ("✘ ...", "x ...", "[-] ...", "- ... @cancelled")
--   separator  "--- ✄ -----------------------" (three dashes, 3 to 5 characters, dashes)
--   archive    the line of "＿" above the archive
--   note       any other text
--   blank      nothing but spaces
--
-- Positions are byte positions in the line (1-based, as Lua's string functions take them);
-- `char_col` turns them into the character columns Helix counts.

local M = {}

-- The bullets of each kind (the configured ones are added by `M.setup`).
local OPEN = { "-", "❍", "❑", "■", "□", "☐", "▪", "▫", "–", "—", "≡", "→", "›", "[ ]" }
local DONE = { "+", "✓", "✔", "☑", "√", "[x]" }
local CANCELLED = { "✘", "❌", "x", "[-]" }

local bullets = { open = OPEN, done = DONE, cancelled = CANCELLED }

local function contains(list, value)
  for _, item in ipairs(list) do
    if item == value then
      return true
    end
  end
  return false
end

-- Adds the configured bullets to those recognized (a custom one, say).
function M.setup(config)
  for kind, key in pairs({
    open = "open_tasks_bullet",
    done = "done_tasks_bullet",
    cancelled = "cancelled_tasks_bullet",
  }) do
    local bullet = config[key]
    if type(bullet) == "string" and bullet ~= "" and not contains(bullets[kind], bullet) then
      table.insert(bullets[kind], bullet)
    end
  end
end

-- The bullet `rest` starts with, from `list`, if a space follows it: the longest one.
local function bullet_at(rest, list)
  local found
  for _, bullet in ipairs(list) do
    if rest:sub(1, #bullet) == bullet and rest:sub(#bullet + 1, #bullet + 1):match("^%s$") then
      if not found or #bullet > #found then
        found = bullet
      end
    end
  end
  return found
end

-- Whether a tag of `name` is in `text` (followed by a space, "(" or the end).
function M.has_tag(text, name)
  for start, finish in text:gmatch("()@" .. name .. "()") do
    local before = start == 1 and " " or text:sub(start - 1, start - 1)
    local after = text:sub(finish, finish)
    if before:match("%s") and (after == "" or after:match("[%s%(]")) then
      return true
    end
  end
  return false
end

-- A tag: "@" after a space (or the start), a word, and maybe a parenthesized value.
-- `text` from byte `init`: the start, end and name of the next tag, and its value.
function M.next_tag(text, init)
  local position = init or 1
  while true do
    local start = text:find("@", position, true)
    if not start then
      return nil
    end
    local before = start == 1 and " " or text:sub(start - 1, start - 1)
    -- a name: letters, digits, "_" or "-" (and any non-ASCII letter)
    local name = text:match("^[%w_%-\128-\255]+", start + 1)
    if before:match("%s") and name then
      local finish = start + #name
      local value
      -- the stamps of done and cancelled tasks have a space before their date
      local gap = (name == "done" or name == "cancelled") and #text:match("^[ \t]*", finish + 1) or 0
      if text:sub(finish + gap + 1, finish + gap + 1) == "(" then
        local close = text:find(")", finish + gap + 2, true)
        if close then
          value = text:sub(finish + gap + 2, close - 1)
          finish = close
        end
      end
      return start, finish, name, value
    end
    position = start + 1
  end
end

-- All the tags of `text`: { { start, finish, name, value } }.
function M.tags(text)
  local found, position = {}, 1
  while true do
    local start, finish, name, value = M.next_tag(text, position)
    if not start then
      return found
    end
    table.insert(found, { start = start, finish = finish, name = name, value = value })
    position = finish + 1
  end
end

-- `text` without the tags (and spaces) at its end.
local function strip_trailing_tags(text)
  while true do
    text = text:gsub("%s+$", "")
    local before = text:match("^(.-)%s+@[^%s%(]+%b()$") or text:match("^(.-)%s+@%S+$")
    if not before then
      return text
    end
    text = before
  end
end

-- The characters a project's first character may not be (bullets, separators, "＿").
local NOT_PROJECT = { "＿", "✄" }

local function starts_a_project(rest)
  -- "#" and a space may come before the name, like a Markdown heading
  local name = rest:match("^#%s?(.*)$") or rest
  local first = name:match("^[%z\1-\127\194-\244][\128-\191]*")
  if not first then
    return false
  end
  if #first == 1 then
    return first:match("[%w_]") ~= nil
  end
  -- a non-ASCII letter: anything but the bullets and lines
  for _, list in pairs(bullets) do
    if contains(list, first) then
      return false
    end
  end
  return not contains(NOT_PROJECT, first)
end

-- Whether `text` (after its indent) is a separator: "---", 3 to 5 characters, "---" or more.
local function is_separator(text)
  if text:sub(1, 3) ~= "---" then
    return false
  end
  local chars = {}
  for char in text:sub(4):gmatch("[%z\1-\127\194-\244][\128-\191]*") do
    table.insert(chars, char)
  end
  for middle = 3, 5 do
    local rest = table.concat(chars, "", middle + 1)
    if #chars >= middle + 3 and rest:match("^%-%-%-+$") then
      return true
    end
  end
  return false
end

-- What `line` (without its line ending) is: a table with
--   kind     see the top of this file
--   indent   the leading spaces and tabs
--   bullet   the bullet of a task
--   text     a task's text after its bullet and the spaces after it; a project's line, trimmed
--   text_at  where that text starts (a byte position in the line)
function M.classify(line)
  local indent, rest = line:match("^(%s*)(.*)$")
  local info = { indent = indent, kind = "note", text = rest, text_at = #indent + 1 }
  if rest == "" then
    info.kind = "blank"
    return info
  end
  -- (＿ is several bytes: a pattern's `+` would repeat its last one)
  if rest:sub(1, #"＿") == "＿" and rest:gsub("＿", ""):match("^%s*$") then
    info.kind = "archive"
    return info
  end
  if is_separator(rest:gsub("%s+$", "")) then
    info.kind = "separator"
    return info
  end
  -- a project ends with a colon, or a colon and tags (checked first, like PlainTasks)
  if starts_a_project(rest) and strip_trailing_tags(rest):sub(-1) == ":" then
    info.kind = "project"
    return info
  end
  for _, kind in ipairs({ "done", "cancelled", "open" }) do
    local bullet = bullet_at(rest, bullets[kind])
    if bullet then
      local spaces = rest:match("^%s+", #bullet + 1)
      info.bullet = bullet
      info.text_at = #indent + #bullet + #spaces + 1
      info.text = rest:sub(#bullet + #spaces + 1)
      if kind == "open" then
        -- "- task @done" is done, "- task @cancelled" cancelled
        if M.has_tag(info.text, "done") then
          info.kind = "done"
        elseif M.has_tag(info.text, "cancelled") then
          info.kind = "cancelled"
        else
          info.kind = "pending"
        end
      else
        info.kind = kind
      end
      return info
    end
  end
  return info
end

-- Whether a line of `kind` is a task.
function M.is_task(kind)
  return kind == "pending" or kind == "done" or kind == "cancelled"
end

-- The character column of byte position `byte` of `line` (0-based).
function M.char_col(line, byte)
  if byte <= 1 then
    return 0
  end
  return utf8.len(line, 1, byte - 1) or (byte - 1)
end

-- The lines of `text`, without their line endings (no last empty one for a final line ending).
function M.lines(text)
  local lines = {}
  for line in (text .. "\n"):gmatch("(.-)\r?\n") do
    table.insert(lines, line)
  end
  if text:sub(-1) == "\n" then
    table.remove(lines)
  end
  return lines
end

-- The width of an indent, tabs counted as `tab_width` columns.
function M.indent_width(indent, tab_width)
  local width = 0
  for char in indent:gmatch(".") do
    width = width + (char == "\t" and (tab_width or 4) or 1)
  end
  return width
end

-- The projects (and separators) the line `index` of `infos` is in, outermost first: the
-- lines before it less indented than it and than each other.
function M.project_path(infos, index)
  local path = {}
  local depth = M.indent_width(infos[index].indent)
  for i = index - 1, 1, -1 do
    local info = infos[i]
    local width = M.indent_width(info.indent)
    if (info.kind == "project" or info.kind == "separator") and width < depth then
      if info.kind == "project" then
        local name = strip_trailing_tags(info.text):gsub("^#%s?", ""):gsub(":$", "")
        table.insert(path, 1, name)
      end
      depth = width
      if depth == 0 then
        break
      end
    end
  end
  return path
end

-- The name of a project line (without its colon and tags).
function M.project_name(info)
  return (strip_trailing_tags(info.text):gsub("^#%s?", ""):gsub(":$", ""))
end

-- The lines after line `index` that belong to it: more indented, or blank among them.
-- The index of the last one (`index` when none).
function M.block_end(infos, index)
  local depth = M.indent_width(infos[index].indent)
  local last = index
  for i = index + 1, #infos do
    local info = infos[i]
    if info.kind ~= "blank" then
      if M.indent_width(info.indent) <= depth then
        break
      end
      last = i
    end
  end
  return last
end

M.bullets = bullets
M.strip_trailing_tags = strip_trailing_tags

return M
