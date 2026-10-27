//! Seam: OS clipboard write + optional input injection.

use crate::capture::Block;

#[derive(Debug, Clone)]
pub struct ClipData {
    pub blocks: Vec<Block>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PasteMode {
    /// Put clip on clipboard only; user pastes manually.
    SetOnly,
    /// Set clipboard, then inject Ctrl+V (Cmd+V on macOS) into the focused window.
    InjectCtrlV,
    /// Type text keystroke by keystroke. Text clips only.
    TypeIn,
}

pub trait Paster: Send {
    fn paste(&self, data: &ClipData, mode: PasteMode) -> anyhow::Result<()>;
}
