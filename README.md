# clipLine

clipLine is a Rust clipboard history manager for Windows, Linux and macOS. It stores history locally in SQLite and aims for interoperability with ClipDiary; see [interop notes](docs/interop.md) and the [parity specification](https://github.com/katsugtgz/clip-line/issues/1).

The application provides a history list, search and filters, stars, snippets and paste actions. The parity implementation is still under development: desktop testing and some requirements in issue #1 remain outstanding. Back up an existing ClipDiary database before using it with clipLine; compatibility with all versions and simultaneous use is not established.

## Build and run

Install a recent stable Rust toolchain, then:

```sh
# Windows and macOS
cargo run -p clip-line-app --features encryption

# Linux: install the X11 and Wayland development libraries first
cargo run -p clip-line-app --features x11,wayland,encryption
```

The Linux adapter is selected from the desktop environment. Wayland capture requires a compositor supporting a data-control protocol; paste injection may require external tools. Windows tray and hotkey behavior and platform paste injection should be tested in an actual desktop session.

By default the database is stored under the platform configuration directory. Set `CLIP_LINE_DB` to choose a database. A `clipline.toml` file is created in the configuration directory. Run `clip-line-app --import-db <path>` to import legacy history into the configured database, or `clip-line-app --import-ini <path>` to import a supported legacy hotkey. Not all legacy settings are imported.

For a new SQLCipher database, build with `encryption` and set `CLIP_LINE_DB_KEY` before launching. Use the same key to reopen it. Keep the key safe: losing it makes the database unreadable. This does not convert an existing unencrypted database or unlock legacy sqlite3mc databases.

`[privacy]` supports `secure_delete = true` and `clear_on_exit = true`. Secure deletion is best effort: SQLite cannot erase backups, old copies, SSD remapping, or all data after abnormal termination. Clear on exit runs on normal shutdown.

## Development

```sh
cargo fmt --check
cargo test --workspace
cargo test -p clip-line-core --features encryption
cargo clippy --workspace --all-targets -- -D warnings
```

Desktop capture, paste, tray, hotkey, and interoperability need manual testing on each supported OS. See [CONTRIBUTING.md](CONTRIBUTING.md). Licensed under [MIT](LICENSE).
