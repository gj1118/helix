-- pile: the chunk and line_num parts of hlchunk.nvim
-- (https://github.com/shellRaining/hlchunk.nvim).
--
-- chunk: the block of code around the cursor (a function, a loop, an if, ...) gets a line
-- down its left side, joined to its first and last lines and drawn over their indentation,
-- never over the code:
--
--   for item in list {
--   ╭───if item.ready {
--   │       item.run();
--   ╰──>}
--   }
--
-- line_num: the line numbers of that block are coloured.
--
-- Each part is on or off on its own, in the `config` of its `[[plugins.plugins]]` entry
-- (the defaults below). A style is a table like in a theme ({ fg = "#806d9c", modifiers =
-- { "bold" } }), a colour ("#806d9c") or a theme key ("comment").
--
--   chunk = {
--     enable = true,
--     style = { fg = "#806d9c" },        -- the line (or a list of two styles, like hlchunk's:
--     error_style = { fg = "#c21f30" },  --   the line, then the line of a block with an error)
--     error_sign = true,                 -- error_style when the block has a syntax error
--     use_treesitter = true,             -- the block from the syntax tree; false (or no syntax
--                                        --   tree): between the braces around the cursor
--     chars = { horizontal_line = "─", vertical_line = "│", left_top = "╭",
--               left_bottom = "╰", right_arrow = ">" },
--     delay = 300,                       -- ms the cursor rests before another block is drawn,
--                                        --   with an animation; 0: at once, not animated
--     duration = 200,                    -- ms the animation takes; 0: not animated
--     max_file_size = 1048576,           -- larger buffers (in characters) are left alone
--     exclude_languages = {},            -- e.g. { "markdown", "text" }
--     textobject = "",                   -- a key selecting the block's lines (e.g. "A-c");
--                                        --   needs the "keymaps" and "edit" permissions
--     node_types = { ... },              -- the syntax nodes that make a block (Lua patterns
--                                        --   of their types, see NODE_TYPES)
--   },
--   line_num = {
--     enable = false,
--     style = { fg = "#806d9c" },
--     use_treesitter = true,
--     max_file_size = 1048576,
--     exclude_languages = {},
--   },
--
-- Commands (with the "commands" permission): :pile-enable, :pile-disable, :pile-toggle, for
-- both parts or the one named (`:pile-toggle chunk`, `:pile-disable line_num`), and
-- :pile-status, which parts are on and the lines of the block. The parts it starts with are
-- written to the log (:log-open).

-- The syntax nodes that make a block: Lua patterns of their types (hlchunk's, and a few more
-- for Rust, Go and others). "case" is a case of a switch (Go's expression_case and
-- default_case, JavaScript's switch_case, C's case_statement, Python's case_clause...): with
-- the cursor in one, the block is the case, not the whole switch.
local NODE_TYPES = {
  "class", "^func", "method", "closure", "lambda", "^if", "while", "for", "loop", "with",
  "try", "except", "catch", "match", "switch", "case", "arguments", "argument_list", "object",
  "dictionary", "element", "table", "tuple", "array", "do_block", "impl", "trait", "struct",
  "enum", "interface", "mod_item", "module", "namespace",
}

local DEFAULTS = {
  chunk = {
    enable = true,
    style = { fg = "#806d9c" },
    error_style = { fg = "#c21f30" },
    error_sign = true,
    use_treesitter = true,
    chars = {
      horizontal_line = "─",
      vertical_line = "│",
      left_top = "╭",
      left_bottom = "╰",
      right_arrow = ">",
    },
    delay = 300,
    duration = 200,
    max_file_size = 1024 * 1024,
    exclude_languages = {},
    textobject = "",
    node_types = NODE_TYPES,
  },
  line_num = {
    enable = false,
    style = { fg = "#806d9c" },
    use_treesitter = true,
    max_file_size = 1024 * 1024,
    exclude_languages = {},
  },
}

local problems = {}

-- `defaults` with what `given` sets over it: a setting of the wrong type is reported and
-- left at its default; `chars` is merged key by key.
local function merge(name, defaults, given)
  local merged = {}
  for key, value in pairs(defaults) do
    merged[key] = value
  end
  if type(given) ~= "table" then
    if given ~= nil then
      table.insert(problems, name .. " should be a table")
    end
    return merged
  end
  for key, value in pairs(given) do
    local default = defaults[key]
    if default == nil then
      table.insert(problems, "unknown setting " .. name .. "." .. tostring(key))
    elseif key == "style" or key == "error_style" then
      merged[key] = value
    elseif type(value) ~= type(default) then
      table.insert(problems, name .. "." .. key .. " should be a " .. type(default))
    elseif key == "chars" then
      merged.chars = merge(name .. ".chars", default, value)
    else
      merged[key] = value
    end
  end
  return merged
end

local given = helix.get_config() or {}
local config = {
  chunk = merge("chunk", DEFAULTS.chunk, given.chunk),
  line_num = merge("line_num", DEFAULTS.line_num, given.line_num),
}
for key in pairs(given) do
  if key ~= "chunk" and key ~= "line_num" then
    table.insert(problems, "unknown setting " .. tostring(key) .. " (chunk or line_num?)")
  end
end

-- The fields of a style: a table, a colour or a theme key.
local function style_fields(style)
  if type(style) == "string" then
    if style:sub(1, 1) == "#" then
      return { fg = style }
    end
    return { style = style }
  elseif type(style) == "table" then
    return style
  end
  return {}
end

local chunk_style, chunk_error_style
if type(config.chunk.style) == "table" and config.chunk.style[1] ~= nil then
  -- hlchunk's list: the line, then the line of a block with an error
  chunk_style = style_fields(config.chunk.style[1])
  chunk_error_style = style_fields(config.chunk.style[2] or config.chunk.error_style)
else
  chunk_style = style_fields(config.chunk.style)
  chunk_error_style = style_fields(config.chunk.error_style)
end
local line_num_style = style_fields(config.line_num.style)

-- `item` with the fields of `style`.
local function styled(item, style)
  for key, value in pairs(style) do
    item[key] = value
  end
  return item
end

local on = { chunk = config.chunk.enable, line_num = config.line_num.enable }

-- What the parts are switched to, and the block of the focused window.
local function status()
  local function state(part)
    return part .. " " .. (on[part] and "on" or "off")
  end
  return "pile: " .. state("chunk") .. ", " .. state("line_num")
end
helix.log.info(status() .. " (the settings of its [[plugins.plugins]] entry)")

-- What is drawn, to leave it be while it stays the same: per part, a key of the buffer,
-- window, block and visible lines, and (chunk) the key of the block alone.
local drawn = { chunk = nil, chunk_block = nil, line_num = nil }
local animation = nil -- the animation's timer
local waiting = nil   -- the timer of a block waiting for the cursor to rest

local function cancel(timer)
  if timer then
    helix.cancel(timer)
  end
  return nil
end

-- Finding the block ---------------------------------------------------------------------

local function suits(kind, patterns)
  for _, pattern in ipairs(patterns) do
    if kind:find(pattern) then
      return true
    end
  end
  return false
end

-- The block around char `cursor` in the syntax tree: the smallest node over several lines
-- of a type of `node_types`. { first, last (1-based lines), error } or nil.
local function block_by_syntax(buffer, cursor)
  local ok, node = pcall(buffer.syntax_node, buffer, cursor)
  while ok and node do
    local last = node.end_line
    -- a node ending with its line's line ending ends on that line
    if node.end_col == 0 and last > node.start_line then
      last = last - 1
    end
    if last > node.start_line and suits(node.type, config.chunk.node_types) then
      return { first = node.start_line, last = last, error = node.has_error }
    end
    ok, node = pcall(buffer.syntax_parent, buffer, node)
  end
  return nil
end

-- Lines looked at each way for the braces around the cursor.
local SCAN = 2000

-- The block between the braces around char `cursor`: { first, last } or nil.
local function block_by_braces(buffer, cursor)
  local line = buffer:char_to_line(cursor) + 1
  local count = buffer:line_count()
  local from = math.max(1, line - SCAN)
  local to = math.min(count, line + SCAN)
  local lines = buffer:get_lines(from, to)
  local here = lines[line - from + 1] or ""
  local col = cursor - buffer:line_to_char(line - 1)
  local byte = utf8.offset(here, col + 1) or (#here + 1)
  -- backwards from before the cursor (from the cursor when it is on a `{`)
  local open
  local depth = 0
  local start = here:sub(byte, byte) == "{" and byte or byte - 1
  for i = line, from, -1 do
    local text = lines[i - from + 1]
    for b = (i == line) and start or #text, 1, -1 do
      local c = text:sub(b, b)
      if c == "}" then
        depth = depth + 1
      elseif c == "{" then
        if depth == 0 then
          open = i
          break
        end
        depth = depth - 1
      end
    end
    if open then
      break
    end
  end
  if not open then
    return nil
  end
  -- forwards from after it (from the cursor when it is not on a `{`)
  local close
  depth = 0
  start = here:sub(byte, byte) == "{" and byte + 1 or byte
  for i = line, to do
    local text = lines[i - from + 1]
    for b = (i == line) and start or 1, #text do
      local c = text:sub(b, b)
      if c == "{" then
        depth = depth + 1
      elseif c == "}" then
        if depth == 0 then
          close = i
          break
        end
        depth = depth - 1
      end
    end
    if close then
      break
    end
  end
  if not close or close == open then
    return nil
  end
  return { first = open, last = close, error = false }
end

-- Whether `part` works in `buffer`.
local function applies(buffer, part)
  local language = buffer:get_language()
  for _, excluded in ipairs(part.exclude_languages) do
    if excluded == language then
      return false
    end
  end
  return buffer:len() <= part.max_file_size
end

-- The blocks found while handling an event (both parts look for it), by how.
local found = nil

local function find_block(buffer, part)
  if not applies(buffer, part) then
    return nil
  end
  local how = (part.use_treesitter and buffer:has_syntax()) and "syntax" or "braces"
  if found and found[how] ~= nil then
    return found[how] or nil
  end
  local cursor = buffer:get_cursor()
  -- in the indentation of a line, the block is that of the line's code
  local line = buffer:char_to_line(cursor)
  local start = buffer:line_to_char(line)
  local text = buffer:get_line(line + 1):gsub("\r?\n$", "")
  local indent = #text:match("^[ \t]*")
  if cursor - start < indent and indent < #text then
    cursor = start + indent
  end
  local block
  if how == "syntax" then
    block = block_by_syntax(buffer, cursor)
  else
    block = block_by_braces(buffer, cursor)
  end
  if found then
    found[how] = block or false
  end
  return block
end

-- Drawing the chunk ---------------------------------------------------------------------

-- The width of the indentation of `text`, and whether the line is blank.
local function indent_of(text, tab_width)
  local col = 0
  for i = 1, #text do
    local c = text:byte(i)
    if c == 32 then
      col = col + 1
    elseif c == 9 then
      col = col + tab_width - col % tab_width
    else
      return col, false
    end
  end
  return col, true
end

local function without_ending(text)
  return (text:gsub("\r?\n$", ""))
end

-- The cells of the chunk of `block` on lines `first_shown` to `last_shown`, in the order
-- the animation draws them: from the first line's code leftwards to the corner, down, then
-- right to the arrow before the last line's code.
local function chunk_cells(buffer, block, first_shown, last_shown)
  local chars = config.chunk.chars
  local info = buffer:get_info()
  local tab_width = math.max(info.tab_width, 1)
  local shift = info.indent_style == "tabs" and tab_width or math.max(info.indent_width, 1)
  local first_indent = indent_of(without_ending(buffer:get_line(block.first)), tab_width)
  local last_indent = indent_of(without_ending(buffer:get_line(block.last)), tab_width)
  local col = math.max(math.min(first_indent, last_indent) - shift, 0)

  local cells = {}
  -- `rows`: on which rows of a soft-wrapped line ("first" by default, "wrapped", "all")
  local function add(line, at, text, rows)
    if line >= first_shown and line <= last_shown then
      table.insert(cells, { line = line, col = at, text = text, rows = rows })
    end
  end
  for at = first_indent - 1, col, -1 do
    add(block.first, at, at == col and chars.left_top or chars.horizontal_line)
  end
  -- down the rows the first line is wrapped onto, then beside every row of the lines between
  add(block.first, col, chars.vertical_line, "wrapped")
  local from = math.max(block.first + 1, first_shown)
  local to = math.min(block.last - 1, last_shown)
  if from <= to then
    for i, text in ipairs(buffer:get_lines(from, to)) do
      local indent, blank = indent_of(text, tab_width)
      if blank or indent > col then
        add(from + i - 1, col, chars.vertical_line, "all")
      end
    end
  end
  for at = col, last_indent - 1 do
    local text = chars.horizontal_line
    if at == col then
      text = chars.left_bottom
    elseif at == last_indent - 1 then
      text = chars.right_arrow
    end
    add(block.last, at, text)
  end
  return cells
end

-- Shows the first `count` cells (all of them without a count).
local function show(buffer, cells, style, count)
  local list = {}
  for i = 1, count or #cells do
    local cell = cells[i]
    list[i] = styled({ line = cell.line, col = cell.col, text = cell.text, rows = cell.rows },
      style)
  end
  buffer:set_overlays(list)
end

-- Draws the cells one after the other over `duration` ms (by the clock: firings of the
-- timer skipped while the editor is busy do not slow it down).
local function animate(buffer, cells, style)
  animation = cancel(animation)
  local started = helix.now()
  buffer:set_overlays({})
  animation = helix.every(16, function()
    local share = math.min(1, (helix.now() - started) / config.chunk.duration)
    local count = math.max(1, math.ceil(#cells * share))
    -- the buffer may no longer be focused: the animation ends
    if not pcall(show, buffer, cells, style, count) or count >= #cells then
      animation = cancel(animation)
    end
  end)
end

-- The lines worth drawing in `window`: those shown, and a screen more each way for a
-- smooth scroll on its way.
local function shown_lines(window)
  local top = window:get_top_line() + 1
  local height = window:get_area().height
  return math.max(1, top - height), top + 2 * height + 20
end

local function clear_chunk(buffer)
  animation = cancel(animation)
  waiting = cancel(waiting)
  pcall(buffer.set_overlays, buffer, {})
  drawn.chunk, drawn.chunk_block = nil, nil
end

-- `moved`: the cursor moved (another block waits for it to rest and is animated); else the
-- text changed or the view did: drawn at once.
local function update_chunk(buffer, window, moved)
  if not on.chunk then
    return
  end
  local block = find_block(buffer, config.chunk)
  if not block then
    clear_chunk(buffer)
    return
  end
  local faulty = block.error and config.chunk.error_sign
  local first_shown, last_shown = shown_lines(window)
  local block_key = table.concat({ buffer:id(), window:id(), block.first, block.last,
    tostring(faulty) }, ":")
  local key = block_key .. ":" .. first_shown
  if key == drawn.chunk then
    -- back on the block drawn: a block waiting to be drawn is not
    waiting = cancel(waiting)
    return
  end
  local animated = moved and block_key ~= drawn.chunk_block
    and config.chunk.delay > 0 and config.chunk.duration > 0
  local function draw()
    waiting = cancel(waiting)
    local cells = chunk_cells(buffer, block, first_shown, last_shown)
    local style = faulty and chunk_error_style or chunk_style
    if animated then
      animate(buffer, cells, style)
    else
      animation = cancel(animation)
      show(buffer, cells, style)
    end
    drawn.chunk, drawn.chunk_block = key, block_key
  end
  if animated then
    -- after the cursor has rested `delay` ms (still on this block)
    waiting = cancel(waiting)
    waiting = helix.defer(config.chunk.delay, function()
      waiting = nil
      local ok, current = pcall(helix.buffer.get_current)
      if ok and current:id() == buffer:id() then
        local now = find_block(buffer, config.chunk)
        if now and now.first == block.first and now.last == block.last then
          pcall(draw)
        end
      end
    end)
  else
    draw()
  end
end

-- Line numbers --------------------------------------------------------------------------

local function update_line_num(buffer, window)
  if not on.line_num then
    return
  end
  local block = find_block(buffer, config.line_num)
  if not block then
    pcall(buffer.set_line_number_styles, buffer, {})
    drawn.line_num = nil
    return
  end
  local key = table.concat({ buffer:id(), window:id(), block.first, block.last }, ":")
  if key ~= drawn.line_num then
    buffer:set_line_number_styles({
      styled({ start_line = block.first, end_line = block.last }, line_num_style),
    })
    drawn.line_num = key
  end
end

-- Events ----------------------------------------------------------------------------------

-- How the view changed: "move" (the cursor), "edit" (the text) or "view" (scrolled,
-- another window or buffer, a setting).
local function update(how)
  local ok, buffer = pcall(helix.buffer.get_current)
  if not ok or not buffer then
    return
  end
  local window = helix.window.get_current()
  if how == "edit" then
    -- the same lines may have other indentation: drawn again
    drawn.chunk, drawn.line_num = nil, nil
  end
  found = {}
  local ok_chunk, problem = pcall(update_chunk, buffer, window, how == "move")
  local ok_num, problem_num = pcall(update_line_num, buffer, window)
  found = nil
  if not ok_chunk or not ok_num then
    error(problem or problem_num)
  end
end

helix.on("selection_change", function()
  update("move")
end)
helix.on("buffer_changed", function()
  update("edit")
end)
for _, event in ipairs({ "scroll", "window_focus", "buffer_focus_lost", "buffer_open",
  "view_change", "config_change", "language_change" }) do
  helix.on(event, function()
    update("view")
  end)
end
helix.on("ready", function()
  update("view")
end)
helix.on("init", function()
  update("view")
end)

-- Commands --------------------------------------------------------------------------------

local PARTS = { chunk = true, line_num = true }

-- The parts named by `args` (both without a name), or nil and a message.
local function parts_of(args)
  local name = args[1]
  if name == nil or name == "" then
    return { "chunk", "line_num" }
  end
  name = name:gsub("-", "_")
  if not PARTS[name] then
    return nil, "pile: unknown part " .. args[1] .. " (chunk or line_num)"
  end
  return { name }
end

local function switch(args, value)
  local parts, problem = parts_of(args)
  if not parts then
    helix.ui.error(problem)
    return
  end
  local ok, buffer = pcall(helix.buffer.get_current)
  for _, part in ipairs(parts) do
    local new = value
    if new == nil then
      new = not on[part]
    end
    on[part] = new
    if not new and ok then
      if part == "chunk" then
        clear_chunk(buffer)
        pcall(buffer.clear_overlays, buffer)
      else
        pcall(buffer.clear_line_number_styles, buffer)
        drawn.line_num = nil
      end
    end
  end
  update("view")
end

if helix.has_permission("commands") then
  local args = "[chunk|line_num]"
  helix.register_command({
    name = "pile-enable",
    doc = "Draw the block around the cursor (chunk) and colour its line numbers (line_num), or the part named",
    args = args,
    handler = function(a) switch(a, true) end,
  })
  helix.register_command({
    name = "pile-disable",
    doc = "Stop drawing the block around the cursor and colouring its line numbers, or the part named",
    args = args,
    handler = function(a) switch(a, false) end,
  })
  helix.register_command({
    name = "pile-status",
    doc = "Show which parts are on, and the lines of the block around the cursor",
    handler = function()
      local message = status()
      local ok, buffer = pcall(helix.buffer.get_current)
      if ok then
        local block = find_block(buffer, config.chunk)
        message = message .. (block and ("; the block: lines " .. block.first .. " to "
          .. block.last) or "; no block around the cursor")
      end
      helix.ui.notify(message)
    end,
  })
  helix.register_command({
    name = "pile-toggle",
    doc = "Switch the block around the cursor and its line numbers on or off, or the part named",
    args = args,
    handler = function(a) switch(a, nil) end,
  })
end

-- The textobject: a key selecting the lines of the block.
if config.chunk.textobject ~= "" then
  if helix.has_permission("keymaps") and helix.has_permission("edit") then
    local function select_block()
      local buffer = helix.buffer.get_current()
      local block = find_block(buffer, config.chunk)
      if not block then
        return
      end
      local from = buffer:line_to_char(block.first - 1)
      local to = buffer:line_to_char(math.min(block.last, buffer:line_count()))
      helix.editor.set_selections({ { anchor = from, head = to } })
    end
    helix.keymap.set({ "normal", "select" }, config.chunk.textobject, select_block,
      { desc = "Select the block around the cursor" })
  else
    table.insert(problems, "chunk.textobject needs the \"keymaps\" and \"edit\" permissions")
  end
end

if #problems > 0 then
  helix.ui.warn("pile: " .. table.concat(problems, "; "))
end
