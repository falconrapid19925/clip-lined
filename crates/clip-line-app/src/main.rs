use clip_line_core::store::{Filter, Store, StoreOpts};
use std::sync::Arc;

fn main() -> anyhow::Result<()> {
    let db = default_db_path()?;
    let store = Arc::new(Store::open(&db, StoreOpts::default())?);
    spawn_capture(store.clone());
    let opts = eframe::NativeOptions::default();
    eframe::run_native(
        "clipLine",
        opts,
        Box::new(|_cc| Ok(Box::new(App { store }))),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))
}

fn default_db_path() -> anyhow::Result<std::path::PathBuf> {
    let base = std::env::var_os("CLIP_LINE_DB")
        .map(std::path::PathBuf::from)
        .or_else(dirs_config_home)
        .ok_or_else(|| anyhow::anyhow!("no config dir"))?;
    if base.extension().is_some() {
        return Ok(base);
    }
    std::fs::create_dir_all(&base)?;
    Ok(base.join("clipline.db"))
}

fn dirs_config_home() -> Option<std::path::PathBuf> {
    std::env::var_os("APPDATA")
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME"))
        .map(std::path::PathBuf::from)
        .map(|p| p.join("clip-line"))
}

#[cfg(windows)]
fn spawn_capture(store: Arc<Store>) {
    std::thread::spawn(move || {
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
                    if let Err(e) = store.append(&ev) {
                        eprintln!("store append: {e}");
                    }
                }
                Err(e) => {
                    eprintln!("capture error: {e}");
                    std::thread::sleep(std::time::Duration::from_secs(1));
                }
            }
        }
    });
}

#[cfg(not(windows))]
fn spawn_capture(_store: Arc<Store>) {
    // ponytail: x11/wayland/macos adapters land in phase 3
}

struct App {
    store: Arc<Store>,
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut eframe::egui::Ui, _frame: &mut eframe::Frame) {
        ui.heading("clipLine");
        let rows = self
            .store
            .list(&Filter {
                limit: 500,
                ..Default::default()
            })
            .unwrap_or_default();
        eframe::egui::ScrollArea::vertical().show_rows(ui, 22.0, rows.len(), |ui, range| {
            for r in &rows[range] {
                ui.label(format!("{}  {}  {}", r.created, kind_name(r.kind), r.title));
            }
        });
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(500));
    }
}

fn kind_name(k: i64) -> &'static str {
    match k {
        0 => "img?",
        1 => "text",
        2 => "text2",
        3 => "url",
        4 => "files",
        5 => "image",
        _ => "?",
    }
}
