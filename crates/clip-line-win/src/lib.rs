//! Windows adapters behind cfg(windows); empty stubs elsewhere so the
//! workspace always compiles.

#[cfg(windows)]
pub mod capture;
#[cfg(windows)]
pub mod paste;
#[cfg(windows)]
pub use capture::WinCapture;
#[cfg(windows)]
pub use paste::WinPaster;
