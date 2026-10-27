//! clipLine desktop app — SPEC.md "ui" section, phase-2 slice.
//! Single main window; hide-after-paste. No tray/global-hotkey/popup viewport yet
//! (TODO(spec ui): phase 2 adds the caret-positioned popup viewport, tray-icon and
//! global-hotkey toggle; v1 interaction = main window + hide after paste is accepted).
//! TODO(phase 4, Q11): GitHub Releases update check.

use clip_line_core::config::Config;
use clip_line_core::paste::{ClipData, PasteMode};
use clip_line_core::store::{ClipRow, Filter, Snippet, Store, StoreOpts};
use eframe::egui;
use egui::{Context, Key, ViewportCommand};
use egui_extras::{Column, TableBuilder};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Self-paste suppression slot: the UI thread stores the payload it is about to
/// write to the clipboard BEFORE writing; the capture thread skips the event
/// whose packed payload matches, then clears the slot. Payload identity is
/// race-free where sequence-number arming is not (the WM_CLIPBOARDUPDATE lands
/// before a cross-thread arm could). `envelope::pack` is deterministic.
/// ponytail: replace with `WinCapture` sequence suppression when the win crate
/// grows a shared ignore slot.
type SuppressSlot = Arc<Mutex<Option<Vec<u8>>>>;

/// ponytail: i18n scaffold — identity fn, English inline; swap for rust-i18n
/// `t!` when locales arrive (mechanical rename).
pub fn t(s: &str) -> &str {
    s
}

const DEFAULT_TOML: &str = r#"# clipLine config — see SPEC.md. Regenerated if deleted.
[hotkeys]
show = "Ctrl+Shift+V"        # TODO(phase 2): global-hotkey registration

[paste]
default_mode = "inject_ctrl_v"   # inject_ctrl_v | set_only | type_in

[ui]
hide_after_ms = 1200

[autostart]
enabled = false

# [storage]
# path = "C:\path\to\clipline.db"
"#;

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
struct Hotkeys {
    show: String,
}

impl Hotkeys {
    fn with_default_show() -> Self {
        Self {
            show: "Ctrl+Shift+V".into(),
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
#[derive(Debug)]
struct Extras {
    hotkeys: Hotkeys,
    autostart: Autostart,
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
    let hotkeys = toml::from_str::<Hotkeys>(&text).unwrap_or_else(|_| Hotkeys::with_default_show());
    let autostart = toml::from_str::<Autostart>(&text).unwrap_or_default();
    Ok((
        toml::from_str(&text)?,
        Extras { hotkeys, autostart },
    ))
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
                    None
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

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let mut import_ini = None;
    while let Some(a) = args.next() {
        if a == "--import-ini" {
            import_ini = args.next().map(PathBuf::from);
        }
    }
    if let Some(ini) = import_ini {
        let hks = clip_line_core::config::import_legacy_ini(&ini)?;
        if hks.is_empty() {
            println!("no hotkeys imported from {}", ini.display());
        }
        for hk in hks {
            // spec "Open": verify the modifier/key bitfield decodes back cleanly
            let ok = parse_hotkey(&hk.display)
                .map(|(m, v)| m == hk.modifier && v == hk.vkey)
                .unwrap_or(false);
            println!(
                "slot {}: {} (modifier={}, vkey={}) {}",
                hk.slot,
                hk.display,
                hk.modifier,
                hk.vkey,
                if ok { "verified" } else { "UNVERIFIED" }
            );
        }
        return Ok(());
    }

    let cfg_path = default_config_path()?;
    let (cfg, extras) = load_config(&cfg_path)?;
    if parse_hotkey(&extras.hotkeys.show).is_none() {
        eprintln!(
            "config: hotkeys.show {:?} not understood; global hotkeys land in phase 2",
            extras.hotkeys.show
        );
    }

    let db = db_path(cfg.storage.path.as_deref())?;
    let store = Arc::new(Store::open(&db, StoreOpts::default())?);
    if let Err(e) = store.attach_snippets(&db.with_file_name("snippets.sdb")) {
        eprintln!("snippets disabled: {e}");
    }

    #[cfg(windows)]
    if let Err(e) = set_autostart(extras.autostart.enabled) {
        eprintln!("autostart: {e}");
    }
    // TODO(spec Q15): macOS Login Items / XDG autostart .desktop land with the
    // phase-3 non-Windows adapters.

    let suppress: SuppressSlot = Arc::new(Mutex::new(None));
    spawn_capture(store.clone(), suppress.clone())?;

    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([860.0, 600.0])
            .with_min_inner_size([640.0, 400.0]),
        ..Default::default()
    };
    eframe::run_native(
        "clipLine",
        opts,
        Box::new(move |_cc| Ok(Box::new(App::new(store, &cfg, suppress)))),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))
}

/// Registry Run key write/remove per `[autostart] enabled` at startup.
#[cfg(windows)]
fn set_autostart(enabled: bool) -> anyhow::Result<()> {
    use windows::Win32::System::Registry::{
        RegDeleteKeyValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ,
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

#[cfg(windows)]
fn spawn_capture(store: Arc<Store>, suppress: SuppressSlot) -> anyhow::Result<()> {
    std::thread::Builder::new()
        .name("capture".into())
        .spawn(move || {
            let mut cap = match clip_line_win::WinCapture::new() {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("capture disabled: {e}");
                    return;
                }
            };
            use clip_line_core::capture::Capture;
            loop {
                match cap.next_event() {
                    Ok(ev) => {
                        // Our own paste write: payload was armed before the
                        // clipboard write; skip and clear the slot.
                        let mine = clip_line_core::envelope::pack(&ev.blocks)
                            .ok()
                            .and_then(|p| {
                                let mut g = suppress.lock().unwrap();
                                match &*g {
                                    Some(want) if *want == p => {
                                        *g = None;
                                        Some(())
                                    }
                                    _ => None,
                                }
                            });
                        if mine.is_some() {
                            continue;
                        }
                        if let Err(e) = store.append(&ev) {
                            eprintln!("store append: {e}");
                        }
                    }
                    Err(e) => {
                        eprintln!("capture error: {e}");
                        std::thread::sleep(Duration::from_secs(1));
                    }
                }
            }
        })
        .map(|_| ())
        .map_err(|e| anyhow::anyhow!("spawn capture: {e}"))?;
    Ok(())
}

#[cfg(not(windows))]
fn spawn_capture(_store: Arc<Store>, _suppress: SuppressSlot) {
    // ponytail: x11/wayland/macos adapters land in phase 3
}

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    History,
    Snippets,
}

struct App {
    store: Arc<Store>,
    suppress: SuppressSlot,
    mode: PasteMode,
    hide_after_ms: u64,
    tab: Tab,
    query: String,
    search_focus: bool,
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
    snip_name: String,
    snip_data: String,
    rename: Option<(i64, String)>,
}

impl App {
    fn new(store: Arc<Store>, cfg: &Config, suppress: SuppressSlot) -> Self {
        App {
            mode: parse_mode(&cfg.paste.default_mode),
            hide_after_ms: cfg.ui.hide_after_ms,
            store,
            suppress,
            tab: Tab::History,
            query: String::new(),
            search_focus: false,
            starred_only: false,
            kind_sel: 0,
            date_from: String::new(),
            date_to: String::new(),
            rows: Vec::new(),
            selected: None,
            dirty: true,
            last_poll: Instant::now(),
            status: String::new(),
            snips: Vec::new(),
            snip_name: String::new(),
            snip_data: String::new(),
            rename: None,
        }
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
        match self.store.snippet_list(0) {
            Ok(v) => self.snips = v,
            Err(e) => self.status = format!("snippets: {e}"),
        }
    }

    fn handle_keys(&mut self, ctx: &Context) {
        if self.search_focus {
            // typing a query then Enter pastes the top hit — parity with the original app
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
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            // TODO(spec ui): Esc closes the hotkey popup in phase 2; v1 hides the window.
            ctx.send_viewport_cmd(ViewportCommand::Visible(false));
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
        match self.do_paste(id, mode, plain) {
            Ok(()) => {
                if mode != PasteMode::SetOnly {
                    hide_soon(ctx.clone(), self.hide_after_ms);
                }
            }
            Err(e) => self.status = format!("paste: {e}"),
        }
    }

    fn do_paste(&mut self, id: i64, mode: PasteMode, plain: bool) -> anyhow::Result<()> {
        let mut blocks = self.store.load_data(id)?;
        if plain {
            blocks.retain(|b| b.format == clip_line_core::Format::UnicodeText);
            if blocks.is_empty() {
                anyhow::bail!("no text block in this clip");
            }
        }
        // Arm suppression BEFORE the clipboard write — see SuppressSlot.
        let payload = clip_line_core::envelope::pack(&blocks)?;
        *self.suppress.lock().unwrap() = Some(payload);
        #[cfg(windows)]
        {
            use clip_line_core::paste::Paster;
            clip_line_win::WinPaster.paste(&ClipData { blocks }, mode)
        }
        #[cfg(not(windows))]
        {
            let _ = (mode, blocks);
            Err(anyhow::anyhow!("paste adapter not built on this OS (phase 3)"))
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
                        if ui.label(star).clicked() {
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
                        ui.label(&r.title);
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
                            .button(if r.starred {
                                t("Unstar")
                            } else {
                                t("Star")
                            })
                            .clicked()
                        {
                            let _ = self.store.set_star(r.id, !r.starred);
                            self.dirty = true;
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
            ui.label(t("Name"));
            ui.text_edit_singleline(&mut self.snip_name);
            ui.label(t("Text"));
            ui.add(
                egui::TextEdit::singleline(&mut self.snip_data).desired_width(f32::INFINITY),
            );
            if ui.button(t("Add snippet")).clicked() && !self.snip_name.trim().is_empty() {
                match self
                    .store
                    .snippet_add(0, self.snip_name.trim(), self.snip_data.trim())
                {
                    Ok(_) => {
                        self.snip_name.clear();
                        self.snip_data.clear();
                        self.reload_snips();
                    }
                    Err(e) => self.status = format!("snippet add: {e}"),
                }
            }
        });
        ui.separator();
        let n = self.snips.len();
        TableBuilder::new(ui)
            .striped(true)
            .column(Column::exact(180.0))
            .column(Column::remainder())
            .column(Column::exact(80.0))
            .column(Column::exact(80.0))
            .body(|body| {
                body.rows(22.0, n, |mut row| {
                    let idx = row.index();
                    let Some(s0) = self.snips.get(idx) else { return };
                    let s = s0.clone();
                    row.col(|ui| {
                        if self.rename.as_ref().is_some_and(|(id, _)| *id == s.id) {
                            let Some((_, buf)) = self.rename.as_mut() else {
                                return;
                            };
                            let r = ui.add(egui::TextEdit::singleline(buf).desired_width(160.0));
                            if r.lost_focus() && !buf.trim().is_empty() {
                                let name = std::mem::take(&mut self.rename).unwrap().1;
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
                });
            });
    }
}

/// Sleep `ms` then hide the main window (pasting keeps the target window's
/// focus this way). The `Context` clone is safe for cross-thread use.
fn hide_soon(ctx: Context, ms: u64) {
    std::thread::Builder::new()
        .name("hide-timer".into())
        .spawn(move || {
            std::thread::sleep(Duration::from_millis(ms));
            ctx.send_viewport_cmd(ViewportCommand::Visible(false));
        })
        .ok();
}

impl eframe::App for App {
    /// Runs even while the window is hidden (0.37 trait) — keeps history fresh.
    fn logic(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        self.refresh_if_needed();
        self.handle_keys(ctx);
        ctx.request_repaint_after(Duration::from_secs(1));
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
                    self.search_focus = r.has_focus();
                    if r.changed() {
                        self.dirty = true;
                    }
                    if ui.checkbox(&mut self.starred_only, t("Starred")).changed() {
                        self.dirty = true;
                    }
                    let kinds = [t("All"), t("Text"), t("Url"), t("Files"), t("Image")];
                    let cr = egui::ComboBox::from_id_salt("kind")
                        .selected_text(kinds[self.kind_sel.min(4)])
                        .show_ui(ui, |ui| {
                            for (i, k) in kinds.iter().enumerate() {
                                ui.selectable_value(&mut self.kind_sel, i, *k);
                            }
                        });
                    if cr.response.changed() {
                        self.dirty = true;
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
