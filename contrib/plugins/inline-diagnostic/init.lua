-- Clean panel-style diagnostics with rounded corners and colored bullets
-- Inspired by tiny-inline-diagnostic.nvim
-- Now with proper rounded corners on ALL lines!

local config = {
  -- Panel colors
  -- panel_bg = "#2d4f5e",    -- Muted teal background
  panel_bg = "#cba6f7",   -- Muted teal background

  -- panel_fg = "#c0ccd4",    -- Soft gray text
  panel_fg = "#1e1e2e",   -- Soft gray text


  -- Severity colors (for bullets)
  error_color = "#ff6b6b",
  warning_color = "#ffd93d",
  info_color = "#6bcb77",
  hint_color = "#4d96ff",

  -- Arrow pointing to the panel
  arrow = "← ",

  -- Bullet character
  bullet = "●",

  -- Powerline rounded caps
  left_cap = utf8.char(0xE0B6),    --
  right_cap = utf8.char(0xE0B4),   --

  -- Max lines before truncation
  max_lines = 4,
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

local function calculate_visual_width(text, tab_width)
  tab_width = tab_width or 4
  local expanded = text:gsub("\t", string.rep(" ", tab_width))
  return utf8.len(expanded) or #expanded
end

local function update_diagnostics()
  local buffer = helix.buffer.get_current()
  if not buffer then return end

  local cursor = buffer:get_cursor()
  local current_line_idx = buffer:char_to_line(cursor)
  local diagnostics = buffer:get_diagnostics()

  local tab_width = 4

  -- Collect diagnostics on current line
  local line_diags = {}
  for _, diag in ipairs(diagnostics) do
    if diag.line == current_line_idx then
      table.insert(line_diags, diag)
    end
  end

  if #line_diags == 0 then
    buffer:set_annotations({})
    return
  end

  -- Sort by severity
  table.sort(line_diags, function(a, b)
    local prio_a = severity_priority[a.severity] or 99
    local prio_b = severity_priority[b.severity] or 99
    return prio_a < prio_b
  end)

  -- Truncate if too many
  local show_diags = {}
  local hidden_count = 0
  if #line_diags > config.max_lines then
    hidden_count = #line_diags - config.max_lines
    for i = 1, config.max_lines do
      table.insert(show_diags, line_diags[i])
    end
  else
    show_diags = line_diags
  end

  -- Calculate panel content width (for padding)
  local max_msg_len = 0
  for _, diag in ipairs(show_diags) do
    local len = utf8.len(config.bullet .. " " .. diag.message) or #diag.message
    if len > max_msg_len then
      max_msg_len = len
    end
  end

  if hidden_count > 0 then
    local trunc_msg = "... (+" .. hidden_count .. " more)"
    local trunc_len = utf8.len(trunc_msg) or #trunc_msg
    if trunc_len > max_msg_len then
      max_msg_len = trunc_len
    end
  end

  local content_width = max_msg_len + 2

  -- Calculate alignment
  local current_line_text = buffer:get_text():sub(
    buffer:line_to_char(current_line_idx),
    buffer:line_to_char(current_line_idx + 1) - 2
  )
  local line_visual_width = calculate_visual_width(current_line_text, tab_width)
  if current_line_text == "" then line_visual_width = 0 end

  local annotations = {}
  local char_idx = line_diags[1].range.start

  -- ========================================
  -- FIRST LINE (INLINE - same row as code)
  -- ========================================
  local first_diag = show_diags[1]
  local first_bullet_color = get_bullet_color(first_diag.severity)

  local first_msg = first_diag.message
  local first_len = utf8.len(config.bullet .. " " .. first_msg) or #first_msg
  local first_padding = content_width - first_len
  if first_padding > 0 then
    first_msg = first_msg .. string.rep(" ", first_padding)
  end

  local offset = 1

  -- Arrow
  table.insert(annotations, helix.buffer.annotation({
    char_idx = char_idx,
    text = " " .. config.arrow .. " ",
    fg = config.panel_bg,
    offset = offset,
    is_line = false
  }))
  offset = offset + 3

  -- Left cap
  table.insert(annotations, helix.buffer.annotation({
    char_idx = char_idx,
    text = config.left_cap,
    fg = config.panel_bg,
    offset = offset,
    is_line = false
  }))
  offset = offset + 1

  -- Colored bullet
  table.insert(annotations, helix.buffer.annotation({
    char_idx = char_idx,
    text = " " .. config.bullet,
    fg = first_bullet_color,
    bg = config.panel_bg,
    offset = offset,
    is_line = false
  }))
  offset = offset + 2

  -- Message text
  table.insert(annotations, helix.buffer.annotation({
    char_idx = char_idx,
    text = " " .. first_msg .. " ",
    fg = config.panel_fg,
    bg = config.panel_bg,
    offset = offset,
    is_line = false
  }))
  offset = offset + utf8.len(" " .. first_msg .. " ")

  -- Right cap
  table.insert(annotations, helix.buffer.annotation({
    char_idx = char_idx,
    text = config.right_cap,
    fg = config.panel_bg,
    offset = offset,
    is_line = false
  }))

  -- ========================================
  -- SUBSEQUENT LINES (Virtual - multi-part with virt_line_idx!)
  -- ========================================
  local virt_line_base_offset = line_visual_width + 5

  for i = 2, #show_diags do
    local diag = show_diags[i]
    local bullet_color = get_bullet_color(diag.severity)
    local virt_idx = i - 2     -- 0-indexed virtual line row

    local msg = diag.message
    local msg_len = utf8.len(config.bullet .. " " .. msg) or #msg
    local msg_padding = content_width - msg_len
    if msg_padding > 0 then
      msg = msg .. string.rep(" ", msg_padding)
    end

    local v_offset = virt_line_base_offset

    -- Left cap (same virt_line_idx = same Y row)
    table.insert(annotations, helix.buffer.annotation({
      char_idx = char_idx,
      text = config.left_cap,
      fg = config.panel_bg,
      offset = v_offset,
      is_line = true,
      virt_line_idx = virt_idx
    }))
    v_offset = v_offset + 1

    -- Colored bullet
    table.insert(annotations, helix.buffer.annotation({
      char_idx = char_idx,
      text = " " .. config.bullet,
      fg = bullet_color,
      bg = config.panel_bg,
      offset = v_offset,
      is_line = true,
      virt_line_idx = virt_idx
    }))
    v_offset = v_offset + 2

    -- Message
    table.insert(annotations, helix.buffer.annotation({
      char_idx = char_idx,
      text = " " .. msg .. " ",
      fg = config.panel_fg,
      bg = config.panel_bg,
      offset = v_offset,
      is_line = true,
      virt_line_idx = virt_idx
    }))
    v_offset = v_offset + utf8.len(" " .. msg .. " ")

    -- Right cap
    table.insert(annotations, helix.buffer.annotation({
      char_idx = char_idx,
      text = config.right_cap,
      fg = config.panel_bg,
      offset = v_offset,
      is_line = true,
      virt_line_idx = virt_idx
    }))
  end

  -- ========================================
  -- TRUNCATION LINE (if needed)
  -- ========================================
  if hidden_count > 0 then
    local trunc_msg = "... (+" .. hidden_count .. " more)"
    local trunc_len = utf8.len(trunc_msg) or #trunc_msg
    local trunc_padding = content_width - trunc_len
    if trunc_padding > 0 then
      trunc_msg = trunc_msg .. string.rep(" ", trunc_padding)
    end

    local virt_idx = #show_diags - 1     -- After all diagnostic lines
    local v_offset = virt_line_base_offset

    -- Left cap
    table.insert(annotations, helix.buffer.annotation({
      char_idx = char_idx,
      text = config.left_cap,
      fg = config.panel_bg,
      offset = v_offset,
      is_line = true,
      virt_line_idx = virt_idx
    }))
    v_offset = v_offset + 1

    -- Truncation text
    table.insert(annotations, helix.buffer.annotation({
      char_idx = char_idx,
      text = " " .. trunc_msg .. " ",
      fg = config.panel_fg,
      bg = config.panel_bg,
      offset = v_offset,
      is_line = true,
      virt_line_idx = virt_idx
    }))
    v_offset = v_offset + utf8.len(" " .. trunc_msg .. " ")

    -- Right cap
    table.insert(annotations, helix.buffer.annotation({
      char_idx = char_idx,
      text = config.right_cap,
      fg = config.panel_bg,
      offset = v_offset,
      is_line = true,
      virt_line_idx = virt_idx
    }))
  end

  buffer:set_annotations(annotations)
end

helix.on("selection_change", function(event)
  update_diagnostics()
end)

helix.on("lsp_diagnostic", function(event)
  update_diagnostics()
end)

helix.log.info("[inline-diagnostic] Full rounded corners enabled with virt_line_idx!")
