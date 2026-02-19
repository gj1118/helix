# Helix Plugins Configuration

This directory contains plugins for Helix. Each plugin has its own `config.toml` file for easy customization.

## Available Plugins

### 1. Pretty-Hover
**Location:** `pretty-hover/`
**Description:** Enhances LSP hover documentation with beautiful styling, gradient borders, and improved formatting.

**Configuration:** Edit `pretty-hover/config.toml`

**Key Features:**
- Configurable gradient borders with multiple colors
- Border styles: rounded, square, thick, double, etc.
- Animation support for borders
- Preset themes (ocean, sunset, rainbow, retro, minimal)
- Advanced Doxygen/JSDoc parsing

**Quick Config Examples:**

```toml
# Ocean theme with vertical gradient
[border.gradient]
colors = ["#1e3a8a", "#3b82f6", "#06b6d4", "#0891b2"]
direction = "vertical"
steps = 15

# Animated rainbow border
[border.gradient]
colors = ["#ff0000", "#ff8000", "#ffff00", "#80ff00", "#00ff00", "#00ff80", "#00ffff"]
direction = "horizontal"
steps = 24
[border]
animation_speed = 2
```

### 2. Auto-Save
**Location:** `auto-save/`
**Description:** Automatically saves files at configurable intervals.

**Configuration:** Edit `auto-save/config.toml`

**Key Features:**
- Configurable save intervals
- Save on focus loss or buffer switch
- File type exclusions
- Notification options

### 3. Inline-Diagnostic
**Location:** `inline-diagnostic/`
**Description:** Shows diagnostic messages inline with your code.

**Configuration:** Edit `inline-diagnostic/config.toml`

**Key Features:**
- Configurable severity levels
- Custom colors and icons
- Position control (after text, overlay)
- Per-line limits

## Configuration Methods

### Method 1: TOML Files (Recommended)
Each plugin has its own `config.toml` file in its directory. This is the easiest way to configure plugins:

```bash
# Edit pretty-hover settings
vim contrib/plugins/pretty-hover/config.toml

# Edit auto-save settings
vim contrib/plugins/auto-save/config.toml

# Edit inline-diagnostic settings
vim contrib/plugins/inline-diagnostic/config.toml
```

### Method 2: Lua Configuration (Advanced)
For advanced users, you can also modify the `config.lua` files directly or create custom presets.

### Method 3: Main Helix Config
Enable/disable plugins in your main Helix `config.toml`:

```toml
[plugins]
enabled = true
plugin_dirs = ["contrib/plugins"]
enabled_plugins = ["auto-save", "inline-diagnostic", "pretty-hover"]
```

## Pretty-Hover Preset Themes

You can quickly apply preset themes by uncommenting the appropriate section in `pretty-hover/config.toml`:

- **Ocean:** Blue gradient, vertical direction
- **Sunset:** Orange to red gradient, diagonal direction
- **Rainbow:** Animated multi-color gradient
- **Retro:** Green terminal-style gradient
- **Minimal:** Simple border, no gradient
- **Colorful:** Multi-color horizontal gradient

## Custom Gradient Configuration

For pretty-hover, you can create custom gradient effects:

```toml
[border.gradient]
# Use 2-5 colors for gradients
colors = ["#your_start_color", "#your_middle_color", "#your_end_color"]

# Choose direction
direction = "horizontal"  # or "vertical", "diagonal", "radial"

# Smooth gradients with more steps
steps = 20

# Enable animation (0 = none, 1-10 = speed)
[border]
animation_speed = 1
```

## Border Styles

Available border styles for pretty-hover:

- `"rounded"` - Rounded corners (default)
- `"all"` - Square corners on all sides
- `"top"` - Border only on top
- `"bottom"` - Border only on bottom
- `"left"` - Border only on left
- `"right"` - Border only on right
- `"none"` - No border

Border thickness options (1-5):
- `1` - Thin lines (default)
- `2` - Thick lines
- `3` - Double lines
- `4` - Block characters
- `5` - Full block characters

## Troubleshooting

1. **Plugin not loading:** Check that the plugin is listed in `enabled_plugins` in your main config.toml
2. **Config not applying:** Restart Helix after making config changes
3. **Syntax errors:** Validate your TOML syntax using an online validator
4. **Colors not showing:** Ensure your terminal supports true color (24-bit color)