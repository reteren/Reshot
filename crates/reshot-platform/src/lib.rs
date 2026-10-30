//! Reshot's Windows integration outside capture and recording: the resident shell thread
//! (global hotkeys, tray icon), single instance, autostart, clipboard and window helpers.

pub mod autostart;
pub mod clipboard;
pub mod shell;
pub mod single_instance;
pub mod win;
