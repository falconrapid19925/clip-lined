# Changelog

All notable changes to clipLine are documented here. Format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow
[SemVer](https://semver.org/).

## [Unreleased]

### Added
- Core: `Store` opens/creates ClipDiary-compatible SQLite history (WAL),
  append with dedup, list/search/star/delete, legacy database import.
- Core: gzip block envelope matching `docs/interop.md`.
- Windows: clipboard listener adapter (`AddClipboardFormatListener`) with
  live round-trip integration test; clipboard-set + Ctrl+V paste adapter.
- App: egui history window (live list, types, timestamps) and a headless
  demo binary for CI.
- CI: fmt/clippy/test matrix on Windows/Linux/macOS, release builds,
  GitHub Pages showcase.
- Community files: CONTRIBUTING, SECURITY, CODE_OF_CONDUCT, issue and PR
  templates, MIT license, interop documentation.

### Known limitations
- Paste writes text clips only (images/files: phase 2)
- Source app name not yet recorded on Windows events (phase 2)
- No tray icon, global hotkey popup, snippets tab yet (phase 2)
- Linux/macOS capture adapters pending (phase 3)
- Database encryption (SQLCipher) pending (phase 4)
