-- Parser module for pretty-hover plugin
-- Transforms Doxygen-style documentation to Markdown

local M = {}

-- Check if a line matches any of the detection patterns
local function matches_pattern(line, patterns)
    for _, pattern in ipairs(patterns) do
        if line:match(pattern) then
            return true, pattern
        end
    end
    return false, nil
end

-- Apply styling to a line based on config
local function apply_style(text, styler)
    if type(styler) == "string" then
        -- Simple wrapper style (e.g., "**" or "`")
        if styler:sub(1, 1) == "#" then
            -- Header style
            return styler .. " " .. text
        else
            -- Wrapper style
            return styler .. text .. styler
        end
    elseif type(styler) == "table" then
        -- Multiple stylers
        local result = text
        for _, s in ipairs(styler) do
            result = s .. result .. s
        end
        return result
    end
    return text
end

-- Remove Doxygen tag from text
local function remove_tag(line, pattern)
    -- Remove the matched pattern from the beginning of the line
    local cleaned = line:gsub(pattern, "", 1)
    -- Trim leading/trailing whitespace
    cleaned = cleaned:gsub("^%s+", ""):gsub("%s+$", "")
    return cleaned
end

-- Parse a single line of hover text
local function parse_line(line, config, state)
    local trimmed = line:gsub("^%s+", ""):gsub("%s+$", "")
    
    -- Check if we're in a code block
    if state.in_code then
        local matches_end = matches_pattern(trimmed, config.code.ending)
        if matches_end then
            state.in_code = false
            return "```"
        else
            return line -- Return raw line in code blocks
        end
    end
    
    -- Check for code block start
    local matches_start = matches_pattern(trimmed, config.code.start)
    if matches_start then
        state.in_code = true
        -- Check for language specifier like @code{cpp}
        local lang = trimmed:match("%{([^}]+)%}")
        if lang then
            return "```" .. lang
        else
            return "```"
        end
    end
    
    -- Check for header tags
    local is_header, header_pattern = matches_pattern(trimmed, config.header.detect)
    if is_header then
        local text = remove_tag(trimmed, header_pattern)
        return apply_style(text, config.header.styler)
    end
    
    -- Check for line tags (brief, etc.)
    local is_line, line_pattern = matches_pattern(trimmed, config.line.detect)
    if is_line then
        local text = remove_tag(trimmed, line_pattern)
        return apply_style(text, config.line.styler)
    end
    
    -- Check for listing tags
    local is_listing, listing_pattern = matches_pattern(trimmed, config.listing.detect)
    if is_listing then
        local text = remove_tag(trimmed, listing_pattern)
        return config.listing.styler .. text
    end
    
    -- Check for reference tags
    local is_ref, ref_pattern = matches_pattern(trimmed, config.references.detect)
    if is_ref then
        local text = remove_tag(trimmed, ref_pattern)
        return apply_style(text, config.references.styler)
    end
    
    -- Check for return statement
    local is_return = matches_pattern(trimmed, config.return_statement)
    if is_return then
        local text = remove_tag(trimmed, config.return_statement[1])
        return "**Returns:** " .. text
    end
    
    -- Check for grouped tags (parameters, etc.)
    for group_name, patterns in pairs(config.group.detect) do
        local is_group, group_pattern = matches_pattern(trimmed, patterns)
        if is_group then
            local text = remove_tag(trimmed, group_pattern)
            -- For parameters, try to extract name and description
            local param_name, param_desc = text:match("^(%S+)%s+(.+)$")
            if param_name and param_desc then
                return "- " .. apply_style(param_name, config.group.styler) .. ": " .. param_desc
            else
                return "- " .. apply_style(text, config.group.styler)
            end
        end
    end
    
    -- Check for highlight groups (error, warning, info)
    for hl_type, hl_config in pairs(config.hl) do
        local is_hl, hl_pattern = matches_pattern(trimmed, hl_config.detect)
        if is_hl then
            local text = remove_tag(trimmed, hl_pattern)
            local prefix = ""
            if hl_type == "error" then
                prefix = "⚠️ **ERROR:** "
            elseif hl_type == "warning" then
                prefix = "⚡ **WARNING:** "
            elseif hl_type == "info" then
                prefix = "ℹ️ **INFO:** "
            end
            return prefix .. text
        end
    end
    
    -- Return line as-is if no patterns matched
    return line
end

-- Parse complete hover text
function M.parse(text, config)
    if not text or text == "" then
        return text
    end
    
    local lines = {}
    local state = {
        in_code = false,
        current_group = nil,
    }
    
    -- Split text into lines
    for line in text:gmatch("[^\r\n]+") do
        local parsed = parse_line(line, config, state)
        table.insert(lines, parsed)
    end
    
    -- Join lines back together
    return table.concat(lines, "\n")
end

return M
