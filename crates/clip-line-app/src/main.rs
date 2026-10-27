//! clipLine desktop application and platform adapter integration.

use clip_line_core::config::{CaptureRules, Config};
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod autostart;
use clip_line_core::paste::{ClipData, PasteMode};
use clip_line_core::store::{ClipRow, Filter, Snippet, Store, StoreOpts};
use eframe::egui;
use egui::{Context, Key, ViewportCommand};
use egui_extras::{Column, TableBuilder};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
mod updates;
rust_i18n::i18n!("locales", fallback = "en");
use std::time::{Duration, Instant};

/// Translate a UI label, falling back to English.
pub fn t(s: &str) -> String {
    rust_i18n::t!(s).to_string()
}

const DEFAULT_TOML: &str = r#"# clipLine configuration. Regenerated if deleted.
[hotkeys]
show = "Ctrl+Shift+Up"      # global show/hide toggle

[paste]
default_mode = "inject_ctrl_v"   # inject_ctrl_v | set_only | type_in

[ui]
hide_after_ms = 1200

[autostart]
enabled = false

[updates]
enabled = false

# [storage]
# path = "C:\path\to\clipline.db"
"#;

#[derive(Debug, serde::Deserialize)]
#[serde(default)]
struct Hotkeys {
    show: String,
}

impl Default for Hotkeys {
    fn default() -> Self {
        Self {
            show: "Ctrl+Shift+Up".into(),
        }
    }
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
struct Autostart {
    enabled: bool,
}

/// App-level config sections on top of `clip_line_core::config::Config`
/// (storage/paste/ui). Parsed from the same TOML text; core ignores unknown keys.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
struct Extras {
    hotkeys: Hotkeys,
    autostart: Autostart,
    updates: UpdateOptions,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
struct UpdateOptions {
    enabled: bool,
}

fn load_config(path: &Path) -> anyhow::Result<(Config, Extras)> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let _ = std::fs::write(path, DEFAULT_TOML); // first run: defaults were missing
            DEFAULT_TOML.to_string()
        }
        Err(e) => return Err(e.into()),
    };
    let extras = toml::from_str::<Extras>(&text)?;
    Ok((toml::from_str(&text)?, extras))
}

/// "Ctrl+Shift+V" → (modifier bits Alt=1|Ctrl=2|Shift=4|Win=8, virtual key).
/// Bit layout mirrors the legacy decode in `clip_line_core::config`.
pub fn parse_hotkey(s: &str) -> Option<(u32, u32)> {
    let mut mods = 0u32;
    let mut key: Option<u32> = None;
    for part in s.split('+') {
        let p = part.trim();
        if p.is_empty() {
            return None;
        }
        if key.is_some() {
            return None; // the key must come last
        }
        match p.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => mods |= 2,
            "shift" => mods |= 4,
            "alt" => mods |= 1,
            "win" | "super" | "meta" => mods |= 8,
            _ => {
                let k = p.to_ascii_uppercase();
                let v = if k.len() == 1 {
                    let c = k.as_bytes()[0];
                    match c {
                        b'A'..=b'Z' | b'0'..=b'9' => Some(c as u32),
                        _ => None,
                    }
                } else if let Some(f) = k.strip_prefix('F') {
                    f.parse::<u32>()
                        .ok()
                        .filter(|n| (1..=24).contains(n))
                        .map(|n| 0x6F + n) // VK_F1 = 0x70
                } else {
                    // Windows virtual-key codes for non-printable keys users bind.
                    match k.as_str() {
                        "UP" => Some(0x26),
                        "DOWN" => Some(0x28),
                        "LEFT" => Some(0x25),
                        "RIGHT" => Some(0x27),
                        "SPACE" => Some(0x20),
                        "INSERT" => Some(0x2D),
                        "DELETE" => Some(0x2E),
                        "HOME" => Some(0x24),
                        "END" => Some(0x23),
                        "PAGEUP" => Some(0x21),
                        "PAGEDOWN" => Some(0x22),
                        "ESC" | "ESCAPE" => Some(0x1B),
                        "TAB" => Some(0x09),
                        "ENTER" => Some(0x0D),
                        "BACKSPACE" => Some(0x08),
                        _ => None,
                    }
                };
                key = Some(v?);
            }
        }
    }
    key.map(|k| (mods, k))
}

/// Config `[paste] default_mode` string → enum; unknown values fall back to
/// InjectCtrlV (the app default: paste into the active window).
pub fn parse_mode(s: &str) -> PasteMode {
    match s.trim().to_ascii_lowercase().as_str() {
        "set_only" | "setonly" | "copy" => PasteMode::SetOnly,
        "type_in" | "typein" => PasteMode::TypeIn,
        _ => PasteMode::InjectCtrlV,
    }
}

/// Tray label for the current paste mode.
#[cfg(any(windows, target_os = "linux", target_os = "macos"))]
fn mode_str(m: PasteMode) -> &'static str {
    match m {
        PasteMode::SetOnly => "set only",
        PasteMode::InjectCtrlV => "inject Ctrl+V",
        PasteMode::TypeIn => "type in",
    }
}

#[cfg(any(windows, target_os = "linux", target_os = "macos"))]
fn tray_mode_label(mode: PasteMode) -> String {
    if cfg!(target_os = "linux") {
        "Paste mode: manual".into()
    } else {
        format!("Paste mode: {}", mode_str(mode))
    }
}

/// Tray mode item click cycles the paste mode.
#[cfg(any(windows, target_os = "linux", target_os = "macos"))]
fn next_mode(m: PasteMode) -> PasteMode {
    match m {
        PasteMode::InjectCtrlV => PasteMode::SetOnly,
        PasteMode::SetOnly => PasteMode::TypeIn,
        PasteMode::TypeIn => PasteMode::InjectCtrlV,
    }
}

/// "YYYY-MM-DD" text inputs → inclusive SQLite bounds for `Filter.from/to`.
/// Empty or malformed input → None (no bound).
pub fn date_range_strings(from: &str, to: &str) -> (Option<String>, Option<String>) {
    let one = |s: &str, suffix: &str| -> Option<String> {
        let s = s.trim();
        let ok = s.len() == 10
            && s.is_ascii()
            && s.as_bytes()[4] == b'-'
            && s.as_bytes()[7] == b'-'
            && s[..4].bytes().all(|b| b.is_ascii_digit())
            && s[5..7].bytes().all(|b| b.is_ascii_digit())
            && s[8..].bytes().all(|b| b.is_ascii_digit());
        ok.then(|| format!("{s}{suffix}"))
    };
    (one(from, " 00:00:00"), one(to, " 23:59:59"))
}

fn config_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        return std::env::var_os("HOME")
            .map(PathBuf::from)
            .map(|p| p.join("Library/Application Support/clip-line"));
    }
    #[cfg(not(target_os = "macos"))]
    std::env::var_os("APPDATA")
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .map(|p| p.join("clip-line"))
}

pub fn default_config_path() -> anyhow::Result<PathBuf> {
    let d = config_dir().ok_or_else(|| anyhow::anyhow!("no config dir"))?;
    std::fs::create_dir_all(&d)?;
    Ok(d.join("clipline.toml"))
}

pub fn default_db_path() -> anyhow::Result<PathBuf> {
    Ok(config_dir()
        .ok_or_else(|| anyhow::anyhow!("no config dir"))?
        .join("clipline.db"))
}

/// CLIP_LINE_DB env (file, or dir → dir/clipline.db) > `[storage] path` > default.
fn db_path(cfg_storage: Option<&Path>) -> anyhow::Result<PathBuf> {
    if let Some(p) = std::env::var_os("CLIP_LINE_DB").map(PathBuf::from) {
        if p.extension().is_some() {
            return Ok(p);
        }
        std::fs::create_dir_all(&p)?;
        return Ok(p.join("clipline.db"));
    }
    match cfg_storage {
        Some(p) => Ok(p.to_path_buf()),
        None => default_db_path(),
    }
}

/// Import legacy hotkey triplets once while preserving unrelated TOML options.
/// Decoded slots are retained without assigning actions: their legacy action
/// mapping is not documented. Unknown legacy settings are not interpreted.
pub fn persist_legacy_hotkeys(path: &Path, ini: &Path) -> anyhow::Result<usize> {
    let entries = clip_line_core::config::import_legacy_ini(ini)?;
    if entries.is_empty() {
        anyhow::bail!("no supported legacy hotkeys found in [Options]");
    }
    let original = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => DEFAULT_TOML.to_owned(),
        Err(e) => return Err(e.into()),
    };
    let mut doc: toml::Value = toml::from_str(&original)?;
    let table = doc
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("config root must be a table"))?;
    if table
        .get("legacy_import")
        .and_then(|v| v.get("complete"))
        .and_then(toml::Value::as_bool)
        == Some(true)
    {
        anyhow::bail!("legacy configuration was already imported");
    }
    let mut legacy = toml::map::Map::new();
    for hk in &entries {
        legacy.insert(
            format!("slot_{}", hk.slot),
            toml::Value::String(hk.display.clone()),
        );
    }
    table.insert("legacy_hotkeys".into(), toml::Value::Table(legacy));
    let mut marker = toml::map::Map::new();
    marker.insert("complete".into(), toml::Value::Boolean(true));
    table.insert("legacy_import".into(), toml::Value::Table(marker));
    let rendered = toml::to_string_pretty(&doc)?;
    if path.exists() {
        std::fs::copy(path, path.with_extension("toml.pre-import"))?;
    }
    std::fs::write(path, rendered)?;
    Ok(entries.len())
}

/// Parse CLI switches before opening or creating any database.
pub fn parse_args(
    args: impl IntoIterator<Item = std::ffi::OsString>,
) -> anyhow::Result<(Option<PathBuf>, Option<PathBuf>, bool)> {
    let mut args = args.into_iter();
    let mut import_ini = None;
    let mut import_db = None;
    let mut encrypted = false;
    while let Some(arg) = args.next() {
        if arg == "--encrypted" {
            if encrypted {
                anyhow::bail!("--encrypted specified more than once");
            }
            encrypted = true;
        } else if arg == "--import-ini" || arg == "--import-db" {
            let name = arg.to_string_lossy().to_string();
            let value = args
                .next()
                .ok_or_else(|| anyhow::anyhow!("{name} requires a path"))?;
            if value.to_string_lossy().starts_with("--") {
                anyhow::bail!("{name} requires a path");
            }
            let target = if arg == "--import-ini" {
                &mut import_ini
            } else {
                &mut import_db
            };
            if target.replace(PathBuf::from(value)).is_some() {
                anyhow::bail!("{name} specified more than once");
            }
        } else {
            anyhow::bail!("unknown argument: {}", arg.to_string_lossy());
        }
    }
    if import_ini.is_some() && (import_db.is_some() || encrypted) {
        anyhow::bail!("--import-ini cannot be combined with --import-db or --encrypted");
    }
    Ok((import_ini, import_db, encrypted))
}

fn main() -> anyhow::Result<()> {
    let (import_ini, import_db, encrypted) = parse_args(std::env::args_os().skip(1))?;
    if let Some(ini) = import_ini {
        let path = default_config_path()?;
        let count = persist_legacy_hotkeys(&path, &ini)?;
        println!(
            "Imported {count} legacy hotkey slots into {}. Their action mapping is unknown; choose hotkeys.show manually.",
            path.display()
        );
        return Ok(());
    }

    let cfg_path = default_config_path()?;
    let (cfg, extras) = load_config(&cfg_path)?;
    if parse_hotkey(&extras.hotkeys.show).is_none() {
        eprintln!(
            "config: hotkeys.show {:?} not understood; global hotkey disabled",
            extras.hotkeys.show
        );
    }

    let db = db_path(cfg.storage.path.as_deref())?;
    if let Some(source) = &import_db {
        if !source.is_file() {
            anyhow::bail!("legacy database does not exist: {}", source.display());
        }
        if db.exists() && std::fs::canonicalize(source)? == std::fs::canonicalize(&db)? {
            anyhow::bail!("legacy source and destination refer to the same database");
        }
    }
    let opts = if encrypted {
        #[cfg(feature = "encryption")]
        {
            let key = rpassword::prompt_password("Database password: ")?;
            if key.is_empty() {
                anyhow::bail!("database password cannot be empty");
            }
            if !db.exists() {
                let confirm = rpassword::prompt_password("Confirm new database password: ")?;
                if key != confirm {
                    anyhow::bail!("passwords do not match");
                }
            }
            StoreOpts { key: Some(key) }
        }
        #[cfg(not(feature = "encryption"))]
        {
            anyhow::bail!("this build lacks encryption support; rebuild with --features encryption")
        }
    } else {
        StoreOpts::default()
    };
    let store = Arc::new(Store::open(&db, opts)?);
    if let Some(source) = import_db {
        let count = store.import_legacy(&source)?;
        println!(
            "Imported {count} clips from {} into {}",
            source.display(),
            db.display()
        );
        return Ok(());
    }
    store.set_secure_delete(cfg.privacy.secure_delete)?;
    if let Err(e) = store.attach_snippets(&db.with_file_name("snippets.sdb")) {
        eprintln!("snippets disabled: {e}");
    }

    #[cfg(windows)]
    if let Err(e) = set_autostart(extras.autostart.enabled) {
        eprintln!("autostart: {e}");
    }
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    if let Err(e) = autostart::set_autostart(extras.autostart.enabled) {
        eprintln!("autostart: {e}");
    }

    let capture_active = Arc::new(Mutex::new(true));
    let (capture_status_tx, capture_status_rx) = std::sync::mpsc::channel();
    spawn_capture(
        store.clone(),
        cfg.capture.clone(),
        capture_active.clone(),
        capture_status_tx,
    )?;
    let update_result = if extras.updates.enabled {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(updates::check());
        });
        Some(rx)
    } else {
        None
    };

    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([860.0, 600.0])
            .with_min_inner_size([640.0, 400.0]),
        ..Default::default()
    };
    eframe::run_native(
        "clipLine",
        opts,
        Box::new(move |_cc| {
            let mut app = App::new(
                store,
                &cfg,
                capture_active,
                capture_status_rx,
                update_result,
            );
            // Both want the main thread (macOS tray rule); eframe's creator runs there.
            app.register_show_hotkey(&extras.hotkeys.show);
            // The channel receivers are polled in logic; installing a handler
            // would divert events away from them. Poll while hidden as well.
            #[cfg(any(windows, target_os = "linux", target_os = "macos"))]
            app.init_tray();
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))
}

/// Registry Run key write/remove per `[autostart] enabled` at startup.
#[cfg(windows)]
fn set_autostart(enabled: bool) -> anyhow::Result<()> {
    use windows::Win32::System::Registry::{
        HKEY_CURRENT_USER, REG_SZ, RegDeleteKeyValueW, RegSetKeyValueW,
    };
    let subkey: Vec<u16> = r"Software\Microsoft\Windows\CurrentVersion\Run"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let value: Vec<u16> = "clipLine\0".encode_utf16().collect();
    unsafe {
        if enabled {
            let exe = std::env::current_exe()?;
            let data: Vec<u16> = exe
                .to_string_lossy()
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            let r = RegSetKeyValueW(
                HKEY_CURRENT_USER,
                windows::core::PCWSTR(subkey.as_ptr()),
                windows::core::PCWSTR(value.as_ptr()),
                REG_SZ.0,
                Some(data.as_ptr().cast()),
                (data.len() * 2) as u32,
            );
            if r.0 != 0 {
                anyhow::bail!("RegSetKeyValueW error {}", r.0);
            }
        } else {
            let _ = RegDeleteKeyValueW(
                HKEY_CURRENT_USER,
                windows::core::PCWSTR(subkey.as_ptr()),
                windows::core::PCWSTR(value.as_ptr()),
            ); // missing value is fine
        }
    }
    Ok(())
}

fn spawn_capture(
    store: Arc<Store>,
    rules: CaptureRules,
    active: Arc<Mutex<bool>>,
    status: std::sync::mpsc::Sender<String>,
) -> anyhow::Result<()> {
    std::thread::Builder::new()
        .name("capture".into())
        .spawn(move || {
            use clip_line_core::capture::Capture;
            let cap: anyhow::Result<Box<dyn Capture>> = create_capture();
            let mut cap = match cap {
                Ok(c) => c,
                Err(e) => { let message = format!("capture disabled: {e:#}"); eprintln!("{message}"); let _ = status.send(message); return; }
            };
            let mut warned_unknown_app = false;
            loop {
                match cap.next_event() {
                    Ok(ev) if rules.allows(&ev) => {
                        let active = active.lock().unwrap();
                        if !*active { break; }
                        if let Err(e) = store.append(&ev) { eprintln!("store append: {e}"); }
                    }
                    Ok(ev) => {
                        if !warned_unknown_app && !rules.exclude_apps.is_empty() && ev.source_app.trim().is_empty() {
                            let message = "capture skipped: source application is unavailable while app exclusions are enabled".to_owned();
                            eprintln!("{message}");
                            let _ = status.send(message);
                            warned_unknown_app = true;
                        }
                    }
                    Err(e) => {
                        let message = format!("capture error: {e}");
                        eprintln!("{message}");
                        let _ = status.send(message);
                        std::thread::sleep(Duration::from_secs(1));
                    }
                }
            }
        })?;
    Ok(())
}

#[cfg(windows)]
fn create_capture() -> anyhow::Result<Box<dyn clip_line_core::capture::Capture>> {
    Ok(Box::new(clip_line_win::WinCapture::new()?))
}
#[cfg(target_os = "linux")]
fn create_capture() -> anyhow::Result<Box<dyn clip_line_core::capture::Capture>> {
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        Ok(Box::new(clip_line_wayland::WaylandCapture::new()?))
    } else {
        Ok(Box::new(clip_line_x11::X11Capture::new()?))
    }
}
#[cfg(target_os = "macos")]
fn create_capture() -> anyhow::Result<Box<dyn clip_line_core::capture::Capture>> {
    Ok(Box::new(clip_line_macos::MacCapture::new()?))
}
#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn create_capture() -> anyhow::Result<Box<dyn clip_line_core::capture::Capture>> {
    anyhow::bail!("clipboard capture is unsupported on this platform")
}

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    History,
    Snippets,
}

struct App {
    store: Arc<Store>,
    capture_active: Arc<Mutex<bool>>,
    clear_on_exit: bool,
    update_result: Option<std::sync::mpsc::Receiver<anyhow::Result<Option<String>>>>,
    capture_status: std::sync::mpsc::Receiver<String>,
    mode: PasteMode,
    pending_paste: Option<(
        Instant,
        Vec<clip_line_core::capture::Block>,
        PasteMode,
        bool,
    )>,
    tab: Tab,
    query: String,
    search_focus: bool,
    /// Focus+select the search box on next frame (hotkey/tray "show" request).
    select_after_show: bool,
    starred_only: bool,
    kind_sel: usize,
    date_from: String,
    date_to: String,
    rows: Vec<ClipRow>,
    selected: Option<usize>,
    dirty: bool,
    last_poll: Instant,
    status: String,
    snips: Vec<Snippet>,
    snip_path: Vec<(i64, String)>,
    snip_name: String,
    snip_data: String,
    rename: Option<(i64, String)>,
    label_edit: Option<(i64, String)>,
    label_manager_open: bool,
    labels: Vec<String>,
    label_selected: String,
    label_new: String,
    label_delete_confirm: bool,
    confirm_delete: bool,
    #[cfg(any(windows, target_os = "macos"))]
    paste_target: Option<isize>,
    // Global show/hide toggle (SPEC "ui": global-hotkey + tray-icon).
    gh_manager: Option<global_hotkey::GlobalHotKeyManager>,
    gh_show: Option<global_hotkey::hotkey::HotKey>,
    window_visible: bool,
    #[cfg(any(windows, target_os = "linux", target_os = "macos"))]
    tray: Option<tray_icon::TrayIcon>,
    #[cfg(any(windows, target_os = "linux", target_os = "macos"))]
    tray_mode_id: tray_icon::menu::MenuId,
}

impl App {
    fn new(
        store: Arc<Store>,
        cfg: &Config,
        capture_active: Arc<Mutex<bool>>,
        capture_status: std::sync::mpsc::Receiver<String>,
        update_result: Option<std::sync::mpsc::Receiver<anyhow::Result<Option<String>>>>,
    ) -> Self {
        App {
            mode: parse_mode(&cfg.paste.default_mode),
            pending_paste: None,
            store,
            capture_active,
            capture_status,
            clear_on_exit: cfg.privacy.clear_on_exit,
            update_result,
            tab: Tab::History,
            query: String::new(),
            search_focus: false,
            select_after_show: false,
            starred_only: false,
            kind_sel: 0,
            date_from: String::new(),
            date_to: String::new(),
            rows: Vec::new(),
            selected: None,
            dirty: true,
            last_poll: Instant::now(),
            status: if cfg
                .capture
                .exclude_apps
                .iter()
                .any(|app| !app.trim().is_empty())
                && std::env::var_os("WAYLAND_DISPLAY").is_some()
            {
                "App exclusions enabled; Wayland cannot identify sources, so clipboard capture is skipped.".into()
            } else {
                String::new()
            },
            snips: Vec::new(),
            snip_path: Vec::new(),
            snip_name: String::new(),
            snip_data: String::new(),
            rename: None,
            label_edit: None,
            label_manager_open: false,
            labels: Vec::new(),
            label_selected: String::new(),
            label_new: String::new(),
            label_delete_confirm: false,
            confirm_delete: false,
            #[cfg(any(windows, target_os = "macos"))]
            paste_target: None,
            gh_manager: None,
            gh_show: None,
            window_visible: true,
            #[cfg(any(windows, target_os = "linux", target_os = "macos"))]
            tray: None,
            #[cfg(any(windows, target_os = "linux", target_os = "macos"))]
            tray_mode_id: tray_icon::menu::MenuId::new("clipline-paste-mode"),
        }
    }

    /// Global show/hide hotkey (spec: failure warns, app continues).
    /// Registration reuses `parse_hotkey` for validation, then the crate's own
    /// `FromStr` (same semantics: `Ctrl+Shift+Up`) for the actual binding.
    fn register_show_hotkey(&mut self, hotkey_str: &str) {
        if parse_hotkey(hotkey_str).is_none() {
            return;
        }
        let hk: global_hotkey::hotkey::HotKey = match hotkey_str.parse() {
            Ok(hk) => hk,
            Err(e) => {
                eprintln!("hotkey: parse {hotkey_str:?}: {e}");
                return;
            }
        };
        let manager = match global_hotkey::GlobalHotKeyManager::new() {
            Ok(m) => m,
            Err(e) => {
                eprintln!("hotkey: manager init failed: {e}");
                return;
            }
        };
        match manager.register(hk) {
            Ok(_) => {
                self.gh_show = Some(hk);
                self.gh_manager = Some(manager);
            }
            Err(e) => eprintln!("hotkey: register {hotkey_str:?} failed: {e}"),
        }
    }

    /// Build a tray menu on the UI thread. Linux uses the StatusNotifier protocol.
    #[cfg(any(windows, target_os = "linux", target_os = "macos"))]
    fn init_tray(&mut self) {
        use tray_icon::menu::{Menu, MenuItem};
        let rgba = [120u8, 30, 180, 255];
        let mut px = Vec::with_capacity(32 * 32 * 4);
        for _ in 0..32 * 32 {
            px.extend_from_slice(&rgba);
        }
        let icon = tray_icon::Icon::from_rgba(px, 32, 32).expect("32x32 RGBA valid");

        let show = MenuItem::with_id("clipline-show", "Show clipLine", true, None);
        let mode_item = MenuItem::with_id(
            self.tray_mode_id.clone(),
            tray_mode_label(self.mode),
            !cfg!(target_os = "linux"),
            None,
        );
        let quit = MenuItem::with_id("clipline-quit", "Quit", true, None);
        let menu = Menu::new();
        let _ = menu.append_items(&[&show, &mode_item, &quit]);

        match tray_icon::TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("clipLine")
            .with_icon(icon)
            .build()
        {
            Ok(tray) => self.tray = Some(tray),
            Err(e) => {
                self.status = format!("Tray unavailable: {e}");
                eprintln!("tray: init failed: {e}");
            }
        }
    }

    /// Show/hide toggle shared by hotkey, tray menu and tray click.
    fn toggle_visible(&mut self, ctx: &Context) {
        self.pending_paste = None;
        if self.window_visible {
            ctx.send_viewport_cmd(ViewportCommand::Visible(false));
            self.window_visible = false;
        } else {
            #[cfg(windows)]
            {
                self.paste_target = clip_line_win::foreground_target();
            }
            #[cfg(target_os = "macos")]
            {
                self.paste_target = clip_line_macos::foreground_target();
            }
            ctx.send_viewport_cmd(ViewportCommand::Decorations(false));
            ctx.send_viewport_cmd(ViewportCommand::WindowLevel(egui::WindowLevel::AlwaysOnTop));
            #[cfg(windows)]
            if let Some((x, y)) = clip_line_win::caret_position() {
                let scale = ctx.pixels_per_point();
                ctx.send_viewport_cmd(ViewportCommand::OuterPosition(egui::pos2(
                    x / scale,
                    y / scale,
                )));
            }
            ctx.send_viewport_cmd(ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(ViewportCommand::Focus);
            self.select_search();
            self.window_visible = true;
        }
    }

    fn show_main(&mut self, ctx: &Context) {
        self.pending_paste = None;
        #[cfg(windows)]
        if !self.window_visible {
            self.paste_target = clip_line_win::foreground_target();
        }
        #[cfg(target_os = "macos")]
        if !self.window_visible {
            self.paste_target = clip_line_macos::foreground_target();
        }
        ctx.send_viewport_cmd(ViewportCommand::Decorations(true));
        ctx.send_viewport_cmd(ViewportCommand::WindowLevel(egui::WindowLevel::Normal));
        ctx.send_viewport_cmd(ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(ViewportCommand::Focus);
        self.select_search();
        self.window_visible = true;
    }

    /// Drain global hotkey and tray event channels.
    fn handle_global_events(&mut self, ctx: &Context) {
        use global_hotkey::GlobalHotKeyEvent;
        if let Some(hk) = self.gh_show {
            while let Ok(ev) = GlobalHotKeyEvent::receiver().try_recv() {
                if ev.state == global_hotkey::HotKeyState::Pressed && ev.id() == hk.id() {
                    self.toggle_visible(ctx);
                }
            }
        }
        #[cfg(any(windows, target_os = "linux", target_os = "macos"))]
        if self.tray.is_some() {
            use tray_icon::menu::MenuEvent;
            while let Ok(ev) = MenuEvent::receiver().try_recv() {
                match ev.id().as_ref() {
                    "clipline-show" => self.show_main(ctx),
                    "clipline-paste-mode" => {
                        self.mode = next_mode(self.mode);
                        if let Some(tray) = &self.tray {
                            tray.set_menu(Some(Box::new(Self::tray_menu(self.mode))));
                        }
                    }
                    "clipline-quit" => ctx.send_viewport_cmd(ViewportCommand::Close),
                    _ => {}
                }
            }
            while let Ok(ev) = tray_icon::TrayIconEvent::receiver().try_recv() {
                if let tray_icon::TrayIconEvent::Click { button, .. } = ev
                    && button == tray_icon::MouseButton::Left
                {
                    self.toggle_visible(ctx);
                }
            }
        }
    }

    #[cfg(any(windows, target_os = "linux", target_os = "macos"))]
    fn tray_menu(mode: PasteMode) -> tray_icon::menu::Menu {
        use tray_icon::menu::{Menu, MenuId, MenuItem};
        let show = MenuItem::with_id("clipline-show", "Show clipLine", true, None);
        let mode_item = MenuItem::with_id(
            MenuId::new("clipline-paste-mode"),
            tray_mode_label(mode),
            !cfg!(target_os = "linux"),
            None,
        );
        let quit = MenuItem::with_id("clipline-quit", "Quit", true, None);
        let menu = Menu::new();
        let _ = menu.append_items(&[&show, &mode_item, &quit]);
        menu
    }

    /// Select the search field: focus it and select its text (hotkey show).
    fn select_search(&mut self) {
        self.search_focus = false;
        self.select_after_show = true;
    }

    fn refresh_if_needed(&mut self) {
        if self.dirty || self.last_poll.elapsed() >= Duration::from_secs(1) {
            self.last_poll = Instant::now();
            self.dirty = false;
            let (from, to) = date_range_strings(&self.date_from, &self.date_to);
            let f = Filter {
                query: (!self.query.trim().is_empty()).then(|| self.query.trim().to_string()),
                starred_only: self.starred_only,
                kind: match self.kind_sel {
                    1 => Some(1), // text
                    2 => Some(3), // url
                    3 => Some(4), // files
                    4 => Some(5), // image
                    _ => None,
                },
                from,
                to,
                limit: 500,
                ..Default::default()
            };
            match self.store.list(&f) {
                Ok(rows) => {
                    if self.selected.is_some_and(|s| s >= rows.len()) {
                        self.selected = rows.len().checked_sub(1);
                    }
                    self.rows = rows;
                }
                Err(e) => self.status = format!("list: {e}"),
            }
            if self.tab == Tab::Snippets {
                self.reload_snips();
            }
        }
    }

    fn reload_snips(&mut self) {
        match self
            .store
            .snippet_list(self.snip_path.last().map_or(0, |(id, _)| *id))
        {
            Ok(v) => self.snips = v,
            Err(e) => self.status = format!("snippets: {e}"),
        }
    }

    fn handle_keys(&mut self, ctx: &Context) {
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            ctx.send_viewport_cmd(ViewportCommand::Visible(false));
            self.window_visible = false;
            return;
        }
        if self.tab == Tab::History {
            let keys = [
                Key::Num1,
                Key::Num2,
                Key::Num3,
                Key::Num4,
                Key::Num5,
                Key::Num6,
                Key::Num7,
                Key::Num8,
                Key::Num9,
            ];
            let quick_pick = !self.search_focus || ctx.input(|i| i.modifiers.alt);
            if quick_pick
                && let Some(idx) = keys
                    .iter()
                    .position(|key| ctx.input(|i| i.key_pressed(*key)))
            {
                if idx < self.rows.len() {
                    self.selected = Some(idx);
                    self.paste_selected(ctx);
                }
                return;
            }
        } else {
            return;
        }
        if self.search_focus {
            if ctx.input(|i| i.key_pressed(Key::Enter)) {
                self.paste_selected(ctx);
            }
            return;
        }
        let mut delta = 0isize;
        ctx.input(|i| {
            if i.key_pressed(Key::ArrowDown) {
                delta = 1;
            }
            if i.key_pressed(Key::ArrowUp) {
                delta = -1;
            }
        });
        if delta != 0 && !self.rows.is_empty() {
            let max = (self.rows.len() - 1) as isize;
            let cur = self.selected.map(|s| s as isize).unwrap_or(-1) + delta;
            self.selected = Some(cur.clamp(0, max) as usize);
        }
        if ctx.input(|i| i.key_pressed(Key::Enter)) {
            self.paste_selected(ctx);
        }
    }

    fn paste_selected(&mut self, ctx: &Context) {
        let idx = self.selected.unwrap_or(0);
        if let Some(r) = self.rows.get(idx) {
            let id = r.id;
            let mode = self.mode;
            self.paste_row(ctx, id, mode, false);
        }
    }

    fn paste_row(&mut self, ctx: &Context, id: i64, mode: PasteMode, plain: bool) {
        match self.store.load_data(id) {
            Ok(mut blocks) => {
                if plain {
                    blocks.retain(|b| b.format == clip_line_core::Format::UnicodeText);
                    if blocks.is_empty() {
                        self.status = "no text block in this clip".into();
                        return;
                    }
                }
                self.paste_blocks(ctx, blocks, mode);
            }
            Err(e) => self.status = format!("paste: {e}"),
        }
    }

    #[allow(unused_mut)]
    fn paste_blocks(
        &mut self,
        ctx: &Context,
        blocks: Vec<clip_line_core::capture::Block>,
        mut mode: PasteMode,
    ) {
        // Wayland deliberately uses manual paste; keep the clipboard write
        // useful even when the configured default asks for input injection.
        #[cfg(target_os = "linux")]
        if mode != PasteMode::SetOnly {
            mode = PasteMode::SetOnly;
            self.status = "Copied to clipboard. Paste manually on Linux.".into();
        }
        if mode == PasteMode::SetOnly {
            if let Err(e) = Self::do_paste(blocks, mode) {
                self.status = format!("paste: {e}");
            }
            return;
        }
        #[cfg(any(windows, target_os = "macos"))]
        if self.paste_target.is_none() {
            self.status =
                "Open clipLine with the global hotkey to choose a paste target, or use Copy only"
                    .into();
            return;
        }
        // Viewport commands are processed after this frame. Wait for the hide
        // before restoring the target and sending input, then allow focus to
        // settle (macOS activation is asynchronous).
        ctx.send_viewport_cmd(ViewportCommand::Visible(false));
        self.window_visible = false;
        self.pending_paste = Some((
            Instant::now() + Duration::from_millis(100),
            blocks,
            mode,
            false,
        ));
        ctx.request_repaint_after(Duration::from_millis(100));
    }

    fn do_paste(
        blocks: Vec<clip_line_core::capture::Block>,
        mode: PasteMode,
    ) -> anyhow::Result<()> {
        #[cfg(windows)]
        {
            use clip_line_core::paste::Paster;
            clip_line_win::WinPaster.paste(&ClipData { blocks }, mode)
        }
        #[cfg(target_os = "linux")]
        {
            use clip_line_core::paste::Paster;
            if std::env::var_os("WAYLAND_DISPLAY").is_some() {
                clip_line_wayland::WaylandPaster::new()?.paste(&ClipData { blocks }, mode)
            } else {
                clip_line_x11::X11Paster::new()?.paste(&ClipData { blocks }, mode)
            }
        }
        #[cfg(target_os = "macos")]
        {
            use clip_line_core::paste::Paster;
            clip_line_macos::MacPaster.paste(&ClipData { blocks }, mode)
        }
        #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
        {
            let _ = (mode, blocks);
            anyhow::bail!("paste is unsupported on this platform")
        }
    }

    fn history_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let n = self.rows.len();
        let selected = self.selected;
        TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .column(Column::exact(24.0)) // star
            .column(Column::exact(140.0)) // created
            .column(Column::exact(56.0)) // kind
            .column(Column::remainder()) // title
            .column(Column::exact(90.0)) // app
            .body(|body| {
                body.rows(20.0, n, |mut row| {
                    let idx = row.index();
                    let Some(r0) = self.rows.get(idx) else { return };
                    let r = r0.clone(); // visible rows only: cheap
                    row.set_selected(selected == Some(idx));
                    row.col(|ui| {
                        let star = if r.starred { "\u{2605}" } else { "\u{2606}" };
                        if ui
                            .add(egui::Label::new(star).sense(egui::Sense::click()))
                            .clicked()
                        {
                            let _ = self.store.set_star(r.id, !r.starred);
                            self.dirty = true;
                        }
                    });
                    row.col(|ui| {
                        ui.label(&r.created);
                    });
                    row.col(|ui| {
                        ui.label(kind_name(r.kind));
                    });
                    row.col(|ui| {
                        if idx < 9 {
                            ui.label(format!("{}  {}", idx + 1, r.title));
                        } else {
                            ui.label(&r.title);
                        }
                    });
                    row.col(|ui| {
                        ui.label(&r.appname);
                    });
                    let resp = row.response();
                    if resp.clicked() {
                        self.selected = Some(idx);
                        let mode = self.mode;
                        self.paste_row(&ctx, r.id, mode, false);
                    }
                    resp.context_menu(|ui| {
                        let mode = self.mode;
                        if ui.button(t("Paste")).clicked() {
                            self.paste_row(ui.ctx(), r.id, mode, false);
                            ui.close();
                        }
                        if ui.button(t("Paste plain text")).clicked() {
                            self.paste_row(ui.ctx(), r.id, mode, true);
                            ui.close();
                        }
                        if ui.button(t("Copy only")).clicked() {
                            self.paste_row(ui.ctx(), r.id, PasteMode::SetOnly, false);
                            ui.close();
                        }
                        if ui.button(t("Type in")).clicked() {
                            self.paste_row(ui.ctx(), r.id, PasteMode::TypeIn, false);
                            ui.close();
                        }
                        if ui
                            .button(if r.starred { t("Unstar") } else { t("Star") })
                            .clicked()
                        {
                            let _ = self.store.set_star(r.id, !r.starred);
                            self.dirty = true;
                            ui.close();
                        }
                        if ui.button(t("Edit tags…")).clicked() {
                            self.label_edit =
                                Some((r.id, r.tags.trim_matches(',').replace(',', ", ")));
                            ui.close();
                        }
                        if ui.button(t("Delete")).clicked() {
                            let _ = self.store.delete(&[r.id]);
                            self.dirty = true;
                            ui.close();
                        }
                    });
                });
            });
    }

    fn snippets_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button("Root").clicked() {
                self.snip_path.clear();
                self.reload_snips();
            }
            if !self.snip_path.is_empty() && ui.button("Up").clicked() {
                self.snip_path.pop();
                self.reload_snips();
            }
            for (_, name) in &self.snip_path {
                ui.label(format!("/ {name}"));
            }
        });
        ui.horizontal(|ui| {
            ui.label(t("Name"));
            ui.text_edit_singleline(&mut self.snip_name);
            ui.label(t("Text"));
            ui.text_edit_singleline(&mut self.snip_data);
            let pid = self.snip_path.last().map_or(0, |(id, _)| *id);
            if ui.button(t("Add snippet")).clicked() && !self.snip_name.trim().is_empty() {
                match self
                    .store
                    .snippet_add(pid, self.snip_name.trim(), &self.snip_data)
                {
                    Ok(_) => {
                        self.snip_name.clear();
                        self.snip_data.clear();
                        self.reload_snips();
                    }
                    Err(e) => self.status = format!("snippet add: {e}"),
                }
            }
            if ui.button("Add folder").clicked() && !self.snip_name.trim().is_empty() {
                match self.store.snippet_add_folder(pid, self.snip_name.trim()) {
                    Ok(_) => {
                        self.snip_name.clear();
                        self.reload_snips();
                    }
                    Err(e) => self.status = format!("folder add: {e}"),
                }
            }
        });
        ui.separator();
        let n = self.snips.len();
        TableBuilder::new(ui)
            .striped(true)
            .column(Column::exact(180.0))
            .column(Column::remainder())
            .column(Column::exact(65.0))
            .column(Column::exact(65.0))
            .column(Column::exact(65.0))
            .body(|body| {
                body.rows(24.0, n, |mut row| {
                    let Some(s) = self.snips.get(row.index()).cloned() else {
                        return;
                    };
                    row.col(|ui| {
                        if self.rename.as_ref().is_some_and(|(id, _)| *id == s.id) {
                            let Some((_, buf)) = self.rename.as_mut() else {
                                return;
                            };
                            let r = ui.add(egui::TextEdit::singleline(buf).desired_width(160.0));
                            if r.lost_focus() && !buf.trim().is_empty() {
                                let name = self.rename.take().unwrap().1;
                                if let Err(e) = self.store.snippet_rename(s.id, name.trim()) {
                                    self.status = format!("snippet rename: {e}");
                                }
                                self.reload_snips();
                            }
                        } else {
                            ui.label(&s.name);
                        }
                    });
                    row.col(|ui| {
                        ui.label(&s.data);
                    });
                    row.col(|ui| {
                        if s.kind == 1 {
                            if ui.button("Open").clicked() {
                                self.snip_path.push((s.id, s.name.clone()));
                                self.reload_snips();
                            }
                        } else if ui.button(t("Paste")).clicked() {
                            let bytes = s.data.encode_utf16().flat_map(u16::to_le_bytes).collect();
                            self.paste_blocks(
                                ui.ctx(),
                                vec![clip_line_core::capture::Block {
                                    format: clip_line_core::Format::UnicodeText,
                                    bytes,
                                }],
                                self.mode,
                            );
                        }
                    });
                    row.col(|ui| {
                        if ui.button(t("Rename")).clicked() {
                            self.rename = Some((s.id, s.name.clone()));
                        }
                    });
                    row.col(|ui| {
                        if ui.button(t("Delete")).clicked() {
                            if let Err(e) = self.store.snippet_delete(s.id) {
                                self.status = format!("snippet delete: {e}");
                            }
                            self.reload_snips();
                        }
                    });
                })
            });
    }
}

impl Drop for App {
    fn drop(&mut self) {
        // Hold the same lock used by capture through clearing, so a pending
        // event cannot repopulate history after the user requested deletion.
        let mut active = self.capture_active.lock().unwrap();
        *active = false;
        if self.clear_on_exit
            && let Err(e) = self
                .store
                .clear_history()
                .and_then(|_| self.store.checkpoint_truncate())
        {
            eprintln!("clear history on exit: {e:#}");
        }
    }
}

impl eframe::App for App {
    /// Runs even while the window is hidden (0.37 trait) — keeps history fresh.
    fn logic(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        while let Ok(message) = self.capture_status.try_recv() {
            self.status = message;
        }
        if let Some(rx) = &self.update_result {
            match rx.try_recv() {
                Ok(Ok(Some(tag))) => {
                    self.status = format!(
                        "Update {tag} available: https://github.com/katsugtgz/clip-line/releases"
                    );
                    self.update_result = None;
                }
                Ok(Ok(None)) => self.update_result = None,
                Ok(Err(e)) => {
                    eprintln!("update check: {e:#}");
                    self.update_result = None;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.update_result = None,
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
        }
        if self
            .pending_paste
            .as_ref()
            .is_some_and(|(when, _, _, _)| Instant::now() >= *when)
        {
            let (_, blocks, mode, activated) = self.pending_paste.take().unwrap();
            if !activated {
                #[cfg(windows)]
                let activation = self
                    .paste_target
                    .map(clip_line_win::activate_target)
                    .unwrap_or_else(|| Err(anyhow::anyhow!("paste target unavailable")));
                #[cfg(target_os = "macos")]
                let activation = self
                    .paste_target
                    .map(clip_line_macos::activate_target)
                    .unwrap_or_else(|| Err(anyhow::anyhow!("paste target unavailable")));
                #[cfg(not(any(windows, target_os = "macos")))]
                let activation: anyhow::Result<()> = Ok(());
                match activation {
                    Ok(()) => {
                        self.pending_paste = Some((
                            Instant::now() + Duration::from_millis(150),
                            blocks,
                            mode,
                            true,
                        ))
                    }
                    Err(e) => {
                        self.status = format!("cannot restore paste target: {e}");
                        self.show_main(ctx);
                    }
                }
            } else {
                #[cfg(windows)]
                let target_matches = clip_line_win::foreground_target() == self.paste_target;
                #[cfg(target_os = "macos")]
                let target_matches = clip_line_macos::foreground_target() == self.paste_target;
                #[cfg(not(any(windows, target_os = "macos")))]
                let target_matches = true;
                if !target_matches {
                    self.status = "Paste cancelled because the target window changed".into();
                    self.show_main(ctx);
                } else if let Err(e) = Self::do_paste(blocks, mode) {
                    self.status = format!("paste: {e}");
                    self.show_main(ctx);
                }
            }
        }
        self.refresh_if_needed();
        self.handle_global_events(ctx);
        self.handle_keys(ctx);
        ctx.request_repaint_after(Duration::from_millis(100));
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.search_focus = false;
        egui::Panel::top("tabs").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tab, Tab::History, t("History"));
                ui.selectable_value(&mut self.tab, Tab::Snippets, t("Snippets"));
            });
        });
        if self.tab == Tab::History {
            egui::Panel::top("filter").show(ui, |ui| {
                ui.horizontal(|ui| {
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut self.query)
                            .hint_text(t("Search"))
                            .desired_width(220.0),
                    );
                    if self.select_after_show {
                        // ponytail: select-all, not just focus — needs a cursor-state
                        // edit via egui::text_edit; plain focus is enough for v1.
                        r.request_focus();
                        self.select_after_show = false;
                    }
                    self.search_focus = r.has_focus();
                    if r.changed() {
                        self.dirty = true;
                    }
                    if ui.checkbox(&mut self.starred_only, t("Starred")).changed() {
                        self.dirty = true;
                    }
                    let kinds = [t("All"), t("Text"), t("Url"), t("Files"), t("Image")];
                    let cr = egui::ComboBox::from_id_salt("kind")
                        .selected_text(kinds[self.kind_sel.min(4)].as_str())
                        .show_ui(ui, |ui| {
                            for (i, k) in kinds.iter().enumerate() {
                                ui.selectable_value(&mut self.kind_sel, i, k.as_str());
                            }
                        });
                    if cr.response.changed() {
                        self.dirty = true;
                    }
                    if ui.button(t("Delete range…")).clicked() {
                        self.confirm_delete = true;
                    }
                    for (buf, hint) in [
                        (&mut self.date_from, t("From YYYY-MM-DD")),
                        (&mut self.date_to, t("To YYYY-MM-DD")),
                    ] {
                        let r = ui.add(
                            egui::TextEdit::singleline(buf)
                                .hint_text(hint)
                                .desired_width(110.0),
                        );
                        if r.changed() {
                            self.dirty = true;
                        }
                    }
                });
            });
        }
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(format!("{} clips", self.rows.len()));
                ui.separator();
                ui.label(self.status.as_str());
            });
        });
        if self.label_manager_open {
            let mut open = true;
            let mut rename = false;
            let mut delete = false;
            let previous_label = self.label_selected.clone();
            egui::Window::new("Manage tags")
                .open(&mut open)
                .collapsible(false)
                .show(ui.ctx(), |ui| {
                    egui::ComboBox::from_id_salt("labels-manager")
                        .selected_text(if self.label_selected.is_empty() {
                            "Select a tag"
                        } else {
                            &self.label_selected
                        })
                        .show_ui(ui, |ui| {
                            for label in &self.labels {
                                ui.selectable_value(
                                    &mut self.label_selected,
                                    label.clone(),
                                    label.as_str(),
                                );
                            }
                        });
                    ui.horizontal(|ui| {
                        ui.label("New name");
                        ui.text_edit_singleline(&mut self.label_new);
                    });
                    rename = ui.button("Rename tag everywhere").clicked();
                    ui.checkbox(
                        &mut self.label_delete_confirm,
                        "Confirm removing this tag from all clips",
                    );
                    delete = ui
                        .add_enabled(
                            self.label_delete_confirm,
                            egui::Button::new("Delete tag everywhere"),
                        )
                        .clicked();
                });
            self.label_manager_open = open;
            if self.label_selected != previous_label {
                self.label_delete_confirm = false;
                delete = false;
            }
            if rename || delete {
                let result = if self.label_selected.is_empty() {
                    Err(anyhow::anyhow!("select a tag first"))
                } else if rename {
                    self.store
                        .rename_tag(&self.label_selected, self.label_new.trim())
                } else {
                    self.store.delete_tag(&self.label_selected)
                };
                match result {
                    Ok(()) => {
                        self.label_selected.clear();
                        self.label_new.clear();
                        self.label_delete_confirm = false;
                        match self.store.list_all_tags() {
                            Ok(labels) => self.labels = labels,
                            Err(e) => self.status = format!("tags: {e}"),
                        }
                        self.dirty = true;
                    }
                    Err(e) => self.status = format!("tags: {e}"),
                }
            }
        }
        if self.label_edit.is_some() {
            let mut save = false;
            let mut cancel = false;
            egui::Window::new("Edit tags")
                .collapsible(false)
                .show(ui.ctx(), |ui| {
                    if let Some((_, tags)) = &mut self.label_edit {
                        ui.label("Comma-separated tags");
                        ui.text_edit_singleline(tags);
                        ui.horizontal(|ui| {
                            save = ui.button("Save").clicked();
                            cancel = ui.button("Cancel").clicked();
                        });
                    }
                });
            if save {
                if let Some((id, tags)) = self.label_edit.take() {
                    let tags: Vec<String> = tags
                        .split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                        .collect();
                    if let Err(e) = self.store.set_tags(id, &tags) {
                        self.status = format!("tags: {e}");
                    }
                    self.dirty = true;
                }
            } else if cancel {
                self.label_edit = None;
            }
        }
        if self.confirm_delete {
            let mut confirm = false;
            let mut cancel = false;
            egui::Window::new("Delete history").collapsible(false).show(ui.ctx(), |ui| {
                ui.label("Delete all clips in the selected date range? Blank dates include all history.");
                ui.horizontal(|ui| { confirm = ui.button("Delete").clicked(); cancel = ui.button("Cancel").clicked(); });
            });
            if confirm {
                let (from, to) = date_range_strings(&self.date_from, &self.date_to);
                if (!self.date_from.trim().is_empty() && from.is_none())
                    || (!self.date_to.trim().is_empty() && to.is_none())
                {
                    self.status = "Invalid date; history was not deleted".into();
                } else {
                    let from = from.as_deref().unwrap_or("0000-01-01 00:00:00");
                    let to = to.as_deref().unwrap_or("9999-12-31 23:59:59");
                    if from > to {
                        self.status = "Start date must be before end date".into();
                    } else if let Err(e) = self.store.delete_range(from, to) {
                        self.status = format!("delete: {e}");
                    } else {
                        self.dirty = true;
                    }
                }
            }
            if confirm || cancel {
                self.confirm_delete = false;
            }
        }
        egui::CentralPanel::default().show(ui, |ui| match self.tab {
            Tab::History => self.history_ui(ui),
            Tab::Snippets => self.snippets_ui(ui),
        });
    }
}

fn kind_name(k: i64) -> &'static str {
    match k {
        0 => "image*",
        1 => "text",
        2 => "text2",
        3 => "url",
        4 => "files",
        5 => "image",
        _ => "?",
    }
}
