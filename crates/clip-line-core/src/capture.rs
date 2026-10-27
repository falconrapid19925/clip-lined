//! Seam: OS clipboard listener. One trait, adapters per OS.

/// A clipboard change observed by a `Capture` adapter.
#[derive(Debug, Clone)]
pub struct ClipEvent {
    pub blocks: Vec<Block>,
    pub source_app: String,
    pub source_title: String,
}

/// One clipboard format's payload from a single event.
#[derive(Debug, Clone)]
pub struct Block {
    pub format: Format,
    pub bytes: Vec<u8>,
}

/// Formats clipLine understands. `Registered` carries the OS id and name.
#[derive(Debug, Clone, PartialEq)]
pub enum Format {
    UnicodeText,
    /// Device-independent bitmap, optionally with a PNG twin block.
    ImageDib,
    FileDrop,
    Registered(u32, String),
}

impl Format {
    /// Stable numeric id used inside the storage envelope.
    /// Standard clipboard ids keep their CF_ values; registered formats keep
    /// the id the OS assigned (stable per machine, matching the original app).
    pub fn envelope_id(&self) -> u32 {
        match self {
            Format::UnicodeText => 13, // CF_UNICODETEXT
            Format::ImageDib => 8,     // CF_DIB
            Format::FileDrop => 15,    // CF_HDROP
            Format::Registered(id, _) => *id,
        }
    }

    pub fn from_envelope_id(id: u32, name: Option<&str>) -> Format {
        match id {
            13 => Format::UnicodeText,
            8 => Format::ImageDib,
            15 => Format::FileDrop,
            other => Format::Registered(other, name.unwrap_or_default().to_string()),
        }
    }
}

/// Blocks until the OS reports a clipboard change.
/// Err only on fatal listener loss; adapters own retries.
pub trait Capture: Send {
    fn next_event(&mut self) -> anyhow::Result<ClipEvent>;
}
