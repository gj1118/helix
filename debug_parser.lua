
local config = {
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
}

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
    local clean_line = trimmed
    
    -- DEBUG PRINT
    print("Orig: '"..line.."'")
    print("Trim: '"..trimmed.."'")
    
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
    
    print("Cleaned: '"..clean_line.."'")
    
    -- MOCKING matches_pattern usage from init.lua
    
    local is_line, line_pattern = matches_pattern(clean_line, config.line.detect)
    if is_line then
        local text = remove_tag(clean_line, line_pattern)
        return apply_style(text, config.line.styler)
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
    
    return clean_line
end

local state = { in_code = false }

print("--- USER REPRODUCTION CASES ---")
print("CASE 1: Brief with extra stars and dash")
-- Screenshot showed: "**** - Calculates the sum..."
print(parse_line(" * **** - Calculates the sum", config, state))

print("\nCASE 2: Param with extra star and colon")
-- Screenshot showed: "* **: a - The..."
print(parse_line(" * * **: a - The first number", config, state))

print("\nCASE 3: Empty line with artifacts")
print(parse_line(" * *", config, state))

