-- Number conversion utility for pretty-hover plugin
-- Converts numbers between decimal, hexadecimal, octal, and binary

local M = {}

-- Convert decimal to hexadecimal
function M.dec_to_hex(num)
    return string.format("0x%X", num)
end

-- Convert decimal to octal
function M.dec_to_oct(num)
    return string.format("0%o", num)
end

-- Convert decimal to binary
function M.dec_to_bin(num)
    local bin = ""
    local n = math.floor(num)
    
    if n == 0 then
        return "0b0"
    end
    
    while n > 0 do
        bin = (n % 2) .. bin
        n = math.floor(n / 2)
    end
    
    return "0b" .. bin
end

-- Detect if a string is a number and return its decimal value
function M.parse_number(str)
    -- Try hexadecimal (0x prefix)
    local hex = str:match("^0[xX]([0-9a-fA-F]+)$")
    if hex then
        return tonumber(hex, 16)
    end
    
    -- Try octal (0 prefix)
    local oct = str:match("^0([0-7]+)$")
    if oct then
        return tonumber(oct, 8)
    end
    
    -- Try binary (0b prefix)
    local bin = str:match("^0[bB]([01]+)$")
    if bin then
        return tonumber(bin, 2)
    end
    
    -- Try decimal
    local dec = str:match("^(%d+)$")
    if dec then
        return tonumber(dec)
    end
    
    return nil
end

-- Get number conversions for display
function M.get_conversions(num)
    if not num then return nil end
    
    local conversions = {
        decimal = tostring(num),
        hexadecimal = M.dec_to_hex(num),
        octal = M.dec_to_oct(num),
        binary = M.dec_to_bin(num),
    }
    
    return conversions
end

-- Format conversions as markdown string
function M.format_conversions(num)
    local conv = M.get_conversions(num)
    if not conv then return "" end
    
    local lines = {
        "**Number Conversions:**",
        "- Decimal: `" .. conv.decimal .. "`",
        "- Hexadecimal: `" .. conv.hexadecimal .. "`",
        "- Octal: `" .. conv.octal .. "`",
        "- Binary: `" .. conv.binary .. "`",
        "",
    }
    
    return table.concat(lines, "\n")
end

return M
