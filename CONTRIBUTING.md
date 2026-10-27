# Contributing to clipLine

Thanks for your interest! This file tells you how to get a change accepted.

## Development setup

1. Install Rust via [rustup](https://rustup.rs) (1.85+).
2. Fork and clone the repo.
3. `cargo test --workspace` must pass before anything else matters.

## Project map

- `crates/clip-line-core` — storage (SQLite), capture/paste seams, envelope
  format. Deep module; read `docs/interop.md` before touching `store.rs` or
  `envelope.rs`.
- `crates/clip-line-win` — Windows adapters behind `#[cfg(windows)]`.
- `crates/clip-line-app` — egui desktop app + headless demo binary.

`SPEC.md`-derived interface decisions live in the core crate docs; keep the
`Capture`/`Paster` traits stable — adapters vary, callers must not know which.

## What we look for

- Tests at the highest seam: `Store` methods, envelope round-trip, capture
  adapters via the trait. No tests reach into private functions.
- `cargo clippy --workspace -- -D warnings` and `cargo fmt` clean.
- One logical change per PR, described in the imperative mood.
- New deps need a justification line in the PR: what it does, why not stdlib
  or an existing dep.

## Compatibility rule

Anything that writes `log.data` must preserve the envelope documented in
`docs/interop.md` — the original ClipDiary reads the same database. If your
change alters the envelope, open an issue first.

## Reporting bugs

Use the issue templates. Include OS, compositor (Linux), clipLine version
(`clip-line-app --version` output once flags land; for now the release tag),
and console output.
