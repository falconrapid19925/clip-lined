//! Headless demo: append scripted events + dump rows. CI-runnable on all OSes,
//! exercises store without GUI or clipboard hardware.

use clip_line_core::capture::{Block, ClipEvent, Format};
use clip_line_core::store::{Filter, Store, StoreOpts};

fn main() -> anyhow::Result<()> {
    let dir = std::env::temp_dir().join("clipline-headless-demo");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    let store = Store::open(&dir.join("demo.db"), StoreOpts::default())?;

    for text in ["first clip", "https://example.com", "hello world"] {
        let units: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        let bytes: Vec<u8> = units.iter().flat_map(|u| u.to_le_bytes()).collect();
        store.append(&ClipEvent {
            blocks: vec![Block {
                format: Format::UnicodeText,
                bytes,
            }],
            source_app: "headless".into(),
            source_title: String::new(),
        })?;
    }

    for r in store.list(&Filter::default())? {
        println!("{} | {} | {}", r.id, r.kind, r.title);
    }
    assert_eq!(store.list(&Filter::default())?.len(), 3, "expected 3 rows");
    Ok(())
}
