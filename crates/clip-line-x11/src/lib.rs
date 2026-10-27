//! X11 clipboard capture driven by XFixes selection notifications.
#[cfg(target_os = "linux")]
pub use clip_line_linux_common as formats;
#[cfg(target_os = "linux")]
mod linux {
    use anyhow::{Context, Result, bail};
    use clip_line_core::capture::{Block, Capture, ClipEvent, Format};
    use clip_line_core::paste::{ClipData, PasteMode, Paster};
    use std::sync::{Mutex, OnceLock};
    use x11rb::connection::Connection;
    use x11rb::protocol::Event;
    use x11rb::protocol::xfixes::{ConnectionExt as _, SelectionEventMask};
    use x11rb::protocol::xproto::{
        AtomEnum, ConnectionExt as _, CreateWindowAux, KEY_PRESS_EVENT, KEY_RELEASE_EVENT,
        WindowClass,
    };
    use x11rb::protocol::xtest::ConnectionExt as _;
    use x11rb::rust_connection::RustConnection;
    use x11rb::{COPY_DEPTH_FROM_PARENT, CURRENT_TIME, NONE};

    // The X selection owner is a window ID. Hold this lock across clipboard writes and
    // owner lookup so the capture thread cannot process our selection before it is armed.
    static OUR_OWNER: Mutex<u32> = Mutex::new(0);
    // X11 clipboard data is served by the owner process. Retain the owner
    // across Paster instances so clipboard contents survive a single paste call.
    static PASTE_CLIPBOARD: OnceLock<Mutex<arboard::Clipboard>> = OnceLock::new();

    pub struct X11Capture {
        conn: RustConnection,
        clipboard: arboard::Clipboard,
        selection: u32,
    }
    impl X11Capture {
        pub fn new() -> Result<Self> {
            let (conn, screen) = x11rb::connect(None).context("connecting to X11 display")?;
            conn.xfixes_query_version(5, 0)?
                .reply()
                .context("XFixes is unavailable on this X server")?;
            let root = conn.setup().roots[screen].root;
            let window = conn.generate_id()?;
            conn.create_window(
                COPY_DEPTH_FROM_PARENT,
                window,
                root,
                0,
                0,
                1,
                1,
                0,
                WindowClass::INPUT_OUTPUT,
                0,
                &CreateWindowAux::new(),
            )?
            .check()?;
            let selection = conn.intern_atom(false, b"CLIPBOARD")?.reply()?.atom;
            conn.xfixes_select_selection_input(
                window,
                selection,
                SelectionEventMask::SET_SELECTION_OWNER,
            )?
            .check()?;
            conn.flush()?;
            let clipboard = arboard::Clipboard::new().context("connecting to X11 clipboard")?;
            Ok(Self {
                conn,
                clipboard,
                selection,
            })
        }
    }
    impl Capture for X11Capture {
        fn next_event(&mut self) -> Result<ClipEvent> {
            loop {
                let Event::XfixesSelectionNotify(event) = self.conn.wait_for_event()? else {
                    continue;
                };
                let owner = OUR_OWNER
                    .lock()
                    .map_err(|_| anyhow::anyhow!("clipboard owner lock poisoned"))?;
                if event.owner == NONE
                    || event.owner == *owner
                    || self
                        .conn
                        .get_selection_owner(self.selection)?
                        .reply()?
                        .owner
                        != event.owner
                {
                    continue;
                }
                let mut blocks = Vec::new();
                if let Ok(image) = self.clipboard.get_image()
                    && let Ok(bytes) =
                        crate::formats::rgba_to_dib(image.width, image.height, &image.bytes)
                {
                    blocks.push(Block {
                        format: Format::ImageDib,
                        bytes,
                    });
                }
                if let Ok(paths) = self.clipboard.get().file_list()
                    && let Ok(bytes) = crate::formats::files_to_hdrop(&paths)
                {
                    blocks.push(Block {
                        format: Format::FileDrop,
                        bytes,
                    });
                }
                if let Ok(text) = self.clipboard.get_text() {
                    let mut bytes = Vec::with_capacity((text.len() + 1) * 2);
                    for u in text.encode_utf16().chain(std::iter::once(0)) {
                        bytes.extend_from_slice(&u.to_le_bytes());
                    }
                    blocks.push(Block {
                        format: Format::UnicodeText,
                        bytes,
                    });
                }
                if blocks.is_empty()
                    || self
                        .conn
                        .get_selection_owner(self.selection)?
                        .reply()?
                        .owner
                        != event.owner
                {
                    continue;
                }
                drop(owner);
                let app = self
                    .conn
                    .get_property(
                        false,
                        event.owner,
                        AtomEnum::WM_CLASS,
                        AtomEnum::STRING,
                        0,
                        256,
                    )
                    .ok()
                    .and_then(|r| r.reply().ok())
                    .map(|r| {
                        String::from_utf8_lossy(&r.value)
                            .split('\0')
                            .find(|s| !s.is_empty())
                            .unwrap_or("")
                            .to_owned()
                    })
                    .unwrap_or_default();
                return Ok(ClipEvent {
                    blocks,
                    source_app: app,
                    source_title: String::new(),
                });
            }
        }
    }
    pub struct X11Paster {
        conn: Mutex<RustConnection>,
    }
    impl X11Paster {
        pub fn new() -> Result<Self> {
            let (conn, _) = x11rb::connect(None).context("connecting to X11 display")?;
            if PASTE_CLIPBOARD.get().is_none() {
                let clipboard = arboard::Clipboard::new().context("connecting to X11 clipboard")?;
                let _ = PASTE_CLIPBOARD.set(Mutex::new(clipboard));
            }
            Ok(Self {
                conn: Mutex::new(conn),
            })
        }
    }
    impl Paster for X11Paster {
        fn paste(&self, data: &ClipData, mode: PasteMode) -> Result<()> {
            if mode == PasteMode::TypeIn {
                bail!("typing emulation is not yet supported on X11");
            }
            let mut owner = OUR_OWNER
                .lock()
                .map_err(|_| anyhow::anyhow!("clipboard owner lock poisoned"))?;
            let conn = self
                .conn
                .lock()
                .map_err(|_| anyhow::anyhow!("X11 connection lock poisoned"))?;
            let mut clipboard = PASTE_CLIPBOARD
                .get()
                .context("clipboard is not initialized")?
                .lock()
                .map_err(|_| anyhow::anyhow!("clipboard lock poisoned"))?;
            if let Some(block) = data
                .blocks
                .iter()
                .find(|b| matches!(b.format, Format::ImageDib | Format::ImageDibV5))
            {
                let (width, height, rgba) = crate::formats::dib_to_rgba(&block.bytes)?;
                clipboard.set_image(arboard::ImageData {
                    width,
                    height,
                    bytes: rgba.into(),
                })?;
            } else if let Some(block) = data.blocks.iter().find(|b| b.format == Format::FileDrop) {
                let paths = crate::formats::hdrop_to_files(&block.bytes)?;
                clipboard.set().file_list(&paths)?;
            } else if let Some(block) = data.blocks.iter().find(|b| b.format == Format::UnicodeText)
            {
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
                clipboard.set_text(
                    String::from_utf16(&units).context("invalid UTF-16 clipboard text")?,
                )?;
            } else {
                bail!("X11 clipboard does not support this clip format");
            }
            drop(clipboard);
            let selection = conn.intern_atom(false, b"CLIPBOARD")?.reply()?.atom;
            *owner = conn.get_selection_owner(selection)?.reply()?.owner;
            drop(owner);
            if mode == PasteMode::InjectCtrlV {
                let setup = conn.setup();
                let first = setup.min_keycode;
                let count = setup.max_keycode - first + 1;
                let mapping = conn.get_keyboard_mapping(first, count)?.reply()?;
                let per = mapping.keysyms_per_keycode as usize;
                if per == 0 {
                    bail!("X11 keyboard has no key mappings");
                }
                let keycode = |sym| {
                    mapping
                        .keysyms
                        .chunks(per)
                        .position(|keys| keys.contains(&sym))
                        .map(|i| first + i as u8)
                };
                let ctrl = keycode(0xffe3).context("Control key unavailable on X11 keyboard")?;
                let v = keycode(u32::from(b'v')).context("V key unavailable on X11 keyboard")?;
                let requests = [
                    (KEY_PRESS_EVENT, ctrl),
                    (KEY_PRESS_EVENT, v),
                    (KEY_RELEASE_EVENT, v),
                    (KEY_RELEASE_EVENT, ctrl),
                ]
                .into_iter()
                .map(|(kind, key)| conn.xtest_fake_input(kind, key, CURRENT_TIME, NONE, 0, 0, 0))
                .collect::<std::result::Result<Vec<_>, _>>()?;
                conn.flush()?;
                for request in requests {
                    request.check().context("XTest key injection failed")?;
                }
            }
            Ok(())
        }
    }
}
#[cfg(target_os = "linux")]
pub use linux::{X11Capture, X11Paster};
