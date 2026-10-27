//! clipLine core: capture/store/paste seams shared by every OS adapter.
//! See SPEC.md — this file is the paper of record for interfaces.

pub mod capture;
pub mod config;
pub mod envelope;
pub mod paste;
pub mod store;

pub use capture::{Block, Capture, ClipEvent, Format};
pub use paste::{ClipData, PasteMode, Paster};
pub use store::{ClipRow, Filter, Store, StoreOpts};
