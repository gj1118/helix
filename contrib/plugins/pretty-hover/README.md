# Pretty Hover Plugin for Helix

Transform LSP hover messages from Doxygen-style documentation to beautiful, readable Markdown.

## Features

- 🎨 **Automatic Formatting**: Converts Doxygen tags (`@param`, `@return`, `@brief`, etc.) to Markdown
- 🔢 **Number Conversion**: Shows hex, octal, and binary representations of numeric values
- ⚙️ **Configurable**: Customize detection patterns and styling for each tag type
- 🎯 **Multi-Server Support**: Works with multiple LSP servers simultaneously
- 🚀 **Lightweight**: Pure Lua implementation with minimal overhead

## Installation

This plugin comes bundled with Helix in the `contrib/plugins/` directory.

To enable it, you can load it manually or add it to your Helix configuration.

## Usage

### Commands

- `:pretty_hover` - Show prettified LSP hover information
- `:toggle_pretty_hover` - Toggle the plugin on/off

### Default Keybinding

Currently, the plugin requires manual activation via the `:pretty_hover` command. You can bind it to a key in your `config.toml`:

```toml
[keys.normal]
K = ":pretty_hover"  # Override default hover
```

## Configuration

The plugin supports extensive configuration through Lua. Create or modify the plugin config:

```lua
-- In your plugin config
local config = {
    enabled = true,
    number_conversion = true,
    
    -- Customize tag styling
    header = {
        detect = { "[\\@]class" },
        styler = '###',  -- Markdown header
    },
    
    line = {
        detect = { "[\\@]brief" },
        styler = '**',  -- Bold
    },
    
    -- ... more options
}
```

## Supported Doxygen Tags

The plugin recognizes and transforms the following Doxygen tags:

### Documentation Tags
- `@brief` - Brief description (bold)
- `@class` - Class name (header)
- `@param` - Parameter (grouped, styled)
- `@tparam` - Template parameter (grouped)
- `@return` / `@retval` - Return value
- `@see` - See also references

### Code Blocks
- `@code{language}` / `@endcode` - Code blocks with syntax highlighting

### Special Tags
- `@error` / `@bug` - Error messages (⚠️ prefix)
- `@warning` / `@throw` - Warnings (⚡ prefix)
- `@note` / `@remark` - Information (ℹ️ prefix)

### Lists
- `@li` - List items (converted to Markdown lists)

### References
- `@ref`, `@c`, `@name` - Code references (inline code style)

## Number Conversion

When you hover over a numeric value, the plugin automatically detects and displays conversions:

```
Number Conversions:
- Decimal: 255
- Hexadecimal: 0xFF
- Octal: 0377
- Binary: 0b11111111
```

Supported formats:
- Decimal: `255`
- Hexadecimal: `0xFF` or `0xff`
- Octal: `0377`
- Binary: `0b11111111`

## Current Limitations

> **Note**: The Helix Lua plugin API currently doesn't expose LSP hover responses directly. This plugin demonstrates the parsing and transformation logic, but requires manual activation via the `:pretty_hover` command.

To enable automatic hover transformation, the Helix plugin API would need to be extended with:
1. Access to raw LSP hover responses
2. Ability to intercept and modify hover before display
3. Event hooks for hover actions

## Examples

### Before (Raw Doxygen)
```
@brief Calculates the sum of two numbers
@param a The first number
@param b The second number
@return The sum of a and b
```

### After (Prettified Markdown)
```markdown
**Calculates the sum of two numbers**

- `a`: The first number
- `b`: The second number

**Returns:** The sum of a and b
```

## Development

### File Structure
```
pretty-hover/
├── plugin.toml      # Plugin metadata
├── init.lua         # Entry point and command registration
├── config.lua       # Configuration defaults and management
├── parser.lua       # Doxygen to Markdown transformation
├── number.lua       # Number conversion utilities
└── README.md        # This file
```

### Testing

Test the parser transformation:

```lua
local parser = require("parser")
local config = require("config").defaults

local raw = "@brief Test function\n@param x Input value"
local prettified = parser.parse(raw, config)
print(prettified)
```

## Contributing

Contributions are welcome! Areas for improvement:
- Additional Doxygen tag support
- Custom styling themes
- Performance optimizations
- Integration with Helix plugin API (when available)

## License

This plugin is part of the Helix editor project and follows the same license.

## Author

Helix Community

## Version

0.1.0
