/// Helix API exposed to Lua plugins
///
/// This module contains all the Rust-Lua bridge code that exposes
/// Helix functionality to Lua plugins.
pub mod buffer;
pub mod editor;
pub mod explorer;
pub mod log;
pub mod lsp;
pub mod process;
pub mod quickfix;
pub mod statusline;
pub(crate) mod style;
pub mod ui;
pub mod window;

// Re-exports for convenience
pub use buffer::*;
pub use editor::*;
pub use explorer::*;
pub use log::*;
pub use lsp::*;
pub use process::register_process_api;
pub use quickfix::*;
pub use statusline::*;
pub use ui::*;
pub use window::*;
