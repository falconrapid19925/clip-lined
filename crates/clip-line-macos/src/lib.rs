//! macOS clipboard adapter using NSPasteboard change counts and Quartz input.
//!
//! Capture polls the pasteboard at 250ms. Paste injection requires Accessibility
//! permission; SetOnly does not. This adapter handles text, files, and common 8-bit RGB images.

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::{MacCapture, MacPaster, activate_target, foreground_target};

mod dib;
pub use dib::dib_to_bmp;
