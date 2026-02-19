-- Pretty Hover Plugin for Helix
-- Transforms Doxygen-style LSP hover documentation to readable Markdown

-- ========================================================================
-- CONFIG MODULE (inlined)
-- ========================================================================
local config_defaults = {
    header = { detect = { "[\\@]class" }, styler = '###' },
    line = { detect = { "[\\@]brief" }, styler = '**' },
    listing = { detect = { "[\\@]li" }, styler = " - " },
    references = { detect = { "[\\@]ref", "[\\@]c", "[\\@]name" }, styler = { "**", "`" } },
    group = {
        detect = {
            ["Parameters"] = { "[\\@]param", "[\\@]*param*" },
            ["Types"] = { "[\\@]tparam" },
            ["See"] = { "[\\@]see" },
            ["Return Value"] = { "[\\@]retval" },
        },
        styler = "`",
    },
    code = { start = { "[\\@]code" }, ending = { "[\\@]endcode" } },
    return_statement = { "[\\@]return", "[\\@]*return*" },
    hl = {
        error = { color = "#DC2626", detect = { "[\\@]error", "[\\@]bug" }, line = false },
        warning = { color = "#FBBF24", detect = { "[\\@]warning", "[\\@]thread_safety", "[\\@]throw" }, line = false },
        info = { color = "#2563EB", detect = { "[\\@]remark", "[\\@]note", "[\\@]notes" }, line = false },
    },
    border = {
        color = "ui.text.focus",
        gradient = {
            colors = { "#dc8a78", "#1e66f5" },
            direction = "horizontal",
            steps = 10
        },
        style = "rounded",
        thickness = 1,
        animation_speed = 0
    },
    wrap = true,
    max_width = 80,
    max_height = 25,
    max_type_lines = 12,
    multi_server = true,
    enabled = true,
    number_conversion = true,
}

local function setup_config(user_config)
    local config = {}
    for k, v in pairs(config_defaults) do
        if type(v) == "table" then
            config[k] = {}
            for k2, v2 in pairs(v) do
                if type(v2) == "table" then
                    config[k][k2] = {}
                    for k3, v3 in pairs(v2) do
                        config[k][k2][k3] = v3
                    end
                else
                    config[k][k2] = v2
                end
            end
        else
            config[k] = v
        end
    end
    if user_config then
        for k, v in pairs(user_config) do
            if type(v) == "table" and type(config[k]) == "table" then
                for k2, v2 in pairs(v) do
                    config[k][k2] = v2
                end
            else
                config[k] = v
            end
        end
    end
    return config
end

-- ========================================================================
-- NUMBER MODULE (inlined)
-- ========================================================================
local function dec_to_hex(num)
    return string.format("0x%X", num)
end

local function dec_to_oct(num)
    return string.format("0%o", num)
end

local function dec_to_bin(num)
    local bin = ""
    local n = math.floor(num)
    if n == 0 then return "0b0" end
    while n > 0 do
        bin = (n % 2) .. bin
        n = math.floor(n / 2)
    end
    return "0b" .. bin
end

local function parse_number(str)
    local hex = str:match("^0[xX]([0-9a-fA-F]+)$")
    if hex then return tonumber(hex, 16) end
    local oct = str:match("^0([0-7]+)$")
    if oct then return tonumber(oct, 8) end
    local bin = str:match("^0[bB]([01]+)$")
    if bin then return tonumber(bin, 2) end
    local dec = str:match("^(%d+)$")
    if dec then return tonumber(dec) end
    return nil
end

-- ========================================================================
-- PARSER MODULE (inlined)
-- ========================================================================
local function matches_pattern(line, patterns)
    for _, pattern in ipairs(patterns) do
        if line:match(pattern) then
            return true, pattern
        end
    end
    return false, nil
end

local function apply_style(text, styler)
    if type(styler) == "string" then
        if styler:sub(1, 1) == "#" then
            return styler .. " " .. text
        else
            return styler .. text .. styler
        end
    elseif type(styler) == "table" then
        local result = text
        for _, s in ipairs(styler) do
            result = s .. result .. s
        end
        return result
    end
    return text
end

local function remove_tag(line, pattern)
    local cleaned = line:gsub(pattern, "", 1)
    cleaned = cleaned:gsub("^%s+", ""):gsub("%s+$", "")
    return cleaned
end

local function parse_line(line, config, state)
    local trimmed = line:gsub("^%s+", ""):gsub("%s+$", "")
    
    -- Handle JSDoc/Doxygen leading asterisks (e.g., "* @brief" or "** @brief")
    -- Handle JSDoc/Doxygen leading asterisks (e.g., "* @brief" or "** @brief")
    local clean_line = trimmed
    
    -- Aggressive loop strip of leading "star(s) + space"
    local stripped = trimmed
    while true do
        local next_strip = stripped:gsub("^%*+%s+", "")
        if next_strip == stripped then break end
        stripped = next_strip
    end
    -- Also strip trailing star if line is just stars
    if stripped:match("^%*+$") then
        stripped = ""
    end
    clean_line = stripped
    
    if state.in_code then
        local matches_end = matches_pattern(clean_line, config.code.ending) or matches_pattern(trimmed, config.code.ending)
        if matches_end then
            state.in_code = false
            return "```"
        else
            -- If inside code block, preserve original line but maybe strip the leading * if it exists
            return trimmed:gsub("^[%*]+%s?", "")
        end
    end
    
    local matches_start = matches_pattern(clean_line, config.code.start)
    if matches_start then
        state.in_code = true
        local lang = clean_line:match("{([^}]+)}")
        if lang then
            return "```" .. lang
        else
            return "```"
        end
    end
    
    local is_header, header_pattern = matches_pattern(clean_line, config.header.detect)
    if is_header then
        local text = remove_tag(clean_line, header_pattern)
        return apply_style(text, config.header.styler)
    end
    
    local is_line, line_pattern = matches_pattern(clean_line, config.line.detect)
    if is_line then
        local text = remove_tag(clean_line, line_pattern)
        return apply_style(text, config.line.styler)
    end
    
    local is_listing, listing_pattern = matches_pattern(clean_line, config.listing.detect)
    if is_listing then
        local text = remove_tag(clean_line, listing_pattern)
        return config.listing.styler .. text
    end
    
    local is_ref, ref_pattern = matches_pattern(clean_line, config.references.detect)
    if is_ref then
        local text = remove_tag(clean_line, ref_pattern)
        return apply_style(text, config.references.styler)
    end
    
    local is_return = matches_pattern(clean_line, config.return_statement)
    if is_return then
        local text = remove_tag(clean_line, config.return_statement[1])
        return "**Returns:** " .. text
    end
    
    for group_name, patterns in pairs(config.group.detect) do
        local is_group, group_pattern = matches_pattern(clean_line, patterns)
        if is_group then
            local text = remove_tag(clean_line, group_pattern)
            local param_name, param_desc = text:match("^(%S+)%s+(.+)$")
            if param_name and param_desc then
                return "- " .. apply_style(param_name, config.group.styler) .. ": " .. param_desc
            else
                return "- " .. apply_style(text, config.group.styler)
            end
        end
    end
    
    for hl_type, hl_config in pairs(config.hl) do
        local is_hl, hl_pattern = matches_pattern(clean_line, hl_config.detect)
        if is_hl then
            local text = remove_tag(clean_line, hl_pattern)
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
    
    return line
end
-- ... (skipping other functions if unchanged, but I need to make sure I don't break file)
-- Wait, the tool replaces CONTIGUOUS blocks. The above function is long.
-- I'll replace from `local function parse_line` down to `end` of `parse_line`.

-- AND I need to replace `transform_hover_text`. They are separated by `parse_text`.
-- I should use `multi_replace_file_content` if possible?
-- Or just two calls.
-- I'll use `multi_replace_file_content`.


local function parse_text(text, config)
    if not text or text == "" then
        return text
    end
    
    local lines = {}
    local state = { in_code = false, current_group = nil }
    
    -- Split by newline while preserving empty lines
    for line in (text .. "\n"):gmatch("(.-)\r?\n") do
        local parsed = parse_line(line, config, state)
        table.insert(lines, parsed)
    end
    
    -- Remove the last added line if it was just from the appended newline
    if #lines > 0 and lines[#lines] == "" and text:sub(-1) ~= "\n" then
        table.remove(lines)
    end
    
    return table.concat(lines, "\n")
end

local function extract_signature(text)
    -- Try to match first code block (standard markdown format)
    local s, e, lang, content = text:find("^%s*```(%w*)\n(.-)\n```")
    if content then
        local rest = text:sub(e + 1)

        -- For TypeScript content, extract a concise signature from the code block
        if lang == "typescript" then
            local content_lines = {}
            for line in content:gmatch("[^\r\n]+") do
                table.insert(content_lines, line)
            end

            if #content_lines > 0 then
                local first_line = content_lines[1]:gsub("^%s+", ""):gsub("%s+$", "")

                -- Extract just the variable/function name and basic type
                -- For "const store: ToolkitStore<{...}", extract "const store: ToolkitStore"
                local concise = first_line:match("^(.-)<") or first_line:match("^(.-)%{") or first_line
                if concise and #concise < #first_line then
                    -- Add appropriate ending based on what was truncated
                    if first_line:match("<") then
                        concise = concise .. "<...>"
                    elseif first_line:match("%{") then
                        concise = concise .. "{...}"
                    end
                end

                -- For body, include ALL content - no truncation needed with scrollable popup
                local body_content = "```typescript\n" .. content .. "\n```"

                if rest and rest ~= "" then
                    body_content = body_content .. "\n\n" .. rest
                end

                return concise or first_line, lang, body_content
            end
        end

        return content, lang, rest
    end

    -- Try to extract TypeScript/JavaScript signature patterns
    local lines = {}
    for line in text:gmatch("[^\r\n]+") do
        table.insert(lines, line)
    end

    if #lines > 0 then
        local first_line = lines[1]
        -- Check if first line looks like a signature (contains certain patterns)
        if first_line:match("^[%w_]+%s*[:%(]") or  -- function/variable with : or (
           first_line:match("^type%s+") or         -- type definition
           first_line:match("^interface%s+") or    -- interface
           first_line:match("^class%s+") or        -- class
           first_line:match("^const%s+.*:") then   -- typed const

            -- Use first line as signature, rest as body
            local rest_lines = {}
            for i = 2, #lines do
                table.insert(rest_lines, lines[i])
            end
            return first_line, "typescript", table.concat(rest_lines, "\n")
        end
    end

    -- Fallback: no signature extraction
    return nil, nil, text
end

-- ========================================================================
-- PLUGIN MAIN MODULE
-- ========================================================================
local M = {}
local config = nil

function M.setup(user_config)
    -- For now, use the enhanced defaults until TOML loading is implemented
    local enhanced_config = user_config or {}

    -- Apply enhanced border configuration as default
    if not enhanced_config.border then
        enhanced_config.border = {
            color = "ui.text.focus",
            gradient = {
                colors = { "#dc8a78", "#1e66f5" },
                direction = "horizontal",
                steps = 10
            },
            style = "rounded",
            thickness = 1,
            animation_speed = 0
        }
        helix.log.warn("[pretty-hover] Applied default gradient border config")
    end

    config = setup_config(enhanced_config)

    -- Debug: Log the final border config
    if config.border then
        helix.log.warn("[pretty-hover] Final border config - style: " .. tostring(config.border.style))
        if config.border.gradient then
            helix.log.warn("[pretty-hover] Gradient colors: " .. table.concat(config.border.gradient.colors or {}, ", "))
        else
            helix.log.warn("[pretty-hover] No gradient in final config")
        end
    else
        helix.log.warn("[pretty-hover] No border config in final config")
    end

    if not config.enabled then
        helix.log.warn("[pretty-hover] Plugin is disabled")
        return
    end

    helix.log.warn("[pretty-hover] Plugin loaded and enabled")
    M.register_commands()
    -- Only register renderer to avoid conflict with transformer
    M.register_hover_renderer()
    helix.log.warn("[pretty-hover] Hover renderer registered successfully")
end

function M.register_hover_renderer()
    if not helix.lsp or not helix.lsp.register_hover_renderer then
        helix.log.warn("[pretty-hover] LSP hover renderer API not available")
        return
    end

    helix.lsp.register_hover_renderer(M.render_hover)
end

function M.register_hover_transformer()
    if not helix.lsp or not helix.lsp.register_hover_transformer then
        helix.log.warn("[pretty-hover] LSP hover transformation API not available")
        return
    end
    
    helix.lsp.register_hover_transformer(function(hover_text)
        if not config.enabled then
            return hover_text
        end
        return M.transform_hover_text(hover_text)
    end)
end

function M.register_commands()
    helix.register_command({
        name = "toggle_pretty_hover",
        doc = "Toggle the pretty-hover plugin on/off",
        handler = function()
            config.enabled = not config.enabled
            local status = config.enabled and "enabled" or "disabled"
            helix.ui.notify("Pretty Hover " .. status)
        end
    })
end

function M.transform_hover_text(raw_text)
    helix.log.warn("[pretty-hover] ===== TRANSFORM_HOVER_TEXT CALLED =====")
    helix.log.warn("[pretty-hover] Transform called on text length: " .. (raw_text and #raw_text or 0))

    if raw_text and raw_text ~= "" then
        helix.log.warn("[pretty-hover] Transform raw text: " .. raw_text)
    end
    
    if not raw_text or raw_text == "" then
        return raw_text
    end
    local result = parse_text(raw_text, config)
    helix.log.warn("[pretty-hover] Transformation complete")
    return result
end

function M.render_hover(raw_text)
    helix.log.warn("[pretty-hover] ===== RENDER_HOVER CALLED =====")

    if not config.enabled then
        helix.log.warn("[pretty-hover] Plugin disabled, not rendering")
        return nil
    end

    helix.log.warn("[pretty-hover] Rendering hover with gradient config: " .. tostring(config.border and config.border.gradient and "enabled" or "disabled"))
    helix.log.warn("[pretty-hover] Raw text length: " .. tostring(raw_text and #raw_text or 0))

    -- Debug: Show raw text in a more visible way
    if raw_text and raw_text ~= "" then
        local preview = raw_text:gsub("\n", "\\n"):sub(1, 200)
        helix.log.warn("[pretty-hover] Raw text preview: [" .. preview .. "]")
    else
        helix.log.warn("[pretty-hover] No raw text provided")
        return nil
    end
    
    local signature, lang, body_text = extract_signature(raw_text)

    helix.log.warn("[pretty-hover] Extracted signature: " .. (signature or "nil"))
    helix.log.warn("[pretty-hover] Body text length: " .. (body_text and #body_text or 0))

    local header_content

    if signature then
        -- Clean up signature (trim whitespace)
        signature = signature:gsub("^%s+", ""):gsub("%s+$", "")
        header_content = { type = "text", content = signature, style = { fg = "ui.text.focus", modifiers = {"bold"} } }
        helix.log.warn("[pretty-hover] Using signature as header: " .. signature)
    else
        header_content = { type = "text", content = "Documentation", style = { modifiers = {"italic"} } }
        body_text = raw_text
        helix.log.warn("[pretty-hover] No signature found, using fallback header")
    end

    local transformed = parse_text(body_text, config)

    helix.log.warn("[pretty-hover] Transformed text length: " .. (transformed and #transformed or 0))
    if transformed and transformed ~= "" then
        local preview = transformed:gsub("\n", "\\n"):sub(1, 200)
        helix.log.warn("[pretty-hover] Transformed preview: [" .. preview .. "]")
    end

    -- For TypeScript/JavaScript content without Doxygen tags, use simpler formatting
    local desc_lines = {}
    local detail_lines = {}

    -- Check if the transformed text has any Doxygen-style content
    local has_doxygen = transformed:match("@%w+") or transformed:match("^%*%*[^*]+%*%*") or transformed:match("^Returns:")

    if has_doxygen then
        -- Split transformed text into description and details for Doxygen content
        local in_details = false
        for line in transformed:gmatch("[^\n]*") do
            if not in_details and (line:match("^Returns:") or line:match("^⚡") or line:match("^ℹ")) then
                in_details = true
            end
            if in_details then
                table.insert(detail_lines, line)
            else
                table.insert(desc_lines, line)
            end
        end
    else
        -- For TypeScript/simple content, just use line breaks to separate content
        for line in transformed:gmatch("[^\n]*") do
            -- Trim empty lines at the start
            if #desc_lines > 0 or line:match("%S") then
                table.insert(desc_lines, line)
            end
        end
    end

    helix.log.warn("[pretty-hover] Description lines: " .. #desc_lines)
    helix.log.warn("[pretty-hover] Detail lines: " .. #detail_lines)
    
    local style = {}
    if type(config.border) == "table" then
        if config.border.color then style.fg = config.border.color end
        if config.border.gradient then
            style.gradient = {
                colors = config.border.gradient.colors or { "#dc8a78", "#1e66f5" },
                direction = config.border.gradient.direction or "horizontal",
                steps = config.border.gradient.steps or 10
            }
            helix.log.warn("[pretty-hover] Applied gradient: " .. table.concat(style.gradient.colors, ", "))
        else
            helix.log.warn("[pretty-hover] No gradient config found")
        end
    end

    local border_style = "rounded"
    if config.border and config.border.style then
        border_style = config.border.style
    end
    
    local separator = { type = "separator", style = { fg = "ui.text.inactive" } }
    
    local children = {
        header_content,
        separator,
    }
    
    local desc_text = table.concat(desc_lines, "\n")
    -- Trim leading/trailing whitespace from description
    desc_text = desc_text:gsub("^%s+", ""):gsub("%s+$", "")
    if desc_text ~= "" then
        table.insert(children, { type = "markdown", content = desc_text })
    end
    
    if #detail_lines > 0 then
        local detail_text = table.concat(detail_lines, "\n")
        detail_text = detail_text:gsub("^%s+", ""):gsub("%s+$", "")
        if detail_text ~= "" then
            table.insert(children, separator)
            table.insert(children, { type = "markdown", content = detail_text })
        end
    end
    
    -- Combine all content into markdown that Helix can render with native scrolling and borders
    local full_content = "\n"  -- Start with padding

    -- Add header
    if header_content and header_content.content then
        full_content = full_content .. "**" .. header_content.content .. "**\n\n"
    end

    -- Add main content
    local desc_text = table.concat(desc_lines, "\n")
    desc_text = desc_text:gsub("^%s+", ""):gsub("%s+$", "")
    if desc_text ~= "" then
        full_content = full_content .. desc_text .. "\n\n"
    end

    -- Add details
    if #detail_lines > 0 then
        local detail_text = table.concat(detail_lines, "\n")
        detail_text = detail_text:gsub("^%s+", ""):gsub("%s+$", "")
        if detail_text ~= "" then
            full_content = full_content .. "---\n\n" .. detail_text .. "\n"
        end
    end

    -- End with padding
    full_content = full_content .. "\n"

    -- Return a Block with gradient border that contains markdown content with padding
    -- This will be wrapped by Popup which handles scrolling
    local result = {
        type = "block",
        border = "rounded",
        style = style,
        direction = "vertical",
        children = {
            {
                type = "markdown",
                content = full_content
            }
        }
    }

    helix.log.warn("[pretty-hover] Returning bordered block with markdown content length: " .. #full_content)
    return result
end

-- Initialize the plugin
helix.log.warn("[pretty-hover] Plugin file loaded, initializing...")
M.setup()
helix.log.warn("[pretty-hover] Plugin initialization complete")

return M
