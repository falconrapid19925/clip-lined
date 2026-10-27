//! Clipboard capture using the compositor's ext-data-control or wlr-data-control protocol.
//! Wayland deliberately does not permit applications to inject keys into other clients.

#[cfg(target_os = "linux")]
mod linux {
    use anyhow::{Context, Result, bail};
    use clip_line_core::capture::{Block, Capture, ClipEvent, Format};
    use clip_line_core::paste::{ClipData, PasteMode, Paster};
    use std::io::Read;
    use std::sync::mpsc::{self, Receiver};
    use wl_clipboard_rs::copy::{MimeSource, MimeType, Options, Source};
    use wl_clipboard_rs::paste::Seat;
    use wl_clipboard_rs::watch::{CancelHandle, ClipboardEvent, ClipboardType, Watcher};

    const ORIGIN: &str = "application/x-clipline-origin";
    const MAX_TEXT: u64 = 16 * 1024 * 1024;

    /// The watcher lives on its own thread and blocks on compositor events, never polling.
    pub struct WaylandCapture {
        events: Receiver<Result<ClipEvent>>,
        cancel: CancelHandle,
    }

    impl WaylandCapture {
        pub fn new() -> Result<Self> {
            let (ready_tx, ready_rx) = mpsc::sync_channel(1);
            let (event_tx, events) = mpsc::channel();
            std::thread::Builder::new()
                .name("clipline-wayland".into())
                .spawn(move || {
                    let mut watcher = match Watcher::new(ClipboardType::Regular, Seat::Unspecified)
                    {
                        Ok(w) => {
                            let _ = ready_tx.send(Ok(w.cancel_handle()));
                            w
                        }
                        Err(e) => {
                            let message = match e {
                                wl_clipboard_rs::paste::Error::MissingProtocol { .. } => format!("Wayland clipboard capture requires ext-data-control or wlr-data-control (GNOME 48+, KDE or wlroots): {e}"),
                                _ => format!("could not initialize Wayland clipboard capture: {e}"),
                            };
                            let _ = ready_tx.send(Err(message));
                            return;
                        }
                    };
                    loop {
                        match watcher.next_event() {
                            Ok(Some(ClipboardEvent::Changed {
                                mime_types,
                                mut offer,
                                ..
                            })) => {
                                if mime_types.iter().any(|m| m == ORIGIN) {
                                    continue;
                                }
                                let mut read_offer = |mime: &str, max: u64| -> Result<Vec<u8>> {
                                    let mut bytes = Vec::new();
                                    offer.receive(mime)?.take(max + 1).read_to_end(&mut bytes)?;
                                    if bytes.len() as u64 > max {
                                        bail!("clipboard offer is too large");
                                    }
                                    Ok(bytes)
                                };
                                let mut blocks = Vec::new();
                                if mime_types.iter().any(|m| m == "image/png")
                                    && let Ok(bytes) = read_offer("image/png", 64 * 1024 * 1024).and_then(|b| clip_line_linux_common::png_to_dib(&b))
                                { blocks.push(Block { format: Format::ImageDib, bytes }); }
                                if mime_types.iter().any(|m| m == "text/uri-list")
                                    && let Ok(bytes) = read_offer("text/uri-list", MAX_TEXT).and_then(|b| clip_line_linux_common::uri_list_to_files(&b)).and_then(|paths| clip_line_linux_common::files_to_hdrop(&paths))
                                { blocks.push(Block { format: Format::FileDrop, bytes }); }
                                let text_mime = mime_types
                                    .iter()
                                    .find(|m| m.as_str() == "text/plain;charset=utf-8")
                                    .or_else(|| {
                                        mime_types.iter().find(|m| m.as_str() == "text/plain")
                                    })
                                    .or_else(|| {
                                        mime_types.iter().find(|m| m.as_str() == "UTF8_STRING")
                                    });
                                if let Some(mime) = text_mime
                                    && let Ok(bytes) = read_offer(mime, MAX_TEXT)
                                    && let Ok(s) = std::str::from_utf8(&bytes)
                                {
                                    let mut data = Vec::with_capacity((s.len()+1)*2);
                                    for unit in s.encode_utf16().chain(std::iter::once(0)) { data.extend_from_slice(&unit.to_le_bytes()); }
                                    blocks.push(Block { format: Format::UnicodeText, bytes: data });
                                }
                                if !blocks.is_empty()
                                    && event_tx
                                        .send(Ok(ClipEvent {
                                            blocks,
                                            source_app: String::new(),
                                            source_title: String::new(),
                                        }))
                                        .is_err()
                                {
                                    return;
                                }
                            }
                            Ok(Some(ClipboardEvent::Cleared { .. })) => {}
                            Ok(None) => {
                                let _ = event_tx.send(Err(anyhow::anyhow!(
                                    "Wayland clipboard watcher stopped"
                                )));
                                return;
                            }
                            Err(e) => {
                                let _ = event_tx.send(Err(e.into()));
                                return;
                            }
                        }
                    }
                })?;
            match ready_rx.recv().context("Wayland watcher failed to start")? {
                Ok(cancel) => Ok(Self { events, cancel }),
                Err(e) => bail!("{e}"),
            }
        }
    }
    impl Drop for WaylandCapture {
        fn drop(&mut self) {
            self.cancel.cancel();
        }
    }
    impl Capture for WaylandCapture {
        fn next_event(&mut self) -> Result<ClipEvent> {
            self.events.recv().context("Wayland watcher exited")?
        }
    }

    /// Offers clipboard data to the compositor. The receiving application must paste manually.
    pub struct WaylandPaster;
    impl WaylandPaster {
        pub fn new() -> Result<Self> {
            Ok(Self)
        }
    }
    impl Paster for WaylandPaster {
        fn paste(&self, data: &ClipData, mode: PasteMode) -> Result<()> {
            if mode != PasteMode::SetOnly {
                bail!(
                    "Wayland does not support simulated paste; use clipboard-only mode and paste manually"
                );
            }
            let mut sources = Vec::new();
            if let Some(block) = data
                .blocks
                .iter()
                .find(|b| matches!(b.format, Format::ImageDib | Format::ImageDibV5))
            {
                sources.push(MimeSource {
                    source: Source::Bytes(clip_line_linux_common::dib_to_png(&block.bytes)?.into()),
                    mime_type: MimeType::Specific("image/png".into()),
                });
            }
            if let Some(block) = data.blocks.iter().find(|b| b.format == Format::FileDrop) {
                let paths = clip_line_linux_common::hdrop_to_files(&block.bytes)?;
                sources.push(MimeSource {
                    source: Source::Bytes(
                        clip_line_linux_common::files_to_uri_list(&paths)?.into(),
                    ),
                    mime_type: MimeType::Specific("text/uri-list".into()),
                });
            }
            if let Some(block) = data.blocks.iter().find(|b| b.format == Format::UnicodeText) {
                if !block.bytes.len().is_multiple_of(2) {
                    bail!("invalid UTF-16 clipboard text");
                }
                let units: Vec<u16> = block
                    .bytes
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|b| u16::from_le_bytes(*b))
                    .take_while(|&u| u != 0)
                    .collect();
                let text = String::from_utf16(&units).context("invalid UTF-16 clipboard text")?;
                sources.push(MimeSource {
                    source: Source::Bytes(text.into_bytes().into()),
                    mime_type: MimeType::Text,
                });
            }
            if sources.is_empty() {
                bail!("Wayland clipboard does not support this clip format");
            }
            sources.push(MimeSource {
                source: Source::Bytes(Box::new([])),
                mime_type: MimeType::Specific(ORIGIN.into()),
            });
            Options::new()
                .copy_multi(sources)
                .context("setting Wayland clipboard")?;
            Ok(())
        }
    }
}
#[cfg(target_os = "linux")]
pub use linux::{WaylandCapture, WaylandPaster};
