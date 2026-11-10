-- Panel-style diagnostics with rounded caps and colored bullets, modelled after
-- tiny-inline-diagnostic.nvim: the diagnostics of the cursor line are shown in a panel
-- after the end of the line, drawn over the text (it never moves the lines below or
-- scrolls the view). Long messages wrap, and further diagnostics follow, over the lines
-- below; with too few columns left beside the line, the panel goes under it. Hidden in
-- insert mode by default.

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

local function update_diagnostics()
  local buffer = helix.buffer.get_current()
  if not buffer then return end

  local cursor = buffer:get_cursor()
  local current_line_idx = buffer:char_to_line(cursor)

  -- Collect diagnostics on current line
  local line_diags = {}
  for _, diag in ipairs(buffer:get_diagnostics()) do
    if diag.line == current_line_idx then
      table.insert(line_diags, diag)
    end
  end

  if #line_diags == 0 then
    buffer:set_panel(nil)
    return
  end

  -- Most severe first
  table.sort(line_diags, function(a, b)
    local prio_a = severity_priority[a.severity] or 99
    local prio_b = severity_priority[b.severity] or 99
    return prio_a < prio_b
  end)

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
