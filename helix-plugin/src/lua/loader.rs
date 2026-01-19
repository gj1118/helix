use crate::error::{PluginError, Result};
use crate::types::{Plugin, PluginMetadata};
use log::{debug, info, warn};
use std::path::{Path, PathBuf};

/// Plugin loader responsible for discovering and loading plugins
pub struct PluginLoader {
    /// Directories to search for plugins
    plugin_dirs: Vec<PathBuf>,
}

impl PluginLoader {
    /// Create a new plugin loader with the given plugin directories
    pub fn new(plugin_dirs: Vec<PathBuf>) -> Self {
        Self { plugin_dirs }
    }

    /// Discover all plugins in the configured directories
    pub fn discover_plugins(&self) -> Result<Vec<Plugin>> {
        let mut plugins = Vec::new();

        for dir in &self.plugin_dirs {
            if !dir.exists() {
                debug!("Plugin directory does not exist: {:?}", dir);
                continue;
            }

            if !dir.is_dir() {
                warn!("Plugin path is not a directory: {:?}", dir);
                continue;
            }

            // Iterate through subdirectories
            let entries = std::fs::read_dir(dir)?;

            for entry in entries {
                let entry = entry?;
                let path = entry.path();

                if path.is_dir() {
                    match self.load_plugin_metadata(&path) {
                        Ok(plugin) => {
                            info!("Discovered plugin: {} at {:?}", plugin.metadata.name, path);
                            plugins.push(plugin);
                        }
                        Err(e) => {
                            warn!("Failed to load plugin at {:?}: {}", path, e);
                        }
                    }
                }
            }
        }

        Ok(plugins)
    }

    /// Load plugin metadata from a directory
    fn load_plugin_metadata(&self, path: &Path) -> Result<Plugin> {
        // Check for plugin.toml
        let metadata_file = path.join("plugin.toml");
        let metadata = if metadata_file.exists() {
            let content = std::fs::read_to_string(&metadata_file)?;
            toml::from_str::<PluginMetadata>(&content).map_err(|e| {
                PluginError::ConfigError(format!("Failed to parse plugin.toml: {}", e))
            })?
        } else {
            // Generate default metadata from directory name
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(|| {
                    PluginError::InvalidPluginStructure("Invalid plugin directory name".to_string())
                })?
                .to_string();

            PluginMetadata {
                name,
                ..Default::default()
            }
        };

        // Verify entry point exists
        let entry = metadata.entry.as_deref().unwrap_or("init.lua");
        let entry_path = path.join(entry);

        if !entry_path.exists() {
            return Err(PluginError::InvalidPluginStructure(format!(
                "Entry point '{}' not found in {:?}",
                entry, path
            )));
        }

        Ok(Plugin {
            metadata,
            path: path.to_path_buf(),
            enabled: true,
        })
    }

    /// Get default plugin directories
    pub fn default_plugin_dirs() -> Vec<PathBuf> {
        let mut dirs = Vec::new();

        // User config directory
        let config_dir = helix_loader::config_dir();
        dirs.push(config_dir.join("plugins"));

        // System-wide plugin directory (if installed)
        #[cfg(target_os = "linux")]
        {
            dirs.push(PathBuf::from("/usr/share/helix/plugins"));
            dirs.push(PathBuf::from("/usr/local/share/helix/plugins"));
        }

        #[cfg(target_os = "macos")]
        {
            dirs.push(PathBuf::from("/Library/Application Support/helix/plugins"));
        }

        dirs
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_discover_empty_directory() {
        let temp_dir = TempDir::new().unwrap();
        let loader = PluginLoader::new(vec![temp_dir.path().to_path_buf()]);

        let plugins = loader.discover_plugins().unwrap();
        assert_eq!(plugins.len(), 0);
    }

    #[test]
    fn test_load_plugin_with_metadata() {
        let temp_dir = TempDir::new().unwrap();
        let plugin_dir = temp_dir.path().join("test-plugin");
        std::fs::create_dir(&plugin_dir).unwrap();

        // Create plugin.toml
        let metadata = r#"
            name = "test-plugin"
            version = "1.0.0"
            description = "A test plugin"
            author = "Test Author"
            entry = "init.lua"
        "#;
        std::fs::write(plugin_dir.join("plugin.toml"), metadata).unwrap();

        // Create init.lua
        std::fs::write(plugin_dir.join("init.lua"), "-- Test plugin").unwrap();

        let loader = PluginLoader::new(vec![temp_dir.path().to_path_buf()]);
        let plugins = loader.discover_plugins().unwrap();

        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].metadata.name, "test-plugin");
        assert_eq!(plugins[0].metadata.version, "1.0.0");
    }

    #[test]
    fn test_load_plugin_without_metadata() {
        let temp_dir = TempDir::new().unwrap();
        let plugin_dir = temp_dir.path().join("simple-plugin");
        std::fs::create_dir(&plugin_dir).unwrap();

        // Create only init.lua
        std::fs::write(plugin_dir.join("init.lua"), "-- Simple plugin").unwrap();

        let loader = PluginLoader::new(vec![temp_dir.path().to_path_buf()]);
        let plugins = loader.discover_plugins().unwrap();

        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].metadata.name, "simple-plugin");
        assert_eq!(plugins[0].metadata.version, "0.1.0");
    }

    #[test]
    fn test_missing_entry_point() {
        let temp_dir = TempDir::new().unwrap();
        let plugin_dir = temp_dir.path().join("broken-plugin");
        std::fs::create_dir(&plugin_dir).unwrap();

        // Create plugin.toml but no init.lua
        let metadata = r#"
            name = "broken-plugin"
            version = "1.0.0"
        "#;
        std::fs::write(plugin_dir.join("plugin.toml"), metadata).unwrap();

        let loader = PluginLoader::new(vec![temp_dir.path().to_path_buf()]);
        let plugins = loader.discover_plugins().unwrap();

        // Should skip the broken plugin
        assert_eq!(plugins.len(), 0);
    }
}
