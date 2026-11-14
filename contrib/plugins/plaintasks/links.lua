-- Links in task lists: to files (`./file/`, `.\file:11:8`, `./file>symbol`, `./file"text"`,
-- Markdown's `[](path)`, wiki and Org-mode's `[[path]]`, `[[path::11:8]]`, `[[path::*symbol]]`,
-- `[[path::text]]`), and URLs (`<scheme:...>`, or a bare one: `https://...`, `www.example.com`).

local M = {}

-- The options after a link's path: ":line:column", ">symbol", "\"text\"".
local function options(rest)
  local link = {}
  local symbol = rest:match("^>([%w_]+)")
  if symbol then
    link.symbol = symbol
    rest = rest:sub(#symbol + 2)
  end
  local line, column = rest:match("^:(%d+):?(%d*)")
  if line then
    link.line = tonumber(line)
    link.column = tonumber(column)
    rest = rest:gsub("^:%d+:?%d*", "")
  end
  local text = rest:match('^"([^"\n]*)"')
  if text then
    link.text = text
  end
  return link
end

-- `./path` or `.\path` (after a space or at the start): the path ends before `:`, `"` or `>`,
-- or at its last slash ("./another filename/").
local function dot_link(line)
  local position = 1
  while true do
    local start = line:find("%.[\\/]", position)
    if not start then
      return nil
    end
    local before = start == 1 and " " or line:sub(start - 1, start - 1)
    if before:match("[ \t]") then
      local rest = line:sub(start + 2)
      -- a Windows drive ("c:\") is part of the path
      local drive = rest:match("^%a:[\\/]") or ""
      local run = drive .. rest:sub(#drive + 1):match('^[^:">\n]*')
      local after = rest:sub(#run + 1)
      local path
      if after:match('^[:">]') then
        path = run:gsub("[\\/]$", "")
      else
        path = run:match("^(.*)[\\/]")
      end
      if path and path ~= "" then
        local link = options(after)
        link.path = path
        return link
      end
    end
    position = start + 2
  end
end

-- `[...](path)` or `[...](path ":11:8")`, `(<path>)`, `(file:///path)`.
local function markdown_link(line)
  local inner = line:match("%][ \t]*(%b())")
  if not inner then
    return nil
  end
  inner = inner:sub(2, -2)
  local path, quoted = inner:match('^(.-)%s*"(.*)"%s*$')
  path = path or inner
  path = path:gsub("^<", ""):gsub(">$", ""):gsub("^file:///?", "/")
  path = path:gsub("\\%(", "("):gsub("\\%)", ")")
  if path == "" then
    return nil
  end
  local link = quoted and options(quoted) or {}
  if quoted and not (link.line or link.symbol or link.text) and quoted ~= "" then
    link.text = quoted
  end
  link.path = path
  return link
end

-- `[[path]]`, `[[path][description]]`, `[[path::11:8]]`, `[[path::*symbol]]`,
-- `[[path::text]]` (Org-mode), `[[path]] ":11:8"` (nvALT); `file:`, `file+sys:` and
-- `./` before the path are dropped.
local function wiki_link(line)
  local start = line:find("[[", 1, true)
  if not start then
    return nil
  end
  local close = line:find("]", start + 2, true)
  if not close then
    return nil
  end
  local target = line:sub(start + 2, close - 1)
  target = target:gsub("^file%+?%a*:", ""):gsub("^%.[\\/]", "")
  local path, option = target:match("^(.-)::(.*)$")
  path = path or target
  local link = {}
  if option then
    local row, column = option:match("^(%d+):?(%d*)$")
    if row then
      link.line, link.column = tonumber(row), tonumber(column)
    elseif option:sub(1, 1) == "*" then
      link.symbol = option:sub(2)
    elseif option ~= "" then
      link.text = option
    end
  else
    local rest = line:sub(close + 1):gsub("^%[.-%]", ""):gsub("^%]", "")
    local quoted = rest:match('^[ \t]*"(.-)"')
    if quoted then
      link = options(quoted)
      if not (link.line or link.symbol or link.text) and quoted ~= "" then
        link.text = quoted
      end
    end
  end
  path = path:gsub("\\%[", "["):gsub("\\%]", "]")
  if path == "" then
    return nil
  end
  link.path = path
  return link
end

-- The link to a file of `line`: { path, line, column, symbol, text }, or nil.
function M.file_link(line)
  return dot_link(line) or markdown_link(line) or wiki_link(line)
end

-- The URL at byte `position` of `line` (1-based): the URL, and its start and end (bytes).
-- `<...>` wraps any scheme (`<skype:nick>`); a bare one is `scheme://...` or `www...`
-- (http then).
function M.url_at(line, position)
  -- in <...>
  for start, url, finish in line:gmatch("()<([%w%+%-%.]+:[^%s>]+)>()") do
    if position >= start and position < finish then
      return url, start + 1, finish - 2
    end
  end
  -- a bare one: the word around the position (up to a space, a quote or angle brackets)
  local start, finish = position, position
  local stop = "[%s\"'<>,]"
  while start > 1 and not line:sub(start - 1, start - 1):match(stop) do
    start = start - 1
  end
  while finish <= #line and not line:sub(finish, finish):match(stop) do
    finish = finish + 1
  end
  local word = line:sub(start, finish - 1)
  local scheme_url = word:match("^%a[%w%+%-%.]*://%S+")
  if scheme_url then
    return (scheme_url:gsub("[%.,;:!%?%)]+$", "")), start, start + #scheme_url - 1
  end
  local www = word:match("^www%d?%d?%d?%.%S+") or word:match("^[%w%.%-]+%.%a%a%a?%a?/%S*")
  if www then
    www = www:gsub("[%.,;:!%?%)]+$", "")
    return "http://" .. www, start, start + #www - 1
  end
  return nil
end

-- The places to look for the file `path` of a link in the task list at `list_path` (or the
-- working directory): absolute paths as they are; relative ones next to the list, then in
-- `cwd`.
function M.candidates(path, list_path, cwd)
  path = path:gsub("\\", "/")
  if path:match("^/") or path:match("^~") or path:match("^%a:/") then
    return { path }
  end
  local found = {}
  if list_path then
    local dir = list_path:gsub("\\", "/"):match("^(.*)/[^/]*$")
    if dir then
      table.insert(found, dir .. "/" .. path)
    end
  end
  if cwd then
    table.insert(found, (cwd:gsub("\\", "/"):gsub("/$", "")) .. "/" .. path)
  end
  return found
end

return M
