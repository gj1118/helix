# Helix Lua Plugin System Implementation Plan

## Overview

This document outlines the plan to implement a Lua-based plugin system for Helix, replacing or providing an alternative to the Steel (Scheme) implementation from the mattwparas/helix fork.

## Architecture

### High-Level Design

The plugin system will follow a modular architecture with these key components:

```
helix-plugin/               # New root-level crate
├── src/
│   ├── lib.rs             # Plugin system API and manager
│   ├── lua/               # Lua engine implementation
│   │   ├── mod.rs         # Lua engine wrapper
│   │   ├── api/           # Helix API exposed to Lua
│   │   │   ├── mod.rs
│   │   │   ├── buffer.rs  # Buffer operations
│   │   │   ├── editor.rs  # Editor operations
│   │   │   ├── window.rs  # Window/view operations
│   │   │   ├── picker.rs  # Picker/UI components
│   │   │   ├── lsp.rs     # LSP integration
│   │   │   └── config.rs  # Configuration access
│   │   ├── bridge.rs      # Rust-Lua type conversion
│   │   └── loader.rs      # Plugin loading and lifecycle
│   ├── event.rs           # Event system integration
│   ├── command.rs         # Custom command registration
│   ├── types.rs           # Shared types and traits
│   └── error.rs           # Error handling
└── Cargo.toml

helix-term/src/commands/
├── engine.rs              # Plugin engine abstraction (modify)
└── typed.rs               # Add plugin commands (modify)
```

### Design Principles

1. **Modular**: Keep plugin code in a separate crate for clean separation
2. **Type-Safe**: Use strong typing between Rust and Lua with `mlua`
3. **Event-Driven**: Hook into Helix's existing event system
4. **Backwards Compatible**: Don't break existing Helix functionality
5. **Performance-Conscious**: Minimize overhead of plugin calls
6. **User-Friendly**: Simple Lua API that mirrors Helix concepts

## Phase 1: Foundation (helix-plugin crate)

### 1.1 Create helix-plugin Crate

**File**: `helix-plugin/Cargo.toml`

```toml
[package]
name = "helix-plugin"
version.workspace = true
authors.workspace = true
edition.workspace = true
license.workspace = true
rust-version.workspace = true

[dependencies]
helix-core = { path = "../helix-core" }
helix-view = { path = "../helix-view" }
helix-lsp = { path = "../helix-lsp" }
helix-event = { path = "../helix-event" }
helix-stdx = { path = "../helix-stdx" }

mlua = { version = "0.10", features = ["lua54", "async", "send"] }
anyhow = "1"
thiserror.workspace = true
tokio = { version = "1", features = ["sync", "rt"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
log = "0.4"
once_cell = "1.21"
parking_lot.workspace = true

[dev-dependencies]
tempfile.workspace = true
```

### 1.2 Core Types and Traits

**File**: `helix-plugin/src/types.rs`

Define the core abstractions:
- `Plugin` trait for plugin lifecycle
- `PluginEngine` trait for scripting engines
- `PluginCommand` for custom commands
- `PluginEvent` for event handling

### 1.3 Error Handling

**File**: `helix-plugin/src/error.rs`

```rust
#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    #[error("Plugin initialization failed: {0}")]
    InitializationFailed(String),
    
    #[error("Lua error: {0}")]
    LuaError(#[from] mlua::Error),
    
    #[error("Plugin not found: {0}")]
    PluginNotFound(String),
    
    #[error("Event handler error: {0}")]
    EventHandlerError(String),
    
    #[error("Command execution failed: {0}")]
    CommandExecutionFailed(String),
}

pub type Result<T> = std::result::Result<T, PluginError>;
```

## Phase 2: Lua Engine Implementation

### 2.1 Lua Engine Wrapper

**File**: `helix-plugin/src/lua/mod.rs`

```rust
pub struct LuaEngine {
    runtime: mlua::Lua,
    plugins: HashMap<String, Plugin>,
    event_handlers: HashMap<EventType, Vec<PluginEventHandler>>,
}

impl LuaEngine {
    pub fn new() -> Result<Self>;
    pub fn load_plugin(&mut self, path: &Path) -> Result<()>;
    pub fn register_api(&mut self) -> Result<()>;
    pub fn call_event_handlers(&self, event: &Event) -> Result<()>;
}
```

### 2.2 Helix API - Buffer Operations

**File**: `helix-plugin/src/lua/api/buffer.rs`

Expose buffer operations to Lua:
```lua
-- Lua API examples
buffer = helix.get_buffer()
text = buffer:get_text()
buffer:insert(pos, "text")
buffer:delete(range)
selection = buffer:get_selection()
buffer:set_selection(selection)
```

Rust implementation creates UserData types for:
- `LuaBuffer` wrapping `DocumentId`
- `LuaSelection` wrapping `Selection`
- `LuaRange` wrapping `Range`

### 2.3 Helix API - Editor Operations

**File**: `helix-plugin/src/lua/api/editor.rs`

```lua
-- Lua API examples
helix.editor.move_cursor("left")
helix.editor.insert_mode()
helix.editor.execute_command("open_file")
helix.editor.get_config()
```

### 2.4 Helix API - UI Components

**File**: `helix-plugin/src/lua/api/picker.rs`

```lua
-- Create custom pickers
helix.ui.picker({
    items = {"item1", "item2"},
    on_select = function(item)
        print("Selected: " .. item)
    end
})
```

### 2.5 Helix API - LSP Integration

**File**: `helix-plugin/src/lua/api/lsp.rs`

```lua
-- LSP operations
helix.lsp.goto_definition()
symbols = helix.lsp.document_symbols()
helix.lsp.format()
```

## Phase 3: Event System Integration

### 3.1 Event Types

**File**: `helix-plugin/src/event.rs`

Define event types that plugins can hook into:
```rust
pub enum PluginEventType {
    OnInit,           // Plugin initialization
    OnBufferOpen,     // Buffer opened
    OnBufferSave,     // Before buffer save
    OnBufferClose,    // Buffer closed
    OnBufferChanged,  // Buffer content changed
    OnModeChange,     // Editor mode changed
    OnKeyPress,       // Key pressed (with filtering)
    OnLspAttach,      // LSP attached to buffer
    OnLspDiagnostic,  // LSP diagnostics received
}
```

### 3.2 Event Handler Registration

```lua
-- Lua plugin registration
helix.on("buffer_save", function(buffer)
    print("Saving buffer: " .. buffer:get_path())
    -- Auto-format on save
    helix.lsp.format()
end)

helix.on("buffer_open", function(buffer)
    -- Set buffer-local configuration
    buffer:set_option("indent", 2)
end)
```

### 3.3 Integration with helix-event

Modify `helix-event` to notify the plugin system:
- Add plugin event hooks to the event registry
- Ensure async event handling doesn't block the editor
- Provide event filtering to avoid unnecessary plugin calls

## Phase 4: Command Registration

### 4.1 Custom Commands

**File**: `helix-plugin/src/command.rs`

```rust
pub struct PluginCommand {
    pub name: String,
    pub description: String,
    pub handler: PluginCallback,
}

impl PluginCommand {
    pub fn execute(&self, cx: &mut Context) -> Result<()>;
}
```

### 4.2 Lua Command Registration

```lua
-- Register custom command
helix.register_command({
    name = "my_custom_command",
    description = "Does something custom",
    handler = function(cx)
        -- Command implementation
        helix.editor.insert("Hello from Lua!")
    end
})
```

### 4.3 Integration with Typed Commands

Modify `helix-term/src/commands/typed.rs` to:
- Query plugin system for custom commands
- Add custom commands to command palette
- Handle command execution through plugin system

## Phase 5: Plugin Manager

### 5.1 Plugin Loading

**File**: `helix-plugin/src/lua/loader.rs`

```rust
pub struct PluginLoader {
    plugin_dirs: Vec<PathBuf>,
}

impl PluginLoader {
    pub fn discover_plugins(&self) -> Result<Vec<PathBuf>>;
    pub fn load_plugin(&self, engine: &mut LuaEngine, path: &Path) -> Result<()>;
    pub fn reload_plugin(&self, engine: &mut LuaEngine, name: &str) -> Result<()>;
}
```

Plugin structure:
```
~/.config/helix/plugins/
├── my-plugin/
│   ├── init.lua       # Entry point
│   ├── module.lua     # Additional modules
│   └── plugin.toml    # Metadata (optional)
```

### 5.2 Plugin Lifecycle

```lua
-- init.lua structure
local M = {}

function M.setup(config)
    -- Initialize plugin
    helix.on("buffer_save", M.on_save)
end

function M.on_save(buffer)
    -- Event handler
end

return M
```

## Phase 6: Configuration

### 6.1 Plugin Configuration in config.toml

```toml
[plugins]
enabled = true
plugin-dir = "~/.config/helix/plugins"

[[plugins.plugin]]
name = "my-plugin"
enabled = true

[plugins.plugin.config]
option1 = "value1"
option2 = 42
```

### 6.2 Runtime Configuration Access

```lua
-- Access plugin config from Lua
config = helix.get_plugin_config("my-plugin")
print(config.option1)
```

## Phase 7: Integration with Helix

### 7.1 Modify helix-term

**File**: `helix-term/Cargo.toml`

Add dependency:
```toml
helix-plugin = { path = "../helix-plugin", optional = true }

[features]
default = ["git", "lua-plugins"]
lua-plugins = ["helix-plugin"]
```

**File**: `helix-term/src/application.rs`

Initialize plugin system:
```rust
use helix_plugin::LuaEngine;

pub struct Application {
    // ... existing fields
    plugin_engine: Option<LuaEngine>,
}

impl Application {
    pub fn new() -> Result<Self> {
        // ... existing initialization
        let plugin_engine = Some(LuaEngine::new()?);
        // Load plugins
        Ok(Self { /* ... */ plugin_engine })
    }
}
```

### 7.2 Event Hooks

Add plugin event calls throughout the codebase:
- `helix-view/src/document.rs`: buffer events
- `helix-term/src/ui/editor.rs`: mode changes
- `helix-lsp/src/lib.rs`: LSP events

## Phase 8: Documentation and Examples

### 8.1 Plugin API Documentation

Create `PLUGINS.md` documenting:
- Available API functions
- Event types and when they fire
- Type conversions between Rust and Lua
- Best practices
- Performance considerations

### 8.2 Example Plugins

Create example plugins in `contrib/plugins/`:
- `auto-format`: Auto-format on save
- `project-files`: Custom file picker
- `scratch-buffer`: Create scratch buffers
- `git-blame`: Show git blame in statusline

## Migration Path from Steel

### Comparison: Steel vs Lua

| Feature | Steel | Lua |
|---------|-------|-----|
| Language | Scheme (Lisp) | Lua |
| Learning Curve | Higher (Lisp syntax) | Lower (familiar C-like) |
| Performance | Good | Excellent (LuaJIT) |
| Ecosystem | Smaller | Large (existing plugins) |
| Type System | Dynamic | Dynamic |
| Async Support | Yes | Yes (with mlua) |

### Migration Strategy

1. **Parallel Support**: Initially support both Steel and Lua
2. **Abstraction Layer**: Use `PluginEngine` trait
3. **Compatibility**: Provide helper scripts to convert plugins
4. **Deprecation**: Eventually deprecate Steel if desired

### Engine Abstraction

**File**: `helix-term/src/commands/engine.rs`

```rust
pub enum ScriptingEngine {
    Lua(helix_plugin::LuaEngine),
    #[cfg(feature = "steel")]
    Steel(SteelEngine),
}

impl ScriptingEngine {
    pub fn call_event_handlers(&self, event: &Event) -> Result<()> {
        match self {
            Self::Lua(engine) => engine.call_event_handlers(event),
            #[cfg(feature = "steel")]
            Self::Steel(engine) => engine.call_event_handlers(event),
        }
    }
}
```

## Implementation Timeline

### Phase 1-2: Foundation (Week 1-2) ✅ COMPLETE
- [x] Create `helix-plugin` crate
- [x] Implement basic Lua engine wrapper
- [x] Define core types and error handling
- [x] Implement event system foundation
- [x] Implement buffer API

### Phase 3: Event System Integration (Week 3-4) ✅ COMPLETE
- [x] Implement event system
- [x] Add editor API
- [x] Add UI/picker API
- [x] Integrate with helix-term contexts
- [x] Add event hooks (OnBufferOpen, OnBufferPostSave, OnModeChange, OnViewChange)

### Phase 4: Command Registration (Week 4) ✅ COMPLETE
- [x] specific command API in Lua
- [x] Integration with Typed Commands (helix-term/src/commands/typed.rs)

### Phase 5-6: Plugin Management (Week 5) ✅ COMPLETE
- [x] Implement plugin loader
- [x] Add configuration support
- [x] Create plugin lifecycle management

### Phase 7: Integration (Week 6) ✅ COMPLETE
- [x] Integrate with helix-term application
- [x] Thread PluginManager through Context
- [x] Global event queue for sync hooks

### Phase 8: Real API Implementation (Week 7) 🏗️ IN PROGRESS
- [x] Implement thread-local `Editor` context for Lua callbacks
- [x] Connect `helix.editor.*` APIs to real editor state (mode, cursor, selection, etc.)
- [x] Connect `helix.buffer.*` APIs to real document state (text, path, list, etc.)
- [x] Implement real `helix.ui.notify` and other UI elements
- [x] Update event hooks to pass real editor context

### Phase 9: Polish & Documentation (Week 8)
- [ ] Write user documentation for the Lua API
- [ ] Create example plugins (auto-format, key-bindings, etc.)
- [ ] Performance optimization & bench-marking
- [ ] Final integration testing and bug fixes

## Testing Strategy

### Unit Tests
- Test Lua API functions in isolation
- Test type conversions
- Test error handling

### Integration Tests
- Test plugin loading
- Test event handling
- Test command execution
- Test with example plugins

### Performance Tests
- Benchmark plugin overhead
- Test with multiple plugins
- Memory usage profiling

## Security Considerations

1. **Sandboxing**: Limit Lua's access to system resources
2. **API Safety**: Carefully validate all data crossing Rust-Lua boundary
3. **Resource Limits**: Prevent infinite loops or excessive memory usage
4. **Plugin Isolation**: Prevent plugins from interfering with each other

## Open Questions

1. Should we support LuaJIT for better performance?
2. How do we handle plugin version compatibility?
3. Should plugins be able to modify keybindings?
4. Do we need a plugin registry/marketplace?
5. Should we support debugging Lua plugins?

## Success Criteria

- [ ] Plugins can respond to all major editor events
- [ ] Plugins can register custom commands
- [ ] Plugins can create UI components (pickers)
- [ ] Performance overhead < 1ms per event
- [ ] At least 5 working example plugins
- [ ] Documentation complete and clear
- [ ] No crashes or memory leaks

## Future Enhancements

- Plugin auto-updates
- Plugin dependency management
- More complex UI components (custom windows, panels)
- Plugin marketplace
- TypeScript/Python support via additional engines
- Debug adapter for Lua plugins
- Plugin profiler

---

**Document Version**: 1.0  
**Created**: 2026-01-18  
**Author**: Helix Development Team
