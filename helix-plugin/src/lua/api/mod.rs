/// Helix API exposed to Lua plugins
///
/// This module contains all the Rust-Lua bridge code that exposes
/// Helix functionality to Lua plugins.
pub(crate) mod actions;
pub mod buffer;
pub mod completion;
pub mod diff;
pub mod editor;
pub mod explorer;
pub mod fs;
pub mod log;
pub mod lsp;
pub mod process;
pub mod quickfix;
pub mod statusline;
pub(crate) mod style;
pub mod treesitter;
pub mod ui;
pub mod window;

// Re-exports for convenience
pub use buffer::*;
pub use completion::register_completion_api;
pub use diff::register_diff_api;
pub use editor::*;
pub use explorer::*;
pub use fs::register_fs_api;
pub use log::*;
pub use lsp::*;
pub use process::register_process_api;
pub use quickfix::*;
pub use statusline::*;
pub use ui::*;
pub use window::*;
