//! Reshot core: everything that knows nothing about windows. Document model, undo, tools,
//! settings, hotkey parsing, the session state machine and the logger.

pub mod document;
pub mod filename;
pub mod fonts;
pub mod history;
pub mod hotkey;
pub mod log;
pub mod paths;
pub mod session;
pub mod settings;
pub mod tools;

pub use skia_safe as skia;
