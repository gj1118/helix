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
-- { "bold" } }), a colour ("#806d9c", "light-blue") or a theme key ("comment"). Without one,
-- the theme's `pile.chunk`, `pile.chunk.error` and `pile.line_num` are used when it sets them
-- (simplelight and simpledark do), else hlchunk's colours (the defaults below).
--
-- The line's style table can also have `line_color` (or `lineColor`; over `fg`) and
-- `thickness`, 1 to 5 like the gradient borders': thin and rounded (1, `─ │ ╭ ╰`), heavy
-- (2, `━ ┃ ┏ ┗`), double (3, `═ ║ ╔ ╚`), half and full blocks (4), full blocks (5):
--
--   chunk = { enable = true, gradient = false, style = { line_color = "#ffc900", thickness = 3 } }
--
-- With `gradient = true` the line takes the gradient borders' colours and `line_color` is
-- left aside (the thickness stays). `chars` set some characters over the thickness's.
--
--   chunk = {
--     enable = true,
--     style = { fg = "#806d9c" },        -- the line (or a list of two styles, like hlchunk's:
--                                        --   the line, then the line of a block with an error),
--                                        --   with line_color and thickness (1, see above)
--     error_style = { fg = "#c21f30" },  -- the line beside a block with an error (its own
--                                        --   thickness, or style's)
--     error_sign = true,                 -- error_style when the block has a syntax error
--     use_treesitter = true,             -- the block from the syntax tree; false (or no syntax
--                                        --   tree): between the braces around the cursor
--     chars = { horizontal_line = "─", vertical_line = "│", left_top = "╭",
--               left_bottom = "╰", right_arrow = ">" },   -- those of thickness 1
--     delay = 300,                       -- ms the cursor rests before another block is drawn
--                                        --   (moved to by a key, a click or a scroll), with an
--                                        --   animation; 0: at once, not animated (in another
--                                        --   window or buffer, or after an edit: at once)
--     duration = 200,                    -- ms the animation takes; 0: not animated
--     gradient = false,                  -- the line in the colours of [editor.gradient-borders]
--                                        --   (start-color, middle-color, end-color): from the
--                                        --   first line's code, down, to the arrow; a block with
--                                        --   an error keeps error_style
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

-- The colours without a style of the user's or the theme's (hlchunk's).
local VIOLET, RED = { fg = "#806d9c" }, { fg = "#c21f30" }

-- The settings that are styles: the user's, else the theme's key, else hlchunk's colour (see
-- `resolve_styles`); not in DEFAULTS, to tell them apart.
local STYLES = { style = true, error_style = true }

local DEFAULTS = {
  chunk = {
    enable = true,
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
    gradient = false,
    max_file_size = 1024 * 1024,
    exclude_languages = {},
    textobject = "",
    node_types = NODE_TYPES,
  },
  line_num = {
    enable = false,
    use_treesitter = true,
    max_file_size = 1024 * 1024,
    exclude_languages = {},
  },
}

-- The characters of each thickness of the line, those of the gradient borders' thicknesses:
-- the first line's horizontal line, the last line's, the vertical line and the corners (the
-- arrow is `right_arrow` in all of them).
local THICKNESS = {
  { horizontal_line = "─", bottom_line = "─", vertical_line = "│", left_top = "╭", left_bottom = "╰" },
  { horizontal_line = "━", bottom_line = "━", vertical_line = "┃", left_top = "┏", left_bottom = "┗" },
  { horizontal_line = "═", bottom_line = "═", vertical_line = "║", left_top = "╔", left_bottom = "╚" },
  { horizontal_line = "▄", bottom_line = "▀", vertical_line = "█", left_top = "█", left_bottom = "█" },
  { horizontal_line = "█", bottom_line = "█", vertical_line = "█", left_top = "█", left_bottom = "█" },
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
    if STYLES[key] and (name == "chunk" or key == "style") then
      if type(value) ~= "table" and type(value) ~= "string" then
        table.insert(problems, name .. "." .. key .. " should be a style: a table, a colour or a theme key")
      else
        merged[key] = value
      end
    elseif default == nil then
      table.insert(problems, "unknown setting " .. name .. "." .. tostring(key))
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

-- The characters the user set (`chunk.chars`): over those of the line's thickness.
local given_chars = {}
if type(given.chunk) == "table" and type(given.chunk.chars) == "table" then
  for key, value in pairs(given.chunk.chars) do
    if DEFAULTS.chunk.chars[key] ~= nil and type(value) == "string" then
      given_chars[key] = value
    end
  end
end

-- The characters of a line `thickness` thick (1 to 5), with the user's over them.
local function line_chars(thickness)
  local set = THICKNESS[thickness] or THICKNESS[1]
  local chars = { right_arrow = DEFAULTS.chunk.chars.right_arrow }
  for key, value in pairs(set) do
    chars[key] = value
  end
  for key, value in pairs(given_chars) do
    chars[key] = value
    if key == "horizontal_line" then
      chars.bottom_line = value
    end
  end
  return chars
end

-- The colour names of themes.
local COLOUR_NAMES = {}
for _, name in ipairs({ "default", "black", "red", "green", "yellow", "blue", "magenta", "cyan",
  "gray", "light-red", "light-green", "light-yellow", "light-blue", "light-magenta",
  "light-cyan", "light-gray", "white" }) do
  COLOUR_NAMES[name] = true
end

-- Whether `value` is a colour written like in a theme: "#rrggbb", a name ("light-blue") or a
-- terminal palette index ("8").
local function is_colour(value)
  if type(value) ~= "string" then
    return false
  end
  if value:match("^#%x%x%x%x%x%x$") or COLOUR_NAMES[value] then
    return true
  end
  local index = math.tointeger(tonumber(value))
  return index ~= nil and index >= 0 and index <= 255
end

-- A style of the settings (a table, a colour or a theme key) as the fields of an overlay's
-- style, and the line's thickness when it sets one. `name` names it in the problems.
local function style_fields(style, name)
  if type(style) == "string" then
    if is_colour(style) then
      return { fg = style }
    end
    return { style = style }
  elseif type(style) ~= "table" then
    return {}
  end
  local fields, thickness = {}, nil
  for key, value in pairs(style) do
    if key == "thickness" then
      if math.type(value) == "integer" and value >= 1 and value <= 5 then
        thickness = value
      else
        table.insert(problems, name .. ".thickness should be 1 to 5")
      end
    elseif key ~= "line_color" and key ~= "lineColor" then
      fields[key] = value
    end
  end
  local line_color = style.line_color
  if line_color == nil then
    line_color = style.lineColor
  end
  if line_color ~= nil then
    fields.fg = line_color
  end
  for _, key in ipairs({ "fg", "bg" }) do
    if fields[key] ~= nil and not is_colour(fields[key]) then
      table.insert(problems, name .. (key == "fg" and line_color ~= nil and ".line_color" or "." .. key)
        .. " should be a colour (\"#ffc900\", \"light-blue\", ...)")
      fields[key] = nil
    end
  end
  return fields, thickness
end

-- The style of the theme's `key` when the theme sets it, else `fallback`.
local function themed(key, fallback)
  local ok, style = pcall(helix.ui.get_style, key)
  if ok and style then
    -- the key itself: drawn with the theme's style as it is
    return { style = key }
  end
  return fallback
end

-- The styles of the settings, read once: the line's, the line's beside a block with an error,
-- the line numbers'; and the line's thickness (beside a block with an error too, unless
-- error_style has its own).
local mine = {}
do
  local style, error_style = config.chunk.style, config.chunk.error_style
  local style_name, error_name = "chunk.style", "chunk.error_style"
  if type(style) == "table" and style[1] ~= nil then
    -- hlchunk's list: the line, then the line of a block with an error
    style, error_style = style[1], style[2] or error_style
    style_name, error_name = "chunk.style[1]", "chunk.style[2]"
  end
  if style ~= nil then
    mine.chunk, mine.thickness = style_fields(style, style_name)
  end
  if error_style ~= nil then
    mine.chunk_error, mine.error_thickness = style_fields(error_style, error_name)
  end
  if config.line_num.style ~= nil then
    mine.line_num = style_fields(config.line_num.style, "line_num.style")
  end
end

local chunk_style, chunk_error_style, line_num_style

-- The styles of the parts: the user's, else the theme's, else hlchunk's. Again when the theme
-- changes.
-- `own` (a style of the settings) over `fallback`: in place of it when it has a colour or a
-- theme key, else added to it (`{ thickness = 3 }` or `{ modifiers = { "bold" } }` keep the
-- theme's colour, or hlchunk's).
local function over(fallback, own)
  if own == nil then
    return fallback
  end
  if own.fg ~= nil or own.style ~= nil then
    return own
  end
  local merged = {}
  for key, value in pairs(fallback) do
    merged[key] = value
  end
  for key, value in pairs(own) do
    merged[key] = value
  end
  return merged
end

local function resolve_styles()
  chunk_style = over(themed("pile.chunk", VIOLET), mine.chunk)
  chunk_error_style = over(themed("pile.chunk.error", RED), mine.chunk_error)
  line_num_style = over(themed("pile.line_num", VIOLET), mine.line_num)
end
resolve_styles()

-- The characters of the line beside a block (`faulty`: with a syntax error).
local function chars_of(faulty)
  local thickness = mine.thickness or 1
  if faulty and mine.error_thickness then
    thickness = mine.error_thickness
  end
  return line_chars(thickness)
end

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

-- What is drawn, to leave it be while it stays the same (most keys change nothing: the
-- cursor stays in the block, the view stays within the lines drawn, an edit leaves the cells
-- as they were).
local drawn = {
  block = nil,     -- the chunk's block: the key of its buffer, window, lines and error
  from = nil,      -- the lines drawn, around the lines shown
  to = nil,
  cells = nil,     -- what the cells sent were (their signature)
  stale = true,    -- after an edit, a theme or settings change: worked out again
  cleared = nil,   -- the buffer and window the chunk was cleared in, nothing drawn since
  here = nil,      -- the buffer and window of the chunk drawn or cleared last
  line_num = nil,  -- the key of the line numbers' block
  numbers_cleared = nil,
}
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

-- Lines looked at each way for the braces around the cursor, and read at a time: few at
-- first (most blocks are short), twice as many each time after, up to `BATCH_MAX`.
local SCAN = 2000
local BATCH = 64
local BATCH_MAX = 512

-- In `text` from byte `init`: the first `stop` brace not inside a block of `go` and `stop`
-- braces, which Lua's `%b` skips (the scan is in C, not char by char in Lua); or nil and
-- the byte to go on from once more text is read (a block going on past `text`, or its end).
local function unmatched(text, init, go, stop)
  local block = "^%b" .. go .. stop
  while true do
    local pos = text:find("[{}]", init)
    if not pos then
      return nil, #text + 1
    end
    if text:sub(pos, pos) == stop then
      return pos
    end
    local _, last = text:find(block, pos)
    if not last then
      return nil, pos
    end
    init = last + 1
  end
end

-- The line (1-based) of the brace closing the block (`step` 1) or opening it (`step` -1),
-- going from char `col` of `line`: or nil within `SCAN` lines. Backwards, the text read is
-- reversed, its `}` opening the blocks skipped.
local function brace_line(buffer, line, col, step)
  local back = step < 0
  local limit = back and math.max(1, line - SCAN) or math.min(buffer:line_count(), line + SCAN)
  local whole = math.maxinteger -- (a column: to the line's end)
  -- the text read, in order, and each part's text, first char and offset in it
  local read, parts = "", {}
  local init, next_line, size = 1, line, BATCH
  while (back and next_line >= limit) or (not back and next_line <= limit) do
    local far = back and math.max(limit, next_line - size + 1)
      or math.min(limit, next_line + size - 1)
    local text, start
    if next_line == line and back then
      text, start = buffer:get_range(far, 0, line, col), buffer:line_to_char(far - 1)
    elseif next_line == line then
      text, start = buffer:get_range(line, col, far, whole), buffer:line_to_char(line - 1) + col
    else
      local first = math.min(next_line, far)
      text, start = buffer:get_range(first, 0, math.max(next_line, far), whole),
        buffer:line_to_char(first - 1)
    end
    -- (lines are joined by "\n": their own line endings matter for no brace)
    local offset = #read > 0 and #read + 1 or 0
    parts[#parts + 1] = { text = text, start = start, offset = offset }
    read = (#read > 0 and read .. "\n" or "") .. (back and text:reverse() or text)
    local pos
    pos, init = unmatched(read, init, back and "}" or "{", back and "{" or "}")
    if pos then
      for i = #parts, 1, -1 do
        local part = parts[i]
        if pos > part.offset then
          local byte = pos - part.offset
          if back then
            byte = #part.text - byte + 1
          end
          return buffer:char_to_line(part.start + utf8.len(part.text, 1, byte) - 1) + 1
        end
      end
    end
    next_line, size = far + step, math.min(size * 2, BATCH_MAX)
  end
  return nil
end

-- The block between the braces around char `cursor`: { first, last } or nil.
local function block_by_braces(buffer, cursor)
  local line = buffer:char_to_line(cursor) + 1
  local col = cursor - buffer:line_to_char(line - 1)
  -- from the cursor, after it when it is on a `{`
  if buffer:get_range(line, col, line, col + 1) == "{" then
    col = col + 1
  end
  local open = brace_line(buffer, line, col, -1)
  if not open then
    return nil
  end
  local close = brace_line(buffer, line, col, 1)
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

-- The widest indentation the cursor is taken out of (to the line's code).
local INDENT_MAX = 256

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
  -- in the indentation of a line, the block is that of the line's code (read near the
  -- cursor only: a line may be a whole minified file)
  local line = buffer:char_to_line(cursor) + 1
  local col = cursor - buffer:line_to_char(line - 1)
  if col <= INDENT_MAX and not buffer:get_range(line, 0, line, col):find("[^ \t]") then
    local code = buffer:get_range(line, col, line, col + INDENT_MAX):find("[^ \t]")
    if code then
      cursor = cursor + code - 1
    end
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

-- The cells of the chunk of `block` on lines `first_shown` to `last_shown`, in the order
-- the animation draws them: from the first line's code leftwards to the corner, down, then
-- right to the arrow before the last line's code. Each has its `place` along the whole line,
-- 0 to `length` - 1 (the second value), shown or not, for the gradient.
local function chunk_cells(buffer, block, first_shown, last_shown, chars)
  local info = buffer:get_info()
  local tab_width = math.max(info.tab_width, 1)
  local shift = info.indent_style == "tabs" and tab_width or math.max(info.indent_width, 1)
  -- (the indentation of each line, worked out by the editor; `false` for a blank one)
  local first_indent = buffer:get_indents(block.first, block.first)[1] or 0
  local last_indent = buffer:get_indents(block.last, block.last)[1] or 0
  local col = math.max(math.min(first_indent, last_indent) - shift, 0)

  -- the lengths of the three parts of the line: along the first line, down, along the last
  local top = math.max(first_indent - col, 0)
  local down = block.last - block.first - 1
  local length = top + down + math.max(last_indent - col, 0)

  local cells = {}
  -- `rows`: on which rows of a soft-wrapped line ("first" by default, "wrapped", "all")
  local function add(line, at, place, text, rows)
    if line >= first_shown and line <= last_shown then
      table.insert(cells, { line = line, col = at, place = place, text = text, rows = rows })
    end
  end
  for at = first_indent - 1, col, -1 do
    add(block.first, at, first_indent - 1 - at, at == col and chars.left_top
      or chars.horizontal_line)
  end
  -- down the rows the first line is wrapped onto, then beside every row of the lines between
  add(block.first, col, math.max(top - 1, 0), chars.vertical_line, "wrapped")
  local from = math.max(block.first + 1, first_shown)
  local to = math.min(block.last - 1, last_shown)
  if from <= to then
    for i, indent in ipairs(buffer:get_indents(from, to)) do
      local line = from + i - 1
      if not indent or indent > col then
        add(line, col, top + line - block.first - 1, chars.vertical_line, "all")
      end
    end
  end
  for at = col, last_indent - 1 do
    local text = chars.bottom_line
    if at == col then
      text = chars.left_bottom
    elseif at == last_indent - 1 then
      text = chars.right_arrow
    end
    add(block.last, at, top + down + at - col, text)
  end
  return cells, length
end

-- The gradient's colours: those of `[editor.gradient-borders]` (its defaults without them), as
-- The gradient's stops and the colours of the places along a line of the length worked out
-- last (most redraws are of the same block): worked out again when the settings change.
local gradient = { stops = nil, length = nil, colours = {} }

-- { r, g, b } stops: start, the middle one when set, end.
local function gradient_stops()
  if gradient.stops then
    return gradient.stops
  end
  local settings = helix.editor.get_option("gradient-borders") or {}
  local function rgb(hex, default)
    hex = type(hex) == "string" and hex:match("^#(%x%x%x%x%x%x)$") or default
    return {
      tonumber(hex:sub(1, 2), 16),
      tonumber(hex:sub(3, 4), 16),
      tonumber(hex:sub(5, 6), 16),
    }
  end
  local stops = { rgb(settings["start-color"], "8A2BE2") }
  if type(settings["middle-color"]) == "string" and settings["middle-color"]:match("^#%x%x%x%x%x%x$") then
    table.insert(stops, rgb(settings["middle-color"]))
  end
  table.insert(stops, rgb(settings["end-color"], "00BFFF"))
  gradient.stops = stops
  return stops
end

-- The colour `share` (0 to 1) of the way through `stops`, like the gradient borders mix them.
local function mix(stops, share)
  share = math.min(math.max(share, 0), 1)
  local from, to = stops[1], stops[#stops]
  if #stops == 3 then
    if share < 0.5 then
      to, share = stops[2], share * 2
    else
      from, share = stops[2], (share - 0.5) * 2
    end
  end
  local function channel(i)
    return math.floor(from[i] + (to[i] - from[i]) * share)
  end
  return string.format("#%02x%02x%02x", channel(1), channel(2), channel(3))
end

-- Gives each cell its colour of the gradient, by its place along the line.
local function colour_cells(cells, length)
  local stops = gradient_stops()
  if gradient.length ~= length then
    gradient.length, gradient.colours = length, {}
  end
  local colours = gradient.colours
  for _, cell in ipairs(cells) do
    local colour = colours[cell.place]
    if not colour then
      colour = mix(stops, length > 1 and cell.place / (length - 1) or 0)
      colours[cell.place] = colour
    end
    cell.fg = colour
  end
end

-- The overlays of `cells` in `style`.
local function overlays_of(cells, style)
  local keys, values = {}, {}
  for key, value in pairs(style) do
    keys[#keys + 1], values[#values + 1] = key, value
  end
  local list = {}
  for i, cell in ipairs(cells) do
    local item = { line = cell.line, col = cell.col, text = cell.text, rows = cell.rows }
    for k = 1, #keys do
      item[keys[k]] = values[k]
    end
    -- its colour of the gradient, over the style's
    if cell.fg then
      item.fg = cell.fg
    end
    list[i] = item
  end
  return list
end

local function show(buffer, cells, style)
  buffer:set_overlays(overlays_of(cells, style))
end

-- Draws the cells one after the other over `duration` ms (by the clock: firings of the
-- timer skipped while the editor is busy do not slow it down).
local function animate(buffer, cells, style)
  animation = cancel(animation)
  local started = helix.now()
  local all, shown = overlays_of(cells, style), {}
  buffer:set_overlays({})
  animation = helix.every(16, function()
    local share = math.min(1, (helix.now() - started) / config.chunk.duration)
    local count = math.max(1, math.ceil(#all * share))
    for i = #shown + 1, count do
      shown[i] = all[i]
    end
    -- the buffer may no longer be focused: the animation ends
    if not pcall(buffer.set_overlays, buffer, shown) or count >= #all then
      animation = cancel(animation)
    end
  end)
end

-- The lines worth drawing in `window`: those shown, and a screen more each way for a
-- smooth scroll on its way.
local function shown_lines(window)
  local top = window:get_top_line() + 1
  local height = window:get_area().height
  return math.max(1, top - height), top + 2 * height + 20, top, top + height
end

-- The buffer and window, for the keys of what is drawn.
local function place(buffer, window)
  return buffer:id() .. ":" .. window:id()
end

-- No chunk in `window` (once: not again while nothing is drawn there).
local function clear_chunk(buffer, window)
  animation = cancel(animation)
  waiting = cancel(waiting)
  local here = place(buffer, window)
  if drawn.cleared ~= here then
    pcall(buffer.set_overlays, buffer, {})
    drawn.cleared = here
  end
  drawn.block, drawn.cells, drawn.here = nil, nil, here
end

-- What the cells sent to a window are, to tell whether they changed.
local function signature(here, cells, faulty)
  local parts, n = { here, tostring(faulty) }, 2
  for _, cell in ipairs(cells) do
    parts[n + 1], parts[n + 2], parts[n + 3] = cell.line, cell.col, cell.text
    parts[n + 4], parts[n + 5] = cell.rows or "", cell.fg or ""
    n = n + 5
  end
  return table.concat(parts, ",")
end

-- `how`: "edit" (the text changed: drawn at once), else "move" or "view". Another block of
-- the same window and buffer is the cursor's having moved (with a key, a click, or a scroll
-- taking it along): it waits for the cursor to rest, and is animated. In another window or
-- buffer, or at the start, the block is drawn at once.
local function update_chunk(buffer, window, how)
  if not on.chunk then
    return
  end
  local block = find_block(buffer, config.chunk)
  if not block then
    clear_chunk(buffer, window)
    return
  end
  local faulty = block.error and config.chunk.error_sign
  local first_shown, last_shown, top, bottom = shown_lines(window)
  local block_key = table.concat({ place(buffer, window), block.first, block.last,
    tostring(faulty) }, ":")
  if block_key == drawn.block and not drawn.stale and top >= drawn.from and bottom <= drawn.to then
    -- the block drawn, and the view within the lines drawn: nothing to do (a block waiting
    -- to be drawn is not, the cursor came back)
    waiting = cancel(waiting)
    return
  end
  local here = place(buffer, window)
  local animated = how ~= "edit" and drawn.here == here and block_key ~= drawn.block
    and config.chunk.delay > 0 and config.chunk.duration > 0
  local function draw()
    waiting = cancel(waiting)
    -- (where the window shows now: it may have scrolled while the block waited)
    first_shown, last_shown = shown_lines(window)
    local cells, length = chunk_cells(buffer, block, first_shown, last_shown, chars_of(faulty))
    local style = faulty and chunk_error_style or chunk_style
    if config.chunk.gradient and not faulty then
      colour_cells(cells, length)
    end
    local cells_now = signature(here, cells, faulty)
    if animated then
      animate(buffer, cells, style)
    elseif cells_now ~= drawn.cells or animation then
      -- (the same cells as drawn after most edits and scrolls: not sent again)
      animation = cancel(animation)
      show(buffer, cells, style)
    end
    drawn.block, drawn.from, drawn.to = block_key, first_shown, last_shown
    drawn.cells, drawn.stale, drawn.cleared, drawn.here = cells_now, false, nil, here
  end
  if animated then
    -- after the cursor has rested `delay` ms (still on this block)
    waiting = cancel(waiting)
    waiting = helix.defer(config.chunk.delay, function()
      waiting = nil
      local ok, current = pcall(helix.buffer.get_current)
      local ok_window, focused = pcall(helix.window.get_current)
      if ok and ok_window and current and focused and place(current, focused) == here then
        local now = find_block(buffer, config.chunk)
        if now and now.first == block.first and now.last == block.last
          and now.error == block.error then
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
    local here = place(buffer, window)
    if drawn.numbers_cleared ~= here then
      pcall(buffer.set_line_number_styles, buffer, {})
      drawn.numbers_cleared = here
    end
    drawn.line_num = nil
    return
  end
  local key = table.concat({ place(buffer, window), block.first, block.last }, ":")
  if key ~= drawn.line_num then
    buffer:set_line_number_styles({
      styled({ start_line = block.first, end_line = block.last }, line_num_style),
    })
    drawn.line_num, drawn.numbers_cleared = key, nil
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
  found = {}
  local ok_chunk, problem = pcall(update_chunk, buffer, window, how)
  local ok_num, problem_num = pcall(update_line_num, buffer, window)
  found = nil
  if not ok_chunk or not ok_num then
    error(problem or problem_num)
  end
end

helix.on("selection_change", function()
  update("move")
end)
-- Whether `text` has a line break.
local function breaks(text)
  return text:find("[\n\r\v\f]") or text:find("\u{85}", 1, true)
    or text:find("\u{2028}", 1, true) or text:find("\u{2029}", 1, true)
end

-- Whether `change` (within a line) is after the first char of its line that is not blank:
-- the line's indentation is as it was, and so are the cells of the chunk.
local function after_indentation(buffer, change)
  local code = buffer:get_line(change.start_line):find("[^ \t\r\n]")
  return code ~= nil and change.from - buffer:line_to_char(change.start_line - 1) >= code
end

helix.on("buffer_changed", function(event)
  -- The overlays and line number styles move with the lines they are on. The chunk's cells
  -- are those of the indentation of the block's lines: most edits (typing) are after it and
  -- change none. Others in a line: the cells are worked out again, and sent if they changed.
  -- One adding, removing or replacing line breaks may have moved them anywhere: sent again.
  local ok, buffer = pcall(helix.buffer.get_current)
  if ok and buffer and buffer:id() == event.document_id then
    if not event.changes then
      drawn.stale, drawn.cells, drawn.line_num = true, nil, nil
    end
    for _, change in ipairs(event.changes or {}) do
      if breaks(change.text) or breaks(change.removed) then
        drawn.stale, drawn.cells, drawn.line_num = true, nil, nil
        break
      elseif not drawn.stale and not after_indentation(buffer, change) then
        drawn.stale = true
      end
    end
  end
  update("edit")
end)
helix.on("theme_change", function()
  resolve_styles()
  drawn.stale, drawn.cells, drawn.line_num = true, nil, nil
  update("view")
end)
helix.on("config_change", function()
  -- the gradient's colours may be others (`:set gradient-borders.start-color ...`)
  gradient.stops, gradient.length = nil, nil
  if config.chunk.gradient then
    drawn.stale = true
  end
  update("view")
end)
for _, event in ipairs({ "scroll", "window_focus", "buffer_focus_lost", "buffer_open",
  "view_change" }) do
  helix.on(event, function()
    update("view")
  end)
end
helix.on("language_change", function()
  -- another syntax tree, not the cursor's having moved: its block is drawn at once
  drawn.here = nil
  update("view")
end)
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
        animation = cancel(animation)
        waiting = cancel(waiting)
        pcall(buffer.clear_overlays, buffer)
        drawn.block, drawn.cells, drawn.cleared, drawn.here = nil, nil, nil, nil
      else
        pcall(buffer.clear_line_number_styles, buffer)
        drawn.line_num, drawn.numbers_cleared = nil, nil
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
