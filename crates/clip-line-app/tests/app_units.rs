//! Headless checks for the pure helpers in main.rs (task item 10; GUI is manual).
//! `main.rs` is a bin, so integration tests re-include it — pure fns only.

// main.rs items unused from the test harness view (main() never runs here)
#[path = "../src/main.rs"]
#[allow(dead_code)]
mod main;

use clip_line_core::paste::PasteMode;
use main::{date_range_strings, parse_hotkey, parse_mode, t};

#[test]
fn hotkey_strings_parse_to_mod_bits_and_vkey() {
    // Ctrl=2 Shift=4 Alt=1 Win=8 (bit layout shared with legacy import).
    assert_eq!(parse_hotkey("Ctrl+Shift+V"), Some((2 | 4, b'V' as u32)), "Ctrl+Shift+V");
    assert_eq!(parse_hotkey("ctrl+shift+v"), Some((6, b'V' as u32)), "case-insensitive");
    assert_eq!(parse_hotkey("Alt+F2"), Some((1, 0x71)), "Alt+F2 (VK_F2=0x71)");
    assert_eq!(parse_hotkey("Ctrl+0"), Some((2, b'0' as u32)), "digit key");
    assert_eq!(parse_hotkey("Win+L"), Some((8, b'L' as u32)), "Win alone = 8");
    assert_eq!(parse_hotkey("F24"), Some((0, 0x87)), "bare function key, no mods");
}

#[test]
fn bad_hotkeys_rejected() {
    assert_eq!(parse_hotkey("Ctrl+Shift"), None, "no key");
    assert_eq!(parse_hotkey("Ctrl+V+X"), None, "key must be last");
    assert_eq!(parse_hotkey(""), None, "empty");
    assert_eq!(parse_hotkey("Ctrl+Percent"), None, "unmapped key");
    assert_eq!(parse_hotkey("Ctrl++"), None, "empty segment");
    assert_eq!(parse_hotkey("Ctrl+F25"), None, "out of VK range");
}

#[test]
fn parse_hotkey_round_trips_legacy_decode() {
    // Legacy import decodes bits→display; verify bitfield decodes back (spec "Open").
    use clip_line_core::config::parse_hotkeys_list;
    let list = parse_hotkeys_list("1,6,86;2,1,113;3,2,117;");
    assert_eq!(list.len(), 3);
    for hk in &list {
        let (m, v) = parse_hotkey(&hk.display).expect("re-parseable");
        assert_eq!(m, hk.modifier, "modifier bits round-trip");
        assert_eq!(v, hk.vkey, "vkey round-trip");
    }
}

#[test]
fn t_is_identity() {
    assert_eq!(t("Paste"), "Paste");
}

#[test]
fn mode_strings_map_to_paste_mode() {
    assert_eq!(parse_mode("inject_ctrl_v"), PasteMode::InjectCtrlV);
    assert_eq!(parse_mode("set_only"), PasteMode::SetOnly);
    assert_eq!(parse_mode("type_in"), PasteMode::TypeIn);
    assert_eq!(parse_mode("nonsense"), PasteMode::InjectCtrlV, "fallback");
}

#[test]
fn date_range_builds_sqlite_bounds() {
    let (f, t) = date_range_strings("2026-10-01", "2026-10-27");
    assert_eq!(f.as_deref(), Some("2026-10-01 00:00:00"));
    assert_eq!(t.as_deref(), Some("2026-10-27 23:59:59"));
    let (f, t) = date_range_strings("", " 2026-1-1 ");
    assert_eq!(f, None, "empty from");
    assert_eq!(t, None, "malformed to ignored");
    // Shape-only validation: digits+dashes pass (SQLite string compare decides the rest).
    let (f, t) = date_range_strings("2026-13-40", "not-a-date!");
    assert_eq!(f.as_deref(), Some("2026-13-40 00:00:00"));
    assert_eq!(t, None);
}
