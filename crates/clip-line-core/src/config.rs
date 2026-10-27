//! TOML config + one-time legacy `clipdiary.cfg` import (subset).

use anyhow::Context;
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub storage: Storage,
    pub paste: Paste,
    pub ui: Ui,
    pub privacy: Privacy,
    pub capture: CaptureRules,
}

/// Capture exclusions. A matching rule discards the entire clipboard event.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct CaptureRules {
    pub excluded_apps: Vec<String>,
    pub excluded_formats: Vec<String>,
    pub max_clip_mb: Option<u64>,
    pub max_image_mb: Option<u64>,
}

impl CaptureRules {
    pub fn excludes(&self, event: &crate::capture::ClipEvent) -> bool {
        use crate::capture::Format;
        if self
            .excluded_apps
            .iter()
            .any(|s| !s.is_empty() && s.eq_ignore_ascii_case(&event.source_app))
        {
            return true;
        }
        let total = event
            .blocks
            .iter()
            .fold(0u64, |n, b| n.saturating_add(b.bytes.len() as u64));
        if self
            .max_clip_mb
            .is_some_and(|max| total > max.saturating_mul(1024 * 1024))
        {
            return true;
        }
        event.blocks.iter().any(|block| {
            let name = match &block.format {
                Format::UnicodeText => "CF_UNICODETEXT",
                Format::ImageDib => "CF_DIB",
                Format::ImageDibV5 => "CF_DIBV5",
                Format::FileDrop => "CF_HDROP",
                Format::Registered(_, name) => name,
            };
            self.excluded_formats.iter().any(|s| {
                s.eq_ignore_ascii_case(name) || s == &block.format.envelope_id().to_string()
            }) || (matches!(block.format, Format::ImageDib | Format::ImageDibV5)
                && self
                    .max_image_mb
                    .is_some_and(|max| block.bytes.len() as u64 > max.saturating_mul(1024 * 1024)))
        })
    }
}

#[derive(Debug, Default, Deserialize)]
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
    for l in text.lines() {
        let l = l.trim();
        if let Some(v) = l
            .strip_prefix("HotKeysList")
            .and_then(|r| r.strip_prefix('='))
        {
            list = v.to_string();
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
