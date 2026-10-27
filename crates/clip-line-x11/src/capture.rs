//! XFixes selection-notify on CLIPBOARD; reads targets+data via the
//! convert-selection dance. Blocks land in envelope layout (text as
//! CF_UNICODETEXT-style UTF-16LE bytes).

use anyhow::{Context, Result};
use x11rb::atom_manager;
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xfixes::ConnectionExt as _;
use x11rb::protocol::xproto::{Atom, AtomEnum, ConnectionExt, GetPropertyReply, Window};
use x11rb::rust_connection::RustConnection;

atom_manager! {
    /// Interned atoms this adapter needs up front.
    pub Atoms: AtomCollectionCookie {
        CLIPBOARD,
        TARGETS,
        MULTIPLE,
        INCR,
        UTF8_STRING,
        _NET_ACTIVE_WINDOW,
        _NET_WM_NAME,
        WM_CLASS,
        WM_NAME,
    }
}

pub struct X11Capture {
    conn: RustConnection,
    win: Window,
    atoms: Atoms,
    root: Window,
}

impl X11Capture {
    pub fn new() -> Result<Self> {
        use x11rb::connection::RequestConnection;
        let (conn, screen_num) = RustConnection::connect(None).context("X11 connect failed")?;
        conn.extension_information("XFIXES")
            .context("query XFIXES extension failed")?
            .context("X server has no XFIXES; clipboard capture impossible")?;
        let screen = conn.setup().roots.get(screen_num).context("no X screen")?;
        let root = screen.root;
        let win = conn.generate_id()?;
        conn.create_window(
            x11rb::COPY_DEPTH_FROM_PARENT,
            win,
            screen.root,
            0,
            0,
            1,
            1,
            0,
            x11rb::protocol::xproto::WindowClass::INPUT_OUTPUT,
            x11rb::COPY_FROM_PARENT,
            &x11rb::protocol::xproto::CreateWindowAux::new(),
        )?
        .check()
        .context("create listener window failed")?;
        let atoms = Atoms::new(&conn)?.reply()?;
        let mask = x11rb::protocol::xfixes::SelectionEventMask::SET_SELECTION_OWNER
            | x11rb::protocol::xfixes::SelectionEventMask::SELECTION_WINDOW_DESTROY
            | x11rb::protocol::xfixes::SelectionEventMask::SELECTION_CLIENT_CLOSE;
        conn.xfixes_select_selection_input(win, atoms.CLIPBOARD, mask)?
            .check()?;
        let _ = conn.flush();
        Ok(X11Capture {
            conn,
            win,
            atoms,
            root,
        })
    }
}

impl Drop for X11Capture {
    fn drop(&mut self) {
        let _ = self.conn.destroy_window(self.win);
        let _ = self.conn.flush();
    }
}
impl clip_line_core::capture::Capture for X11Capture {
    fn next_event(&mut self) -> Result<clip_line_core::capture::ClipEvent> {
        loop {
            let ev = self.conn.wait_for_event().context("X connection lost")?;
            let Event::XfixesSelectionNotify(se) = ev else {
                continue;
            };
            if se.subtype != x11rb::protocol::xfixes::SelectionEvent::SET_SELECTION_OWNER {
                continue; // destroy/close = clipboard emptied: no clip to record
            }
            // ponytail: cross-process self-suppression needs a name lookup on
            // the owner window; this covers only our own daemon (v1).
            if se.owner == x11rb::NONE || se.owner == crate::paste::owner_window() {
                continue;
            }
            let ev = self.read_selection()?;
            if !ev.blocks.is_empty() {
                return Ok(ev);
            }
        }
    }
}

impl X11Capture {
    /// Convert-selection read of the current CLIPBOARD owner: TARGETS first,
    /// then one conversion per target. Empty when nothing decodable.
    fn read_selection(&self) -> Result<clip_line_core::capture::ClipEvent> {
        let (source_app, source_title) = self.active_window_metadata();
        let mut ev = clip_line_core::capture::ClipEvent {
            blocks: Vec::new(),
            source_app,
            source_title,
        };
        let Some(targets) = self.targets() else {
            return Ok(ev);
        };
        let skip = [self.atoms.TARGETS, self.atoms.MULTIPLE, self.atoms.INCR];
        let mut blocks = Vec::new();
        for t in targets {
            if skip.contains(&t) {
                continue;
            }
            if let Some(bytes) = self.convert(t) {
                if let Some(b) = block_from_atom_name(&self.conn, t, bytes) {
                    blocks.push(b);
                }
            }
        }
        ev.blocks = blocks;
        Ok(ev)
    }

    /// The active top-level window is a best-effort source; X11 selection
    /// ownership alone can belong to a hidden clipboard helper window.
    fn active_window_metadata(&self) -> (String, String) {
        let active = self
            .conn
            .get_property(
                false,
                self.root,
                self.atoms._NET_ACTIVE_WINDOW,
                AtomEnum::WINDOW,
                0,
                1,
            )
            .ok()
            .and_then(|c| c.reply().ok())
            .and_then(|r| r.value32().and_then(|mut values| values.next()));
        let Some(window) = active.filter(|w| *w != x11rb::NONE) else {
            return (String::new(), String::new());
        };
        let read = |atom| {
            self.conn
                .get_property(false, window, atom, AtomEnum::ANY, 0, 4096)
                .ok()
                .and_then(|c| c.reply().ok())
                .map(|r| r.value)
                .unwrap_or_default()
        };
        let class = read(self.atoms.WM_CLASS);
        // WM_CLASS is instance NUL class NUL. Prefer the human-readable class.
        let mut fields = class.split(|b| *b == 0).filter(|s| !s.is_empty());
        let instance = fields.next().unwrap_or_default();
        let app = fields.next().unwrap_or(instance);
        let mut title = read(self.atoms._NET_WM_NAME);
        if title.is_empty() {
            title = read(self.atoms.WM_NAME);
        }
        (
            String::from_utf8_lossy(app).into_owned(),
            String::from_utf8_lossy(&title)
                .trim_end_matches('\0')
                .to_owned(),
        )
    }

    /// TARGETS list of the current selection, None when unobtainable.
    fn targets(&self) -> Option<Vec<Atom>> {
        let prop = self.atoms.TARGETS; // reuse atom as scratch property name
        self.conn
            .convert_selection(
                self.win,
                self.atoms.CLIPBOARD,
                self.atoms.TARGETS,
                prop,
                x11rb::CURRENT_TIME,
            )
            .ok()?;
        let reply = self.wait_property(prop)?;
        let atoms: Vec<Atom> = reply
            .value
            .chunks_exact(4)
            .filter(|c| c != &[0, 0, 0, 0])
            .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
            .collect();
        let _ = self.conn.delete_property(self.win, prop);
        Some(atoms)
    }

    /// One target conversion; None on refusal or INCR (huge payload).
    fn convert(&self, target: Atom) -> Option<Vec<u8>> {
        let prop = self.atoms.UTF8_STRING; // scratch property name
        self.conn
            .convert_selection(
                self.win,
                self.atoms.CLIPBOARD,
                target,
                prop,
                x11rb::CURRENT_TIME,
            )
            .ok()?;
        let reply = self.wait_property(prop)?;
        let bytes = reply.value;
        let _ = self.conn.delete_property(self.win, prop);
        Some(bytes)
    }

    /// ConvertSelection -> SelectionNotify -> GetProperty for one scratch prop.
    /// None when the owner refused (property NONE) or offered INCR.
    fn wait_property(&self, prop: Atom) -> Option<GetPropertyReply> {
        loop {
            let ev = self.conn.wait_for_event().ok()?;
            let Event::SelectionNotify(sn) = ev else {
                continue;
            };
            if sn.requestor != self.win || sn.selection != self.atoms.CLIPBOARD {
                continue;
            }
            if sn.property == x11rb::NONE {
                return None;
            }
            let reply = self
                .conn
                .get_property(false, self.win, prop, AtomEnum::ANY, 0, 1 << 20)
                .ok()?
                .reply()
                .ok()?;
            if reply.type_ == self.atoms.INCR {
                // ponytail: INCR needs a chunked read loop; huge clips skip
                // capture until phase 3 adds it.
                return None;
            }
            return Some(reply);
        }
    }
}
/// Map a target atom's payload into a Block.
fn block_from_atom_name(
    conn: &RustConnection,
    target: Atom,
    bytes: Vec<u8>,
) -> Option<clip_line_core::capture::Block> {
    let reply = conn.get_atom_name(target).ok()?.reply().ok()?;
    let name = String::from_utf8_lossy(&reply.name).into_owned();
    Some(clip_line_core::capture::Block {
        format: map_target(&name),
        bytes: payload_for(&name, bytes),
    })
}

/// Text atoms share the UnicodeText id; everything else is registered by name.
pub(crate) fn map_target(name: &str) -> clip_line_core::capture::Format {
    use clip_line_core::capture::Format;
    if is_text_atom(name) {
        Format::UnicodeText
    } else {
        Format::Registered(registered_id(name), name.to_string())
    }
}

/// Atoms whose payload is character data (UTF-8, or Latin-1 for STRING).
pub(crate) fn is_text_atom(name: &str) -> bool {
    matches!(
        name,
        "UTF8_STRING"
            | "TEXT"
            | "STRING"
            | "COMPOUND_TEXT"
            | "text/plain"
            | "text/plain;charset=utf-8"
    )
}

/// Stable envelope id from the atom name: FNV-1a forced into the high range
/// so it can never collide with the CF_ ids (8/13/15/17) or raw small atoms.
/// 0 only when the name is empty.
pub(crate) fn registered_id(name: &str) -> u32 {
    if name.is_empty() {
        return 0;
    }
    let mut h: u32 = 0x811c_9dc5;
    for b in name.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    h | 0x8000_0000
}

/// Payload bytes for a target: text becomes UTF-16LE (the CF_UNICODETEXT
/// layout the storage envelope expects); everything else passes through raw.
pub(crate) fn payload_for(name: &str, bytes: Vec<u8>) -> Vec<u8> {
    if is_text_atom(name) {
        decode_text(name, &bytes)
            .encode_utf16()
            .flat_map(|u| u.to_le_bytes())
            .collect()
    } else {
        bytes
    }
}

/// Decode a text atom's bytes: STRING/COMPOUND_TEXT are Latin-1, rest UTF-8.
fn decode_text(name: &str, bytes: &[u8]) -> String {
    if name == "STRING" || name == "COMPOUND_TEXT" {
        bytes.iter().map(|&b| b as char).collect()
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::{decode_text, is_text_atom, map_target, payload_for, registered_id};
    use clip_line_core::capture::Format;

    #[test]
    fn text_atoms_map_to_unicode_text() {
        for a in [
            "UTF8_STRING",
            "TEXT",
            "STRING",
            "COMPOUND_TEXT",
            "text/plain",
        ] {
            assert!(is_text_atom(a), "{a}");
            assert_eq!(map_target(a), Format::UnicodeText);
        }
    }

    #[test]
    fn image_png_maps_registered_by_name() {
        match map_target("image/png") {
            Format::Registered(id, name) => {
                assert_eq!(name, "image/png");
                assert_eq!(id, registered_id("image/png"));
                assert_ne!(id, 0);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn registered_ids_avoid_cf_range() {
        for name in ["image/png", "image/bmp", "application/x-qt-image"] {
            let id = registered_id(name);
            assert!(id >= 0x8000_0000, "{name}: {id:#x}");
        }
        assert_eq!(registered_id(""), 0);
    }

    #[test]
    fn utf8_text_becomes_utf16le_payload() {
        let bytes = payload_for("UTF8_STRING", b"h\xC3\xA9llo".to_vec());
        let want: Vec<u8> = "h\u{e9}llo"
            .encode_utf16()
            .flat_map(|u| u.to_le_bytes())
            .collect();
        assert_eq!(bytes, want);
    }

    #[test]
    fn string_atom_is_latin1() {
        assert_eq!(decode_text("STRING", &[0x68, 0xE9]), "h\u{e9}");
    }

    #[test]
    fn non_text_payload_passes_through() {
        let raw = vec![1u8, 2, 3];
        assert_eq!(payload_for("image/png", raw.clone()), raw);
    }
}
