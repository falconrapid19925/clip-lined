# clipLine

[![CI](https://github.com/clip-line/clip-line/actions/workflows/ci.yml/badge.svg)](../../actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/clip-line.svg)](https://crates.io/crates/clip-line)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](#license)

A fast, open-source clipboard history manager for Windows, Linux and macOS,
written in Rust.

![clipLine screenshot](docs/assets/screenshot.png)

## Features

- **Clipboard history** — every text, URL, image and file-drop clip is captured
  automatically, with the source application recorded
- **Instant search** — incremental search across titles, URLs and source apps
- **Stars & tags** — mark clips you reuse, filter to starred only
- **Paste back** — put a clip on the clipboard, or inject Ctrl+V into the
  window you were working in
- **Reads ClipDiary history** — opens the original `diary.db` in place or
  imports it; both programs can share one history
- **Local first** — single SQLite file, WAL mode, no network, no telemetry
- **Light** — small binary, native widgets

Phase status: capture (Windows), storage, search, list UI live. Paste
injection, snippets tab, tray, hotkey popup, Linux/macOS adapters on the
[roadmap](https://github.com/clip-line/clip-line/milestones).

## Prerequisites

- Rust 1.85+ (rustup recommended)
- On Windows: nothing else — the clipboard listener is built in
- On Linux: X11 or Wayland with `ext-data-control` (GNOME 48+) /
  `wlr-data-control` (KDE, wlroots compositors)
- On macOS: macOS 13+; input injection needs the Accessibility permission

## Install

From source:

```sh
git clone https://github.com/clip-line/clip-line.git
cd clip-line
cargo install --path crates/clip-line-app
```

Prebuilt binaries: see [Releases](../../releases) (Windows x64, Linux x64,
macOS universal).

## Usage

Run the app:

```sh
clip-line-app          # history window
clip-line-headless     # CLI demo: store + list, no GUI
```

The app watches your clipboard and appends new clips to:

- Windows: `%APPDATA%\clip-line\clipline.db`
- Linux: `$XDG_CONFIG_HOME/clip-line/clipline.db`
- macOS: `~/Library/Application Support/clip-line/clipline.db`

Set `CLIP_LINE_DB=/path/to.db` to override.

### Reading an existing ClipDiary history

Point `CLIP_LINE_DB` at (or configure `[storage] path` to) the original
database; clipLine opens it in place and adds to it. Alternatively copy the
files — the format is documented in [`docs/interop.md`](docs/interop.md).

## Configuration

`clipline.toml` next to the database (created with defaults on first run):

```toml
[storage]
path = ""            # default: OS config dir

[paste]
default_mode = "set_only"   # set_only | inject_ctrl_v | type_in

[ui]
hide_after_ms = 1200
```

## Development

```sh
cargo build --workspace          # debug build
cargo test --workspace           # unit + integration tests
cargo run -p clip-line-headless  # GUI-free smoke run
cargo clippy --workspace -- -D warnings
cargo fmt --check
```

`cargo test` on Windows includes a live clipboard round-trip; on headless CI
(Linux container) the store tests run without a display.

## Troubleshooting

- **"capture disabled" in the console** — the clipboard listener could not
  start; another instance may hold it. Kill stale `clip-line-app` processes.
- **Linux Wayland: no events** — your compositor lacks the data-control
  protocol. GNOME needs 48+; otherwise run under XWayland or use the X11
  session.
- **Database locked** — the original ClipDiary is running and holding a write
  lock. Close it, or point clipLine at a copy.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Bug reports and PRs welcome; feature
discussion in [Issues](../../issues).

## License

[MIT](LICENSE) © 2026 clipLine contributors. clipLine is not affiliated with
Softvoile ClipDiary; interoperability notes in `docs/interop.md` are
clean-room format documentation.
