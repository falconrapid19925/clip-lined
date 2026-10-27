//! TOML config + one-time legacy `clipdiary.cfg` import (subset).

use crate::capture::{ClipEvent, Format};
use anyhow::Context;
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub storage: Storage,
    pub paste: Paste,
    pub ui: Ui,
    pub capture: CaptureRules,
    pub privacy: Privacy,
}

/// Exclusions are applied to a whole clipboard event before it reaches storage.
/// App and format names are matched case-insensitively; an empty list disables
/// the corresponding rule. `max_bytes` limits the sum of all format payloads.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct CaptureRules {
    pub exclude_apps: Vec<String>,
    pub exclude_formats: Vec<String>,
    pub max_bytes: Option<u64>,
}

impl CaptureRules {
    pub fn allows(&self, event: &ClipEvent) -> bool {
        // An exclusion cannot protect the user when the platform cannot
        // identify the writer. Do not persist that event in this case.
        if event.source_app.trim().is_empty()
            && self.exclude_apps.iter().any(|app| !app.trim().is_empty())
        {
            return false;
        }
        if self.exclude_apps.iter().any(|app| {
            let app = app.trim();
            let source = event.source_app.trim();
            !app.is_empty()
                && (app.eq_ignore_ascii_case(source)
                    || without_exe(app).eq_ignore_ascii_case(without_exe(source)))
        }) {
            return false;
        }
        if event.blocks.iter().any(|block| {
            self.exclude_formats.iter().any(|format| {
                let format = format.trim();
                if format.is_empty() {
                    return false;
                }
                let name = match &block.format {
                    Format::UnicodeText => "text",
                    Format::ImageDib => "dib",
                    Format::ImageDibV5 => "dibv5",
                    Format::FileDrop => "files",
                    Format::Registered(_, name) => name.as_str(),
                };
                format.eq_ignore_ascii_case(name)
                    || format == block.format.envelope_id().to_string()
            })
        }) {
            return false;
        }
        if let Some(max) = self.max_bytes {
            let size = event.blocks.iter().try_fold(0u64, |total, block| {
                u64::try_from(block.bytes.len())
                    .ok()
                    .and_then(|n| total.checked_add(n))
            });
            if size.is_none_or(|size| size > max) {
                return false;
            }
        }
        true
    }
}

/// Windows adapters may report an executable basename without its extension.
fn without_exe(name: &str) -> &str {
    match name.len().checked_sub(4) {
        Some(index)
            if name
                .get(index..)
                .is_some_and(|s| s.eq_ignore_ascii_case(".exe")) =>
        {
            name.get(..index).unwrap_or(name)
        }
        _ => name,
    }
}

/// Privacy options are opt-in. Secure deletion is subject to SQLite, WAL,
/// filesystem, and storage device limitations.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Privacy {
    pub clear_on_exit: bool,
    pub secure_delete: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Storage {
    pub path: Option<std::path::PathBuf>,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Paste {
    pub default_mode: String,
}

impl Default for Paste {
    fn default() -> Self {
        Self {
            default_mode: "set_only".into(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Ui {
    pub hide_after_ms: u64,
}

impl Default for Ui {
    fn default() -> Self {
        Self {
            hide_after_ms: 1200,
        }
    }
}

/// One imported legacy hotkey slot: 13 slots in `HotKeysList`, format
/// `id,modifier,vkey;` per FINDINGS.md. Modifier is a bitfield:
/// Ctrl=2, Shift=4, Alt=1, Win=8 (FINDINGS.md, unverified upstream).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LegacyHotkey {
    pub slot: u32,
    /// Human form, e.g. "Ctrl+Shift+V".
    pub display: String,
    pub modifier: u32,
    pub vkey: u32,
}

/// Ctrl=2, Shift=4, Alt=1, Win=8 → "Ctrl+Shift+" (trailing '+').
fn decode_modifier(bits: u32) -> Option<String> {
    let mut s = String::new();
    if bits & 1 != 0 {
        s.push_str("Alt+");
    }
    if bits & 2 != 0 {
        s.push_str("Ctrl+");
    }
    if bits & 4 != 0 {
        s.push_str("Shift+");
    }
    if bits & 8 != 0 {
        s.push_str("Win+");
    }
    if bits & !0xF != 0 {
        return None; // unknown bits → caller falls back
    }
    Some(s)
}

/// Virtual-key name for the printable/common range; None elsewhere (caller falls back).
fn key_name(vkey: u32) -> Option<String> {
    match vkey {
        0x30..=0x39 => Some((vkey as u8 as char).to_string()), // 0-9
        0x41..=0x5A => Some((vkey as u8 as char).to_string()), // A-Z
        0x70..=0x87 => Some(format!("F{}", vkey - 0x6F)),      // F1-F24
        _ => None,
    }
}

/// Parses `HotKeysList=1,6,86;2,2,162;` — semicolon-separated triplets.
/// Unparseable slots are skipped (spec: fall back to defaults on parse failure).
pub fn parse_hotkeys_list(list: &str) -> Vec<LegacyHotkey> {
    let mut out = Vec::new();
    for entry in list.split(';') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let mut it = entry.split(',');
        let (Ok(slot), Ok(modifier), Ok(vkey), None) = (
            it.next().map(str::trim).unwrap_or("").parse::<u32>(),
            it.next().map(str::trim).unwrap_or("").parse::<u32>(),
            it.next().map(str::trim).unwrap_or("").parse::<u32>(),
            it.next(),
        ) else {
            continue; // malformed triplet → skip slot
        };
        let (Some(mods), Some(key)) = (decode_modifier(modifier), key_name(vkey)) else {
            continue;
        };
        out.push(LegacyHotkey {
            slot,
            display: format!("{mods}{key}"),
            modifier,
            vkey,
        });
    }
    out
}

/// Reads a legacy `clipdiary.cfg` `[Options]` subset. Only hotkeys import today.
pub fn import_legacy_ini(path: &Path) -> anyhow::Result<Vec<LegacyHotkey>> {
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let mut list = String::new();
    let mut in_options = false;
    for l in text.lines() {
        let l = l.trim();
        if l.starts_with('[') && l.ends_with(']') {
            in_options = l[1..l.len() - 1].trim().eq_ignore_ascii_case("options");
            continue;
        }
        if let Some((_, value)) = l
            .split_once('=')
            .filter(|(key, _)| in_options && key.trim().eq_ignore_ascii_case("HotKeysList"))
        {
            list = value.trim().to_string();
        }
    }
    Ok(parse_hotkeys_list(&list))
}

impl Config {
    pub fn load(path: &Path) -> anyhow::Result<Config> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        Ok(toml::from_str(&text)?)
    }
}
