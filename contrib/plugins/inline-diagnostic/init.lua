-- Clean panel-style diagnostics inspired by tiny-inline-diagnostic.nvim
-- First line inline, subsequent lines as virtual lines below

local config = {
    -- Softer, more muted colors like the Neovim plugin
    panel_bg = "#2d4f5e",    -- Muted teal background
    panel_fg = "#c0ccd4",    -- Soft gray text
    
    -- Severity indicator colors (dots)
    error_color = "#ff6b6b",
    warning_color = "#ffd93d", 
    info_color = "#6bcb77",
    hint_color = "#4d96ff",
    
    -- Arrow pointing to the panel
    arrow = "←",
    
    -- Bullet character for each diagnostic
    bullet = "●",
    
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
    
    -- Calculate panel width based on longest message
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
    
    local panel_width = max_msg_len + 2  -- Add padding
    
    -- Calculate alignment offset for virtual lines
    local current_line_text = buffer:get_text():sub(
        buffer:line_to_char(current_line_idx),
        buffer:line_to_char(current_line_idx + 1) - 2
    )
    local line_visual_width = calculate_visual_width(current_line_text, tab_width)
    if current_line_text == "" then line_visual_width = 0 end
    
    -- The offset where the panel starts (after code + arrow + gap)
    local panel_start_offset = line_visual_width + 6
    
    local annotations = {}
    local char_idx = line_diags[1].range.start
    
    -- Helper to pad message to panel width
    local function pad_message(msg)
        local display = config.bullet .. " " .. msg
        local padding = panel_width - (utf8.len(display) or #display)
        if padding > 0 then
            display = display .. string.rep(" ", padding)
        end
        return " " .. display .. " "
    end
    
    -- 1. FIRST LINE: Arrow + First message (INLINE - same line as code)
    local first_diag = show_diags[1]
    local first_content = " " .. config.arrow .. " " .. pad_message(first_diag.message)
    
    table.insert(annotations, helix.buffer.annotation({
        char_idx = char_idx,
        text = first_content,
        fg = config.panel_fg,
        bg = config.panel_bg,
        offset = 1,
        is_line = false  -- INLINE!
    }))
    
    -- 2. SUBSEQUENT LINES: Remaining messages as virtual lines
    for i = 2, #show_diags do
        local diag = show_diags[i]
        local content = pad_message(diag.message)
        
        table.insert(annotations, helix.buffer.annotation({
            char_idx = char_idx,
            text = content,
            fg = config.panel_fg,
            bg = config.panel_bg,
            offset = panel_start_offset,
            is_line = true  -- Virtual line below
        }))
    end
    
    -- 3. Truncation message if needed (as virtual line)
    if hidden_count > 0 then
        local trunc_msg = "... (+" .. hidden_count .. " more)"
        local padding = panel_width - (utf8.len(trunc_msg) or #trunc_msg)
        if padding > 0 then
            trunc_msg = trunc_msg .. string.rep(" ", padding)
        end
        
        table.insert(annotations, helix.buffer.annotation({
            char_idx = char_idx,
            text = " " .. trunc_msg .. " ",
            fg = config.panel_fg,
            bg = config.panel_bg,
            offset = panel_start_offset,
            is_line = true
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

helix.log.info("[inline-diagnostic] Panel style v2 enabled")
