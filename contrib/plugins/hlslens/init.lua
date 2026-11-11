-- hlslens: a port of nvim-hlslens (https://github.com/kevinhwang91/nvim-hlslens).
--
-- After a search (`/`, `?`, `n`, `N`, `*`, ...), each match in view gets a lens at the end
-- of its line: `[3/12]` for the nearest match (its index and the number of matches), `[5]`
-- for the others. The matches in view are highlighted, the nearest one apart. The lenses
-- follow the cursor, scrolling and edits, until `:hlslens-stop` (like Vim's `:nohlsearch`)
-- or the next search.
--
-- Settings (`config` of its `[[plugins.plugins]]` entry), with their defaults:
--
--   auto_enable = true          -- lenses after every search; false: after :hlslens-enable
--   calm_down = false           -- stop once the cursor leaves the matches, or on an edit
--   nearest_only = false        -- a lens on the nearest match only
--   near_lens = "[{index}/{total}]"  -- the nearest match's lens
--   lens = "[{index}]"          -- the other matches' lens ({arrow}: ▲ above, ▼ below)
--   lens_style = "ui.virtual"   -- theme keys: of the lenses,
--   near_lens_style = "ui.cursor.match"   -- of the nearest match's lens,
--   match_style = "ui.highlight"          -- of the matches ("" for none),
--   near_match_style = "ui.cursor.match"  -- of the nearest match ("" for none)
--   per_line = "one"            -- a line with several matches: "one" lens (the nearest
--                               -- match's, or the first one's), "all" of them, or "count":
--                               -- one lens and how many more matches the line has
--   more = " +{count}"          -- what "count" adds to the lens
--   incsearch = true            -- lenses while the search is typed
--   align = "eol_or_right"      -- after the end of the line, or at the window's right edge
--                               -- when the line is too long ("right": always there; "eol":
--                               -- below a line too long, like other annotations)
--   padding = 0                 -- columns between the end of the line and its lens, beyond
--                               -- the one Helix leaves
--   max_matches = 10000         -- matches counted at most (the total reads "10000+")

local config = {
  auto_enable = true,
  calm_down = false,
  nearest_only = false,
  near_lens = "[{index}/{total}]",
  lens = "[{index}]",
  lens_style = "ui.virtual",
  near_lens_style = "ui.cursor.match",
  match_style = "ui.highlight",
  near_match_style = "ui.cursor.match",
  per_line = "one",
  more = " +{count}",
  incsearch = true,
  align = "eol_or_right",
  padding = 0,
  max_matches = 10000,
}

for key, value in pairs(helix.get_config() or {}) do
  config[key] = value
end

-- The commands that search with the last pattern.
local SEARCHES = {
  search_next = true,
  search_prev = true,
  extend_search_next = true,
  extend_search_prev = true,
  search_selection = true,
  search_selection_detect_word_boundaries = true,
  make_search_word_bounded = true,
}

local enabled = config.auto_enable
local active = false      -- lenses shown (since the last search, until stopped)
local in_prompt = false   -- the search prompt is open
local typed = nil         -- what is typed in it (the lenses follow it with `incsearch`)
local decorated = {}      -- document ids of the buffers with lenses or highlights
local cache = nil         -- { document_id, pattern, matches, truncated }

-- The matches of `pattern` in `buffer`, kept until the buffer changes.
local function matches_of(buffer, pattern)
  local id = buffer:id()
  if cache and cache.document_id == id and cache.pattern == pattern then
    return cache.matches, cache.truncated
  end
  local ok, found = pcall(buffer.find_all, buffer, pattern, { max = config.max_matches + 1 })
  if not ok then
    found = {}
  end
  local truncated = #found > config.max_matches
  if truncated then
    found[#found] = nil
  end
  cache = { document_id = id, pattern = pattern, matches = found, truncated = truncated }
  return found, truncated
end

-- Removes the lenses and highlights of every buffer that has some.
local function clear_all()
  for id in pairs(decorated) do
    local buffer = helix.buffer.get_by_id(id)
    if buffer then
      buffer:set_highlights({})
      buffer:clear_annotations()
    end
  end
  decorated = {}
end

-- The index of the first match that ends after `cursor` (#matches + 1 when none does).
local function first_after(matches, cursor)
  local low, high = 1, #matches + 1
  while low < high do
    local mid = (low + high) // 2
    if matches[mid].to <= cursor then
      low = mid + 1
    else
      high = mid
    end
  end
  return low
end

-- The index of the match nearest to `cursor`: the one under it, or the closest one.
local function nearest(matches, cursor)
  local after = first_after(matches, cursor)
  if after <= #matches and matches[after].from <= cursor then
    return after
  end
  local before = after - 1
  if after > #matches then
    return before
  end
  if before < 1 then
    return after
  end
  if cursor - (matches[before].to - 1) <= matches[after].from - cursor then
    return before
  end
  return after
end

local function format(text, values)
  return (text:gsub("{(%w+)}", values))
end

-- The pattern the lenses are for: what is typed in the search prompt, or the last search.
local function current_pattern()
  if in_prompt then
    return config.incsearch and typed or nil
  end
  return helix.editor.last_search()
end

-- Draws the lenses and highlights of the matches in view, for the focused window.
local function refresh()
  if not enabled or not (active or (in_prompt and config.incsearch)) then
    return
  end
  local buffer = helix.buffer.get_current()
  local window = helix.window.get_current()
  if not buffer or not window then
    return
  end
  local pattern = current_pattern()
  local matches, truncated = {}, false
  if pattern and pattern ~= "" then
    matches, truncated = matches_of(buffer, pattern)
  end
  if #matches == 0 then
    clear_all()
    return
  end

  local cursor = buffer:get_cursor()
  local near = nearest(matches, cursor)
  local near_match = matches[near]
  if config.calm_down and not in_prompt
      and not (near_match.from <= cursor and cursor < near_match.to) then
    active = false
    clear_all()
    return
  end

  -- the matches in view, by line: from the top line to the bottom of the window
  local top = window:get_top_line()
  local bottom = top + window:get_area().height
  local last_line = buffer:line_count()
  local view_from = buffer:line_to_char(math.min(top, last_line - 1))
  local view_to = bottom < last_line and buffer:line_to_char(bottom) or buffer:len()
  local total = tostring(#matches) .. (truncated and "+" or "")

  local highlights, lines, order = {}, {}, {}
  local function add(index)
    local match = matches[index]
    local style = index == near and config.near_match_style or config.match_style
    if style and style ~= "" and match.to > match.from then
      highlights[#highlights + 1] = { from = match.from, to = match.to, style = style }
    end
    local line = buffer:char_to_line(match.from)
    if not lines[line] then
      lines[line] = {}
      order[#order + 1] = line
    end
    table.insert(lines[line], index)
  end
  for index = first_after(matches, view_from), #matches do
    if matches[index].from >= view_to then
      break
    end
    add(index)
  end
  if near_match.to <= view_from or near_match.from >= view_to then
    -- the nearest match is out of view (the loop above did not reach it)
    add(near)
  end

  local annotations = {}
  local function lens(index, extra)
    local is_near = index == near
    local arrow = index < near and "▲" or (is_near and "" or "▼")
    local values = { index = tostring(index), total = total, arrow = arrow }
    local text = format(is_near and config.near_lens or config.lens, values)
    if extra and extra > 0 then
      text = text .. format(config.more, { count = tostring(extra) })
    end
    annotations[#annotations + 1] = {
      char_idx = matches[index].from,
      text = text,
      style = is_near and config.near_lens_style or config.lens_style,
      offset = config.padding,
      align = config.align,
    }
  end
  for _, line in ipairs(order) do
    local indices = lines[line]
    local has_near = false
    for _, index in ipairs(indices) do
      has_near = has_near or index == near
    end
    if config.nearest_only then
      if has_near then
        lens(near)
      end
    elseif config.per_line == "all" then
      for _, index in ipairs(indices) do
        lens(index)
      end
    else
      local shown = has_near and near or indices[1]
      lens(shown, config.per_line == "count" and #indices - 1 or nil)
    end
  end

  -- the lenses of another buffer (the focus moved) go
  for id in pairs(decorated) do
    if id ~= buffer:id() then
      local other = helix.buffer.get_by_id(id)
      if other then
        other:set_highlights({})
        other:clear_annotations()
      end
      decorated[id] = nil
    end
  end
  buffer:set_highlights(highlights)
  buffer:set_annotations(annotations)
  decorated[buffer:id()] = true
end

local function start()
  active = true
  refresh()
end

local function stop()
  active = false
  clear_all()
end

helix.on("command", function(event)
  if SEARCHES[event.name] then
    start()
  end
end)

-- The search prompt: the lenses follow what is typed (`incsearch`); Enter shows those of
-- the new search, Esc those of the last one again (or none).
helix.on("prompt_open", function(event)
  if event.kind == "search" then
    in_prompt = true
    typed = event.text
    refresh()
  end
end)

helix.on("prompt_change", function(event)
  if in_prompt and event.kind == "search" then
    typed = event.text
    refresh()
  end
end)

helix.on("prompt_close", function(event)
  if not in_prompt or event.kind ~= "search" then
    return
  end
  in_prompt = false
  typed = nil
  if event.accepted then
    start()
  elseif active then
    refresh()
  else
    clear_all()
  end
end)

local function refresh_if_shown()
  if active or in_prompt then
    refresh()
  end
end

helix.on("selection_change", refresh_if_shown)
helix.on("scroll", refresh_if_shown)
helix.on("window_focus", refresh_if_shown)
helix.on("view_change", refresh_if_shown)

helix.on("buffer_changed", function(event)
  if cache and cache.document_id == event.document_id then
    cache = nil
  end
  if config.calm_down and active then
    stop()
  else
    refresh_if_shown()
  end
end)

helix.on("buffer_close", function(event)
  decorated[event.document_id] = nil
  if cache and cache.document_id == event.document_id then
    cache = nil
  end
end)

-- The commands need the `commands` permission; without it the lenses still work.
if helix.has_permission("commands") then
  helix.register_command({
    name = "hlslens-stop",
    doc = "Remove the search lenses until the next search (like :nohlsearch)",
    handler = stop,
  })
  helix.register_command({
    name = "hlslens-start",
    doc = "Show the search lenses of the last search",
    handler = function()
      enabled = true
      start()
    end,
  })
  helix.register_command({
    name = "hlslens-enable",
    doc = "Show search lenses after each search",
    handler = function()
      enabled = true
    end,
  })
  helix.register_command({
    name = "hlslens-disable",
    doc = "No search lenses (until :hlslens-enable)",
    handler = function()
      enabled = false
      stop()
    end,
  })
  helix.register_command({
    name = "hlslens-toggle",
    doc = "Turn the search lenses on or off",
    handler = function()
      if enabled then
        enabled = false
        stop()
      else
        enabled = true
        start()
      end
    end,
  })
end
