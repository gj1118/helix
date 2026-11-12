-- Panel-style diagnostics with rounded caps and colored bullets, modelled after
-- tiny-inline-diagnostic.nvim: the diagnostics of the cursor line are shown in a panel
-- after the end of the line, drawn over the text (it never moves the lines below or
-- scrolls the view). Long messages wrap, and further diagnostics follow, over the lines
-- below; with too few columns left beside the line, the panel goes under it. Hidden in
-- insert mode and while a prompt is open (a search, the command line) by default.
--
-- With `other_lines`, the other lines in view show their most severe diagnostic after
-- their end too, in one line (tiny-inline-diagnostic's `multilines`); on a line too long
-- for the window it goes against the window's right edge instead of out of view.

local config = {
  -- Panel colors
  panel_bg = "#cba6f7",
  panel_fg = "#1e1e2e",

  -- Severity colors (for bullets)
  error_color = "#ff6b6b",
  warning_color = "#ffd93d",
  info_color = "#6bcb77",
  hint_color = "#4d96ff",

  -- Arrow pointing from the line to the panel beside it
  arrow = "←",

  -- Arrow pointing from the panel under the line to it
  arrow_dropped = "╰",

  -- Bullet character
  bullet = "●",

  -- Powerline rounded caps
  left_cap = utf8.char(0xE0B6),    --
  right_cap = utf8.char(0xE0B4),   --

  -- Diagnostics shown before "+N more"
  max_lines = 4,

  -- Fewer columns than this left for the messages beside the line put the panel under it
  -- (tiny-inline-diagnostic's `softwrap`)
  min_width = 30,

  -- Show the panel in insert mode too (tiny-inline-diagnostic's `enable_on_insert`)
  show_in_insert = false,

  -- Show the panel while a prompt is open too (a search moves the cursor as it is typed)
  show_in_prompt = false,

  -- The other lines in view: their most severe diagnostic after their end, in the color of
  -- its severity (tiny-inline-diagnostic's `multilines`)
  other_lines = false,

  -- Columns of a message on another line, at most (it is cut with "…")
  other_lines_width = 60,

  -- Where it goes: "eol_or_right" (after the end of the line, or at the window's right edge
  -- when the line is too long), "right" (always at the right edge) or "eol"
  other_lines_align = "eol_or_right",
}

-- Load user config
local user_config = helix.get_config()

if user_config then
  for k, v in pairs(user_config) do
    config[k] = v
  end
end

local severity_priority = {
  error = 1,
  warning = 2,
  info = 3,
  hint = 4
}

local function get_bullet_color(severity)
  if severity == "error" then
    return config.error_color
  elseif severity == "warning" then
    return config.warning_color
  elseif severity == "info" then
    return config.info_color
  elseif severity == "hint" then
    return config.hint_color
  end
  return config.info_color
end

local in_prompt = false     -- a prompt is open
local annotated = nil       -- the buffer whose other lines have diagnostics drawn

-- The first line of `message`, cut to `width` columns.
local function one_line(message, width)
  local line = message:match("^[^\n]*") or ""
  if utf8.len(line) and utf8.len(line) > width then
    line = line:sub(1, utf8.offset(line, width)  - 1) .. "…"
  end
  return line
end

-- The most severe diagnostic of each line in view but the cursor's, after its end.
local function update_other_lines(buffer, by_line, cursor_line)
  if annotated and annotated ~= buffer:id() then
    local previous = helix.buffer.get_by_id(annotated)
    if previous then
      previous:clear_annotations()
    end
    annotated = nil
  end
  if not config.other_lines then
    return
  end
  local window = helix.window.get_current()
  if not window then
    return
  end
  local top = window:get_top_line()
  local bottom = top + window:get_area().height
  local annotations = {}
  for line, diags in pairs(by_line) do
    if line ~= cursor_line and line >= top and line < bottom then
      local diag = diags[1]
      local text = config.bullet .. " " .. one_line(diag.message, config.other_lines_width)
      if #diags > 1 then
        text = text .. " (+" .. (#diags - 1) .. ")"
      end
      annotations[#annotations + 1] = {
        char_idx = buffer:line_to_char(line),
        text = text,
        fg = get_bullet_color(diag.severity),
        align = config.other_lines_align,
      }
    end
  end
  buffer:set_annotations(annotations)
  annotated = buffer:id()
end

local function update_diagnostics()
  local buffer = helix.buffer.get_current()
  if not buffer then return end

  local cursor = buffer:get_cursor()
  local current_line_idx = buffer:char_to_line(cursor)

  -- The diagnostics by line, most severe first
  local by_line = {}
  for _, diag in ipairs(buffer:get_diagnostics()) do
    by_line[diag.line] = by_line[diag.line] or {}
    table.insert(by_line[diag.line], diag)
  end
  for _, diags in pairs(by_line) do
    table.sort(diags, function(a, b)
      local prio_a = severity_priority[a.severity] or 99
      local prio_b = severity_priority[b.severity] or 99
      return prio_a < prio_b
    end)
  end

  update_other_lines(buffer, by_line, current_line_idx)

  local line_diags = by_line[current_line_idx] or {}
  if #line_diags == 0 or (in_prompt and not config.show_in_prompt) then
    buffer:set_panel(nil)
    return
  end

  local items = {}
  for i = 1, math.min(#line_diags, config.max_lines) do
    local diag = line_diags[i]
    table.insert(items, {
      icon = config.bullet,
      icon_fg = get_bullet_color(diag.severity),
      text = diag.message,
    })
  end

  local footer = nil
  if #line_diags > config.max_lines then
    footer = "... (+" .. (#line_diags - config.max_lines) .. " more)"
  end

  -- Helix lays the panel out (beside or under the line, wrapped to the view) on every frame.
  buffer:set_panel({
    char_idx = line_diags[1].range.start,
    items = items,
    footer = footer,
    fg = config.panel_fg,
    bg = config.panel_bg,
    arrow = " " .. config.arrow .. " ",
    under_arrow = config.arrow_dropped .. " ",
    arrow_fg = config.panel_bg,
    left_cap = config.left_cap,
    right_cap = config.right_cap,
    min_width = config.min_width,
    show_in_insert = config.show_in_insert,
  })
end

helix.on("selection_change", function(event)
  update_diagnostics()
end)

helix.on("lsp_diagnostic", function(event)
  update_diagnostics()
end)

-- The panel stays where it was put: follow edits (`dd`, say) until the new diagnostics come.
helix.on("buffer_changed", function(event)
  update_diagnostics()
end)

-- The other lines in view change with scrolling and the focused window.
helix.on("scroll", function(event)
  if config.other_lines then
    update_diagnostics()
  end
end)

helix.on("window_focus", function(event)
  update_diagnostics()
end)

-- No panel while a prompt is open (unless `show_in_prompt`): a search moves the cursor
-- from match to match as it is typed.
helix.on("prompt_open", function(event)
  in_prompt = true
  update_diagnostics()
end)

helix.on("prompt_close", function(event)
  in_prompt = false
  update_diagnostics()
end)
