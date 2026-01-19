use thiserror::Error;

/// Errors that can occur in the plugin system
#[derive(Debug, Error)]
pub enum PluginError {
    /// Plugin initialization failed
    #[error("Plugin initialization failed: {0}")]
    InitializationFailed(String),

    /// Lua runtime error
    #[error("Lua error: {0}")]
    LuaError(#[from] mlua::Error),

    /// Plugin not found
    #[error("Plugin not found: {0}")]
    PluginNotFound(String),

    /// Plugin already loaded
    #[error("Plugin already loaded: {0}")]
    PluginAlreadyLoaded(String),

    /// Event handler error
    #[error("Event handler error in plugin '{plugin}': {error}")]
    EventHandlerError { plugin: String, error: String },

    /// Command execution failed
    #[error("Command execution failed: {0}")]
    CommandExecutionFailed(String),

    /// Invalid plugin structure
    #[error("Invalid plugin structure: {0}")]
    InvalidPluginStructure(String),

    /// IO error
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    /// Configuration error
    #[error("Configuration error: {0}")]
    ConfigError(String),

    /// API access error
    #[error("API access error: {0}")]
    ApiAccessError(String),
}

/// Result type for plugin operations
pub type Result<T> = std::result::Result<T, PluginError>;
