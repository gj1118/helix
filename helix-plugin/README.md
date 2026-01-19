# Helix Lua Plugin System

A Lua-based plugin system for the Helix text editor, enabling users to extend functionality through custom scripts.

## Features

- **Event-driven architecture**: React to editor events like buffer open, save, mode changes, etc.
- **Custom commands**: Register new commands accessible from the command palette
- **UI integration**: Create custom pickers and UI components
- **LSP integration**: Interact with Language Server Protocol features
- **Safe sandboxing**: Plugins run in a sandboxed Lua environment
- **Hot reloading**: Reload plugins without restarting the editor (planned)

## Quick Start

### 1. Enable Plugins

Add to your `~/.config/helix/config.toml`:

```toml
[plugins]
enabled = true
plugin-dir = "~/.config/helix/plugins"
```

### 2. Create Your First Plugin

Create a directory structure:

```
~/.config/helix/plugins/
└── my-plugin/
    ├── plugin.toml    # Metadata (optional)
    └── init.lua       # Entry point (required)
```

**plugin.toml**:
```toml
name = "my-plugin"
version = "0.1.0"
description = "My first Helix plugin"
author = "Your Name"
```

**init.lua**:
```lua
-- Simple plugin that logs when buffers are opened
helix.on("buffer_open", function(event)
    print("Buffer opened!")
end)
```

### 3. Restart Helix

Plugins are loaded on startup. Your plugin should now be active!

## API Reference

### Event System

Subscribe to editor events using `helix.on(event_name, callback)`:

```lua
-- Available events:
-- "init"              - Plugin system initialized
-- "ready"             - Editor ready
-- "buffer_open"       - Buffer opened
-- "buffer_pre_save"   - Before buffer save
-- "buffer_post_save"  - After buffer save
-- "buffer_close"      - Buffer closed
-- "buffer_changed"    - Buffer content changed
-- "mode_change"       - Editor mode changed
-- "key_press"         - Key pressed
-- "lsp_attach"        - LSP attached to buffer
-- "lsp_diagnostic"    - LSP diagnostics received
-- "selection_change"  - Selection changed
-- "view_change"       - View/window changed

helix.on("buffer_save", function(event)
    print("Saving buffer: " .. event.path)
end)
```

### Buffer Operations (Planned)

```lua
-- Get current buffer
local buffer = helix.get_buffer()

-- Read buffer content
local text = buffer:get_text()
local selection = buffer:get_selection()

-- Modify buffer
buffer:insert(position, "text")
buffer:delete(range)
buffer:set_selection(selection)
```

### Editor Operations (Planned)

```lua
-- Execute commands
helix.editor.execute_command("open_file")

-- Mode operations
helix.editor.insert_mode()
helix.editor.normal_mode()

-- Cursor movement
helix.editor.move_cursor("left")
helix.editor.move_cursor("right")
```

### UI Components (Planned)

```lua
-- Create a custom picker
helix.ui.picker({
    items = {"option1", "option2", "option3"},
    on_select = function(item)
        print("Selected: " .. item)
    end,
    prompt = "Choose an option:"
})

-- Show a notification
helix.ui.notify("Hello from plugin!", "info")
```

### LSP Integration (Planned)

```lua
-- Format current buffer
helix.lsp.format()

-- Get document symbols
local symbols = helix.lsp.document_symbols()

-- Go to definition
helix.lsp.goto_definition()
```

### Custom Commands (Planned)

```lua
-- Register a custom command
helix.register_command({
    name = "my_command",
    description = "Does something useful",
    handler = function()
        helix.ui.notify("Command executed!")
    end
})
```

## Plugin Structure

### Basic Plugin

Minimal plugin with just an `init.lua`:

```
my-plugin/
└── init.lua
```

### Advanced Plugin

Full-featured plugin with metadata and modules:

```
my-plugin/
├── plugin.toml     # Metadata
├── init.lua        # Entry point
├── config.lua      # Configuration handling
├── commands.lua    # Custom commands
└── utils.lua       # Utility functions
```

### Module System

Use Lua's module system to organize code:

**init.lua**:
```lua
local commands = require("my-plugin.commands")
local config = require("my-plugin.config")

local M = {}

function M.setup(user_config)
    config.setup(user_config)
    commands.register_all()
end

M.setup()
return M
```

## Configuration

### Global Plugin Configuration

In `config.toml`:

```toml
[plugins]
enabled = true
plugin-dir = "~/.config/helix/plugins"

# Configure individual plugins
[[plugins.plugin]]
name = "auto-save"
enabled = true

[plugins.plugin.config]
delay = 1000
auto_format = true
```

### Accessing Configuration in Lua

```lua
local config = helix.get_plugin_config("my-plugin")
if config then
    local delay = config.delay or 1000
    print("Delay: " .. delay)
end
```

## Example Plugins

See `contrib/plugins/` for example plugins:

- **auto-save**: Automatically save buffers
- **git-blame**: Show git blame in statusline (planned)
- **project-files**: Quick file navigation (planned)
- **scratch-buffer**: Create temporary buffers (planned)

## Best Practices

1. **Error Handling**: Always use `pcall` for operations that might fail:
   ```lua
   helix.on("buffer_save", function(event)
       local success, err = pcall(function()
           -- Your code here
       end)
       if not success then
           print("Error: " .. tostring(err))
       end
   end)
   ```

2. **Performance**: Minimize work in event handlers, especially for frequent events like `buffer_changed`

3. **Naming**: Use descriptive names for your plugins and commands to avoid conflicts

4. **Documentation**: Include a README.md in your plugin directory

5. **Testing**: Test your plugin with different editor states and configurations

## Security

Plugins run in a sandboxed Lua environment with restricted access:

- **Disabled functions**: `os.execute`, `os.exit`, `io.*`, `loadfile`, `dofile`
- **File system**: Only specific API calls can access files
- **Network**: No network access (currently)

## Troubleshooting

### Plugin Not Loading

1. Check that `plugins.enabled = true` in config.toml
2. Verify plugin directory path is correct
3. Ensure `init.lua` exists in the plugin directory
4. Check Helix logs for error messages

### Event Handlers Not Firing

1. Verify event name is correct (see API Reference)
2. Check that the event is being triggered (add debug prints)
3. Ensure no errors in the handler callback

### Performance Issues

1. Profile your event handlers
2. Avoid heavy computations in frequent events
3. Use debouncing for expensive operations

## Development

### Building

The plugin system is integrated into Helix. Build with:

```bash
cargo build --release --features lua-plugins
```

### Testing

Run plugin system tests:

```bash
cargo test -p helix-plugin
```

### Debugging

Enable debug logging:

```bash
RUST_LOG=helix_plugin=debug hx
```

## Roadmap

- [x] Basic event system
- [x] Plugin loading and discovery
- [x] Sandboxed Lua environment
- [ ] Buffer API
- [ ] Editor API
- [ ] UI components  
- [ ] Custom commands
- [ ] LSP integration
- [ ] Hot reloading
- [ ] Plugin marketplace
- [ ] Debug adapter for Lua

## Contributing

Contributions are welcome! Please see the main [Helix contributing guide](../CONTRIBUTING.md).

## License

Licensed under the Mozilla Public License 2.0. See [LICENSE](../LICENSE) for details.
